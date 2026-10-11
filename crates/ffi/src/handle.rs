//! The opaque session handle and its process-global registry (spec D4).
//!
//! A handle is the data pointer of an `Arc<SessionHandle>` stored in a
//! process-global `Mutex<HashMap<usize, Arc<SessionHandle>>>`, keyed by that
//! pointer and handed to C as `void*`. Lookup **clones the `Arc` out of the
//! registry** before the caller uses it, so a concurrent `dispose` — which
//! removes the entry and drops the registry's own reference — can never free
//! memory a call is still using. The pointer is therefore invalid for *later*
//! calls and safe for in-flight ones.
//!
//! Ticket 02 owns the per-handle event queue (mutex + condvar) on
//! [`SessionHandle`] and drains it from `slimcode_session_next_event`; ticket
//! 04's `dispose` closes the queue ([`SessionHandle::close`]) so blocked pump
//! threads wake up immediately. The `closed` flag below is the piece ticket 01
//! needs (`dispose` invalidates the handle, and an in-flight lookup must not
//! resurrect it).

use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use slimcode_sdk::AgentSession;

use crate::{error, wire};

/// One live session behind an opaque pointer.
pub struct SessionHandle {
    session: AgentSession,
    /// Set once `dispose` ran: the handle is dead even for a call that cloned
    /// the `Arc` just before the registry entry was removed.
    closed: AtomicBool,
    /// The session's event queue. `Arc` because the one-time listener holds a
    /// clone (it must outlive this struct's borrow inside the `'static`
    /// closure) and never the handle itself, so there is no reference cycle.
    queue: Arc<EventQueue>,
}

impl SessionHandle {
    fn new(session: AgentSession) -> Self {
        Self {
            session,
            closed: AtomicBool::new(false),
            queue: Arc::new(EventQueue::new()),
        }
    }

    /// The SDK session this handle drives.
    pub fn session(&self) -> &AgentSession {
        &self.session
    }

    /// This handle's event queue (`slimcode_session_next_event` blocks on it).
    pub fn queue(&self) -> &EventQueue {
        &self.queue
    }

    /// Whether `dispose` already closed this handle.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Mark the handle closed (idempotent) and close its queue: every thread
    /// blocked in `next_event` wakes with `NULL` immediately instead of
    /// waiting out its timeout (spec D6).
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.queue.close();
    }
}

/// The per-handle event queue: an unbounded FIFO of encoded wire JSON plus the
/// condvar `slimcode_session_next_event` blocks on.
///
/// Unbounded by design (spec D2): events are never dropped or merged while the
/// session is live — the host drains the queue while a turn runs. [`close`]
/// is dispose's half of the contract: set the flag, drop the unconsumed events,
/// wake every waiter with `None`.
///
/// [`close`]: EventQueue::close
pub struct EventQueue {
    state: Mutex<QueueState>,
    ready: Condvar,
}

/// The queue's mutable half; one lock protects both fields, so "pop the last
/// event / observe the close" is a single atomic decision.
struct QueueState {
    events: VecDeque<String>,
    closed: bool,
}

impl EventQueue {
    fn new() -> Self {
        Self {
            state: Mutex::new(QueueState {
                events: VecDeque::new(),
                closed: false,
            }),
            ready: Condvar::new(),
        }
    }

    /// Push one encoded event. A push after [`close`](Self::close) is dropped:
    /// the handle is gone, and nothing may resurrect its queue.
    fn push(&self, event: String) {
        let mut state = self.lock();
        if state.closed {
            return;
        }
        state.events.push_back(event);
        drop(state);
        // `notify_all`: next_event has no single-consumer requirement, and the
        // waiters that lose the race for this event simply wait again.
        self.ready.notify_all();
    }

