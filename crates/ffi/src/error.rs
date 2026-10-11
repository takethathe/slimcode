//! The thread-local `slimcode_last_error` state (spec D4).
//!
//! Every fallible ABI entry point calls [`clear`] before it does anything, so
//! a stale message can never be mistaken for this call's failure; a failure
//! then calls [`set`]. [`get`] hands the calling thread's message to C as a
//! borrowed pointer that stays valid until the next `clear` / `set` **on that
//! same thread** — it is never freed by the host.

use std::cell::RefCell;
use std::ffi::{CString, c_char};
use std::ptr;

thread_local! {
    /// The calling thread's most recent error, or `None`.
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

/// Drop the calling thread's stored error (the first step of every fallible
/// entry point).
pub(crate) fn clear() {
    LAST_ERROR.with(|slot| *slot.borrow_mut() = None);
}

/// Store `message` as the calling thread's last error. Interior NULs cannot
/// exist in a C string, so they are escaped rather than panicking on a
/// pathological error message.
pub(crate) fn set(message: impl Into<String>) {
    let message = message.into().replace('\0', "\\0");
    let message = CString::new(message).expect("interior NUL bytes were replaced");
    LAST_ERROR.with(|slot| *slot.borrow_mut() = Some(message));
}

/// The calling thread's last error as a borrowed C string pointer (`NULL` when
/// there is none). Valid until the next `clear` / `set` on this thread.
pub(crate) fn get() -> *const c_char {
    LAST_ERROR.with(|slot| match slot.borrow().as_ref() {
        Some(message) => message.as_ptr(),
        None => ptr::null(),
    })
}
