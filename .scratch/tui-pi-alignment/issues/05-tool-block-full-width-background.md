# 05: Tool blocks paint their state background full-width

**What to build:** Tool blocks currently color only the cells that hold glyphs — the
middle rows (`" <text>"`) stop mid-row, so the pending/success/error background ends
where the text ends; only the top/bottom spacer rows span the full width. pi paints the
whole `Box` rect (padding included) in the state bg. Fix: every row of a tool block is
padded to the pane width with the block's background so the colored region is one
continuous full-width rectangle, exactly like pi and like the existing user prompt box.

**Blocked by:** — (independent)

**Status:** resolved

- [x] Add a pad-to-width helper in `app.rs` (append trailing `Span` of background
      spaces so the row measures exactly `width` cells; compute padding with
      `display_width` so CJK double-width stays exact) and use it for **every** non-
      spacer row in `tool_rows` — title, args (while 06 still has them), output
      preview, and the two-tone `… (N more lines, Ctrl+O to expand)` hint row. Empty
      spacer rows stay `" ".repeat(width)`.
- [x] Audit the other transcript blocks for the same defect: the user prompt box pads
      to width already (leave it); flat notice/error lines have no background (leave).
      Only tool blocks change.
- [x] Frame-buffer tests (TestBackend): extend the existing `tool_block_*` tests and
      add a dedicated test asserting the state bg reaches the **rightmost column** of
      the pane for each row kind — title row, an output row, and the expand-hint row —
      for pending, success, and error states. Keep `cell_bg_at`/`row_bg_equals`
      helpers as the assertion seam.
- [x] `cargo fmt` + clippy `-D warnings` clean + `cargo test` green (tui suite; tmux
      smoke unaffected visually — it cannot see colors, so it needs no change).
- [x] Docs sync (AGENTS.md): spec US 5 wording and ADR-0006 D2 block description note
      that the state-colored region spans the full pane width (pi `Box` bgFn fills the
      padded rect); `docs/user-manual.md` tool-block bullet if it describes the
      colored region's shape. Feature→test map line added.

## Notes

- Ground truth: pi `tool-execution.ts` wraps call/result content in a `Box(1, 1, …)`
  whose `bgFn` paints the whole padded width; collapsed state colors the same full box.
- No interaction change; this is purely a render fix. Row count math
  (`total_lines`/`all_rows`) is unaffected (padded rows replace unpadded ones
  ​1:1).
- Related: ticket 06 rebuilds the title area on top of this; land 05 first so the new
  per-tool titles already render full-width.
