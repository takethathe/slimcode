//! Turn-end auto-compaction through the public `AgentSession` API only
//! (spec Testing Decisions list item 6, User Story 17).
//!
//! A scripted `FakeProvider` distinguishes the compaction summary request (a
//! `chat` with NO tools — research notes §2: `compact_messages` sends
//! `[system, user]` with no tools) from ordinary turn calls (which carry the
//! session's tools), so one provider drives the turn, the summary, the failure
//! and the cancel paths. Assertions cover the `Compacted` event (emitted AFTER
//! `TurnFinished`, spec §5 event order), the `CompactSummary` head in the live
//! history, the log replaying with the checkpoint as the head (ADR-0020 D2), a
//! silent compaction failure keeping the full history, a cancelled or failed
//! turn never compacting, and the summary request's token cost landing in
//! `session.usage`.

mod common;
use common::*;

use std::sync::{Arc, Mutex};

use slimcode_app::{
    AgentMessage, CancelToken, Delta, FinishReason, Message, Provider, ProviderConfig, StopReason,
    TokenUsage, ToolSpec,
};

use slimcode_sdk::SdkEvent;

/// The compaction trigger in tokens: `should_compact` fires once the estimate
/// reaches `200_000 - 16_000` (the estimator is chars/4 per message, so the
/// corresponding char count is 4x this).
const COMPACT_TRIGGER_TOKENS: usize = 200_000 - 16_000;

/// A scripted provider (the cli/tui + app/runner precedent) that treats a
/// `chat` with NO tools as the compaction summary request (research notes §2:
/// `compact_messages` sends `[system, user]` with no tools) and any other call
/// as an ordinary turn. Turn calls consume `script` batches in order; the
/// summary call streams `summary_reply` (or errors when `fails_summary`), and
/// a turn call can be made to error with `fails_on_turn` (1-based).
/// `total_usage()` accumulates `per_call` across every call, mirroring the
/// CLI's `run_compaction` usage-delta trick (research notes §4).
struct FakeProvider {
    script: Vec<Vec<Delta>>,
    turn_calls: usize,
    summary_reply: String,
    fail_summary: bool,
    fail_turn: Option<usize>,
    cancel_on_turn: Option<usize>,
    per_call: TokenUsage,
    total: TokenUsage,
}

impl FakeProvider {
    fn new(script: Vec<Vec<Delta>>) -> Self {
        Self {
            script,
            turn_calls: 0,
            summary_reply: String::new(),
            fail_summary: false,
            fail_turn: None,
            cancel_on_turn: None,
            per_call: TokenUsage::default(),
            total: TokenUsage::default(),
        }
    }
    /// The text the compaction summary call streams (the summary reply must be
    /// non-trivial — `compact_messages` errors on an empty summary).
    fn with_summary(mut self, reply: &str) -> Self {
        self.summary_reply = reply.to_string();
        self
    }
    /// Make the compaction summary call error (turn calls still succeed).
    fn fails_summary(mut self) -> Self {
        self.fail_summary = true;
        self
    }
    /// Make the 1-based turn call `call` error (summary calls still succeed).
    fn fails_on_turn(mut self, call: usize) -> Self {
        self.fail_turn = Some(call);
        self
    }
    /// Cancel the shared token during the 1-based turn call `call`.
    fn cancels_on_turn(mut self, call: usize) -> Self {
        self.cancel_on_turn = Some(call);
        self
    }
    fn with_per_call(mut self, usage: TokenUsage) -> Self {
        self.per_call = usage;
        self
    }
}

