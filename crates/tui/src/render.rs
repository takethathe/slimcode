//! The TUI's own display vocabulary (ADR-0014 D1).
//!
//! The application layer stays frontend-agnostic with its `DisplayItem` /
//! `Renderer` seam (ADR-0004); the CLI implements a `Renderer` that converts
//! each `DisplayItem` into a [`RenderItem`] and streams it into the library
//! (ADR-0014 D2). Nothing in this module names an `ai`/`core`/`app` type, so
//! the TUI's transcript state machine — the merge and tool-pairing rules in
//! [`App::apply`](crate::app::App::apply) — is independent of the runtime.

use crate::footer::FooterUsage;

/// One unit of display state the TUI applies.
///
/// Agent output (streamed text/reasoning, tool start/result) is derived from an
/// `AgentEvent` by the CLI's adapter; the remaining variants carry CLI-owned
/// state that is not derived from an event — notices and errors, the tokens
/// summary, the git branch and a new/loaded session. Both kinds travel the same
/// channel so the TUI has exactly one way in.
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
    /// A skill active block: the full `<skill>` block of an activated skill
    /// (a `/skill:name` trigger or a `read` of a `SKILL.md`), rendered as a
    /// distinct boxed block with a `[skill] <name>` header instead of a
    /// generic user prompt or a `read` tool block.
    Skill { name: String, content: String },
    /// The session's accumulated token usage for the footer stats line.
    Usage(FooterUsage),
    /// The current git branch (`None` outside a repository).
    Branch(Option<String>),
    /// A new or loaded session: clear the transcript and point the status line
    /// at `id`. The accompanying notice is the CLI's wording.
    SessionChanged { id: String },
    /// The session picker's rows: opens the library's full-screen picker view.
    /// `Esc` (or a later `SessionChanged`) closes it again. The rows are the
    /// CLI's data — the title is already the session's, or its id when it has
    /// none, and `meta` is the preformatted right column — while the library
    /// owns the selection, the layout and the `*`/`›` markers.
    SessionPicker { rows: Vec<SessionRow> },
}

/// One row of the session picker: the id the CLI loads when the row is picked,
/// the display title and the preformatted right column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRow {
    /// The session id that `Effect::LoadSession` names.
    pub id: String,
    /// The display title (the session's title, or its id when it has none).
    pub title: String,
    /// The right-aligned detail column (message count and modification time),
    /// composed by the CLI. Empty when there is nothing to show.
    pub meta: String,
}
