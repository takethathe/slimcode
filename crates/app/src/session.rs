//! Session persistence: append-only JSONL session logs at
//! `~/.slimcode/sessions/<project-key>/<id>.jsonl` (ADR-0009).
//!
//! Sessions are partitioned per project: the store is rooted at the sessions
//! directory plus a project key derived from the project home path (git root,
//! falling back to the OS user home), so `/load` and `/sessions` only ever see
//! the current project's sessions. Disk use is bounded two ways: a startup
//! sweep drops the current project's empty logs, and every append enforces a
//! whole-store byte quota by evicting oldest-first down to half the quota
//! (legacy whole-file `.json` sessions still count toward the quota, though
//! they are invisible to load/list/cleanup).
//!
//! A log is a header line plus one typed record per line: the header carries
//! the identity (`type`/`v`/`id`/`created_at`/`project_home`) and is mandatory,
//! then `message` and `title` records follow. Writing is append-only and never
//! validates or fsyncs; reading is lenient — unknown/malformed records are
//! skipped and counted, a torn tail is dropped and sealed, and a dangling tool
//! batch is repaired in memory only (the log bytes are never rewritten on
//! read). See ADR-0009 D3/D4/D5/D6 for the invariants.
//!
//! The frontend-agnostic metadata decisions live here: id =
//! `slimcode-<unix>-<pid>-<n>`, `created_at` = RFC3339 UTC (no chrono dep;
//! civil-from-days below), and the title is inferred from the first user
//! message (truncated to 48 chars).

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use slimcode_core::session::{AgentMessage, Message, MessageStopReason, Role, Session};

/// Max title length before truncation.
const TITLE_MAX: usize = 48;

/// Default quota for the whole sessions directory: 500 MiB.
pub const DEFAULT_MAX_BYTES: u64 = 500 * 1024 * 1024;

/// FNV-1a 64-bit offset basis.
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
/// FNV-1a 64-bit prime.
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// FNV-1a 64-bit hash (stable across releases, no external dep). Used only to
/// disambiguate project directories: the basename already provides readability,
/// the hash guards against same-named repos at different paths.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Derive the per-project directory key: `<basename>-<12 hex chars of the
/// FNV-1a hash of the full path>`. Deterministic in the path, so the same
/// project home always maps to the same directory across runs and machines;
/// the path is canonicalized first so relative spellings (`repo`, `a/sub/..`)
/// and symlinked aliases collapse onto one key. Falls back to the given path
/// when it cannot be resolved (e.g. in unit tests with synthetic paths).
pub fn project_key(project_home: &Path) -> String {
    let path = project_home
        .canonicalize()
        .unwrap_or_else(|_| project_home.to_path_buf());
    let basename = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "root".to_string());
    let hash = fnv1a(path.to_string_lossy().as_bytes());
    format!("{basename}-{:012x}", hash >> 16)
}

/// True when `path` is a session log (a plain `.jsonl` file, not a directory
/// or other entry that happens to end in `.jsonl`).
fn is_log_file(path: &Path) -> bool {
    path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("jsonl")
}

/// True when `path` is a legacy whole-file session (a plain `.json` file). Old
/// files stay on disk and still count toward the quota, but are invisible to
/// load/list/cleanup (ADR-0009 D1).
fn is_legacy_json_file(path: &Path) -> bool {
    path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("json")
}

/// Convert unix epoch seconds to a UTC `YYYY-MM-DDTHH:MM:SSZ` string.
fn unix_to_rfc3339(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 -> (year, month, day).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// RFC3339 UTC string for the current time.
pub fn now_rfc3339() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    unix_to_rfc3339(secs)
}

/// Infer a session title from the first user message (truncated).
pub fn infer_title(messages: &[AgentMessage]) -> Option<String> {
    let text = messages
        .iter()
        .find(|m| m.role() == &Role::User)
        .map(AgentMessage::text_content)?;
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let truncated: String = text.chars().take(TITLE_MAX).collect();
    let with_ellipsis = if truncated.chars().count() < text.chars().count() {
        format!("{truncated}…")
    } else {
        truncated
    };
    Some(with_ellipsis)
}

/// The mandatory first line of a session log (ADR-0009 D4): the identity
/// record. `project_home` is the canonicalized project home the project key
/// was derived from. A missing or malformed header makes the whole log
/// unreadable — the load is a loud error, never a silent empty session.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct LogHeader {
    #[serde(rename = "type")]
    kind: String,
    v: u32,
    id: String,
    created_at: String,
    project_home: String,
}

impl LogHeader {
    fn new(session: &Session, project_home: &str) -> Self {
        Self {
            kind: "session".to_string(),
            v: 1,
            id: session.id.clone(),
            created_at: session.created_at.clone(),
            project_home: project_home.to_string(),
        }
    }
}

/// One record line after the header (ADR-0009 D4, ADR-0012 D4). Internally
/// tagged on `type` so the log stays self-describing and future versions can
/// skip unknown record types on read. A message record carries the log-only
/// `stop_reason` / `error` in the record envelope (omitted when absent).
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum LogRecord<'a> {
    Message {
        message: &'a AgentMessage,
        #[serde(skip_serializing_if = "Option::is_none")]
        stop_reason: Option<&'a MessageStopReason>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<&'a str>,
    },
    Title {
        title: &'a str,
    },
}

impl<'a> LogRecord<'a> {
    /// An ordinary message record (empty envelope).
    fn message(message: &'a AgentMessage) -> Self {
        Self::Message {
            message,
            stop_reason: None,
            error: None,
        }
    }

    /// A turn-closing message record: the reason lives in the envelope.
    fn closing(
        message: &'a AgentMessage,
        stop_reason: &'a MessageStopReason,
        error: Option<&'a str>,
    ) -> Self {
        Self::Message {
            message,
            stop_reason: Some(stop_reason),
            error,
        }
    }
}

/// What a lenient log replay produced (ADR-0009 D3): the session plus how much
/// was skipped/repaired, so the frontend can surface it.
#[derive(Clone, Debug)]
pub struct LoadOutcome {
    pub session: Session,
    /// Malformed or unknown record lines that were skipped.
    pub skipped_records: usize,
    /// Tool calls whose result never arrived; each got an in-memory
    /// `Error: interrupted` result (the log bytes are untouched).
    pub repaired_tool_calls: usize,
}

/// A project-scoped, directory-backed session store
/// (`<base>/<project-key>/<id>.jsonl`).
pub struct SessionStore {
    base: PathBuf,
    project: String,
    /// Canonicalized project home, written into every log header.
    project_home: String,
    /// Whole-store byte quota; appends evict oldest sessions past this.
    max_bytes: u64,
    next_id: AtomicUsize,
}

impl SessionStore {
    /// Build a store rooted at `base` (the sessions directory itself) and
    /// scoped to the given project key, with the default quota. `project_home`
    /// is the same project home the key was derived from; it is canonicalized
    /// (falling back to the path as given) and stamped into every log header.
    pub fn new(
        base: impl Into<PathBuf>,
        project: impl Into<String>,
        project_home: impl Into<PathBuf>,
    ) -> Self {
        let home = project_home.into();
        let project_home = home
            .canonicalize()
            .unwrap_or(home)
            .to_string_lossy()
            .into_owned();
        Self {
            base: base.into(),
            project: project.into(),
            project_home,
            max_bytes: DEFAULT_MAX_BYTES,
            next_id: AtomicUsize::new(0),
        }
    }

