# 03: tmux smoke suite updated for the new input box, status-in-border, and popup-above-input

**What to build:** Update the real-terminal tmux smoke suite (`crates/cli/tests/tui_smoke.rs`) so it verifies the re-aligned layout and stays green: the spinner is no longer the first character of its line (it follows `── ` on the input top border), the completion popup still opens with `→ `, and the dock anchoring still holds after resize.

**Blocked by:** 01 — borderless input box + runner status embedded in the top border, 02 — completion popup style/position

**Status:** resolved

- [ ] `tmux_smoke_renders_pi_style_ui` step 5 (spinner animation): the frame-extraction must no longer assume the spinner is `line.trim_start().chars().next()` — the working row is now the input box's top border (`── ⠋ Working... ────`), so capture the frame from the `Working` line at the known offset (after `── `). Keep the "frames animate across ≥2 distinct chars" assertion.
- [ ] Step 7 (`/` completion popup): `→ ` assertion still holds (the popup now renders above the input box, still visible in the capture pane); optionally tighten to check the popup rows appear above the input top border row.
- [ ] Add an assertion (running state) that the row containing `Working` is the input box's top border row — i.e. the same row that becomes the plain `─` border when idle — so the status-in-border behavior is verified end-to-end.
- [ ] Step 9 (resize back up, dock fixed): keep the dock-anchoring assertion; confirm the footer still occupies the bottom two rows and the input box remains at a fixed position with the popup open/closed.
- [ ] `cargo test` passes locally (smoke auto-skips without tmux; run with tmux present to actually exercise it).

- [ ] The full workspace test suite passes; clippy is clean.

## Notes

- The smoke suite auto-skips when `tmux` is absent (CI), but the layout assertions must stay meaningful when tmux is available.
- The escape-cancel smoke (`tmux_escape_cancels_a_running_turn`) waits for `Working` — verify it still matches the border-embedded status (it should, since the text is unchanged, just relocated).

## Answer

Implemented and green (Ticket 03):
- Step 5 of `tmux_smoke_renders_pi_style_ui` now reads the spinner frame from the embedded status line (first char that is neither `─` nor a space), and asserts the status line is the input top border (`── ⠋ Working...`). The old `line.trim_start().chars().next()` (line-first char) was the only failing assertion after tickets 01–02.
- Both `tui_smoke` tests pass in real tmux (2 passed). Full workspace: clippy 0 warnings, all suites green.
