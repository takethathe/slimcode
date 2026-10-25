//! The session's internal bounded event **message-bus** (ADR-0031 D3, engine
//! ticket 06).
//!
//! Read-only events leave a turn through a bounded channel that a dedicated
//! dispatcher thread drains, so a slow subscriber (an SSE sink, a laggy GUI) is
//! absorbed by the buffer instead of stalling the thread the Loop skeleton runs
//! on; sustained overflow applies backpressure (the publishing `send` blocks on
//! a full buffer). Because there is one FIFO channel with a single consumer,
//! delivery order is exactly publish order — the per-event `Display` /
//! `Message` / terminal stream the synchronous seam produced is unchanged.
//!
//! A [`BusMsg::Flush`] marker closes the seam back to synchronous from the
//! host's point of view: `prompt` / `compact_now` publish their turn's events,
//! then a flush, and block until the dispatcher has *delivered* everything
//! queued before that marker. So when the call returns its whole ordered event
//! stream has gone out — the FFI queue is populated and the equivalence
//! baseline is complete even though delivery happened on the dispatcher thread.
//! It also keeps the ordering contract intact: the terminal events are published
//! only after the taken provider / tools / session state is put back, so a
//! subscriber that reads the session inside them sees the turn's own result.

use std::panic::{self, AssertUnwindSafe};
use std::sync::mpsc::Receiver;
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};

use crate::{Listener, SdkEvent};

/// Buffer capacity: enough to absorb a normal turn's event burst (display
/// deltas, per-message, terminal) so the loop never blocks on subscriber
/// latency; a genuinely stuck subscriber beyond this applies backpressure.
pub(crate) const EVENT_BUS_CAPACITY: usize = 256;

/// A message on the session's event bus.
pub(crate) enum BusMsg {
    /// Deliver one read-only event to every current subscriber.
    Event(SdkEvent),
    /// Barrier: the dispatcher has delivered everything queued before this
    /// marker; ack it so the publishing turn can finish synchronously.
    Flush(SyncSender<()>),
    /// Stop the dispatcher (session disposed or dropped).
    Shutdown,
}

/// Drain the bus on the dispatcher thread. Each event is delivered to a
/// snapshot of the subscriber list taken under the lock but *invoked outside
/// it* (so a listener may call its own `Unsubscribe`, and a concurrent
/// `dispose` that clears the list, cannot deadlock on the mutex), each isolated
/// by `catch_unwind` so one panicking subscriber neither aborts the turn nor
/// starves the others. A [`BusMsg::Flush`] is acked in order (everything before
/// it has already been delivered); [`BusMsg::Shutdown`] ends the loop.
pub(crate) fn run(rx: Receiver<BusMsg>, listeners: Arc<Mutex<Vec<Listener>>>) {
    while let Ok(msg) = rx.recv() {
        match msg {
            BusMsg::Event(event) => {
                let snapshot: Vec<Arc<dyn Fn(SdkEvent) + Send + Sync>> = listeners
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .iter()
                    .map(|(_, listener)| Arc::clone(listener))
                    .collect();
                for listener in snapshot {
                    let _ = panic::catch_unwind(AssertUnwindSafe(|| listener(event.clone())));
                }
            }
            BusMsg::Flush(ack) => {
                let _ = ack.send(());
            }
            BusMsg::Shutdown => break,
        }
    }
}
