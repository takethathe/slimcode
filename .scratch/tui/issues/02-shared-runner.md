# 02: shared runner in common

**What to build:** The shared turn runner in `slimcode-common` (ADR-0004, spec §Implementation Decisions): a free `run_turn` that takes a provider, tools, messages, a `RunConfig`, and a `&mut dyn Renderer`, drives the existing agent loop, and streams every event to the renderer live as it happens, returning the updated message history. Not yet wired into the CLI; verified by unit tests.

**Blocked by:** 01 — shared renderer model in common

**Status:** resolved

- [ ] `run_turn(provider, tools, messages, &RunConfig, &mut dyn Renderer) -> Result<Vec<Message>, String>` in `slimcode-common` (no new dependencies; the agent crate stays untouched).
- [ ] Every `AgentEvent` from the existing agent loop is mapped through `map_event` and streamed to the renderer in order, live, as the loop runs.
- [ ] The returned message history is the loop's updated history (assistant replies and tool results appended), matching today's `RunResult::messages` semantics.
- [ ] A provider or runner error propagates as `Err(String)`; no partial history is fabricated.
- [ ] Token usage is NOT rendered by the runner — it stays a frontend concern (the frontend reads its provider's total usage and feeds a `DisplayItem::Usage` itself).

- [ ] Unit tests use the agent crate's scripted `FakeProvider` pattern and a recording renderer, asserting: the ordered display-item stream (text, reasoning, tool start/result, stop), that raw tool deltas are suppressed, that a runner error propagates, and that the returned history is correct.

- [ ] The full common-crate test suite passes.

## Notes

- Errors stay `String`, matching the existing agent API (spec §Implementation Decisions).
- This is the primary test seam from the spec: inject a scripted provider + recording renderer and assert the external behavior (the display-item stream and returned history).

## Answer

Implemented in `crates/common/src/runner.rs` (`run_turn`) + a live event sink in
`crates/agent/src/agent.rs` (`run_agent_from_messages_sink`), committed together
with ticket 01 as the shared seam (ADR-0004). `run_turn` drives the existing
agent loop through the sink, maps every event with `map_event`, and streams it
live to the `&mut dyn Renderer`; it returns the loop's updated `Vec<Message>`
(identical to `RunResult::messages`, asserted against the agent loop), propagates
provider/renderer errors as `Err(String)`, and never renders token usage (frontend
concern). The agent crate's existing `run_agent`/`run_agent_from_messages` APIs
and behavior are unchanged; tests use the `FakeProvider` + recording-renderer
pattern.
