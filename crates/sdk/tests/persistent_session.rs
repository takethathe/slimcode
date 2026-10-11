//! The SDK's persistent session mode: one synchronous prompt and its event
//! stream, the JSONL log it persists, and cross-turn history continuity —
//! driven through the public `AgentSession` API only (spec Testing Decisions).
//!
//! A scripted `FakeProvider` injected via `AgentSessionOptions.provider` runs
//! against the real shared turn loop; assertions cover event ordering and the
//! `TurnFinished` end signal, per-message `SdkEvent::Message` emission,
//! persisted-log replay equivalence, provider-error and cancel closing
//! messages, multi-turn history continuity, and the empty-prompt rejection.

mod common;
use common::*;

use std::sync::{Arc, Mutex};

use slimcode_app::render::DisplayItem;
use slimcode_app::{Message, Role, StopReason};

use slimcode_sdk::SdkEvent;

// --- behavior -------------------------------------------------------------

#[test]
fn prompt_streams_ordered_display_and_ends_with_turn_finished() {
    let (_guard, _home, cwd) = test_env("stream");
    let provider = FakeProvider::new(vec![vec![text("hello "), text("world"), done_stop()]])
        .with_per_call(usage(10, 5));
    let session = make_session(&cwd, Box::new(provider), Vec::new());
    let recorder = Recorder::new();
    let _unsubscribe = session.subscribe(recorder.listener());

    let stop = session.prompt("hi").expect("prompt");
    assert_eq!(stop, StopReason::Completed);

    let events = recorder.events();
    // The authoritative end signal is the very last event of the turn.
    assert!(matches!(
        events.last(),
        Some(SdkEvent::TurnFinished {
            stop_reason: StopReason::Completed
        })
    ));
    // The user prompt's Message event opens the turn, before the stream.
    assert!(matches!(
        events.first(),
        Some(SdkEvent::Message { message, usage: None })
            if message.text_content() == "hi"
    ));
    // Display items stream in order: a turn marker, the text fragments, stop.
    let displays = recorder.displays();
    assert_eq!(displays.first(), Some(&DisplayItem::Turn { turn: 1 }));
    let texts: Vec<String> = displays
        .iter()
        .filter_map(|d| match d {
            DisplayItem::Text(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, vec!["hello ", "world"]);
    assert!(matches!(
        displays.last(),
        Some(DisplayItem::Stop(StopReason::Completed))
    ));
    // The session records the prompt and the reply.
    let messages = session.messages();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].text_content(), "hi");
    assert_eq!(messages[1].text_content(), "hello world");
    assert!(session.usage().total_tokens > 0);
}

/// User Story 5 taken literally: every message that enters history — the
/// user prompt included — emits a `Message` event; assistant replies carry
/// usage, the user prompt and tool results carry `None`.
#[test]
fn every_history_entry_emits_a_message_event() {
    let (_guard, _home, cwd) = test_env("messages");
    let per_call = usage(10, 5);
    let provider = FakeProvider::new(vec![
        vec![
            text("checking "),
            tc_start(0, "call_1", "probe"),
            tc_args(0, "{}"),
            done_tools(),
        ],
        vec![text("probed!"), done_stop()],
    ])
    .with_per_call(per_call);
    let session = make_session(&cwd, Box::new(provider), vec![probe_tool()]);
    let recorder = Recorder::new();
    let _unsubscribe = session.subscribe(recorder.listener());
    session.prompt("go").expect("prompt");

    let messages = recorder.message_events();
    // user prompt + assistant (with the tool call) + tool result + final
    // assistant — one event per history entry.
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0].0.role(), &Role::User);
    assert_eq!(messages[0].0.text_content(), "go");
    assert_eq!(messages[0].1, None);
    assert_eq!(messages[1].0.role(), &Role::Assistant);
    assert_eq!(messages[1].0.tool_calls().len(), 1);
    assert_eq!(messages[1].1, Some(per_call));
    assert_eq!(messages[2].0.role(), &Role::Tool);
    assert_eq!(messages[2].1, None);
    assert_eq!(messages[3].0.role(), &Role::Assistant);
    assert_eq!(messages[3].0.text_content(), "probed!");
    assert_eq!(messages[3].1, Some(per_call));
    // The same history is mirrored into the session.
    let live = session.messages();
    assert_eq!(live.len(), 4); // user + assistant(tool_calls) + tool + assistant
    assert_eq!(live[0], messages[0].0);
    assert_eq!(live[1], messages[1].0);
    assert_eq!(live[2], messages[2].0);
    assert_eq!(live[3], messages[3].0);
}

