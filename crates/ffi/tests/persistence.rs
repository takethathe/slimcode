//! Ticket 05: the two persistence modes and `session_id` resume — the session
//! log as a host sees it.
//!
//! A host that passes no persistence option gets the CLI's behavior (ADR-0021
//! D3): an append-only JSONL log at `<home>/sessions/<project-key>/<id>.jsonl`,
//! the project-partitioned layout the `/session` picker and the SDK's
//! `SessionStore::load` read. `persist: false` writes no file at all, and
//! `session_id` resumes a persisted session — messages and title replayed, the
//! same log appended to — so "continue the last session" needs no new symbol.
//!
//! The assertions stay on the two pre-agreed seams: the C ABI functions and the
//! mock SSE endpoint. `slimcode-ffi` cannot dev-depend on `slimcode-app` (the
//! architecture test's `ffi → only sdk` row reads dev-dependencies too), so the
//! log is validated structurally with `serde_json`: the header `load` requires,
//! the record types it replays, and the message payloads equal to the ones the
//! ABI itself reports for the same handle — over the event queue (`message`
//! events) and through the read-only `messages_json` accessor, both of which
//! spec U13 and D3 pin to the session-log shape. Replay into the resumed session is
//! then proven through the ABI and the provider seam: the resumed turn's
//! request body carries the replayed history, which only a real load can
//! produce.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use common::{MockServer, Options, Reply, SSE_DONE};

/// The hermetic mock-endpoint options **without** a `persist` key: the FFI's
/// own default (`true`) must be what makes the log appear, exactly like a host
/// that never thought about persistence.
fn default_persist_options(home: &Path, cwd: &Path, base_url: &str) -> Options {
    Options::new()
        .home(home)
        .cwd(cwd)
        .base_url(base_url)
        .model("test-model")
        .api_key("sk-test")
}

/// One SSE reply: a single assistant text chunk that closes the model's answer
/// (`finish_reason: "stop"`), the request's usage, and the `[DONE]` tail.
fn text_turn(text: &str, prompt_tokens: u64, completion_tokens: u64, total_tokens: u64) -> Reply {
    Reply::Sse(vec![
        common::sse_text(text, Some("stop")),
        common::sse_usage(prompt_tokens, completion_tokens, total_tokens, None),
        SSE_DONE.to_string(),
    ])
}

/// The `message` payloads (`message.message`) the ABI reported for a turn, in
/// order — the events route to the same session-log shape the stored records
/// carry.
fn reported_messages(events: &[Value]) -> Vec<Value> {
    events
        .iter()
        .filter(|event| common::kind(event) == "message")
        .map(|event| event["message"].clone())
        .collect()
}

/// The text of one `AgentMessage` payload (its concatenated `parts`); these
/// tests only ever produce plain text messages.
fn message_text(message: &Value) -> String {
    message["parts"]
        .as_array()
        .unwrap_or_else(|| panic!("an llm message payload carries parts: {message}"))
        .iter()
        .filter_map(|part| part["text"].as_str())
        .collect()
}

/// Every message text the endpoint received, in order. A request's `messages`
/// entry carries `content` either as a plain string or as an array of
/// `{"type":"text","text":…}` blocks (the cached shape), so both are collected
/// without pinning either.
fn request_texts(body: &Value) -> Vec<String> {
    let messages = body["messages"]
        .as_array()
        .unwrap_or_else(|| panic!("the request has no messages array: {body}"));
    let mut texts = Vec::new();
    for message in messages {
        match &message["content"] {
            Value::String(text) => texts.push(text.clone()),
            Value::Array(blocks) => texts.extend(
                blocks
                    .iter()
                    .filter_map(|block| block["text"].as_str())
                    .map(str::to_string),
            ),
            other => panic!("unexpected message content shape: {other}"),
        }
    }
    texts
}

