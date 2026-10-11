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
//! - [`compaction`]: token estimation, the auto-compaction threshold, and the
//!   LLM-backed summarization that replaces an old history span with a
//!   `CompactSummary`.
//! - [`context_files`]: `AGENTS.md` discovery (global home file + project
//!   cwd/git-root files) and its `## Project context` system-prompt rendering.
//! - [`tools`]: the seven-tool set bound to a working directory.
//! - [`render`]: the frontend-agnostic renderer seam — `DisplayItem`,
//!   `map_event`, and the `Renderer` trait (ADR-0004).
//! - [`runner`]: the shared turn loop that streams every `AgentEvent` to a
//!   `Renderer` live (ADR-0004).
//! - [`setup`]: shared frontend runtime construction (provider + tools).

pub mod compaction;
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

// The core/ai types the SDK (and other app consumers) name in their public
// APIs, re-exported so a downstream crate can depend on `slimcode-app` alone
// and still spell `slimcode_app::AgentMessage` / `slimcode_app::StopReason` / …
// without a direct `slimcode-core` / `slimcode-ai` dependency (ADR-0011 D4,
// ADR-0021 D1: sdk → app → ai/core/commands).
pub use slimcode_ai::BailianProvider;
pub use slimcode_core::agent::ToolSpec;
pub use slimcode_core::agent::{
    CancelToken, Delta, FinishReason, Provider, ProviderConfig, RunConfig, RunHooks, StopReason,
    TokenUsage, Tool, ToolDecision,
};
pub use slimcode_core::session::{AgentMessage, Message, MessageStopReason, Role, Session};

#[doc(hidden)]
pub mod testutil;
