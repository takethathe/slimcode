//! Ticket 03: the tool-turn chain and the read-only state accessors.
//!
//! A model tool call is observable end to end — the `tool_start` / `tool_result`
//! display pair, and the tool result coming back to the endpoint in the next
//! request — and between turns the host can read the session back through
//! `usage_json` / `messages_json` / `session_id` / `title`.

mod common;

use std::ffi::c_void;
use std::ptr;

use serde_json::{Value, json};

use common::{MockServer, Reply, SSE_DONE};

/// A one-turn text script (`text` then a normal stop), for the accessor tests
/// that do not care about the request shape.
fn text_turn(text: &str) -> Reply {
    Reply::Sse(vec![
        common::sse_text(text, Some("stop")),
        SSE_DONE.to_string(),
    ])
}

/// The concatenated text of one wire message's `content`, whichever shape the
/// cache flag chose (a plain string, or the one-block array the cache path
/// uses); the endpoint decides nothing here — this only reads what was sent.
fn wire_content_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks.iter().filter_map(|b| b["text"].as_str()).collect(),
        other => panic!("unexpected wire content shape: {other}"),
    }
}

/// A tool turn runs the model's call, emits the display pair in order, and the
/// follow-up request carries the tool result — the whole chain the host is
/// supposed to see.
#[test]
fn a_tool_turn_emits_the_pair_and_the_next_request_carries_the_result() {
    let mock = MockServer::start(vec![
        // First response: the model asks for `ls` and nothing else.
        Reply::Sse(vec![
            common::sse_tool_call("call_ls", "ls", "{\"path\":\".\"}"),
            common::sse_tool_finish(),
            SSE_DONE.to_string(),
        ]),
        // Second response: the answer that uses the tool's output.
        text_turn("There it is."),
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
            "message", // the user prompt enters history first
            "turn",    // turn 1: the tool-call round
            "message", // the assistant message carrying the call
            "tool_start",
            "tool_result",
            "message", // the tool result enters history
            "turn",    // turn 2: the answer that follows the tool
            "text",
            "message",
            "stop",
            "turn_finished",
        ],
        "events: {events:#?}"
    );
    assert_eq!(events[1]["turn"], 1);
    assert_eq!(events[3]["name"], "ls");
    assert_eq!(events[3]["arguments"], "{\"path\":\".\"}");
    // The tool ran against the scratch cwd, whose only entry is the `.git`
    // marker `common::scratch` created.
    assert_eq!(events[4]["ok"], true, "the ls tool call succeeded");
    assert_eq!(
        events[4]["result"].as_str().map(str::trim),
        Some(".git"),
        "the tool result carries the real listing"
    );
    assert_eq!(events[6]["turn"], 2, "the text arrives in a later turn");
    assert_eq!(events[7]["text"], "There it is.");
    assert_eq!(events[10]["stop_reason"], "completed");

    // The endpoint saw two requests: the tool call, then a replay of the whole
    // conversation with the tool result appended. The FIRST request has no
    // tool message; the second one does — the result really entered history
    // and went back to the model.
    let requests = mock.requests();
    assert_eq!(requests.len(), 2, "the tool round trip needs two requests");
    assert!(
        !requests[0].body.contains("\"role\":\"tool\""),
        "the first request cannot carry a tool result yet: {}",
        requests[0].body
    );
    let follow_up = requests[1].json();
    let messages = follow_up["messages"]
        .as_array()
        .unwrap_or_else(|| panic!("the request body has no messages array: {follow_up}"));
    let assistant = messages
        .iter()
        .find(|m| m["role"] == "assistant")
        .expect("the assistant tool-call message went back");
    assert_eq!(assistant["tool_calls"][0]["id"], "call_ls");
    assert_eq!(assistant["tool_calls"][0]["function"]["name"], "ls");
    assert_eq!(
        assistant["tool_calls"][0]["function"]["arguments"],
        "{\"path\":\".\"}"
    );
    let tool = messages
        .iter()
        .find(|m| m["role"] == "tool")
        .expect("the tool result went back");
    assert_eq!(tool["tool_call_id"], "call_ls");
    assert_eq!(
        wire_content_text(&tool["content"]).trim(),
        ".git",
        "the tool output is what the endpoint received"
    );

    common::dispose(handle);
}

/// The explicit `base_url` / `model` / `api_key` options are what the endpoint
/// sees: the request reached the mock's own host and path, and the model and
/// key are the values passed in — never the SDK's defaults.
#[test]
fn explicit_endpoint_options_reach_the_request() {
    let mock = MockServer::start(vec![text_turn("pong")]);
    let (home, cwd) = common::scratch("explicit-options");
    let options = common::options(&home, &cwd)
        .base_url(&mock.base_url())
        .model("ticket-03-model")
        .api_key("sk-ticket-03");
    let handle = common::create_ok(&options.to_json());

    assert_eq!(
        common::prompt(handle, "ping"),
        0,
        "prompt failed: {:?}",
        common::last_error()
    );

    let requests = mock.requests();
    assert_eq!(requests.len(), 1);
    // base_url: the mock's own host received it, on the base URL's path (the
    // provider appends `/chat/completions`).
    let host = mock
        .base_url()
        .trim_start_matches("http://")
        .trim_end_matches("/v1")
        .to_string();
    assert_eq!(requests[0].header("host").as_deref(), Some(host.as_str()));
    assert_eq!(requests[0].target(), "/v1/chat/completions");
    // model: in the request body.
    assert_eq!(requests[0].json()["model"], json!("ticket-03-model"));
    // api_key: in the Authorization header.
    assert_eq!(
        requests[0].header("authorization").as_deref(),
        Some("Bearer sk-ticket-03")
    );

    common::dispose(handle);
}

