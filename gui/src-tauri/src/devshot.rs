//! Debug-only webview screenshot for the GUI frontend: the development
//! feedback loop that lets an agent (or a human) see what the webview actually
//! renders.
//!
//! Tauri itself has no screenshot API (`WebviewWindow` exposes window / icon /
//! theme controls only), and macOS WKWebView's Web Inspector protocol has no
//! capture command (that is a Chrome DevTools Protocol feature, i.e. Windows
//! WebView2). The one real path on macOS is
//! `WKWebView takeSnapshotWithConfiguration:completionHandler:`, reached
//! through Tauri's documented escape hatch `WebviewWindow::with_webview`. That
//! closure is dispatched to the main thread by Tauri itself, so it must be
//! called from a worker thread and never from inside `run_on_main_thread`,
//! which would deadlock waiting for an event loop it is itself blocking.
//!
//! Protocol: one fixed directory (default `/tmp/slimcode-gui-dev`, overridable
//! with `SLIMCODE_DEVSHOT_DIR` so tests never touch the real one) and three
//! files:
//!
//! - `trigger` - a file whose appearance asks for exactly one capture
//! - `shot.png` - the result, published atomically (tmp file + rename)
//! - `shot.error` - the failure reason, when the capture itself failed
//!
//! A caller does `rm -f shot.png shot.error; touch trigger` and then waits for
//! `shot.png`: presence is the success signal, `shot.error` is the failure
//! signal, and neither can ever be observed half-written.
//!
//! The module is compiled only under `cfg(all(debug_assertions,
//! target_os = "macos"))` (see `lib.rs`): release builds and non-macOS hosts
//! contain none of it. Nothing here is reachable from the frontend or from the
//! agent's tool set. The picture is for the development harness, not for the
//! model, whose wire messages carry text parts only (`slimcode-ai`'s
//! `Part::Text`).

use std::io;
use std::path::PathBuf;
use std::time::Duration;
use std::{fs, thread};

use block2::RcBlock;
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage};
use objc2_foundation::{NSDictionary, NSError};
use objc2_web_kit::WKWebView;
use tauri::Manager;

/// The capture directory when `SLIMCODE_DEVSHOT_DIR` is unset.
pub const DEFAULT_DIR: &str = "/tmp/slimcode-gui-dev";
/// The file whose appearance requests one capture.
pub const TRIGGER: &str = "trigger";
/// The published screenshot.
pub const SHOT: &str = "shot.png";
/// The capture failure reason, when the capture failed.
pub const ERROR: &str = "shot.error";
/// The in-progress write target, renamed onto [`SHOT`] when complete.
pub const SHOT_TMP: &str = "shot.png.tmp";

/// Resolve the capture directory: the `SLIMCODE_DEVSHOT_DIR` value when
/// present, [`DEFAULT_DIR`] otherwise. Pure (the value arrives as an argument),
/// so tests need no process-global environment mutation.
pub fn resolve_dir(env: Option<&str>) -> PathBuf {
    match env {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from(DEFAULT_DIR),
    }
}

/// One pending capture: the directory it publishes into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shot {
    /// Monotonic capture counter, used in the log line only.
    pub index: u32,
    pub dir: PathBuf,
}

impl Shot {
    /// Where the PNG bytes are written before being published.
    pub fn tmp_path(&self) -> PathBuf {
        self.dir.join(SHOT_TMP)
    }

    /// The published screenshot path.
    pub fn final_path(&self) -> PathBuf {
        self.dir.join(SHOT)
    }

    /// Where the failure reason goes.
    pub fn error_path(&self) -> PathBuf {
        self.dir.join(ERROR)
    }

    /// Publish the written tmp file as the result and report its path. A
    /// missing tmp file means the capture produced no bytes and surfaces as an
    /// `io::Error`, exactly like a failed rename would.
    pub fn publish(&self) -> io::Result<PathBuf> {
        let tmp = self.tmp_path();
        let final_path = self.final_path();
        fs::rename(&tmp, &final_path)?;
        Ok(final_path)
    }

    /// Record why the capture failed, so the caller is not left staring at a
    /// missing file until its own timeout.
    pub fn fail(&self, message: &str) {
        // Best effort: if the note cannot be written the caller's only signal
        // is the screenshot that never arrives.
        let _ = fs::write(self.error_path(), message);
    }
}

/// The trigger-file state machine, driven from the watcher thread.
pub struct Watcher {
    pub dir: PathBuf,
    shots: u32,
}

