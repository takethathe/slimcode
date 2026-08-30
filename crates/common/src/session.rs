//! Session persistence: `~/.slimcode/sessions/<id>.json`.
//!
//! Locked by grilling Q7 (JSON sessions) and ticket 04 (`Session` shape). The
//! frontend-agnostic metadata decisions live here: id =
//! `slimcode-<unix>-<pid>-<n>`, `created_at` = RFC3339 UTC (no chrono dep;
//! civil-from-days below), and the title is inferred from the first user
//! message (truncated to 48 chars).

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use slimcode_agent::session::{Message, Role, Session};

/// Max title length before truncation.
const TITLE_MAX: usize = 48;

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

/// A directory-backed session store (`<dir>/<id>.json`).
pub struct SessionStore {
    dir: PathBuf,
    next_id: AtomicUsize,
}

impl SessionStore {
    /// Build a store rooted at `dir` (the sessions directory itself).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            next_id: AtomicUsize::new(0),
        }
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

    /// Absolute path for a session id (id is validated to be a safe filename).
    pub fn session_path(&self, id: &str) -> Result<PathBuf, String> {
        if id.is_empty()
            || id.len() > 128
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(format!("invalid session id: {id:?}"));
        }
        Ok(self.dir.join(format!("{id}.json")))
    }

    /// Persist a session as pretty JSON, creating the directory if needed.
    pub fn save(&self, session: &Session) -> Result<PathBuf, String> {
        let path = self.session_path(&session.id)?;
        fs::create_dir_all(&self.dir).map_err(|e| format!("{}: {e}", self.dir.display()))?;
        let json =
            serde_json::to_string_pretty(session).map_err(|e| format!("serialize session: {e}"))?;
        fs::write(&path, json).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(path)
    }

    /// Load a session by id.
    pub fn load(&self, id: &str) -> Result<Session, String> {
        let path = self.session_path(id)?;
        let json = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        serde_json::from_str(&json).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// List all session ids (stable-sorted).
    pub fn list(&self) -> Result<Vec<String>, String> {
        if !self.dir.exists() {
            return Ok(Vec::new());
        }
        let mut ids: Vec<String> = fs::read_dir(&self.dir)
            .map_err(|e| format!("{}: {e}", self.dir.display()))?
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name.strip_suffix(".json").map(str::to_string)
            })
            .collect();
        ids.sort();
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::unique_temp_dir;

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
        let s = SessionStore::new(std::env::temp_dir());
        let a = s.new_id();
        let b = s.new_id();
        assert_ne!(a, b);
        assert!(a.starts_with("slimcode-"));
    }

    #[test]
    fn new_session_has_fresh_id_and_empty_messages() {
        let s = SessionStore::new(std::env::temp_dir());
        let session = s.new_session();
        assert!(session.id.starts_with("slimcode-"));
        assert!(session.messages.is_empty());
        assert!(session.title.is_none());
        assert!(!session.created_at.is_empty());
    }

    #[test]
    fn session_round_trips_through_store() {
        let dir = temp_dir();
        let store = SessionStore::new(&dir);
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
        let store = SessionStore::new(&dir);
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
        let store = SessionStore::new(&dir);
        assert!(store.list().unwrap().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_id_rejected() {
        let store = SessionStore::new(std::env::temp_dir());
        assert!(store.session_path("../evil").is_err());
        assert!(store.session_path("a/b").is_err());
        assert!(store.session_path("").is_err());
    }
}