/// `messages_json` is the session's history as a JSON array in the session-log
/// shape, in order, and agrees with the `message` events the turn streamed.
#[test]
fn messages_json_returns_the_history_in_the_session_log_shape() {
    let mock = MockServer::start(vec![text_turn("Hello back.")]);
    let handle = common::mock_session("messages-json", &mock);

    // Before the first prompt the history is the empty array — not NULL and
    // not an object.
    assert_eq!(common::messages_json(handle), Some(json!([])));

    assert_eq!(
        common::prompt(handle, "hello there"),
        0,
        "prompt failed: {:?}",
        common::last_error()
    );
    let events = common::drain_until_turn_finished(handle);

    let messages = common::messages_json(handle).expect("messages_json");
    let history = messages
        .as_array()
        .unwrap_or_else(|| panic!("messages_json must be a JSON array: {messages}"));
    assert_eq!(
        history.len(),
        2,
        "the prompt plus the assistant reply: {messages}"
    );
    assert_eq!(history[0]["kind"], "llm");
    assert_eq!(history[0]["role"], "user");
    assert_eq!(history[0]["parts"][0]["type"], "text");
    assert_eq!(history[0]["parts"][0]["text"], "hello there");
    assert_eq!(history[1]["kind"], "llm");
    assert_eq!(history[1]["role"], "assistant");
    assert_eq!(history[1]["parts"][0]["text"], "Hello back.");

    // The accessor and the event stream describe the same history: the
    // `message` payloads, in order, are exactly this array.
    let streamed: Vec<Value> = events
        .iter()
        .filter(|event| event["kind"] == "message")
        .map(|event| event["message"].clone())
        .collect();
    assert_eq!(
        messages,
        Value::Array(streamed),
        "messages_json must agree with the message events"
    );

    common::dispose(handle);
}

/// `usage_json` is the SDK's accumulated session usage: the endpoint's token
/// numbers for every request of the turn, summed, in the `TokenUsage` shape.
#[test]
fn usage_json_accumulates_the_turns_reported_usage() {
    // A tool turn makes two requests and the endpoint reports usage for both:
    // the accessor must show the sum, not just the last request.
    let mock = MockServer::start(vec![
        Reply::Sse(vec![
            common::sse_tool_call("call_ls", "ls", "{\"path\":\".\"}"),
            common::sse_tool_finish(),
            common::sse_usage(11, 5, 16, Some((3, 1))),
            SSE_DONE.to_string(),
        ]),
        Reply::Sse(vec![
            common::sse_text("done", Some("stop")),
            common::sse_usage(20, 4, 24, Some((2, 0))),
            SSE_DONE.to_string(),
        ]),
    ]);
    let handle = common::mock_session("usage-json", &mock);

    // A fresh session reports zeros; absent cache details stay `null`, the
    // same shape the `usage` event carries.
    assert_eq!(
        common::usage_json(handle),
        Some(json!({
            "prompt_tokens": 0,
            "completion_tokens": 0,
            "total_tokens": 0,
            "prompt_tokens_details": null,
        }))
    );

    assert_eq!(
        common::prompt(handle, "list"),
        0,
        "prompt failed: {:?}",
        common::last_error()
    );

    let usage = common::usage_json(handle).expect("usage_json");
    assert_eq!(
        usage,
        json!({
            "prompt_tokens": 31,          // 11 + 20
            "completion_tokens": 9,       // 5 + 4
            "total_tokens": 40,           // 16 + 24
            "prompt_tokens_details": {
                "cached_tokens": 5,       // 3 + 2
                "cache_creation_input_tokens": 1, // 1 + 0
            },
        }),
        "the session total is the sum of both requests' usage"
    );

    common::dispose(handle);
}

/// The title is the SDK's inference from the first prompt: empty before any
/// prompt, the prompt text (truncated by the SDK) after one.
#[test]
fn the_title_is_inferred_from_the_first_prompt() {
    let mock = MockServer::start(vec![text_turn("Noted.")]);
    let handle = common::mock_session("title", &mock);

    assert_eq!(
        common::title(handle).as_deref(),
        Some(""),
        "a session without a prompt has the empty title"
    );

    assert_eq!(
        common::prompt(handle, "summarize the release notes"),
        0,
        "prompt failed: {:?}",
        common::last_error()
    );

    let title = common::title(handle).expect("title is never NULL for a live handle");
    assert!(!title.is_empty(), "the first prompt produced a title");
    assert_eq!(title, "summarize the release notes");

    common::dispose(handle);
}

/// The accessors follow the handle error contract: a NULL / disposed handle is
/// `NULL` plus `last_error`, never a panic and never an empty payload.
#[test]
fn the_state_accessors_reject_null_and_disposed_handles() {
    // No turn runs here, so no endpoint is needed.
    let (home, cwd) = common::scratch("accessor-errors");
    let handle = common::create_ok(&common::options(&home, &cwd).to_json());
    common::dispose(handle);

    for pointer in [ptr::null_mut::<c_void>(), handle] {
        assert_eq!(common::usage_json(pointer), None);
        assert!(
            common::last_error().is_some(),
            "usage_json on {pointer:?} must set last_error"
        );
        assert_eq!(common::messages_json(pointer), None);
        assert!(
            common::last_error().is_some(),
            "messages_json on {pointer:?} must set last_error"
        );
    }
}
