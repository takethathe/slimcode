# 03: Compaction execution — LLM call + message replacement

**What to build:** The core compaction logic: given a list of messages that exceed the token threshold, invoke the LLM to generate a structured summary and replace the oldest messages with a single `CompactSummary` entry.

This ticket owns the "what does compact actually do" question. It reads messages from history, calls the provider to generate a summary using pi's prompt template, then returns a new message list where the old messages have been replaced by one `CompactSummary`. If there's already a previous summary in history (from an earlier compaction), this ticket merges it into the new one so we keep cumulative context.

**Blocked by:** 01 — CompactSummary variant; 02 — Token estimation

**Status:** resolved

- [x] `compact_messages()` function: takes all messages, detects the CompactSummary boundary point if any, collects messages to compress, calls LLM for summary, returns `[CompactSummary, ...preserved recent messages]`
- [x] Prompt uses pi's EXACT structured template (`## Goal`, `## Constraints & Preferences`, `## Progress`, `## Key Decisions`, `## Next Steps`, `## Critical Context`)
- [x] Message serialization: converts AgentMessage list to text format (`[User]: ...`, `[Assistant]: ...`, `[Assistant tool calls]: ...`, `[Tool result]: ...`) before sending to LLM
- [x] Previous summary merge: when `previous_summary` exists, sends both the new conversation text AND the existing summary, asking the LLM to UPDATE the summary rather than regenerate
- [x] System prompt for summarization: fixed string ("You are a context summarization assistant...")
- [x] max_tokens set conservatively (2048)
- [x] Error handling: if LLM call fails, return Err(String); caller decides retry strategy
- [x] Unit tests: verify prompt structure, verify CompactSummary is returned, verify previous summary merges correctly
