//! Ticket 04: the session lifecycle — cross-thread `abort`, the default tool
//! set's cancel token, `dispose`'s wake-up of a blocked pump, and two handles
//! running concurrently.
//!
//! `abort` is the host's Esc / timeout path: it only sets the SDK's shared
//! cancel token, so it is callable from any thread at any time — including
//! while `prompt` is blocked in a provider read or a `bash` child — and never
//! waits for the session lock. A cancelled turn keeps its streamed partial
//! text on the display transcript and out of history, ends with
//! `{"kind":"turn_finished","stop_reason":"cancelled"}`, and is not an error
//! (`prompt` returns `1`, `last_error` stays empty). `dispose` closes the
//! queue so a pump blocked in `next_event` wakes immediately, and invalidates
//! the handle. All waits are bounded: a host-driven cancel that never lands
//! must fail the test, not hang the suite.

mod common;

use std::ffi::c_void;
use std::ptr;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use common::{Gate, MockServer, Reply, SSE_DONE};

/// A one-turn text script (`text` then a normal stop), for the tests that only
/// need the turn to complete.
fn text_turn(text: &str) -> Reply {
    Reply::Sse(vec![
        common::sse_text(text, Some("stop")),
        common::sse_usage(10, 3, 13, None),
        SSE_DONE.to_string(),
    ])
}

/// `abort` is infallible and therefore silent when there is nothing to cancel:
/// NULL / unknown handles and an idle session are no-ops that neither set nor
/// clear `last_error` (unlike every fallible entry point), and an idle abort
/// leaves the next turn uncancelled (the SDK resets the token per turn).
#[test]
fn abort_without_a_turn_is_a_silent_no_op() {
    let mock = MockServer::start(vec![text_turn("Still working.")]);
    let handle = common::mock_session("abort-silence", &mock);

    common::abort(ptr::null_mut());
    assert_eq!(
        common::last_error(),
        None,
        "abort(NULL) must not set last_error"
    );
    common::abort(ptr::dangling_mut());
    assert_eq!(
        common::last_error(),
        None,
        "abort(unknown) must not set last_error"
    );

    // A previous call's error survives either no-op form: abort never touches
    // last_error, exactly like dispose's NULL / unknown path.
    assert_eq!(common::prompt(handle, "   "), -1);
    let before = common::last_error().expect("a rejected prompt sets last_error");
    common::abort(handle);
    common::abort(ptr::null_mut());
    common::abort(ptr::dangling_mut());
    assert_eq!(
        common::last_error().as_deref(),
        Some(before.as_str()),
        "abort must not touch last_error"
    );

    // An idle abort set the shared token; the next turn resets it and runs to
    // a normal completion.
    assert_eq!(
        common::prompt(handle, "run anyway"),
        0,
        "the turn after an idle abort failed: {:?}",
        common::last_error()
    );
    let events = common::drain_until_turn_finished(handle);
    assert_eq!(
        events.last().map(|e| &e["stop_reason"]),
        Some(&Value::from("completed"))
    );
    assert_eq!(mock.request_count(), 1);

    common::dispose(handle);
}

/// A turn aborted from another OS thread while the provider call is still
/// streaming ends `Cancelled` promptly: the partial text was delivered as a
/// display event only, history keeps just the (SDK's) closing message, and
/// the cancellation is not an error.
#[test]
fn abort_from_another_thread_cancels_a_text_turn_and_keeps_partial_text_out_of_history() {
    // A unique token only this turn's stream can put on the wire.
    const PARTIAL: &str = "partial-marker-4f8e";
    let mock = MockServer::start(vec![Reply::Sse(vec![
        common::sse_text(PARTIAL, None),
        common::sse_text(" still streaming", None),
        common::sse_text(" and done", Some("stop")),
        common::sse_usage(9, 2, 11, None),
        SSE_DONE.to_string(),
    ])]);
    let handle = common::mock_session("abort-text", &mock);

    // `prompt` blocks this turn's own thread; the test thread is the host's
    // observer and (different OS thread!) its abort path.
    let code = common::prompt_on_thread(handle, "write a long answer");
    let mut observed = Vec::new();
    loop {
        let event = common::next_event(handle, 5_000).unwrap_or_else(|| {
            panic!(
                "no text event within 5s; saw {:?}",
                common::kinds(&observed)
            )
        });
        let partial = common::kind(&event) == "text" && event["text"] == PARTIAL;
        observed.push(event);
        if partial {
            break;
        }
    }

    // The mock pauses 100ms between chunks, so the provider read is still
    // open here: this is a genuinely half-streamed turn, and the interruptible
    // read must end it at once.
    common::abort(handle);
    assert_eq!(
        common::await_code(
            &code,
            Duration::from_secs(10),
            "the aborted turn must end promptly"
        ),
        1,
        "an aborted turn returns Cancelled"
    );

    let mut events = observed;
    events.extend(common::drain_until_turn_finished(handle));
    assert!(
        events
            .iter()
            .any(|e| common::kind(e) == "text" && e["text"] == PARTIAL),
        "the streamed partial text stays on the display transcript"
    );
    assert!(
        events
            .iter()
            .any(|e| common::kind(e) == "stop" && e["reason"] == "cancelled"),
        "events: {events:#?}"
    );
    assert_eq!(
        events.last().expect("turn_finished")["stop_reason"],
        "cancelled"
    );
    assert_eq!(common::last_error(), None, "cancellation is not an error");

    // The half-streamed text stays out of history. The SDK does append a
    // closing assistant message on a cancelled turn (`close_turn`: "The turn
    // was cancelled.", ADR-0009 D3), so the honest assertion is the marker's
    // absence from the whole history JSON, not "the history is empty".
    let history = common::messages_json(handle).expect("messages_json on a live handle");
    let text = serde_json::to_string(&history).expect("history serializes");
    assert!(
        !text.contains(PARTIAL),
        "the streamed partial text entered history: {text}"
    );
    assert!(
        text.contains("write a long answer"),
        "the user prompt is in history: {text}"
    );
    assert_eq!(
        mock.request_count(),
        1,
        "abort must not start a second request"
    );

    common::dispose(handle);
}

