//! Test-only helpers shared by the CLI's test modules.

use std::io::Write;
use std::sync::{Arc, Mutex};

/// A `Write` into a shared buffer, so a test can read back what `run` or a
/// renderer wrote after the boxed writer was moved in. A poisoned lock is
/// recovered from (`into_inner`): a panicking thread must not turn another
/// assertion's output into a second panic.
#[derive(Clone, Default)]
pub struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl SharedBuf {
    /// The bytes written so far, decoded as UTF-8.
    pub fn contents(&self) -> String {
        let bytes = self.0.lock().unwrap_or_else(|e| e.into_inner()).clone();
        String::from_utf8(bytes).expect("buffered output is utf-8")
    }
}

impl Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