    /// Override the whole-store quota (bytes); eviction then targets half of
    /// this value.
    pub fn with_max_bytes(mut self, bytes: u64) -> Self {
        self.max_bytes = bytes;
        self
    }

    /// Generate a fresh session id: `slimcode-<unix>-<pid>-<n>`.
    pub fn new_id(&self) -> String {
        let unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let n = self.next_id.fetch_add(1, Ordering::SeqCst);
        format!("slimcode-{unix}-{}-{n}", std::process::id())
    }

    /// Build a fresh session with a new id and timestamp.
    pub fn new_session(&self) -> Session {
        Session {
            id: self.new_id(),
            created_at: now_rfc3339(),
            messages: Vec::new(),
            title: None,
        }
    }

    /// Absolute path for a session id within this project's subdirectory (id
    /// is validated to be a safe filename).
    pub fn session_path(&self, id: &str) -> Result<PathBuf, String> {
        if id.is_empty()
            || id.len() > 128
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(format!("invalid session id: {id:?}"));
        }
        Ok(self.base.join(&self.project).join(format!("{id}.jsonl")))
    }

    /// Append one message to `session`'s log (ADR-0009 D2/D5):
    ///
    /// * before the first assistant message the call is a no-op — no file is
    ///   created and `None` is returned;
    /// * the first assistant message creates the log exclusively — the header,
    ///   the current title (if any) and the session's whole message backlog
    ///   are written, and `Some(path)` is returned;
    /// * afterwards, exactly one record line is appended per call.
    ///
    /// `session` must already contain `message` (the caller pushes it into
    /// memory first); the creation path writes the backlog from
    /// `session.messages`.
    ///
    /// Creation is exclusive (`create_new`): if the log already exists between
    /// the existence check and the open, that is a loud error, never a merge.
    /// Nothing here validates or fsyncs (ADR-0009 D6) — appends are
    /// fire-and-forget; the store quota is enforced best-effort after each
    /// write.
    pub fn append(
        &self,
        session: &Session,
        message: &AgentMessage,
    ) -> Result<Option<PathBuf>, String> {
        self.append_with_envelope(session, message, None, None)
    }

    /// Append the assistant message that closes a failed/cancelled turn
    /// (ADR-0009 D5, ADR-0012 D4): the log-only `stop_reason` / `error` ride
    /// the record envelope, never the message payload.
    pub fn append_closing(
        &self,
        session: &Session,
        message: &AgentMessage,
        stop_reason: &MessageStopReason,
        error: Option<&str>,
    ) -> Result<Option<PathBuf>, String> {
        self.append_with_envelope(session, message, Some(stop_reason), error)
    }

    fn append_with_envelope(
        &self,
        session: &Session,
        message: &AgentMessage,
        stop_reason: Option<&MessageStopReason>,
        error: Option<&str>,
    ) -> Result<Option<PathBuf>, String> {
        let path = self.session_path(&session.id)?;
        if path.exists() {
            let record = match stop_reason {
                Some(reason) => LogRecord::closing(message, reason, error),
                None => LogRecord::message(message),
            };
            let line =
                serde_json::to_string(&record).map_err(|e| format!("serialize record: {e}"))?;
            append_line(&path, &line)?;
            self.evict_over_quota(&session.id);
            return Ok(None);
        }
        if message.role() != &Role::Assistant {
            // Nothing worth persisting yet: no file until the first assistant
            // message (ADR-0009 D5).
            return Ok(None);
        }
        let parent = path
            .parent()
            .ok_or_else(|| format!("no parent for {}", path.display()))?;
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let header = LogHeader::new(session, &self.project_home);
        write_header(&mut file, &path, &header)?;
        if let Some(title) = &session.title {
            write_record(&mut file, &path, &LogRecord::Title { title })?;
        }
        // The closing call's envelope belongs to the message that was just
        // pushed, i.e. the backlog's last one.
        let last = session.messages.len().saturating_sub(1);
        for (i, m) in session.messages.iter().enumerate() {
            let record = if i == last {
                match stop_reason {
                    Some(reason) => LogRecord::closing(m, reason, error),
                    None => LogRecord::message(m),
                }
            } else {
                LogRecord::message(m)
            };
            write_record(&mut file, &path, &record)?;
        }
        drop(file);
        self.evict_over_quota(&session.id);
        Ok(Some(path))
    }

    /// Append a title record to `session`'s log. Errors when the log does not
    /// exist yet (a title record must never precede the header).
    pub fn append_title(&self, session: &Session, title: &str) -> Result<(), String> {
        let path = self.session_path(&session.id)?;
        if !path.exists() {
            return Err(format!(
                "{}: no session log to append a title to",
                path.display()
            ));
        }
        let line = serde_json::to_string(&LogRecord::Title { title })
            .map_err(|e| format!("serialize record: {e}"))?;
        append_line(&path, &line)?;
        self.evict_over_quota(&session.id);
        Ok(())
    }

    /// Leniently replay `id`'s log into a session (ADR-0009 D3). The header is
    /// mandatory: a missing or malformed header is a loud error. Every record
    /// line after it is replayed with best effort — unknown record types and
    /// malformed lines are skipped and counted; a torn tail is dropped and
    /// sealed (earlier bytes are never rewritten); a dangling tool batch is
    /// repaired in memory only, never on disk.
    pub fn load(&self, id: &str) -> Result<LoadOutcome, String> {
        let path = self.session_path(id)?;
        let text = fs::read_to_string(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                format!("session {id:?} not found in project {:?}", self.project)
            } else {
                format!("{}: {e}", path.display())
            }
        })?;
        if text.is_empty() {
            return Err(format!("{}: empty session log", path.display()));
        }
        let mut lines = text.lines();
        let header_line = lines
            .next()
            .ok_or_else(|| format!("{}: missing log header", path.display()))?;
        let header: LogHeader = serde_json::from_str(header_line)
            .map_err(|e| format!("{}: invalid log header: {e}", path.display()))?;
        if header.kind != "session" {
            return Err(format!(
                "{}: not a session log (header type {:?})",
                path.display(),
                header.kind
            ));
        }

        // A torn tail is the signature of a crash mid-append: the final line is
        // a partial write. It is kept only when it parses as a complete
        // record; either way the tail is sealed with a newline so later
        // appends stay aligned. The seal is the only disk write a read may
        // ever perform, it never touches earlier bytes, and its failure is
        // best-effort (the replay already succeeded) — ADR-0009 D6.
        let mut record_lines: Vec<&str> = lines.collect();
        if !text.ends_with('\n') {
            if let Some(last) = record_lines.pop()
                && parse_record(last).is_ok()
            {
                record_lines.push(last);
            }
            let _ = seal_tail(&path);
        }

        let mut skipped_records = 0usize;
        let mut repaired_tool_calls = 0usize;
        let mut title: Option<String> = None;
        let mut messages: Vec<AgentMessage> = Vec::new();
        let mut batch: Option<OpenBatch> = None;

        for line in record_lines {
            let record = match parse_record(line) {
                Ok(r) => r,
                Err(_) => {
                    skipped_records += 1;
                    continue;
                }
            };
            match record {
                Record::Message(message) => {
                    // A legacy system record is skipped on load (ADR-0012 D3):
                    // the system prompt is rebuilt on the next turn. No log
                    // rewrite, no version bump.
                    if message.role() == &Role::System {
                        continue;
                    }
                    if message.role() == &Role::Assistant && !message.tool_calls().is_empty() {
                        // A new tool batch starts; any previous dangling batch
                        // closes first (later results cannot repair it). The
                        // requested ids stay in tool_calls order (a Vec) so a
                        // dangling batch is repaired deterministically
                        // (ADR-0009 D3); the HashSet is only a membership test.
                        close_batch(&mut batch, &mut messages, &mut repaired_tool_calls);
                        let ids: Vec<String> = message
                            .tool_calls()
                            .iter()
                            .map(|tc| tc.id.clone())
                            .collect();
                        let matched: HashSet<String> = HashSet::new();
                        batch = Some(OpenBatch { ids, matched });
                        messages.push(message);
                    } else if message.role() == &Role::Tool {
                        // A result belongs to the open batch only when its id
                        // was actually requested; anything else is an orphan
                        // and is dropped (ADR-0009 D3).
                        let belongs = batch.as_ref().is_some_and(|b| {
                            message
                                .tool_call_id()
                                .is_some_and(|id| b.ids.iter().any(|x| x == id))
                        });
                        if belongs {
                            let id = message.tool_call_id().unwrap().to_string();
                            batch.as_mut().unwrap().matched.insert(id);
                            messages.push(message);
                        }
                    } else {
                        // Any other message (user text, or an assistant message
                        // without tool calls) closes a dangling batch, then
                        // passes through.
                        close_batch(&mut batch, &mut messages, &mut repaired_tool_calls);
                        messages.push(message);
                    }
                }
                Record::Title(t) => title = Some(t),
            }
        }
        close_batch(&mut batch, &mut messages, &mut repaired_tool_calls);

        Ok(LoadOutcome {
            session: Session {
                id: header.id,
                created_at: header.created_at,
                title,
                messages,
            },
            skipped_records,
            repaired_tool_calls,
        })
    }

    /// List this project's session ids (stable-sorted). Legacy `.json` files
    /// stay invisible (ADR-0009 D1).
    pub fn list(&self) -> Result<Vec<String>, String> {
        let dir = self.base.join(&self.project);
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut ids: Vec<String> = fs::read_dir(&dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let path = e.path();
                let name = e.file_name().to_string_lossy().into_owned();
                if !is_log_file(&path) {
                    return None;
                }
                name.strip_suffix(".jsonl").map(str::to_string)
            })
            .collect();
        ids.sort();
        Ok(ids)
    }

    /// Delete empty session logs in this project's directory and return how
    /// many were removed. A log is empty when it replays to no assistant
    /// message: a zero-byte file, a missing/malformed header, or a header-only
    /// residue from a crash during log creation. Unreadable files are kept
    /// (removal is best-effort and silent). Legacy `.json` files and other
    /// projects are out of scope.
    pub fn cleanup_empty(&self) -> Result<usize, String> {
        let dir = self.base.join(&self.project);
        if !dir.exists() {
            return Ok(0);
        }
        let mut removed = 0;
        let entries = fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if !is_log_file(&path) {
                continue;
            }
            // A read failure keeps the file; replaying to no assistant message
            // marks it for removal.
            let empty = match fs::read(&path) {
                Ok(bytes) => !replays_to_assistant(&bytes),
                Err(_) => false,
            };
            if empty && fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Best-effort quota enforcement: once the whole sessions directory exceeds
    /// `max_bytes`, delete session files (logs and legacy whole-file sessions
    /// alike) oldest-first by mtime — skipping the active session — until the
    /// total is at or below `max_bytes / 2`. Legacy root-level files
    /// participate, and directories emptied by eviction are removed. Failures
    /// are ignored so a storage hiccup never breaks an append.
    fn evict_over_quota(&self, keep: &str) {
        let mut files = Vec::new();
        let mut total = 0u64;
        walk_store_files(&self.base, &mut files, &mut total);
        if total <= self.max_bytes {
            return;
        }
        let target = self.max_bytes / 2;
        files.sort_by_key(|(_, _, mtime)| *mtime);
        let keep_path = self.session_path(keep).ok();
        let mut remaining = total;
        for (path, size, _) in files {
            if remaining <= target {
                break;
            }
            if keep_path.as_ref() == Some(&path) {
                continue;
            }
            if fs::remove_file(&path).is_ok() {
                remaining = remaining.saturating_sub(size);
            }
        }
        remove_empty_subdirs(&self.base);
    }
}

