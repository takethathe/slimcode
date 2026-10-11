//! Ticket 06: `compact_now` — forcing one compaction outside a turn.
//!
//! One C ABI symbol drives the SDK's `/compact` semantics: the call returns
//! `{"before":N,"after":M}`, the compaction arrives in the handle's ordinary
//! event queue (the ticket-02 listener already pushes every `SdkEvent`, so the
//! `compacted` event costs this ticket nothing), and a failure is `NULL` plus
//! `last_error` with the history untouched and the session still usable.
//!
//! The assertions stay on the two pre-agreed seams: the C ABI functions and
//! the mock SSE endpoint.

mod common;

use std::ffi::c_void;
use std::ptr;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use common::{MockServer, Reply, SSE_DONE};

/// `estimate_message_tokens` is `ceil(chars / 4)` and the retained budget is
/// 8% of the 200 000-token window = 16 000 estimated tokens, so a history must
/// estimate above that to have anything to summarize. One prompt of 70 000
/// characters estimates 17 500 tokens — the cheapest compaction-eligible
/// history — and the boundary then keeps the newest message (the reply).
const LONG_PROMPT_CHARS: usize = 70_000;

/// The `## Goal`-shaped summary the mock serves for the compaction's own
/// provider request: 28 characters, i.e. `ceil(28 / 4) = 7` estimated tokens.
const SUMMARY: &str = "## Goal\nCompact the history.";

