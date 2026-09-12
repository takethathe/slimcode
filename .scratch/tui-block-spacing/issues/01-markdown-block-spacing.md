# 01: Markdown block spacing (blank row between blocks)

**What to build:** Assistant text and thinking blocks currently render as one dense run of
rows: a blank line in the model's markdown, the gap after a heading, and the gap around a
code block or horizontal rule all vanish, because the markdown renderer only ever *ends* a
block — it never emits a separator, and the parser reports no event for a blank line. After
this ticket, two adjacent markdown blocks are separated by exactly one blank row, a heading
is always followed by a blank row before its section body, a run of blank lines in the
source collapses to that single row, a blank line inside a blockquote renders as a `│ `
row, and a document that ends with blank lines gains no trailing blank row.

**Blocked by:** — (independent)

**Status:** resolved

- [x] The markdown renderer gains a gap row: an unstyled blank row normally, and a row
      carrying only the `│ ` quote prefix (in the quote-border token, so the quote
      border stays continuous) inside a blockquote. The helper is **idempotent** — if the
      output already ends with a gap row it appends nothing.
- [x] Exactly one gap row is emitted between adjacent markdown blocks, covering: heading,
      top-level paragraph, code block, blockquote, list, horizontal rule. A heading is
      followed by a blank row even when the source has no blank line before the next
      block.
- [x] A run of two or more blank lines in the source collapses to one blank row; a
      document whose text ends with blank lines renders no trailing gap row.
- [x] A blank line inside a blockquote renders as a `│ ` row, and a two-paragraph
      blockquote gets exactly one such row between the paragraphs.
- [x] No gap row is emitted *inside* a list item (consecutive blocks within one item stay
      adjacent rows) — recorded here as the rule ticket 04 reuses, since the loose-list
      gap is an explicit exception to it.
- [x] Markdown renderer unit tests cover every case above at the leaf seam (the pure
      markdown render function), including: heading followed by a paragraph; two source
      paragraphs; a collapsed run of blank lines; heading directly after a paragraph;
      fenced code block fenced by gap rows; rule fenced by gap rows; blank line inside a
      quote; no trailing gap row.
- [x] The existing markdown tests that assert exact row counts and indices
      (`mixed_document_renders_blocks_in_order`, the code-block and rule cases, the
      blockquote case) are updated to the new expected rows as part of the red step —
      they are not left to drift.
- [x] The markdown module's header comment is rewritten: it currently states that
      inter-block spacing belongs to the caller, which stops being true. The new text
      states the two-level rule — the renderer owns spacing *inside* a markdown document
      (one gap row between blocks), the transcript row builder owns spacing *between*
      transcript blocks.
- [x] `docs/development.md`'s TUI section states the markdown-side half of the rule (one
      gap row between adjacent markdown blocks; source blank-line runs collapse; the
      heading gap follows from it).
- [x] `cargo fmt --all` applied, `cargo clippy --all-targets --all-features -D warnings`
      clean, `cargo test` green (tmux smoke unaffected).

## Notes

- Spec: `.scratch/tui-block-spacing/spec.md` — Problem Statement, Solution, Implementation
  Decisions ("Markdown renderer owns markdown spacing", "Gap sites", "No gap inside a list
  item", "Trailing gap"), Testing Decisions seam 1.
- Ground truth (pi v0.84.4): `pi-tui/dist/components/markdown.js` appends `""` after a
  heading / paragraph / code block / blockquote / hrule unless the next token is `space`
  (or, for a paragraph, a list), and renders a `space` token as an empty line — i.e. the
  net effect is exactly one blank row between blocks, source blank lines included.
- Deliberately simplified versus pi: pi inspects the byte gap between block tokens to
  avoid doubling a blank row and therefore draws no gap between a paragraph and a directly
  following list. slimcode uses the single rule above, so a paragraph followed directly by
  a list gains one blank row that pi would not draw. This is the documented deviation.
- Lists keep their current rendering in this ticket (flat `• `, no indentation); the loose
  list's item gap and the list geometry are ticket 04's, on top of ticket 03's prefactor.
- Streaming needs no new state: the merged block is re-parsed every frame, so the gap rows
  are recomputed per frame like everything else.
