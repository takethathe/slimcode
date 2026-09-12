//! Build the seven-tool set bound to a working directory.
//!
//! read / write / edit / bash / grep / find / ls — the v1 tool list from the
//! wayfinder destination. Each factory captures the cwd so the model operates
//! relative to the directory the CLI was launched in.

use std::path::Path;

use slimcode_core::agent::{CancelToken, Tool};
use slimcode_core::tools::files;

/// The six non-bash tools shared by the plain and the cancellable tool set
/// (the bash variant differs: cancellable for the TUI, plain for the CLI).
fn non_bash_tools(cwd: &Path) -> Vec<Tool> {
    vec![
        files::read_tool(cwd),
        files::write_tool(cwd),
        files::edit_tool(cwd),
        files::grep_tool(cwd),
        files::find_tool(cwd),
        files::ls_tool(cwd),
    ]
}

/// The full v1 tool set for a working directory.
pub fn build_tools(cwd: &Path) -> Vec<Tool> {
    let mut tools = non_bash_tools(cwd);
    tools.insert(3, files::bash_tool(cwd));
    tools
}

/// The cancellable tool set for the interactive TUI (ticket 07): identical to
/// [`build_tools`] except `bash` is the token-aware variant, so a long
/// running command observes an Esc cancel and kills its child promptly.
pub fn build_tools_with_cancel(cwd: &Path, cancel: &CancelToken) -> Vec<Tool> {
    let mut tools = non_bash_tools(cwd);
    tools.insert(3, files::bash_tool_with_cancel(cwd, cancel.clone()));
    tools
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::thread;
    use std::time::{Duration, Instant};

    #[test]
    fn build_tools_exposes_the_seven_tools() {
        let cwd = std::env::temp_dir();
        let tools = build_tools(&cwd);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["read", "write", "edit", "bash", "grep", "find", "ls"]
        );
        assert!(tools.iter().all(|t| !t.description.is_empty()));
    }

    #[test]
    fn build_tools_tools_run_relative_to_cwd() {
        let dir = std::env::temp_dir().join(format!("slimcode-tools-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("hello.txt"), "world").unwrap();

        let tools = build_tools(&dir);
        let read = tools.iter().find(|t| t.name == "read").unwrap();
        let out = (read.run)(serde_json::json!({"path": "hello.txt"})).unwrap();
        assert_eq!(out, "world");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_tool_slices_by_offset_and_limit_via_build_tools() {
        let dir = std::env::temp_dir().join(format!("slimcode-tools-slice-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("hello.txt"), "one\ntwo\nthree\nfour\nfive\n").unwrap();

        let tools = build_tools(&dir);
        let read = tools.iter().find(|t| t.name == "read").unwrap();
        // Full read is byte-identical to the file.
        let full = (read.run)(serde_json::json!({"path": "hello.txt"})).unwrap();
        assert_eq!(full, "one\ntwo\nthree\nfour\nfive\n");
        // A 1-indexed offset/limit window returns exactly those lines.
        let sliced =
            (read.run)(serde_json::json!({"path": "hello.txt", "offset": 3, "limit": 2})).unwrap();
        assert_eq!(sliced, "three\nfour");
        let _ = fs::remove_dir_all(&dir);
    }

    // --- cancellable tool set (ticket 07) ---------------------------------

    #[test]
    fn build_tools_with_cancel_exposes_the_seven_tools() {
        let cwd = std::env::temp_dir();
        let cancel = CancelToken::new();
        let tools = build_tools_with_cancel(&cwd, &cancel);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["read", "write", "edit", "bash", "grep", "find", "ls"]
        );
        // A cancel left unset must not change normal behavior.
        assert!(!cancel.is_cancelled());
    }

    #[test]
    fn cancellable_bash_kills_the_child_promptly_on_cancel() {
        let dir =
            std::env::temp_dir().join(format!("slimcode-tools-cancel-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let cancel = CancelToken::new();
        let tools = build_tools_with_cancel(&dir, &cancel);
        let bash = tools.into_iter().find(|t| t.name == "bash").unwrap();

        // Run `sleep 30` on a worker thread; cancel shortly after it starts.
        let handle =
            thread::spawn(move || (bash.run)(serde_json::json!({ "command": "sleep 30" })));
        thread::sleep(Duration::from_millis(300));
        let started = Instant::now();
        cancel.cancel();
        let result = handle.join().expect("tool returns");
        let elapsed = started.elapsed();

        // No 30s wait: the tool observes the cancel, kills the child group,
        // and returns promptly with a cancelled marker.
        assert!(
            elapsed < Duration::from_secs(5),
            "cancel must not wait for `sleep 30`: took {elapsed:?}"
        );
        assert!(
            matches!(&result, Err(e) if e.contains("cancelled")),
            "expected a cancelled marker, got: {result:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancellable_bash_still_returns_normal_output_when_not_cancelled() {
        let dir = std::env::temp_dir().join(format!("slimcode-tools-bash-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let cancel = CancelToken::new();
        let tools = build_tools_with_cancel(&dir, &cancel);
        let bash = tools.into_iter().find(|t| t.name == "bash").unwrap();
        let out = (bash.run)(serde_json::json!({ "command": "echo hi" })).unwrap();
        assert_eq!(out.trim(), "hi");
        assert!(!cancel.is_cancelled());
        let _ = fs::remove_dir_all(&dir);
    }
}
