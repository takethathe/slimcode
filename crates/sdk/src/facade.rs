//! The runtime-agnostic **async facade** over the synchronous kernel
//! (ADR-0031 D3, engine ticket 07).
//!
//! The kernel stays synchronous — this module only reshapes how a host *drives*
//! it. [`AgentSession::prompt_async`] runs one whole turn on a **dedicated OS
//! thread** (the non-tokio loop thread the ADR names) and hands back a
//! [`PromptFuture`] that resolves when the turn finishes;
//! [`AgentSession::event_stream`] subscribes to the session's bounded event bus
//! (ticket 06) and hands back the same read-only events as a
//! [`futures_core::Stream`]. Together a host consumes the engine without
//! blocking a thread and without slimcode adopting tokio.
//!
//! Operands stay synchronous traits, so slimcode's own code preset (sync
//! provider / sync tools) crosses no bridge. A host whose provider / tools are
//! async adapts them to the synchronous [`crate::Provider`] / [`crate::Tool`]
//! operands with a host-side `block_on` shim *running on that dedicated loop
//! thread* — a plain thread, not a tokio worker, so `block_on` there is
//! legitimate. The contract
//! is documented on [`AgentSession::prompt_async`] and exercised by the
//! async-to-sync bridge test (spec checklist 11).
//!
//! The `Stream` inherits the bus's ordering and delivery contract verbatim: the
//! turn's terminal events (`TurnFinished`, then an optional `Compacted`) enter
//! it only after the taken provider / tools / session state are put back
//! (ADR-0021 D4), and the sequence a host pulls matches the synchronous kernel
//! event-for-event (spec checklist 10).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures_channel::{mpsc, oneshot};
use futures_core::Stream;

use crate::{AgentSession, SdkEvent, StopReason, Unsubscribe};

/// A host-facing handle that resolves with one turn's stop reason once the
/// dedicated kernel thread finishes it.
///
/// Produced by [`AgentSession::prompt_async`]. Dropping it does not cancel the
/// turn — the kernel runs to completion regardless — matching the synchronous
/// [`AgentSession::prompt`], which likewise only ends through its own return or
/// [`AgentSession::abort`].
pub struct PromptFuture {
    rx: oneshot::Receiver<Result<StopReason, String>>,
}

impl Future for PromptFuture {
    type Output = Result<StopReason, String>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.rx).poll(cx) {
            // The kernel thread reported: hand back its turn result.
            Poll::Ready(Ok(result)) => Poll::Ready(result),
            // The kernel thread vanished (panicked) without sending: surface it
            // as a turn failure rather than an unfinished future.
            Poll::Ready(Err(_canceled)) => {
                Poll::Ready(Err("the kernel thread ended without a result".to_string()))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

/// The session's read-only events as a runtime-agnostic [`Stream`].
///
/// Produced by [`AgentSession::event_stream`]. It yields every event the event
/// bus delivers, in bus order, until the session is disposed or this stream is
/// dropped (dropping it unsubscribes, so the bus stops forwarding to it).
pub struct EventStream {
    rx: mpsc::UnboundedReceiver<SdkEvent>,
    /// Kept so the bus's own snapshot stays consistent; dropping the stream
    /// calls it to remove this stream's forwarding subscriber.
    unsubscribe: Option<Unsubscribe>,
}

impl Stream for EventStream {
    type Item = SdkEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<SdkEvent>> {
        Pin::new(&mut self.rx).poll_next(cx)
    }
}

impl Drop for EventStream {
    fn drop(&mut self) {
        // Calling the handle removes the forwarding listener from the bus, so a
        // dropped stream neither leaks its subscriber nor blocks a later
        // `flush_events` behind an abandoned channel.
        if let Some(unsubscribe) = self.unsubscribe.take() {
            unsubscribe();
        }
    }
}

impl AgentSession {
    /// Run one whole agent turn on a **dedicated kernel thread**, returning a
    /// [`PromptFuture`] for its stop reason — the non-blocking counterpart of
    /// [`AgentSession::prompt`].
    ///
    /// The synchronous kernel (the code preset over the public Engine seam, with
    /// its turn-end event-bus flush) runs unchanged; this only moves it off the
    /// caller's thread so a host can `await` it on any runtime while pulling
    /// events from [`AgentSession::event_stream`]. Single-flight is the session's
    /// own: a concurrent turn — synchronous or async — resolves with
    /// `"a turn is already running"`.
    ///
    /// # Async operand bridge (host side)
    ///
    /// Operands are synchronous traits. A host with an async provider / tools
    /// adapts each to the synchronous [`crate::Provider`] / [`crate::Tool`]
    /// operands with a `block_on` shim; because
    /// the shim's `block_on` runs on this dedicated (non-tokio) kernel thread,
    /// driving a foreign future there is sound. slimcode's code preset needs no
    /// such shim — its operands are already synchronous — which is why the
    /// kernel stays synchronous (ADR-0031 D3).
    pub fn prompt_async(self: Arc<Self>, text: impl Into<String>) -> PromptFuture {
        let text = text.into();
        let (tx, rx) = oneshot::channel();
        // A named thread so a stuck kernel is identifiable in a host's tooling;
        // `spawn` only fails on resource exhaustion, which surfaces as the
        // receiver being dropped (the future then resolves with the vanish
        // error rather than hanging).
        let spawned = std::thread::Builder::new()
            .name("slimcode-kernel".to_string())
            .spawn(move || {
                let result = self.prompt(&text);
                // A dropped receiver means the host stopped caring; the turn
                // still ran to completion so the session log stays consistent.
                let _ = tx.send(result);
            });
        if spawned.is_err() {
            // Report the failure through the same vanish path the poll maps, so
            // the future's contract stays uniform.
            drop(rx);
            // Re-create a receiver that is already closed so `poll` yields the
            // "ended without a result" error rather than a leaked `SendError`.
            let (dead_tx, dead_rx) = oneshot::channel();
            drop(dead_tx);
            return PromptFuture { rx: dead_rx };
        }
        PromptFuture { rx }
    }

    /// Subscribe to the session's read-only events as a runtime-agnostic
    /// [`Stream`], fed off the bounded event bus (ticket 06).
    ///
    /// The stream carries every subsequent turn's events in bus order — streamed
    /// `Display` items, per-message `Message` events, and each turn's terminal
    /// `TurnFinished` / optional `Compacted` — exactly as [`AgentSession::subscribe`]
    /// observes them, delivered on the bus's dispatcher thread. Drop the stream
    /// to stop receiving; the underlying subscription is removed with it.
    pub fn event_stream(&self) -> EventStream {
        let (tx, rx) = mpsc::unbounded();
        let unsubscribe = self.subscribe(move |event| {
            // An `unbounded_send` only fails once the receiver is gone, which
            // happens exactly when this stream is dropped — the subscriber is
            // then removed too, so the send never races a live consumer.
            let _ = tx.unbounded_send(event);
        });
        EventStream {
            rx,
            unsubscribe: Some(unsubscribe),
        }
    }
}
