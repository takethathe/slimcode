# 02: completion popup — pi SelectList style (no box) and positioned above the input box

**What to build:** The `/` completion popup drops its `Block::bordered().title(" completion ")` chrome and renders as pi-style bare SelectList lines (selected `→ ` + accent name, description muted, overflow `(i/n)` muted), and moves from between the input box and footer to **above the input box**, so the input box + footer stay anchored and the popup appearance never shifts the input position.

**Blocked by:** 01 — borderless input box + runner status embedded in the top border (the dock layout changes together)

**Status:** resolved

- [ ] `render_completion` stops using `List`/`ListState`/`Block::bordered()` and renders the visible candidates as plain styled `Line`s into `popup_area`: selected row `→ ` prefix + name in `Token::Accent` (no bold, no bg inversion), non-selected rows in default style, description in `Token::Muted`, and the overflow `(i/n)` scroll-info row in `Token::Muted` (only when the list overflows `COMPLETION_VISIBLE`).
- [ ] `App::draw`'s dock becomes `[transcript(Min0) | popup(Length 0|n) | input(Length) | footer(Length 2)]`: the popup region sits directly above the input box and is 0 rows when closed. The input box and footer rows are computed from the bottom (input height + footer height) so the popup opening/closing only resizes the transcript above, never the input box.
- [ ] The scroll-window logic (`Completion.offset` / `selected`, `COMPLETION_VISIBLE`, `clamp_offset`, page keys) is unchanged — only the rendering and placement move.
- [ ] Frame-buffer tests (`TestBackend`): with `/` typed, the popup rows render **above** the input top border row (assert row order), with no ` completion ` title and no box border chars (`┌┐└┘│`); selected `→ ` accent, description muted, `(i/n)` muted overflow row when overflowing; the input box's top-border row index is identical with the popup open vs closed (input position stability).

- [ ] Update tests that referenced the old chrome/position: `completion_popup_renders_below_input_with_selection` (rename + no `completion` title assertion), `completion_popup_uses_select_list_tokens` (drop the ` completion ` assertion), `completion_popup_shows_scroll_info_when_overflowing` (row index per new layout).
- [ ] The full workspace test suite passes; clippy is clean.

## Notes

- This is the user's deliberate slimcode preference: the popup sits **above** the input box, whereas pi renders its autocomplete below the editor — the one intentional non-alignment (spec §Out of Scope).
- Pi reference for the popup style: `packages/tui/src/components/select-list.ts` + `getSelectListTheme()` in `packages/coding-agent/src/modes/interactive/theme/theme.ts` (selectedPrefix/selectedText = accent, description/scrollInfo/noMatch = muted).
- `CONTEXT.md` `Completion popup` entry ("shown below the TUI input box") is corrected in ticket 04.

## Answer

Implemented and green (Ticket 02):
- Dock reordered to `[transcript(Min 0) | popup(0|n) | input | footer]`: the borderless popup eats into the transcript, so the input box and footer stay anchored (`completion_popup_does_not_shift_the_input_box` proves the input top-border row is identical open vs closed).
- `render_completion` is now bare pi SelectList rows (no `Block::bordered()`, no ` completion ` title): selected `→ ` prefix + name in `Token::Accent`, descriptions `muted`, overflow row `(i/n)` muted.
- `popup_height` no longer adds `+2` for the box border (one row per candidate + overflow row).
- Tests: `completion_popup_renders_above_input_with_selection` (renamed; asserts popup row < input top border), `completion_popup_does_not_shift_the_input_box` (new), `completion_popup_uses_select_list_tokens` dropped the old title assertion, `completion_popup_shows_scroll_info_when_overflowing` still green (muted `(1/` row). 129 lib tests green; fmt + clippy 0 warnings. Removed now-unused `Text`/`Block`/`List`/`ListItem`/`ListState` imports.
