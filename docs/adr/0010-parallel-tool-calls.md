# Parallel tool calls

The agent loop expected exactly one tool call per model response and executed calls serially. That
left two capabilities unused. On the wire, the Bailian OpenAI-compatible endpoint accepts
`parallel_tool_calls` (default `false`); with it on, the model answers an independent multi-tool
request — "read A and read B", "weather in Beijing and Shanghai" — with several `tool_calls` in one
response instead of one call per round-trip. In the runtime, a `RunConfig.parallel_tools` switch
existed but was never set by any frontend, and its "parallel" branch only batched results: the calls
themselves still ran one after another, so the batch cost the sum of its calls. This ADR turns both
on by default and records the semantics the runtime adopts for a Tool batch: true concurrency, a
deterministic history, and completion-ordered display events.

## Decisions

### D1 — The request opts into parallel tool calls by default, with no configuration

Every request that declares `tools` carries `parallel_tool_calls: true`. A request without tools
omits the flag entirely, so the plain-answer path keeps the byte shape it had before this change.
There is no config knob: the capability is on for everyone, and a model that cannot use it simply
returns one call as before.

### D2 — A Tool batch executes on scoped threads, not async

A batch's calls are dispatched concurrently with `std::thread::scope` — one thread per call — and a
channel reports each completion back to the dispatching thread. This is the only concurrency
mechanism consistent with the rest of the codebase: the `Provider` seam is synchronous (blocking
reqwest), the tools are blocking I/O (file reads, process spawns), and there is no async runtime to
join. `std::thread::scope` needs no new dependency and no `'static` tool storage, because the scope
guarantees every worker is joined before it returns.

`Tool.run` is tightened from `Fn + Send` to `Fn + Send + Sync` so the batch's worker threads can
share one `&[Tool]` read-only. Every existing tool closure captures owned `PathBuf` / `CancelToken`
state, so no tool implementation changes.

### D3 — Results enter history in model order; tool events stream in completion order

For one batch the runtime separates two orders that were previously the same:

- **History** (and the per-message events a session layer persists, ADR-0009 D5) is appended in the
  order the model emitted the calls — the `tool_calls` index. A Session log therefore records a
  deterministic, replayable order no matter which call happened to finish first, and the in-memory
  history matches the log exactly.
- **Tool display events** (`ToolStart` / `ToolResult`) are emitted the moment a call finishes, so a
  long call no longer delays the block of a fast one. The renderer shows completions as they land.

### D4 — Tool events carry the call id, and renderers pair on it

`ToolStart` and `ToolResult` (and the `DisplayItem`s they map to) gain a `tool_call_id`. The TUI
pairs a result with a pending block by that id. Pairing by name could not survive this change: two
`get_weather` calls in one parallel batch both left a pending block named `get_weather`, and the
second result landed on the first call's block. The id is the only stable identity a call has; the
id is non-optional on both events, so there is no id-less source to fall back for.

### D5 — The cancel contract is unchanged

A cancel is checked before the batch dispatches, inside each worker before it dispatches its call,
and once more after the batch completes but before any result is applied. A cancel that lands while
a batch runs discards the whole unapplied batch — nothing enters history — while results already
pushed by earlier batches stay. Cancellable tools (bash) keep killing their process group, so a
cancel is not made slower by concurrency.

### D6 — The serial path is kept

`RunConfig.parallel_tools` remains, defaulting to `true`. Every batch — including a single-call one
— takes the parallel branch by default; tests that assert the serial append semantics set the flag
to `false` explicitly. Keeping the path costs one branch and keeps the old behaviour expressible.

## Considered Options

- **An async runtime (tokio) for tool fan-out** — rejected: it would pull an async runtime into a
  synchronous seam (`Provider` is blocking) for a fan-out the standard library already expresses.
- **`rayon`'s `par_iter`** — rejected: a dependency for one scoped fan-out, and it would still need
  the same `Sync` bound.
- **Bounded concurrency (a semaphore / fixed pool)** — rejected: batches are the size the model chose
  them (typically two to five calls), and the model already controls the count through its response.
- **Read tools in parallel, write tools serial** — rejected by the user: every tool runs in parallel.
  The model is responsible for not parallelizing dependent or conflicting calls; a single `fs::write`
  is atomic, and `edit`'s read-modify-write race is a modelling mistake the runtime does not police.
- **Emitting history in completion order** — rejected: it would make the Session log and the
  replayed history non-deterministic and turn a reordering into a spurious history diff.
- **Emitting tool events in model order** — rejected: it would hold a finished call's block until the
  slowest call of the batch finished, defeating the point of concurrency for the UI.
- **Keeping name-based pairing** — rejected: it mis-assigns same-tool calls in exactly the parallel
  case this change introduces (D4).
- **Gating the flag by model / falling back for thinking models** — rejected: official documentation
  does not declare thinking models unsupported, and a silent per-model downgrade would hide the
  capability. The risk is recorded under Consequences instead.

## Consequences

- Requests that declare tools now carry one extra field; `tools`-less requests are byte-identical to
  before.
- `Tool` closures must be `Sync`; existing tools satisfy this unchanged.
- `AgentEvent::ToolStart` / `ToolResult` and their `DisplayItem`s carry `tool_call_id`; the TUI's
  `Entry::Tool` records the id and pairs on it.
- A parallel batch's display-event order no longer matches its history order. Anything that assumed
  a single global order must use the per-message events (history order) or the tool events
  (completion order) deliberately; the session layer uses the former, the renderers the latter.
- Thinking-model compatibility is not handled: the flag is always sent. Should a thinking model
  misbehave with parallel calls, the fix is a targeted follow-up, not a silent default change.
- Tests added: concurrency peak ≥ 2 (`slimcode-agent`), history-vs-event ordering split
  (`slimcode-agent`, `slimcode-common`), request serialization (`slimcode-ai`), and same-tool
  pairing by id (`slimcode-tui`).
