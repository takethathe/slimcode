//! Ticket 02: the turn path — `prompt` / `next_event` against the mock SSE
//! endpoint, the flat event wire contract, and the real event order of a text
//! turn. Tool turns and the accessors live in `tool_turn_accessors.rs`; the
//! lifecycle (abort / dispose wake-up) in `lifecycle.rs`.

mod common;

use std::ffi::{c_char, c_void};
use std::ptr;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::json;
use slimcode_ffi::abi::events as events_abi;

use common::{MockServer, Reply, SSE_DONE};

/// One turn of the real event order, pinned: the user `message` (the prompt
/// enters history before the model is called), `turn`, the streamed
/// `reasoning` / `text`, the assistant `message` with its usage, the `usage`
/// item, `stop`, and the authoritative `turn_finished`.
#[test]
fn a_text_turn_streams_the_pinned_event_order() {
    let mock = MockServer::start(vec![Reply::Sse(vec![
        common::sse_reasoning("Thinking about it."),
        common::sse_text("Hello", None),
        common::sse_text(" world", Some("stop")),
        common::sse_usage(10, 3, 13, None),
        SSE_DONE.to_string(),
    ])]);
    let handle = common::mock_session("text-turn", &mock);

    assert_eq!(
        common::prompt(handle, "say hello"),
        0,
        "prompt failed: {:?}",
        common::last_error()
    );
    assert_eq!(
        common::last_error(),
        None,
        "a completed turn is not an error"
    );

    let events = common::drain_ready(handle);
    assert_eq!(
        common::kinds(&events),
        [
            "message",
            "turn",
            "reasoning",
            "text",
            "text",
            "message",
            "usage",
            "stop",
            "turn_finished",
        ],
        "events: {events:#?}"
    );
    // The user prompt is the first history entry, with no usage.
    assert_eq!(events[0]["message"]["kind"], "llm");
    assert_eq!(events[0]["message"]["role"], "user");
    assert_eq!(events[0]["usage"], json!(null));
    // Streamed deltas are faithful to what the endpoint sent.
    assert_eq!(events[1]["turn"], 1);
    assert_eq!(events[2]["text"], "Thinking about it.");
    assert_eq!(events[3]["text"], "Hello");
    assert_eq!(events[4]["text"], " world");
    // The assistant message carries the request's usage; the `usage` display
    // item repeats it. The endpoint omitted the cache details, so the payload
    // keeps `null` (never a normalized zero object).
    assert_eq!(events[5]["message"]["role"], "assistant");
    assert_eq!(events[5]["usage"]["total_tokens"], 13);
    assert_eq!(events[6]["usage"]["total_tokens"], 13);
    assert_eq!(events[6]["usage"]["prompt_tokens"], 10);
    assert_eq!(
        events[6]["usage"]["prompt_tokens_details"],
        json!(null),
        "the derived TokenUsage shape keeps null for absent details"
    );
    assert_eq!(events[7]["reason"], "completed");
    assert_eq!(events[8]["stop_reason"], "completed");

    // The options reached the endpoint: the prompt text in the body, the key
    // in the Authorization header.
    let requests = mock.requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].target().ends_with("/chat/completions"));
    assert_eq!(
        requests[0].header("authorization").as_deref(),
        Some("Bearer sk-test")
    );
    assert!(requests[0].body.contains("say hello"));

    common::dispose(handle);
}

/// A tool turn's `message` usage semantics: the assistant message carries the
/// request's usage (here `null`, the endpoint omitted it) and the tool-result
/// message is always usage-less. The tool call also proves the display pair
/// (`tool_start` / `tool_result`) then the history entry.
#[test]
fn a_tool_result_message_carries_no_usage() {
    let mock = MockServer::start(vec![
        Reply::Sse(vec![
            common::sse_text("Let me look. ", None),
            common::sse_tool_call("call_ls", "ls", "{\"path\":\".\"}"),
            common::sse_tool_finish(),
            SSE_DONE.to_string(),
        ]),
        Reply::Sse(vec![
            common::sse_text("There it is.", Some("stop")),
            common::sse_usage(20, 4, 24, None),
            SSE_DONE.to_string(),
        ]),
    ]);
    let handle = common::mock_session("tool-turn", &mock);

    assert_eq!(
        common::prompt(handle, "list the files"),
        0,
        "prompt failed: {:?}",
        common::last_error()
    );
    let events = common::drain_until_turn_finished(handle);
    assert_eq!(
        common::kinds(&events),
        [
            "message",
            "turn",
            "text",
            "message",
            "tool_start",
            "tool_result",
            "message",
            "turn",
            "text",
            "message",
            "usage",
            "stop",
            "turn_finished",
        ],
        "events: {events:#?}"
    );
    // The assistant message that requested the tool (endpoint omitted usage).
    assert_eq!(events[3]["message"]["role"], "assistant");
    assert_eq!(events[3]["usage"], json!(null));
    // The tool display pair, faithful to the call and its outcome.
    assert_eq!(events[4]["tool_call_id"], "call_ls");
    assert_eq!(events[4]["name"], "ls");
    assert_eq!(events[4]["arguments"], "{\"path\":\".\"}");
    assert_eq!(events[5]["ok"], true);
    // The tool-result history entry has no usage (the model request did).
    assert_eq!(events[6]["message"]["role"], "tool");
    assert_eq!(events[6]["message"]["tool_call_id"], "call_ls");
    assert_eq!(events[6]["usage"], json!(null));
    // The follow-up request's answer and usage still arrive.
    assert_eq!(events[9]["message"]["role"], "assistant");
    assert_eq!(events[9]["usage"]["total_tokens"], 24);
    assert_eq!(events[10]["usage"]["total_tokens"], 24);
    assert_eq!(events[12]["stop_reason"], "completed");
    assert_eq!(mock.request_count(), 2);

    common::dispose(handle);
}