/// The one session log under `home`; a second log would mean the store's
/// project scoping broke.
fn single_log(home: &Path) -> PathBuf {
    let logs = common::persisted_logs(home);
    assert_eq!(logs.len(), 1, "expected exactly one session log: {logs:?}");
    logs.into_iter().next().expect("checked non-empty")
}

/// Every file under `dir`, recursively (any extension) — the `persist: false`
/// test asserts "zero files", not merely "no `.jsonl`".
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(files_under(&path));
        } else {
            found.push(path);
        }
    }
    found.sort();
    found
}

/// Assert the store's `<home>/sessions/<project-key>/<id>.jsonl` layout for
/// `log`: named after the session id, with exactly one project-key directory
/// between `sessions` and the file. The key's basename prefix is asserted
/// rather than a recomputed hash — `project_key` belongs to `slimcode-app`,
/// which this crate may not link.
fn assert_log_layout(log: &Path, home: &Path, id: &str) {
    assert_eq!(
        log.file_stem().and_then(|stem| stem.to_str()),
        Some(id),
        "the log is named after the handle's session id: {log:?}"
    );
    let project_dir = log.parent().expect("a project-key directory");
    let sessions = home.join("sessions");
    assert_eq!(
        project_dir.parent(),
        Some(sessions.as_path()),
        "the log lives directly under <home>/sessions/<project-key>: {log:?}"
    );
    let key = project_dir
        .file_name()
        .and_then(|name| name.to_str())
        .expect("a project-key directory name");
    assert!(
        key.starts_with("proj-"),
        "the project key starts with the project home's basename: {key:?}"
    );
}

/// Omitting `persist` entirely is the CLI behavior: the turn persists to
/// `<home>/sessions/<project-key>/<id>.jsonl`, in the exact shape `load`
/// replays (a well-formed header, known record types, the ABI's own message
/// payloads), and the log's title agrees with the read-only accessor.
#[test]
fn an_omitted_persist_option_writes_the_default_session_log() {
    let mock = MockServer::start(vec![text_turn("Persisted.", 10, 3, 13)]);
    let (home, cwd) = common::scratch("default-persist");
    // `persist` is deliberately absent, so the log proves the FFI's own default
    // (`true`), not the test builder's.
    let options = default_persist_options(&home, &cwd, &mock.base_url());
    let json = options.to_json();
    assert!(
        !json.contains("\"persist\""),
        "the option must be omitted: {json}"
    );

    let handle = common::create_ok(&json);
    let id = common::session_id(handle).expect("session id");
    assert_eq!(
        common::prompt(handle, "keep this turn"),
        0,
        "prompt failed: {:?}",
        common::last_error()
    );
    let events = common::drain_until_turn_finished(handle);
    let reported = reported_messages(&events);
    assert_eq!(
        reported.len(),
        2,
        "the prompt and the reply enter history: {events:#?}"
    );

    let log = single_log(&home);
    assert_log_layout(&log, &home, &id);
    assert!(
        fs::read_to_string(&log)
            .expect("read the log")
            .ends_with('\n'),
        "an append-only log stays line-aligned: {log:?}"
    );

    // What `SessionStore::load` requires: a mandatory header and typed records.
    let records = common::log_records(&log);
    assert_eq!(
        records
            .iter()
            .map(|record| record["type"].clone())
            .collect::<Vec<Value>>(),
        ["session", "title", "message", "message"],
        "log records: {records:#?}"
    );
    let header = &records[0];
    assert_eq!(header["v"], 1, "the log format version: {header:#?}");
    assert_eq!(header["id"], id.as_str());
    assert!(
        header["created_at"].as_str().is_some_and(|s| !s.is_empty()),
        "the header carries a timestamp: {header:#?}"
    );
    assert_eq!(
        header["project_home"],
        fs::canonicalize(&cwd)
            .expect("canonical scratch cwd")
            .to_string_lossy()
            .as_ref(),
        "the header records the canonical project home: {header:#?}"
    );

    // The title record and the accessor agree — a resume replays exactly this.
    assert_eq!(records[1]["title"], "keep this turn");
    assert_eq!(common::title(handle).as_deref(), Some("keep this turn"));

    // The stored message payloads are the very ones the ABI reported, in order:
    // the user prompt first, the assistant reply second.
    let stored: Vec<Value> = records[2..]
        .iter()
        .map(|record| record["message"].clone())
        .collect();
    assert_eq!(stored, reported, "log records: {records:#?}");
    assert_eq!(stored[0]["role"], "user");
    assert_eq!(message_text(&stored[0]), "keep this turn");
    assert_eq!(stored[1]["role"], "assistant");
    assert_eq!(message_text(&stored[1]), "Persisted.");

    // The same payloads must be what ticket 03's `messages_json` accessor
    // reports for this handle: the log, the event queue and the accessor are
    // one shape (spec D3), so a host can parse all three with one parser.
    let messages = common::messages_json(handle).expect("messages_json");
    let accessor = messages
        .as_array()
        .unwrap_or_else(|| panic!("messages_json must be a JSON array: {messages}"));
    assert_eq!(accessor, &stored, "messages_json: {messages}");

    common::dispose(handle);
}

