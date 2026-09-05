# TUI frontend + shared renderer/runner in slimcode-common

Status: ready-for-agent

## Problem Statement

slimcode today has two frontends that share almost nothing except the agent runtime: a one-shot CLI (`slimcode "<prompt>"`) and a line-based REPL (`slimcode`). The REPL is awkward to use — no scrollback, no line editing, no structured view of tool activity, just lines streaming past — and the turn-execution and rendering logic is welded to the terminal binary, so building a richer interactive frontend means either duplicating that logic or ripping it apart first. Without a shared seam, the CLI and any future TUI would inevitably drift.

## Solution

From the user's perspective, `slimcode` becomes a full-screen TUI: start it without a prompt and you get a scrollable transcript, live streamed assistant and tool output, a multi-line input box (Enter submits, Shift+Enter inserts a newline), arrow-key input-history recall, a status line, and all the `/` commands and skills you already know — while `slimcode "<prompt>"` keeps working as a one-shot CLI exactly as before. Under the hood the frontend-agnostic parts — mapping agent events to display units, and the turn loop that streams them — move into `slimcode-common` behind a `Renderer` trait, so the CLI and the TUI each implement only their own rendering.

## User Stories

1. As a slimcode user, I want `slimcode` with no prompt to start the TUI, so that I get an interactive experience instead of the old line-based REPL.
2. As a slimcode user, I want `slimcode "<prompt>"` to still run a one-shot turn, so that my scripts and one-off questions keep working.
3. As a slimcode user, I want `slimcode` on a non-TTY (pipe, CI) to fail with a clear error and a non-zero exit, so that I am not dumped into a broken terminal UI.
4. As a slimcode user, I want `--help` to keep printing usage, so that the options remain discoverable.
5. As a slimcode user, I want `--cwd`, `--model`, and `--base-url` to apply to both the TUI and the one-shot CLI, so that configuration is consistent.
6. As a TUI user, I want the assistant's text to stream into the transcript as it is generated, so that I can read along while the agent works.
7. As a TUI user, I want tool starts and tool results shown as distinct entries, so that I can see what the agent is doing.
8. As a TUI user, I want failed tool results visually distinct from successful ones, so that I can spot problems at a glance.
9. As a TUI user, I want turn and stop markers rendered, so that I can tell where one turn ends and the next begins.
10. As a TUI user, I want the model's reasoning shown as its own line kind, distinct from the final answer, so that I can follow the model's thinking without confusing it with the response.
11. As a TUI user, I want token usage shown after a run, so that I can track cost and consumption.
12. As a TUI user, I want a scrollable transcript that keeps earlier output reachable, so that long sessions remain readable.
13. As a TUI user, I want the transcript to auto-scroll to the newest output during a run, so that I can read along without chasing the cursor.
14. As a TUI user, I want PgUp/PgDn to move through the transcript, so that I can revisit earlier output even mid-session.
15. As a TUI user, I want a multi-line input box, so that I can write long prompts comfortably.
16. As a TUI user, I want Enter to submit the current input, so that submitting a prompt is one keystroke.
17. As a TUI user, I want Shift+Enter to insert a newline, so that I can write multi-line prompts without a continuation marker.
18. As a TUI user, I want ↑/↓ to recall previous prompts, so that I can re-run recent work without retyping.
19. As a TUI user, I want `/history` to list recent prompts, so that I can see what I have been working on.
20. As a TUI user, I want `/!!` to re-run my most recent prompt, so that I can repeat the last action instantly.
21. As a TUI user, I want `/!N` to re-run the N-th listed prompt, so that I can repeat any recent prompt.
22. As a TUI user, I want all existing `/` commands to keep working (`/help /new /load /sessions /usage /save /history /skills /install-skill /!! /!N /exit`), so that I lose nothing by switching from the REPL.
23. As a TUI user, I want `/help` and unknown-`/` suggestions generated from the same shared command registry, so that behavior matches the documented commands.
24. As a TUI user, I want to trigger an installed skill with `/name`, so that skills work exactly as they did in the REPL.
25. As a TUI user, I want `/skills` and `/install-skill` to work, so that I can manage skills from inside the TUI.
26. As a TUI user, I want my session saved automatically after each turn, so that I can resume later.
27. As a TUI user, I want `/load <id>` to resume a saved session, so that long work survives a restart.
28. As a TUI user, I want a missing or invalid API key reported before the alternate screen opens, so that I am not stuck in a dead UI.
29. As a TUI user, I want the output of `/` commands (`/help /history /skills /sessions /usage` and install confirmations) rendered into the transcript, so that everything I did is visible in one scrollable place.
30. As a TUI user, I want a failed turn (provider or runner error) shown inline in the transcript and control returned to the input box, so that the TUI survives a bad turn.
31. As a TUI user, I want `/new` to clear the transcript for the new session, so that I can see clearly where the new conversation starts.
32. As a TUI user, I want `/load <id>` to clear the transcript and show the restored session id, so that I am not confused by output from a previous session.
33. As a TUI user, I want a status line showing cwd, session id, model, and a running indicator, so that I always know where I am and what is happening.
34. As a TUI user, I want Ctrl+C and Ctrl+D to quit the TUI from the input state, so that exiting does not require typing a command.
35. As a TUI user, I want terminal resize to re-render cleanly, so that the layout is never corrupted.
36. As a one-shot CLI user, I want the exact same rendered output I had before, so that the shared-renderer migration is invisible to me.
37. As a developer, I want the CLI and TUI to share one event-to-display mapping and one turn loop, so that behavior cannot drift between frontends.

