# TUI second-round alignment with pi: borderless editor, embedded status, popup above input

ADR-0006 aligned slimcode's TUI display to pi's interaction model. This ADR records the
**second round** of alignment, driven by a newer pi (v0.85, commit `1d9787c11`, 2026-09-03
`feat(tui): prettier Working... spinner (#8799)`): pi now embeds the runner status in the
editor's top border (`embedWorkingStatus`) and renders its completion SelectList borderless.
slimcode adopts both, plus one deliberate divergence — the completion popup sits **above**
the input box (below the editor in pi) so the input box's position stays stable while the
popup opens and closes.

## Decisions

### D1 — Borderless editor box (pi editor top/bottom borders)

The input box is no longer `Block::bordered()` (four sides + `┌┐└┘` corners). It is drawn
manually as a **full-width top `─` line and a full-width bottom `─` line, with no corner
and no vertical side characters**, matching pi's `renderTopBorder` / `renderBottomBorder`
(`─` × width). Content keeps a 1-space left inset (so the cursor math
`x = area.x + 1 + display_width(prefix)` is unchanged) and the content rows are
right-padded to the box width.

### D2 — Runner status embedded in the editor's top border (pi `embedWorkingStatus`)

The separate status-indicator row above the editor is gone. While a turn runs, the input
box's top border embeds the status, left-aligned:

```
── ⠋ Working... ─────────────────────────────────
```

The whole status + border row uses the running border color (`borderAccent` cyan); idle it
is a plain `─` line in `border` blue. This matches pi's `embedWorkingStatus`, which colors
the embedded spinner and message with the editor's border color. Consequently
`STATUS_HEIGHT`, `render_status_indicator`, and the `status_area` layout slot are removed;
`input_box_height` no longer reserves a status row.

### D3 — Borderless completion SelectList with a top separator (pi `SelectList`)

The `/` completion popup drops `Block::bordered().title(" completion ")` and renders bare
SelectList rows, exactly like pi's borderless SelectList: the selected row is `→ ` prefix +
name in `accent` (no background inversion, no bold), non-selected rows default, descriptions
are `muted`, and the overflow row is `(i/n)` in `muted`. `popup_height` no longer adds the
`+2` border rows. **Divergence from pi**: because slimcode places the popup *above* the input
box (D4), its top row would abut the transcript with no visual gap, so the popup is preceded
by a full-width `─` separator line in the semantic `border` color (matching the editor's
border line).

### D4 — Dock order: popup above the editor (deliberate divergence from pi)

The dock order changes from `[transcript | status | editor | popup | footer]` (ADR-0006 D4)
to `[transcript(Min 0) | popup(0|n) | editor | footer]`. This is a deliberate slimcode
preference over pi (which attaches the popup *below* the editor): because the popup is
rendered directly above the input box, opening/closing the popup **eats into the
transcript** and never shifts the input box or footer, so the user's typed input never jumps.
Verified by `completion_popup_does_not_shift_the_input_box` (the input top-border row is
byte-identical open vs closed). The popup region includes its top separator line (D3), which
is why it takes one extra row when open.

## Consequences

- **Removed from the TUI display**: the status-indicator row and its `Working...` spinner
  line (now on the editor's top border), the bordered completion box and its
  ` completion ` title.
- **Editor**: borderless sides (only full-width top/bottom `─` lines), same semantic border
  color (blue `border` idle, cyan `borderAccent` running) with the status embedded while
  running.
- **Completion popup**: renders above the input box, preceded by a full-width `─` top
  separator line (border color) so it stays visually distinct from the transcript; the input
  box + footer are position-stable.
- **Tests updated**: `runner_status_embedded_in_input_top_border_and_animates`,
  `completion_popup_renders_above_input_with_selection`,
  `completion_popup_does_not_shift_the_input_box`, `completion_popup_has_top_separator_line`;
  the tmux smoke suite reads the spinner
  frame from the embedded border status line (`── ⠋ Working...`) rather than as a
  line-first char.
- **Docs**: this ADR supersedes the layout/status/popup portions of ADR-0006 D4/D5 and
  ADR-0005's popup form; `CONTEXT.md`, `development.md`, `user-manual.md`, and `index.md`
  are updated to match.
- **pi divergence (deliberate, kept)**: the completion popup is above the input box, not
  below the editor, so input position is stable.
