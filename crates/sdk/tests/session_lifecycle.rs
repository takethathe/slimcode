//! The session lifecycle — cross-thread `abort`, `dispose`, single-flight
//! enforcement, and listener robustness — driven through the public
//! `AgentSession` API only (spec Testing Decisions items 9-11, 15).
//!
//! The shared FakeProvider/Delta/blocking-provider scaffolding lives in
//! `common`; the extra `CancelAwareBlockingProvider` holds a turn open inside
//! `chat` so tests can drive `abort` / `dispose` / a second `prompt` from
//! another thread while the first turn is provably in flight.

mod common;
use common::*;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use slimcode_app::render::DisplayItem;
use slimcode_app::{
    CancelToken, Delta, Message, Provider, ProviderConfig, StopReason, TokenUsage, ToolSpec,
};

use slimcode_sdk::{SdkEvent, Unsubscribe};

/// A provider that streams one text fragment, then parks inside `chat` until
/// the cancel token is set (mirroring the Bailian provider's interruptible
/// body read), then finishes the request. Used to hold a turn open while the
/// test aborts it from another thread.
struct CancelAwareBlockingProvider {
    started: Arc<Mutex<bool>>,
}

impl Provider for CancelAwareBlockingProvider {
    fn chat(
        &mut self,
        _messages: &[Message],
        _tools: &[ToolSpec],
        _config: &ProviderConfig,
        cancel: &CancelToken,
        on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
    ) -> Result<Option<TokenUsage>, String> {
        on_delta(text("partial reply"))?;
        *self.started.lock().expect("started lock") = true;
        while !cancel.is_cancelled() {
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(None)
    }
}

// --- lifecycle ------------------------------------------------------------

/// Cross-thread abort: a turn running on a worker thread (its provider parked
/// on the interruptible read) ends `Cancelled` promptly once `abort()` is
/// called from the main thread; the partial text streamed before the cancel
/// was emitted as `Display` events but never adopted into history.
#[test]
fn cross_thread_abort_ends_the_turn_cancelled() {
    let (_guard, _home, cwd) = test_env("abort");
    let started = Arc::new(Mutex::new(false));
    let provider = CancelAwareBlockingProvider {
        started: started.clone(),
    };
    let session = Arc::new(make_session(&cwd, Box::new(provider), Vec::new()));
    let recorder = Recorder::new();
    let _unsubscribe = session.subscribe(recorder.listener());

    let session_thread = Arc::clone(&session);
    let handle = std::thread::spawn(move || session_thread.prompt("first"));
    wait_until(&started);
    // The partial text was streamed (and delivered) before the cancel lands.
    std::thread::sleep(Duration::from_millis(50));

    let began = Instant::now();
    session.abort();
    let stop = handle.join().expect("prompt thread").expect("prompt");
    let elapsed = began.elapsed();

    assert_eq!(stop, StopReason::Cancelled);
    assert!(elapsed < Duration::from_secs(5), "cancel took {elapsed:?}");

    // Partial text streamed before the cancel was emitted as Display events.
    let texts: Vec<String> = recorder
        .displays()
        .into_iter()
        .filter_map(|d| match d {
            DisplayItem::Text(t) => Some(t),
            _ => None,
        })
        .collect();
    assert!(
        texts.iter().any(|t| t.contains("partial")),
        "partial text must be emitted, got: {texts:?}"
    );
    // ... but the cancelled request's half text is NOT adopted into history.
    assert!(
        !session
            .messages()
            .iter()
            .any(|m| m.text_content().contains("partial")),
        "partial text must not enter history"
    );
    // The authoritative end signal of the cancelled turn.
    assert!(matches!(
        recorder.events().last(),
        Some(SdkEvent::TurnFinished {
            stop_reason: StopReason::Cancelled
        })
    ));
}

/// abort() during a turn must not wait on the session lock: the turn runs
/// with the lock free (take/put), so `abort` touches only the shared cancel
/// token and returns while the provider is still parked. If it waited on the
/// lock, this test would hang — the provider is only released after `abort`
/// returns.
#[test]
fn abort_never_waits_on_the_session_lock() {
    let (_guard, _home, cwd) = test_env("abort-nolock");
    let started = Arc::new(Mutex::new(false));
    let release = Arc::new(Mutex::new(false));
    let provider = BlockingProvider {
        started: started.clone(),
        release: release.clone(),
    };
    let session = Arc::new(make_session(&cwd, Box::new(provider), Vec::new()));

    let session_thread = Arc::clone(&session);
    let handle = std::thread::spawn(move || session_thread.prompt("first"));
    wait_until(&started);

    let began = Instant::now();
    session.abort();
    let elapsed = began.elapsed();
    assert!(
        !*release.lock().expect("release lock"),
        "abort returned while the turn was still running"
    );
    assert!(elapsed < Duration::from_secs(1), "abort took {elapsed:?}");

    // Releasing the provider lets the turn finish; the runner notices the
    // token at the next boundary and ends the turn cancelled.
    *release.lock().expect("release lock") = true;
    let stop = handle.join().expect("prompt thread").expect("prompt");
    assert_eq!(stop, StopReason::Cancelled);
}

/// dispose() rejects further prompts and silences subscribers: a listener
/// subscribed before dispose receives no events at all afterwards.
#[test]
fn dispose_rejects_prompts_and_silences_subscribers() {
    let (_guard, _home, cwd) = test_env("dispose");
    let provider = FakeProvider::new(vec![vec![text("hi there"), done_stop()]]);
    let session = make_session(&cwd, Box::new(provider), Vec::new());
    let recorder = Recorder::new();
    let _unsubscribe = session.subscribe(recorder.listener());

    session.dispose();

    let err = session.prompt("after dispose").unwrap_err();
    assert!(err.contains("session is disposed"), "err: {err}");
    assert!(
        recorder.events().is_empty(),
        "no events may reach a listener after dispose"
    );
}

/// Single-flight: a second prompt while one turn is running is rejected with
/// "a turn is already running"; releasing the first turn lets it complete.
#[test]
fn single_flight_rejects_a_second_prompt_while_one_runs() {
    let (_guard, _home, cwd) = test_env("singleflight");
    let started = Arc::new(Mutex::new(false));
    let release = Arc::new(Mutex::new(false));
    let provider = BlockingProvider {
        started: started.clone(),
        release: release.clone(),
    };
    let session = Arc::new(make_session(&cwd, Box::new(provider), Vec::new()));

    let session_thread = Arc::clone(&session);
    let handle = std::thread::spawn(move || session_thread.prompt("first"));
    wait_until(&started);

    let err = session.prompt("second").unwrap_err();
    assert!(err.contains("a turn is already running"), "err: {err}");

    *release.lock().expect("release lock") = true;
    let stop = handle.join().expect("prompt thread").expect("first prompt");
    assert_eq!(stop, StopReason::Completed);
}

/// abort only affects the in-flight turn: a fresh prompt after a cancelled
/// turn starts uncancelled (the token is reset at take) and runs normally.
#[test]
fn a_fresh_prompt_after_a_cancelled_turn_runs_normally() {
    let (_guard, home, cwd) = test_env("fresh-after-cancel");
    // The first provider call cancels the token itself (as if the user hit
    // Esc mid-request); the second runs to completion.
    let provider = FakeProvider::new(vec![
        vec![text("partial one"), done_stop()],
        vec![text("reply two"), done_stop()],
    ])
    .cancels_on(1);
    let session = make_session(&cwd, Box::new(provider), Vec::new());

    assert_eq!(
        session.prompt("first").expect("first prompt"),
        StopReason::Cancelled
    );
    // The cancelled turn produced nothing, so it must not have created a
    // session log just to hold a closing message (ADR-0009 D5).
    assert_eq!(
        count_jsonl(&home.join("sessions")),
        0,
        "a cancelled turn that produced nothing must leave no log"
    );
    assert_eq!(
        session.prompt("second").expect("second prompt"),
        StopReason::Completed
    );
    // The completed turn persisted its log under the project-key directory.
    assert_eq!(
        count_jsonl(&home.join("sessions")),
        1,
        "a completed turn must persist its log"
    );

    // user1, user2, assistant2 — the cancelled request's half text does not
    // enter history, and the turn produced nothing, so no closing message and
    // no log exist (ADR-0009 D5, mirroring the CLI's `close_turn` guard).
    let messages = session.messages();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].text_content(), "first");
    assert_eq!(messages[1].text_content(), "second");
    assert_eq!(messages[2].text_content(), "reply two");
}

