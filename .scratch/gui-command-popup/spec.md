# Spec: GUI `/` Command popup (Completion popup) + shared Prompt preprocessing

Status: ready-for-agent

> Vocabulary follows `GLOSSARY.md`. Design rationale lives in **ADR-0026** (this feature) and it
> partially supersedes **ADR-0013 D3**; the popup key/matching semantics extend **ADR-0005**.

## Problem Statement

The GUI 前端 has no `/` command support at all. Its `InputBox` only sends raw text, and the Tauri
宿主 forwards whatever it gets straight to `AgentSession::prompt`. So typing `/help`, `/new`,
`/compact`, or a `/skill:name` trigger into the GUI does nothing useful — the `/` text reaches the
model as a literal prompt. Meanwhile the TUI already gives a live **Completion popup** (fuzzy
candidate list of Commands + installed Skills), and a submitted `/skill:name` there is expanded into
the skill's `<skill>` block before the model sees it. GUI users get none of that.

## Solution

Bring the TUI's `/` experience to the GUI 前端, and factor the shared part out so both frontends use
identical logic:

- Typing `/` at the start of the GUI input opens a **Completion popup** above the box — the same
  fuzzy, best-first candidate list (Commands + installed Skills) the TUI shows, ranked by the single
  shared `complete()` source.
- `↑`/`↓` move the selection, `Tab` (and, while the popup is open, `Enter`) **Commit to buffer**,
  `Esc` closes the popup; a later `Enter` with the popup closed submits.
- A committed, recognised leading `/` token is drawn as a **Command chip** to emphasise it is a
  Command, not prompt text.
- Submitting a `/` token actually *does* something: the small set of Commands that map onto existing
  GUI actions (`/new`, `/compact`, `/exit`, `/help`) fire those actions; a `/skill:name` trigger is
  expanded through a new shared **Prompt preprocessing** seam so the model receives the `<skill>`
  block.
- **Prompt preprocessing** (`prepare_user_prompt`) is lifted into `slimcode-app` as the one public
  seam every frontend consumes, replacing the CLI's two private copies of the same logic.

## User Stories

1. As a GUI user, I want typing `/` to open a candidate popup, so that I can discover available Commands and Skills without memorising them.
2. As a GUI user, I want the popup to list Commands and installed Skills together, so that it matches what the TUI offers and behaves "like the TUI".
3. As a GUI user, I want candidates fuzzy-ranked best-first as I keep typing, so that the thing I probably meant is at the top.
4. As a GUI user, I want the popup to open only for a leading `/` token (before I type a space/newline), so that it never interrupts ordinary multi-line prompts.
5. As a GUI user, I want `↑`/`↓` to move the highlighted candidate, so that I can pick one from the keyboard.
6. As a GUI user, I want the highlighted candidate to track the best match automatically while I only type, so that I can hit accept without navigating.
7. As a GUI user, I want my manual `↑`/`↓` choice to stick (by value) until I move again or it's filtered out, so that typing an argument doesn't yank my selection away.
8. As a GUI user, I want `Tab` to write the selected spelling into the input with a trailing space and not submit, so that I can append an argument.
9. As a GUI user, I want `Enter` while the popup is open to also just Commit to buffer, so that committing and submitting are deliberately separate in the GUI.
10. As a GUI user, I want `Enter` with the popup closed to submit, so that a normal prompt still sends on Enter.
11. As a GUI user, I want `Shift+Enter` to always insert a newline, so that multi-line prompts are unaffected by the popup.
12. As a GUI user, I want `Esc` to close the popup without clearing what I typed, so that I can abandon completion and keep editing.
13. As a GUI user, I want a committed, recognised `/` token shown as a Command chip, so that I can see at a glance the leading token is a Command rather than my prose.
14. As a GUI user, I want both Commands and `/skill:name` triggers to get a chip, so that either kind of command reads as "special".
15. As a GUI user, I want the chip to be display-only, so that I can still edit the underlying text freely.
16. As a GUI user, I want submitting `/new` to start a fresh session, so that the Command does the same thing the sidebar "New" button does.
17. As a GUI user, I want submitting `/compact` to compact the current session, so that it matches the existing compact action.
18. As a GUI user, I want submitting `/exit` to close the window, so that the Command does what it says.
19. As a GUI user, I want submitting `/help` to show the command list, so that I can read what's available without leaving the input.
20. As a GUI user, I want `/history`, `/!!`, `/!N`, and `/install-skill` to NOT appear in my popup, so that I'm never offered something the GUI can't do.
21. As a GUI user, I want to type `/skill:<name>` (or `/name`) and have the skill actually run, so that the model receives the skill instructions instead of my literal slash text.
22. As a GUI user, I want a skill I already loaded earlier in the same session to be shown as an "already-loaded" notice rather than re-injected in full, so that the context stays lean — the same behaviour the TUI has.
23. As a GUI user, I want the popup positioned above the input, so that the input box doesn't jump when the popup opens/closes.
24. As a GUI user, I want each popup row to show the candidate value and its description, so that I can tell similar Commands apart.
25. As a GUI user, I want newly relevant candidates to reflect the currently installed Skills, so that the popup isn't stale against what's actually installed.
26. As a TUI user, I want the CLI one-shot and TUI skill paths to use the same Prompt preprocessing as the GUI, so that `/skill:name` expands identically in every frontend.
27. As a developer, I want the Command registry to stay the single source of truth for candidates, so that GUI and TUI can't drift apart.
28. As a developer, I want a `/skill:name` that resolves to no installed skill to fall through as normal prompt text, so that unknown triggers don't hard-error the submit path.
29. As a maintainer, I want the "which Commands a given frontend offers" decision declarative in the registry, so that adding a frontend later doesn't mean hand-maintaining allowlists.

