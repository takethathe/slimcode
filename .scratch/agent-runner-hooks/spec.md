# AgentRunner and Run hooks

## Problem Statement

The agent loop offers exactly one way to observe a run: the `AgentEvent` stream. Events are
emitted after the fact — a tool result is announced once it exists, and even `ToolStart` is emitted
after the call has already finished — so a caller can watch a run but cannot **intervene** in it.
Three things a coding agent needs are therefore impossible today:

- refuse to execute a tool call (a permission gate over `bash`, or a sandbox that rejects a path);
- rewrite what a tool returned before the model, the session log and the user see it (redaction, a
  truncated result, a corrected error);
- rewrite the message history at the end of a run (the mounting point compaction will need).

The loop also arrives as three free functions threaded through seven positional arguments, so there
is no single value a frontend, a test or a future hook registry can hold.

## Solution

The loop becomes a value: `AgentRunner`, holding the run's tools, config, cancel token, event
subscription and its optional `RunHooks`. Three hook boundaries exist, each receiving the
`AgentMessage` that is about to enter history, mutably:

- **before a Tool batch** — every call of the batch, in model order, before anything is dispatched;
  the hook may rewrite a call's arguments, or return `ToolDecision::Skip` to supply the result
  instead of running the tool;
- **as each tool result is about to enter history** — in completion order, with an `ok` flag;
- **when the run stops** — once, with the turn count and the stop reason, holding the whole history.

Every hook defaults to unset and, with all of them unset, the loop behaves exactly as it does
today. Hooks run **before** the events and before the log append for the same fact, so the model
request, the session log and the display always see the same text.

## User Stories

1. As a maintainer, I want the agent loop to be a value instead of three free functions, so that a
   new boundary (a hook, a subscriber, a second frontend) has one place to attach to.
2. As a maintainer, I want the loop's per-run collaborators in one struct, so that adding a
   collaborator does not lengthen every call site's argument list.
3. As a frontend author, I want the event subscription to stay exactly as it is, so that the display
   contract (`Renderer`, `DisplayItem`) and both frontends keep working untouched.
4. As a frontend author, I want all hooks unset to be byte-identical to today's behaviour, so that
   the seam cannot change what the CLI prints.
5. As a plugin author, I want a hook that runs before a tool call is dispatched, so that I can
   inspect the arguments of every call in a batch before any of them executes.
6. As a plugin author, I want that hook to be able to rewrite a call's arguments, so that I can
   normalise or constrain them without forking the runtime.
7. As a plugin author, I want to skip a call and supply the result myself, so that I can deny a
   dangerous command (a permission gate) without the tool ever running.
8. As a plugin author, I want a denied call to still appear as a tool result to the model, so that
   the conversation stays valid (every `tool_call` paired with a result) and the model can adapt.
9. As a plugin author, I want a denied call's result to be able to be an error, so that the model is
   told the call was refused rather than silently handed a fake success.
10. As a plugin author, I want a hook that runs as each tool result is about to enter history, so
    that I can redact or truncate output before it is stored or shown.
11. As a plugin author, I want that hook to know whether the tool succeeded, so that I do not have to
    sniff an `Error: ` prefix out of the text.
12. As a plugin author, I want a hook at the end of a run that receives the whole message history, so
    that I can rewrite the conversation (the mounting point for compaction) with the run over.
13. As a plugin author, I want a hook that errors to abort the run and propagate its message, so that
    a broken gate fails loudly instead of letting a call through.
14. As a plugin author, I want every hook to be optional, so that a consumer sets only the boundary
    it cares about.
15. As a session-log reader, I want a hook's rewrite to be what is stored, so that the log never
    disagrees with what the model was shown.
16. As a TUI user, I want a hook's rewrite to be what the transcript shows, so that the screen never
    disagrees with the log.
17. As a TUI user, I want a skipped call to render as a normal tool block with its supplied result,
    so that a denied call is visible rather than a mystery gap in the transcript.
