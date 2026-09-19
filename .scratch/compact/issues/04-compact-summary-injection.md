# 04: CompactSummary injection into ContextBuilder

**What to build:** When `ContextBuilder::build()` assembles the message list for a turn, it now scans the history for `CompactSummary` variants and injects each one as a formatted user text message into the conversation flow. This is how the model "sees" previous compactions — not as a native enum variant, but as structured text with XML `<summary>` tags, exactly like pi's approach.

Without this ticket, compaction would exist in memory/log but the model would never receive its content. With it, the next LLM request includes the summary right before any un-compactified recent messages.

**Blocked by:** 01 — CompactSummary variant

**Status:** resolved

- [x] `ContextBuilder::build()` scans self.history; for each `AgentMessage::CompactSummary`, emits a `Role::User` message with content: `"The conversation history before this point was compacted into the following summary:\n\n<summary>\n{summary}\n</summary>"`
- [x] Non-CompactSummary (Llm) messages pass through unchanged via `to_llm()` conversion
- [x] CompactSummary entries appear in history order (earlier summaries before later ones)
- [x] The injected user message is appended to the message list BEFORE the turn's own user prompt
- [x] No additional context files or skills are affected by this change
- [x] Unit test: history with CompactSummary + regular messages → verify output messages contain injected summary text in correct position
- [x] Regression: existing tests without CompactSummary still produce identical context
