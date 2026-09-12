# TUI transcript spacing + markdown list geometry

Status: resolved

## Problem Statement

The TUI transcript reads as one dense wall of text. Two separate causes:

1. **No gap between transcript blocks.** The row builder inserts a blank row only before a user prompt and before a tool block; assistant text, thinking blocks, notices and errors butt directly against whatever precedes them.
2. **Markdown block spacing is thrown away.** `pulldown-cmark` emits no event for a blank line, and the renderer's block flush only ends a block — it never emits a separator — so a blank line between two paragraphs, a loose list, a blank line inside a quote, and the gap after a heading all vanish. A heading is followed immediately by its section body.

List rendering is a third, related defect that the same code owns: the bullet glyph is `• ` instead of `- `, nesting depth is dropped entirely (`- a` / `  - b` / `    - c` all render in the same column), continuation rows are not aligned under the item text, and a list item holding two paragraphs has its paragraphs concatenated into one row (`- a1` / blank / `  a2` renders as `a1a2`).

The user wants: a blank row between blocks, markdown blank lines rendered, a heading followed by a blank row before its section body, `-` as the bullet glyph, and checkboxes displayed exactly as written.

## Solution

From the user's perspective, TUI output gains breathing room at two levels, and lists render like real markdown:

- **Between transcript blocks** — exactly one blank row separates every transcript block from the one above it (the startup header excepted: nothing precedes it, and no blank row trails the last block before the dock). This covers user prompts, assistant text, thinking blocks, tool blocks, notices and errors.
- **Inside markdown** — exactly one blank row separates any two adjacent markdown blocks (heading, paragraph, code block, blockquote, rule, list). A heading is therefore always followed by a blank row before its section body. A run of blank lines in the source collapses to that one blank row. A blank line inside a blockquote renders as a `│ ` row. Code blocks keep a blank row before and after them.
- **Lists** — items render with a `- ` bullet (the source's `*` / `+` normalize to `- `), ordered items keep their `N. ` renumbering (honoring an explicit start), a checkbox written as `[x]` / `[ ]` / `[X]` is preserved exactly as written, nesting indents four spaces per level, continuation rows line up under the item text, a loose list puts one blank row between items, and an item holding several paragraphs gives each paragraph its own row (no concatenation, no blank row between them).
- The one-shot CLI frontend renders byte-identical output as before; only the TUI changes.

## User Stories

1. As a TUI user, I want a blank row between my prompt and the assistant's answer, so that the turn boundary is visible.
2. As a TUI user, I want a blank row between assistant text and the thinking block that precedes or follows it, so that reasoning is not glued to the answer.
3. As a TUI user, I want a blank row between assistant text and a tool block, and between a tool block and the assistant text that follows it, so that tool activity reads as its own unit.
4. As a TUI user, I want a blank row before every notice (dim line) and error (red line), so that command output such as `/usage`, `/help` or a failure is visibly separate.
5. As a TUI user, I want consecutive tool blocks separated by a blank row, so that a parallel tool batch is scannable.
6. As a TUI user, I want the startup header to have no blank row above it, so that the transcript does not start with wasted space.
7. As a TUI user, I want no blank row between the last transcript block and the dock, so that the input box stays as close to the conversation as possible.
8. As a TUI user, I want exactly one blank row between two transcript blocks (never two, never zero), so that spacing is predictable as content streams in.
9. As a TUI user, I want a blank row between two paragraphs of an assistant answer, so that multi-paragraph answers keep their shape.
10. As a TUI user, I want a blank row after every heading, so that the section body does not stick to its title.
11. As a TUI user, I want a heading that directly follows a paragraph (no blank line in the source) to still be separated by one blank row, so that structure survives tight markdown.
12. As a TUI user, I want a run of two or more blank lines in the model's markdown collapsed to a single blank row, so that output stays compact.
13. As a TUI user, I want a blank row before and after a fenced or indented code block, so that code is visually fenced off.
14. As a TUI user, I want a blank row around a horizontal rule, so that a section break reads as one.
15. As a TUI user, I want a blank row inside a blockquote to render as a `│ ` row, so that the quote border stays continuous.
16. As a TUI user, I want a blockquote that contains two paragraphs to be separated by a quoted blank row, so that the quote's own block structure is preserved.
17. As a TUI user, I want list items to use a `- ` bullet, so that rendering matches markdown as I typed it.
18. As a TUI user, I want `*` and `+` bullets normalized to `- `, so that all unordered lists look the same.
19. As a TUI user, I want ordered list items numbered `N. ` from the list's start number, so that ordering stays correct even when the source misnumbers.
20. As a TUI user, I want a checkbox (`[x]`, `[ ]`, `[X]`) rendered exactly as written, so that I can tell a task list from prose and the model's capitalization is not mangled.
21. As a TUI user, I want a checkbox to keep its own text color rather than being repainted as bullet decoration, so that what I see is the model's text.
22. As a TUI user, I want nested list items indented four spaces per level, so that I can see the tree structure my markdown expressed.
23. As a TUI user, I want a nested ordered list inside an unordered item (and vice versa) to keep both its numbering and its indentation, so that mixed trees render correctly.
24. As a TUI user, I want a wrapped list item's continuation rows aligned under the item text (not under the bullet), so that long items stay readable.
25. As a TUI user, I want a list item's second paragraph to start on its own row, so that paragraphs inside items are not concatenated.
26. As a TUI user, I want a fenced code block inside a list item indented with its item, so that the code belongs to the item visually.
27. As a TUI user, I want a loose list (blank lines between items) to keep one blank row between items and none after the last item, so that loose and tight lists are distinguishable and no trailing gap appears.
28. As a TUI user, I want text wrapping to account for the list indentation and marker width, so that no row overflows the pane.
29. As a TUI user, I want a markdown document that ends with a blank line to render no trailing blank row, so that the transcript does not carry dead space.
30. As a TUI user, I want all of the above to hold while text is streaming (the merged block is re-parsed every frame), so that spacing is stable as the answer arrives.
31. As a TUI user, I want scrolling, resizing, the scrollbar and the dock to behave exactly as before, so that this change is purely about spacing.
32. As a one-shot CLI user, I want byte-identical output, so that scripts and piping are unaffected.
33. As a developer, I want the spacing rules to live in the two existing seams (the pure markdown renderer and the pure transcript row builder), so that no new module or public API is introduced.
34. As a developer, I want the markdown renderer's module documentation to state the two-level spacing rule, so that the next reader knows why blank rows exist and who owns them.

## Implementation Decisions

- **Markdown renderer owns markdown spacing** (`crates/tui/src/markdown.rs`, `render_markdown`). It gains a gap-row helper: a row that is a plain blank line, or — inside a blockquote — a row carrying only the `│ ` quote prefix in the quote-border token. The helper is **idempotent**: if the output already ends with a gap row it pushes nothing, which is what guarantees "exactly one blank row" for every path (empty blocks, nested containers, a loose list's gap meeting the next block's gap).
- **Gap sites**: a gap row is emitted before each of heading, top-level paragraph, code block, blockquote, list and horizontal rule, whenever the output is non-empty. Headings therefore get their blank row for free, and a source blank line between two blocks produces exactly one gap row (a run of blanks collapses, because the renderer never sees a blank line as a block).
- **No gap inside a list item**: within an item, consecutive blocks (a second paragraph, a nested list, a code block) are adjacent rows, each carrying the item's prefix. This reproduces pi's `renderList`, which passes no next-token type to its item tokens so that item paragraphs produce no trailing blank row.
- **Loose lists add their own gap**: after an item whose content is paragraph-wrapped (the CommonMark signal of a loose list), one gap row is emitted. Because the helper is idempotent and the document's trailing gap is stripped, this yields "one blank row between items, none after the last".
- **Trailing gap**: `render_markdown` pops a trailing gap row before returning, so the caller never receives a document that ends in a blank row.
- **List state becomes a stack**: a frame per open list holding ordered-ness, the next number and the nesting depth, plus a current-item frame holding the marker (`- ` / `N. `), the item's indent and the continuation prefix. Depth drives the indentation; the item marker drives both the first row's prefix and the continuation rows' alignment.
- **List geometry (pi's `renderList`)**: indent = four spaces per nesting level; first row prefix = indent + marker; continuation prefix = indent + spaces as wide as the marker; nested lists render at depth + 1 with their own indent. Wrapped width subtracts the quote prefix, the indent and the marker width, so indented items wrap narrower instead of overflowing.
- **Paragraph flushing inside items**: a paragraph that starts inside an open item flushes the item's accumulated content as its own row block instead of merging into it. This is what gives a second paragraph its own row and fixes the concatenation defect.
- **Bullet glyph**: the bullet constant becomes `- `. Ordered markers keep the current `N. ` renumbering and the explicit-start behavior.
- **Checkboxes stay literal text**: `Options::ENABLE_TASKLISTS` remains off, so `[x]` / `[ ]` / `[X]` is ordinary item text rendered in the body color. This is the deliberate choice over pi's synthesized, bullet-colored (and `[x]`-normalizing) task marker: the user's requirement is that the checkbox appears exactly as written.
- **Transcript row builder owns block spacing** (`crates/tui/src/app.rs`, `all_rows`): the "spacer before user prompt or tool block" condition becomes "spacer before every entry while the row buffer is non-empty" — the startup header is first, so it stays flush at the top; nothing is appended after the last entry. The spacer stays a plain unstyled blank row (pi's `Spacer(1)`), and no de-duplication is applied at this level, so the box-style blocks (user prompt, tool block) keep their own internal background padding row plus the true blank row above them.
- **Unchanged interfaces**: `render_markdown(text: &str, width: usize) -> Vec<Line<'static>>` keeps its signature, the markdown module stays the only markdown seam, and the transcript's public surface (`App`, `RenderItem`, `Entry`) is untouched. No new module, no new public item, no new dependency.
- **pi reference** (pi v0.84.4 sources): block spacing and the `space`-token rule in `pi-tui/dist/components/markdown.js` (heading/paragraph/code/blockquote/hr trailing blank unless the next token is `space`; `space` renders as an empty line); list geometry in the same file's `renderList` (four-space indent per depth, `firstPrefix` / `continuationPrefix`, blank row after a non-last item when the list is loose); transcript-level blank rows in `pi-coding-agent/dist/modes/interactive/interactive-mode.js` and the message components (`Spacer(1)` before a user message, a notice, an error, and each tool execution; a leading `Spacer(1)` inside assistant content).
- **Documentation**: the `markdown.rs` module header is rewritten — it currently claims that inter-block spacing belongs to the caller, which stops being true and would mislead the next reader; the rule becomes "the renderer owns spacing inside a markdown document (one gap row between blocks), the transcript builder owns spacing between transcript blocks". `docs/development.md`'s TUI section gains the same two-level rule plus the list geometry, and `CONTEXT.md`'s **Entry** term gains a clarifying clause that an Entry is not a markdown block (the word "block" is used at both levels, and the spec pins the meaning). `docs/user-manual.md` wording is synced only if it describes the affected display. No ADR: the rule is easy to reverse, unsurprising without context, and settles no genuine trade-off among alternatives.
- **No ADR-0006 change**: the ADR's Decisions body stays as written; this specification is the record of the spacing rule.

## Testing Decisions

- **Good tests** assert observed output, not mechanism: the exact sequence of rendered rows (text content, and token color where the row is styled) for a markdown document, and the exact row sequence a transcript produces. No assertions on renderer internals, state stacks or helper calls.
- **Two seams, both pre-existing; no new seam.**
  1. **Markdown leaf** — `slimcode_tui::markdown::render_markdown`, a pure `&str → Vec<Line>` function, tested by the in-file `#[cfg(test)] mod tests` that already covers headings, emphasis, inline code, code blocks, quotes, lists, links and rules. This is the highest seam for everything markdown-internal (block gaps, quote gaps, list geometry, bullets, checkboxes, wrapping, no trailing gap).
  2. **Transcript rows** — `App::all_rows` / the `TestBackend` frame buffer, driven by scripted `RenderItem`s, with the existing prior art in the TUI crate (`render_buffer`, `line_at`, `buffer_contains`, scripted key events) for the entry-level blank row. `all_rows` is already reachable from the in-file test module; any new helper for reading back row text is test-only.
- The tmux smoke test (`crates/cli/tests/tui_smoke.rs`) is **not** extended: it asserts presence of content, not adjacency, and blank-row assertions through `capture-pane` are brittle. Real-terminal spacing is verified by the frame-buffer seam.
- Cases to cover — markdown seam: heading followed by a paragraph gets one gap row; two source paragraphs get one gap row; a run of blank lines collapses to one; heading directly after a paragraph still gets its gap; code block fenced by gap rows; rule fenced by gap rows; blockquote-internal blank line is a `│ ` row (and a two-paragraph quote gets exactly one); tight list has no gaps between items; loose list has one gap row between items and none after the last; `- ` bullet; `*` normalized to `- `; ordered `N. ` numbering with an explicit start; checkbox (`[x]`, `[ ]`, `[X]`) preserved verbatim in body color; nested list indented four spaces per level; nested ordered inside unordered; a wrapped item's continuation rows aligned under the item text and within the pane width; a two-paragraph item renders two rows without concatenation and without a gap; a code block inside an item indented with its item; a document ending in blank lines renders no trailing gap row. Transcript seam: a blank row before every block after the header (user prompt, assistant, thinking, tool, notice, error); exactly one blank row (never two) between adjacent blocks; no blank row before the header and none after the last block; the header stays the first row.
- Development order is red → green per case: the existing markdown tests that assert exact row counts and indices (`mixed_document_renders_blocks_in_order`, `unordered_list_renders_bullets_with_md_list_color`, `blockquote_prefixes_every_line_in_quote_colors`, and the code-block/rule cases whose indices shift) are updated to the new expected rows as part of the red step, not left behind as green-by-accident.

## Out of Scope

- The one-shot CLI frontend: its streaming text output stays byte-identical (ADR-0006 consequences); markdown spacing is a TUI-only concern.
- Editing the ADR-0006 Decisions body; a new ADR (rejected above).
- pi's exact source-gap algorithm: pi compares the byte gap between block tokens to decide whether a source blank line already provides the separation, which is why pi does not insert a blank row between a paragraph and a following list. slimcode uses the one-rule-per-block-pair simplification, so a paragraph followed directly by a list gains one blank row that pi would not draw. Documented deviation, deliberately taken (see Further Notes).
- Tables, LaTeX blocks, footnotes, strikethrough-only constructs, HTML blocks and images: still outside the aligned subset.
- Synthesizing or recoloring checkboxes (pi's `[x] ` task marker): `ENABLE_TASKLISTS` stays off.
- Preserving ordered-list source markers (`1)`) — pi only does that for user messages.
- Nested blockquote depth (still a single `│ ` prefix for any depth), syntax highlighting, and pi's `trimPartialClosingFences` streaming heuristic.
- Any change to scrolling, the dock, the footer, the spinner, tool block internals, or the theme.

## Further Notes

- **pi facts used** (verified by reading the installed pi sources, not from memory): `pi-tui/dist/components/markdown.js` — heading/paragraph/code/blockquote/hr each append `""` unless the next token is `space` or (for a paragraph) a `list`; `case "space"` renders `""`; `renderList` uses `const indent = "    ".repeat(depth)`, `firstPrefix = indent + listBullet(marker)`, `continuationPrefix = indent + " ".repeat(visibleWidth(marker))`, `itemWidth = width - visibleWidth(firstPrefix)`, recurses into nested lists at `depth + 1`, and appends `""` after every non-last item when `token.loose`. `pi-coding-agent/dist/modes/interactive/components/tool-execution.js` and `interactive-mode.js` place `Spacer(1)` before tool executions, notices and errors; `assistant-message.js` puts a leading `Spacer(1)` inside assistant content and `Spacer(1)` after a thinking run that has visible content after it.
- **Observed slimcode behaviour before this change** (probe run against `render_markdown`, since deleted): `- [x] done` → `• [x] done`; `- a` / `  - b` / `    - c` → three rows at the same column; `* a` → `• a`; `1) one` → `1. one`; `- a` / blank / `- b` → two adjacent rows; `- a1` / blank / `  a2` → `• a1a2` (paragraphs concatenated); `> a` / `>` / `> b` → two adjacent `│ ` rows; heading followed by a paragraph → two adjacent rows.
- **Drive-by fix**: the concatenation of a multi-paragraph list item is a pre-existing defect that the new in-item paragraph flushing removes; it is covered by a new test rather than a separate ticket.
- **Deliberate deviations from pi, to keep the record straight**: bullet glyph now matches pi (`- `); the paragraph→list no-gap nuance is not reproduced; checkboxes are preserved verbatim instead of normalized and recolored; ordered markers are renumbered as pi does for assistant messages, not preserved as `1)`.
- The effort lives in `.scratch/tui-block-spacing/`; the pi-alignment effort (`.scratch/tui-pi-alignment/`) and the v1 TUI effort (`.scratch/tui/`) stay as history.
