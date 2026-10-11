//! Shared scaffolding for the `slimcode-ffi` integration tests.
//!
//! Created by ticket 01 with the pure-stdlib pieces every FFI test needs:
//! temp scratch dirs (a test must never touch the developer's real
//! `~/.slimcode`), the `SLIMCODE_HOME` env guard for the `{}`-options case,
//! the options-JSON builder, and thin wrappers over ticket 01's ABI symbols.
//! **Ticket 02 appends the mock SSE endpoint and the `prompt` / `next_event`
//! wrappers here; tickets 03–06 extend this module additively** (append
//! helpers, do not restructure what is already here).
//!
//! The architecture test reads every dependency table, so `slimcode-ffi` may
//! not dev-depend on `slimcode-app`: this module is stdlib + `serde_json` only
//! — no `testutil`, no `SessionStore` helpers.
//!
//! `#![allow(dead_code)]` keeps the per-test-binary "unused helper" warnings
//! quiet when a file uses only part of the scaffolding.

#![allow(dead_code)]

use std::env;
use std::ffi::{CStr, CString, OsString, c_char, c_void};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};
use slimcode_ffi::abi::session as abi;

/// A fresh temp root unique per process and call: `<home>` (the slimcode home /
/// session store root) and `<cwd>` (with a `.git` marker, so project
/// resolution lands on `cwd` itself). Never the developer's real home.
pub fn scratch(tag: &str) -> (PathBuf, PathBuf) {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after 1970")
        .as_nanos();
    let root = env::temp_dir().join(format!("slimcode-ffi-{tag}-{}-{nanos}", std::process::id()));
    let home = root.join("home");
    let cwd = root.join("proj");
    fs::create_dir_all(&home).expect("create the scratch home");
    fs::create_dir_all(cwd.join(".git")).expect("create the scratch cwd git marker");
    (home, cwd)
}

/// Serializes tests that point the process-global `SLIMCODE_HOME` at a temp
/// dir (the tests run in parallel; without the lock they clobber each other's
/// setup — the SDK common module's pattern).
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Point the `{}`-options test's environment at the scratch home for the
/// guard's lifetime: `SLIMCODE_HOME` at `home` (so no real `config.toml` is
/// read) and `DASHSCOPE_API_KEY` at a dummy value (the SDK's config resolution
/// requires a key, and `{}` passes no options to carry one). Everything is
/// restored on drop. Every other test passes an explicit `home` plus an
/// explicit `api_key` through the options JSON and needs none of this.
pub fn home_guard(home: &Path) -> HomeGuard {
    let lock = ENV_LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
    let old_home = env::var_os("SLIMCODE_HOME");
    let old_key = env::var_os("DASHSCOPE_API_KEY");
    unsafe {
        env::set_var("SLIMCODE_HOME", home);
        env::set_var("DASHSCOPE_API_KEY", "sk-test");
    }
    HomeGuard {
        _lock: lock,
        old_home,
        old_key,
    }
}

/// The guard returned by [`home_guard`]; dropping it restores the environment.
pub struct HomeGuard {
    _lock: MutexGuard<'static, ()>,
    old_home: Option<OsString>,
    old_key: Option<OsString>,
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        match &self.old_home {
            Some(value) => unsafe { env::set_var("SLIMCODE_HOME", value) },
            None => unsafe { env::remove_var("SLIMCODE_HOME") },
        }
        match &self.old_key {
            Some(value) => unsafe { env::set_var("DASHSCOPE_API_KEY", value) },
            None => unsafe { env::remove_var("DASHSCOPE_API_KEY") },
        }
    }
}

/// [`scratch`] plus `SLIMCODE_HOME` pointed at the scratch home for the whole
/// test (the `{}`-options case).
pub fn test_env(tag: &str) -> (HomeGuard, PathBuf, PathBuf) {
    let (home, cwd) = scratch(tag);
    let guard = home_guard(&home);
    (guard, home, cwd)
}

