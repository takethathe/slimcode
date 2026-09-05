//! Real-terminal smoke verification of the TUI against a mock SSE provider,
//! driven through tmux (ticket 04 acceptance; spec §Verification, tmux
//! rendering verification). Auto-skips when `tmux` is absent.
//!
//! The test stands up a local HTTP server speaking the Bailian wire format
//! (`wire::parse_stream`: `data: {chunk}` SSE events), launches the real
//! `slimcode` binary in a detached tmux pane pointed at the mock via
//! `SLIMCODE_AI_BASE_URL`, drives it with `send-keys`, and asserts on
//! `capture-pane` output: header, boxed prompt, thinking block, tool block
//! with real tool output, an animating spinner, the two-line footer, the `/`
//! completion popup, scroll, resize, the OSC 0 pane title, and Ctrl+C quit.
//!
//! Timing is poll-until-expected with a timeout; every capture hit is sought
//! before asserting so slow machines stay green.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

/// A scripted SSE response for the Bailian compatible-mode endpoint. Returns
/// the raw event payloads; the server prefixes `data: ` and sends one event
/// per payload, pausing between some so the CLI's spinner animates.
///
/// `tool`: when true this is turn one — reasoning, then partial text, then an
/// `ls` tool call ending with `finish_reason":"tool_calls"` so the agent
/// dispatches the tool and makes a second request. When false it is the
/// follow-up answer (with usage so the footer shows stats) ending in `"stop"`.
fn sse(tool: bool) -> Vec<String> {
    let mut out = Vec::new();
    if tool {
        out.push(
            "{\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"mock\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"reasoning_content\":\"Thinking about the directory...\",\"content\":\"\"},\"finish_reason\":null}]}".to_string(),
        );
        out.push(
            "{\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"mock\",\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"\",\"content\":\"Here is the listing: \"},\"finish_reason\":null}]}".to_string(),
        );
        out.push(
            "{\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"mock\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\",\"reasoning_content\":\"\",\"tool_calls\":[{\"id\":\"call_ls\",\"type\":\"function\",\"index\":0,\"function\":{\"name\":\"ls\",\"arguments\":\"{\\\"path\\\": \\\".\\\"}\"}}]},\"finish_reason\":null}]}".to_string(),
        );
        out.push(
            "{\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"mock\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"name\":null,\"arguments\":\"\"}}]},\"finish_reason\":\"tool_calls\"}]}".to_string(),
        );
    } else {
        out.push(
            "{\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"mock\",\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"\",\"content\":\"Second answer.\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1234,\"completion_tokens\":56,\"total_tokens\":1290,\"prompt_tokens_details\":{\"cached_tokens\":100,\"cache_creation_input_tokens\":0}}}".to_string(),
        );
    }
    out.push("[DONE]".to_string());
    out
}

/// A local HTTP server speaking the mock wire. Serves a fixed sequence of
/// responses (one per `chat` request). Chunks are written with pauses so the
/// CLI's spinner has frames to animate through while the body is in flight.
struct MockServer {
    addr: String,
    handle: Option<thread::JoinHandle<()>>,
}