## Implementation Decisions

### Prompt preprocessing (the new shared seam — `slimcode-app`)

- A pure function `prepare_user_prompt(input, skills, history)` in the skills module returns a small
  enum:
  ```rust
  pub enum PreparedPrompt {
      Prompt(String),                       // plain text; unrecognised leading /xxx is normalized
      Skill { name, model_text, display },  // model_text → SDK prompt; display → frontend block
  }
  ```
- It composes the already-public primitives: parse a leading skill trigger, resolve it via
  `find_skill`, render `skill_prompt(skill, arg, already_loaded)` where `already_loaded` is computed
  from `history` via `skill_loaded_in`. A miss returns `Prompt(normalize_skill_trigger(input))`.
- Command recognition is deliberately **not** in this seam — Commands become frontend effects, not a
  prompt. Only skill-trigger → model-prompt expansion lives here.
- This supersedes the "skill semantics stay on the CLI side" half of ADR-0013 D3 (see ADR-0026).

### Command registry gains frontend applicability (`slimcode-commands`)

- `Command` gains a `gui_applicable: bool`. `/history`, `/!!`, `/!N`, `/install-skill` are `false`;
  the rest are `true`.
- The candidate assembly used by the GUI (`complete()` and its `CompletionItem` values, including the
  `/skill:name` skill half) filters out Commands whose `gui_applicable` is `false`; the TUI path keeps
  the full registry. The bare-name skill scoring, sticky/tracking selection, and value-canonical rules
  from ADR-0005 are unchanged and shared.
- Rejected: a host-side allowlist (a second source of truth) and a `frontends` bitset (speculative at
  two frontends).

### GUI submit dispatch splits by layer

- **Frontend TS** intercepts the effect-mapped Command set before calling the host: `/new`→newSession,
  `/compact`→compactNow, `/exit`→close, `/help`→a locally rendered command list. These do not start a
  model turn.
- **Everything else** goes to the host `prompt` path: skill triggers and plain text. Un-mapped Commands
  that still appear (`/usage`, `/skills`) fall through as ordinary prompt text (accepted scope).
- The host `prompt` path runs `prepare_user_prompt`; for a `Skill` it sends `model_text` to
  `AgentSession::prompt`; the `display` is first surfaced as an ordinary user message in the GUI
  transcript (a dedicated `[skill]` block is deferred).

### GUI Completion popup IPC + controller