18. As a test author, I want to assert hook behaviour through the loop's public surface (returned
    history plus the event stream), so that tests do not reach into the runtime's internals.
19. As a test author, I want a hook-only test provider to be unnecessary, so that hook tests reuse
    the scripted provider the loop tests already have.
20. As a maintainer, I want the removed entry points to have no dead remainder, so that a reader
    cannot mistake an unused wrapper for a supported path.

## Implementation Decisions

- **Where.** `slimcode-core`'s agent runtime module owns the runner and the hook vocabulary; the
  application layer's shared turn runner constructs an `AgentRunner` instead of calling the free
  loop function. No new module, no new crate, no dependency change (the matrix in ADR-0011 D4 is
  unaffected).
- **Shape.** The frozen shape below came out of the design session; it is the decision, not a
  sketch. `EventSink` becomes public because it is now part of a public struct's surface.

  ```rust
  pub enum ToolDecision { Run, Skip(Result<String, String>) }

  pub type EventSink<'a> = &'a mut dyn FnMut(AgentEvent) -> Result<(), String>;

  #[derive(Default)]
  pub struct RunHooks<'a> {
      pub before_tool: Option<Box<dyn FnMut(&mut AgentMessage, usize) -> Result<ToolDecision, String> + 'a>>,
      pub after_tool:  Option<Box<dyn FnMut(&mut AgentMessage, bool) -> Result<(), String> + 'a>>,
      pub turn_end:    Option<Box<dyn FnMut(&mut Vec<AgentMessage>, usize, &StopReason) -> Result<(), String> + 'a>>,
  }

  pub struct AgentRunner<'a> {
      pub tools: &'a [Tool],
      pub cfg: RunConfig,
      pub cancel: &'a CancelToken,
      pub on_event: EventSink<'a>,
      pub hooks: RunHooks<'a>,
  }

  impl<'a> AgentRunner<'a> {
      pub fn new(tools: &'a [Tool], cfg: RunConfig, cancel: &'a CancelToken, on_event: EventSink<'a>) -> Self;
      pub fn run<P: Provider>(
          &mut self,
          provider: &mut P,
          system: &Message,
          messages: Vec<AgentMessage>,
      ) -> Result<(Vec<AgentMessage>, StopReason), String>;
  }
  ```

- **One mutation entry point.** `AgentMessage` gains `llm_mut(&mut self) -> &mut Message`. It is
  preferred over purpose-built accessors (`tool_calls_mut`, `set_text`) because the wire message's
  fields are already public, so this adds no new exposure and does not need a new method per field.
- **Free functions and `RunResult` are deleted.** `run_agent` has no caller outside core's own
  tests; `run_agent_from_messages` is used by one test; `run_agent_from_messages_sink` by the shared
  turn runner. `RunResult` exists only to package those entry points' extra outputs, so it goes too
  (a test harness collects the event stream and counts `Turn` events).
- **Hook ordering is a contract.** A batch's `before_tool` calls all run on the loop thread, in
  model order, before any call in that batch is dispatched; `after_tool` runs in completion order,
  at the same moment and in the same order as the `ToolStart`/`ToolResult` events; `turn_end` runs
  before `AgentEvent::Stop`. Every hook therefore precedes the event and the log append for the same
  fact.
- **A skipped call is still a result.** `ToolDecision::Skip(Ok(body))` yields the tool result
  `body`; `Skip(Err(err))` yields the `Error: {err}` shape tool failures already use. A skipped call
  is not dispatched, but its result still goes through `after_tool` and still emits the
  `ToolStart`/`ToolResult` pair, so a denial is visible in the transcript and the log stays paired.
- **`before_tool` may not restructure the batch.** It may rewrite `arguments`; it may not add or
  remove `tool_calls` (denial is expressed by `Skip`). The loop re-reads `tool_calls[index]` after
  each hook call and returns `Err` if the index vanished, rather than panicking.
- **Errors.** A hook's `Err(String)` aborts the run and propagates exactly like an event-sink or
  provider error; no stop event is emitted, and `turn_end` does not run on that path.