impl MockServer {
    fn start(responses: Vec<Vec<String>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        // Non-blocking accept so the handler thread always exits and Drop's
        // join can never hang the test (see the accept loop below).
        listener.set_nonblocking(true).expect("set nonblocking");
        let addr = listener.local_addr().unwrap().to_string();
        let expected = responses.len();
        let handle = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(30);
            let mut served = 0;
            while served < expected && Instant::now() < deadline {
                let Ok((mut stream, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(50)); // poll tick
                    continue;
                };
                read_request(&mut stream);
                let payloads = responses[served].clone();
                served += 1;
                // Build the full body first so Content-Length is exact, then
                // write the response head and stream the events with pauses
                // so the CLI's 80ms spinner ticks show distinct frames
                // while it waits on the body.
                let body: String = payloads.iter().map(|p| format!("data: {p}\n\n")).collect();
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let mut sent = 0;
                for (i, payload) in payloads.iter().enumerate() {
                    let chunk = format!("data: {payload}\n\n");
                    let _ = stream.write_all(chunk.as_bytes());
                    let _ = stream.flush();
                    sent += chunk.len();
                    // 400ms between events keeps the run alive for ~2.5s so
                    // the test's captures (each spawns tmux) reliably land in
                    // several distinct 80ms spinner ticks.
                    if i < payloads.len() - 1 {
                        thread::sleep(Duration::from_millis(400));
                    }
                }
                debug_assert_eq!(sent, body.len());
                let _ = stream.shutdown(Shutdown::Both);
            }
        });
        MockServer {
            addr,
            handle: Some(handle),
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}/compatible-mode/v1", self.addr)
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Read the HTTP request head and, when a `Content-Length` is present, the
/// body too, so the client has finished sending before we respond.
fn read_request(stream: &mut TcpStream) {
    let mut buf = [0u8; 4096];
    let mut acc = Vec::new();
    loop {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                acc.extend_from_slice(&buf[..n]);
                let head = String::from_utf8_lossy(&acc);
                if let Some(pos) = head.find("\r\n\r\n") {
                    let rest = &head[pos + 4..];
                    let content_len = head
                        .lines()
                        .find_map(|l| {
                            l.strip_prefix("content-length:")
                                .or_else(|| l.strip_prefix("Content-Length:"))
                                .and_then(|v| v.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    let mut needed = content_len.saturating_sub(rest.len());
                    while needed > 0 {
                        match stream.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => needed = needed.saturating_sub(n),
                        }
                    }
                    break;
                }
                if acc.len() > 64 * 1024 {
                    break;
                }
            }
        }
    }
}

/// A running tmux session + pane with helpers.
struct Tmux {
    session: String,
}

