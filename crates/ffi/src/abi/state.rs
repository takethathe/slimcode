//! `slimcode_session_usage_json` / `slimcode_session_messages_json` — ticket
//! 03 (`03-tool-turn-and-accessors.md`).
//!
//! The two read-only state accessors, mirroring [`crate::abi::session`]'s
//! `session_id` / `title` pattern: clear `last_error`, run under
//! [`catch_panic`], look the handle up, and fail with
//! `"invalid or disposed session handle"` for a `NULL` / dead pointer.
//!
//! Both payloads are the SDK's own serde shapes (spec D3): `usage_json` is the
//! derived `TokenUsage` serialization (the same object the `usage` event
//! carries), `messages_json` is the `AgentMessage` history array — the shape
//! the session log stores per record, so the host reuses one parser.

use std::ffi::{c_char, c_void};
use std::ptr;

use crate::{catch_panic, error, handle, into_c_string};

/// `char* slimcode_session_usage_json(void* session)` — the session's
/// accumulated token usage as a JSON object in the `usage` event payload's
/// shape; the host frees the string.
///
/// # Safety
///
/// `session` must be a handle from `slimcode_session_create` that has not been
/// disposed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slimcode_session_usage_json(session: *mut c_void) -> *mut c_char {
    error::clear();
    catch_panic(ptr::null_mut(), || {
        let Some(handle) = handle::lookup(session) else {
            error::set("invalid or disposed session handle");
            return ptr::null_mut();
        };
        // `TokenUsage` serialization has no failure mode (plain scalars and an
        // optional struct); a hypothetical error still surfaces as `NULL` +
        // last_error rather than a panic.
        match serde_json::to_string(&handle.session().usage()) {
            Ok(json) => into_c_string(json),
            Err(e) => {
                error::set(format!("session usage is not serializable: {e}"));
                ptr::null_mut()
            }
        }
    })
}

/// `char* slimcode_session_messages_json(void* session)` — the session's
/// message history as a JSON array of session-log-shaped objects
/// (`{"kind":"llm",…}` / `{"kind":"compact_summary",…}`); the host frees the
/// string.
///
/// # Safety
///
/// `session` must be a handle from `slimcode_session_create` that has not been
/// disposed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slimcode_session_messages_json(session: *mut c_void) -> *mut c_char {
    error::clear();
    catch_panic(ptr::null_mut(), || {
        let Some(handle) = handle::lookup(session) else {
            error::set("invalid or disposed session handle");
            return ptr::null_mut();
        };
        match serde_json::to_string(&handle.session().messages()) {
            Ok(json) => into_c_string(json),
            Err(e) => {
                error::set(format!("session messages are not serializable: {e}"));
                ptr::null_mut()
            }
        }
    })
}
