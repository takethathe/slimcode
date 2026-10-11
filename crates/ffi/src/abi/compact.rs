//! `slimcode_session_compact_now` — ticket 06 (`06-compact-now.md`).
//!
//! The out-of-turn compaction the host drives as a `/compact` command: the
//! SDK's own [`AgentSession::compact_now`](slimcode_sdk::AgentSession) is
//! synchronous and single-flight with `prompt`, replaces the summarized span
//! with a `CompactSummary` checkpoint, and emits
//! [`SdkEvent::Compacted`](slimcode_sdk::SdkEvent::Compacted) synchronously —
//! which the one-time listener of `handle` already pushes into this handle's
//! queue, so the wire event needs nothing from this file.
//!
//! The symbol is the usual thin wrapper (spec D4): clear `last_error`, run
//! under [`catch_panic`], look the handle up, and turn the SDK result into the
//! ABI's two shapes — `{"before":N,"after":M}` as a library-owned string, or
//! `NULL` plus `last_error`. A failure (a provider error on the summarization
//! request, or `"nothing to compact"` for a history that fits the retained
//! budget) leaves the session's history untouched and the session usable.

use std::ffi::{c_char, c_void};
use std::ptr;

use crate::{catch_panic, error, handle, into_c_string};

/// `char* slimcode_session_compact_now(void* session)`.
///
/// Forces one compaction of the session's history outside a turn (the
/// `/compact` command), returning `{"before":N,"after":M}` — the estimated
/// history tokens before and after — as a library-owned string the host
/// releases with `slimcode_free_string`. The compaction also arrives in the
/// handle's event queue as `{"kind":"compacted","before":N,"after":M}`, before
/// this call returns.
///
/// Returns `NULL` with `last_error` set when the handle is NULL, unknown or
/// already disposed, when a turn (or another `compact_now`) is already running
/// on this handle, when the history fits the retained budget ("nothing to
/// compact"), or when the summarization request fails. The history is then
/// untouched and the session stays usable.
///
/// # Safety
///
/// `session` must be a handle from `slimcode_session_create` that has not been
/// disposed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slimcode_session_compact_now(session: *mut c_void) -> *mut c_char {
    error::clear();
    catch_panic(ptr::null_mut(), || {
        let Some(handle) = handle::lookup(session) else {
            error::set("invalid or disposed session handle");
            return ptr::null_mut();
        };
        match handle.session().compact_now() {
            // The header pins this payload's shape, field order included:
            // {"before":N,"after":M}. Both values are plain integers, so
            // formatting them cannot produce invalid JSON.
            Ok((before, after)) => {
                into_c_string(format!("{{\"before\":{before},\"after\":{after}}}"))
            }
            Err(message) => {
                error::set(message);
                ptr::null_mut()
            }
        }
    })
}