/// A provider failure is a failed turn (`-1` + `last_error`) that emitted
/// neither `stop` nor `turn_finished`, and the session keeps working.
#[test]
fn a_provider_error_fails_the_turn_and_the_session_stays_usable() {
    let mock = MockServer::start(vec![
        Reply::Status {
            status: 500,
            body: "boom".to_string(),
        },
        Reply::Sse(vec![
            common::sse_text("Recovered.", Some("stop")),
            common::sse_usage(5, 1, 6, None),
            SSE_DONE.to_string(),
        ]),
    ]);
    let handle = common::mock_session("provider-error", &mock);

    assert_eq!(common::prompt(handle, "first"), -1);
    let message = common::last_error().expect("a failed turn sets last_error");
    assert!(
        message.contains("Bailian API error 500"),
        "unexpected error: {message}"
    );

    // The events that happened before the failed request are queued; a failed
    // turn has no `stop` and no `turn_finished` (there is nothing to finish).
    let failed = common::drain_ready(handle);
    assert_eq!(
        common::kinds(&failed),
        ["message", "turn"],
        "failed turn events: {failed:#?}"
    );

    // The session is still usable: the next prompt runs a normal turn.
    assert_eq!(
        common::prompt(handle, "second"),
        0,
        "second prompt failed: {:?}",
        common::last_error()
    );
    let events = common::drain_until_turn_finished(handle);
    assert_eq!(
        common::kinds(&events),
        [
            "message",
            "turn",
            "text",
            "message",
            "usage",
            "stop",
            "turn_finished",
        ],
        "second turn events: {events:#?}"
    );
    assert_eq!(events[6]["stop_reason"], "completed");
    assert_eq!(mock.request_count(), 2);

    common::dispose(handle);
}

/// The SDK's single-flight seam surfaces as `-1` + "a turn is already
/// running" while another thread's turn is in flight.
#[test]
fn a_second_concurrent_prompt_is_rejected() {
    // Two payloads with the mock's pause between them: the turn cannot finish
    // before the main thread observes the in-flight request.
    let mock = MockServer::start(vec![Reply::Sse(vec![
        common::sse_text("Busy", Some("stop")),
        SSE_DONE.to_string(),
    ])]);
    let handle = common::mock_session("single-flight", &mock);
    let pointer = handle as usize;

    let worker = thread::spawn(move || common::prompt(pointer as *mut c_void, "work"));
    // Wait until the request arrived: at that point the provider is blocked
    // reading the response, so the turn holds the single-flight slot.
    let deadline = Instant::now() + Duration::from_secs(5);
    while mock.request_count() == 0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(mock.request_count(), 1, "the mock never saw the request");

    assert_eq!(common::prompt(handle, "second"), -1);
    let message = common::last_error().expect("last_error");
    assert!(
        message.contains("already running"),
        "unexpected error: {message}"
    );

    assert_eq!(worker.join().expect("worker thread"), 0);
    let events = common::drain_until_turn_finished(handle);
    assert_eq!(common::kind(events.last().unwrap()), "turn_finished");
    assert_eq!(
        mock.request_count(),
        1,
        "the rejected prompt must not reach the endpoint"
    );
    common::dispose(handle);
}

/// An idle session's short timeout returns `NULL` with an EMPTY `last_error`
/// ("nothing yet"), including the `timeout_ms = 0` poll.
#[test]
fn next_event_times_out_on_an_idle_session_without_an_error() {
    let mock = MockServer::start(Vec::new());
    let handle = common::mock_session("idle-timeout", &mock);

    assert_eq!(common::next_event(handle, 50), None);
    assert_eq!(common::last_error(), None, "a timeout is not an error");
    assert_eq!(
        common::next_event(handle, 0),
        None,
        "an empty queue at 0 polls NULL"
    );
    assert_eq!(common::last_error(), None);

    common::dispose(handle);
}