## Implementation Decisions

- **New frontend crate**: a `slimcode-tui` library crate (ratatui + crossterm + tui-textarea) that owns the TUI; the existing `slimcode` binary depends on it and dispatches to it when started without a prompt. The workspace gains this crate.
- **Shared renderer model** in `slimcode-common`: a frontend-agnostic `DisplayItem` enum — turn marker, reasoning line, streamed text fragment, tool start, tool result, stop marker, token usage — produced from an agent event by a pure `map_event` function, and consumed through a `Renderer` trait whose single method takes a `DisplayItem` and returns a `Result<(), String>`.
- **Shared runner** in `slimcode-common`: a free function that takes a provider, tools, messages, a `RunConfig`, and a `&mut dyn Renderer`, drives the existing agent loop, and streams every event to the renderer live as it happens, returning the updated message history. Errors stay `String`, matching the existing agent API.
- **Token usage stays a frontend concern**: after the run, the frontend reads its concrete provider's total usage and feeds a `DisplayItem::Usage` to its renderer; the runner does not render usage.
- **CLI shrinks and switches to live streaming**: the CLI's existing renderer becomes a thin `TextRenderer` over the shared trait (preserving the current text formatting, including the "structural lines start on their own row" rule), the line-based REPL module is removed, and the CLI's one-shot mode stops rendering post-hoc and instead streams through the shared `run_turn`. The final output is byte-identical to today (user story 36), only its timing changes (output appears as it is produced).
- **TUI pure core**: the TUI is split into a pure `App` state with a render-to-frame function and an on-key reducer, wrapped by a thin crossterm/ratatui terminal loop. The pure core is testable with ratatui's `TestBackend` without a real terminal. `render` takes the current terminal `Rect`, so resize is just a re-render.
- **Run-time behavior (synchronous provider)**: the `Provider` seam stays synchronous, so during a run the terminal loop does not poll keys — output still draws live (the renderer renders as events stream), but the user cannot scroll or interrupt mid-run. This matches today's REPL. Ctrl+C during a run terminates the process as the terminal would normally. Interrupting a running turn is explicitly out of scope (async provider is a separate effort).
- **Launch rule**: no prompt and a TTY → TUI; no prompt and no TTY → error to stderr and non-zero exit; a prompt → one-shot CLI. TTY detection uses `std::io::IsTerminal`.
- **Input-history recall keys**: in the multi-line input box ↑/↓ would otherwise move the cursor, so history recall is a distinct `App` state — ↑/↓ at the empty input enters recall, ↑/↓ move through entries, and any other key exits recall back to editing. This state lives in the pure core and is covered by unit tests, not by terminal-specific code.
- **Auto-scroll**: the transcript follows the newest output during a run; PgUp/PgDn scroll the view and stop following until the next streamed item re-follows.
- **Command output into the transcript**: `/help`, `/history`, `/skills`, `/sessions`, `/usage`, and install/replay confirmations render as frontend-owned display entries appended to the transcript, so there is no separate modal/popup in v1.
- **Inline turn errors**: a provider or runner error renders as an error entry in the transcript and control returns to the input box; the session is still persisted.
- **`/new` and `/load` clear the transcript**: a fresh session starts with an empty transcript; `/load` shows the restored session id but does not replay the saved messages' output.
- **Quit keys**: Ctrl+C and Ctrl+D quit from the input state (equivalent to `/exit`); `/exit` and `/quit` remain.
- **Status line**: shows cwd, session id, model, and a running indicator while a turn executes.
- **Commands and history are reused, not rewritten**: `/` commands come from `slimcode-commands`; input history comes from `slimcode-common::history` (↑/↓ recall plus the three history commands).
- **Config is unchanged**: the existing four-layer config resolution feeds both frontends; setup failures (API key, config) happen before the TUI alternate screen opens.

