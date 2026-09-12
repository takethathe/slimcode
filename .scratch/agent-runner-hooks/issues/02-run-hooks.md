# 02: Run hooks — the loop can be intervened in

**What to build:** A caller can intervene in a run at three boundaries: before a Tool batch is
dispatched (rewrite a call's arguments, or skip the call and supply its result), as each tool result
is about to enter history (rewrite what the model, the log and the screen will see), and when the run
stops (rewrite the whole history). Hooks are optional closures carrying the `AgentMessage` that is
about to enter history, they run before the events and the log append for the same fact, and a hook
that errors aborts the run. With no hook set, nothing about a run changes.

**Blocked by:** 01.

**Status:** ready-for-agent

- [ ] `RunHooks` exists with three optional closure fields (`before_tool` / `after_tool` /
      `turn_end`), defaulting to unset, and `AgentRunner` carries it as a public field.
- [ ] `ToolDecision { Run, Skip(Result<String, String>) }` exists; a hook returning `Skip` stops that
      call from executing, and its supplied result (or `Error: {err}` shape) enters history and is
      announced by the usual `ToolStart`/`ToolResult` events, keeping every `tool_call` paired with a
      result.
- [ ] `AgentMessage` gains `llm_mut`, the single mutation entry point hooks use.
- [ ] A batch's `before_tool` calls all run on the loop thread in model order before anything in that
      batch is dispatched — in parallel mode too; `after_tool` runs in completion order, at the same
      moment as the tool events, and also visits a skipped call's yielded result.
- [ ] `turn_end` runs once per run, on `Completed` and on `Cancelled`, before the stop event, with the
      turn count, the stop reason and the run's final history (what it leaves there is what is
      returned).
- [ ] A hook that returns `Err` aborts the run and propagates the message like a provider or sink
      error: no stop event, no `turn_end`.
- [ ] A `before_tool` that removes a call from the batch produces an error, not a panic.
- [ ] Mutation is observable on both sides of the boundary: the rewritten text is what the returned
      history carries and what the event stream reported.
- [ ] Seam tests are written red first and cover each boundary, the batch ordering in parallel mode,
      skip semantics (success and error), mutation visibility, hook aborts, and `turn_end` on both
      stop reasons; the loop tests from ticket 01 still pass untouched.
- [ ] Documentation: ADR-0015 (hooks may rewrite the messages that enter history, with the rejected
      alternatives), `CONTEXT.md`'s `Run hooks` and `ToolDecision`, the `.scratch/arch-realignment`
      spec section that claimed the seam was missing, and `TODO.md`'s open item.
- [ ] `cargo test --workspace` green, `cargo fmt --all`, `cargo clippy --all-targets
      --all-features -- -D warnings` clean, tmux smoke tests (including one-shot text mode) green.

## Notes

- The seam ships with no production consumer; its tests are the demonstration.
- No frontend gains a hook API: the display contract still sees only events.
- The end-of-run hook is deliberately the mounting point for a future compaction feature, which is
  why it receives the history rather than just the stop reason.