/// A builder for the `slimcode_session_create` options JSON. Each setter is
/// one documented option field, so a test overrides exactly what it cares
/// about; [`Options::set`] adds an arbitrary key (the unknown-field test).
#[derive(Clone, Debug, Default)]
pub struct Options(Map<String, Value>);

impl Options {
    /// The empty object (`{}` = all SDK defaults).
    pub fn new() -> Self {
        Self::default()
    }

    /// Set an arbitrary option field.
    pub fn set(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.0.insert(key.to_string(), value.into());
        self
    }

    /// Pin the slimcode home directory.
    pub fn home(self, home: &Path) -> Self {
        self.set("home", path_value(home))
    }

    /// Set the working directory.
    pub fn cwd(self, cwd: &Path) -> Self {
        self.set("cwd", path_value(cwd))
    }

    /// Enable or disable JSONL persistence.
    pub fn persist(self, persist: bool) -> Self {
        self.set("persist", persist)
    }

    /// Resume a persisted session by id.
    pub fn session_id(self, id: &str) -> Self {
        self.set("session_id", id)
    }

    /// Override the provider base URL.
    pub fn base_url(self, base_url: &str) -> Self {
        self.set("base_url", base_url)
    }

    /// Override the provider model.
    pub fn model(self, model: &str) -> Self {
        self.set("model", model)
    }

    /// Override the provider API key.
    pub fn api_key(self, api_key: &str) -> Self {
        self.set("api_key", api_key)
    }

    /// Override the explicit-cache flag.
    pub fn cache(self, cache: bool) -> Self {
        self.set("cache", cache)
    }

    /// Render the object as the UTF-8 JSON string the ABI takes.
    pub fn to_json(&self) -> String {
        serde_json::to_string(&self.0).expect("options are JSON-serializable")
    }
}

/// The hermetic options every FFI test starts from: pinned `home` and `cwd`,
/// persistence on, and a dummy endpoint / model / key so creation never
/// consults the developer's config or environment.
pub fn options(home: &Path, cwd: &Path) -> Options {
    Options::new()
        .home(home)
        .cwd(cwd)
        .persist(true)
        .base_url("http://127.0.0.1:1/v1")
        .model("test-model")
        .api_key("sk-test")
}

/// A path as a JSON string (non-UTF-8 paths are not a thing the tests use).
fn path_value(path: &Path) -> Value {
    Value::String(path.to_string_lossy().into_owned())
}

/// `slimcode_abi_version()`.
pub fn abi_version() -> u32 {
    unsafe { abi::slimcode_abi_version() }
}

/// `slimcode_session_create(options_json)`; `NULL` on failure (the reason is
/// in [`last_error`]).
pub fn create(options_json: &str) -> *mut c_void {
    let json = CString::new(options_json).expect("options JSON has no interior NUL");
    unsafe { abi::slimcode_session_create(json.as_ptr()) }
}

/// [`create`] with the failure detail baked into the panic message.
pub fn create_ok(options_json: &str) -> *mut c_void {
    let handle = create(options_json);
    assert!(
        !handle.is_null(),
        "create({options_json}) failed: {:?}",
        last_error()
    );
    handle
}

/// `slimcode_session_id(handle)` — `None` for NULL handles.
pub fn session_id(handle: *mut c_void) -> Option<String> {
    take_string(unsafe { abi::slimcode_session_id(handle) })
}

/// `slimcode_session_title(handle)` — `None` for NULL handles.
pub fn title(handle: *mut c_void) -> Option<String> {
    take_string(unsafe { abi::slimcode_session_title(handle) })
}

/// `slimcode_session_dispose(handle)` (NULL and unknown handles are no-ops).
pub fn dispose(handle: *mut c_void) {
    unsafe { abi::slimcode_session_dispose(handle) };
}

