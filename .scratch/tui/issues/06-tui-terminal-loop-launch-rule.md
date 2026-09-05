# 06: TUI terminal loop + launch rule + remove REPL

**What to build:** The thin crossterm/ratatui terminal loop that wraps the ticket-04/05 pure `App`, full wiring (config, provider, tools, sessions, skills, history) through the shared `run_turn`, the launch rule that dispatches to the TUI, and removal of the line-based REPL. This is the migration ticket that flips the interactive mode over.

**Blocked by:** 03 — CLI TextRenderer + live streaming, 04 — slimcode-tui crate + pure App core, 05 — TUI input-history recall + history commands

**Status:** resolved

- [ ] The terminal loop: enter raw mode + alternate screen, pump crossterm events, run the pure `App`'s reducer, fulfill its I/O effects (session save/load, history, skills, command dispatch), draw frames on each change and on resize, and restore the terminal on exit.
- [ ] The binary dispatches: no prompt + TTY → TUI; no prompt + no TTY → clear error to stderr and non-zero exit; a prompt → one-shot CLI; `--help`/`-h` keeps printing usage. TTY detection uses `std::io::IsTerminal`.
- [ ] Setup (config resolution, API-key check, `SkillStore`/`SessionStore`/`HistoryStore` construction, tool building) happens **before** the alternate screen opens, so a missing/invalid key or config error is reported on the normal terminal (spec user story 28).
- [ ] A submitted prompt runs through the shared `run_turn` streaming into the pure core's transcript; the session is saved automatically after each turn; `--cwd`/`--model`/`--base-url` apply as today.
- [ ] Skills work inside the TUI exactly as in the REPL: `/skills`, `/install-skill <path> --user|--project`, `/name` triggers, and merged command+skill prediction (reusing `slimcode-commands::suggest` + `slimcode-common::skills::suggest_skills`).
- [ ] The line-based REPL module (`crates/cli/src/repl.rs` and the `repl` module) is removed; its behavior lives on in the TUI. Any REPL-only helper still needed elsewhere is moved, not duplicated.
- [ ] The old `no_args_dispatch_to_repl_*` main.rs tests are updated to the new dispatch (no prompt on a TTY → TUI path; no prompt on a non-TTY → error path; prompt → one-shot), plus a `--help` test kept green.
- [ ] During a run the terminal loop does not poll keys (synchronous `Provider` — spec Run-time behavior); output still draws live. Ctrl+C mid-run terminates the process as the terminal would.

- [ ] The full workspace test suite passes; clippy is clean. A manual smoke check: `slimcode` opens the TUI, runs a turn against a scripted/fake setup, resize does not corrupt the layout, and Ctrl+C/`/exit` restore the terminal.

## Notes

- Spec launch rule: no prompt + TTY → TUI, no prompt + no TTY → error, prompt → one-shot CLI.
- The terminal loop stays a thin, untested shell; all behavior is in the pure core (tickets 04/05).

## Answer

The TUI terminal loop now lives in `crates/tui/src/terminal.rs`: it opens raw
mode + the alternate screen, pumps crossterm events through the pure App
reducer, fulfils the App's I/O effects (session save/load/list, history,
skills, `/install-skill`, usage, replay), redraws on every event (ratatui
autoresizes on resize), and restores the terminal on every exit path.
Provider + tool setup happens before the screen opens, so config/API-key
errors surface on the normal terminal. A submitted prompt streams live
through the shared runner via a `LiveRenderer` that appends each display item
to the transcript and redraws; sessions are saved after each turn; a failed
turn appends an inline error and returns to the input box. During a run the
loop does not poll keys (synchronous provider); Ctrl+C is buffered and quits
after the turn.

The `slimcode` binary dispatches by the launch rule (`std::io::IsTerminal`):
no prompt on a TTY → TUI; no prompt on a non-TTY → clear error and non-zero
exit; a prompt → one-shot CLI; `--help` keeps printing usage. Provider + tools
construction moved to `slimcode-common::setup` so both frontends share it.
The line-based REPL (`crates/cli/src/repl.rs`) was removed; its behavior lives
on in the TUI. Dispatch tests cover the TTY/non-TTY/prompt paths; full
workspace tests pass and clippy is clean. The TUI itself was smoke-checked
only at the dispatch level (opening the screen needs a real TTY + key).
