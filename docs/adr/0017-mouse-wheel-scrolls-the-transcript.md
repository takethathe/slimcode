# The mouse wheel scrolls the TUI transcript, and buys that with native selection

In the TUI the wheel did the one thing a reader never wants: it recalled input history. The
app holds the alternate screen and never subscribed to mouse events, so the terminal applied
**DECSET 1007** (alternate scroll) and translated every notch into repeated `↑`/`↓` keys —
which the reducer answers with history recall at the empty input. One flick of the wheel
overwrote the input box with old prompts while the view jittered, and the transcript itself
remained reachable only through `PgUp`/`PgDn`. Reusing the keyboard semantic for a scrolling
gesture was the mistake, not the terminal's translation of it.

The fix has to decide what the app subscribes to, what the wheel does, and what it costs.

## Decisions

### D1 — The wheel is a view-only scroll gesture, in the `PgUp`/`PgDn` family

`App::handle_mouse` is the mouse sibling of `App::handle_key`: it takes a mouse event and
returns nothing, because the wheel produces no `Effect` — it can never submit a turn, run a
command, change a session or cancel a run. It recognises only `ScrollUp`/`ScrollDown`, and
each notch delegates to the *existing* scroll semantics (`scroll_up`/`scroll_down`) by one
notch of `WHEEL_LINES` (3 rows): scrolling up stops following, returning to the bottom
re-follows, and the scrollbar fade counter restarts exactly as it does for a page key. No new
scroll model, and no change to `↑`/`↓` history recall or to `PgUp`/`PgDn` paging (their extreme bound is
corrected in D5) — the wheel is additive.

### D2 — Subscribe to the wheel only: `?1000h` + `?1006h`

The CLI enables DECSET `?1000` (button press/release, which is how a wheel notch arrives) and
`?1006` (SGR coordinates), and sends the exact inverse on the way out. Crossterm's
all-in-one `EnableMouseCapture` is deliberately **not** used: it also sends `?1002`/`?1003`/
`?1015` and thereby subscribes to *mouse motion*, which the loop has no semantics for and
which would flood it with move events. `XTSHIFTESCAPE` is not sent either, so Shift stays
with the terminal and Shift+drag keeps the native selection.

### D3 — The accepted cost: drag-selection degrades to Shift+drag

Once the app captures the mouse, a plain drag reaches the app instead of the terminal, so a
plain drag can no longer start a native selection. This ADR accepts that trade for the wheel,
and the escape hatches are the terminal's, not ours: Ghostty keeps `mouse-shift-capture`
at its default `false`, so **Shift+drag** is still a native selection, and its
`mouse-reporting = false` / `toggle_mouse_reporting` key turn app mouse capture off entirely
for a session. tmux needs `set -g mouse on`, otherwise tmux consumes the wheel and no event
ever reaches the app. All of this is user-manual material, not code.

### D4 — The wheel has no region routing, and outranks the completion popup

Pointer coordinates, modifiers, clicks, press/release, drags, motion and the horizontal wheel
are ignored outright — the wheel behaves the same with the pointer over the transcript, the
input box or the footer. There is no "which region is under the pointer?" dispatch, so there
is no way for a stray click to move the cursor, expand a tool block or pick a candidate.

The wheel also scrolls the transcript while the `/` completion popup is open, instead of
copying `PgUp`/`PgDn`'s popup-first branch (which pages the candidate list). The popup already
has `↑`/`↓`/`PgUp`/`PgDn` for navigation; the wheel is a whole-screen reading gesture, and a
pointer-independent one cannot be "inside" the popup. This is a deliberate difference in scope
priority between the wheel and the page keys, pinned by a test.

### D5 — Scroll gestures stop at a full pane, and the wheel leaves a short transcript alone

The scroll offset is now capped at `max_scroll(total_rows, pane_height)` — "the pane stays full of
transcript" — instead of the old "keep one row visible" bound, so a scroll gesture can never pull
blank rows into view. The page keys keep their paging semantics (10 lines per press, stop following,
re-follow at the bottom, popup-first while the completion popup is open); only the extreme bound
they settle on is corrected. With the corrected bound they also stop hiding the bottom rows of a
transcript shorter than the pane — the same short-transcript oddity the wheel is required not to
have.