impl Tmux {
    fn new() -> Self {
        // Unique per instance: the two smoke tests run concurrently in one
        // binary, so a bare pid (shared by both) would collide.
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let session = format!("slimcode-tmux-{}-{n}", std::process::id());
        // Clean any stale session from a crashed previous run.
        let _ = Command::new("tmux")
            .args(["kill-session", "-t", &session])
            .output();
        let out = Command::new("tmux")
            .args(["new-session", "-d", "-s", &session, "-x", "100", "-y", "34"])
            .output()
            .expect("tmux new-session runs");
        assert!(
            out.status.success(),
            "tmux new-session failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        Tmux { session }
    }

    fn send_keys(&self, keys: &str) {
        let out = Command::new("tmux")
            .args(["send-keys", "-t", &self.session, keys])
            .output()
            .unwrap();
        assert!(out.status.success());
    }

    fn send_literal(&self, text: &str) {
        let out = Command::new("tmux")
            .args(["send-keys", "-t", &self.session, "-l", text])
            .output()
            .unwrap();
        assert!(out.status.success());
    }

    fn capture(&self) -> String {
        let out = Command::new("tmux")
            .args(["capture-pane", "-t", &self.session, "-p"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    fn pane_title(&self) -> String {
        let out = Command::new("tmux")
            .args([
                "display-message",
                "-t",
                &self.session,
                "-p",
                "#{pane_title}",
            ])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn resize(&self, x: u16, y: u16) {
        let out = Command::new("tmux")
            .args([
                "resize-window",
                "-t",
                &self.session,
                "-x",
                &x.to_string(),
                "-y",
                &y.to_string(),
            ])
            .output()
            .unwrap();
        assert!(out.status.success());
    }

    /// Poll `capture().contains(needle)` until true or `timeout` lapses.
    fn wait_for(&self, needle: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            let cap = self.capture();
            if cap.contains(needle) {
                return true;
            }
            if Instant::now() > deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(120));
        }
    }

    fn quit(&self) {
        let _ = Command::new("tmux")
            .args(["kill-session", "-t", &self.session])
            .output();
    }
}

impl Drop for Tmux {
    fn drop(&mut self) {
        self.quit();
    }
}

/// Collapse a capture for readable failure messages (trim blank margins).
fn redact(cap: &str) -> String {
    cap.lines()
        .filter(|l| !l.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Launch the real `slimcode` binary in the tmux pane, pointed at the mock.
fn launch(mock: &MockServer, ui: &Tmux, cwd: &Path, home: &Path) {
    let bin = env!("CARGO_BIN_EXE_slimcode");
    let cmd = format!(
        "cd {} && SLIMCODE_HOME={} SLIMCODE_AI_MODEL=mock-model DASHSCOPE_API_KEY=test-key SLIMCODE_AI_BASE_URL={} {}",
        cwd.display(),
        home.display(),
        mock.base_url(),
        bin,
    );
    ui.send_literal(&cmd);
    ui.send_keys("Enter");
}

/// The full pi-style rendering smoke: every display surface the alignment
/// introduced, visible in real terminal captures.
#[test]
fn tmux_smoke_renders_pi_style_ui() {
    // Auto-skip when tmux is absent (CI without tmux).
    if Command::new("tmux").arg("-V").output().is_err() {
        eprintln!("skipping: tmux not available");
        return;
    }

    // A temp working dir with a known file, so the `ls` tool output is
    // deterministic (the mock calls `ls`); and a temp SLIMCODE_HOME so the
    // smoke never touches the real one.
    let dir = std::env::temp_dir().join(format!("slimcode-smoke-{}", std::process::id()));
    let home = std::env::temp_dir().join(format!("slimcode-home-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(dir.join("marker.txt"), "marker").unwrap();

    // Mock: turn 1 streams reasoning + text then asks for `ls` (the real tool
    // runs, then the agent makes turn 2); turn 2 gives the final answer.
    let mock = MockServer::start(vec![sse(true), sse(false)]);

    let ui = Tmux::new();
    launch(&mock, &ui, &dir, &home);

    // 1. Startup header (bold accent `slimcode` + dim hints line). Wait for
    // the hints line — `/help for commands` only exists inside the TUI,
    // whereas the echoed launch command already contains the bare word
    // "slimcode" (the binary path) before the TUI has drawn.
    assert!(
        ui.wait_for("/help for commands", Duration::from_secs(8)),
        "header not shown: {}",
        redact(&ui.capture())
    );
    assert!(ui.capture().contains("slimcode"), "name line not shown");

    // 2. Type a prompt and submit -> boxed user message.
    ui.send_literal("show me the files");
    ui.send_keys("Enter");
    assert!(
        ui.wait_for("show me the files", Duration::from_secs(3)),
        "prompt box not shown"
    );

    // 3. Thinking block (reasoning_content) + assistant text.
    assert!(
        ui.wait_for("Thinking about the directory", Duration::from_secs(8)),
        "thinking not shown: {}",
        redact(&ui.capture())
    );

    // 4. Tool block: compact call title (`ls .` — ticket 06) + real tool
    // output (marker.txt).
    assert!(
        ui.wait_for("ls .", Duration::from_secs(8)),
        "compact tool title not shown: {}",
        redact(&ui.capture())
    );
    assert!(
        ui.wait_for("marker.txt", Duration::from_secs(8)),
        "tool output not shown: {}",
        redact(&ui.capture())
    );

    // 5. Spinner row while running: "Working..." with frames that differ
    // between captures, then the final answer from turn 2.
    let spin_start = Instant::now();
    let mut frames: Vec<char> = vec![];
    let mut saw_working = false;
    loop {
        let cap = ui.capture();
        if cap.contains("Second answer.") {
            break;
        }
        for line in cap.lines() {
            if line.contains("Working") {
                saw_working = true;
                if let Some(ch) = line.trim_start().chars().next() {
                    frames.push(ch);
                }
            }
        }
        if Instant::now() - spin_start > Duration::from_secs(15) {
            break;
        }
        thread::sleep(Duration::from_millis(90));
    }
    assert!(
        saw_working,
        "never saw Working... spinner: {}",
        redact(&ui.capture())
    );
    let distinct: std::collections::HashSet<char> = frames.iter().copied().collect();
    assert!(
        frames.len() >= 3 && distinct.len() >= 2,
        "spinner should animate across frames: {:?}",
        frames
    );
    assert!(
        ui.capture().contains("Second answer."),
        "final answer not shown: {}",
        redact(&ui.capture())
    );

    // 6. Footer: two dim lines; line 2 has up-arrow stats and the model
    // right-aligned.
    let cap = ui.capture();
    let lines: Vec<&str> = cap.lines().collect();
    let h = lines.len();
    assert!(
        h >= 4,
        "expected footer + input rows, got {} lines: {}",
        h,
        redact(&cap)
    );
    assert!(
        lines[h - 2].contains(" • "),
        "footer line 1: {:?}",
        lines[h - 2]
    );
    assert!(
        lines[h - 1].contains("↑"),
        "footer line 2: {:?}",
        lines[h - 1]
    );
    assert!(
        lines[h - 1].trim_start().ends_with("mock-model"),
        "model right-aligned: {:?}",
        lines[h - 1]
    );

    // 7. `/` completion popup: opens with `→ ` cursor; Esc closes it.
    ui.send_keys("/");
    assert!(
        ui.wait_for("→ ", Duration::from_secs(3)),
        "completion popup not shown: {}",
        redact(&ui.capture())
    );
    ui.send_keys("Esc");
    assert!(
        !ui.capture().contains("→ "),
        "popup did not close on Esc: {}",
        redact(&ui.capture())
    );

    // 8. Resize to a small pane so the transcript overflows: PageUp reveals
    // the header at the top, PageDown returns to the bottom. Needles must not
    // collide with the footer, whose session id contains the word
    // "slimcode" — use the header-only hint line `/skills to run`.
    ui.resize(80, 14);
    thread::sleep(Duration::from_millis(400));
    // At the bottom with overflow the header has scrolled off.
    assert!(
        !ui.capture().contains("/skills to run"),
        "expected header scrolled off at bottom: {}",
        redact(&ui.capture())
    );
    ui.send_keys("PageUp");
    assert!(
        ui.wait_for("/skills to run", Duration::from_secs(3)),
        "PageUp did not scroll to the header: {}",
        redact(&ui.capture())
    );
    ui.send_keys("PageDown");
    assert!(
        ui.wait_for("Second answer.", Duration::from_secs(3)),
        "PageDown did not return to bottom: {}",
        redact(&ui.capture())
    );

    // 9. Resize back up: dock stays fixed, footer still renders at the bottom.
    ui.resize(100, 30);
    thread::sleep(Duration::from_millis(300));
    let cap2 = ui.capture();
    let lines2: Vec<&str> = cap2.lines().collect();
    assert!(
        lines2.len() >= 4 && lines2[lines2.len() - 1].contains("↑"),
        "dock lost on resize: {}",
        redact(&cap2)
    );

    // 10. OSC 0 pane title: `slimcode - <session> - <cwd basename>`.
    let title = ui.pane_title();
    assert!(
        title.contains("slimcode - ") && title.contains("slimcode-smoke-"),
        "pane title unexpected: {title:?}"
    );

    // 11. Ctrl+C while idle quits and restores the pane. Detect exit by the
    // TUI-only hint line disappearing — the pane's own shell prompt contains
    // the word "slimcode" (its cwd) and the echoed launch command, so bare
    // "slimcode" can never be used as the gone-marker.
    ui.send_keys("C-c");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let cap3 = ui.capture();
        if !cap3.contains("/help for commands") {
            break;
        }
        if Instant::now() > deadline {
            panic!("TUI still running after Ctrl+C: {}", redact(&cap3));
        }
        thread::sleep(Duration::from_millis(150));
    }
}

/// A scripted response that drips one reasoning chunk then `text_chunks` text
/// chunks, ~400ms apart, with **no terminal finish** — a run against it can
/// only end when the client cancels (Esc), which is what the escape-cancel
/// smoke drives.
fn drip_response(text_chunks: usize) -> Vec<String> {
    let mut out = vec![
        "{\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"mock\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"reasoning_content\":\"drip thinking\",\"content\":\"\"},\"finish_reason\":null}]}"
            .to_string(),
    ];
    for i in 1..=text_chunks {
        out.push(format!(
            "{{\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"mock\",\"choices\":[{{\"index\":0,\"delta\":{{\"reasoning_content\":\"\",\"content\":\"drip text {i} \"}},\"finish_reason\":null}}]}}"
        ));
    }
    out
}

/// Esc cancels a running turn anywhere in the runner (ticket 07): while the
/// mock drips a slow response, Esc returns the TUI to an idle, usable input
/// with whatever already streamed kept, no error text, and a fresh prompt
/// then runs normally (the token was reset per turn).
#[test]
fn tmux_escape_cancels_a_running_turn() {
    // Auto-skip when tmux is absent (CI without tmux).
    if Command::new("tmux").arg("-V").output().is_err() {
        eprintln!("skipping: tmux not available");
        return;
    }

    let dir = std::env::temp_dir().join(format!("slimcode-cancel-{}", std::process::id()));
    let home = std::env::temp_dir().join(format!("slimcode-cancel-home-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(&home).unwrap();

    // Turn 1: a slow drip that never finishes on its own (Esc ends it).
    // Turn 2: the follow-up prompt's normal answer.
    let mock = MockServer::start(vec![drip_response(6), sse(false)]);

    let ui = Tmux::new();
    launch(&mock, &ui, &dir, &home);
    assert!(
        ui.wait_for("/help for commands", Duration::from_secs(8)),
        "header not shown: {}",
        redact(&ui.capture())
    );

    // Submit the first prompt and wait for the working spinner to appear.
    ui.send_literal("cancel me");
    ui.send_keys("Enter");
    assert!(
        ui.wait_for("Working", Duration::from_secs(5)),
        "spinner did not appear: {}",
        redact(&ui.capture())
    );
    // Let a couple of drip chunks land while the request is in flight, then
    // press Esc.
    thread::sleep(Duration::from_millis(900));
    ui.send_keys("Esc");

    // The turn ends silently: the spinner row disappears (idle), while the
    // mock is still mid-response.
    let end = Instant::now() + Duration::from_secs(8);
    loop {
        if !ui.capture().contains("Working") {
            break;
        }
        if Instant::now() > end {
            panic!("turn did not cancel: {}", redact(&ui.capture()));
        }
        thread::sleep(Duration::from_millis(120));
    }

    // Whatever already streamed was kept (drip text 1 landed before the
    // cancel); no error/stopped marker appeared.
    let cap = ui.capture();
    assert!(
        cap.contains("drip text 1"),
        "partial text not kept after cancel: {}",
        redact(&cap)
    );
    assert!(
        !cap.contains("stopped"),
        "no stop marker expected: {}",
        redact(&cap)
    );
    assert!(
        !cap.contains("⚠"),
        "no warning marker expected: {}",
        redact(&cap)
    );

    // A fresh prompt runs normally (the per-turn token reset is verified): the
    // second mock response completes with the final answer.
    ui.send_literal("continue please");
    ui.send_keys("Enter");
    assert!(
        ui.wait_for("Second answer.", Duration::from_secs(12)),
        "follow-up turn did not complete: {}",
        redact(&ui.capture())
    );
}