impl Provider for FakeProvider {
    fn chat(
        &mut self,
        _messages: &[Message],
        tools: &[ToolSpec],
        _config: &ProviderConfig,
        cancel: &CancelToken,
        on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
    ) -> Result<Option<TokenUsage>, String> {
        // The compaction summary request: no tools (research notes §2). The
        // turn calls carry the session's tools, so the slice discriminates.
        if tools.is_empty() {
            if self.fail_summary {
                return Err("summary provider exploded".to_string());
            }
            on_delta(Delta::Text(self.summary_reply.clone()))?;
            on_delta(Delta::Done(FinishReason::Stop))?;
            self.total = self.total.saturating_add(&self.per_call);
            return Ok(Some(self.per_call));
        }
        if self.fail_turn == Some(self.turn_calls + 1) {
            return Err("turn provider exploded".to_string());
        }
        if self.cancel_on_turn == Some(self.turn_calls + 1) {
            cancel.cancel();
        }
        let batch = self
            .script
            .get(self.turn_calls)
            .cloned()
            .unwrap_or_default();
        self.turn_calls += 1;
        for delta in batch {
            on_delta(delta)?;
        }
        self.total = self.total.saturating_add(&self.per_call);
        Ok(Some(self.per_call))
    }

    fn total_usage(&self) -> TokenUsage {
        self.total
    }
}

/// A completed turn that replies with `chars` characters of text.
fn big_reply(chars: usize) -> Vec<Delta> {
    vec![text(&"y".repeat(chars)), done_stop()]
}

// --- behavior -------------------------------------------------------------

#[test]
fn completed_turn_crossing_the_threshold_compacts_and_replays_the_checkpoint() {
    let (_guard, home, cwd) = test_env("compact");
    // Turn 1 is large but stays under the threshold: ~730k chars ≈ 182.5k
    // tokens < 184k. Turn 2 (8k chars ≈ 2k tokens) pushes the total over, so
    // compaction fires at the END of turn 2 — the summary call is the third
    // provider `chat`.
    let provider = FakeProvider::new(vec![
        big_reply(730_000),
        vec![text(&"z".repeat(8_000)), done_stop()],
    ])
    .with_summary("## Goal\nfinish the sdk");
    let session = make_session(&cwd, Box::new(provider), vec![probe_tool()]);
    let recorder = Recorder::new();
    let _unsubscribe = session.subscribe(recorder.listener());

    session.prompt("first").expect("first turn");
    let stop = session.prompt("second").expect("second turn");
    assert_eq!(stop, StopReason::Completed);

    let events = recorder.events();
    let compacted: Vec<(usize, usize)> = events
        .iter()
        .filter_map(|e| match e {
            SdkEvent::Compacted { before, after } => Some((*before, *after)),
            _ => None,
        })
        .collect();
    // Exactly one compaction, on the second turn.
    assert_eq!(compacted.len(), 1, "{events:?}");
    let (before, after) = compacted[0];
    // The compaction shrank the history, from past the threshold to a tail.
    assert!(before >= COMPACT_TRIGGER_TOKENS, "before: {before}");
    assert!(after < before, "after: {after}, before: {before}");
    // The Compacted event arrived AFTER the last TurnFinished (spec §5:
    // ... → TurnFinished → Compacted).
    let last_finished = events
        .iter()
        .rposition(|e| matches!(e, SdkEvent::TurnFinished { .. }))
        .expect("a TurnFinished");
    let compacted_at = events
        .iter()
        .position(|e| matches!(e, SdkEvent::Compacted { .. }))
        .expect("a Compacted");
    assert!(last_finished < compacted_at, "{events:?}");

    // The live session starts with the checkpoint whose summary is the reply.
    let live = session.messages();
    let checkpoint = live.first().expect("a checkpoint head");
    assert!(checkpoint.is_compact_summary());
    let summary = match checkpoint {
        AgentMessage::CompactSummary { summary, .. } => summary,
        other => panic!("expected a checkpoint, got {other:?}"),
    };
    assert!(summary.contains("finish the sdk"), "{summary}");

    // The log replays to the live session's effective history exactly: the
    // checkpoint head plus the Kept tail it recorded, and none of the span the
    // summary replaced. A resume no longer drops the kept tail — the live and
    // reloaded sessions agree (ADR-0025).
    let store = store_for(&home, &cwd);
    let outcome = store.load(session.session_id()).expect("load");
    assert_eq!(outcome.session.messages, live);
}