/// `persist: false` leaves zero files after a whole turn: not just no
/// `.jsonl`, and not even the store directory.
#[test]
fn persist_false_leaves_no_file_after_a_whole_turn() {
    let mock = MockServer::start(vec![text_turn("Ephemeral.", 5, 1, 6)]);
    let (home, cwd) = common::scratch("no-persist");
    let options = common::options(&home, &cwd)
        .base_url(&mock.base_url())
        .persist(false);
    let handle = common::create_ok(&options.to_json());

    assert_eq!(
        common::prompt(handle, "do not write me"),
        0,
        "prompt failed: {:?}",
        common::last_error()
    );
    // The turn really ran to its authoritative end: a zero-file result must not
    // come from a turn that died before writing.
    let events = common::drain_until_turn_finished(handle);
    assert_eq!(common::kind(events.last().unwrap()), "turn_finished");
    assert_eq!(mock.request_count(), 1);

    let files = files_under(&home);
    assert!(files.is_empty(), "persist: false wrote {files:?}");
    assert!(
        !home.join("sessions").exists(),
        "persist: false must not even create the store directory"
    );

    common::dispose(handle);
}

/// `session_id` resumes a persisted session through the same options object:
/// the id and the title come back (from the log's header and title record), the
/// replayed history reaches the model context, and the resumed turn keeps
/// appending to the same log instead of recreating it.
#[test]
fn resuming_by_session_id_replays_the_session_and_keeps_appending() {
    let mock = MockServer::start(vec![
        text_turn("First answer.", 10, 3, 13),
        text_turn("Second answer.", 20, 4, 24),
    ]);
    let (home, cwd) = common::scratch("resume");
    // Both handles omit `persist`: resuming needs the store, and the FFI's
    // default provides it — the host writes no persistence option.
    let options = default_persist_options(&home, &cwd, &mock.base_url());

    let first = common::create_ok(&options.to_json());
    let id = common::session_id(first).expect("session id");
    assert_eq!(
        common::prompt(first, "first question"),
        0,
        "first prompt failed: {:?}",
        common::last_error()
    );
    common::drain_until_turn_finished(first);
    let title = common::title(first).expect("the first prompt infers a title");
    assert_eq!(title, "first question");
    common::dispose(first);

    // The persisted log so far: header + title + the prompt + the reply.
    let before = single_log(&home);
    let records_before = common::log_records(&before);
    assert_eq!(records_before.len(), 4, "log records: {records_before:#?}");

    // Resume: a new handle, same home / cwd / endpoint, carrying the id.
    let resumed = common::create_ok(
        &default_persist_options(&home, &cwd, &mock.base_url())
            .session_id(&id)
            .to_json(),
    );
    assert_eq!(
        common::session_id(resumed).as_deref(),
        Some(id.as_str()),
        "a resumed session keeps its id, and therefore its log file"
    );
    assert_eq!(
        common::title(resumed).as_deref(),
        Some(title.as_str()),
        "the title is replayed from the log, not re-inferred"
    );

    assert_eq!(
        common::prompt(resumed, "second question"),
        0,
        "resumed prompt failed: {:?}",
        common::last_error()
    );
    let events = common::drain_until_turn_finished(resumed);

    // The resume really replayed the history: the resumed session's first
    // request carries the first turn's prompt and reply again.
    let requests = mock.requests();
    assert_eq!(requests.len(), 2, "one request per turn");
    let sent = request_texts(&requests[1].json());
    assert!(
        sent.iter().any(|text| text == "first question"),
        "the replayed prompt is missing from the request: {sent:#?}"
    );
    assert!(
        sent.iter().any(|text| text == "First answer."),
        "the replayed reply is missing from the request: {sent:#?}"
    );
    assert!(
        sent.iter().any(|text| text == "second question"),
        "the new prompt is missing from the request: {sent:#?}"
    );

    // Same file, appended to — never recreated: one header, one title record,
    // and the new records carry exactly the resumed turn's messages.
    let after = single_log(&home);
    assert_eq!(after, before, "the log path must not change on resume");
    let records_after = common::log_records(&after);
    assert_eq!(
        records_after
            .iter()
            .filter(|record| record["type"] == "session")
            .count(),
        1,
        "the header must not be written twice: {records_after:#?}"
    );
    assert_eq!(
        records_after
            .iter()
            .filter(|record| record["type"] == "title")
            .count(),
        1,
        "a resumed session must not append a second title record: {records_after:#?}"
    );
    assert!(
        records_after.len() > records_before.len(),
        "the log must grow: {records_before:#?} -> {records_after:#?}"
    );
    let appended: Vec<Value> = records_after[records_before.len()..]
        .iter()
        .map(|record| record["message"].clone())
        .collect();
    assert_eq!(
        appended,
        reported_messages(&events),
        "the appended records are the resumed turn's messages: {records_after:#?}"
    );
    let appended_texts: Vec<String> = appended.iter().map(message_text).collect();
    assert!(
        appended_texts.iter().any(|text| text == "second question"),
        "the new prompt must be appended: {appended_texts:#?}"
    );
    assert!(
        appended_texts.iter().any(|text| text == "Second answer."),
        "the new reply must be appended: {appended_texts:#?}"
    );

    common::dispose(resumed);
}

