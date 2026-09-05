# 02: Block-aware transcript + two-region layout

**What to build:** Restructure the pure `App` transcript from flat glyph-prefixed lines to pi-style blocks, and re-lay the screen into a scrollable Transcript plus a fixed Dock. Blocks: user prompt (boxed `userMessageBg` + markdown), assistant text (merged markdown stream), thinking (italic gray markdown), tool block (paired start/result; background pending → success/error; bold title, pretty args, gray output, collapsed to 10 lines), dim notice, red error. Startup header block (bold accent `slimcode` + dim version + compact hints). Right-edge transcript scrollbar (`selectedBg` thumb, auto mode). Editor border color (blue rest / accent running) with title removed, placeholder kept. Completion popup restyled to SelectList tokens (accent selection + `→ ` cursor, muted descriptions, `(i/n)` muted). Ctrl+O toggles tool expansion.

**Blocked by:** 01 — Theme + markdown

**Status:** resolved

- [x] Transcript entries become block-aware kinds; streamed text/reasoning still merge; tool start/result pair into one block (sequential per the shared runner).
- [x] User prompts (typed, recalled, `/!!`-replayed) render as boxed markdown messages; no `> prompt` notice line.
- [x] Turn markers, `✓ done`/`⚠ stopped` stop lines, and the per-turn usage line removed; abnormal stops and turn failures render as red error text; `/usage` keeps a dim detail line.
- [x] Header block pushed at startup; `/new` clears it with the transcript.
- [x] Layout becomes Transcript / Status row (reserved) / editor / completion popup / Footer; transcript pane has the scrollbar; resize keeps the dock fixed.
- [x] Editor border color reflects running state; popup uses SelectList tokens; Ctrl+O expands/collapses tool output.
- [x] All changes covered by `TestBackend` tests (buffer assertions for each new block kind, expansion toggle, scrollbar thumb, layout under resize); clippy clean.

## Notes

- Spec: User Stories 1–9, 15–17, 21; Implementation Decisions "Block-aware transcript model", "Tool pairing and expansion", "Two-region layout", "Startup header", "Completion popup", "Removals"; ADR-0006 D2/D4.
- The status indicator row and the Footer are implemented in ticket 03; this ticket reserves their rows (or the layout constants they introduce).