- **`turn_end` fires on `Completed` and `Cancelled`** — a cancelled run is precisely when a caller
  wants to seal its bookkeeping. The history handed to it is the run's final history (including
  partial tool results from a cancelled batch) and what it leaves there is what the caller stores.
- **Behavioral identity.** With `RunHooks::default()` the loop's messages, events and stop reasons
  are exactly what they are before this change; the existing loop tests are the regression net.
- **Documentation.** ADR-0015 records the hooks contract (including the rejected observe-only and
  trait-registry alternatives); ADR-0011's implementation note becomes "implemented, here is the
  shape"; `CONTEXT.md` gains `AgentRunner`, `Run hooks` and `ToolDecision`; the core section of
  `development.md` describes the runner and drops the "known spec deviation" note; the spec's
  runtime section (`docs`-adjacent, under `.scratch/arch-realignment/`) and `TODO.md` stop claiming
  the seam is missing.

## Testing Decisions

- **One seam, and it is the highest one that exists.** Everything is tested through
  `AgentRunner::run` with the scripted `FakeProvider` the loop tests already use: a run returns its
  history and streams its events into a collecting sink, so hook behaviour is observable without
  touching a private function. No new seam is introduced in the application layer: the shared turn
  runner keeps its existing recording-renderer tests, and the byte-identical claim for one-shot
  output stays covered by the CLI's tmux smoke test.
- **A good test here asserts external behaviour**: the messages that come back, the sequence of
  events the sink received, what the hook itself recorded, and the `Err` a hook raised — never which
  internal helper called the hook.
- **Prior art**: the loop tests in the core crate
  (`message_events_sit_after_stream_and_before_stop` for the event-sequence style,
  `parallel_history_follows_model_order_while_events_follow_completion_order` for the ordering
  style, `cancelled_parallel_batch_emits_no_tool_events` for the "what must not happen" style), and
  `slimcode-app`'s runner tests for the recording-renderer pattern.
- **What is covered**: each hook fires at its boundary, exactly once per call or run; a batch's
  `before_tool` calls all precede any dispatch, even in parallel mode; mutation is visible in the
  returned history *and* in the events; `Skip(Ok)`/`Skip(Err)` produce a paired, `after_tool`-visited
  result; a hook error aborts the run (no stop event, no `turn_end`); `turn_end` fires on
  `Completed` and on `Cancelled`; `RunHooks::default()` changes nothing (the migrated tests).
- **TDD**: the seam tests are written red first, then the loop is reshaped until they pass; the
  existing loop tests move to the runner harness without changing an assertion.

## Out of Scope

- **No production hook consumer.** The seam ships unused: no permission gate, no redaction, no
  compaction, no config flag to enable any of them.
- **No hook on the display side.** `DisplayItem`/`Renderer`/`map_event`/`run_turn` are unchanged;
  a hook is a core-level seam, and no frontend gains a hook API.
- **No iteration-boundary hook.** The "continue to the next turn" point stays internal; `turn_end`
  is the only run-end boundary and is where compaction will mount.
- **No tool-execution semantics changes.** Parallel/serial dispatch, completion-order events and
  model-order history stay as they are, apart from the skip path.
- **No session or message-model changes.** The log format, the `AgentMessage` variants and the
  `to_llm`/`convert` step are untouched.
- **No async hooks** and no trait-based hook registry (`RunHooks` is three optional closures).

## Further Notes

- The design was settled with the user in a grilling round: payloads are `AgentMessage`, hooks may
  rewrite, the short-circuit must keep a call paired with a result, and hooks precede events. Those
  answers are what ADR-0015 records; the alternatives that were rejected are listed there.
- ADR-0011 D1 already claimed `AgentRunner`; this work makes the claim true and its implementation
  note stops describing a deviation.
- `RunResult`'s removal is the only place where existing test code changes shape rather than merely
  moving: the tests keep their assertions and gain a local harness.
