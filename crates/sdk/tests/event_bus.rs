//! Event-bus tests (`.scratch/engine` ticket 06). The SDK now publishes every
//! read-only event onto an internal bounded message-bus drained on a dispatcher
//! thread, instead of calling subscribers synchronously on the turn's thread.
//! These tests pin the properties that migration owes:
//!
//! - a slow / blocking subscriber does **not** stall the thread the Loop
//!   skeleton runs on (the turn's provider calls still complete while the
//!   subscriber is parked);
//! - the terminal events still observe the **post-turn** session state (the
//!   ordering contract: publish only after the taken state is put back);
//! - a panicking subscriber is isolated (catch_unwind) — the turn completes and
//!   the other subscribers still receive the whole ordered stream.
//!
//! Delivery-order equality itself is pinned by `equivalence_baseline` (spec
//! item 13), which these tests deliberately do not duplicate.

mod common;
use common::*;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use slimcode_app::{CancelToken, Delta, Message, Provider, ProviderConfig, TokenUsage, ToolSpec};
use slimcode_sdk::{SdkEvent, StopReason};

/// A scripted provider that counts how many `chat` calls it has finished, so a
/// test can watch the loop progress independently of event delivery.
struct CountingProvider {
    script: Vec<Vec<Delta>>,
    calls: usize,
    completed: Arc<AtomicUsize>,
}

impl Provider for CountingProvider {
    fn chat(
        &mut self,
        _messages: &[Message],
        _tools: &[ToolSpec],
        _config: &ProviderConfig,
        _cancel: &CancelToken,
        on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
    ) -> Result<Option<TokenUsage>, String> {
        let batch = self.script.get(self.calls).cloned().unwrap_or_default();
        self.calls += 1;
        for delta in batch {
            on_delta(delta)?;
        }
        self.completed.fetch_add(1, Ordering::SeqCst);
        Ok(Some(TokenUsage::default()))
    }
}

/// The loop keeps running while a subscriber is parked: a provider that makes
/// two `chat` calls finishes BOTH — and the turn reaches its terminal event —
/// even though the only subscriber is blocked on the very first delivered
/// event. Under the old synchronous seam the first event's callback would have
/// run on the loop thread and blocked everything behind it.
#[test]
fn a_blocking_subscriber_does_not_stall_the_loop_thread() {
    let (_guard, _home, cwd) = test_env("bus_stall");
    let completed = Arc::new(AtomicUsize::new(0));
    let provider = CountingProvider {
        script: vec![
            vec![
                tc_start(0, "call_1", "probe"),
                tc_args(0, "{}"),
                done_tools(),
            ],
            vec![text("done"), done_stop()],
        ],
        calls: 0,
        completed: Arc::clone(&completed),
    };
    let session = Arc::new(make_session(&cwd, Box::new(provider), vec![probe_tool()]));

    let events: Arc<Mutex<Vec<SdkEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let entered = Arc::new(AtomicBool::new(false));
    let block_once = Arc::new(AtomicBool::new(false));
    let release = Arc::new((Mutex::new(false), Condvar::new()));

    let (ev, entered_cb, block_once, release_cb) = (
        Arc::clone(&events),
        Arc::clone(&entered),
        Arc::clone(&block_once),
        Arc::clone(&release),
    );
    let _unsubscribe = session.subscribe(move |event| {
        ev.lock().unwrap().push(event);
        // Park the dispatcher on the first event until the test releases it.
        if !block_once.swap(true, Ordering::SeqCst) {
            entered_cb.store(true, Ordering::SeqCst);
            let (lock, cv) = &*release_cb;
            let mut held = lock.lock().unwrap();
            while !*held {
                held = cv.wait(held).unwrap();
            }
        }
    });

    // Run the turn on a worker thread so the main thread can observe the loop
    // progressing past the parked subscriber.
    let worker_session = Arc::clone(&session);
    let worker = std::thread::spawn(move || worker_session.prompt("go"));

    // Subscriber entered its park.
    let deadline = Instant::now() + Duration::from_secs(5);
    while !entered.load(Ordering::SeqCst) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(entered.load(Ordering::SeqCst), "subscriber never ran");

    // The loop finished BOTH provider calls while the subscriber stayed blocked.
    while completed.load(Ordering::SeqCst) < 2 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        completed.load(Ordering::SeqCst),
        2,
        "the loop must not be stalled by a blocked subscriber"
    );
    // …and only the single first event had been delivered (dispatcher still stuck).
    assert_eq!(
        events.lock().unwrap().len(),
        1,
        "the dispatcher is parked on the first event while the loop ran ahead"
    );

    // Release the subscriber; the flush barrier then drains the rest.
    {
        let (lock, cv) = &*release;
        *lock.lock().unwrap() = true;
        cv.notify_all();
    }
    let stop = worker.join().expect("prompt thread").expect("prompt");
    assert_eq!(stop, StopReason::Completed);
    // Whole ordered stream landed: prompt + terminal, TurnFinished last.
    let all = events.lock().unwrap();
    assert!(all.len() > 1, "remaining events delivered after release");
    assert!(
        matches!(all.last().unwrap(), SdkEvent::TurnFinished { .. }),
        "TurnFinished closes the stream"
    );
}