/// Spec §3: an empty / whitespace-only prompt is rejected immediately — no
/// turn, no events, no session log, and the session still works afterwards.
#[test]
fn empty_prompt_is_rejected_without_a_turn_or_files() {
    let (_guard, home, cwd) = test_env("empty-prompt");
    let provider = FakeProvider::new(vec![vec![text("reply"), done_stop()]]);
    let session = make_session(&cwd, Box::new(provider), Vec::new());
    let recorder = Recorder::new();
    let _unsubscribe = session.subscribe(recorder.listener());

    for empty in ["", "   ", "\t\n"] {
        let err = session.prompt(empty).unwrap_err();
        assert_eq!(err, "prompt text must not be empty", "input: {empty:?}");
    }
    // No turn ran: no events reached the subscriber and no log exists.
    assert!(recorder.events().is_empty(), "{:?}", recorder.events());
    assert_eq!(count_jsonl(&home.join("sessions")), 0);
    // The session is not damaged by the rejections.
    let stop = session.prompt("real").expect("prompt");
    assert_eq!(stop, StopReason::Completed);
    assert_eq!(session.messages().len(), 2); // user + assistant
}

#[test]
fn persistent_session_replays_identically_from_the_log() {
    let (_guard, home, cwd) = test_env("persist");
    let provider = FakeProvider::new(vec![vec![text("hi there"), done_stop()]]);
    let session = make_session(&cwd, Box::new(provider), Vec::new());
    session.prompt("first prompt").expect("prompt");

    // The log file exists under `<home>/sessions/<project-key>/<id>.jsonl`.
    let store = store_for(&home, &cwd);
    let path = store.session_path(session.session_id()).expect("path");
    assert!(path.exists(), "log at {}", path.display());

    // Replaying the log yields exactly the live session's messages + title.
    let outcome = store.load(session.session_id()).expect("load");
    assert_eq!(outcome.session.messages, session.messages());
    assert_eq!(outcome.session.title, session.title());
    assert_eq!(outcome.session.title.as_deref(), Some("first prompt"));
    assert_eq!(outcome.skipped_records, 0);
}

#[test]
fn provider_error_returns_err_and_closes_the_log_on_an_assistant_boundary() {
    let (_guard, home, cwd) = test_env("error");
    let provider = FakeProvider::new(vec![vec![text("first reply"), done_stop()]]).fails_on(1);
    let session = make_session(&cwd, Box::new(provider), Vec::new());
    session.prompt("first").expect("first prompt");

    let err = session.prompt("second").unwrap_err();
    assert!(err.contains("provider exploded"), "err: {err}");
    // Prior messages stay in the live session, plus the error closing message.
    let live = session.messages();
    assert_eq!(live.len(), 4); // user1, assistant1, user2, closing
    let last = live.last().expect("closing message");
    assert_eq!(last.role(), &Role::Assistant);
    assert!(
        last.text_content().contains("error"),
        "{}",
        last.text_content()
    );

    // The log ends on an assistant boundary with the same closing message.
    let store = store_for(&home, &cwd);
    let outcome = store.load(session.session_id()).expect("load");
    assert_eq!(outcome.session.messages, live);
    let replayed_last = outcome.session.messages.last().expect("closing");
    assert_eq!(replayed_last, last);
    assert_eq!(outcome.skipped_records, 0);
}

