# 02: Transcript block gap (blank row before every Entry)

**What to build:** In the TUI transcript, only a user prompt and a tool block are preceded
by a blank row today; assistant text, thinking blocks, notices and errors butt directly
against whatever precedes them. After this ticket, exactly one blank row separates every
transcript block from the one above it: user prompts, assistant text, thinking blocks, tool
blocks, notices (dim) and errors (red). The startup header stays flush at the top of the
transcript (nothing precedes it), and no blank row is appended after the last block before
the dock, so the input box stays as close to the conversation as possible.

**Blocked by:** — (independent; touches the transcript row builder only, not the markdown
renderer)

**Status:** resolved

- [x] The transcript row builder inserts exactly one blank row before every entry after the
      first, replacing today's "spacer only before a user prompt or a tool block"
      condition. The header is the first entry, so it keeps no leading blank row.
- [x] No blank row is appended after the last entry (the dock follows the transcript
      directly), and no blank row appears between two entries when the transcript is empty
      or holds a single entry.
- [x] The separating row stays a plain unstyled blank row (pi's `Spacer(1)`), and no
      de-duplication is applied against an entry's own trailing padding row: the box-style
      blocks (user prompt, tool block) keep their internal background padding row *plus*
      the true blank row above them.
- [x] The scroll/scrollbar math stays consistent with the extra rows (the same row builder
      feeds both the viewport window and the scrollbar thumb; total row count changes, the
      scrollbar does not).
- [x] Frame-buffer tests at the transcript seam (`App::all_rows` / ratatui `TestBackend`
      with scripted `RenderItem`s, the existing `render_buffer` / `line_at` /
      `buffer_contains` prior art) cover: a blank row before each block kind (user prompt,
      assistant, thinking, tool, notice, error); exactly one blank row between adjacent
      blocks (never two); no blank row before the header; no blank row after the last
      block; the header remains the first row.
- [x] `CONTEXT.md`'s **Entry** term gains a clarifying clause that an Entry is not a
      markdown block, so "block" is no longer overloaded between the transcript model and
      markdown rendering.
- [x] `docs/development.md`'s TUI section states the transcript-side half of the rule (one
      blank row before every transcript block after the header; none after the last one;
      plain unstyled spacer) and states the two-level rule as a whole.
- [x] `cargo fmt --all` applied, `cargo clippy --all-targets --all-features -D warnings`
      clean, `cargo test` green (tmux smoke unaffected).

## Notes

- Spec: `.scratch/tui-block-spacing/spec.md` — Solution ("Between transcript blocks"),
  Implementation Decisions ("Transcript row builder owns block spacing"), Testing
  Decisions seam 2.
- Ground truth (pi v0.84.4): `pi-coding-agent/dist/modes/interactive/interactive-mode.js`
  and the message components place `Spacer(1)` before a user message, a notice, an error,
  and each tool execution; `assistant-message.js` puts a leading `Spacer(1)` inside
  assistant content. There is no spacer after the last block before the dock.
- Pairing with ticket 01: the two tickets own the two halves of the same user-visible rule
  and touch different modules, so they may land in either order. Their
  `docs/development.md` edits are in different paragraphs of the TUI section.
