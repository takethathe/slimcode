# 07: docs sync + ADR commit

**What to build:** Sync the docs with the TUI migration (per AGENTS.md rule 3 and the spec's Further Notes), and land the two pending ADRs. No behavior change.

**Blocked by:** 06 — TUI terminal loop + launch rule + remove REPL

**Status:** resolved

- [ ] `docs/development.md`: crate table gains `crates/tui` (`slimcode-tui`); `crates/cli` row drops the REPL and notes the TUI + shared renderer/runner in `slimcode-common`; add sections for the `render`/`runner` modules in common, the TUI pure core, and the synchronous-provider run-time behavior.
- [ ] `docs/user-manual.md`: the interactive section moves from the line-based REPL to the full-screen TUI — key bindings (Enter / Shift+Enter / ↑-↓ history recall / PgUp-PgDn / Ctrl+C / Ctrl+D), status line, transcript behavior, `/new`/`/load` clearing the transcript, and the non-TTY error; the `\`-continuation multi-line prompt explanation is replaced by Shift+Enter.
- [ ] `docs/index.md`: table updated to reflect the TUI.
- [ ] `docs/explanation.md`: the frontend-agnostic `DisplayItem` / `Renderer` / `run_turn` seam and why the TUI replaces the REPL.
- [ ] `CONTEXT.md` is consistent with what was actually built (the `TUI`, `Frontend`, `Renderer`, `DisplayItem`, `Multi-line prompt` terms already exist — verify none need adjusting, e.g. the multi-line prompt definition now references Shift+Enter, which is already the case).
- [ ] ADR-0003 and ADR-0004 (currently untracked) are committed with the work they describe, and the ADR directory listing in `docs/index.md` mentions the TUI entries.
- [ ] `cargo fmt --all` clean and `cargo clippy --all-targets --all-features --message-format=json -- -D warnings` has zero errors/warnings before committing.

## Notes

- Docs may be in Chinese or English but keep the glossary terms consistent (per AGENTS.md rule 6 and `CONTEXT.md`).
- Commit as `docs(...)` / `feat(...)` per AGENTS.md git conventions; local commits only, no push.

## Answer

All docs synced with the TUI migration (AGENTS.md rule 3): development.md
gains the `slimcode-tui` crate (pure App core + terminal loop, sync-provider
run-time behavior), the `render`/`runner`/`setup` modules in `slimcode-common`,
and the CLI launch rule; user-manual.md moves the interactive section from the
line-based REPL to the full-screen TUI (Enter submits, Shift+Enter inserts a
newline, up/down history recall, PgUp/PgDn, Ctrl+C/Ctrl+D, status line,
transcript behavior, `/new` and `/load` clearing the transcript, and the
non-TTY error); explanation.md documents the frontend-agnostic `DisplayItem` /
`Renderer` / `run_turn` seam (ADR-0004) and why the TUI replaces the REPL
(ADR-0003); index.md lists the ADR entries.

ADR-0003 and ADR-0004 landed in `acee219` together with the pre-existing root
files (`.gitignore` `.codex`, AGENTS.md CodeGraph section, CONTEXT.md
TUI/Frontend/Renderer/DisplayItem glossary terms — CONTEXT.md was already
consistent with what was built). Stale REPL references in code doc-strings were
refreshed (`/exit` description is now "quit the TUI"; common/agent/ai/cli
comments point at the TUI or "interactive frontend"). `cargo fmt --all` clean,
clippy 0 issues, full workspace tests green.