/// The ordering contract survives the bus: a subscriber reading the session
/// inside the terminal `TurnFinished` sees the turn's own result, never the
/// pre-turn snapshot — because the terminal events are published only after the
/// taken state is put back.
#[test]
fn terminal_events_observe_the_post_turn_session_state() {
    let (_guard, _home, cwd) = test_env("bus_order");
    let provider = FakeProvider::new(vec![vec![text("hi"), done_stop()]]);
    let session = Arc::new(make_session(&cwd, Box::new(provider), vec![]));

    let seen_len = Arc::new(Mutex::new(Vec::<usize>::new()));
    // A `Weak` avoids an Arc cycle (the subscriber list lives inside the
    // session); upgrade succeeds because the session is alive when the event
    // is delivered.
    let weak = Arc::downgrade(&session);
    let sl = Arc::clone(&seen_len);
    let _unsubscribe = session.subscribe(move |event| {
        if let SdkEvent::TurnFinished { .. } = event
            && let Some(s) = weak.upgrade()
        {
            sl.lock().unwrap().push(s.messages().len());
        }
    });

    session.prompt("hello").expect("prompt");

    let lens = seen_len.lock().unwrap().clone();
    // A fresh session had zero messages before the turn; the terminal event
    // must observe the post-turn history (user prompt + assistant) = 2, proving
    // it read the put-back state rather than the pre-turn snapshot.
    assert_eq!(
        lens,
        vec![2],
        "TurnFinished must observe the put-back session, not the pre-turn snapshot"
    );
}

/// A panicking subscriber is isolated by the dispatcher's `catch_unwind`: the
/// turn is not aborted, the session stays usable, and the other subscriber
/// still receives the entire ordered stream.
#[test]
fn a_panicking_subscriber_is_isolated_and_others_still_receive() {
    let (_guard, _home, cwd) = test_env("bus_panic");
    let provider = FakeProvider::new(vec![vec![text("answer"), done_stop()]]);
    let session = make_session(&cwd, Box::new(provider), vec![]);

    let good = Recorder::new();
    let _good_un = session.subscribe(good.listener());
    // The panicking subscriber runs first (subscribed second is delivered after
    // `good` per subscription order; ordering between them is irrelevant — what
    // matters is that its panic does not stop delivery to the other).
    let panicked = Arc::new(AtomicBool::new(false));
    let pflag = Arc::clone(&panicked);
    let _panic_un = session.subscribe(move |_event| {
        // Panic once, to keep stderr quiet, but that single panic must not
        // abort the turn or starve the healthy subscriber.
        if !pflag.swap(true, Ordering::SeqCst) {
            panic!("subscriber panic on purpose");
        }
    });

    let stop = session
        .prompt("hello")
        .expect("a panicking subscriber must not fail the turn");
    assert_eq!(stop, StopReason::Completed);
    assert!(
        panicked.load(Ordering::SeqCst),
        "the panicking subscriber ran"
    );

    // The healthy subscriber received the full stream, terminal event included.
    let events = good.events();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, SdkEvent::TurnFinished { .. })),
        "the healthy subscriber must still receive TurnFinished"
    );
    assert!(
        events.iter().any(|e| matches!(e, SdkEvent::Display(_))),
        "the healthy subscriber must still receive the streamed display"
    );
    // The session stayed usable: its state is intact after the isolated panic.
    assert_eq!(
        session.messages().len(),
        2,
        "session history survived the panic"
    );
}