#[test]
fn failed_compaction_is_silent_and_keeps_the_full_history() {
    let (_guard, home, cwd) = test_env("fail");
    // Turn 1 crosses the threshold (~740k chars ≈ 185k tokens), so a
    // compaction is attempted at its end; the summary call errors (the
    // provider fails every no-tools chat). Turn 2 still sees the full history.
    let provider = FakeProvider::new(vec![
        big_reply(740_000),
        vec![text("final reply"), done_stop()],
    ])
    .with_summary("never used")
    .fails_summary();
    let session = make_session(&cwd, Box::new(provider), vec![probe_tool()]);
    let recorder = Recorder::new();
    let _unsubscribe = session.subscribe(recorder.listener());

    let first = session.prompt("first").expect("first turn");
    assert_eq!(first, StopReason::Completed);
    let second = session.prompt("second").expect("second turn");
    assert_eq!(second, StopReason::Completed);

    // No Compacted event at all — the failure is silent (spec User Story 28:
    // keep the original history and this turn's outcome, losing nothing
    // silently).
    let events = recorder.events();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, SdkEvent::Compacted { .. })),
        "{events:?}"
    );
    // The history and both turns' results survive untouched: no checkpoint
    // replaced anything, and the second prompt saw the full history again.
    let live = session.messages();
    assert_eq!(live.len(), 4);
    assert!(
        !live.iter().any(AgentMessage::is_compact_summary),
        "{live:?}"
    );
    assert_eq!(live[0].text_content(), "first");
    assert_eq!(live[1].text_content().len(), 740_000);
    assert_eq!(live[2].text_content(), "second");
    assert_eq!(live[3].text_content(), "final reply");

    // The persisted log still replays the full (uncompacted) history.
    let store = store_for(&home, &cwd);
    let outcome = store.load(session.session_id()).expect("load");
    assert_eq!(outcome.session.messages, live);
}

#[test]
fn a_cancelled_turn_never_compacts_even_with_huge_history() {
    let (_guard, home, cwd) = test_env("cancel");
    // Turn 1 is large but under the threshold (~730k chars ≈ 182.5k tokens).
    // Turn 2's prompt is itself huge (16.1k chars ≈ 4k tokens), pushing the
    // accumulated history over the threshold — but the turn is cancelled, and
    // compaction only runs inside the Completed arm (spec D3: never a
    // cancelled or errored turn).
    let provider = FakeProvider::new(vec![big_reply(730_000), vec![text("partial"), done_stop()]])
        .with_summary("unused")
        .cancels_on_turn(2);
    let session = make_session(&cwd, Box::new(provider), vec![probe_tool()]);
    let recorder = Recorder::new();
    let _unsubscribe = session.subscribe(recorder.listener());

    session.prompt("first").expect("first turn");
    let big_prompt = "x".repeat(16_100);
    let stop = session.prompt(&big_prompt).expect("second turn");
    assert_eq!(stop, StopReason::Cancelled);

    let events = recorder.events();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, SdkEvent::Compacted { .. })),
        "{events:?}"
    );
    // The cancelled turn adopted no assistant reply; the log is closed with
    // the Aborted message, and the huge history is intact (never compacted).
    let live = session.messages();
    assert_eq!(live.len(), 4); // user1, assistant1(730k), user2(16.1k), closing
    assert_eq!(live[3].text_content(), "The turn was cancelled.");
    assert!(!live.iter().any(AgentMessage::is_compact_summary));
    // The retained history genuinely crosses the threshold — yet no
    // compaction ran, because the turn was not Completed.
    let estimate = slimcode_app::compaction::estimate_total_tokens(&live);
    assert!(
        estimate >= COMPACT_TRIGGER_TOKENS,
        "history must cross the threshold: {estimate}"
    );

    let store = store_for(&home, &cwd);
    let outcome = store.load(session.session_id()).expect("load");
    assert_eq!(outcome.session.messages, live);
}

