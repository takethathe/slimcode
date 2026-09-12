# TUI runtime seam: terminal lifecycle in the library, application lifecycle in the CLI

`slimcode-tui` is entered, not run: the CLI owns the process (raw mode, alternate screen, panic
hook, signals, exit code), builds the terminal, and passes it to `tui::run`. The library owns the
frame loop and everything that exists only to keep the frame loop responsive; the CLI owns
everything that decides what should happen. Today the opposite is true:
`tui::terminal::run(cwd, config, store, history, skills, context_files, environment)` builds the
provider, loads sessions, interprets `/` commands and appends to the session log from inside the TUI
crate (~844 lines of `terminal.rs`), so the "library" cannot be reused and the "entry point" owns
only argv.

## Decisions

### D1 — `tui::run(terminal, app, handler)`

The library's entry point takes a terminal the caller built, the pure `App`, and a
`UiHandler` the caller implements:

```rust
pub trait UiHandler {
    /// Answer one user intent the reducer produced. Returning Quit ends the loop.
    fn on_effect(&mut self, effect: Effect, emit: &mut dyn FnMut(RenderItem)) -> ControlFlow;
    /// Start one turn; stream its display items through `emit`, return when done.
    fn submit(&mut self, prompt: Prompt, emit: &mut dyn FnMut(RenderItem)) -> Result<TurnReport, String>;
    /// Cancel the running turn at the next runner boundary.
    fn cancel(&mut self);
}
```

Raw mode, the alternate screen, the terminal title, the panic hook and the exit code stay in the
CLI, before and after `tui::run`. `App::new` takes the initial UI state (session id, branch, skills,
history) plus an injected completion provider; everything that changes afterwards arrives as a
`RenderItem` (ADR-0014), so there is no second mutation surface to keep in sync.

### D2 — The library owns UI mechanics: frame loop, input polling, tick, worker thread

The loop polls crossterm events with an ~80 ms frame budget, applies `RenderItem`s drained from a
channel, redraws, and calls `handler.on_effect` for each `Effect` the reducer produces. When the
handler reports a turn should run, the library spawns it on a scoped worker thread and drains the
channel while it streams — the worker exists solely so the frame loop (spinner, live text) keeps
running during a blocking provider call. This is UI mechanics, not application lifecycle, so it
stays in the library.

### D3 — `Effect` is the UI vocabulary; `RenderItem` is the display vocabulary

`Effect` (quit, submit, load session, list sessions, install skill, …) is the reducer's *intent* and
carries no semantics: `Effect::LoadSession(id)` means "the user asked to load this session", not
"read this file". `on_effect` answers it — the CLI reads the store, updates its own state and emits
the resulting `RenderItem`s (`SessionChanged`, `Notice`, `Error`, replayed transcript entries). The
reducer never parses commands beyond turning text into an intent, and never touches a file.

## Considered Options

- **The CLI writes the whole loop** (library exports only `App`, `draw`, key handling and terminal
  helpers) — rejected: `FRAME_MS` polling, the mpsc channel, the scoped worker thread and resize
  handling would move into the CLI crate; the library would be a widget kit rather than a terminal
  application library, and every future frontend would rewrite the loop.
- **Keep today's `run(cwd, config, store, …)` signature and swap internals for callbacks** —
  rejected: the library's public API would still name config, session store and skills, so the
  "no application lifecycle in the library" claim could not be asserted by a dependency test.
- **Put the worker thread in the CLI** (queue turns, let the library poll a caller-provided source)
  — rejected: the channel and its backpressure are part of the frame loop, and splitting them across
  the seam means two places must agree on when a turn is in flight for the spinner/`Esc` states.

## Consequences

- Provider construction, session I/O, skills, history, context files, environment, command
  semantics, `close_turn` and the session-log append path all move from `crates/tui` to
  `crates/cli`; the TUI keeps `App`, `Entry`, the components, and the loop.
- `tui::App`'s public mutation API shrinks to `App::new` + `apply(RenderItem)`; the
  `impl Renderer for App` disappears (ADR-0014).
- Tests: the pure `App` keeps its `TestBackend` snapshot coverage; the CLI's handler is testable
  against a fake `UiHandler` that records the rendered items, replacing today's terminal-loop
  integration tests.
- `docs/development.md`'s TUI section (worker-thread runner, `terminal::run`) is rewritten in S4.
