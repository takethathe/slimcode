//! slimcode-common: frontend-agnostic application modules.
//!
//! Everything here is reusable by any frontend — the current line-based REPL,
//! a future TUI, or a web UI — without depending on the terminal binary.
//!
//! Modules:
//! - [`config`]: four-layer config resolution (frontend overrides > env >
//!   `config.toml` > defaults), producing a [`slimcode_ai::BailianConfig`].
//! - [`session`]: `SessionStore` + session metadata helpers.
//! - [`history`]: `HistoryStore` for input history persistence.
//! - [`tools`]: the seven-tool set bound to a working directory.

pub mod config;
pub mod history;
pub mod session;
pub mod tools;

#[cfg(test)]
mod testutil;