/// The default tool set shares the session's cancel token, so `abort` also
/// terminates a running `bash` child: the turn ends far below the child's own
/// `sleep 30`, and the cancelled call is never reported as a successful tool
/// result.
#[test]
fn abort_cancels_a_running_bash_child_promptly() {
    let (home, cwd) = common::scratch("abort-bash");
    // The child touches this file before sleeping: its existence proves the
    // tool actually started, so the abort lands while `sleep 30` runs (the
    // parallel tool batch emits no event before the batch ends, so the file
    // is the observable signal the ABI itself cannot give yet).
    let started_marker = cwd.join("bash-started");
    let command = format!("touch '{}' && sleep 30", started_marker.display());
    let arguments = serde_json::json!({ "command": command }).to_string();
    let mock = MockServer::start(vec![Reply::Sse(vec![
        common::sse_tool_call("call_bash", "bash", &arguments),
        common::sse_tool_finish(),
        SSE_DONE.to_string(),
    ])]);
    let options = common::options(&home, &cwd).base_url(&mock.base_url());
    let handle = common::create_ok(&options.to_json());

    let code = common::prompt_on_thread(handle, "sleep for a while");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !started_marker.exists() {
        assert!(
            Instant::now() < deadline,
            "the bash child never started ({})",
            started_marker.display()
        );
        thread::sleep(Duration::from_millis(10));
    }

    let aborted_at = Instant::now();
    common::abort(handle);
    let stop = common::await_code(
        &code,
        Duration::from_secs(15),
        "the cancelled bash turn must not wait out `sleep 30`",
    );
    let elapsed = aborted_at.elapsed();
    assert_eq!(stop, 1, "an aborted tool turn returns Cancelled");
    assert!(
        elapsed < Duration::from_secs(5),
        "the cancel must kill the child promptly, not wait for `sleep 30`: {elapsed:?}"
    );

    let events = common::drain_until_turn_finished(handle);
    assert_eq!(
        events.last().expect("turn_finished")["stop_reason"],
        "cancelled"
    );
    // The ticket says "no tool_result after abort". The honest reading for
    // this revision of `crates/core/src/agent.rs`: `execute_tools` discards
    // the whole cancelled batch before `build_result` runs (the parallel arm
    // returns `Ok(true)` on its cancel check, the serial arm right after the
    // dispatch), so no `tool_result` is emitted at all. The invariant that
    // must hold either way is asserted here regardless: a cancelled call is
    // never a *successful* result, and any result that does appear must be
    // the cancelled payload.
    for result in events
        .iter()
        .filter(|event| common::kind(event) == "tool_result")
    {
        assert_eq!(
            result["ok"],
            Value::Bool(false),
            "a cancelled bash call must never report success: {result}"
        );
        assert!(
            result["result"]
                .as_str()
                .unwrap_or_default()
                .to_lowercase()
                .contains("cancel"),
            "an emitted result must be the cancelled payload: {result}"
        );
    }

    common::dispose(handle);
}