#[test]
fn a_failed_turn_never_compacts_even_with_huge_history() {
    let (_guard, home, cwd) = test_env("failturn");
    // Turn 1 is large but under the threshold (~730k chars ≈ 182.5k tokens).
    // Turn 2's prompt is itself huge (16.1k chars ≈ 4k tokens), pushing the
    // accumulated history over the threshold — but the turn fails, and
    // compaction only runs inside the Completed arm (spec D3: never a
    // cancelled or errored turn). The summary reply is non-trivial, so a
    // wrongly-triggered compaction would succeed and expose itself.
    let provider = FakeProvider::new(vec![
        big_reply(730_000),
        vec![text("never sent"), done_stop()],
    ])
    .with_summary("## Goal\nunused")
    .fails_on_turn(2);
    let session = make_session(&cwd, Box::new(provider), vec![probe_tool()]);
    let recorder = Recorder::new();
    let _unsubscribe = session.subscribe(recorder.listener());

    session.prompt("first").expect("first turn");
    let big_prompt = "x".repeat(16_100);
    let err = session.prompt(&big_prompt).unwrap_err();
    assert!(err.contains("turn provider exploded"), "err: {err}");

    let events = recorder.events();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, SdkEvent::Compacted { .. })),
        "{events:?}"
    );
    // The failed turn emits no end signal either: only the first turn's
    // `TurnFinished` is in the stream, and no `Compacted` follows it (spec
    // §5: `TurnFinished` belongs to a Completed / Cancelled turn).
    let finished = events
        .iter()
        .filter(|e| matches!(e, SdkEvent::TurnFinished { .. }))
        .count();
    assert_eq!(finished, 1, "{events:?}");
    // The failed turn produced no reply; the log is closed with the Error
    // message and the huge history is intact (never compacted).
    let live = session.messages();
    assert_eq!(live.len(), 4); // user1, assistant1(730k), user2(16.1k), closing
    assert_eq!(live[0].text_content(), "first");
    assert_eq!(live[1].text_content().len(), 730_000);
    assert_eq!(live[2].text_content(), big_prompt.as_str());
    assert!(
        live[3].text_content().contains("error"),
        "{}",
        live[3].text_content()
    );
    assert!(!live.iter().any(AgentMessage::is_compact_summary));
    // The retained history genuinely crosses the threshold — yet no
    // compaction ran, because the turn did not complete.
    let estimate = slimcode_app::compaction::estimate_total_tokens(&live);
    assert!(
        estimate >= COMPACT_TRIGGER_TOKENS,
        "history must cross the threshold: {estimate}"
    );

    let store = store_for(&home, &cwd);
    let outcome = store.load(session.session_id()).expect("load");
    assert_eq!(outcome.session.messages, live);
}

#[test]
fn compaction_cost_lands_in_session_usage() {
    let (_guard, _home, cwd) = test_env("usage");
    let per_call = usage(10, 5); // 15 tokens per chat call
    // Turn 1 crosses the threshold, so a compaction runs at its end; both the
    // turn's assistant message and the summary request cost `per_call`.
    let provider = FakeProvider::new(vec![big_reply(740_000)])
        .with_summary("## Goal\nfinish the sdk")
        .with_per_call(per_call);
    let session = make_session(&cwd, Box::new(provider), vec![probe_tool()]);

    session.prompt("first").expect("turn");

    // The turn's assistant message carries `per_call` into `session.usage` via
    // the message sink; the compaction request adds its own `per_call` via the
    // provider-total delta (ADR-0018 D4) — exactly two calls' worth in total.
    assert_eq!(session.usage().total_tokens, per_call.total_tokens * 2);
}

/// What a listener saw at the `Compacted` event: the history and the usage it
/// read through the public accessors.
type SeenState = (Vec<AgentMessage>, TokenUsage);

