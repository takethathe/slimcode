# Ticket 05 — Completion popup top separator line

## What to build

The borderless completion popup sits directly above the input box and its top row abuts the
transcript with **no visual gap**. Give the popup a full-width top border line
(`─` × width) in the semantic border color (`Token::Border` blue), so there is a clear
interval between the transcript and the popup. The popup area grows by 1 row when open (the
transcript gives up that row); the input box and footer stay anchored.

## Files

- `crates/tui/src/app.rs`:
  - `App::draw` — `popup_height` gains `+1` for the separator line.
  - `render_completion` — prepend a full-width `─` line in `Token::Border`.
- Tests: new `completion_popup_has_top_separator_line`; existing popup tests locate rows
  relatively (row_containing) so they keep passing, but are re-verified.
- `crates/cli/tests/tui_smoke.rs` — unchanged (only asserts `→ ` and Esc).

## Acceptance

- [ ] The row directly above the first candidate row is a full-width `─` line in the
      border color.
- [ ] Popup still renders above the input; input box position still stable open vs closed.
- [ ] `cargo test` green (incl. both tmux smoke tests), clippy 0 warnings, `cargo fmt --all`.
- [ ] Docs updated: ADR-0007 D3/D4 (divergence note), `user-manual.md`, `development.md`.

## Notes

Slimcode divergence from pi (whose borderless SelectList sits *below* the editor and needs
no top rule): because slimcode places the popup above the input, a top rule keeps the popup
visually distinct from the transcript above it.