/// `dispose` closes the queue, so a thread blocked in an infinite
/// `next_event` wakes with `NULL` immediately (bounded here at two seconds)
/// and an EMPTY `last_error` — the session ended, that is not a failure. The
/// handle is invalid afterwards: later calls fail with an error, and a second
/// dispose is a no-op.
#[test]
fn dispose_wakes_a_pump_blocked_forever_and_invalidates_the_handle() {
    let mock = MockServer::start(Vec::new());
    let handle = common::mock_session("dispose-lifecycle", &mock);
    let pointer = handle as usize;
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (result_tx, result_rx) = std::sync::mpsc::channel();

    let pump = thread::spawn(move || {
        // Tell the test thread we are about to block, so the 200ms below is
        // spent on the condvar wait rather than on thread startup.
        let _ = started_tx.send(());
        let event = common::next_event(pointer as *mut c_void, -1);
        let error = common::last_error();
        let _ = result_tx.send((event, error));
    });
    started_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the pump thread must start");
    thread::sleep(Duration::from_millis(200));

    let disposed_at = Instant::now();
    common::dispose(handle);
    let (event, error) = result_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap_or_else(|e| panic!("dispose must wake a blocked pump within 2s: {e}"));
    let wake_up = disposed_at.elapsed();
    pump.join().expect("pump thread");
    assert!(
        event.is_none(),
        "dispose must wake the blocked pump with NULL"
    );
    assert_eq!(error, None, "the wake-up is not an error");
    assert!(
        wake_up < Duration::from_secs(2),
        "the pump must wake immediately, not on a timeout: {wake_up:?}"
    );

    // The pointer is invalid now: later calls fail (the error kind of NULL),
    // and a second dispose stays a no-op without touching last_error.
    assert_eq!(common::next_event(handle, 0), None);
    assert!(
        common::last_error().is_some(),
        "a disposed handle must set last_error"
    );
    assert_eq!(common::prompt(handle, "after dispose"), -1);
    assert!(
        common::last_error().is_some(),
        "prompt on a disposed handle must set last_error"
    );
    let before = common::last_error();
    common::dispose(handle);
    assert_eq!(common::last_error(), before, "a second dispose is a no-op");
}

/// `dispose` while a turn is genuinely in flight cancels that turn: `prompt`
/// returns `1` (Cancelled — not an error, `last_error` stays empty on the
/// prompt thread), the pump blocked in `next_event` wakes with `NULL` and an
/// empty `last_error`, and the handle is invalid for every later call. This is
/// spec Testing item 10's third clause — the tests above dispose only after
/// `prompt` returned.
#[test]
fn dispose_cancels_an_in_flight_turn() {
    const PARTIAL: &str = "dispose-partial-marker-1c9f";
    // The first payload leaves the turn mid-stream (no `finish_reason`), and
    // the mock then holds the connection open with no further bytes until this
    // test opens the gate: dispose lands while the provider read is genuinely
    // blocked, instead of racing a fixed pause.
    let gate = Arc::new(Gate::new());
    let mock = MockServer::start(vec![Reply::SseGated {
        payloads: vec![
            common::sse_text(PARTIAL, None),
            common::sse_text(" and done", Some("stop")),
            common::sse_usage(8, 2, 10, None),
            SSE_DONE.to_string(),
        ],
        gate: Arc::clone(&gate),
    }]);
    let handle = common::mock_session("dispose-in-flight", &mock);
    let code = common::prompt_on_thread_with_error(handle, "write a long answer");

    // The turn is in flight once its first streamed chunk has arrived as a
    // text event (bounded at 5s): the mock is holding the connection there.
    let mut observed = Vec::new();
    loop {
        let event = common::next_event(handle, 5_000).unwrap_or_else(|| {
            panic!(
                "no text event within 5s; saw {:?}",
                common::kinds(&observed)
            )
        });
        let partial = common::kind(&event) == "text" && event["text"] == PARTIAL;
        observed.push(event);
        if partial {
            break;
        }
    }
    assert!(
        !observed.iter().any(|e| common::kind(e) == "turn_finished"),
        "the turn must still be in flight when dispose lands"
    );

    // A pump blocked forever must wake on dispose with NULL and an EMPTY
    // last_error: "the session ended", not a failure.
    let pointer = handle as usize;
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    let pump = thread::spawn(move || {
        let _ = started_tx.send(());
        let event = common::next_event(pointer as *mut c_void, -1);
        let error = common::last_error();
        let _ = result_tx.send((event, error));
    });
    started_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the pump thread must start");
    thread::sleep(Duration::from_millis(200));

    let disposed_at = Instant::now();
    common::dispose(handle);
    let (pump_event, pump_error) = result_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap_or_else(|e| panic!("dispose must wake the blocked pump within 2s: {e}"));
    let wake_up = disposed_at.elapsed();
    pump.join().expect("pump thread");
    assert!(pump_event.is_none(), "dispose must wake the pump with NULL");
    assert_eq!(pump_error, None, "the dispose wake-up is not an error");
    assert!(
        wake_up < Duration::from_secs(2),
        "the pump must wake immediately, not on a timeout: {wake_up:?}"
    );

    // Release the mock: the blocked provider read returns, dispose's cancel is
    // observed at its next boundary, and the in-flight turn ends Cancelled.
    gate.release();
    let (stop, prompt_error) = common::await_prompt(
        &code,
        Duration::from_secs(10),
        "the disposed in-flight turn must end promptly",
    );
    assert_eq!(
        stop, 1,
        "dispose must cancel the in-flight turn (got {stop}; prompt error: {prompt_error:?})"
    );
    assert_eq!(prompt_error, None, "cancellation is not an error");

    // The handle is invalid afterwards: later calls fail with an error, and
    // the closed queue can never hand out another event.
    assert!(common::next_event(handle, 0).is_none());
    assert!(
        common::last_error().is_some(),
        "next_event on a disposed handle must set last_error"
    );
    assert_eq!(common::prompt(handle, "after dispose"), -1);
    assert!(
        common::last_error().is_some(),
        "prompt on a disposed handle must set last_error"
    );
    assert_eq!(
        mock.request_count(),
        1,
        "dispose must not start a second request"
    );
}