/// Unsubscribing removes exactly that listener: events already delivered are
/// kept, and the next turn's events never reach it (checklist item 15).
#[test]
fn unsubscribe_stops_further_delivery() {
    let (_guard, _home, cwd) = test_env("unsubscribe");
    let provider = FakeProvider::new(vec![
        vec![text("reply one"), done_stop()],
        vec![text("reply two"), done_stop()],
    ]);
    let session = make_session(&cwd, Box::new(provider), Vec::new());
    let recorder = Recorder::new();
    let unsubscribe = session.subscribe(recorder.listener());

    assert_eq!(
        session.prompt("first").expect("first prompt"),
        StopReason::Completed
    );
    let delivered = recorder.events();
    assert!(
        !delivered.is_empty(),
        "the subscribed listener must receive the first turn's events"
    );

    unsubscribe();

    // The second turn still runs normally (and is persisted), but the
    // unsubscribed listener receives nothing further.
    assert_eq!(
        session.prompt("second").expect("second prompt"),
        StopReason::Completed
    );
    assert_eq!(
        recorder.events().len(),
        delivered.len(),
        "no event may reach an unsubscribed listener: {:?}",
        recorder.events()
    );
}

/// dispose() while a turn is in flight cancels it: the turn ends `Cancelled`
/// at the next boundary and every subsequent prompt is rejected.
#[test]
fn dispose_cancels_an_in_flight_turn() {
    let (_guard, _home, cwd) = test_env("dispose-inflight");
    let started = Arc::new(Mutex::new(false));
    let provider = CancelAwareBlockingProvider {
        started: started.clone(),
    };
    let session = Arc::new(make_session(&cwd, Box::new(provider), Vec::new()));

    let session_thread = Arc::clone(&session);
    let handle = std::thread::spawn(move || session_thread.prompt("first"));
    wait_until(&started);

    // Dispose while the turn is parked in the provider: the shared token is
    // cancelled so the turn ends Cancelled, and the session is marked disposed
    // for all subsequent prompts.
    session.dispose();
    let stop = handle.join().expect("prompt thread").expect("prompt");
    assert_eq!(stop, StopReason::Cancelled);

    let err = session.prompt("after dispose").unwrap_err();
    assert!(err.contains("session is disposed"), "err: {err}");
}