/// A NULL / unknown handle at call time is the *other* NULL: `last_error`
/// tells the host which one it got.
#[test]
fn next_event_on_an_invalid_handle_is_an_error() {
    assert_eq!(common::next_event(ptr::null_mut(), 0), None);
    assert!(
        common::last_error().is_some(),
        "a NULL handle must set last_error"
    );
    assert_eq!(common::next_event(ptr::dangling_mut(), 0), None);
    assert!(
        common::last_error().is_some(),
        "an unknown handle must set last_error"
    );
}

/// `timeout_ms < 0` blocks indefinitely: the pump receives events while the
/// turn is still running, and the request's cache details survive the wire.
#[test]
fn a_pump_waiting_forever_receives_events_while_the_turn_runs() {
    let mock = MockServer::start(vec![Reply::Sse(vec![
        common::sse_text("Once ", None),
        common::sse_text("more.", Some("stop")),
        common::sse_usage(7, 2, 9, Some((4, 1))),
        SSE_DONE.to_string(),
    ])]);
    let handle = common::mock_session("pump", &mock);
    let pointer = handle as usize;

    let pump = thread::spawn(move || {
        let pointer = pointer as *mut c_void;
        let mut events = Vec::new();
        loop {
            let event = common::next_event(pointer, -1).unwrap_or_else(|| {
                panic!(
                    "the pump woke without an event; got {:?}",
                    common::kinds(&events)
                )
            });
            let finished = event["kind"] == "turn_finished";
            events.push(event);
            if finished {
                return events;
            }
        }
    });

    assert_eq!(
        common::prompt(handle, "again"),
        0,
        "prompt failed: {:?}",
        common::last_error()
    );
    let events = pump.join().expect("pump thread");

    assert_eq!(
        common::kinds(&events),
        [
            "message",
            "turn",
            "text",
            "text",
            "message",
            "usage",
            "stop",
            "turn_finished",
        ],
        "events: {events:#?}"
    );
    // The endpoint reported cache details this time: the derived shape carries
    // the object (and the fields the endpoint sent).
    assert_eq!(
        events[5]["usage"]["prompt_tokens_details"]["cached_tokens"],
        4
    );
    assert_eq!(
        events[5]["usage"]["prompt_tokens_details"]["cache_creation_input_tokens"],
        1
    );
    common::dispose(handle);
}

/// `dispose` wakes a pump already blocked in `next_event` with `NULL` and an
/// EMPTY `last_error` — the session ended, that is not a failure (the second
/// NULL kind the header documents; ticket 04 verifies it as part of the
/// lifecycle tests).
#[test]
fn dispose_wakes_a_blocked_pump_without_an_error() {
    let mock = MockServer::start(Vec::new());
    let handle = common::mock_session("dispose-wake", &mock);
    let pointer = handle as usize;

    let pump = thread::spawn(move || {
        let event = common::next_event(pointer as *mut c_void, -1);
        (event, common::last_error())
    });
    // Give the pump thread time to enter the condvar wait, then dispose.
    thread::sleep(Duration::from_millis(200));
    common::dispose(handle);

    let (event, error) = pump.join().expect("pump thread");
    assert!(event.is_none(), "dispose must wake the pump with NULL");
    assert_eq!(error, None, "the wake-up is not an error");
}

/// Empty / whitespace-only prompts (and a NULL string) are errors that never
/// reach the provider and never create a session log.
#[test]
fn an_empty_prompt_is_rejected_without_a_log() {
    let mock = MockServer::start(Vec::new());
    let (home, cwd) = common::scratch("empty-prompt");
    let options = common::options(&home, &cwd).base_url(&mock.base_url());
    let handle = common::create_ok(&options.to_json());

    for text in ["", "   ", "\t\n"] {
        assert_eq!(
            common::prompt(handle, text),
            -1,
            "{text:?} must be rejected"
        );
        let message = common::last_error().expect("last_error");
        assert!(
            message.contains("empty"),
            "{text:?}: unexpected error {message:?}"
        );
    }
    let logs = common::persisted_logs(&home);
    assert!(
        logs.is_empty(),
        "a rejected prompt must not create a log: {logs:?}"
    );

    let raw = unsafe { events_abi::slimcode_session_prompt(handle, ptr::null::<c_char>()) };
    assert_eq!(raw, -1, "a NULL prompt string must be rejected");
    assert!(common::last_error().is_some());

    assert_eq!(mock.request_count(), 0, "no request may leave the process");
    common::dispose(handle);
}