/// The turn-end events are delivered after the taken session state is put
/// back: a listener that reads the session inside `Compacted` sees the
/// post-compaction history and the usage that includes the summary request,
/// not the pre-turn snapshot the state was taken from. The CLI's footer
/// refresh (`route_event` reseeds the live view from the session) depends on
/// it — the stale read renders usage 0 / context 0% after a turn-end
/// auto-compaction.
#[test]
fn the_compacted_listener_sees_the_restored_session() {
    let (_guard, _home, cwd) = test_env("compacted-state");
    let per_call = usage(10, 5); // 15 tokens per chat call
    let provider = FakeProvider::new(vec![big_reply(740_000)])
        .with_summary("## Goal\nfinish the sdk")
        .with_per_call(per_call);
    let session = Arc::new(make_session(&cwd, Box::new(provider), vec![probe_tool()]));
    // What a listener saw at the `Compacted` event, read through the public
    // accessors exactly like the CLI's `route_event` does.
    let seen: Arc<Mutex<Vec<SeenState>>> = Arc::new(Mutex::new(Vec::new()));
    let session_handle = Arc::downgrade(&session);
    let sink = Arc::clone(&seen);
    let _unsubscribe = session.subscribe(move |event| {
        if matches!(event, SdkEvent::Compacted { .. }) {
            let session = session_handle
                .upgrade()
                .expect("the session outlives the turn");
            sink.lock()
                .expect("seen lock")
                .push((session.messages(), session.usage()));
        }
    });

    let stop = session.prompt("first").expect("turn");
    assert_eq!(stop, StopReason::Completed);

    let seen = seen.lock().expect("seen lock");
    let (messages, usage_at_event) = seen.first().expect("the Compacted listener ran");
    // The post-compaction history is already visible: the checkpoint head
    // replaced the span it summarized.
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].is_compact_summary(), "{messages:?}");
    // The usage already includes the turn's request and the summary request.
    assert_eq!(usage_at_event.total_tokens, per_call.total_tokens * 2);
}

// --- compact_now (on-demand compaction) -----------------------------------

/// compact_now on a small history forces a compaction the auto path would
/// skip: ~100k chars ≈ 25k tokens sits far below the 184k auto threshold, so
/// the completed turn never auto-compacts, yet above the 16k keep budget so
/// `compact_messages` has something to summarize. The history is replaced by
/// a CompactSummary checkpoint, the `Compacted` event fires synchronously
/// once with the returned tokens, and the log replays with the checkpoint as
/// its head (ADR-0020 D2).
#[test]
fn compact_now_on_a_small_history_forces_compaction() {
    let (_guard, home, cwd) = test_env("compact-now");
    let provider =
        FakeProvider::new(vec![big_reply(100_000)]).with_summary("## Goal\nfinish the sdk");
    let session = make_session(&cwd, Box::new(provider), vec![probe_tool()]);
    let recorder = Recorder::new();
    let _unsubscribe = session.subscribe(recorder.listener());

    session.prompt("first").expect("first turn");
    // The small history is far below the auto threshold: the completed turn
    // never auto-compacts, so no Compacted event precedes compact_now.
    let before_events = recorder.events();
    assert!(
        !before_events
            .iter()
            .any(|e| matches!(e, SdkEvent::Compacted { .. })),
        "{before_events:?}"
    );

    let (before, after) = session.compact_now().expect("compact_now");

    // The returned change is real: the history shrank to a checkpoint.
    assert!(after < before, "after: {after}, before: {before}");
    let live = session.messages();
    let checkpoint = live.first().expect("a checkpoint head");
    assert!(checkpoint.is_compact_summary());
    let summary = match checkpoint {
        AgentMessage::CompactSummary { summary, .. } => summary,
        other => panic!("expected a checkpoint, got {other:?}"),
    };
    assert!(summary.contains("finish the sdk"), "{summary}");

    // The Compacted event fires synchronously with the returned tokens.
    let events = recorder.events();
    let compacted: Vec<(usize, usize)> = events
        .iter()
        .filter_map(|e| match e {
            SdkEvent::Compacted { before, after } => Some((*before, *after)),
            _ => None,
        })
        .collect();
    assert_eq!(compacted, vec![(before, after)], "{events:?}");

    // The log replays with the checkpoint as the head (ADR-0020 D2).
    let store = store_for(&home, &cwd);
    let outcome = store.load(session.session_id()).expect("load");
    assert_eq!(outcome.session.messages, live);
}

/// compact_now on an empty history has nothing to summarize: it errors with a
/// clear message, emits no event, and leaves the history untouched.
#[test]
fn compact_now_with_an_empty_history_errors() {
    let (_guard, _home, cwd) = test_env("compact-now-empty");
    let provider = FakeProvider::new(vec![]).with_summary("unused");
    let session = make_session(&cwd, Box::new(provider), vec![probe_tool()]);
    let recorder = Recorder::new();
    let _unsubscribe = session.subscribe(recorder.listener());

    let err = session.compact_now().unwrap_err();
    assert!(err.contains("nothing to compact"), "err: {err}");
    assert!(
        !recorder
            .events()
            .iter()
            .any(|e| matches!(e, SdkEvent::Compacted { .. })),
        "no event on a failed compact_now"
    );
    assert!(session.messages().is_empty(), "history untouched");
}

