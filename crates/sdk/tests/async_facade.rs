//! Async-facade tests (`.scratch/engine` ticket 07). The SDK now exposes a
//! runtime-agnostic async surface over the synchronous kernel: a turn runs on a
//! **dedicated OS thread** behind a `PromptFuture`, and the session's read-only
//! events are handed to the host as a `futures_core::Stream` fed off the ticket-
//! 06 event bus. slimcode pulls in no tokio — the tests drive the facade with
//! `futures::executor::block_on` and pull the stream with `StreamExt::next`.
//!
//! These tests pin the properties the facade owes:
//!
//! - `block_on`-driving `prompt_async` while pulling `event_stream` runs a whole
//!   turn, and the event sequence matches the synchronous kernel event-for-event
//!   (spec checklist 10);
//! - the terminal event still observes the **post-turn** session state when it
//!   reaches the host through the facade (the ordering contract carries over);
//! - an async provider adapted to the synchronous `Provider` operand with a
//!   host-side `block_on` shim completes a turn on the dedicated kernel thread
//!   (spec checklist 11) — slimcode's own code preset crosses no such bridge.
//!
//! Order-equality against the synchronous seam is what `equivalence_baseline`
//! already pins for the bus; here it is re-checked across the async boundary to
//! prove the facade preserves it.

mod common;
use common::*;

use std::sync::{Arc, Mutex};

use futures::{StreamExt, executor::block_on, future};
use slimcode_app::{CancelToken, Delta, Message, Provider, ProviderConfig, TokenUsage, ToolSpec};
use slimcode_sdk::{SdkEvent, StopReason};

/// Render an event sequence to comparable debug strings, so order and content
/// can be asserted without the public `SdkEvent` carrying `PartialEq`.
fn shape(events: &[SdkEvent]) -> Vec<String> {
    events.iter().map(|event| format!("{event:?}")).collect()
}

/// A one-turn script: a tool call, then a plain text answer that ends the turn.
fn two_call_script() -> Vec<Vec<Delta>> {
    vec![
        vec![tc_start(0, "c1", "probe"), tc_args(0, "{}"), done_tools()],
        vec![text("here you go"), done_stop()],
    ]
}

/// `block_on`-driving `prompt_async` while pulling `event_stream` runs a whole
/// turn and yields the same ordered event stream the synchronous `prompt` does.
#[test]
fn block_on_drives_the_prompt_future_and_event_stream() {
    let (_guard, _home, cwd) = test_env("facade_order");

    // Synchronous reference: the same script through the plain `prompt`, events
    // captured by a subscriber.
    let sync_session = make_session(&cwd, Box::new(FakeProvider::new(two_call_script())), vec![]);
    let recorder = Recorder::new();
    let _unsubscribe = sync_session.subscribe(recorder.listener());
    let sync_stop = sync_session.prompt("go").expect("sync turn succeeds");
    let sync_events = recorder.events();

    // Async facade: a `PromptFuture` driven on the dedicated kernel thread, the
    // event `Stream` pulled on the host thread until the terminal event.
    let session = Arc::new(make_session(
        &cwd,
        Box::new(FakeProvider::new(two_call_script())),
        vec![],
    ));
    let (async_stop, async_events) = block_on(async {
        let mut stream = session.event_stream();
        let collect = async {
            let mut events = Vec::new();
            while let Some(event) = stream.next().await {
                let terminal = matches!(event, SdkEvent::TurnFinished { .. });
                events.push(event);
                if terminal {
                    break;
                }
            }
            events
        };
        // Drive the turn future and the stream together; the kernel runs on its
        // own thread while this thread awaits both.
        let (stop, events) = future::join(session.clone().prompt_async("go"), collect).await;
        (stop, events)
    });

    let async_stop = async_stop.expect("async turn succeeds");
    assert_eq!(async_stop, sync_stop);
    assert_eq!(
        shape(&async_events),
        shape(&sync_events),
        "the async facade must preserve the synchronous kernel's event order"
    );
    // The stream is non-trivial: deltas, per-message events and a terminal one.
    assert!(
        async_events.len() >= 4,
        "expected a streamed turn, got {async_events:?}"
    );
}

/// The terminal event reaches the host only after the taken state is put back:
/// reading the session inside it sees the whole turn, not the pre-turn snapshot.
#[test]
fn terminal_event_observes_the_post_turn_state_through_the_facade() {
    let (_guard, _home, cwd) = test_env("facade_putback");

    let session = Arc::new(make_session(
        &cwd,
        Box::new(FakeProvider::new(vec![vec![text("answer"), done_stop()]])),
        vec![],
    ));
    let weak = Arc::downgrade(&session);
    let observed: Arc<Mutex<Option<usize>>> = Arc::new(Mutex::new(None));
    let cell = Arc::clone(&observed);
    let _unsubscribe = session.subscribe(move |event| {
        if matches!(event, SdkEvent::TurnFinished { .. })
            && let Some(session) = weak.upgrade()
        {
            *cell.lock().unwrap_or_else(|e| e.into_inner()) = Some(session.messages().len());
        }
    });

    block_on(session.clone().prompt_async("go")).expect("turn succeeds");

    // The turn entered a user prompt + one assistant reply = two messages, and
    // the terminal event observed that post-turn length.
    assert_eq!(
        *observed.lock().unwrap_or_else(|e| e.into_inner()),
        Some(2),
        "the terminal event must observe the put-back session state"
    );
}

/// A provider whose generation is asynchronous, adapted to the synchronous
/// `Provider` operand with a host-side `block_on` shim. The shim's `block_on`
/// runs on the dedicated (non-tokio) kernel thread, so driving the foreign
/// future there is sound — the documented bridge contract for a future async
/// host (spec checklist 11).
struct BridgedProvider {
    script: Vec<Vec<Delta>>,
    calls: usize,
}

impl BridgedProvider {
    /// The "async" half: streams deltas across await points. The returned future
    /// borrows the delta sink, so it is not `Send` — which is fine, `block_on`
    /// drives it to completion on the current (kernel) thread.
    async fn generate(
        &mut self,
        on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
    ) -> Result<Option<TokenUsage>, String> {
        let batch = self.script.get(self.calls).cloned().unwrap_or_default();
        self.calls += 1;
        for delta in batch {
            // A genuine await, standing in for an async network chunk.
            future::ready(()).await;
            on_delta(delta)?;
        }
        Ok(Some(TokenUsage::default()))
    }
}

impl Provider for BridgedProvider {
    fn chat(
        &mut self,
        _messages: &[Message],
        _tools: &[ToolSpec],
        _config: &ProviderConfig,
        _cancel: &CancelToken,
        on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
    ) -> Result<Option<TokenUsage>, String> {
        // The bridge: `block_on` the async body on this synchronous operand
        // call, which the facade runs on the dedicated kernel thread.
        block_on(self.generate(on_delta))
    }
}

#[test]
fn an_async_provider_bridges_to_the_sync_operand_via_block_on() {
    let (_guard, _home, cwd) = test_env("facade_bridge");

    let provider = BridgedProvider {
        script: vec![vec![text("bridged answer"), done_stop()]],
        calls: 0,
    };
    let session = Arc::new(make_session(&cwd, Box::new(provider), vec![]));

    let stop = block_on(session.clone().prompt_async("go")).expect("bridged turn succeeds");
    assert_eq!(stop, StopReason::Completed);
    assert!(
        session
            .messages()
            .iter()
            .any(|m| m.text_content().contains("bridged answer")),
        "the bridged provider's reply must reach history"
    );
}