/// `slimcode_last_error()` as an owned copy; the ABI pointer itself must never
/// be freed, so the copy happens before the next call.
pub fn last_error() -> Option<String> {
    let pointer = unsafe { abi::slimcode_last_error() };
    if pointer.is_null() {
        return None;
    }
    Some(
        unsafe { CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned(),
    )
}

/// Copy a library-owned string and release it with `slimcode_free_string`;
/// `None` for a NULL pointer (the error/timeout return).
pub fn take_string(pointer: *mut c_char) -> Option<String> {
    if pointer.is_null() {
        return None;
    }
    let text = unsafe { CStr::from_ptr(pointer) }
        .to_string_lossy()
        .into_owned();
    free_string(pointer);
    Some(text)
}

/// `slimcode_free_string(pointer)` (NULL is a no-op).
pub fn free_string(pointer: *mut c_char) {
    unsafe { abi::slimcode_free_string(pointer) };
}

/// Every `slimcode_*` symbol defined in `src/**/*.rs`, by scanning the source
/// text (the header-consistency check; no codegen introspection needed).
pub fn implemented_symbols(src_dir: &Path) -> Vec<(String, PathBuf)> {
    const MARKER: &str = "pub unsafe extern \"C\" fn slimcode_";
    let mut found = Vec::new();
    for path in rust_sources(src_dir) {
        let text = fs::read_to_string(&path).expect("read a crate source file");
        for line in text.lines() {
            let Some(rest) = line.trim().strip_prefix(MARKER) else {
                continue;
            };
            let name = rest.split('(').next().unwrap_or_default().trim();
            found.push((format!("slimcode_{name}"), path.clone()));
        }
    }
    found
}

/// Every `.rs` file under `dir`, recursively (no walkdir dependency).
fn rust_sources(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            found.extend(rust_sources(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
    found
}

// ---------------------------------------------------------------------------
// Ticket 02: the mock SSE provider endpoint and the turn-path wrappers.
// ---------------------------------------------------------------------------
//
// The imports live here (not in the file header) so this section appends
// without touching ticket 01's lines; later tickets do the same.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use slimcode_ffi::abi::events as events_abi;

/// The `[DONE]` payload that terminates an SSE response (served like any other
/// payload, so it goes last in a [`Reply::Sse`] script).
pub const SSE_DONE: &str = "[DONE]";

/// One streamed assistant text delta; `finish_reason` of `Some("stop")` makes
/// this the chunk that closes the model's answer.
pub fn sse_text(text: &str, finish_reason: Option<&str>) -> String {
    format!(
        "{{\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"mock\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":{}}},\"finish_reason\":{}}}]}}",
        json_string(text),
        finish_reason.map_or_else(|| "null".to_string(), json_string),
    )
}

/// One reasoning (thinking) delta, the qwen-style `reasoning_content` the
/// endpoint streams before the answer.
pub fn sse_reasoning(text: &str) -> String {
    format!(
        "{{\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"mock\",\"choices\":[{{\"index\":0,\"delta\":{{\"reasoning_content\":{}}},\"finish_reason\":null}}]}}",
        json_string(text),
    )
}

/// One tool-call delta: the model starts a call with its id, name and raw
/// argument JSON string (followed by [`sse_tool_finish`] to end the request
/// with `finish_reason: "tool_calls"`).
pub fn sse_tool_call(id: &str, name: &str, arguments: &str) -> String {
    format!(
        "{{\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"mock\",\"choices\":[{{\"index\":0,\"delta\":{{\"tool_calls\":[{{\"id\":{},\"type\":\"function\",\"index\":0,\"function\":{{\"name\":{},\"arguments\":{}}}}}]}},\"finish_reason\":null}}]}}",
        json_string(id),
        json_string(name),
        json_string(arguments),
    )
}

/// The empty tail chunk that closes a tool-call request with
/// `finish_reason: "tool_calls"` (the shape the endpoint actually sends).
pub fn sse_tool_finish() -> String {
    "{\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"mock\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"name\":null,\"arguments\":\"\"}}]},\"finish_reason\":\"tool_calls\"}]}".to_string()
}

/// The final usage-only chunk the endpoint sends with `include_usage`
/// (`choices: []`, usage last). `details` of `Some((cached, cache_creation))`
/// includes `prompt_tokens_details`; `None` omits the field entirely — the
/// event payload then carries `"prompt_tokens_details":null` (the wire shape
/// pinned in `include/slimcode.h`).
pub fn sse_usage(
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
    details: Option<(u64, u64)>,
) -> String {
    let details = match details {
        Some((cached, cache_creation)) => format!(
            ",\"prompt_tokens_details\":{{\"cached_tokens\":{cached},\"cache_creation_input_tokens\":{cache_creation}}}"
        ),
        None => String::new(),
    };
    format!(
        "{{\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"mock\",\"choices\":[],\"usage\":{{\"prompt_tokens\":{prompt_tokens},\"completion_tokens\":{completion_tokens},\"total_tokens\":{total_tokens}{details}}}}}",
    )
}

/// One string as a JSON literal (quoted and escaped).
fn json_string(value: &str) -> String {
    serde_json::to_string(value).expect("a string is JSON-serializable")
}

/// One scripted HTTP reply, served in request order (the nth reply answers the
/// nth `chat/completions` request).
#[derive(Clone, Debug)]
pub enum Reply {
    /// An SSE response: each entry is one raw payload, written as
    /// `data: {payload}\n\n`. There is a short pause between payloads, so a
    /// turn stays in flight long enough for a concurrent test to observe it
    /// (single-flight, the pump test).
    Sse(Vec<String>),
    /// Like [`Reply::Sse`], but the connection is held open with no further
    /// bytes after the first payload until the test opens `gate` (bounded at
    /// 5 s). A provider read is then genuinely blocked while the test acts on
    /// the turn — dispose / abort land without racing a fixed pause — and the
    /// released payloads arrive on the read that follows the open.
    SseGated {
        payloads: Vec<String>,
        gate: Arc<Gate>,
    },
    /// A raw non-2xx response (`status` plus a plain-text body), which the
    /// provider path turns into a failed turn.
    Status { status: u16, body: String },
}

/// One request the mock endpoint received: the head (request line + headers)
/// and the body, verbatim.
#[derive(Clone, Debug)]
pub struct CapturedRequest {
    /// The request line plus headers, exactly as received.
    pub head: String,
    /// The request body (empty when there was none).
    pub body: String,
}

impl CapturedRequest {
    /// The value of one header, case-insensitively on the name (`None` when
    /// absent); the value keeps its original case.
    pub fn header(&self, name: &str) -> Option<String> {
        let name = name.to_ascii_lowercase();
        self.head.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            (key.trim().to_ascii_lowercase() == name).then(|| value.trim().to_string())
        })
    }

    /// The request target (path plus query) from the request line.
    pub fn target(&self) -> &str {
        self.head.split_whitespace().nth(1).unwrap_or_default()
    }

    /// The body parsed as JSON (panics with the raw body on malformed input).
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body)
            .unwrap_or_else(|e| panic!("request body is not JSON ({e}): {}", self.body))
    }
}

