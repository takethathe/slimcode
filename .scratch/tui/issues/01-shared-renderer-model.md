# 01: shared renderer model in common

**What to build:** The frontend-agnostic renderer seam in `slimcode-common` (per ADR-0004 and spec §Implementation Decisions): a `DisplayItem` enum, a pure `map_event` function turning an `AgentEvent` into an optional display unit, and a `Renderer` trait that every frontend implements. This slice is not yet wired into the CLI or any TUI and is verified by unit tests.

**Blocked by:** None (can start immediately)

**Status:** resolved

- [ ] A `DisplayItem` enum covering every display unit from the spec: turn marker, reasoning line, streamed text fragment, tool start, tool result, stop marker, and token usage.
- [ ] A pure `map_event(AgentEvent) -> Option<DisplayItem>`: raw tool-call deltas (`ToolCallStart` / `ToolCallArgs` / `Done`) are suppressed; streamed text maps to a streamed text fragment; reasoning maps to a reasoning line; `ToolStart` / `ToolResult` / `Stop` / `Turn` map to their own items.
- [ ] A `Renderer` trait whose single method takes a `DisplayItem` and returns `Result<(), String>`, matching the existing agent error style.
- [ ] The module is registered in the crate so other crates can consume it.
- [ ] Unit tests cover: every event kind maps as specified, raw tool deltas are suppressed, and the mapped item order for a small event stream.

- [ ] The full common-crate test suite passes.

## Notes

- Domain terms to use (per `CONTEXT.md`): `DisplayItem`, `Renderer`, `Frontend`.
- The mapping must stay byte-equivalent to today's `crates/cli/src/render.rs` text output once rendered by a text renderer (checked in ticket 03); keep the text fragments faithful (no trimming, no re-wording).

## Answer

Implemented in `crates/common/src/render.rs` and committed with ticket 02 as part
of the shared seam (ADR-0004): `DisplayItem` enum, pure `map_event`
(`ToolCallStart`/`ToolCallArgs`/`Done` suppressed, every other kind maps
faithfully), and the `Renderer` trait (single `render(&mut self, &DisplayItem)
-> Result<(), String>`). Module registered in `slimcode-common`. Unit tests cover
every event kind, raw-delta suppression, and mapped order for a small stream.
