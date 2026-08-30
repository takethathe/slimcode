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
//! - [`skills`]: `Skill` model + `SkillStore` for user/project-scoped skill
//!   discovery, installation, and `/`-trigger prediction.
//! - [`context`]: `ContextBuilder` assembling one turn's message list from a
//!   system prompt, skills, history, and a user prompt / skill trigger.
//! - [`tools`]: the seven-tool set bound to a working directory.

pub mod config;
pub mod context;
pub mod history;
pub mod session;
pub mod skills;
pub mod tools;

#[doc(hidden)]
pub mod testutil;