/// A local mock of the Bailian compatible-mode endpoint (the spec's only
/// provider seam: stdlib `TcpListener` + a thread, no Rust test hooks in
/// `sdk` / `app`). It serves one scripted [`Reply`] per request, in order, and
/// captures every request it receives.
pub struct MockServer {
    addr: String,
    state: Arc<MockState>,
    thread: Option<thread::JoinHandle<()>>,
}

/// The captured requests plus the stop flag `Drop` uses to end the server
/// thread without ever blocking on a final `accept`.
struct MockState {
    requests: Mutex<Vec<CapturedRequest>>,
    stop: AtomicBool,
}

impl MockServer {
    /// Bind `127.0.0.1:0` and serve `replies`, one per request. An empty list
    /// is fine: the endpoint exists but is never expected to answer.
    pub fn start(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the mock endpoint");
        // Non-blocking accept so the thread always exits and `Drop`'s join can
        // never hang (the `crates/cli/tests/tui_smoke.rs` precedent).
        listener.set_nonblocking(true).expect("non-blocking accept");
        let addr = listener.local_addr().expect("mock address").to_string();
        let state = Arc::new(MockState {
            requests: Mutex::new(Vec::new()),
            stop: AtomicBool::new(false),
        });
        let thread_state = Arc::clone(&state);
        let thread = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(30);
            let mut served = 0;
            while served < replies.len()
                && !thread_state.stop.load(Ordering::SeqCst)
                && Instant::now() < deadline
            {
                let Ok((mut stream, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(10)); // poll tick
                    continue;
                };
                let request = read_request(&mut stream);
                thread_state
                    .requests
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .push(request);
                write_reply(&mut stream, &replies[served]);
                served += 1;
            }
        });
        Self {
            addr,
            state,
            thread: Some(thread),
        }
    }

    /// The provider base URL to pass as `base_url` (the provider appends
    /// `/chat/completions`).
    pub fn base_url(&self) -> String {
        format!("http://{}/v1", self.addr)
    }

    /// Every request received so far (a snapshot copy).
    pub fn requests(&self) -> Vec<CapturedRequest> {
        self.state
            .requests
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    /// How many requests the endpoint has received so far.
    pub fn request_count(&self) -> usize {
        self.state
            .requests
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .len()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        // Signal, then join: the poll tick bounds how long the join can take.
        self.state.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Write one scripted reply and close the connection.
fn write_reply(stream: &mut TcpStream, reply: &Reply) {
    match reply {
        Reply::Sse(payloads) => write_sse(stream, payloads, None),
        Reply::SseGated { payloads, gate } => write_sse(stream, payloads, Some(gate)),
        Reply::Status { status, body } => {
            let head = format!(
                "HTTP/1.1 {status} Mock\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body.as_bytes());
            let _ = stream.flush();
        }
    }
    let _ = stream.shutdown(Shutdown::Both);
}

/// Write one SSE reply's head plus each payload as `data: {payload}\n\n`. A
/// short pause between payloads keeps the turn in flight long enough for a
/// concurrent test to observe it (single-flight, the blocked pump test). When
/// `gate` is `Some`, the connection is held open with no further bytes after
/// the first payload until the gate opens (bounded at 5 s), so the provider
/// read is genuinely blocked while the test disposes / aborts the in-flight
/// turn — no timing race — and the released payloads arrive on the read that
/// follows the open.
fn write_sse(stream: &mut TcpStream, payloads: &[String], gate: Option<&Gate>) {
    let body: String = payloads
        .iter()
        .map(|payload| format!("data: {payload}\n\n"))
        .collect();
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    for (index, payload) in payloads.iter().enumerate() {
        let chunk = format!("data: {payload}\n\n");
        let _ = stream.write_all(chunk.as_bytes());
        let _ = stream.flush();
        if index == 0
            && let Some(gate) = gate
        {
            gate.wait(Duration::from_secs(5));
        }
        if index + 1 < payloads.len() {
            thread::sleep(Duration::from_millis(100));
        }
    }
}

/// Read one HTTP request: the head (request line + headers) and, when a
/// `Content-Length` is present, the whole body.
fn read_request(stream: &mut TcpStream) -> CapturedRequest {
    let mut buf = [0u8; 4096];
    let mut acc: Vec<u8> = Vec::new();
    loop {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => acc.extend_from_slice(&buf[..n]),
        }
        if acc.windows(4).any(|window| window == b"\r\n\r\n") || acc.len() > 1024 * 1024 {
            break;
        }
    }
    let head_end = acc
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map_or(acc.len(), |pos| pos + 4);
    let head = String::from_utf8_lossy(&acc[..head_end]).into_owned();
    let mut body = acc[head_end..].to_vec();
    let content_length = head.lines().find_map(|line| {
        line.to_ascii_lowercase()
            .strip_prefix("content-length:")
            .and_then(|value| value.trim().parse::<usize>().ok())
    });
    let content_length = content_length.unwrap_or(0);
    while body.len() < content_length {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => body.extend_from_slice(&buf[..n]),
        }
    }
    CapturedRequest {
        head,
        body: String::from_utf8_lossy(&body).into_owned(),
    }
}

/// `slimcode_session_prompt(handle, text)` — the stop code: 0 Completed,
/// 1 Cancelled, -1 error (the reason is in [`last_error`]).
pub fn prompt(handle: *mut c_void, text: &str) -> i32 {
    let text = CString::new(text).expect("prompt text has no interior NUL");
    unsafe { events_abi::slimcode_session_prompt(handle, text.as_ptr()) }
}

/// `slimcode_session_next_event(handle, timeout_ms)` parsed as JSON; `None`
/// for the timeout / empty-queue / disposed return (assert on [`last_error`]
/// separately — the two NULL kinds are distinguished by it).
pub fn next_event(handle: *mut c_void, timeout_ms: i32) -> Option<serde_json::Value> {
    let pointer = unsafe { events_abi::slimcode_session_next_event(handle, timeout_ms) };
    let text = take_string(pointer)?;
    Some(
        serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("next_event produced non-JSON ({e}): {text}")),
    )
}