/// Append one record line to `path` (adding a trailing newline).
fn append_line(path: &Path, line: &str) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    file.write_all(line.as_bytes())
        .and_then(|_| file.write_all(b"\n"))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Write the header line through an open file handle (the create path already
/// holds the handle exclusively).
fn write_header(file: &mut fs::File, path: &Path, header: &LogHeader) -> Result<(), String> {
    let line = serde_json::to_string(header).map_err(|e| format!("serialize header: {e}"))?;
    file.write_all(line.as_bytes())
        .and_then(|_| file.write_all(b"\n"))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Write one record line through an open file handle.
fn write_record(file: &mut fs::File, path: &Path, record: &LogRecord<'_>) -> Result<(), String> {
    let line = serde_json::to_string(record).map_err(|e| format!("serialize record: {e}"))?;
    file.write_all(line.as_bytes())
        .and_then(|_| file.write_all(b"\n"))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Seal a torn log tail with a trailing newline so later appends stay aligned
/// (the only disk write a read may ever perform; ADR-0009 D6).
fn seal_tail(path: &Path) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    file.write_all(b"\n")
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// A replayed record line. The record envelope's `stop_reason` / `error` are
/// log-only metadata; the load replays the message itself and drops them.
enum Record {
    Message(AgentMessage),
    Title(String),
}

/// Parse one record line; any line that is not a well-formed, known record is
/// an error (the caller counts it as skipped).
fn parse_record(line: &str) -> Result<Record, String> {
    let value: Value = serde_json::from_str(line).map_err(|e| format!("invalid JSON: {e}"))?;
    match value.get("type").and_then(Value::as_str) {
        Some("message") => {
            let message = value
                .get("message")
                .cloned()
                .ok_or_else(|| "message record without message".to_string())?;
            Ok(Record::Message(parse_agent_message(message)?))
        }
        Some("title") => {
            let title = value
                .get("title")
                .and_then(Value::as_str)
                .ok_or_else(|| "title record without title".to_string())?;
            Ok(Record::Title(title.to_string()))
        }
        Some(other) => Err(format!("unknown record type {other:?}")),
        None => Err("record without a type".to_string()),
    }
}

/// Parse a message record payload. Tagged payloads (`{"kind":"llm",…}`) are
/// the current shape; a payload without `kind` is a legacy (pre-ADR-0012)
/// record whose payload was a bare wire message (possibly with the log-only
/// `stop_reason`/`error` fields inline, ignored here).
fn parse_agent_message(value: Value) -> Result<AgentMessage, String> {
    if value.get("kind").is_some() {
        serde_json::from_value(value).map_err(|e| format!("invalid message record: {e}"))
    } else {
        let message: Message = serde_json::from_value(value)
            .map_err(|e| format!("invalid legacy message record: {e}"))?;
        Ok(AgentMessage::llm(message))
    }
}

/// A tool batch awaiting its results during replay: the ids the batch calls,
/// in tool_calls order (a Vec — iteration order is deterministic), and which
/// of them already matched a tool result (a HashSet — membership only).
struct OpenBatch {
    ids: Vec<String>,
    matched: HashSet<String>,
}

/// Close a dangling batch: every requested call that never got a result is
/// patched in memory with `Error: interrupted` (ADR-0009 D3) — in history
/// order relative to the results that did arrive — and the batch is cleared.
fn close_batch(
    batch: &mut Option<OpenBatch>,
    messages: &mut Vec<AgentMessage>,
    repaired: &mut usize,
) {
    let Some(open) = batch.take() else {
        return;
    };
    for id in &open.ids {
        if !open.matched.contains(id) {
            messages.push(AgentMessage::tool_result(id.clone(), "Error: interrupted"));
            *repaired += 1;
        }
    }
}

/// True when `bytes` (a log's raw contents) replay to at least one assistant
/// message record. Lenient like load: lines are parsed independently and bad
/// lines are skipped. A zero-byte file or a missing/malformed header is "no
/// assistant" (crash residue).
fn replays_to_assistant(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes);
    let mut lines = text.lines();
    let Some(header) = lines
        .next()
        .and_then(|l| serde_json::from_str::<LogHeader>(l).ok())
    else {
        return false;
    };
    if header.kind != "session" {
        return false;
    }
    lines.any(|line| {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            return false;
        };
        value.get("type").and_then(Value::as_str) == Some("message")
            && value.pointer("/message/role").and_then(Value::as_str) == Some("assistant")
    })
}

/// Collect every session file under `dir` (recursing into subdirectories) as
/// `(path, size, mtime)`; `total` accumulates the byte sum. Both `.jsonl` logs
/// and legacy `.json` sessions count (ADR-0009 D1). mtime read failures are
/// treated as the oldest (evicted first) so a quota run still makes progress.
fn walk_store_files(
    dir: &Path,
    out: &mut Vec<(PathBuf, u64, std::time::SystemTime)>,
    total: &mut u64,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_store_files(&path, out, total);
            continue;
        }
        if !(is_log_file(&path) || is_legacy_json_file(&path)) {
            continue;
        }
        if let Ok(meta) = fs::metadata(&path) {
            let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
            *total += meta.len();
            out.push((path, meta.len(), mtime));
        }
    }
}

