# 01: CompactSummary message variant in AgentMessage enum

**What to build:** `AgentMessage` enum gains a new `CompactSummary` variant alongside the existing `Llm` variant. It carries the structured summary text, the number of tokens before compression, and an optional reference to the previous summary (for incremental updates later). All existing methods (`to_llm`, `role`, `text_content`, `tool_calls`) behave correctly for both variants. The serde tagged enum serializes/deserializes `CompactSummary` correctly so it survives `.jsonl` log round-trips.

This is the foundation ticket — everything else builds on this type.

**Blocked by:** None (can start immediately)

**Status:** resolved

- [x] `AgentMessage::CompactSummary { summary, tokens_before, previous_summary }` added as second variant in the tagged enum
- [x] `to_llm()` returns `Some` for Llm, `None` for CompactSummary
- [x] `text_content()` returns the summary string for CompactSummary
- [x] `tool_calls()` returns empty slice for CompactSummary
- [x] Serde round-trip: serialize CompactSummary → deserialize back → equality holds
- [x] Existing `session_round_trips_losslessly` test still passes (regression guard)
