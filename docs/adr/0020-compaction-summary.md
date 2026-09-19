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

D1–D5 cover compaction. D6 (added later) records the token-usage/context seam that grew out of it:
usage travels on the assistant message events and the footer estimates its own context segment with
the same estimator and window the trigger uses.

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
tool-call names and arguments), `should_compact` (estimated total ≥ the 200k `DEFAULT_CONTEXT_WINDOW`
minus a fixed `MIN_CONTEXT_REMAINING` of 16k — i.e. compact once at most 16k tokens of headroom are
left, and the newest entry is not already a checkpoint), and `compact_messages`. The frontend
triggers it after a **completed** turn (never a cancelled or errored one) or on demand with
`/compact`, and folds the summary request's provider usage into the session's token total
(ADR-0018 D4). Keeping the trigger out of `RunHooks::turn_end` matches ADR-0015: hooks rewrite
messages a run is producing, while compaction is a separate provider request the frontend can
fail without losing the turn's result.

The trigger was originally "estimated total > 92% of a hard-coded 128k window". Both constants moved
when the target models' advertised 200k contexts became the reference: a fixed margin (16k, equal
to the 8% keep budget below) tracks "enough room for one more turn" better than a percentage, which
would have grown the headroom to 16k only by coincidence. The window and margin stay hard-coded
(non-configurable) for v1.

### D4 — Keep a recent tail; one provider call reuses the chat seam

`compact_messages` walks back to a `keep_recent_tokens` (8% of the window = 16k, the same budget as
D3's `MIN_CONTEXT_REMAINING`) boundary, refusing to
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

### D6 — Usage rides the message events; the footer estimates its own context

Token usage travels with the assistant message that earned it. `Provider::chat` returns
`Result<Option<TokenUsage>, String>`: the request's own usage, or `Ok(None)` when the endpoint
omitted it **and** when the request was cancelled (a torn request never earned its tokens; an HTTP
error still returns `Err`). The runtime carries that value on the assistant `AgentEvent::Message`
(the Stop and ToolCalls branches alike; tool results carry `None`), and the shared turn runner's
sink hands it to the frontend's per-message hook and renders one usage display item per assistant
message. The frontend accumulates the session total from those events (`session.usage`), replacing
ADR-0018 D4's turn-end diff of the provider's running counter: the footer still reads the session's
own total — not the provider's — but it now updates after each assistant reply rather than once per
turn, so a multi-request tool loop shows every reply's effect as it lands.

The footer's new context segment (`45.3%/200k`, dim ≤ 70% / yellow > 70% / red > 90%) is estimated
from the live session history with the same `chars / 4` estimator and the same 200k window the
compaction trigger uses, so the displayed percentage and the auto-compact boundary always agree.
`ContextUsage { percent, window }` is deliberately not an `Option` (pi carries a `?` unknown state):
slimcode uses a constant window and estimates it itself, so the value is always computable. The
usage render item's `context` is an `Option` only to cover the brief window between a session
switch (which hides the old session's segment) and the CLI's follow-up item that re-establishes it.

Carrying usage on the message also keeps the seam single: the compaction call folds its own request
cost in through the same event, and a session switch re-establishes the footer through one usage
item — there is no separate usage event stream to keep in sync with the messages.

- **A `Delta::Usage` variant** — rejected: usage is not a streamed content fragment. It arrives
  once with the response tail and would have to be threaded through the delta assembler, which
  exists to join text/tool-call fragments and would otherwise aggregate it as content.
- **An `on_usage` callback alongside `on_delta`** — rejected: a second callback for a datum that
  already has a natural carrier (the `chat` return value), and one the runner would then have to
  route separately from the message it describes.

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
