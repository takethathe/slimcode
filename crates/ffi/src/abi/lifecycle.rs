//! `slimcode_session_abort` — ticket 04 (`04-lifecycle-abort-dispose.md`).
//!
//! The host's cancel path (spec D6): `abort` is callable from any thread at
//! any time, including while `prompt` is blocked in a provider or tool call,
//! because it only sets the SDK's shared cancel token and never waits on the
//! session lock. `dispose`'s half of the lifecycle — waking threads blocked
//! in `slimcode_session_next_event` — lives on
//! [`crate::handle::SessionHandle`], which ticket 02 already had mark the
//! queue closed; `tests/lifecycle.rs` pins that behavior rather than
//! re-implementing it.

use std::ffi::c_void;

use crate::{catch_panic, handle};

/// `void slimcode_session_abort(void* session)`.
///
/// Cancels the in-flight turn: the running `prompt` ends at the next runner
/// boundary with `StopReason::Cancelled` — the text streamed so far stays on
/// the transcript as display events but never enters history (core's agent
/// loop discards the half message) — and a later `prompt` starts from a
/// fresh, uncancelled token. A cancelled `bash` child of the default tool set
/// is killed promptly, because the tool set shares this same token.
///
/// This only touches the SDK's cancel token, never the session lock, so it
/// returns promptly even while a turn is running. It is a no-op for `NULL` /
/// unknown / already disposed handles and when no turn is running. Infallible
/// by design: it neither sets nor clears `last_error` (matching `dispose`'s
/// no-op path and the header's "no error" contract), unlike every fallible
/// entry point.
///
/// # Safety
///
/// `session` must be `NULL` or a handle from `slimcode_session_create` that
/// has not been disposed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slimcode_session_abort(session: *mut c_void) {
    catch_panic((), || {
        // A NULL / unknown / disposed handle has nothing to cancel. `lookup`
        // (not `take`) keeps the handle alive for the in-flight turn, which
        // may be holding its own `Arc` from `prompt`.
        let Some(handle) = handle::lookup(session) else {
            return;
        };
        handle.session().abort();
    });
}