impl Watcher {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir, shots: 0 }
    }

    /// Consume a pending trigger: remove the trigger and any stale result, so
    /// that a later `shot.png` can only be this capture's output. Returns
    /// `None` when no trigger is pending.
    pub fn take_trigger(&mut self) -> Option<Shot> {
        let trigger = self.dir.join(TRIGGER);
        if !trigger.is_file() {
            return None;
        }
        // Claim the trigger first: two concurrent watchers then never both
        // capture for one request.
        if fs::remove_file(&trigger).is_err() {
            return None;
        }
        let _ = fs::remove_file(self.dir.join(SHOT));
        let _ = fs::remove_file(self.dir.join(ERROR));
        self.shots += 1;
        Some(Shot {
            index: self.shots,
            dir: self.dir.clone(),
        })
    }
}

/// Turn one `NSImage` pointer (or the `NSError` explaining its absence) into
/// PNG bytes. The image argument is autoreleased, so it is borrowed here and
/// never retained or released.
unsafe fn png_bytes(image: *mut NSImage, error: *mut NSError) -> Result<Vec<u8>, String> {
    if image.is_null() {
        let reason = if error.is_null() {
            "snapshot returned no image".to_string()
        } else {
            format!(
                "snapshot failed: {}",
                unsafe { &*error }.localizedDescription()
            )
        };
        return Err(reason);
    }
    // NSImage has no PNG representation of its own: go through TIFF, which any
    // NSBitmapImageRep can re-encode from.
    let image = unsafe { &*image };
    let tiff = image
        .TIFFRepresentation()
        .ok_or_else(|| "snapshot produced no TIFF representation".to_string())?;
    let rep = NSBitmapImageRep::imageRepWithData(&tiff)
        .ok_or_else(|| "could not decode the snapshot TIFF".to_string())?;
    let properties = NSDictionary::new();
    let png = unsafe {
        rep.representationUsingType_properties(NSBitmapImageFileType::PNG, properties.as_ref())
    }
    .ok_or_else(|| "could not encode the snapshot as PNG".to_string())?;
    Ok(png.to_vec())
}

/// Ask the main webview for one snapshot. Returns immediately: the PNG appears
/// when WebKit calls the completion handler back on the main thread.
///
/// `with_webview` posts its closure to the main thread itself, which is why the
/// watcher calls it from its own thread; calling it while already on the main
/// thread would deadlock.
fn request_capture(app: &tauri::AppHandle, shot: Shot) {
    let Some(webview) = app.get_webview_window("main") else {
        shot.fail("no webview labelled `main` to capture");
        return;
    };
    let reported = shot.clone();
    let outcome = webview.with_webview(move |platform| unsafe {
        let view: &WKWebView = &*platform.inner().cast();
        let block = RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
            finish(&shot, png_bytes(image, error));
        });
        view.takeSnapshotWithConfiguration_completionHandler(None, &block);
    });
    if let Err(e) = outcome {
        reported.fail(&format!("cannot reach the webview: {e}"));
    }
}

/// Write, then publish, one capture outcome. Split out of the completion block
/// so the failure reporting is one place.
fn finish(shot: &Shot, png: Result<Vec<u8>, String>) {
    let bytes = match png {
        Ok(bytes) => bytes,
        Err(message) => return shot.fail(&message),
    };
    if let Err(e) = fs::write(shot.tmp_path(), &bytes) {
        return shot.fail(&format!("cannot write the snapshot: {e}"));
    }
    match shot.publish() {
        Ok(path) => eprintln!("[devshot] shot #{} -> {}", shot.index, path.display()),
        Err(e) => shot.fail(&format!("cannot publish the snapshot: {e}")),
    }
}

/// How often the watcher looks for a trigger. A debug facility, so latency
/// beats a `notify` dependency.
const POLL: Duration = Duration::from_millis(200);

