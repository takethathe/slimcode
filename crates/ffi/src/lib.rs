//! slimcode-ffi: the plain C ABI over [`slimcode_sdk`] (ADR-0023, spec
//! `.scratch/ffi/spec.md`).
//!
//! A non-Rust host — Node ≥ 26 via the built-in `node:ffi`, Python via
//! ctypes/cffi — loads the `cdylib` this crate builds and drives a slimcode
//! session through opaque handles:
//!
//! * **options** are one UTF-8 JSON object ([`options::FfiOptions`], unknown
//!   fields rejected) so adding fields never breaks the ABI;
//! * **events** are flat UTF-8 JSON strings taken from a per-handle queue
//!   ([`abi::events`], ticket 02) instead of callbacks, so the C side never
//!   calls back into the host language (spec D2);
//! * **failures** are `NULL` / `-1` plus a thread-local message
//!   ([`error`], `slimcode_last_error`), and every returned string is
//!   library-owned ([`into_c_string`], `slimcode_free_string`);
//! * **panics never cross the ABI**: every symbol body that can fail runs
//!   under [`catch_panic`] and turns into an error code instead (spec D4);
//!   `slimcode_abi_version` only loads a constant, so it needs no guard.
//!
//! `include/slimcode.h` is the contract: written once, complete, and kept
//! honest by `tests/abi_contract.rs` (every implemented `slimcode_*` symbol is
//! declared there).
//!
//! # Symbol inventory
//!
//! Ticket 01 ([`abi::session`]): `slimcode_abi_version`,
//! `slimcode_session_create`, `slimcode_session_id`, `slimcode_session_title`,
//! `slimcode_session_dispose`, `slimcode_last_error`, `slimcode_free_string`.
//! Ticket 02 ([`abi::events`]): `slimcode_session_prompt`,
//! `slimcode_session_next_event`. Ticket 03 ([`abi::state`]):
//! `slimcode_session_usage_json`, `slimcode_session_messages_json`. Ticket 04
//! ([`abi::lifecycle`]): `slimcode_session_abort` (plus the `dispose` wake-up
//! in [`handle`]). Ticket 06 ([`abi::compact`]):
//! `slimcode_session_compact_now`.
//!
//! The crates 02–06 own are declared as empty placeholders on purpose: each of
//! those tickets fills exactly one file, so their parallel branches never
//! contend on `abi/mod.rs`.

pub mod abi;

mod error;
mod handle;
mod options;
mod wire;

use std::ffi::{CStr, CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};

/// Read a NUL-terminated C string pointer as `&str`; `None` for a NULL pointer
/// or invalid UTF-8. The caller guarantees the pointer is valid for the call
/// (the C ABI contract documents who owns it).
pub(crate) unsafe fn str_from_c<'a>(pointer: *const c_char) -> Option<&'a str> {
    if pointer.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(pointer) }.to_str().ok()
}

/// Move a Rust string into a library-owned C string that the host releases
/// with `slimcode_free_string`. Interior NULs cannot exist in a C string, so
/// they are escaped instead of silently truncating the payload — an ABI
/// boundary must never lose data.
pub(crate) fn into_c_string(value: impl Into<String>) -> *mut c_char {
    let value = value.into().replace('\0', "\\0");
    CString::new(value)
        .expect("interior NUL bytes were replaced")
        .into_raw()
}

/// Run one ABI symbol body, converting a Rust panic into `fallback` plus the
/// calling thread's `last_error`: unwinding into a foreign frame is undefined
/// behavior, so an internal bug must surface as an error code instead.
pub(crate) fn catch_panic<T>(fallback: T, body: impl FnOnce() -> T) -> T {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(value) => value,
        Err(payload) => {
            error::set(format!(
                "internal panic: {}",
                panic_message(payload.as_ref())
            ));
            fallback
        }
    }
}

/// The message inside a caught panic payload (`panic!("…")` / `panic!("{x}")`
/// are the expected shapes; anything else gets a placeholder).
fn panic_message(payload: &(dyn std::any::Any + Send)) -> &str {
    if let Some(message) = payload.downcast_ref::<&str>() {
        return message;
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.as_str();
    }
    "<non-string panic payload>"
}
