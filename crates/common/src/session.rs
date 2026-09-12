//! Session persistence: `~/.slimcode/sessions/<project-key>/<id>.json`.
//!
//! Sessions are partitioned per project: the store is rooted at the sessions
//! directory plus a project key derived from the project home path (git root,
//! falling back to the OS user home), so `/load` and `/sessions` only ever see
//! the current project's sessions. Disk use is bounded two ways: a startup
//! sweep drops the current project's empty sessions, and every save enforces a
//! whole-store byte quota by evicting oldest-first down to half the quota. The
//! frontend-agnostic metadata decisions live here: id =
//! `slimcode-<unix>-<pid>-<n>`, `created_at` = RFC3339 UTC (no chrono dep;
//! civil-from-days below), and the title is inferred from the first user
//! message (truncated to 48 chars).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use slimcode_agent::session::{Message, Role, Session};

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

/// True when `path` is a regular session file (a plain `.json` file, not a
/// directory or other entry that happens to end in `.json`).
fn is_session_file(path: &Path) -> bool {
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
pub fn infer_title(messages: &[Message]) -> Option<String> {
    let text = messages
        .iter()
        .find(|m| m.role == Role::User)
        .map(Message::text_content)?;
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

/// A project-scoped, directory-backed session store
/// (`<base>/<project-key>/<id>.json`).
pub struct SessionStore {
    base: PathBuf,
    project: String,
    /// Whole-store byte quota; `save` evicts oldest sessions past this.
    max_bytes: u64,
    next_id: AtomicUsize,
}

impl SessionStore {
    /// Build a store rooted at `base` (the sessions directory itself) and
    /// scoped to the given project key, with the default quota.
    pub fn new(base: impl Into<PathBuf>, project: impl Into<String>) -> Self {
        Self {
            base: base.into(),
            project: project.into(),
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
        Ok(self.base.join(&self.project).join(format!("{id}.json")))
    }

    /// Persist a session as pretty JSON, creating the project directory if
    /// needed, then enforce the store quota (best-effort; never fails the save).
    pub fn save(&self, session: &Session) -> Result<PathBuf, String> {
        let path = self.session_path(&session.id)?;
        let parent = path
            .parent()
            .ok_or_else(|| format!("no parent for {}", path.display()))?;
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        let json =
            serde_json::to_string_pretty(session).map_err(|e| format!("serialize session: {e}"))?;
        fs::write(&path, json).map_err(|e| format!("{}: {e}", path.display()))?;
        self.evict_over_quota(&session.id);
        Ok(path)
    }

    /// Best-effort quota enforcement: once the whole sessions directory exceeds
    /// `max_bytes`, delete session files oldest-first by mtime (skipping the
    /// active session) until the total is at or below `max_bytes / 2`. Legacy
    /// root-level files participate, and directories emptied by eviction are
    /// removed. Failures are ignored so a storage hiccup never breaks `save`.
    fn evict_over_quota(&self, keep: &str) {
        let mut files = Vec::new();
        let mut total = 0u64;
        walk_json(&self.base, &mut files, &mut total);
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

    /// Load a session by id. A missing id reports that it is absent from the
    /// current project (rather than exposing a raw path error), since sessions
    /// of other projects and legacy root files are deliberately invisible.
    pub fn load(&self, id: &str) -> Result<Session, String> {
        let path = self.session_path(id)?;
        let json = fs::read_to_string(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                format!("session {id:?} not found in project {:?}", self.project)
            } else {
                format!("{}: {e}", path.display())
            }
        })?;
        serde_json::from_str(&json).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// List this project's session ids (stable-sorted).
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
                if !is_session_file(&path) {
                    return None;
                }
                name.strip_suffix(".json").map(str::to_string)
            })
            .collect();
        ids.sort();
        Ok(ids)
    }

    /// Delete empty session files in this project's directory and return how
    /// many were removed. Empty means: a session whose `messages` is empty, a
    /// zero-byte file, or a JSON file that fails to parse. Unreadable files are
    /// kept (removal is best-effort and silent; callers decide what to
    /// surface). Other projects and legacy root-level files are out of scope.
    pub fn cleanup_empty(&self) -> Result<usize, String> {
        let dir = self.base.join(&self.project);
        if !dir.exists() {
            return Ok(0);
        }
        let mut removed = 0;
        let entries = fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if !is_session_file(&path) {
                continue;
            }
            // A read failure keeps the file; a parse failure (including a
            // zero-byte file) or empty messages marks it for removal.
            let empty = match fs::read(&path) {
                Ok(bytes) => serde_json::from_slice::<Session>(&bytes)
                    .map(|s| s.messages.is_empty())
                    .unwrap_or(true),
                Err(_) => false,
            };
            if empty && fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }
}

