# 04: slimcode-tui crate + pure App core

**What to build:** A new `slimcode-tui` library crate (ratatui + crossterm + tui-textarea) whose heart is a pure `App` state with a render-to-frame function and an on-key reducer, wrapped later (ticket 06) by the terminal loop. This slice builds and tests the pure core with ratatui `TestBackend` — no real terminal required.

**Blocked by:** 01 — shared renderer model in common, 02 — shared runner in common

**Status:** resolved

- [ ] The crate is a workspace member; `slimcode-common` (and through it the agent/ai types) is the only app dependency of the pure core — the terminal loop lives in a thin, separate shell added in ticket 06.
- [ ] A pure `App` state holding: the transcript (accumulated `DisplayItem`s), the multi-line input box (tui-textarea), the status line fields (cwd, session id, model, running indicator), and view state (scroll offset / follow mode, history-recall state — recall behavior is ticket 05).
- [ ] `App` renders to a frame for a given terminal `Rect` via `TestBackend`: transcript pane, input box, status line. Resize is just re-rendering with a new `Rect`.
- [ ] An on-key reducer handles: Enter submits the current input, Shift+Enter inserts a newline, Ctrl+C and Ctrl+D quit from the input state (equivalent to `/exit`), PgUp/PgDn scroll the transcript.
- [ ] The transcript follows the newest output during a run; PgUp/PgDn stop following until the next streamed item re-follows (auto-scroll decision).
- [ ] Turn markers, reasoning lines, streamed text, tool starts/results (success vs failure visually distinct), stop markers, and token usage all render as distinct entries.
- [ ] `/` commands resolve through `slimcode-commands` (plus skills via `slimcode-common::skills` for `/name` triggers and combined prediction); `/help`, `/history`, `/skills`, `/sessions`, `/usage`, and install/replay confirmations render their output as frontend-owned entries appended to the transcript (no modal/popup in v1).
- [ ] `/new` clears the transcript for the new session; `/load <id>` clears the transcript and shows the restored session id without replaying saved output.
- [ ] A failed turn renders an inline error entry in the transcript and control returns to the input box.
- [ ] I/O-bound commands (saving/loading sessions, reading history, listing skills) surface as an effect that the ticket-06 terminal loop fulfills, so the `App` core stays pure and testable.

- [ ] Unit tests with `TestBackend` frame buffers and scripted key events cover: transcript accumulates streamed text; tool/stop markers render; Enter submits; Shift+Enter appends a multi-line input; Ctrl+C/Ctrl+D quit; auto-scroll follows and yields to PgUp/PgDn; resize re-renders; `/new`/`/load` clear the transcript; a failed turn appends an error entry and returns to input.

- [ ] The full workspace test suite passes; clippy is clean.

## Notes

- This is the "TUI pure core" decision from the spec; `TestBackend` is new prior art for this repo.
- The pure core is the spec's second test seam — keep the terminal loop out of it entirely.

## Answer

The `slimcode-tui` crate now exists as a workspace member with a pure `App`
core in `crates/tui/src/app.rs`. The `App` owns the transcript
(`Vec<Entry>` where an entry is a streamed `DisplayItem`, a frontend
`Notice`, or an inline `Error`), a `tui-textarea` multi-line input box, a
`StatusLine` (cwd, session id, model, running indicator), and scroll/follow
view state. It implements the shared `Renderer` trait so the shared runner
can stream events straight into it, and renders to a caller-supplied
`Frame` (ratatui `TestBackend` in tests — no real terminal).

`App::handle_key` is the on-key reducer: Enter submits (echoing the prompt
as a notice), Shift+Enter inserts a newline, Ctrl+C/Ctrl+D quit, and
PgUp/PgDn scroll the transcript (stopping follow mode; the next streamed
item re-follows). `/` commands resolve through `slimcode-commands` + skills
via `slimcode-common::skills`, with numbered replay (`/!N`), skill
(`/name`) and unknown-command prediction all handled. I/O-bound commands
surface as `Effect`s for the ticket-06 terminal loop to fulfil, keeping the
core pure. `/new`/`/load` clear the transcript (the loop calls
`clear_for_new_session` / `apply_loaded_session`), and a failed turn
appends an inline error entry and returns to the input box.

19 tests cover the required behaviours (streamed-text accumulation, tool/
stop/usage markers, Enter/Shift+Enter, Ctrl+C/Ctrl+D, auto-scroll vs
PgUp/PgDn, resize, `/new`/`/load` clearing, failed-turn error entry) plus
command dispatch. Full workspace tests pass and clippy is clean.
