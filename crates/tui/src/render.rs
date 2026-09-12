//! The TUI's own display vocabulary (ADR-0014 D1).
//!
//! The application layer stays frontend-agnostic with its `DisplayItem` /
//! `Renderer` seam (ADR-0004); the CLI implements a `Renderer` (`TuiAdapter`)
//! that converts each `DisplayItem` into a [`RenderItem`] and sends it over the
//! TUI's channel. Nothing in this module names an `ai`/`core`/`app` type, so
//! the TUI's transcript state machine — the merge and tool-pairing rules in
//! [`App::apply`](crate::app::App::apply) — is independent of the runtime.

use crate::footer::FooterUsage;

/// A skill as the TUI knows it: enough to list, suggest and dispatch it.
/// Built by the CLI from the skills store, so the TUI never imports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    /// Manual-only skills (not advertised to the model) are marked in the
    /// `/skills` list.
    pub disable_model_invocation: bool,
    pub scope: SkillScope,
}

/// Where a skill was found. The TUI-local mirror of the store's scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkillScope {
    /// Shared across every project (`~/.slimcode/skills`).
    User,
    /// Project-local (`<cwd>/.slimcode/skills`).
    Project,
}

/// One unit of display state the TUI applies.
///
/// Agent output (streamed text/reasoning, tool start/result) is derived from an
/// `AgentEvent` by the CLI's adapter; the remaining variants carry CLI-owned
/// state that is not derived from an event — notices and errors, the tokens
/// summary, an installed-skills refresh, the git branch and a new/loaded
/// session. Both kinds travel the same channel so the TUI has exactly one way
/// in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenderItem {
    /// A streamed assistant text fragment (no trailing newline implied).
    Text(String),
    /// A streamed reasoning fragment.
    Reasoning(String),
    /// A tool invocation started, with the raw JSON arguments string.
    ToolStart {
        tool_call_id: String,
        name: String,
        arguments: String,
    },
    /// A tool finished: `ok` distinguishes success from failure.
    ToolResult {
        tool_call_id: String,
        name: String,
        ok: bool,
        result: String,
    },
    /// A frontend-owned notice line (dim).
    Notice(String),
    /// A frontend-owned error line (red).
    Error(String),
    /// A boxed user prompt block (typed, recalled or replayed).
    UserPrompt(String),
    /// The session's accumulated token usage for the footer stats line.
    Usage(FooterUsage),
    /// Replace the skills snapshot used by `/skills` and `/` prediction.
    Skills(Vec<SkillInfo>),
    /// The current git branch (`None` outside a repository).
    Branch(Option<String>),
    /// A new or loaded session: clear the transcript and point the status line
    /// at `id`. The accompanying notice is the CLI's wording.
    SessionChanged { id: String },
}
