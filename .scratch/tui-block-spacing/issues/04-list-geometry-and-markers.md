# 04: List geometry, bullet glyph and checkboxes

**What to build:** Lists render like real markdown in the TUI. Items open with a `- `
bullet (`*` and `+` normalize to `- `), ordered items keep `N. ` numbering from the list's
start, a checkbox written as `[x]` / `[ ]` / `[X]` is preserved exactly as written in the
body color, nesting indents four spaces per level so the tree structure is visible, a
wrapped item's continuation rows line up under the item text rather than under the bullet,
each paragraph inside an item gets its own row (no more `a1a2` concatenation), a code block
inside an item is indented with its item, and a loose list puts exactly one blank row
between items and none after the last one. Wrapping accounts for the quote prefix, the
indentation and the marker width, so no indented row overflows the pane.

**Blocked by:** 03 (the frame stack and prefix pipeline this ticket drives); transitively 01
(the gap-row helper the loose-list gap reuses)

**Status:** resolved

- [x] Unordered items render with a `- ` bullet; source `*` and `+` bullets normalize to
      `- `. Ordered items render `N. ` renumbered from the list's start number, still
      honouring an explicit start (e.g. a list starting at 5).
- [x] A checkbox written as `[x]`, `[ ]` or `[X]` is preserved verbatim, including its
      capitalization, and stays in the body color (not repainted as bullet decoration).
      The parser's task-list option stays off, so the checkbox remains ordinary item text
      — the deliberate choice over pi's synthesized, `[x]`-normalizing, bullet-colored task
      marker.
- [x] Nesting indents four spaces per level; a nested list renders at its own depth with
      its own indent, and a nested ordered list inside an unordered item (and vice versa)
      keeps both its numbering and its indentation.
- [x] An item's first row is `indent + marker`; its continuation rows (wrapped text, a
      second paragraph, a nested list's non-item rows, a code block inside the item) are
      prefixed by `indent + spaces as wide as the marker`, aligning under the item text.
- [x] Wrapped width subtracts the quote prefix, the indentation and the marker width, so
      every rendered row fits the pane (no row exceeds the content width, CJK double-width
      included).
- [x] A paragraph starting inside an open item flushes the item's accumulated content as
      its own row block: an item holding two paragraphs renders two row blocks, not one
      concatenated row, and no gap row is inserted between them.
- [x] A loose list (items separated by blank lines, or an item holding two block-level
      elements separated by a blank line) gets exactly one gap row between items and none
      after the last item — the documented exception to ticket 01's "no gap row inside a
      list item" rule, implemented through ticket 01's idempotent gap helper so the
      trailing position never doubles into the following block's gap.
- [x] Markdown renderer unit tests cover every case above at the leaf seam, and the
      characterization tests added by ticket 03 are updated to the new expected rows
      (that update is the red step for this ticket): bullet glyph and normalization;
      ordered numbering and explicit start; each checkbox spelling verbatim; four-space
      nesting; nested ordered inside unordered; a wrapped item's continuation alignment;
      a two-paragraph item; a code block inside an item; a loose list's single gap row and
      its absent trailing gap; no row wider than the content width at a nested level.
- [x] `docs/development.md`'s TUI section states the list geometry (bullet glyph,
      checkbox preserved verbatim, four-space nesting, continuation alignment, loose-list
      gap) next to the markdown spacing rule from ticket 01; `docs/user-manual.md` gains or
      updates a line only where it describes list or checkbox rendering.
- [x] `cargo fmt --all` applied, `cargo clippy --all-targets --all-features -D warnings`
      clean, `cargo test` green (tmux smoke unaffected).

## Notes

- Spec: `.scratch/tui-block-spacing/spec.md` — Solution ("Lists"), User Stories 17–27,
  Implementation Decisions ("List geometry (pi's `renderList`)", "Paragraph flushing inside
  items", "Bullet glyph", "Checkboxes stay literal text"), Testing Decisions.
- Ground truth (pi v0.84.4, `pi-tui/dist/components/markdown.js` `renderList`): four-space
  indent per depth, `firstPrefix`/`continuationPrefix`, `itemWidth = width -
  visibleWidth(firstPrefix)`, nested lists recursed at `depth + 1`, and a blank row after
  every non-last item when the list is loose.
- Fixes a pre-existing defect found while specifying this work: an item holding two
  paragraphs (`- a1`, blank, indented `a2`) currently renders `a1a2`, because a paragraph
  boundary inside an item is not flushed. Covered by a new test rather than a separate
  ticket.
- Deliberate deviations from pi, to keep the record straight: checkboxes are preserved
  verbatim rather than normalized to `[x] ` and recolored; ordered markers are renumbered
  as pi does for assistant messages (pi preserves the source marker only for user
  messages); a paragraph directly followed by a list gains one blank row that pi does not
  draw (ticket 01's documented simplification).