/// Every event currently queued (`timeout_ms = 0` poll until empty).
pub fn drain_ready(handle: *mut c_void) -> Vec<serde_json::Value> {
    let mut events = Vec::new();
    while let Some(event) = next_event(handle, 0) {
        events.push(event);
    }
    events
}

/// Drain events until `turn_finished` (the authoritative end of a completed /
/// cancelled turn; a turn-end auto-compaction's `compacted` may still follow
/// it). Panics after 5s without one.
pub fn drain_until_turn_finished(handle: *mut c_void) -> Vec<serde_json::Value> {
    let mut events = Vec::new();
    loop {
        let event = next_event(handle, 5_000)
            .unwrap_or_else(|| panic!("no turn_finished within 5s; got {:?}", kinds(&events)));
        let finished = event["kind"] == "turn_finished";
        events.push(event);
        if finished {
            return events;
        }
    }
}

/// The `kind` field of one event object.
pub fn kind(event: &serde_json::Value) -> &str {
    event["kind"]
        .as_str()
        .unwrap_or_else(|| panic!("an event must have a string kind: {event}"))
}

/// The `kind` of every event, in order (sequence assertions).
pub fn kinds(events: &[serde_json::Value]) -> Vec<&str> {
    events.iter().map(kind).collect()
}

// ---------------------------------------------------------------------------
// Ticket 03: the read-only state accessors.
// ---------------------------------------------------------------------------
//
// Appended after ticket 02's block — the same additive rule: nothing above
// this line changes.

