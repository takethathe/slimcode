//! The default tool set is cancellable and bound to the session's own cancel
//! token (ticket 01): `abort()` terminates a running bash child, so an
//! embedded host's cancel is complete — it kills the subprocess, not just the
//! provider read. Behavior-level: the scripted provider drives a `bash` call
//! that runs `sleep 30`; the test aborts from another thread and asserts the
//! child is gone (the turn ends `Cancelled` promptly, and the recorded pid no
//! longer names a live process).

mod common;
use common::*;

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use slimcode_app::StopReason;

use slimcode_sdk::{AgentSessionOptions, create_agent_session};

/// Wait until the bash tool's `sh` child wrote its own pid to `path`, and
/// return it. The child writes the pid before running `sleep 30`, so a pid on
/// disk means the tool is provably mid-command.
fn wait_for_pid(path: &Path) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(content) = fs::read_to_string(path)
            && let Ok(pid) = content.trim().parse::<u32>()
        {
            return pid;
        }
        assert!(
            Instant::now() < deadline,
            "bash tool never wrote its pid to {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Whether a pid still names a live process (`kill -0` succeeds).
fn process_alive(pid: u32) -> bool {
    std::process::Command::new("/bin/kill")
        .arg("-0")
        .arg(pid.to_string())
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// `abort()` during a `bash` tool run terminates the subprocess: the turn
/// ends `Cancelled` within seconds (no 30s wait), and the recorded `sh` pid
/// is no longer alive afterwards.
#[test]
fn abort_terminates_the_running_bash_child_of_the_default_tools() {
    let (_guard, _home, cwd) = test_env("cancel-bash");
    let pidfile = cwd.join("bash.pid");
    let command = format!("echo $$ > {}; sleep 30", pidfile.display());
    // The scripted model calls the default `bash` tool, then (had the tool
    // returned normally) would finish; the turn must instead end Cancelled.
    let args = serde_json::json!({ "command": command }).to_string();
    let script = vec![
        vec![
            tc_start(0, "call_1", "bash"),
            tc_args(0, &args),
            done_tools(),
        ],
        vec![text("done"), done_stop()],
    ];
    // No `.with_tools`: the default (cancellable) tool set is under test.
    let session = Arc::new(
        create_agent_session(
            AgentSessionOptions::new(Some(cwd))
                .with_api_key("sk-test")
                .with_base_url("https://example.invalid/v1")
                .with_model("test-model")
                .with_provider(Box::new(FakeProvider::new(script))),
        )
        .expect("create_agent_session"),
    );

    let session_thread = Arc::clone(&session);
    let handle = std::thread::spawn(move || session_thread.prompt("run the command"));
    // The bash tool spawned and is mid-`sleep 30` once the pid is on disk.
    let pid = wait_for_pid(&pidfile);

    let began = Instant::now();
    session.abort();
    let stop = handle.join().expect("prompt thread").expect("prompt");
    let elapsed = began.elapsed();

    // The turn ended cancelled promptly — no 30s wait — and the bash child is
    // actually gone: its pid no longer names a live process.
    assert_eq!(stop, StopReason::Cancelled);
    assert!(
        elapsed < Duration::from_secs(5),
        "abort must kill the bash child promptly, took {elapsed:?}"
    );
    assert!(
        !process_alive(pid),
        "the bash child (pid {pid}) must be terminated by abort"
    );
}
