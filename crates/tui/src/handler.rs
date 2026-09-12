//! The runtime seam between the terminal library and the CLI (ADR-0013).
//!
//! `slimcode-tui` is entered, not run: the CLI owns the process (raw mode, the
//! alternate screen, the terminal title, the exit code), the application
//! lifecycle (provider, sessions, skills, history, command semantics) and
//! implements [`UiHandler`]. The library owns what exists only to keep the
//! frame loop responsive: input polling, the tick, drawing, the render-item
//! channel and the worker thread a turn runs on.
//!
//! Everything the library asks the CLI for is expressed with the two TUI
//! vocabularies: [`Effect`](crate::app::Effect) for intent coming out of the
//! reducer, [`RenderItem`] for display state going in.

use crate::app::Effect;
use crate::render::RenderItem;

/// One completion candidate: the `/`-prefixed spelling to commit and a short
/// description.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionItem {
    /// The spelling to commit (e.g. `/usage` or `/skill:name`) — the bare
    /// spelling, never a usage string with an argument placeholder.
    pub value: String,
    pub description: String,
}

/// The `/`-candidate pool, injected at construction (ADR-0014 D3).
///
/// The reducer refreshes the popup on every keystroke, but it neither knows
/// the command registry nor the skills store: the CLI builds one of these from
/// both and hands it to the library. Candidates that the user then picks come
/// back as an ordinary [`Effect::Command`], so the provider is a suggestion
/// source, never a dispatcher.
pub trait CompletionProvider: Send + Sync {
    /// Ranked candidates for a partial `/` input, best first. An input that
    /// does not start with `/` yields none.
    fn complete(&self, input: &str) -> Vec<CompletionItem>;
}

/// One turn's input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    /// The message the model sees: a typed prompt, or a CLI-built skill
    /// trigger block.
    pub text: String,
    /// Whether the raw input also belongs in the input history. Replays and
    /// skill triggers do not record.
    pub record: bool,
}

/// What a finished turn produced, as far as the frame loop cares.
///
/// The library ignores the counters — the CLI has already streamed everything
/// the user sees — but the report keeps the seam honest: the CLI can describe
/// what happened without the library naming a runtime type.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TurnReport {
    /// Messages the turn itself produced (assistant replies and tool results)
    /// that entered the session log. The prompt the turn was built from is not
    /// counted: it is recorded before the turn runs.
    pub appended: usize,
}

/// What the frame loop does after the handler answered an effect.
///
/// A crate-local enum rather than `std::ops::ControlFlow`: the handler often
/// turns a command (`/!!`, a skill trigger) into a turn, and the library has to
/// learn about that without the handler running anything itself — only the
/// library may own the worker thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlFlow {
    /// Keep the frame loop running.
    Continue,
    /// Run this turn on the library's worker thread, then keep running.
    Submit(Prompt),
    /// End the frame loop (the user quit).
    Quit,
}

/// The application half of the terminal library, implemented by the CLI
/// (ADR-0013 D1).
///
/// `submit` and `cancel` take `&self` because a turn runs on a worker thread
/// while the frame loop stays alive: the CLI is in charge of whichever
/// interior mutability that needs. The trait is `Sync` for the same reason.
pub trait UiHandler: Sync {
    /// Answer one user intent the reducer produced. Whatever the CLI decides
    /// reaches the UI as a [`RenderItem`] through `emit`.
    fn on_effect(&mut self, effect: Effect, emit: &mut dyn FnMut(RenderItem)) -> ControlFlow;

    /// Run one turn to completion, streaming its display items through `emit`.
    /// Called on the library's worker thread; `Err` is a failure the library
    /// shows as an error line (the CLI has already closed the session log).
    fn submit(
        &self,
        prompt: Prompt,
        emit: &mut dyn FnMut(RenderItem),
    ) -> Result<TurnReport, String>;

    /// Cancel the running turn at the next runner boundary (the user pressed
    /// Esc). Called from the frame loop while `submit` is blocked.
    fn cancel(&self);
}