use slimcode_ffi::abi::state as state_abi;

/// `slimcode_session_usage_json(handle)` parsed as JSON; `None` for the NULL
/// return (a NULL / disposed handle — assert on [`last_error`] separately).
pub fn usage_json(handle: *mut c_void) -> Option<serde_json::Value> {
    let pointer = unsafe { state_abi::slimcode_session_usage_json(handle) };
    let text = take_string(pointer)?;
    Some(parse_state_json("usage_json", &text))
}

/// `slimcode_session_messages_json(handle)` parsed as the JSON array; `None`
/// for the NULL return (a NULL / disposed handle).
pub fn messages_json(handle: *mut c_void) -> Option<serde_json::Value> {
    let pointer = unsafe { state_abi::slimcode_session_messages_json(handle) };
    let text = take_string(pointer)?;
    Some(parse_state_json("messages_json", &text))
}

/// Parse one accessor payload, panicking with the raw text on malformed JSON.
fn parse_state_json(symbol: &str, text: &str) -> serde_json::Value {
    serde_json::from_str(text)
        .unwrap_or_else(|e| panic!("{symbol} produced non-JSON ({e}): {text}"))
}

// ---------------------------------------------------------------------------
// Ticket 05: session-log discovery and record parsing (the persistence tests).
// ---------------------------------------------------------------------------
//
// The log is read with `serde_json` rather than through the SDK's own loader:
// `slimcode-ffi` cannot dev-depend on `slimcode-app` (the architecture test's
// "`ffi` → only `sdk`" row reads every dependency table, dev-dependencies
// included), and the SDK does not re-export `SessionStore`. Walking the tree
// also keeps the tests honest about where the log landed instead of
// recomputing `project_key`'s hash.

