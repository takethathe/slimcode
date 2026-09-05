# TUI second-round pi alignment: borderless input box, runner status in the top border, completion popup above the input box

Status: ready-for-agent

## Problem Statement

slimcode's TUI was aligned with pi in ADR-0006 (tickets 01-07, commit `8c295c7`). Since then, pi's own TUI has moved on: the latest pi (commit `1d9787c11` "prettier Working... spinner", pi v0.85) embeds the runner status **in the editor's top border** instead of on its own row, and the completion popup renders as bare SelectList lines (no bordered box). Comparing the current slimcode TUI against current pi reveals three visible differences plus one deliberate UX improvement the user wants:

1. **Input box style** — slimcode renders the input box with `Block::bordered()` (left/right vertical lines + corners); pi's editor renders only full-width top/bottom `─` lines, no side lines, no corners.
2. **Runner status (pin status) location** — slimcode shows `⠋ Working...` on its own status row above the editor (collapsed when idle); pi's latest shows it **inside the editor's top border, left-aligned** (`── ⠋ Working ────────`, via `embedWorkingStatus` + `renderTopBorder` override in `CustomEditor`).
3. **Completion popup color scheme/style** — slimcode wraps the popup in `Block::bordered().title(" completion ")`; pi renders the SelectList as bare lines below the editor (no border, no title), with the same tokens (selected `→ ` + accent, description muted, scroll info muted).
4. **Completion popup position (slimcode-specific preference)** — the user wants slimcode's popup **above the input box**, so the input box + footer stay anchored at the bottom and the popup appearance does not shift the input box (better UX than pi, which grows the editor downward).

## Solution

Re-align slimcode's TUI with current pi and apply the user's positioning preference:

- **Input box** becomes borderless on the sides: a full-width top `─` line, content rows with 1-space left padding, and a full-width bottom `─` line — matching pi's editor (no corners, no vertical lines). The border keeps its semantic color (blue at rest, `borderAccent` cyan while a turn runs).
- **Runner status** moves **into the input box's top border**, pi-style: while a turn runs the top border renders `── ⠋ Working... ──────` (all in the running border color, `borderAccent`), with the braille spinner animating at 80ms; when idle it is a plain `─` line. The separate status row (`STATUS_HEIGHT`, `render_status_indicator`) is removed.
- **Completion popup** loses the bordered box and ` completion ` title and renders as pi-style bare SelectList lines (selected `→ ` + accent name, description muted, overflow `(i/n)` muted). It is placed **above the input box** (between transcript and input) so the input box + footer stay fixed; the popup eats into the transcript region when open.

## User Stories

1. As a TUI user, I want the input box to have only top/bottom horizontal lines (no left/right verticals, no corners), so it matches pi's editor and looks cleaner.
2. As a TUI user, I want the runner status (`⠋ Working...`) shown on the same line as the input box's top border, left-aligned, so I can see at a glance that a turn is running without a separate row appearing/disappearing.
3. As a TUI user, I want the `/` completion popup to use pi's SelectList colors (selected `→` accent, description muted) without a surrounding box, so it matches pi's popup.
4. As a TUI user, I want the `/` completion popup to open **above** the input box, so the input box and footer never move when the popup appears or disappears (input position stays stable while typing).

## Implementation Decisions

