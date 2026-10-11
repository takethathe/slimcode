//! The `extern "C"` surface, split one file per ticket (spec D1) so the FFI
//! slice's parallel branches never contend on `mod.rs`.
//!
//! * [`session`] — ticket 01: `abi_version` / `create` / `session_id` /
//!   `title` / `dispose`, plus the crate-wide `last_error` / `free_string`
//!   ownership entry points;
//! * [`events`] — ticket 02: `prompt` / `next_event` and the flat event wire
//!   contract;
//! * [`state`] — ticket 03: `usage_json` / `messages_json`;
//! * [`lifecycle`] — ticket 04: `abort` and the dispose wake-up;
//! * [`compact`] — ticket 06: `compact_now`.
//!
//! Every symbol is `#[unsafe(no_mangle)] pub unsafe extern "C" fn` and must be
//! declared in `include/slimcode.h`; `tests/abi_contract.rs` scans both sides
//! to keep that true.

pub mod compact;
pub mod events;
pub mod lifecycle;
pub mod session;
pub mod state;