/// Every `.jsonl` session log under `<home>/sessions`, recursively — the
/// store's `<home>/sessions/<project-key>/<id>.jsonl` layout. Sorted, so two
/// snapshots compare directly. Promoted from ticket 02's private copy in
/// `turn_stream.rs` so later test files share one walker.
pub fn persisted_logs(home: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    collect_jsonl(&home.join("sessions"), &mut found);
    found.sort();
    found
}

/// The recursive half of [`persisted_logs`]; a missing directory means "no
/// logs" (with `persist: false` there is no store directory at all).
fn collect_jsonl(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl(&path, found);
        } else if path.extension().is_some_and(|ext| ext == "jsonl") {
            found.push(path);
        }
    }
}

/// One append-only session log as parsed records: the mandatory header line
/// plus one JSON object per following line. Panics on a malformed line — a
/// record the SDK's `SessionStore::load` would silently skip is a bug these
/// tests must surface, not paper over.
pub fn log_records(path: &Path) -> Vec<Value> {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).unwrap_or_else(|e| {
                panic!("{} has a malformed record ({e}): {line}", path.display())
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Ticket 04: the lifecycle helpers (`abort` plus a bounded prompt thread).
// ---------------------------------------------------------------------------
//
// Appended after ticket 05's block — the same additive rule: nothing above
// this line changes. The lifecycle tests observe a turn that is in flight, so
// they need a way to call `prompt` on another thread and *bound* the wait for
// its stop code: a `JoinHandle` cannot be given a timeout.

use std::sync::mpsc::{Receiver, RecvTimeoutError};

use slimcode_ffi::abi::lifecycle as lifecycle_abi;

/// `slimcode_session_abort(handle)` — callable from any thread (NULL / unknown
/// handles are silent no-ops).
pub fn abort(handle: *mut c_void) {
    unsafe { lifecycle_abi::slimcode_session_abort(handle) };
}

/// Run `slimcode_session_prompt(handle, text)` on its own OS thread and hand
/// the stop code back over a channel, so a test can `recv_timeout` instead of
/// joining without a bound (a host-driven cancel / dispose must end the
/// wait). The raw pointer crosses the thread boundary as its `usize` key, the
/// same way the ticket-02 tests pass handles to a pump thread.
pub fn prompt_on_thread(handle: *mut c_void, text: &str) -> Receiver<i32> {
    let pointer = handle as usize;
    let text = text.to_string();
    let (sender, receiver) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let code = prompt(pointer as *mut c_void, &text);
        // The receiver may be gone when a test failed early; dropping the
        // code then is fine (there is nothing left to assert).
        let _ = sender.send(code);
    });
    receiver
}

/// The stop code [`prompt_on_thread`] will send, within `timeout`; panics with
/// `what` when the wait expires, so a turn that never ends fails the test
/// instead of hanging the suite.
pub fn await_code(receiver: &Receiver<i32>, timeout: Duration, what: &str) -> i32 {
    match receiver.recv_timeout(timeout) {
        Ok(code) => code,
        Err(RecvTimeoutError::Timeout) => {
            panic!("{what}: no stop code within {timeout:?}")
        }
        Err(RecvTimeoutError::Disconnected) => {
            panic!("{what}: the prompt thread vanished without sending a stop code")
        }
    }
}

// ---------------------------------------------------------------------------
// Ticket 06: compact_now.
// ---------------------------------------------------------------------------
//
// Appended after ticket 05's block — the same additive rule: nothing above
// this line changes.

use slimcode_ffi::abi::compact as compact_abi;

