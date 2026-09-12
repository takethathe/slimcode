# Frontend-agnostic renderer: DisplayItem + Renderer trait + shared turn runner in slimcode-common

> **Amended by ADR-0014 (TUI mode)**: the `Renderer` that consumes `DisplayItem`s for the
> interactive TUI is the CLI's `TuiAdapter`, which converts them into `slimcode_tui::RenderItem`s;
> `slimcode-tui` itself no longer implements `Renderer` and depends on no other slimcode crate.
> `map_event`, the `Renderer` trait, the `DisplayItem` enum, `usage_summary` and the shared
> `run_turn` stay exactly as decided here.

The rendering and turn-execution seam moves into `slimcode-common`: a pure `map_event(AgentEvent) -> Option<DisplayItem>` produces frontend-agnostic display units, a `Renderer` trait consumes them, and a shared `run_turn` streams each event to a `&mut dyn Renderer` as it happens. This gives the CLI and TUI one shared event-to-display mapping and one shared turn loop, each frontend implementing only its own `Renderer` (text lines for the CLI, widget state for the TUI). Rejected alternatives: a `Renderer` trait consuming raw `AgentEvent` (each frontend re-implements the mapping), and returning the full `RunResult` for post-hoc rendering (loses live streaming).

**Consequences**: `crates/cli/src/render.rs` shrinks to a thin `TextRenderer` over the common trait. Token usage stays a frontend concern: after the run the frontend reads its concrete provider's total usage and feeds a `DisplayItem::Usage` to its renderer.
