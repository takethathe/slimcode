//! The ticket-01 ABI symbols (spec D4, D7): the ABI version, session creation
//! from options JSON, the read-only `session_id` / `title` accessors, handle
//! disposal, and the crate-wide error / string-ownership entry points.
//!
//! Every body that can fail runs under [`catch_panic`] (the exception is
//! `slimcode_abi_version`, which only loads a constant), every fallible one
//! clears the previous `last_error` first, and `NULL` / unknown handles are
//! tolerated
//! exactly as `include/slimcode.h` documents: `dispose` is a silent no-op, the
//! accessors fail with an error.

use std::ffi::{CString, c_char, c_void};
use std::ptr;

use slimcode_sdk::create_agent_session;

use crate::options::FfiOptions;
use crate::{catch_panic, error, handle, into_c_string};

/// The ABI version this build implements, returned by
/// `slimcode_abi_version`. Bump it — and document the change in
/// `include/slimcode.h` — only for a breaking change to the ABI (spec D7).
pub const ABI_VERSION: u32 = 1;

/// `uint32_t slimcode_abi_version(void)`.
///
/// # Safety
///
/// No preconditions: the function returns a constant.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slimcode_abi_version() -> u32 {
    // Deliberately no `catch_panic` guard: a constant load cannot unwind, so
    // the guard would only add a `0` fallback that a host asserting `== 1`
    // would misread as a real ABI mismatch instead of an internal bug. The
    // rule that panics never cross the ABI still holds — this body has no
    // operation that can panic.
    ABI_VERSION
}

/// `void* slimcode_session_create(const char* options_json)`.
///
/// Builds an SDK session from the options object and registers it behind an
/// opaque handle. Returns `NULL` plus a `last_error` for malformed JSON, an
/// unknown option field, or an SDK-level creation failure (for example
/// `persist: false` together with `session_id`).
///
/// # Safety
///
/// `options_json` must be `NULL` or a NUL-terminated UTF-8 C string that
/// stays valid for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slimcode_session_create(options_json: *const c_char) -> *mut c_void {
    error::clear();
    catch_panic(ptr::null_mut(), || {
        if options_json.is_null() {
            error::set("options_json must not be NULL");
            return ptr::null_mut();
        }
        let Some(json) = (unsafe { crate::str_from_c(options_json) }) else {
            error::set("options_json is not valid UTF-8");
            return ptr::null_mut();
        };
        let options = match FfiOptions::parse(json) {
            Ok(options) => options,
            Err(message) => {
                error::set(message);
                return ptr::null_mut();
            }
        };
        match create_agent_session(options.into_agent_options()) {
            Ok(session) => handle::insert(session),
            Err(message) => {
                error::set(message);
                ptr::null_mut()
            }
        }
    })
}

/// `char* slimcode_session_id(void* session)` — the session's id
/// (`slimcode-<unix>-<pid>-<n>`); the host frees the string.
///
/// # Safety
///
/// `session` must be a handle from `slimcode_session_create` that has not been
/// disposed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slimcode_session_id(session: *mut c_void) -> *mut c_char {
    error::clear();
    catch_panic(ptr::null_mut(), || {
        let Some(handle) = handle::lookup(session) else {
            error::set("invalid or disposed session handle");
            return ptr::null_mut();
        };
        into_c_string(handle.session().session_id())
    })
}

/// `char* slimcode_session_title(void* session)` — the inferred title, or the
/// **empty string** (never `NULL`) for a session without one; the host frees
/// the string.
///
/// # Safety
///
/// `session` must be a handle from `slimcode_session_create` that has not been
/// disposed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slimcode_session_title(session: *mut c_void) -> *mut c_char {
    error::clear();
    catch_panic(ptr::null_mut(), || {
        let Some(handle) = handle::lookup(session) else {
            error::set("invalid or disposed session handle");
            return ptr::null_mut();
        };
        into_c_string(handle.session().title().unwrap_or_default())
    })
}

/// `void slimcode_session_dispose(void* session)`.
///
/// Disposes the SDK session and invalidates the handle. `NULL` and unknown
/// pointers are a silent no-op — like `abort`, `dispose` never touches
/// `last_error` (see `include/slimcode.h`'s "Errors, return codes and
/// ownership" block) — and a second `dispose` of the same handle is a no-op
/// too.
///
/// # Safety
///
/// `session` must be `NULL` or a handle from `slimcode_session_create`; after
/// the call the pointer is invalid and must not be used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slimcode_session_dispose(session: *mut c_void) {
    catch_panic((), || {
        let Some(handle) = handle::take(session) else {
            return;
        };
        handle.close();
        handle.session().dispose();
    });
}

/// `const char* slimcode_last_error(void)` — the calling thread's most recent
/// error, or `NULL` when the last fallible call on this thread succeeded.
///
/// The returned pointer is **library-owned and must not be freed**; it stays
/// valid until the next call that updates it on the same thread (every
/// fallible entry point clears it on entry and sets it on failure; `abort`,
/// `dispose`, `last_error`, `free_string` and `abi_version` never touch it),
/// so copy it if it must outlive that.
///
/// # Safety
///
/// No preconditions: the function returns a thread-local pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slimcode_last_error() -> *const c_char {
    catch_panic(ptr::null(), error::get)
}

/// `void slimcode_free_string(char* s)` — release a string the library
/// returned. `NULL` is a no-op; anything else must be an untouched pointer
/// from a `slimcode_*` function (never `slimcode_last_error`'s).
///
/// # Safety
///
/// `value` must be `NULL` or a pointer the library returned to this caller
/// from a `slimcode_*` function; it must not be passed twice, and
/// `slimcode_last_error`'s pointer must never be passed at all.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slimcode_free_string(value: *mut c_char) {
    if value.is_null() {
        return;
    }
    catch_panic((), || {
        drop(unsafe { CString::from_raw(value) });
    });
}