/// `slimcode_session_compact_now(handle)` parsed as `{"before":N,"after":M}`;
/// `None` for the NULL return (assert on [`last_error`] separately — it tells
/// the "nothing to compact" / provider-failure / invalid-handle kinds apart).
pub fn compact_now(handle: *mut c_void) -> Option<serde_json::Value> {
    let pointer = unsafe { compact_abi::slimcode_session_compact_now(handle) };
    let text = take_string(pointer)?;
    Some(
        serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("compact_now produced non-JSON ({e}): {text}")),
    )
}

// ---------------------------------------------------------------------------
// Code-review follow-up: the shared turn helper and the gated SSE reply.
// ---------------------------------------------------------------------------
//
// Appended after ticket 06's block — the same additive rule: nothing above
// this line changes.

/// A one-shot gate for [`Reply::SseGated`]: [`release`](Self::release) opens
/// it, [`wait`](Self::wait) blocks until then or the timeout expires.
#[derive(Debug, Default)]
pub struct Gate {
    open: Mutex<bool>,
    ready: std::sync::Condvar,
}

impl Gate {
    /// A closed gate.
    pub fn new() -> Self {
        Self::default()
    }

    /// Open the gate (idempotent); every waiter wakes.
    pub fn release(&self) {
        let mut open = self.lock();
        *open = true;
        drop(open);
        self.ready.notify_all();
    }

    /// Block until the gate opens, at most `timeout`; `true` when it is open.
    pub fn wait(&self, timeout: Duration) -> bool {
        let open = self.lock();
        let (open, _) = self
            .ready
            .wait_timeout_while(open, timeout, |open| !*open)
            .unwrap_or_else(|poison| poison.into_inner());
        *open
    }

    /// The gate's lock, recovering from poisoning like the rest of this
    /// module's test scaffolding.
    fn lock(&self) -> MutexGuard<'_, bool> {
        self.open
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }
}

/// A hermetic session (scratch home + cwd) pointed at the mock endpoint — the
/// shared seam every test that runs a turn starts from, so nothing reaches the
/// developer's real `~/.slimcode`. Promoted from the four private copies in
/// `turn_stream.rs`, `tool_turn_accessors.rs`, `lifecycle.rs` and
/// `compact_now.rs`: their bodies were byte-identical, only the doc comments
/// differed.
pub fn mock_session(tag: &str, mock: &MockServer) -> *mut c_void {
    let (home, cwd) = scratch(tag);
    let options = options(&home, &cwd).base_url(&mock.base_url());
    create_ok(&options.to_json())
}

/// [`prompt_on_thread`] plus the prompt thread's own `last_error`: another
/// thread's error state is invisible to the test thread, so a test asserting
/// "the cancellation left it empty" must have it sent over.
pub fn prompt_on_thread_with_error(
    handle: *mut c_void,
    text: &str,
) -> Receiver<(i32, Option<String>)> {
    let pointer = handle as usize;
    let text = text.to_string();
    let (sender, receiver) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let code = prompt(pointer as *mut c_void, &text);
        let error = last_error();
        // The receiver may be gone when a test failed early; dropping the
        // result then is fine (there is nothing left to assert).
        let _ = sender.send((code, error));
    });
    receiver
}

/// The stop code plus the prompt thread's `last_error` that
/// [`prompt_on_thread_with_error`] will send, within `timeout`; panics with
/// `what` when the wait expires, so a turn that never ends fails the test
/// instead of hanging the suite.
pub fn await_prompt(
    receiver: &Receiver<(i32, Option<String>)>,
    timeout: Duration,
    what: &str,
) -> (i32, Option<String>) {
    match receiver.recv_timeout(timeout) {
        Ok(result) => result,
        Err(RecvTimeoutError::Timeout) => panic!("{what}: no stop code within {timeout:?}"),
        Err(RecvTimeoutError::Disconnected) => {
            panic!("{what}: the prompt thread vanished without sending a stop code")
        }
    }
}