- The Tauri 宿主 embeds a Skills snapshot in the controller (mirroring the CLI's completion provider)
  and exposes a new `complete(input) -> Vec<CompletionItem>` IPC that calls the shared `complete()`
  source, filtered by `gui_applicable`. Candidates are fetched per keystroke (the pool is small and
  this keeps ranking in one place).
- The controller's `prompt` gains the `prepare_user_prompt` step described above.

### GUI frontend popup + chip (presentation only)

- The popup renders as a floating list above the input box (position-stable on open/close) with `value`
  + `description` per row; selection is pre-set to the top match and follows it on typing, becoming
  sticky after `↑`/`↓` (ADR-0005).
- While the popup is open, `↑/↓/Tab/Enter/Esc` are captured by the popup; `Shift+Enter` still inserts a
  newline; `Esc` only closes the popup.
- The **Command chip** is a display-only overlay: when the leading token is a complete, exact candidate
  value and the popup is closed, the token is highlighted; the buffer stays editable plain text.
  Recognition reuses `complete(token)` and checks for a `value == token` match — no extra IPC.

### CLI migration

- `crates/cli` (both the one-shot entry and the interactive TUI handler) is refactored to call
  `prepare_user_prompt`; the one-shot passes an empty history (reproducing its previous
  `already_loaded: false`). Behaviour is preserved; existing CLI tests guard it.

## Testing Decisions

Good tests assert external behaviour, not internals: given an input and a skills/history snapshot, the
preprocessing produces the right model text / prompt text; given a partial `/`, `complete()` returns
the right ranked, applicability-filtered candidates; a submitted Command triggers the right host
action or turn.

- **Primary seam — `prepare_user_prompt` (one new seam, all injection logic).** Direct unit tests of
  the pure function: skill hit with argument; skill hit without argument; already-loaded skill (dedup
  notice) vs fresh; non-skill plain prompt; unrecognised leading `/xxx`; empty history vs populated
  history. Prior art: unit tests in `slimcode-app`'s skills module (`skill_prompt`, `complete`,
  `combined_suggestions`).
- **`slimcode-commands`** — unit tests that `gui_applicable` filtering removes the four non-GUI
  Commands for the GUI caller while the TUI caller still sees all, and that ranking/values are
  unchanged. Prior art: `fuzzy.rs` and registry `lib.rs` tests.
- **`gui/src-tauri::AppController`** — controller-level tests using the existing `FakeProvider` +
  temp-home harness: `complete()` IPC returns candidates for a leading `/`; submitting `/skill:name`
  causes the provider to receive the `<skill>` block text (not the literal slash text); submitting a
  plain prompt is unchanged. Prior art: existing controller `prompt`/turn tests.
- **`crates/cli` migration** — the existing CLI skill/one-shot tests act as regression guards that the
  shared seam is wired in without behaviour change. Prior art: `skill_trigger_submits_the_skill_block_
  without_recording` and the one-shot skill test.
- **GUI frontend popup/chip/keymap** — no new automated seam (Option A). It is a thin projection of the
  Rust `complete()` ranking plus a key state machine; ranking is proven in Rust, and the UI is verified
  by the existing debug webview screenshot path + manual smoke. `tsc --noEmit` + `vite build` gate
  compile-level correctness.

## Out of Scope

- **Input history in the GUI** (persisting submitted prompts) — therefore `/history`, `/!!`, `/!N`
  stay unsupported and are filtered from the GUI popup.
- **`/install-skill` in the GUI** — no store-write path; filtered from the popup.
- **A dedicated `[skill]` transcript block in the GUI** — the skill invocation first renders as an
  ordinary user message; the collapsed/expandable dedicated block (TUI parity) is deferred.
- **`/usage` and `/skills` actually doing something in the GUI** — accepted as pass-through plain
  prompts for now.
- **New test tooling for `gui/ui`** (e.g. adding vitest) — rejected for this slice.
- **Changing the SDK to interpret `/` commands or skill triggers** — rejected; the SDK stays a
  semantics-free programmatic seam (ADR-0022).
- **Reworking the TUI popup itself** — only its skill-expansion call is migrated onto the shared seam.

## Further Notes

- Every Rust change is gated by `cargo fmt` + `cargo clippy -D warnings` + `cargo test` per AGENTS.md;
  `gui/src-tauri` uses the GUI's own gates (it lives outside the cargo workspace).
- The `slimcode-commands` `Command` shape change and the `slimcode-app` `prepare_user_prompt`
  addition must keep `crates/cli/tests/architecture.rs` dependency-matrix expectations passing.
- ADR-0026, the supersession note on ADR-0013, the `Completion popup` / `Commit to buffer` glossary
  generalization, and the new `Prompt preprocessing` / `Command chip` terms are already recorded.