/// dispose() drops the subscribed listeners mid-turn: events already streamed
/// before the dispose are kept, but the cancelled in-flight turn's remaining
/// events — in particular its final `TurnFinished` — never reach the listener.
#[test]
fn dispose_mid_turn_drops_the_subscribed_listener() {
    let (_guard, _home, cwd) = test_env("dispose-silence");
    let started = Arc::new(Mutex::new(false));
    let provider = CancelAwareBlockingProvider {
        started: started.clone(),
    };
    let session = Arc::new(make_session(&cwd, Box::new(provider), Vec::new()));
    let recorder = Recorder::new();
    let _unsubscribe = session.subscribe(recorder.listener());

    let session_thread = Arc::clone(&session);
    let handle = std::thread::spawn(move || session_thread.prompt("first"));
    wait_until(&started);
    // The partial text streamed before the dispose was delivered.
    let before = recorder.events();
    assert!(!before.is_empty(), "partial text must have been delivered");

    session.dispose();
    let stop = handle.join().expect("prompt thread").expect("prompt");
    assert_eq!(stop, StopReason::Cancelled);
    assert_eq!(
        recorder.events().len(),
        before.len(),
        "no event may reach a listener after dispose: {:?}",
        recorder.events()
    );
}

// --- listener robustness (review findings) ---------------------------------

