# Compaction replaces old history with a session-only summary

Long sessions eventually overflow a model's context window. pi solves this with *compaction*:
walk back to a recent-keep boundary, summarize the older span with an LLM, append a
`CompactionEntry` carrying a `firstKeptEntryId`, and rebuild every later request as `summary +
entries from that id onward`. slimcode has a different substrate: a two-layer message model
(ADR-0012) and an append-only JSONL log (ADR-0009) whose messages have no stable ids.

We adopt compaction with the checkpoint carried *inside the message history itself*:
`AgentMessage` gains a session-only `CompactSummary` variant, and context assembly treats it as a
boundary. This keeps the feature inside the existing types — no new log record kind, no message
ids, no log rewrite — and reuses the `to_llm` seam ADR-0012 already reserved for exactly this
("the first planned one is a compaction summary").

## Decisions

### D1 — The checkpoint is an `AgentMessage` variant, not a log record

`AgentMessage::CompactSummary { summary, tokens_before, previous_summary }` joins `Llm`. The
serde tag is `{"kind":"compact_summary",…}`, so it round-trips through the existing
`LogRecord::Message` path and restores on load with no parser change. `to_llm` returns `None`
for it and `role()` reports `User` (the role its injection takes). `llm_mut` returns
`Option<&mut Message>` so ADR-0015's "a hook may rewrite the message" cannot rewrite a checkpoint
(a session-only message has no wire form to rewrite).

A separate log record kind was rejected: the record would need its own ordering marker relative to
message records, which is exactly the `firstKeptEntryId` machinery slimcode otherwise avoids.

### D2 — The boundary is applied on load; `ContextBuilder` injects in place

The append-only log keeps *every* record, including the span a checkpoint replaced. The restored
Session must not. `SessionStore::load` therefore drops every message record before the **newest**
`CompactSummary`; the log stays a truthful journal, while a resumed session holds the effective
history. `ContextBuilder::build()` then walks the history and substitutes each remaining
`CompactSummary` with one `user` message in place:

```
The conversation history before this point was compacted into the following summary:

<summary>
{summary}
</summary>
```

A later checkpoint naturally supersedes an earlier one because only records from the newest one
onward survive the load. Putting the drop on the load path (and leaving `build()` a
position-preserving substitution) means the live session, the resumed session and the assembled
request all agree, and the model never sees a compacted span. The known cost is that the
recent-keep tail written *before* the checkpoint in the append-only log is treated as part of the
replaced span on reload: it survives the live session but not a resume. Rewriting the log at
compaction was rejected (ADR-0009), and re-deriving the tail from the keep heuristic on load would
couple the reader to a policy that may change.

### D3 — The frontend decides when; the app module decides what

`app::compaction` owns `estimate_message_tokens` / `estimate_total_tokens` (`chars / 4`, counting
tool-call names and arguments), `should_compact` (estimated total > 92% of a hard-coded 128k
window, and the newest entry is not already a checkpoint), and `compact_messages`. The frontend
triggers it after a **completed** turn (never a cancelled or errored one) or on demand with
`/compact`, and folds the summary request's provider usage into the session's token total
(ADR-0018 D4). Keeping the trigger out of `RunHooks::turn_end` matches ADR-0015: hooks rewrite
messages a run is producing, while compaction is a separate provider request the frontend can
fail without losing the turn's result.

### D4 — Keep a recent tail; one provider call reuses the chat seam

`compact_messages` walks back to a `keep_recent_tokens` (8% of the window) boundary, refusing to
start the tail on a `tool` result (a kept result must keep the assistant tool call it answers),
serializes the span with pi's `[User]` / `[Assistant]` / `[Assistant tool calls]` / `[Tool result]`
text, and sends pi's structured prompt (`SUMMARIZATION_SYSTEM_PROMPT` + the initial or update
instruction) through the ordinary `Provider::chat` seam with no tools, no cache writes and
`max_tokens = 2048`. The result is `[CompactSummary, ...kept]`. A provider error, a cancel or an
empty summary returns `Err` and leaves the caller's history untouched; the frontend reports it and
the next turn tries again with the full history.

Keeping a tail is deliberate: it gives the turn right after a compaction exact recent context
instead of only the summary. (It does not survive a reload — see D2.)

### D5 — Out of scope for v1

Branch summarization (slimcode is a linear session), cumulative file-operation tracking, split
turns (partially summarizing one oversized message), configurable thresholds, streaming summary
generation, and compaction in one-shot mode (no persistent session, no trigger) are all excluded.

## Considered Options

- **pi's `firstKeptEntryId` / a dedicated compaction log record** — rejected: it needs stable
  message ids and a second ordering concept in the log, for a linear session that only ever has
  one live boundary.
- **Rewriting the log at compaction** (header + title + checkpoint + kept records) — rejected: it
  would break ADR-0009's append-only invariant and require the load path to trust a rewritten
  file. Instead the boundary lives in `build()`, so the log stays a truthful append-only journal.
- **Dropping the kept tail (summarize everything)** — rejected: the spec asks compaction to keep
  recent messages, and pi does too; a tail preserves the immediate working context exactly.
- **Running compaction inside `RunHooks::turn_end`** — rejected: a hook that issues its own
  provider request would make a hook failure abort the run, contradicting the requirement that a
  failed compaction keeps the turn's result.
- **Async compaction** — rejected: `Provider` is a synchronous seam, and the summary request is a
  plain blocking call like any other.