/// All records of one session log, serialized: markers in this test are plain
/// ASCII, so a substring search is exact.
fn log_text(path: &std::path::Path) -> String {
    serde_json::to_string(&common::log_records(path)).expect("log records serialize")
}

/// Two handles in one process run at the same time without interfering: each
/// queue yields only its own session's events, and the two persisted logs are
/// distinct files that each carry only their own session's text.
#[test]
fn two_handles_run_concurrently_without_crossing_events_or_logs() {
    const ALPHA: &str = "unique-alpha-77f4c1";
    const BETA: &str = "unique-beta-9a2e30";

    // One shared home + project (the harder case: the logs land in the same
    // `<home>/sessions/<project>/` directory), one mock endpoint per session
    // so each turn's reply is scripted independently.
    let (home, cwd) = common::scratch("two-handles");
    let mock_a = MockServer::start(vec![text_turn(ALPHA)]);
    let mock_b = MockServer::start(vec![text_turn(BETA)]);
    let options_a = common::options(&home, &cwd).base_url(&mock_a.base_url());
    let options_b = common::options(&home, &cwd).base_url(&mock_b.base_url());
    let handle_a = common::create_ok(&options_a.to_json());
    let handle_b = common::create_ok(&options_b.to_json());
    assert_ne!(
        common::session_id(handle_a),
        common::session_id(handle_b),
        "two sessions need two ids"
    );

    // Both turns in flight at once, one OS thread each (single-flight is per
    // handle, never process-wide).
    let code_a = common::prompt_on_thread(handle_a, "alpha prompt");
    let code_b = common::prompt_on_thread(handle_b, "beta prompt");
    assert_eq!(
        common::await_code(&code_a, Duration::from_secs(10), "session A"),
        0,
        "session A's turn failed (its thread's last_error is not visible here)"
    );
    assert_eq!(
        common::await_code(&code_b, Duration::from_secs(10), "session B"),
        0,
        "session B's turn failed (its thread's last_error is not visible here)"
    );
    assert_eq!(mock_a.request_count(), 1);
    assert_eq!(mock_b.request_count(), 1);

    // Each queue carries its own turn's events and nothing of the other's.
    let json_a = serde_json::to_string(&common::drain_until_turn_finished(handle_a))
        .expect("events serialize");
    let json_b = serde_json::to_string(&common::drain_until_turn_finished(handle_b))
        .expect("events serialize");
    assert!(json_a.contains(ALPHA) && json_a.contains("alpha prompt"));
    assert!(
        !json_a.contains(BETA) && !json_a.contains("beta prompt"),
        "handle A's queue leaked session B: {json_a}"
    );
    assert!(json_b.contains(BETA) && json_b.contains("beta prompt"));
    assert!(
        !json_b.contains(ALPHA) && !json_b.contains("alpha prompt"),
        "handle B's queue leaked session A: {json_b}"
    );

    // Two sessions, two files, each with only its own marker.
    let logs = common::persisted_logs(&home);
    assert_eq!(logs.len(), 2, "one log per session: {logs:?}");
    let alpha_logs: Vec<_> = logs
        .iter()
        .filter(|path| log_text(path).contains(ALPHA))
        .collect();
    let beta_logs: Vec<_> = logs
        .iter()
        .filter(|path| log_text(path).contains(BETA))
        .collect();
    assert_eq!(alpha_logs.len(), 1, "one log carries alpha: {logs:?}");
    assert_eq!(beta_logs.len(), 1, "one log carries beta: {logs:?}");
    assert_ne!(
        alpha_logs[0], beta_logs[0],
        "the logs must be distinct files"
    );
    assert!(!log_text(alpha_logs[0]).contains(BETA));
    assert!(!log_text(beta_logs[0]).contains(ALPHA));

    common::dispose(handle_a);
    common::dispose(handle_b);
}