The wheel additionally does nothing at all when the whole transcript already fits: neither the
scroll position, nor the visible window, nor the scrollbar changes. That rule lives in `scroll_up`
itself — the shared scroll semantic, so the page keys get it too — and it also skips the
"stop following" part, so a short transcript can never silently leave follow mode. Both properties
are pinned by tests.

### D6 — A parked view stays parked while output streams

The wheel is worth little mid-run if the very next delta snaps the reader back to the bottom, and
the tickets require scrolling up to "stop following new output". The existing `App::apply`
re-anchored the view at the bottom on *every* item, so a scroll gesture held only until the next
token. `apply` now re-anchors only while the view is following; while the user is parked (scrolled
up) it grows the scroll offset by the rows the item appended, so the window keeps showing the same
content. When the turn streams into the last block, those appended rows are exactly the ones below
the window, so the window genuinely holds still — and scrolling down still reaches the newest row
and re-follows. A transcript replacement (`/new`, `/load`) still resets the view outright, and
`scroll_up` only clears following when the offset actually moved, so `follow == false` keeps meaning
"the user parked the view somewhere".

### D7 — No config item, no acceleration modifier, no pi-style mouse furniture

`WHEEL_LINES` is a constant beside `PAGE_LINES`. If the feel is wrong on real hardware (a
terminal's scroll multiplier can deliver several events per notch), the single constant is
tuned — not a new setting, and not an `Alt+wheel` acceleration path. The reference
implementation's other mouse features (drag selection and clipboard copy, double-click word
select, link clicks, "jump to latest", transcript search, scrollbar dragging, click-to-place
cursor) are explicitly out of scope: they are why the wheel subscription is worth its cost,
not part of it.

### D8 — The terminal lifecycle stays in the CLI

The subscription follows ADR-0013's split: the CLI sends the enable sequence after entering the
alternate screen, and `restore_terminal()` sends the disable sequence. The panic hook already
calls that same function, so the abnormal path is symmetric with the normal one and a crash
never leaves the shell with the app's mouse state.

## Considered Options

- **`crossterm::event::EnableMouseCapture`** — rejected: it subscribes to mouse motion as well
  (D2), and there is no motion semantics to justify the event flood.
- **Let the terminal scroll (main screen instead of the alternate screen)** — rejected: the
  wheel would work natively, but the app-side scroll model would go with it, taking `Ctrl+O`
  global expansion, resize reflow, `/load` transcript replay and the scrollbar along.
- **Route the wheel by the region under the pointer** (wheel over the popup pages the popup) —
  rejected: it makes the gesture depend on a pointer position the user cannot see the target of,
  and it contradicts "the wheel is a whole-screen reading gesture" (D4).
- **A configuration item for lines-per-notch, plus `Alt+wheel` acceleration** — rejected: the
  value is a single constant that real-hardware feel can settle (D7).
- **Enable mouse capture per the reference implementation's full furniture** — rejected: drag
  selection, clipboard, link clicks and scrollbar dragging are a project of their own; shipping
  the wheel alone keeps the change small and the cost (D3) explainable.

## Consequences

- Wheel scrolling works while idle and while a turn runs (both frame-loop paths forward mouse
  events), and never disturbs the run: it is not a cancel and not an input.
- A view parked by a scroll gesture holds its rows while the turn streams (D6), so mid-run reading
  is possible instead of being yanked back to the bottom on every delta.
- The shared scroll bound is now "fill the pane" (D5): the wheel and `PgUp`/`PgDn` all stop before
  scrolling blank rows into the view, and `PgUp`/`PgDn` no longer scroll a short transcript's bottom
  rows off the pane. Their paging semantics are otherwise untouched.
- Native drag-selection now needs Shift+drag (Ghostty default), and tmux needs
  `set -g mouse on`; iTerm2's fast trackpad can drop scroll increments. Recorded in the user
  manual.
- Boundary behaviour is pinned at the `App` reducer seam (short content, popup open, pointer
  position, non-wheel events), so a later "make the wheel behave like `PgUp`" refactor fails a
  test instead of regressing silently.
- The render layer is untouched: it already windows the transcript and fades the scrollbar, and
  the wheel feeds the same state.
- A future full mouse feature set (selection, clipboard) can be built on top of this
  subscription; it would revisit D3's trade rather than start from "capture everything".
