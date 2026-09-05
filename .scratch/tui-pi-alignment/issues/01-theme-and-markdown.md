# 01: Theme token layer + markdown rendering

**What to build:** The semantic `Theme` token layer (names/hex mirroring pi's `dark.json`: accent, border, borderAccent, muted, dim, userMessageBg, toolPendingBg/SuccessBg/ErrorBg, selectedBg, markdown tokens, thinking/text tokens) that resolves tokens to ratatui styles, and the markdown renderer that maps `pulldown-cmark` events onto those tokens for user/assistant message text (headings, bold/italic, inline code, fenced code blocks, blockquotes, lists, rules, links). Tool output stays plain gray.

**Blocked by:** —

**Status:** resolved

- [x] Add `pulldown-cmark` to the TUI crate only; keep the workspace dependency tree otherwise unchanged.
- [x] `Theme` exposes fg/bg style helpers for every token the alignment uses; token → hex mapping is exact against pi `dark.json` (dark theme only).
- [x] Markdown renderer produces styled `Line`/`Span` vectors from markdown text, wrapping to a content width; every construct maps to its pi token (heading `mdHeading`, link `mdLink`, code `mdCode`, codeBlock `mdCodeBlock` + gray fence border, quote gray, hr gray, list bullet `accent`, bold/italic modifiers).
- [x] Both modules are pure functions with unit tests: token resolution, per-construct span mapping, wrapping, and a mixed-document smoke case.

## Notes

- Spec: Implementation Decisions "Semantic Theme layer" + "Markdown rendering"; CONTEXT.md `Theme` term; ADR-0006 D1/D3.
- Streaming re-parses the merged block text per frame (render-time only), so no incremental parsing state.