/// An explicit `home` option wins over the environment's slimcode home: with
/// `SLIMCODE_HOME` pointed at a second scratch dir, the session still lands
/// under the option's home and the environment home stays empty.
#[test]
fn an_explicit_home_option_wins_over_the_environment_home() {
    let mock = MockServer::start(vec![text_turn("Pinned.", 4, 1, 5)]);
    let (env_home, _env_cwd) = common::scratch("env-home");
    let (pinned_home, cwd) = common::scratch("pinned-home");
    // The `{}`-options pattern's guard: it serializes this test against the
    // other env-pinning test and restores the environment on drop.
    let _guard = common::home_guard(&env_home);

    let options = common::options(&pinned_home, &cwd).base_url(&mock.base_url());
    let handle = common::create_ok(&options.to_json());
    let id = common::session_id(handle).expect("session id");
    assert_eq!(
        common::prompt(handle, "pin the home"),
        0,
        "prompt failed: {:?}",
        common::last_error()
    );
    common::drain_until_turn_finished(handle);

    let log = single_log(&pinned_home);
    assert_log_layout(&log, &pinned_home, &id);
    assert!(
        log.starts_with(pinned_home.join("sessions")),
        "the session lands under the pinned home: {log:?}"
    );
    assert!(
        common::persisted_logs(&env_home).is_empty(),
        "the environment home must stay empty"
    );
    assert!(
        !env_home.join("sessions").exists(),
        "the environment home must not grow a store directory"
    );

    common::dispose(handle);
}
