//! Built-in agent tools.
//!
//! Each tool is a pure engine (no I/O) exposed as a function; the tool-loop
//! binding lands with ticket 04. The `edit` engine is implemented per
//! `.scratch/slimcode-v1` ticket 03 (pi-style semantics + lightweight fuzzy).

pub mod edit;
pub mod files;
