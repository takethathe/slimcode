# 02: Token estimation and threshold check

**What to build:** A set of pure helper functions to estimate how many tokens a session's message history consumes and decide whether compaction should trigger. Uses the char/4 heuristic from pi — conservative overestimate, no external deps.

A compacted session never re-enters the loop: once a `CompactSummary` is in history, the threshold check skips it (it won't exceed the limit again). This means compaction is self-regulating — it only fires when fresh Llm messages push us past 92%.

**Blocked by:** 01 — CompactSummary variant (needs `text_content()` on CompactSummary for token counting)

**Status:** resolved

- [x] `estimate_message_tokens(msg: &AgentMessage) -> usize` returns estimated token count using char/4 heuristic
- [x] `estimated_context_window() -> usize` returns the constant 128_000
- [x] `should_compact(messages: &[AgentMessage]) -> bool` returns true if total est. tokens > 92% of window AND the most recent entry is NOT already a CompactSummary (prevents re-triggering)
- [x] `estimate_total_tokens(messages: &[AgentMessage]) -> usize` sums per-message estimates
- [x] Unit tests: single message, multi-message, edge cases at boundary (91%, 92%, 100%)
