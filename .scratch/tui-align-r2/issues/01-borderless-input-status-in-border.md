# 01: borderless input box (top/bottom only) + runner status embedded in the top border

**What to build:** The input box loses its left/right vertical border lines and corners and renders only full-width top/bottom `─` lines (matching pi's editor), and the runner status (`⠋ Working...`) moves from its own status row **into the input box's top border**, left-aligned (pi's `embedWorkingStatus` behavior, commit `1d9787c11`).

**Blocked by:** None (can start immediately)

**Status:** resolved

- [ ] `render_input` stops using `Block::bordered()` and renders the box manually: top border row = `─`.repeat(width), content rows (1-space left inset, right-padded to the pane width), bottom border row = `─`.repeat(width) — no corners (`┌┐└┘`), no side cells (`│`).
- [ ] While a turn runs, the top border row becomes `── ` + `<spinner> Working...` + ` ` + `─`-fill, all styled with the running border color (`Token::BorderAccent`); idle it is a plain `─` line in `Token::Border`. The message text stays `WORKING_MESSAGE` ("Working...") and the braille spinner keeps animating via `App::tick`.
- [ ] The cursor placement math stays correct: content keeps its 1-space left inset so `x = area.x + 1 + display_width(prefix)` is unchanged.
- [ ] The separate status row is removed: `STATUS_HEIGHT` constant, `render_status_indicator`, and the `status_area` layout region are deleted; `App::draw`'s dock becomes `[transcript(Min0) | input(Length) | footer(Length 2)]` for this ticket (the popup region lands in ticket 02).
- [ ] Frame-buffer tests (`TestBackend`): top/bottom input rows are full-width `─` with no `┌┐└┘│` anywhere in them; content starts at column 1; while running the top border row contains `⠋ Working...` on the same row as the border; `tick()` advances the frame; idle shows a plain `─` row with no `Working`; the old status row's row index is now transcript space (update `status_indicator_only_while_running_and_animates` and `editor_border_color_reflects_running_state` accordingly).

- [ ] The full workspace test suite passes; clippy is clean.

## Notes

- Domain terms per `CONTEXT.md`: `Status indicator`, `Dock`, `Editor`. The `Status indicator` glossary entry will be rewritten in ticket 04.
- Pi reference: `packages/coding-agent/src/modes/interactive/components/custom-editor.ts` (`renderTopBorder` with `embedWorkingStatus`), `packages/tui/src/components/editor.ts` (`renderTopBorder`/`renderBottomBorder`, borderless content rows).

## Answer

Implemented and green (Ticket 01):
- `render_input` now renders the box manually: full-width top/bottom `─` lines, no corners/side lines; content keeps a 1-space left inset so the cursor math (`x = area.x + 1 + …`) is unchanged.
- While running the top border embeds the runner status: `── ⠋ Working... ────` in `Token::BorderAccent` (pi `embedWorkingStatus`); idle is a plain `─` line in `Token::Border`.
- Removed `STATUS_HEIGHT`, `render_status_indicator`, the `status_area` region, and its layout slot; `input_box_height` no longer reserves a status row.
- Tests updated: `runner_status_embedded_in_input_top_border_and_animates` (replaces `status_indicator_only_while_running_and_animates`), `editor_border_color_reflects_running_state` now asserts the status text sits on the border row. 128 lib tests green; fmt clean; clippy 0 warnings.
- Smoke suite now fails only at the ticket-03 spinner-frame assertion (captures `─` instead of frames) — deferred to ticket 03.