## Testing Decisions

- A good test asserts **external behavior** — the ordered stream of display items out of the runner, the bytes out of the CLI renderer, the frame buffer out of the TUI pure core — never internal state.
- **Primary seam**: the `Renderer` trait + `run_turn` boundary. Tests inject a scripted provider (the same `FakeProvider` pattern as the agent crate) and a recording renderer, asserting the display-item stream and the returned message history.
- **Second seam**: the TUI pure core (`App` render/on-key), tested with ratatui `TestBackend` frame buffers and scripted key events; the terminal loop itself is a thin, untested shell.
- **Modules under test**: the new `render` and `runner` modules in `slimcode-common`, the CLI `TextRenderer`, and the TUI pure core. The agent and ai crates are untouched.
- **Prior art**: agent's scripted `FakeProvider`; the CLI's `Vec<u8>` output capture for `render_events`; the REPL's scripted-byte-input integration tests with a temp dir. The TUI's `TestBackend` approach is new prior art for this repo.
- Cases to cover: raw tool-call deltas are suppressed; tool start/result map to distinct display items; streamed text and structural lines stream in order; a runner error propagates; CLI text output is byte-identical to the old renderer; TUI transcript accumulates streamed text, renders tool/stop markers, updates on key events, and appends multi-line input on Shift+Enter; history recall enters/exits on ↑/↓ and exits on typing; auto-scroll follows on new items and yields to PgUp/PgDn; resize re-renders the layout; `/new`/`/load` clear the transcript; a failed turn appends an error entry and returns to input.

## Out of Scope

- Web or other non-terminal frontends.
- Async providers or changes to the `Provider` seam (it stays synchronous); consequently, interrupting a running turn from the TUI is out of scope (Ctrl+C mid-run terminates the process).
- Changing the default serial tool execution or `RunConfig` semantics.
- Mouse support, themes, syntax highlighting, or clipboard integration in the TUI v1.
- A separate `slimcode-tui` binary — the TUI is a library invoked by the existing binary.
- Per-session input history — history remains global, as today.
- Replaying a loaded session's saved output into the transcript.

## Further Notes

- Architecture decisions are recorded in ADR-0003 (ratatui + crossterm, TUI replaces the REPL, non-TTY errors) and ADR-0004 (shared `DisplayItem` + `Renderer` + `run_turn`).
- Glossary terms live in `CONTEXT.md`: `Frontend`, `TUI`, `Renderer`, `DisplayItem` (plus the existing `Prompt`, `Command`, `Session`, `Input history`, `Multi-line prompt`); use them consistently in code, tests, and docs.
- Docs to sync before commit: `docs/development.md`, `docs/user-manual.md`, `docs/index.md`, `docs/explanation.md`.