/// Collect every `.json` file under `dir` (recursing into subdirectories) as
/// `(path, size, mtime)`; `total` accumulates the byte sum. mtime read failures
/// are treated as the oldest (evicted first) so a quota run still makes
/// progress.
fn walk_json(dir: &Path, out: &mut Vec<(PathBuf, u64, std::time::SystemTime)>, total: &mut u64) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_json(&path, out, total);
            continue;
        }
        if !is_session_file(&path) {
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
    use std::time::Duration;

    /// Unique temp dir per test (tests run in parallel and must not share).
    fn temp_dir() -> PathBuf {
        unique_temp_dir("slimcode-session-test")
    }

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
            Message::text(Role::System, "be helpful"),
            Message::text(Role::User, "short prompt"),
            Message::text(Role::User, "second"),
        ];
        assert_eq!(infer_title(&msgs).unwrap(), "short prompt");
    }

    #[test]
    fn title_truncates_long_prompts() {
        let long = "x".repeat(100);
        let msgs = vec![Message::text(Role::User, &long)];
        let t = infer_title(&msgs).unwrap();
        assert!(t.chars().count() <= TITLE_MAX + 1); // +1 for ellipsis
        assert!(t.ends_with('…'));
    }

    #[test]
    fn title_none_without_user_message() {
        let msgs = vec![Message::text(Role::System, "sys")];
        assert!(infer_title(&msgs).is_none());
    }

    #[test]
    fn new_id_is_unique_and_prefixed() {
        let s = SessionStore::new(std::env::temp_dir(), "proj-test");
        let a = s.new_id();
        let b = s.new_id();
        assert_ne!(a, b);
        assert!(a.starts_with("slimcode-"));
    }

    #[test]
    fn new_session_has_fresh_id_and_empty_messages() {
        let s = SessionStore::new(std::env::temp_dir(), "proj-test");
        let session = s.new_session();
        assert!(session.id.starts_with("slimcode-"));
        assert!(session.messages.is_empty());
        assert!(session.title.is_none());
        assert!(!session.created_at.is_empty());
    }

    #[test]
    fn session_round_trips_through_store() {
        let dir = temp_dir();
        let store = SessionStore::new(&dir, "proj-test");
        let mut session = Session {
            id: store.new_id(),
            created_at: now_rfc3339(),
            title: Some("hello".to_string()),
            messages: vec![
                Message::text(Role::System, "be helpful"),
                Message::text(Role::User, "hi"),
                Message::text(Role::Assistant, "hey!"),
            ],
        };
        let path = store.save(&session).unwrap();
        assert!(path.exists());
        let loaded = store.load(&session.id).unwrap();
        assert_eq!(loaded.messages, session.messages);
        // mutate and reload from disk
        session.messages.push(Message::text(Role::User, "again"));
        store.save(&session).unwrap();
        let loaded2 = store.load(&session.id).unwrap();
        assert_eq!(loaded2.messages.len(), 4);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_returns_sorted_ids() {
        let dir = temp_dir();
        let store = SessionStore::new(&dir, "proj-test");
        let s1 = Session {
            id: "slimcode-1-a".to_string(),
            created_at: now_rfc3339(),
            title: None,
            messages: vec![],
        };
        let s2 = Session {
            id: "slimcode-2-b".to_string(),
            created_at: now_rfc3339(),
            title: None,
            messages: vec![],
        };
        store.save(&s1).unwrap();
        store.save(&s2).unwrap();
        let ids = store.list().unwrap();
        assert_eq!(ids, vec!["slimcode-1-a", "slimcode-2-b"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_empty_when_dir_missing() {
        let dir = temp_dir();
        let store = SessionStore::new(&dir, "proj-test");
        assert!(store.list().unwrap().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_id_rejected() {
        let store = SessionStore::new(std::env::temp_dir(), "proj-test");
        assert!(store.session_path("../evil").is_err());
        assert!(store.session_path("a/b").is_err());
        assert!(store.session_path("").is_err());
    }

    // --- ticket 01: project-scoped layout + isolation ----------------------

    /// A minimal session with the given id, ready to save.
    fn session_with(id: &str) -> Session {
        Session {
            id: id.to_string(),
            created_at: now_rfc3339(),
            title: None,
            messages: vec![Message::text(Role::User, "hi")],
        }
    }

    /// An empty session (no messages) with the given id.
    fn empty_session(id: &str) -> Session {
        Session {
            id: id.to_string(),
            created_at: now_rfc3339(),
            title: None,
            messages: vec![],
        }
    }

    #[test]
    fn save_writes_into_project_subdirectory() {
        let dir = temp_dir();
        let store = SessionStore::new(&dir, "proj-a");
        let path = store.save(&session_with("slimcode-1-a")).unwrap();
        assert_eq!(path, dir.join("proj-a/slimcode-1-a.json"));
        assert!(dir.join("proj-a/slimcode-1-a.json").exists());
        // No root-level file: the store never scatters sessions at the base.
        assert!(!dir.join("slimcode-1-a.json").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_is_isolated_to_current_project() {
        let dir = temp_dir();
        // Legacy root-level file (old flat layout) must not be loadable.
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("slimcode-old-1.json"), "{}").unwrap();
        // Another project's file must not be loadable either.
        let other = SessionStore::new(&dir, "proj-b");
        other.save(&session_with("slimcode-2-b")).unwrap();
        // The current project's own file loads fine.
        let store = SessionStore::new(&dir, "proj-a");
        store.save(&session_with("slimcode-3-a")).unwrap();
        assert!(store.load("slimcode-3-a").is_ok());
        assert!(store.load("slimcode-old-1").is_err());
        assert!(store.load("slimcode-2-b").is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_only_current_project() {
        let dir = temp_dir();
        let a = SessionStore::new(&dir, "proj-a");
        let b = SessionStore::new(&dir, "proj-b");
        a.save(&session_with("slimcode-1-a")).unwrap();
        b.save(&session_with("slimcode-2-b")).unwrap();
        a.save(&session_with("slimcode-3-a")).unwrap();
        assert_eq!(a.list().unwrap(), vec!["slimcode-1-a", "slimcode-3-a"]);
        assert_eq!(b.list().unwrap(), vec!["slimcode-2-b"]);
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

    #[test]
    fn load_missing_id_reports_project_scope() {
        let dir = temp_dir();
        let store = SessionStore::new(&dir, "proj-a");
        let err = store.load("slimcode-absent").unwrap_err();
        assert!(err.contains("slimcode-absent"), "err: {err}");
        assert!(err.contains("proj-a"), "err: {err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_ignores_a_directory_named_like_a_session_file() {
        let dir = temp_dir();
        let store = SessionStore::new(&dir, "proj-a");
        store.save(&session_with("slimcode-real")).unwrap();
        // A directory whose name ends in `.json` must not be listed as a
        // session (it could not be loaded anyway).
        fs::create_dir_all(dir.join("proj-a/slimcode-fake.json")).unwrap();
        assert_eq!(store.list().unwrap(), vec!["slimcode-real"]);
        let _ = fs::remove_dir_all(&dir);
    }

    // --- ticket 02: startup empty-session cleanup --------------------------

    #[test]
    fn cleanup_empty_removes_empty_zero_byte_and_corrupt_but_keeps_nonempty() {
        let dir = temp_dir();
        let store = SessionStore::new(&dir, "proj-a");
        store.save(&empty_session("slimcode-empty-1")).unwrap();
        store.save(&session_with("slimcode-keep-1")).unwrap();
        // Zero-byte and corrupt JSON files written directly.
        let proj = dir.join("proj-a");
        fs::write(proj.join("slimcode-zerobyte.json"), "").unwrap();
        fs::write(proj.join("slimcode-corrupt.json"), "{ not json").unwrap();
        let removed = store.cleanup_empty().unwrap();
        assert_eq!(removed, 3);
        assert!(!proj.join("slimcode-empty-1.json").exists());
        assert!(!proj.join("slimcode-zerobyte.json").exists());
        assert!(!proj.join("slimcode-corrupt.json").exists());
        assert!(proj.join("slimcode-keep-1.json").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cleanup_empty_scopes_to_current_project() {
        let dir = temp_dir();
        let a = SessionStore::new(&dir, "proj-a");
        let b = SessionStore::new(&dir, "proj-b");
        a.save(&empty_session("slimcode-empty-a")).unwrap();
        b.save(&session_with("slimcode-keep-b")).unwrap();
        // Legacy root-level file must be untouched by this pass.
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("slimcode-old-empty.json"), "{}").unwrap();
        let removed = a.cleanup_empty().unwrap();
        assert_eq!(removed, 1);
        assert!(!dir.join("proj-a/slimcode-empty-a.json").exists());
        assert!(dir.join("proj-b/slimcode-keep-b.json").exists());
        assert!(dir.join("slimcode-old-empty.json").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cleanup_empty_is_noop_when_dir_missing() {
        let dir = temp_dir();
        let store = SessionStore::new(&dir, "proj-a");
        assert_eq!(store.cleanup_empty().unwrap(), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    // --- ticket 03: quota eviction on save ---------------------------------

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

    #[test]
    fn evict_deletes_oldest_first_down_to_half_and_keeps_active() {
        let dir = temp_dir();
        let store = SessionStore::new(&dir, "proj-a").with_max_bytes(1000);
        let proj = dir.join("proj-a");
        fs::create_dir_all(&proj).unwrap();
        write_aged(
            &proj.join("slimcode-old1.json"),
            &[b'x'; 450],
            Duration::from_secs(3 * 3600),
        );
        write_aged(
            &proj.join("slimcode-old2.json"),
            &[b'x'; 450],
            Duration::from_secs(2 * 3600),
        );
        write_aged(
            &proj.join("slimcode-old3.json"),
            &[b'x'; 100],
            Duration::from_secs(3600),
        );
        store.save(&session_with("slimcode-new")).unwrap(); // triggers eviction
        // Oldest two (450+450) evicted to get the total ≤ half (500); the
        // smallest old file and the just-saved active session survive.
        assert!(!proj.join("slimcode-old1.json").exists());
        assert!(!proj.join("slimcode-old2.json").exists());
        assert!(proj.join("slimcode-old3.json").exists());
        assert!(proj.join("slimcode-new.json").exists());
        let total: u64 = fs::read_dir(&proj)
            .unwrap()
            .flatten()
            .map(|e| e.metadata().unwrap().len())
            .sum();
        assert!(total <= 500, "total after eviction: {total}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn evict_cleans_legacy_root_files_and_removes_emptied_project_dirs() {
        let dir = temp_dir();
        let store = SessionStore::new(&dir, "proj-a").with_max_bytes(1000);
        // Another project with a single old file, and a legacy root-level file.
        let proj_b = dir.join("proj-b");
        fs::create_dir_all(&proj_b).unwrap();
        write_aged(
            &proj_b.join("slimcode-b.json"),
            &[b'x'; 600],
            Duration::from_secs(5 * 3600),
        );
        write_aged(
            &dir.join("slimcode-legacy.json"),
            &[b'x'; 600],
            Duration::from_secs(4 * 3600),
        );
        // Current project: one old file, then a save triggers eviction.
        let proj_a = dir.join("proj-a");
        fs::create_dir_all(&proj_a).unwrap();
        write_aged(
            &proj_a.join("slimcode-a-old.json"),
            &[b'x'; 600],
            Duration::from_secs(3 * 3600),
        );
        store.save(&session_with("slimcode-a-new")).unwrap();
        // All three old files (1800 bytes) are evicted, the active one stays,
        // and the emptied proj-b directory is removed.
        assert!(!proj_b.join("slimcode-b.json").exists());
        assert!(!dir.join("slimcode-legacy.json").exists());
        assert!(!proj_a.join("slimcode-a-old.json").exists());
        assert!(proj_a.join("slimcode-a-new.json").exists());
        assert!(
            !proj_b.exists(),
            "emptied project directory should be removed"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn evict_skips_when_under_threshold() {
        let dir = temp_dir();
        let store = SessionStore::new(&dir, "proj-a").with_max_bytes(10_000);
        let proj = dir.join("proj-a");
        fs::create_dir_all(&proj).unwrap();
        write_aged(
            &proj.join("slimcode-old.json"),
            &[b'x'; 100],
            Duration::from_secs(3600),
        );
        store.save(&session_with("slimcode-new")).unwrap();
        assert!(proj.join("slimcode-old.json").exists());
        assert!(proj.join("slimcode-new.json").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn evict_never_deletes_the_active_session_even_when_over() {
        let dir = temp_dir();
        // The smallest session JSON is already larger than this threshold, so
        // every save overshoots; the active session must still be kept.
        let store = SessionStore::new(&dir, "proj-a").with_max_bytes(100);
        store.save(&session_with("slimcode-new")).unwrap();
        assert!(dir.join("proj-a/slimcode-new.json").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
