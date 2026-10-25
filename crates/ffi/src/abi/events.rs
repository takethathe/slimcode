//! `slimcode_session_prompt` / `slimcode_session_next_event` — ticket 02
//! (`02-event-stream-first-turn.md`).
//!
//! The event shapes live in [`crate::wire`] (the FFI-owned flat enum whose
//! mapping over `SdkEvent` / `DisplayItem` is exhaustive, so a new display
//! variant breaks the build); this file owns the two symbols and the queue
//! semantics `include/slimcode.h` documents:
//!
//! * `prompt` is the SDK's synchronous, single-flight turn: `0` Completed,
//!   `1` Cancelled (not an error), `-1` plus `last_error` for anything else.
//! * `next_event` pops the handle's queue, blocking at most `timeout_ms`
//!   (`< 0` forever). Every `NULL` return is one of two kinds: the timeout /
//!   empty-queue / the waiter-woke-because-dispose kind, which leaves
//!   `last_error` empty, and the NULL / unknown / already-disposed-handle
//!   kind, which sets it — that difference is what a host pump loop branches
//!   on.

use std::ffi::{c_char, c_void};
use std::ptr;
use std::time::Duration;

use slimcode_sdk::StopReason;

use crate::{catch_panic, error, handle};

/// `int32_t slimcode_session_prompt(void* session, const char* text)`.
///
/// Runs one whole turn on the calling thread. `0` = the model produced a final
/// answer (the turn's events are queued, ending with `turn_finished`), `1` =
/// the turn was cancelled (not an error; `last_error` stays empty), `-1` = the
/// handle is invalid / disposed, `text` is empty, whitespace-only or not a
/// UTF-8 C string, a turn is already running, or the turn failed (the reason
/// is in `last_error`; the session stays usable).
///
/// # Safety
///
/// `session` must be a handle from `slimcode_session_create` that has not been
/// disposed; `text` must be a NUL-terminated UTF-8 C string that stays valid
/// for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slimcode_session_prompt(session: *mut c_void, text: *const c_char) -> i32 {
    error::clear();
    catch_panic(-1, || {
        let Some(handle) = handle::lookup(session) else {
            error::set("invalid or disposed session handle");
            return -1;
        };
        let Some(text) = (unsafe { crate::str_from_c(text) }) else {
            error::set("prompt text must be a non-NULL UTF-8 C string");
            return -1;
        };
        match handle.session().prompt(text) {
            Ok(StopReason::Completed) => 0,
            Ok(StopReason::Cancelled) => 1,
            // Guardrail stops (ticket 08) are a bot-only concern: the code
            // preset this ABI serves leaves `max_iterations` / `timeout` unset,
            // so they never arise here. Map any such stop to the clean code.
            Ok(_) => 0,
            Err(message) => {
                error::set(message);
                -1
            }
        }
    })
}

/// `char* slimcode_session_next_event(void* session, int32_t timeout_ms)`.
///
/// Takes the next queued event as one flat JSON object (the host releases it
/// with `slimcode_free_string`), waiting at most `timeout_ms` (`< 0` waits
/// forever, `0` polls).
///
/// Returns `NULL` with `last_error` **empty** when the timeout expires, when a
/// `0` poll finds the queue empty, or when the call was already blocked and
/// the handle got disposed — "nothing yet" / "the session ended", not a
/// failure. Returns `NULL` with `last_error` **set** when the handle is NULL,
/// unknown or already disposed at call time.
///
/// # Safety
///
/// `session` must be a handle from `slimcode_session_create` that has not been
/// disposed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slimcode_session_next_event(
    session: *mut c_void,
    timeout_ms: i32,
) -> *mut c_char {
    error::clear();
    catch_panic(ptr::null_mut(), || {
        let Some(handle) = handle::lookup(session) else {
            error::set("invalid or disposed session handle");
            return ptr::null_mut();
        };
        // `< 0` waits forever; `>= 0` waits at most that many milliseconds
        // (`0` returns immediately on an empty queue).
        let timeout = (timeout_ms >= 0).then(|| Duration::from_millis(timeout_ms as u64));
        match handle.queue().pop(timeout) {
            Some(json) => crate::into_c_string(json),
            // Timeout / empty queue / the waiter woke because the handle was
            // disposed: no event, and `last_error` stays empty so the host can
            // tell "nothing yet" from "failed".
            None => ptr::null_mut(),
        }
    })
}
