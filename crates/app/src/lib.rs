//! slimcode-app: frontend-agnostic application modules.
//!
//! Everything here is reusable by any frontend — the one-shot CLI, the TUI,
//! or a web UI — without depending on the terminal binary.
//!
//! Modules:
//! - [`config`]: four-layer config resolution (frontend overrides > env >
//!   `config.toml` > defaults), producing a [`slimcode_ai::ProviderConfig`].
//! - [`session`]: `SessionStore` + session metadata helpers.
//! - [`history`]: `HistoryStore` for input history persistence.
//! - [`skills`]: `Skill` model + `SkillStore` for user/project-scoped skill
//!   discovery, installation, and `/`-trigger prediction.
//! - [`context`]: `ContextBuilder` assembling one turn's message list from a
//!   system prompt, context files, skills, history, and a user prompt / skill
//!   trigger.
//! - [`context_files`]: `AGENTS.md` discovery (global home file + project
//!   cwd/git-root files) and its `## Project context` system-prompt rendering.
//! - [`tools`]: the seven-tool set bound to a working directory.
//! - [`render`]: the frontend-agnostic renderer seam — `DisplayItem`,
//!   `map_event`, and the `Renderer` trait (ADR-0004).
//! - [`runner`]: the shared turn loop that streams every `AgentEvent` to a
//!   `Renderer` live (ADR-0004).
//! - [`setup`]: shared frontend runtime construction (provider + tools).

pub mod config;
pub mod context;
pub mod context_files;
pub mod history;
pub mod render;
pub mod runner;
pub mod session;
pub mod setup;
pub mod skills;
pub mod tools;

#[doc(hidden)]
pub mod testutil;
