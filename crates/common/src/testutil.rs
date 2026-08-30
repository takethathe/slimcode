//! Test-only helpers shared across the workspace's test modules (exposed as
//! `#[doc(hidden)]` so both this crate's tests and cli's tests can use them).

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A unique per-test temp directory (`<prefix>-<pid>-<n>` under the system
/// temp dir). Tests run in parallel, so each must own its directory; the
/// directory is removed if it already exists.
pub fn unique_temp_dir(prefix: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("{prefix}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}