/// One SSE reply: a single assistant text chunk that closes the model's answer
/// (`finish_reason: "stop"`) plus the `[DONE]` tail.
fn text_turn(text: &str) -> common::Reply {
    common::Reply::Sse(vec![
        common::sse_text(text, Some("stop")),
        SSE_DONE.to_string(),
    ])
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

/// The success path end to end: the call returns the token change, the
/// `compacted` event carries the same numbers, and `messages_json` shows the
/// old span replaced by a `compact_summary` record plus the kept tail.
#[test]
fn compact_now_returns_the_token_change_and_replaces_the_history() {
    let mock = MockServer::start(vec![
        // Request 1: the turn that fills the history.
        text_turn("Noted."),
        // Request 2: the summarization call a forced compaction makes.
        text_turn(SUMMARY),
    ]);
    let handle = common::mock_session("compact-now", &mock);

    assert_eq!(
        common::prompt(handle, &"x".repeat(LONG_PROMPT_CHARS)),
        0,
        "prompt failed: {:?}",
        common::last_error()
    );
    common::drain_until_turn_finished(handle);
    let before_history = common::messages_json(handle).expect("messages_json");
    assert_eq!(
        before_history.as_array().map(Vec::len),
        Some(2),
        "the turn left the prompt and the reply: {before_history}"
    );

    let compacted = common::compact_now(handle)
        .unwrap_or_else(|| panic!("compact_now failed: {:?}", common::last_error()));

    // `before` is the whole history's estimate: ceil(70 000 / 4) = 17 500 for
    // the prompt plus ceil(6 / 4) = 2 for "Noted."; `after` is the checkpoint
    // (ceil(28 / 4) = 7) plus the same 2-token reply kept as the tail.
    assert_eq!(compacted["before"], 17_502, "compacted: {compacted}");
    assert_eq!(compacted["after"], 9, "compacted: {compacted}");
    assert!(
        compacted["before"].as_u64() > compacted["after"].as_u64(),
        "compaction must shrink the estimate: {compacted}"
    );

    // The same numbers reach the host through the ordinary event queue.
    let events = common::drain_ready(handle);
    assert_eq!(common::kinds(&events), ["compacted"], "events: {events:#?}");
    assert_eq!(events[0]["before"], compacted["before"]);
    assert_eq!(events[0]["after"], compacted["after"]);

    // The history is now `[compact_summary, kept tail]`: the checkpoint
    // records the pre-compaction estimate, the tail is the turn's reply, and
    // the summarized prompt is gone.
    let history = common::messages_json(handle).expect("messages_json");
    let history = history
        .as_array()
        .unwrap_or_else(|| panic!("messages_json must be a JSON array: {history}"));
    assert_eq!(history.len(), 2, "history: {history:#?}");
    assert_eq!(history[0]["kind"], "compact_summary");
    assert_eq!(history[0]["summary"], SUMMARY);
    assert_eq!(history[0]["tokens_before"], compacted["before"]);
    assert_eq!(history[1]["kind"], "llm");
    assert_eq!(history[1]["role"], "assistant");
    assert_eq!(message_text(&history[1]), "Noted.");

    // The summarization request really went to the endpoint, carrying the
    // serialized conversation the checkpoint replaced.
    assert_eq!(mock.request_count(), 2, "one turn plus one summarization");
    let requests = mock.requests();
    assert!(
        requests[1].body.contains("<conversation>"),
        "the second request is the summarization one: {}",
        requests[1].body
    );

    common::dispose(handle);
}

/// A history that fits the retained budget has nothing to compact: `NULL` plus
/// the SDK's own reason, with the history and the queue untouched — and no
/// summarization request. A NULL / disposed handle follows the same failure
/// contract (`NULL` + `last_error`), so a stale host pointer never panics.
#[test]
fn nothing_to_compact_and_invalid_handles_follow_the_failure_contract() {
    let mock = MockServer::start(vec![text_turn("Hi.")]);
    let handle = common::mock_session("compact-nothing", &mock);

    assert_eq!(
        common::prompt(handle, "hello"),
        0,
        "prompt failed: {:?}",
        common::last_error()
    );
    common::drain_until_turn_finished(handle);
    let before_history = common::messages_json(handle).expect("messages_json");

    assert_eq!(
        common::compact_now(handle),
        None,
        "a history below the budget must not compact"
    );
    let message = common::last_error().expect("a failed compaction sets last_error");
    assert!(
        message.contains("nothing to compact"),
        "unexpected error: {message}"
    );
    assert_eq!(
        common::messages_json(handle),
        Some(before_history),
        "a refused compaction must leave the history alone"
    );
    assert!(
        common::drain_ready(handle).is_empty(),
        "a refused compaction must queue no event"
    );
    assert_eq!(
        mock.request_count(),
        1,
        "an ineligible history must not reach the endpoint"
    );

    // The invalid-handle contract: NULL and a disposed pointer are both
    // `NULL` + last_error, never a panic and never an empty payload.
    common::dispose(handle);
    for pointer in [ptr::null_mut::<c_void>(), handle] {
        assert_eq!(common::compact_now(pointer), None);
        assert!(
            common::last_error().is_some(),
            "compact_now on {pointer:?} must set last_error"
        );
    }
}

/// A failed summarization request is `NULL` plus `last_error`: the history is
/// exactly the one from before the call, no `compacted` event is queued, and
/// the next prompt on the same handle still runs a normal turn.
#[test]
fn a_failed_summarization_keeps_the_history_and_the_session_usable() {
    let mock = MockServer::start(vec![
        // Request 1: the turn that fills the history.
        text_turn("Noted."),
        // Request 2: the summarization call fails.
        Reply::Status {
            status: 500,
            body: "summarizer exploded".to_string(),
        },
        // Request 3: the recovery turn.
        text_turn("Still here."),
    ]);
    let handle = common::mock_session("compact-failure", &mock);

    assert_eq!(
        common::prompt(handle, &"x".repeat(LONG_PROMPT_CHARS)),
        0,
        "prompt failed: {:?}",
        common::last_error()
    );
    common::drain_until_turn_finished(handle);
    let before_history = common::messages_json(handle).expect("messages_json");

    assert_eq!(
        common::compact_now(handle),
        None,
        "a failed compaction must return NULL"
    );
    let message = common::last_error().expect("a failed compaction sets last_error");
    assert!(!message.is_empty(), "last_error must carry the reason");

    // The history is untouched and no event was queued for the failure.
    assert_eq!(
        common::messages_json(handle),
        Some(before_history),
        "a failed compaction must leave the history alone"
    );
    assert!(
        common::drain_ready(handle).is_empty(),
        "a failed compaction must queue no event"
    );

    // The session stays usable: the next prompt runs a normal turn.
    assert_eq!(
        common::prompt(handle, "carry on"),
        0,
        "the session must survive a failed compaction: {:?}",
        common::last_error()
    );
    common::drain_until_turn_finished(handle);
    let history = common::messages_json(handle).expect("messages_json");
    assert_eq!(
        history.as_array().map(Vec::len),
        Some(4),
        "the recovery turn appended the prompt and the reply: {history}"
    );
    assert_eq!(
        mock.request_count(),
        3,
        "one turn, one failed summary, one recovery turn"
    );

    common::dispose(handle);
}

/// `compact_now` shares the SDK's single-flight slot with `prompt`: while a
/// turn is in flight it is rejected with `NULL` + "a turn is already running",
/// never reaches the endpoint, and the turn itself finishes normally.
#[test]
fn compact_now_is_rejected_while_a_turn_is_in_flight() {
    // Four payloads, so the mock's pause between them keeps the turn in flight
    // long enough for the main thread to observe it.
    let mock = MockServer::start(vec![Reply::Sse(vec![
        common::sse_reasoning("Thinking."),
        common::sse_text("Busy", None),
        common::sse_text(" turn", Some("stop")),
        SSE_DONE.to_string(),
    ])]);
    let handle = common::mock_session("compact-single-flight", &mock);
    let pointer = handle as usize;

    let worker = thread::spawn(move || common::prompt(pointer as *mut c_void, "a slow turn"));
    // Wait until the request arrived: the turn takes the single-flight slot
    // before it calls the provider, so the slot is held from here on.
    let deadline = Instant::now() + Duration::from_secs(5);
    while mock.request_count() == 0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(mock.request_count(), 1, "the mock never saw the request");

    assert_eq!(
        common::compact_now(handle),
        None,
        "an in-flight turn must reject compact_now"
    );
    let message = common::last_error().expect("last_error");
    assert!(
        message.contains("a turn is already running"),
        "unexpected error: {message}"
    );

    assert_eq!(
        worker.join().expect("worker thread"),
        0,
        "the turn must finish normally: {:?}",
        common::last_error()
    );
    let events = common::drain_until_turn_finished(handle);
    assert_eq!(common::kind(events.last().unwrap()), "turn_finished");
    assert_eq!(
        mock.request_count(),
        1,
        "the rejected compact_now must not reach the endpoint"
    );

    common::dispose(handle);
}

/// The other single-flight direction: while a `compact_now` is in flight, a
/// `prompt` is rejected with the same error, and the compaction still completes
/// and queues its event.
#[test]
fn prompt_is_rejected_while_a_compaction_is_in_flight() {
    let mock = MockServer::start(vec![
        // Request 1: the turn that fills the history.
        text_turn("Noted."),
        // Request 2: a slow summarization — the mock's pauses between these
        // payloads keep compaction in flight while the main thread prompts.
        Reply::Sse(vec![
            common::sse_reasoning("Summarizing."),
            common::sse_text("## Goal", None),
            common::sse_text("\nCompact the history.", Some("stop")),
            SSE_DONE.to_string(),
        ]),
    ]);
    let handle = common::mock_session("compact-reverse-single-flight", &mock);

    assert_eq!(
        common::prompt(handle, &"x".repeat(LONG_PROMPT_CHARS)),
        0,
        "prompt failed: {:?}",
        common::last_error()
    );
    common::drain_until_turn_finished(handle);

    let pointer = handle as usize;
    let worker = thread::spawn(move || {
        common::compact_now(pointer as *mut c_void)
            .ok_or_else(|| format!("{:?}", common::last_error()))
    });
    // Wait for the summarization request: compaction holds the slot from
    // before that request, so a prompt from here on must be rejected.
    let deadline = Instant::now() + Duration::from_secs(5);
    while mock.request_count() < 2 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(mock.request_count(), 2, "the summary request never arrived");

    assert_eq!(
        common::prompt(handle, "second"),
        -1,
        "an in-flight compaction must reject a prompt"
    );
    let message = common::last_error().expect("last_error");
    assert!(
        message.contains("a turn is already running"),
        "unexpected error: {message}"
    );

    let compacted = worker
        .join()
        .expect("the compaction worker")
        .unwrap_or_else(|e| panic!("compact_now failed: {e}"));
    // The rejected prompt did not disturb the compaction: it completed, and
    // its event carries the numbers it returned.
    let events = common::drain_ready(handle);
    assert_eq!(common::kinds(&events), ["compacted"], "events: {events:#?}");
    assert_eq!(events[0]["before"], compacted["before"]);
    assert_eq!(events[0]["after"], compacted["after"]);
    assert_eq!(
        mock.request_count(),
        2,
        "the rejected prompt must not reach the endpoint"
    );

    common::dispose(handle);
}
