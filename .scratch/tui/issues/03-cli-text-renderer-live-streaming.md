# 03: CLI TextRenderer + live streaming

**What to build:** The CLI's renderer becomes a thin `TextRenderer` over the shared `Renderer` trait, and the one-shot mode stops rendering post-hoc and streams through the shared `run_turn`. Output stays byte-identical to today (spec user story 36), only its timing changes. The line-based REPL keeps working unchanged during this slice and is removed in ticket 06.

**Blocked by:** 01 — shared renderer model in common, 02 — shared runner in common

**Status:** resolved

- [ ] `crates/cli/src/render.rs` shrinks to a `TextRenderer` implementing the shared `Renderer` trait, preserving today's exact text formatting, including the "structural lines start on their own row" rule (a structural line after non-newline-terminated assistant text begins on a fresh row).
- [ ] The one-shot path (`run_once` / `run_turn` in `main.rs`) calls the shared `run_turn`, streaming live through `TextRenderer`, instead of collecting events and rendering afterwards.
- [ ] Token usage is still printed after the run: the CLI reads its provider's total usage and feeds a `DisplayItem::Usage` to its renderer (the shared runner never renders usage).
- [ ] The CLI's `run_turn` signature stays compatible so the REPL keeps compiling and passing its tests unchanged during this slice (e.g. `TextRenderer` wraps the existing `out: &mut dyn Write`).
- [ ] Tests: one-shot rendered bytes are identical to the old renderer for the same event stream (text + structural line cases, including the structural-line-after-text case and the trailing-newline case); the full CLI test suite passes.

- [ ] The full workspace test suite passes; clippy is clean.

## Notes

- This is the "CLI shrinks and switches to live streaming" decision from the spec; the REPL removal and TUI dispatch land in ticket 06, not here.
- The old `render_event` / `render_events` functions and their tests either move behind `TextRenderer` or are replaced by tests asserting the same bytes through the new seam.

## Answer

`crates/cli/src/render.rs` is now a thin `TextRenderer` over the shared
`Renderer` trait, preserving byte-identical output including the
"structural line starts on its own row" rule and the trailing-newline case.
`main.rs` `run_turn` streams through the shared `slimcode_common::runner::run_turn`
(live) instead of collecting events and rendering post-hoc; `run_once` reads
`provider.total_usage` after the run and feeds a `DisplayItem::Usage` to its own
`TextRenderer`. `run_turn`'s signature is unchanged so the REPL kept compiling
and its tests passed unchanged. Tests assert byte-identity against an inline
reimplementation of the old renderer (including the full-stream case).