/// compact_now on a tiny history that fits the retained budget errors too
/// (there is no span old enough to fold into a summary).
#[test]
fn compact_now_on_a_tiny_history_errors() {
    let (_guard, _home, cwd) = test_env("compact-now-tiny");
    let provider = FakeProvider::new(vec![vec![text("hello"), done_stop()]]).with_summary("unused");
    let session = make_session(&cwd, Box::new(provider), vec![probe_tool()]);

    session.prompt("first").expect("first turn");

    let err = session.compact_now().unwrap_err();
    assert!(err.contains("nothing to compact"), "err: {err}");
    assert_eq!(session.messages().len(), 2, "history untouched");
}

/// Single-flight: compact_now while a turn is running is rejected with "a
/// turn is already running"; releasing the turn lets both complete.
#[test]
fn compact_now_during_a_turn_is_rejected_single_flight() {
    let (_guard, _home, cwd) = test_env("compact-now-singleflight");
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

    let err = session.compact_now().unwrap_err();
    assert!(err.contains("a turn is already running"), "err: {err}");

    *release.lock().expect("release lock") = true;
    let stop = handle.join().expect("prompt thread").expect("first prompt");
    assert_eq!(stop, StopReason::Completed);
}

/// A failed summary request surfaces the provider error detail, leaves the
/// history untouched, and emits no event.
#[test]
fn compact_now_failure_reports_the_provider_error_and_keeps_the_history() {
    let (_guard, _home, cwd) = test_env("compact-now-fail");
    let provider = FakeProvider::new(vec![big_reply(100_000)])
        .with_summary("never used")
        .fails_summary();
    let session = make_session(&cwd, Box::new(provider), vec![probe_tool()]);
    let recorder = Recorder::new();
    let _unsubscribe = session.subscribe(recorder.listener());

    session.prompt("first").expect("first turn");
    let before = session.messages();

    let err = session.compact_now().unwrap_err();
    assert!(err.contains("summary provider exploded"), "err: {err}");

    // The failure leaves the history untouched and emits no event.
    assert_eq!(session.messages(), before, "history must not change");
    assert!(
        !recorder
            .events()
            .iter()
            .any(|e| matches!(e, SdkEvent::Compacted { .. })),
        "no Compacted event on a failed compact_now"
    );
}

/// The summary request's token cost folds into `session.usage` (ADR-0018 D4)
/// exactly like the auto path: the turn's assistant message carries one
/// `per_call`, the compaction request adds its own via the provider-total
/// delta.
#[test]
fn compact_now_cost_lands_in_session_usage() {
    let (_guard, _home, cwd) = test_env("compact-now-usage");
    let per_call = usage(10, 5); // 15 tokens per chat call
    let provider = FakeProvider::new(vec![big_reply(100_000)])
        .with_summary("## Goal\nfinish the sdk")
        .with_per_call(per_call);
    let session = make_session(&cwd, Box::new(provider), vec![probe_tool()]);

    session.prompt("first").expect("turn");
    // The small history never auto-compacted: only the turn's assistant
    // message carried `per_call` in.
    assert_eq!(session.usage().total_tokens, per_call.total_tokens);

    session.compact_now().expect("compact_now");
    // Exactly two calls' worth in total.
    assert_eq!(session.usage().total_tokens, per_call.total_tokens * 2);
}

/// A disposed session rejects compact_now like it rejects prompt.
#[test]
fn compact_now_after_dispose_errors() {
    let (_guard, _home, cwd) = test_env("compact-now-dispose");
    let provider = FakeProvider::new(vec![big_reply(100_000)]).with_summary("unused");
    let session = make_session(&cwd, Box::new(provider), vec![probe_tool()]);
    session.dispose();

    let err = session.compact_now().unwrap_err();
    assert!(err.contains("session is disposed"), "err: {err}");
}
