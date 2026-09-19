# Runtime hooks may rewrite the messages that enter history

`core`'s agent loop becomes an `AgentRunner` value carrying three optional closure hooks
(`RunHooks`): before a Tool batch is dispatched, as each tool result is about to enter history, and
once when the run stops. The hooks are **not** observers — each receives the `AgentMessage` the
model is about to be shown and may rewrite it. Every hook defaults to unset, and with all three
unset the loop behaves exactly as it did before the seam existed.

Pure observers were rejected because the seam would then buy nothing: the event stream already
reports what happened. What a permission gate, an argument rewriter or a result redactor needs is a
place to *intervene* before a tool runs and before a result becomes history — and that place cannot
be built on top of events, which are emitted after the fact.

## Decisions

### D1 — `AgentRunner` owns the per-run collaborators; the free functions are gone

The loop is a borrowed, per-run value: `AgentRunner { tools: &[Tool], cfg: RunConfig, cancel:
&CancelToken, on_event: EventSink, hooks: RunHooks }`, with `run(provider, system, messages)`
taking the provider, the system prompt and the history. The three free entry points (`run_agent`,
`run_agent_from_messages`, `run_agent_from_messages_sink`) are deleted — `run_agent` had no caller
at all outside core's own tests, and keeping wrappers would have left three middle men over one
loop. Callers now construct a runner and call `run`.

### D2 — Hook payloads are `AgentMessage`, the type that travels everywhere else

The hook does not receive a `ToolCall` or a `String`: it receives the `AgentMessage` that is about
to enter history (for `before_tool`, the assistant message carrying the batch's `tool_calls`), so
what a hook sees is exactly what the session persists and the display renders. This needed one new
mutation entry point, `AgentMessage::llm_mut`, because `AgentMessage` was read-only.

### D3 — Hooks run before the events that announce the same thing

`before_tool` runs for the whole batch (model order, loop thread) before any call is dispatched, so
a hook can rewrite a call's arguments or skip it; `after_tool` runs as each result is about to enter
history, in completion order; `turn_end` runs before `AgentEvent::Stop`. Because the rewrite happens
before both the event and the log append, the UI, the session log and the next request see one and
the same text — the coherence ADR-0009 D2 and ADR-0012 D1 were built for.

### D4 — A skipped tool call still produces a result

`before_tool` returns `ToolDecision::Run` or `ToolDecision::Skip(Result<String, String>)`. A skip
does not remove the call from the assistant message (the provider requires every `tool_call` to be
paired with a result on the next request); it produces that result instead of executing the tool,
and still goes through `after_tool`. `Err` from any hook aborts the run and propagates like a
renderer error.

## Considered Options

- **Observe-only hooks** (`FnMut(&ToolCall)`, no mutation, no return value) — rejected: no caller
  can act on the observation, and the two real uses (deny a dangerous call, rewrite a result before
  it is logged) both need the write.
- **An `AgentRunner` that owns the provider and the tools** (`AgentRunner<P>` reusable across turns)
  — rejected: nothing wants a reusable runner today, and it would force ownership of the CLI's
  `Box<dyn Provider + Send>` and of the per-turn cancel token to move, which is new semantics rather
  than a seam.
- **A trait (`trait RunHooks { fn before_tool(..) {} … }`) whose implementor overrides one method**
  — rejected: it puts a five-method surface in the API for zero implementors, where three optional
  closure fields say the same thing with `Default`.
- **A hook payload of `&mut ToolCall` / `&mut Result<String, String>`** — rejected: it hides which
  message the rewrite lands in, and makes "which of the batch's calls is this" ambiguous.
- **An `AgentMessage::ToolResult` variant so `ok` is readable without the `Error: ` prefix** —
  rejected as too much churn on the log format; `after_tool` takes `ok: bool` alongside instead.

## Consequences

- ADR-0011's `AgentRunner` claim becomes true; its implementation note records the shape (borrowed
  per-run struct, free functions deleted) instead of the earlier deviation.
- `RunHooks` is a seam with no production caller yet: the spec's "seam only, no new semantics" was
  amended to "a seam that may intervene" when this was decided, and the tests cover the boundaries
  (each hook fires once, at the right point; all-`None` is byte-identical to the old loop).
- `AgentEvent` is unchanged: the events keep reporting what actually happened, now including a
  skipped call's synthesized result.
- **Amended by ADR-0020**: compaction was implemented, and it deliberately does *not* mount on
  `turn_end`. A hook that issued its own provider request would make a summary failure abort the
  run; the frontend runs compaction as a separate provider call after a completed turn instead.
  `turn_end` still hands the hook the whole history if a future feature needs to observe it.