/// A listener may call its own `Unsubscribe` from inside a callback: the
/// current event has already been delivered, later events stop. This must
/// not deadlock (the std mutex is not reentrant; `emit` invokes listeners
/// outside the lock).
#[test]
fn unsubscribing_from_inside_a_callback_does_not_deadlock() {
    let (_guard, _home, cwd) = test_env("reentrant-unsubscribe");
    let provider = FakeProvider::new(vec![
        vec![text("reply one"), done_stop()],
        vec![text("reply two"), done_stop()],
    ]);
    let session = make_session(&cwd, Box::new(provider), Vec::new());
    let deliveries = Arc::new(Mutex::new(0usize));

    // The listener unsubscribes itself on its very first event, from inside
    // the callback. `emit` invokes listeners outside the lock, so this must
    // not deadlock; the current event is already being delivered and later
    // ones stop.
    let slot = Arc::new(Mutex::new(None::<Unsubscribe>));
    let slot_2 = Arc::clone(&slot);
    let deliveries_2 = Arc::clone(&deliveries);
    let unsubscribe = session.subscribe(move |_event| {
        *deliveries_2.lock().expect("deliveries lock") += 1;
        if let Some(unsubscribe) = slot_2.lock().expect("slot lock").take() {
            unsubscribe();
        }
    });
    *slot.lock().expect("slot lock") = Some(unsubscribe);

    // The turn completes without deadlocking; exactly the first event (the
    // user prompt's `Message`) reached the listener before it removed itself.
    assert_eq!(
        session.prompt("first").expect("prompt"),
        StopReason::Completed
    );
    assert_eq!(
        *deliveries.lock().expect("deliveries lock"),
        1,
        "the listener must receive exactly the first event"
    );
    // The session keeps working; the unsubscribed listener gets nothing more.
    assert_eq!(
        session.prompt("second").expect("prompt"),
        StopReason::Completed
    );
    assert_eq!(*deliveries.lock().expect("deliveries lock"), 1);
}

/// A panicking listener is isolated: it is skipped, the other listeners and
/// the turn continue, and the session stays usable afterwards.
#[test]
fn a_panicking_listener_is_isolated_and_the_session_survives() {
    let (_guard, _home, cwd) = test_env("panicking-listener");
    let provider = FakeProvider::new(vec![
        vec![text("reply one"), done_stop()],
        vec![text("reply two"), done_stop()],
    ]);
    let session = make_session(&cwd, Box::new(provider), Vec::new());
    let good = Recorder::new();
    let _u1 = session.subscribe(good.listener());
    let _u2 = session.subscribe(|_event| panic!("listener exploded"));

    // The turn completes normally despite the panicking listener; the good
    // listener received the whole stream (prompt Message + displays +
    // TurnFinished).
    let stop = session.prompt("first").expect("prompt");
    assert_eq!(stop, StopReason::Completed);
    assert!(
        !good.events().is_empty(),
        "good listener must receive events"
    );
    assert!(matches!(
        good.events().last(),
        Some(SdkEvent::TurnFinished {
            stop_reason: StopReason::Completed
        })
    ));

    // The session is not bricked by the panic: the next turn still runs and
    // the good listener still receives events.
    let stop = session.prompt("second").expect("prompt");
    assert_eq!(stop, StopReason::Completed);
    let finished = good
        .events()
        .iter()
        .filter(|e| matches!(e, SdkEvent::TurnFinished { .. }))
        .count();
    assert_eq!(finished, 2);
}