/// Best-effort removal of empty subdirectories directly under `base` (the
/// per-project directories). Non-empty directories fail and are ignored.
fn remove_empty_subdirs(base: &Path) {
    let Ok(entries) = fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let _ = fs::remove_dir(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::unique_temp_dir;
    use slimcode_core::session::{AgentMessage, Message, MessageStopReason, ToolCall};
    use std::time::Duration;

    /// Unique temp dir per test (tests run in parallel and must not share).
    fn temp_dir() -> PathBuf {
        unique_temp_dir("slimcode-session-test")
    }

    /// A store scoped to `proj-a` under `base`, with a synthetic project home.
    fn store(base: &Path) -> SessionStore {
        SessionStore::new(base, "proj-a", base.join("home"))
    }

    /// A minimal session with the given id and one user message.
    fn session_with(id: &str) -> Session {
        Session {
            id: id.to_string(),
            created_at: now_rfc3339(),
            title: None,
            messages: vec![AgentMessage::text(Role::User, "hi")],
        }
    }

    /// The raw header line a store would write for `id`/`created_at`.
    fn raw_header(id: &str, created: &str) -> String {
        serde_json::to_string(&LogHeader {
            kind: "session".to_string(),
            v: 1,
            id: id.to_string(),
            created_at: created.to_string(),
            project_home: "/synthetic/proj".to_string(),
        })
        .unwrap()
    }

    /// A message record line.
    fn msg_line(m: &AgentMessage) -> String {
        serde_json::to_string(&LogRecord::message(m)).unwrap()
    }

    /// A title record line.
    fn title_line(title: &str) -> String {
        serde_json::to_string(&LogRecord::Title { title }).unwrap()
    }

    /// Write a log file from raw lines (each gets a trailing newline).
    fn write_raw(path: &Path, lines: &[&str]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut content = String::new();
        for l in lines {
            content.push_str(l);
            content.push('\n');
        }
        fs::write(path, content).unwrap();
    }

    /// Write `contents` to `path` with an mtime `age` in the past.
    fn write_aged(path: &Path, contents: &[u8], age: Duration) {
        fs::write(path, contents).unwrap();
        let mtime = SystemTime::now() - age;
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
    }

    // --- metadata (unchanged behaviors) ------------------------------------

    #[test]
    fn unix_epoch_formats_rfc3339() {
        assert_eq!(unix_to_rfc3339(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn known_timestamp_formats_rfc3339() {
        // 2026-08-29T12:34:56Z = ? unix. Compute via day count.
        // Days from 1970-01-01 to 2026-08-29 (leap years 1972..2024 = 14):
        let days = 56 * 365 + 14 + (31 + 28 + 31 + 30 + 31 + 30 + 31 + 28); // to Aug 28
        let secs = days as i64 * 86_400 + 12 * 3600 + 34 * 60 + 56;
        assert_eq!(unix_to_rfc3339(secs), "2026-08-29T12:34:56Z");
    }

    #[test]
    fn civil_from_days_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // 2000-03-01 (leap day boundary)
        let days_to_2000_03_01 = 30 * 365 + 7 + (31 + 29); // 1970..1999 = 30y, 7 leap; +Jan+Feb 2000
        assert_eq!(civil_from_days(days_to_2000_03_01), (2000, 3, 1));
    }

    #[test]
    fn title_from_first_user_message_truncated() {
        let msgs = vec![
            AgentMessage::text(Role::System, "be helpful"),
            AgentMessage::text(Role::User, "short prompt"),
            AgentMessage::text(Role::User, "second"),
        ];
        assert_eq!(infer_title(&msgs).unwrap(), "short prompt");
    }

    #[test]
    fn title_truncates_long_prompts() {
        let long = "x".repeat(100);
        let msgs = vec![AgentMessage::text(Role::User, &long)];
        let t = infer_title(&msgs).unwrap();
        assert!(t.chars().count() <= TITLE_MAX + 1); // +1 for ellipsis
        assert!(t.ends_with('…'));
    }

    #[test]
    fn title_none_without_user_message() {
        let msgs = vec![AgentMessage::text(Role::System, "sys")];
        assert!(infer_title(&msgs).is_none());
    }

    #[test]
    fn new_id_is_unique_and_prefixed() {
        let s = SessionStore::new(std::env::temp_dir(), "proj-test", std::env::temp_dir());
        let a = s.new_id();
        let b = s.new_id();
        assert_ne!(a, b);
        assert!(a.starts_with("slimcode-"));
    }

    #[test]
    fn new_session_has_fresh_id_and_empty_messages() {
        let s = SessionStore::new(std::env::temp_dir(), "proj-test", std::env::temp_dir());
        let session = s.new_session();
        assert!(session.id.starts_with("slimcode-"));
        assert!(session.messages.is_empty());
        assert!(session.title.is_none());
        assert!(!session.created_at.is_empty());
    }

    #[test]
    fn invalid_id_rejected() {
        let s = SessionStore::new(std::env::temp_dir(), "proj-test", std::env::temp_dir());
        assert!(s.session_path("../evil").is_err());
        assert!(s.session_path("a/b").is_err());
        assert!(s.session_path("").is_err());
    }

    #[test]
    fn session_path_points_at_jsonl_not_json() {
        let dir = temp_dir();
        let store = store(&dir);
        let path = store.session_path("slimcode-1").unwrap();
        assert_eq!(path.file_name().unwrap(), "slimcode-1.jsonl");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn project_key_is_deterministic() {
        let p = PathBuf::from("/home/u/work/repo");
        assert_eq!(project_key(&p), project_key(&p));
    }

    #[test]
    fn project_key_disambiguates_same_basename() {
        let a = PathBuf::from("/home/u/a/repo");
        let b = PathBuf::from("/home/u/b/repo");
        assert_ne!(project_key(&a), project_key(&b));
        assert!(project_key(&a).starts_with("repo-"));
        assert!(project_key(&b).starts_with("repo-"));
    }

    #[test]
    fn project_key_hashes_path_not_just_basename() {
        let a = PathBuf::from("/home/u/repo");
        let b = PathBuf::from("/home/v/repo");
        assert_ne!(project_key(&a), project_key(&b));
    }

    #[test]
    fn project_key_canonicalizes_equivalent_paths() {
        // A relative-ish spelling with a `..` component must map to the same
        // key as the resolved directory (otherwise `--cwd repo` would split a
        // project's sessions across two directories).
        let dir = temp_dir();
        fs::create_dir_all(dir.join("sub")).unwrap();
        assert_eq!(project_key(&dir.join("sub").join("..")), project_key(&dir));
        let _ = fs::remove_dir_all(&dir);
    }

    // --- append: lazy exclusive creation (ticket 01) ------------------------

    #[test]
    fn append_before_first_assistant_writes_no_file() {
        let dir = temp_dir();
        let store = store(&dir);
        let mut session = store.new_session();
        let user = AgentMessage::text(Role::User, "hi");
        session.messages.push(user.clone());
        assert!(store.append(&session, &user).unwrap().is_none());
        // No file exists before the first assistant message.
        assert!(!store.session_path(&session.id).unwrap().exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn first_assistant_creates_log_with_header_title_and_backlog() {
        let dir = temp_dir();
        let store = store(&dir);
        let session = Session {
            id: "slimcode-1".to_string(),
            created_at: "2026-08-29T00:00:00Z".to_string(),
            title: Some("hello".to_string()),
            messages: vec![
                AgentMessage::text(Role::User, "hi"),
                AgentMessage::text(Role::Assistant, "hey!"),
            ],
        };
        let asst = session.messages.last().unwrap().clone();
        let created = store.append(&session, &asst).unwrap().unwrap();
        assert_eq!(created, store.session_path("slimcode-1").unwrap());
        let text = fs::read_to_string(&created).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        // header + title + the whole message backlog
        assert_eq!(lines.len(), 1 + 1 + 2);
        let v: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(v["type"], "session");
        assert_eq!(v["v"], 1);
        assert_eq!(v["id"], "slimcode-1");
        assert_eq!(v["created_at"], "2026-08-29T00:00:00Z");
        assert!(
            v["project_home"]
                .as_str()
                .unwrap()
                .contains("slimcode-session-test")
        );
        assert_eq!(lines[1], title_line("hello"));
        for (i, expected) in session.messages.iter().enumerate() {
            assert_eq!(lines[2 + i], msg_line(expected));
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_adds_one_record_line_per_message_after_creation() {
        let dir = temp_dir();
        let store = store(&dir);
        let mut session = Session {
            id: "slimcode-1".to_string(),
            created_at: "2026-08-29T00:00:00Z".to_string(),
            title: None,
            messages: vec![
                AgentMessage::text(Role::User, "hi"),
                AgentMessage::text(Role::Assistant, "first reply"),
            ],
        };
        let asst = session.messages[1].clone();
        store.append(&session, &asst).unwrap();
        let m4 = AgentMessage::text(Role::User, "again");
        session.messages.push(m4.clone());
        store.append(&session, &m4).unwrap();
        let m5 = AgentMessage::text(Role::Assistant, "second reply");
        session.messages.push(m5.clone());
        store.append(&session, &m5).unwrap();
        let lines: Vec<String> = fs::read_to_string(store.session_path("slimcode-1").unwrap())
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        // header + one line per message, exactly
        assert_eq!(lines.len(), 1 + 4);
        assert_eq!(lines[1], msg_line(&session.messages[0]));
        assert_eq!(lines[3], msg_line(&m4));
        assert_eq!(lines[4], msg_line(&m5));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_title_writes_a_title_record_and_load_takes_the_last() {
        let dir = temp_dir();
        let store = store(&dir);
        let mut session = session_with("slimcode-1");
        let asst = AgentMessage::text(Role::Assistant, "hi");
        session.messages.push(asst.clone());
        store.append(&session, &asst).unwrap();
        store.append_title(&session, "first title").unwrap();
        store.append_title(&session, "second title").unwrap();
        let outcome = store.load("slimcode-1").unwrap();
        // The latest title record wins.
        assert_eq!(outcome.session.title.as_deref(), Some("second title"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_title_errors_before_the_log_exists() {
        let dir = temp_dir();
        let store = store(&dir);
        let session = session_with("slimcode-1");
        let err = store.append_title(&session, "hi").unwrap_err();
        assert!(err.contains("slimcode-1"), "err: {err}");
        let _ = fs::remove_dir_all(&dir);
    }

    // --- load: lenient replay (ticket 01) -----------------------------------

    #[test]
    fn load_round_trips_appended_messages() {
        let dir = temp_dir();
        let store = store(&dir);
        let mut session = Session {
            id: "slimcode-1".to_string(),
            created_at: "2026-08-29T00:00:00Z".to_string(),
            title: None,
            messages: vec![AgentMessage::text(Role::User, "hi")],
        };
        let asst = AgentMessage::text(Role::Assistant, "hello");
        session.messages.push(asst.clone());
        store.append(&session, &asst).unwrap();
        let outcome = store.load("slimcode-1").unwrap();
        assert_eq!(outcome.session.messages, session.messages);
        assert_eq!(outcome.session.created_at, "2026-08-29T00:00:00Z");
        assert_eq!(outcome.skipped_records, 0);
        assert_eq!(outcome.repaired_tool_calls, 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_skips_malformed_and_unknown_records_with_count() {
        let dir = temp_dir();
        let store = store(&dir);
        let path = store.session_path("slimcode-1").unwrap();
        write_raw(
            &path,
            &[
                &raw_header("slimcode-1", "2026-08-29T00:00:00Z"),
                &msg_line(&AgentMessage::text(Role::User, "hi")),
                "{ not json",                        // malformed
                r#"{"type":"future_record","x":1}"#, // unknown type
                &msg_line(&AgentMessage::text(Role::Assistant, "hello")),
            ],
        );
        let outcome = store.load("slimcode-1").unwrap();
        assert_eq!(outcome.skipped_records, 2);
        assert_eq!(outcome.session.messages.len(), 2);
        assert_eq!(outcome.session.messages[1].text_content(), "hello");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_missing_or_malformed_header_errors() {
        let dir = temp_dir();
        let store = store(&dir);
        let path = store.session_path("slimcode-1").unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "not json at all\n").unwrap();
        let err = store.load("slimcode-1").unwrap_err();
        assert!(err.contains("header"), "err: {err}");
        // A header of a different kind is rejected too.
        fs::write(
            &path,
            r#"{"type":"other","v":1,"id":"x","created_at":"c","project_home":"p"}"#,
        )
        .unwrap();
        let err = store.load("slimcode-1").unwrap_err();
        assert!(err.contains("not a session log"), "err: {err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_empty_log_errors() {
        let dir = temp_dir();
        let store = store(&dir);
        let path = store.session_path("slimcode-1").unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "").unwrap();
        let err = store.load("slimcode-1").unwrap_err();
        assert!(err.contains("empty"), "err: {err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_missing_id_reports_project_scope() {
        let dir = temp_dir();
        let store = store(&dir);
        let err = store.load("slimcode-absent").unwrap_err();
        assert!(err.contains("slimcode-absent"), "err: {err}");
        assert!(err.contains("proj-a"), "err: {err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_drops_a_torn_tail_and_seals_it() {
        let dir = temp_dir();
        let store = store(&dir);
        let path = store.session_path("slimcode-1").unwrap();
        let partial = format!(
            "{}\n{}\n{}\n{{\"type\":\"mess",
            raw_header("slimcode-1", "2026-08-29T00:00:00Z"),
            msg_line(&AgentMessage::text(Role::User, "hi")),
            msg_line(&AgentMessage::text(Role::Assistant, "hello")),
        );
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, partial).unwrap();
        let outcome = store.load("slimcode-1").unwrap();
        assert_eq!(outcome.session.messages.len(), 2);
        assert_eq!(outcome.skipped_records, 0);
        // The torn tail was sealed with a newline so a later append stays
        // aligned on its own line.
        assert!(fs::read_to_string(&path).unwrap().ends_with('\n'));
        let m = AgentMessage::text(Role::Assistant, "after");
        let mut sess = outcome.session.clone();
        sess.messages.push(m.clone());
        store.append(&sess, &m).unwrap();
        let outcome2 = store.load("slimcode-1").unwrap();
        assert_eq!(outcome2.session.messages.len(), 3);
        assert_eq!(outcome2.session.messages[2].text_content(), "after");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_keeps_a_complete_json_torn_tail_and_seals_it() {
        let dir = temp_dir();
        let store = store(&dir);
        let path = store.session_path("slimcode-1").unwrap();
        // The final record is complete JSON but lacks the trailing newline
        // (the crash hit between the record and the newline): it is kept.
        let content = format!(
            "{}\n{}\n{}",
            raw_header("slimcode-1", "2026-08-29T00:00:00Z"),
            msg_line(&AgentMessage::text(Role::User, "hi")),
            msg_line(&AgentMessage::text(Role::Assistant, "hello")),
        );
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        let outcome = store.load("slimcode-1").unwrap();
        assert_eq!(outcome.session.messages.len(), 2);
        assert_eq!(outcome.skipped_records, 0);
        assert!(fs::read_to_string(&path).unwrap().ends_with('\n'));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_repairs_dangling_tool_batches_in_memory_only() {
        let dir = temp_dir();
        let store = store(&dir);
        let path = store.session_path("slimcode-1").unwrap();
        let mut wire = Message::text(Role::Assistant, "");
        wire.tool_calls = vec![
            ToolCall {
                id: "call_a".to_string(),
                name: "t".to_string(),
                arguments: "{}".to_string(),
            },
            ToolCall {
                id: "call_b".to_string(),
                name: "t".to_string(),
                arguments: "{}".to_string(),
            },
            ToolCall {
                id: "call_c".to_string(),
                name: "t".to_string(),
                arguments: "{}".to_string(),
            },
            ToolCall {
                id: "call_d".to_string(),
                name: "t".to_string(),
                arguments: "{}".to_string(),
            },
        ];
        let asst = AgentMessage::Llm(wire);
        write_raw(
            &path,
            &[
                &raw_header("slimcode-1", "2026-08-29T00:00:00Z"),
                &msg_line(&AgentMessage::text(Role::System, "sys")),
                &msg_line(&AgentMessage::text(Role::User, "hi")),
                &msg_line(&asst),
                &msg_line(&AgentMessage::tool_result("call_a", "{\"a\":1}")),
                &msg_line(&AgentMessage::tool_result("call_c", "{\"c\":1}")),
                // An orphan result: no batch ever requested this id — dropped.
                &msg_line(&AgentMessage::tool_result("call_orphan", "x")),
            ],
        );
        let before = fs::read(&path).unwrap();
        let outcome = store.load("slimcode-1").unwrap();
        assert_eq!(outcome.repaired_tool_calls, 2);
        assert_eq!(outcome.skipped_records, 0);
        // user, asst, tool(a), tool(c), tool(b interrupted), tool(d
        // interrupted) — the legacy system record is skipped (ADR-0012 D3);
        // the missing results land in history order after the results that did
        // arrive, and the two repairs are in tool_calls order (deterministic:
        // same file replays to the same history, ADR-0009 D3).
        let msgs = &outcome.session.messages;
        assert_eq!(msgs.len(), 6);
        assert_eq!(msgs[2].role(), &Role::Tool);
        assert_eq!(msgs[2].tool_call_id(), Some("call_a"));
        assert_eq!(msgs[3].tool_call_id(), Some("call_c"));
        assert_eq!(msgs[4].tool_call_id(), Some("call_b"));
        assert_eq!(msgs[4].text_content(), "Error: interrupted");
        assert_eq!(msgs[5].tool_call_id(), Some("call_d"));
        assert_eq!(msgs[5].text_content(), "Error: interrupted");
        // Repair is in memory only: the log bytes are untouched.
        assert_eq!(fs::read(&path).unwrap(), before);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_closes_a_dangling_batch_at_the_next_boundary() {
        let dir = temp_dir();
        let store = store(&dir);
        let path = store.session_path("slimcode-1").unwrap();
        let mut wire = Message::text(Role::Assistant, "");
        wire.tool_calls = vec![ToolCall {
            id: "call_a".to_string(),
            name: "t".to_string(),
            arguments: "{}".to_string(),
        }];
        let asst = AgentMessage::Llm(wire);
        write_raw(
            &path,
            &[
                &raw_header("slimcode-1", "2026-08-29T00:00:00Z"),
                &msg_line(&AgentMessage::text(Role::User, "hi")),
                &msg_line(&asst),
                // The next assistant message closes the dangling batch before
                // it is replayed itself.
                &msg_line(&AgentMessage::text(Role::Assistant, "hello")),
            ],
        );
        let outcome = store.load("slimcode-1").unwrap();
        assert_eq!(outcome.repaired_tool_calls, 1);
        // user, asst, tool(interrupted), assistant(hello)
        assert_eq!(outcome.session.messages.len(), 4);
        assert_eq!(outcome.session.messages[2].tool_call_id(), Some("call_a"));
        assert_eq!(outcome.session.messages[3].text_content(), "hello");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn closing_message_round_trips_through_the_log() {
        let dir = temp_dir();
        let store = store(&dir);
        let mut session = session_with("slimcode-1");
        let user = session.messages[0].clone();
        store.append(&session, &user).unwrap(); // no-op pre-assistant
        let closing = AgentMessage::text(Role::Assistant, "The turn ended with an error: boom");
        session.messages.push(closing.clone());
        store
            .append_closing(&session, &closing, &MessageStopReason::Error, Some("boom"))
            .unwrap();
        // The reason lives in the record envelope, never in the message payload
        // (ADR-0012 D4).
        let text = fs::read_to_string(store.session_path("slimcode-1").unwrap()).unwrap();
        let v: Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
        assert_eq!(v["stop_reason"], "error");
        assert_eq!(v["error"], "boom");
        assert!(v["message"].get("stop_reason").is_none());
        assert!(v["message"].get("error").is_none());
        // Load replays the message itself; the envelope is metadata.
        let outcome = store.load("slimcode-1").unwrap();
        assert_eq!(outcome.session.messages, session.messages);
        let loaded = &outcome.session.messages[1];
        assert_eq!(loaded.role(), &Role::Assistant);
        assert_eq!(loaded.text_content(), "The turn ended with an error: boom");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ordinary_message_record_omits_the_envelope() {
        let dir = temp_dir();
        let store = store(&dir);
        let mut session = session_with("slimcode-1");
        let asst = AgentMessage::text(Role::Assistant, "hey");
        session.messages.push(asst.clone());
        store.append(&session, &asst).unwrap();
        let text = fs::read_to_string(store.session_path("slimcode-1").unwrap()).unwrap();
        let v: Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
        assert!(v.get("stop_reason").is_none());
        assert!(v.get("error").is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_skips_a_legacy_system_record() {
        let dir = temp_dir();
        let store = store(&dir);
        let path = store.session_path("slimcode-1").unwrap();
        // A pre-ADR-0012 log: a system record (bare wire payload, with the
        // log-only fields inline) precedes the user/assistant messages.
        write_raw(
            &path,
            &[
                &raw_header("slimcode-1", "2026-08-29T00:00:00Z"),
                r#"{"type":"message","message":{"role":"system","parts":[{"type":"text","text":"be helpful"}]}}"#,
                &msg_line(&AgentMessage::text(Role::User, "hi")),
                &msg_line(&AgentMessage::text(Role::Assistant, "hello")),
            ],
        );
        let outcome = store.load("slimcode-1").unwrap();
        // The system record is skipped without being counted as a bad record,
        // and the file is not rewritten.
        assert_eq!(outcome.skipped_records, 0);
        assert_eq!(outcome.session.messages.len(), 2);
        assert!(
            outcome
                .session
                .messages
                .iter()
                .all(|m| m.role() != &Role::System)
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_accepts_a_legacy_bare_message_payload() {
        let dir = temp_dir();
        let store = store(&dir);
        let path = store.session_path("slimcode-1").unwrap();
        // Pre-ADR-0012 payloads had no `kind` tag and carried the log-only
        // fields inline; they still load (the extra fields are ignored).
        write_raw(
            &path,
            &[
                &raw_header("slimcode-1", "2026-08-29T00:00:00Z"),
                r#"{"type":"message","message":{"role":"user","parts":[{"type":"text","text":"hi"}]}}"#,
                r#"{"type":"message","message":{"role":"assistant","parts":[{"type":"text","text":"boom"}],"stop_reason":"error","error":"boom"}}"#,
            ],
        );
        let outcome = store.load("slimcode-1").unwrap();
        assert_eq!(outcome.skipped_records, 0);
        assert_eq!(outcome.session.messages.len(), 2);
        assert_eq!(outcome.session.messages[1].text_content(), "boom");
        let _ = fs::remove_dir_all(&dir);
    }

    // --- listing + isolation ------------------------------------------------

    #[test]
    fn list_only_current_project() {
        let dir = temp_dir();
        let a = store(&dir);
        let b = SessionStore::new(&dir, "proj-b", dir.join("home"));
        let mut s1 = session_with("slimcode-1-a");
        let asst = AgentMessage::text(Role::Assistant, "hi");
        s1.messages.push(asst.clone());
        a.append(&s1, &asst).unwrap();
        let mut s2 = session_with("slimcode-2-b");
        let asst = AgentMessage::text(Role::Assistant, "hi");
        s2.messages.push(asst.clone());
        b.append(&s2, &asst).unwrap();
        let mut s3 = session_with("slimcode-3-a");
        let asst = AgentMessage::text(Role::Assistant, "hi");
        s3.messages.push(asst.clone());
        a.append(&s3, &asst).unwrap();
        assert_eq!(a.list().unwrap(), vec!["slimcode-1-a", "slimcode-3-a"]);
        assert_eq!(b.list().unwrap(), vec!["slimcode-2-b"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_is_isolated_to_current_project() {
        let dir = temp_dir();
        // Legacy root-level file (old flat layout) must not be loadable.
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("slimcode-old-1.json"), "{}").unwrap();
        // Another project's file must not be loadable either.
        let other = SessionStore::new(&dir, "proj-b", dir.join("home"));
        let mut s2 = session_with("slimcode-2-b");
        let asst = AgentMessage::text(Role::Assistant, "hi");
        s2.messages.push(asst.clone());
        other.append(&s2, &asst).unwrap();
        // The current project's own file loads fine.
        let store = store(&dir);
        let mut s3 = session_with("slimcode-3-a");
        let asst = AgentMessage::text(Role::Assistant, "hi");
        s3.messages.push(asst.clone());
        store.append(&s3, &asst).unwrap();
        assert!(store.load("slimcode-3-a").is_ok());
        assert!(store.load("slimcode-old-1").is_err());
        assert!(store.load("slimcode-2-b").is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_ignores_a_directory_named_like_a_session_file() {
        let dir = temp_dir();
        let store = store(&dir);
        let mut s = session_with("slimcode-real");
        let asst = AgentMessage::text(Role::Assistant, "hi");
        s.messages.push(asst.clone());
        store.append(&s, &asst).unwrap();
        // A directory whose name ends in `.jsonl` must not be listed.
        fs::create_dir_all(dir.join("proj-a/slimcode-fake.jsonl")).unwrap();
        assert_eq!(store.list().unwrap(), vec!["slimcode-real"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_ignores_legacy_json_files() {
        let dir = temp_dir();
        let store = store(&dir);
        let mut s = session_with("slimcode-jsonl");
        let asst = AgentMessage::text(Role::Assistant, "hi");
        s.messages.push(asst.clone());
        store.append(&s, &asst).unwrap();
        // A legacy whole-file session stays invisible to the listing.
        fs::write(dir.join("proj-a/slimcode-legacy.json"), "{}").unwrap();
        assert_eq!(store.list().unwrap(), vec!["slimcode-jsonl"]);
        let _ = fs::remove_dir_all(&dir);
    }

    // --- startup empty-log cleanup (ticket 01) ------------------------------

    #[test]
    fn cleanup_empty_removes_zero_byte_and_assistantless_logs_but_keeps_live() {
        let dir = temp_dir();
        let store = store(&dir);
        let proj = dir.join("proj-a");
        fs::create_dir_all(&proj).unwrap();
        // A live log with an assistant message.
        let mut live = session_with("slimcode-live");
        let asst = AgentMessage::text(Role::Assistant, "hello");
        live.messages.push(asst.clone());
        store.append(&live, &asst).unwrap();
        // A zero-byte log and a header-only residue (crash during creation).
        fs::write(proj.join("slimcode-zero.jsonl"), "").unwrap();
        fs::write(
            proj.join("slimcode-residue.jsonl"),
            format!("{}\n", raw_header("slimcode-residue", "c")),
        )
        .unwrap();
        let removed = store.cleanup_empty().unwrap();
        assert_eq!(removed, 2);
        assert!(!proj.join("slimcode-zero.jsonl").exists());
        assert!(!proj.join("slimcode-residue.jsonl").exists());
        assert!(proj.join("slimcode-live.jsonl").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cleanup_empty_scopes_to_current_project_and_skips_legacy() {
        let dir = temp_dir();
        let a = store(&dir);
        let b = SessionStore::new(&dir, "proj-b", dir.join("home"));
        fs::create_dir_all(dir.join("proj-a")).unwrap();
        fs::write(dir.join("proj-a/slimcode-empty-a.jsonl"), "").unwrap();
        // A live log in another project must be untouched.
        let mut live = session_with("slimcode-keep-b");
        let asst = AgentMessage::text(Role::Assistant, "hello");
        live.messages.push(asst.clone());
        b.append(&live, &asst).unwrap();
        // Legacy whole-file sessions are out of scope too.
        fs::write(dir.join("proj-a/slimcode-legacy.json"), "{}").unwrap();
        let removed = a.cleanup_empty().unwrap();
        assert_eq!(removed, 1);
        assert!(!dir.join("proj-a/slimcode-empty-a.jsonl").exists());
        assert!(dir.join("proj-a/slimcode-legacy.json").exists());
        assert!(dir.join("proj-b/slimcode-keep-b.jsonl").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cleanup_empty_is_noop_when_dir_missing() {
        let dir = temp_dir();
        let store = store(&dir);
        assert_eq!(store.cleanup_empty().unwrap(), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    // --- quota eviction on append (ticket 01) -------------------------------

    #[test]
    fn evict_deletes_oldest_first_down_to_half_and_keeps_active() {
        let dir = temp_dir();
        let store = store(&dir).with_max_bytes(1000);
        let proj = dir.join("proj-a");
        fs::create_dir_all(&proj).unwrap();
        write_aged(
            &proj.join("slimcode-old1.jsonl"),
            &[b'x'; 450],
            Duration::from_secs(3 * 3600),
        );
        write_aged(
            &proj.join("slimcode-old2.jsonl"),
            &[b'x'; 450],
            Duration::from_secs(2 * 3600),
        );
        write_aged(
            &proj.join("slimcode-old3.jsonl"),
            &[b'x'; 100],
            Duration::from_secs(3600),
        );
        // A create-triggering append enforces the quota.
        let mut session = session_with("slimcode-new");
        let asst = AgentMessage::text(Role::Assistant, "hi");
        session.messages.push(asst.clone());
        store.append(&session, &asst).unwrap();
        // Oldest two (450+450) evicted to get the total ≤ half (500); the
        // smallest old file and the just-created active session survive.
        assert!(!proj.join("slimcode-old1.jsonl").exists());
        assert!(!proj.join("slimcode-old2.jsonl").exists());
        assert!(proj.join("slimcode-old3.jsonl").exists());
        assert!(proj.join("slimcode-new.jsonl").exists());
        let total: u64 = fs::read_dir(&proj)
            .unwrap()
            .flatten()
            .map(|e| e.metadata().unwrap().len())
            .sum();
        assert!(total <= 500, "total after eviction: {total}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn evict_counts_legacy_json_and_jsonl_together() {
        let dir = temp_dir();
        let store = store(&dir).with_max_bytes(1000);
        let proj = dir.join("proj-a");
        fs::create_dir_all(&proj).unwrap();
        write_aged(
            &proj.join("slimcode-legacy.json"),
            &[b'x'; 500],
            Duration::from_secs(3600),
        );
        write_aged(
            &proj.join("slimcode-old.jsonl"),
            &[b'x'; 500],
            Duration::from_secs(2 * 3600),
        );
        let mut session = session_with("slimcode-new");
        let asst = AgentMessage::text(Role::Assistant, "hi");
        session.messages.push(asst.clone());
        store.append(&session, &asst).unwrap();
        // Oldest first across both formats: the .jsonl (2h) then the legacy
        // .json (1h) are evicted; only the active session survives.
        assert!(!proj.join("slimcode-old.jsonl").exists());
        assert!(!proj.join("slimcode-legacy.json").exists());
        assert!(proj.join("slimcode-new.jsonl").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn evict_cleans_legacy_root_files_and_removes_emptied_project_dirs() {
        let dir = temp_dir();
        let store = store(&dir).with_max_bytes(1000);
        // Another project with a single old file, and a legacy root-level file.
        let proj_b = dir.join("proj-b");
        fs::create_dir_all(&proj_b).unwrap();
        write_aged(
            &proj_b.join("slimcode-b.jsonl"),
            &[b'x'; 600],
            Duration::from_secs(5 * 3600),
        );
        write_aged(
            &dir.join("slimcode-legacy.json"),
            &[b'x'; 600],
            Duration::from_secs(4 * 3600),
        );
        // Current project: one old file, then an append triggers eviction.
        let proj_a = dir.join("proj-a");
        fs::create_dir_all(&proj_a).unwrap();
        write_aged(
            &proj_a.join("slimcode-a-old.jsonl"),
            &[b'x'; 600],
            Duration::from_secs(3 * 3600),
        );
        let mut session = session_with("slimcode-a-new");
        let asst = AgentMessage::text(Role::Assistant, "hi");
        session.messages.push(asst.clone());
        store.append(&session, &asst).unwrap();
        // All three old files (1800 bytes) are evicted, the active one stays,
        // and the emptied proj-b directory is removed.
        assert!(!proj_b.join("slimcode-b.jsonl").exists());
        assert!(!dir.join("slimcode-legacy.json").exists());
        assert!(!proj_a.join("slimcode-a-old.jsonl").exists());
        assert!(proj_a.join("slimcode-a-new.jsonl").exists());
        assert!(
            !proj_b.exists(),
            "emptied project directory should be removed"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn evict_skips_when_under_threshold() {
        let dir = temp_dir();
        let store = store(&dir).with_max_bytes(10_000);
        let proj = dir.join("proj-a");
        fs::create_dir_all(&proj).unwrap();
        write_aged(
            &proj.join("slimcode-old.jsonl"),
            &[b'x'; 100],
            Duration::from_secs(3600),
        );
        let mut session = session_with("slimcode-new");
        let asst = AgentMessage::text(Role::Assistant, "hi");
        session.messages.push(asst.clone());
        store.append(&session, &asst).unwrap();
        assert!(proj.join("slimcode-old.jsonl").exists());
        assert!(proj.join("slimcode-new.jsonl").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn evict_never_deletes_the_active_session_even_when_over() {
        let dir = temp_dir();
        // The smallest log is already larger than this threshold, so every
        // append overshoots; the active session must still be kept.
        let store = store(&dir).with_max_bytes(100);
        let mut session = session_with("slimcode-new");
        let asst = AgentMessage::text(Role::Assistant, "hi");
        session.messages.push(asst.clone());
        store.append(&session, &asst).unwrap();
        assert!(dir.join("proj-a/slimcode-new.jsonl").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
