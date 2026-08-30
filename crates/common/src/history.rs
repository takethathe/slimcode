//! Input history persistence: `<home>/history.json`.
//!
//! Frontend-agnostic: mirrors the `session` module pattern, a `HistoryStore`
//! backed by a single JSON file (an array of prompt strings). Records only
//! ordinary prompts (never `/` commands), capped at `HISTORY_LIMIT` with the
//! oldest dropped. See CONTEXT.md: `input history` is distinct from `message
//! history` (`session.messages`).

use std::fs;
use std::path::PathBuf;

/// Maximum number of entries kept on disk (oldest dropped beyond this).
pub const HISTORY_LIMIT: usize = 500;
/// Maximum number of entries shown by `/history`.
pub const HISTORY_DISPLAY: usize = 20;

/// A file-backed store of input history entries.
pub struct HistoryStore {
    path: PathBuf,
}

impl HistoryStore {
    /// Build a store rooted at `path` (the history JSON file itself).
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Load all entries; a missing file yields an empty list.
    pub fn load(&self) -> Result<Vec<String>, String> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let json =
            fs::read_to_string(&self.path).map_err(|e| format!("{}: {e}", self.path.display()))?;
        serde_json::from_str(&json).map_err(|e| format!("{}: {e}", self.path.display()))
    }

    /// Append one entry, trim to the limit, and persist.
    pub fn append(&self, entry: &str) -> Result<(), String> {
        let mut entries = self.load()?;
        entries.push(entry.to_string());
        trim_to_limit(&mut entries, HISTORY_LIMIT);
        self.save(&entries)
    }

    fn save(&self, entries: &[String]) -> Result<(), String> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let json =
            serde_json::to_string_pretty(entries).map_err(|e| format!("serialize history: {e}"))?;
        fs::write(&self.path, json).map_err(|e| format!("{}: {e}", self.path.display()))?;
        Ok(())
    }
}

/// Drop the oldest entries so at most `limit` remain.
pub fn trim_to_limit(entries: &mut Vec<String>, limit: usize) {
    if entries.len() > limit {
        let excess = entries.len() - limit;
        entries.drain(0..excess);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::unique_temp_dir;
    use std::fs;

    #[test]
    fn trim_to_limit_drops_oldest() {
        let mut v = vec!["a".into(), "b".into(), "c".into()];
        trim_to_limit(&mut v, 2);
        assert_eq!(v, vec!["b".to_string(), "c".to_string()]);
        // Within limit: unchanged.
        trim_to_limit(&mut v, 10);
        assert_eq!(v, vec!["b".to_string(), "c".to_string()]);
    }

    #[test]
    fn load_missing_file_is_empty() {
        let dir = unique_temp_dir("slimcode-history");
        let store = HistoryStore::new(dir.join("history.json"));
        assert_eq!(store.load().unwrap(), Vec::<String>::new());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_and_load_roundtrips() {
        let dir = unique_temp_dir("slimcode-history");
        let store = HistoryStore::new(dir.join("history.json"));
        store.append("hello").unwrap();
        store.append("fix it\nnow").unwrap(); // multi-line entry, verbatim
        assert_eq!(store.load().unwrap(), vec!["hello", "fix it\nnow"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_creates_parent_dir() {
        let dir = unique_temp_dir("slimcode-history");
        let store = HistoryStore::new(dir.join("nested").join("history.json"));
        store.append("hello").unwrap();
        assert!(dir.join("nested").join("history.json").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_keeps_duplicates() {
        // No dedup: every submitted prompt is recorded (spec: predictable).
        let dir = unique_temp_dir("slimcode-history");
        let store = HistoryStore::new(dir.join("history.json"));
        store.append("hello").unwrap();
        store.append("hello").unwrap();
        assert_eq!(store.load().unwrap(), vec!["hello", "hello"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_caps_at_limit_dropping_oldest() {
        let dir = unique_temp_dir("slimcode-history");
        let store = HistoryStore::new(dir.join("history.json"));
        let total = HISTORY_LIMIT + 10;
        for i in 0..total {
            store.append(&format!("prompt-{i}")).unwrap();
        }
        let entries = store.load().unwrap();
        assert_eq!(entries.len(), HISTORY_LIMIT);
        // Oldest dropped: the first surviving entry is prompt-10.
        assert_eq!(entries.first().map(|s| s.as_str()), Some("prompt-10"));
        let last_expected = format!("prompt-{}", total - 1);
        assert_eq!(
            entries.last().map(|s| s.as_str()),
            Some(last_expected.as_str())
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