/// Start the watcher thread; called from the Tauri `setup` hook in debug macOS
/// builds only. The thread is a daemon: it exits with the process.
pub fn spawn(app: tauri::AppHandle) {
    let configured = std::env::var("SLIMCODE_DEVSHOT_DIR").ok();
    let dir = resolve_dir(configured.as_deref());
    if let Err(e) = fs::create_dir_all(&dir) {
        eprintln!("[devshot] cannot create {}: {e}", dir.display());
        return;
    }
    eprintln!(
        "[devshot] debug screenshots: `touch {d}/trigger` writes `{d}/shot.png`",
        d = dir.display()
    );
    if let Err(e) = thread::Builder::new()
        .name("devshot".to_string())
        .spawn(move || {
            let mut watcher = Watcher::new(dir);
            loop {
                thread::sleep(POLL);
                if let Some(shot) = watcher.take_trigger() {
                    request_capture(&app, shot);
                }
            }
        })
    {
        eprintln!("[devshot] cannot start the watcher thread: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory private to one test: the watcher's protocol is a fixed set
    /// of filenames, so tests that shared a directory would consume each
    /// other's triggers.
    fn dir(name: &str) -> PathBuf {
        let root = std::env::temp_dir()
            .join(format!("slimcode-devshot-test-{}", std::process::id()))
            .join(name);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn resolve_dir_uses_env_override_when_present() {
        assert_eq!(resolve_dir(Some("/tmp/other")), PathBuf::from("/tmp/other"));
    }

    #[test]
    fn resolve_dir_falls_back_when_unset_or_empty() {
        let expected = PathBuf::from(DEFAULT_DIR);
        assert_eq!(resolve_dir(None), expected);
        assert_eq!(resolve_dir(Some("")), expected);
    }

    #[test]
    fn shot_paths_are_all_under_the_dir() {
        let shot = Shot {
            index: 1,
            dir: dir("paths"),
        };
        assert_eq!(shot.tmp_path(), shot.dir.join(SHOT_TMP));
        assert_eq!(shot.final_path(), shot.dir.join(SHOT));
        assert_eq!(shot.error_path(), shot.dir.join(ERROR));
    }

    #[test]
    fn take_trigger_is_quiet_when_nothing_is_pending() {
        let d = dir("take_trigger_is_quiet_when_nothing_is_pending");
        let _ = fs::remove_file(d.join(TRIGGER));
        let mut watcher = Watcher::new(d);
        assert!(watcher.take_trigger().is_none());
    }

    #[test]
    fn take_trigger_consumes_it_and_clears_stale_results() {
        let d = dir("take_trigger_consumes_it_and_clears_stale_results");
        fs::write(d.join(TRIGGER), "").unwrap();
        // A previous run's outputs must never read as this run's answer.
        fs::write(d.join(SHOT), b"stale").unwrap();
        fs::write(d.join(ERROR), "stale").unwrap();

        let mut watcher = Watcher::new(d.clone());
        let shot = watcher.take_trigger().expect("trigger pending");

        assert!(!d.join(TRIGGER).exists(), "trigger is consumed");
        assert!(!d.join(SHOT).exists(), "stale shot cleared");
        assert!(!d.join(ERROR).exists(), "stale error cleared");
        assert_eq!(shot.dir, d);
        assert_eq!(shot.index, 1);
        assert!(watcher.take_trigger().is_none(), "one trigger, one capture");
    }

    #[test]
    fn take_trigger_counts_captures_up() {
        let d = dir("take_trigger_counts_captures_up");
        let mut watcher = Watcher::new(d.clone());
        for expected in 1..=3u32 {
            fs::write(d.join(TRIGGER), "").unwrap();
            assert_eq!(watcher.take_trigger().unwrap().index, expected);
        }
    }

    #[test]
    fn publish_renames_the_tmp_file_onto_the_shot() {
        let d = dir("publish_renames_the_tmp_file_onto_the_shot");
        let shot = Shot { index: 9, dir: d };
        let _ = fs::remove_file(shot.final_path());
        fs::write(shot.tmp_path(), b"png-bytes").unwrap();

        assert_eq!(shot.publish().unwrap(), shot.final_path());
        assert_eq!(fs::read(shot.final_path()).unwrap(), b"png-bytes");
        assert!(!shot.tmp_path().exists(), "tmp consumed by the rename");
    }

    #[test]
    fn publish_without_bytes_is_an_error() {
        let d = dir("publish_without_bytes_is_an_error");
        let shot = Shot { index: 10, dir: d };
        let _ = fs::remove_file(shot.tmp_path());
        assert!(shot.publish().is_err());
    }

    #[test]
    fn finish_publishes_written_bytes() {
        let shot = Shot {
            index: 20,
            dir: dir("finish_ok"),
        };
        let _ = fs::remove_file(shot.final_path());
        let _ = fs::remove_file(shot.error_path());
        finish(&shot, Ok(b"fake-png".to_vec()));
        assert_eq!(fs::read(shot.final_path()).unwrap(), b"fake-png");
        assert!(!shot.error_path().exists());
    }

    #[test]
    fn finish_reports_a_failed_capture() {
        let shot = Shot {
            index: 21,
            dir: dir("finish_err"),
        };
        let _ = fs::remove_file(shot.final_path());
        finish(&shot, Err("snapshot returned no image".to_string()));
        assert!(!shot.final_path().exists(), "nothing to read as a success");
        assert_eq!(
            fs::read_to_string(shot.error_path()).unwrap(),
            "snapshot returned no image"
        );
    }

    #[test]
    fn fail_writes_the_reason_at_the_error_path() {
        let d = dir("fail_writes_the_reason_at_the_error_path");
        let shot = Shot { index: 11, dir: d };
        let _ = fs::remove_file(shot.error_path());
        shot.fail("snapshot returned no image");
        assert_eq!(
            fs::read_to_string(shot.error_path()).unwrap(),
            "snapshot returned no image"
        );
    }
}