    /// Pop the next event, waiting at most `timeout` (`None` waits forever).
    ///
    /// `None` means there is no event to hand out: the timeout expired, a
    /// `Some(Duration::ZERO)` poll found the queue empty, or the queue was
    /// closed (dispose) — never an error by itself, which is exactly what the
    /// header promises for `slimcode_session_next_event`.
    pub fn pop(&self, timeout: Option<Duration>) -> Option<String> {
        let deadline = timeout.map(|duration| Instant::now() + duration);
        let mut state = self.lock();
        loop {
            if let Some(event) = state.events.pop_front() {
                return Some(event);
            }
            if state.closed {
                return None;
            }
            match deadline {
                // Infinite wait (timeout_ms < 0).
                None => state = self.wait(state),
                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline {
                        return None;
                    }
                    let (guard, _) = self
                        .ready
                        .wait_timeout(state, deadline - now)
                        .unwrap_or_else(|poison| poison.into_inner());
                    state = guard;
                }
            }
        }
    }

    /// Close the queue: drop the unconsumed events and wake every waiter with
    /// `None` (dispose semantics, spec D6). Idempotent.
    fn close(&self) {
        let mut state = self.lock();
        state.closed = true;
        state.events.clear();
        drop(state);
        self.ready.notify_all();
    }

    /// Wait on the condvar, recovering from poisoning: a panic in one session
    /// must not make every other handle unusable (the queue itself is always
    /// consistent).
    fn wait<'a>(&self, state: MutexGuard<'a, QueueState>) -> MutexGuard<'a, QueueState> {
        self.ready
            .wait(state)
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn lock(&self) -> MutexGuard<'_, QueueState> {
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }
}

/// The registry of every live handle, keyed by its `Arc` data pointer.
fn registry() -> &'static Mutex<HashMap<usize, Arc<SessionHandle>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<usize, Arc<SessionHandle>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The registry key of `handle`: its `Arc` data pointer, stable for as long as
/// the registry holds a reference to it.
fn key_of(handle: &Arc<SessionHandle>) -> usize {
    Arc::as_ptr(handle) as usize
}

/// Register `session` and return its opaque pointer (never NULL).
pub fn insert(session: AgentSession) -> *mut c_void {
    let handle = Arc::new(SessionHandle::new(session));
    let key = key_of(&handle);
    // The one-time listener (spec D2): every `SdkEvent` is encoded and pushed
    // to this handle's queue, in subscription order, synchronously on the
    // turn's thread. The closure holds the queue (not the handle), and
    // dropping the returned `Unsubscribe` keeps the listener subscribed, so it
    // lives exactly as long as the session.
    let queue = Arc::clone(&handle.queue);
    let _unsubscribe = handle
        .session()
        .subscribe(move |event| match wire::encode(&event) {
            Ok(json) => queue.push(json),
            // Unreachable for today's `WireEvent` types (flat scalars and the
            // SDK's own serde shapes, no fallible custom impls), but an encoding
            // failure must never drop an event silently: record it as the calling
            // thread's `last_error`. The listener runs synchronously on the turn's
            // thread inside `prompt` (or `compact_now`), so the message survives
            // for the host to read when the call returns. Deliberately no panic
            // here — that would unwind into the SDK's dispatch thread.
            Err(e) => error::set(format!("event was not encodable and got dropped: {e}")),
        });
    lock().insert(key, Arc::clone(&handle));
    key as *mut c_void
}

/// Clone the live handle for `pointer`; `None` for NULL, unknown or already
/// disposed pointers.
pub fn lookup(pointer: *mut c_void) -> Option<Arc<SessionHandle>> {
    if pointer.is_null() {
        return None;
    }
    let handle = lock().get(&(pointer as usize)).cloned()?;
    (!handle.is_closed()).then_some(handle)
}

/// Remove `pointer` from the registry so the caller can dispose it; `None` for
/// NULL or unknown pointers (a second `dispose` is a no-op).
pub fn take(pointer: *mut c_void) -> Option<Arc<SessionHandle>> {
    if pointer.is_null() {
        return None;
    }
    lock().remove(&(pointer as usize))
}

/// The registry lock, recovering from poisoning: a panic in one session must
/// not make every other handle unusable (the map itself is always consistent).
fn lock() -> std::sync::MutexGuard<'static, HashMap<usize, Arc<SessionHandle>>> {
    registry()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}