#[test]
fn cancelled_prompt_returns_cancelled_and_closes_the_log_as_aborted() {
    let (_guard, home, cwd) = test_env("cancel");
    let provider = FakeProvider::new(vec![
        vec![text("first reply"), done_stop()],
        vec![text("partial"), done_stop()],
    ])
    .cancels_on(2);
    let session = make_session(&cwd, Box::new(provider), Vec::new());
    session.prompt("first").expect("first prompt");

    let stop = session.prompt("second").expect("prompt");
    assert_eq!(stop, StopReason::Cancelled);
    // The cancelled request streamed its partial text but no assistant message
    // entered history; the log is closed with the Aborted message.
    let live = session.messages();
    assert_eq!(live.len(), 4); // user1, assistant1, user2, closing
    let last = live.last().expect("closing message");
    assert_eq!(last.text_content(), "The turn was cancelled.");

    let store = store_for(&home, &cwd);
    let outcome = store.load(session.session_id()).expect("load");
    assert_eq!(outcome.session.messages, live);
    let replayed_last = outcome.session.messages.last().expect("closing");
    assert_eq!(replayed_last.text_content(), "The turn was cancelled.");
}

#[test]
fn second_prompt_sees_the_first_turns_history_and_the_log_stays_continuous() {
    let (_guard, home, cwd) = test_env("multi");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let provider = FakeProvider::new(vec![
        vec![text("reply one"), done_stop()],
        vec![text("reply two"), done_stop()],
    ])
    .records_into(seen.clone());
    let session = make_session(&cwd, Box::new(provider), Vec::new());
    let recorder = Recorder::new();
    let _unsubscribe = session.subscribe(recorder.listener());
    session.prompt("first").expect("first");
    session.prompt("second").expect("second");

    // Accumulated history: user1, assistant1, user2, assistant2.
    let live = session.messages();
    assert_eq!(live.len(), 4);
    assert_eq!(live[0].text_content(), "first");
    assert_eq!(live[1].text_content(), "reply one");
    assert_eq!(live[2].text_content(), "second");
    assert_eq!(live[3].text_content(), "reply two");

    // The second provider call saw the first turn's history on the wire
    // (the runner prepends the assembled system message per request, so
    // filter it out — the system prompt is not session history, ADR-0012 D3).
    let seen = seen.lock().expect("seen lock");
    assert_eq!(seen.len(), 2);
    let second_call = &seen[1];
    let texts: Vec<String> = second_call
        .iter()
        .filter(|m| m.role != Role::System)
        .map(Message::text_content)
        .collect();
    assert_eq!(texts, vec!["first", "reply one", "second"]);

    // The persisted log replays the same continuity.
    let store = store_for(&home, &cwd);
    let outcome = store.load(session.session_id()).expect("load");
    assert_eq!(outcome.session.messages, live);

    // Each prompt produced exactly one authoritative end signal.
    let finished = recorder
        .events()
        .into_iter()
        .filter(|e| matches!(e, SdkEvent::TurnFinished { .. }))
        .count();
    assert_eq!(finished, 2);
}

#[test]
fn single_flight_rejects_a_second_prompt_while_one_runs() {
    let (_guard, _home, cwd) = test_env("singleflight");
    // The provider signals it started, then blocks until released, so the
    // first prompt is provably in flight while the second prompt is rejected.
    let started = Arc::new(Mutex::new(false));
    let release = Arc::new(Mutex::new(false));
    let provider = BlockingProvider {
        started: started.clone(),
        release: release.clone(),
    };
    let session = Arc::new(make_session(&cwd, Box::new(provider), Vec::new()));

    let session2 = Arc::clone(&session);
    let handle = std::thread::spawn(move || session2.prompt("first"));
    // Wait until the first turn is inside the provider, then reject.
    wait_until(&started);
    let err = session.prompt("second").unwrap_err();
    assert!(err.contains("a turn is already running"), "err: {err}");
    *release.lock().expect("release lock") = true;
    let first = handle.join().expect("prompt thread");
    assert_eq!(first.expect("first prompt"), StopReason::Completed);
}