- **Input box rendering** (`render_input`): replace `Block::bordered()` with manual line rendering — top border row, content rows (1-space left padding, right-padded to width), bottom border row — so top/bottom lines are exactly `─`.repeat(width) with no corners. The cursor placement math (`x = area.x + 1 + …`) stays valid because content keeps its 1-space left inset.
- **Runner status in the top border**: while `running`, the top border line is `── ` + `<spinner> <Working...>` + ` ` + `─`-fill, all styled with the running border color (`borderAccent`), mirroring pi's `CustomEditor::renderTopBorder` with `embedWorkingStatus`. The message stays pi's `Working...` (kept as `WORKING_MESSAGE`); the braille frames keep animating via `App::tick`. When idle, the top border is a plain `─` line in the rest border color (`border`).
- **Dock layout** (`App::draw`): the five-region dock becomes a four-region dock `[transcript(Min0) | popup(Length 0|n) | input(Length) | footer(Length 2)]`. The status region is deleted. The input box and footer are anchored; the popup (0 rows when closed) sits directly above the input box and shrinks the transcript when open.
- **Completion popup rendering** (`render_completion`): drop `Block::bordered().title(" completion ")` and `ListState`/`List`; render the visible candidates + optional `(i/n)` overflow row as plain styled `Line`s (pi SelectList tokens: selected `→ ` + accent, non-selected default, description muted, scroll info muted). The scroll window (`offset`/`selected`) logic is unchanged.
- **Constants/helpers**: `STATUS_HEIGHT` and `render_status_indicator` are removed; `STATUS_HEIGHT`-based layout math in tests is updated. `render_input` gains whatever it needs to render the status line (the current spinner char + `WORKING_MESSAGE` are already available on `App`).
- **Kept from ADR-0006**: the two-line footer, transcript blocks, markdown, scrollbar, worker-thread runner, Esc cancellation, terminal title, theme tokens. No pi feature beyond the three alignments + the popup-position preference is in scope.
- The border color while running stays `borderAccent` (cyan), matching pi's `getThinkingBorderColor` behavior conceptually (pi recolors the border + status together per thinking level; slimcode has one running state).

## Testing Decisions

- Pure `App` frame-buffer tests (ratatui `TestBackend`) are the primary seam, as before:
  - Input box: top/bottom rows are full-width `─` (no `┌`/`┐`/`└`/`┘` corners, no `│` side cells); content begins at column 1; cursor maps correctly.
  - Runner status: while running, the input top-border row contains `⠋ Working...` on the same row; spinner frame advances on `tick()`; idle shows a plain `─` line with no `Working` text; the separate status row no longer exists (its former row is transcript space).
  - Completion popup: renders above the input box (row order: popup rows then input top border); no ` completion ` title and no box border; selected `→ ` accent, description muted, `(i/n)` muted overflow row; input box row position does not change between popup-open and popup-closed.
- The tmux smoke suite (`crates/cli/tests/tui_smoke.rs`) is updated: the spinner-frames assertion no longer expects the spinner as the first non-space char of a line (it now follows `── ` on the input top border); the `→ ` popup assertion still holds; add/keep an assertion that the input top border carries `Working` while running.
- Existing tests that reference `STATUS_HEIGHT`, the ` completion ` title, the bordered popup, or the status row row-index math are updated to the new layout.
- `cargo fmt --all` clean; `cargo clippy --all-targets --all-features --message-format=json -- -D warnings` zero errors/warnings before commit.

## Out of Scope

- No other pi features: no file-path autocomplete, no thinking-level border colors, no overlays/selectors, no mouse, no light theme.
- The popup stays above the input box (the user's deliberate slimcode preference) even though pi renders its popup below the editor; this is the one intentional non-alignment.
- No changes to the completion matching/assembly (`slimcode-commands` / `slimcode-common::skills`) — only the popup's rendering position and chrome change.

## Further Notes

- Docs to sync before commit (AGENTS.md rule 3): `docs/user-manual.md` (status indicator section, completion popup section, editor border section, key table where needed), `docs/development.md` (layout description `[transcript | status | input | popup | footer]` → `[transcript | popup | input | footer]`, status-in-border, popup-above-input), `docs/explanation.md` if it describes the dock/status, `CONTEXT.md` glossary (`Completion popup`, `Dock`, `Status indicator`), `docs/index.md` if needed.
- New ADR-0007 records the second-round alignment decisions (borderless input, runner status in top border, popup above input, borderless popup) and supersedes the ADR-0006 D4/D5 wording for the dock/status/popup.
- Git commit conventions per AGENTS.md: `feat(tui): …` / `docs(tui): …` / `test(tui): …`, local commits only.
