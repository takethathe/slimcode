# TUI stays dependency-free: the CLI owns the DisplayItem → RenderItem adapter

`DisplayItem` + `Renderer` remain a frontend-agnostic contract in `slimcode-app` (ADR-0004), but
`slimcode-tui` must not depend on `app` — otherwise "the TUI is a graphics library" is unassertable
and the library drags the agent runtime, config and session types in. The resolution is a second,
TUI-owned vocabulary: the CLI implements a `Renderer` (`TuiAdapter`) that converts each
`DisplayItem` into a `slimcode_tui::RenderItem`, and the TUI only ever sees its own types.

Two frontends, three vocabularies: `AgentEvent` (core) → `DisplayItem` (app, shared mapping) →
`RenderItem` (tui, via the CLI's adapter). Text mode is `DisplayItem` → lines; TUI mode is
`DisplayItem` → `RenderItem`.

## Decisions

### D1 — `RenderItem` is the TUI's display vocabulary

`RenderItem` carries only what the TUI renders: `Text`, `Reasoning`, `ToolStart`, `ToolResult`,
`Notice`, `Error`, `UserPrompt`, `Usage(FooterUsage)`, `Skills(Vec<SkillInfo>)`,
`Branch(Option<String>)`, `SessionChanged { id }`. CLI-owned state that is not derived from an
`AgentEvent` (notices, an installed-skills refresh, a branch change, a loaded session, token usage)
is emitted as `RenderItem`s too, so there is exactly one channel the TUI applies.
`DisplayItem::Turn` and `DisplayItem::Stop` are dropped by the adapter — the TUI already ignores
them (the pi-aligned transcript has no turn markers and renders nothing for a stop).

`Usage` carries `slimcode_tui::FooterUsage`, not `ai::TokenUsage`: the TUI's own footer type is
already the one it draws, and keeping the AI type out of it is what makes the zero-dependency claim
true. The CLI does the conversion.

### D2 — The TUI's channel carries `RenderItem`; `App::apply` replaces `impl Renderer`

`AgentEvent → DisplayItem` stays in `app::map_event` and runs on the turn's worker thread inside
`app::run_turn`. The CLI's `TuiAdapter` (an `impl Renderer`) runs on that same worker thread and
sends `RenderItem`s over the TUI's mpsc channel; the UI thread drains them into
`App::apply(RenderItem)`. The TUI never sees `DisplayItem`, `AgentEvent`, `TokenUsage` or a
`Session`. The transcript merge/pairing rules (streaming text and reasoning merged into the last
entry, tool start/result pairing on `tool_call_id`, status colours) stay private to `App`, with
their `TestBackend` tests.

### D3 — Command prediction is injected, not imported

The reducer still refreshes the `/` completion popup on every keystroke, but the candidate pool
comes from a `CompletionProvider` injected at `App::new` (the CLI builds it from
`slimcode-commands` + the skills store). `/help`, the unknown-command "did you mean" notice and
skill-name resolution are produced by the CLI when it answers the corresponding `Effect` and arrive
as `RenderItem::Notice` / `Error`. The TUI therefore has no `slimcode-commands` dependency either.

### D4 — `usage_summary` stays in `app`

It is wording, not state, and both of its consumers are now in the CLI. There is no reason to move
it, and keeping it in the display contract keeps the "the two modes cannot drift on wording" rule
from ADR-0004.

## Considered Options

- **Move `DisplayItem` into the TUI** — rejected: text mode would then depend on the TUI crate (and
  ratatui/crossterm) for a data type, and ADR-0004's shared event-to-display mapping would be
  abandoned for the sake of one crate's purity.
- **Let the TUI depend on `app` for the display contract only** — rejected: the dependency matrix
  could no longer assert "`tui` has no slimcode dependencies", the intended boundary would live in a
  review convention instead of in `Cargo.toml`, and `app`'s future service additions would be one
  `use` away from leaking into the UI.
- **The CLI constructs `tui::Entry` and pushes it** — rejected: `Entry` and the merge/pairing rules
  are the TUI's deepest module; exposing them would move the transcript state machine and its
  `TestBackend` tests into the CLI.
- **Send `AgentEvent`s over the channel and map them on the UI thread** — rejected: mapping belongs
  on the producer side (it is cheap, and the UI thread must stay free to draw), and the CLI's
  handler would have to participate in every drain.

## Consequences

- Three vocabularies exist, and the third one is the price of the zero-dependency claim; the
  duplication is mechanical (`DisplayItem` variants map 1:1 or are dropped) and lives entirely in
  one CLI file.
- ADR-0004 is amended: in TUI mode the `Renderer` implementation is the CLI's adapter, not the
  TUI's widget state. The shared `map_event`, the `Renderer` trait, the `DisplayItem` enum and
  `usage_summary` are unchanged.
- `CONTEXT.md` gains `RenderItem`; `Renderer`'s entry names the CLI's two implementations.
- Ticket 06's dependency test asserts `crates/tui/Cargo.toml` has no `slimcode-*` dependency and that
  `crates/tui/src` mentions no `SessionStore`/`SkillStore`/`Config`.
