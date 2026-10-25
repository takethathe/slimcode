//! Agent runtime loop, folded from the ticket 04 prototype.
//!
//! Shape locked by `.scratch/slimcode-v1` ticket 04:
//! - the loop is a value, [`AgentRunner`] (ADR-0011 D1): one run's tools,
//!   config, cancel token and event subscription, with `run` driving the loop;
//!   ADR-0031 widens that seam into the seven-phase pipeline ([`PhaseHooks`]:
//!   `pre_turn` / `pre_chat` / `after_chat` / `pre_tool` / `after_tool` /
//!   `post_iteration` / `after_turn`) over an immutable [`RunSpec`] and a
//!   mutable [`RunState`] — the single loop's only extension seam, reached by
//!   the expand–contract move that retired ADR-0015's three hooks;
//! - loop: model response with `tool_calls` → execute tools → append tool
//!   results → loop, until the model stops calling tools;
//! - stop conditions: no tool_calls → `Completed`; user interrupt (cancel
//!   token) → `Cancelled` (the CLI/TUI layer concern);
//! - tool execution defaults to **parallel** (ADR-0010): a Tool batch's calls
//!   run on scoped threads, its tool events stream in completion order and its
//!   results enter history in model order; `RunConfig.parallel_tools` keeps a
//!   serial path for tests that assert the serial append semantics;
//! - tool errors are surfaced to the model as a `role: tool` message prefixed
//!   `Error: …` so it can recover naturally;
//! - deltas mirror the live Bailian wire shape (ticket 05): `reasoning` before
//!   `content`; a tool call starts with `ToolCallStart` (id + name) followed by
//!   `ToolCallArgs` fragments; `assemble` re-joins them.
//!
//! The `Provider` trait (in `slimcode-ai`) is the seam the Bailian/
//! OpenAI-compatible provider implements. It is synchronous so the loop stays
//! testable; the async boundary lives inside the
//! provider implementation (a blocking call / runtime handle).

use crate::session::convert;
pub use crate::session::{AgentMessage, Message, Role, ToolCall};
use serde::Serialize;
use serde_json::Value;
use std::any::{Any, TypeId};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

// The LLM seam types are owned by `slimcode-ai` (ADR-0011 D1) and re-exported
// here so callers can keep importing them from the runtime module.
pub use slimcode_ai::config::ProviderConfig;
pub use slimcode_ai::llm::{CancelToken, Delta, FinishReason, Provider, ToolSpec};
pub use slimcode_ai::trace::{SpanRecorder, Tracer};
pub use slimcode_ai::wire::TokenUsage;

/// A registered tool the agent can invoke: the schema the provider sees plus
/// the closure that executes it.
pub struct Tool {
    pub spec: ToolSpec,
    /// Runs the tool against parsed JSON arguments. `Err` becomes an
    /// `Error: …` tool message in history. `Send + Sync` so one batch of
    /// calls can share the same tools across the worker threads of a
    /// parallel dispatch (all tool closures capture owned data).
    pub run: Box<dyn Fn(Value) -> Result<String, String> + Send + Sync>,
}

impl Tool {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
        run: impl Fn(Value) -> Result<String, String> + Send + Sync + 'static,
    ) -> Self {
        Self {
            spec: ToolSpec::new(name, description, parameters),
            run: Box::new(run),
        }
    }
}

/// Reassemble an assistant message from a delta stream (provider-agnostic).
fn assemble(deltas: &[Delta]) -> (String, Vec<ToolCall>, FinishReason) {
    let mut text = String::new();
    let mut tcs: BTreeMap<usize, ToolCall> = BTreeMap::new();
    let mut reason = FinishReason::Stop;
    for d in deltas {
        match d {
            Delta::Text(t) => text.push_str(t),
            Delta::Reasoning(_) => {}
            Delta::ToolCallStart { index, id, name } => {
                tcs.entry(*index).or_insert_with(|| ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    arguments: String::new(),
                });
            }
            Delta::ToolCallArgs { index, fragment } => {
                if let Some(tc) = tcs.get_mut(index) {
                    tc.arguments.push_str(fragment);
                }
            }
            Delta::Done(fr) => reason = fr.clone(),
        }
    }
    (text, tcs.into_values().collect(), reason)
}

/// How a run ended.
// Serialized for the FFI wire contract (ADR-0023): the `stop` and
// `turn_finished` event payloads carry "completed" / "cancelled".
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The model produced a final answer with no tool calls.
    Completed,
    /// The caller requested a cancel (Esc in the TUI) at some boundary of the
    /// run. Whatever already streamed / was already applied stays; no error
    /// is implied and the stop renders nothing.
    Cancelled,
    /// The run hit the `max_iterations` guardrail and stopped under control at
    /// the iteration boundary (ticket 08). Only ever produced when
    /// [`RunConfig::max_iterations`] is `Some`; the code preset leaves it
    /// `None`, so this never arises on that path (the equivalence gate).
    MaxIterations,
    /// The run exhausted its wall-clock `timeout` budget and stopped under
    /// control at a loop boundary (ticket 08). Only produced when
    /// [`RunConfig::timeout`] is `Some`; unset on the code path.
    Timeout,
}

/// The loop's internal per-call dispatch decision: run it, or skip execution
/// and supply the result the model will see (ADR-0015 D4, lifted onto the
/// `pre_tool` phase in ADR-0031). A skip still yields a tool result, because
/// every `tool_call` must be paired with one on the next provider request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolDecision {
    /// Execute the tool.
    Run,
    /// Do not execute; use this result instead. `Err` becomes the `Error: …`
    /// shape tool failures already use.
    Skip(Result<String, String>),
}

/// Every observable thing the runtime emits during a run — the CLI renders this.
#[derive(Clone, Debug)]
pub enum AgentEvent {
    Turn {
        turn: usize,
    },
    /// Raw delta, forwarded live so the CLI can stream text / a thinking line.
    Stream(Delta),
    ToolStart {
        /// The model-emitted call id: renderers pair a start with its result
        /// by this id, so the same tool invoked several times in one batch
        /// (parallel execution) stays unambiguous.
        tool_call_id: String,
        name: String,
        arguments: String,
    },
    ToolResult {
        tool_call_id: String,
        name: String,
        ok: bool,
        result: String,
    },
    /// A message just entered history — the assembled assistant reply, and
    /// every tool result as it is pushed (ADR-0009 D2/D5). The session layer
    /// persists these as they happen; nothing here touches a file. Serial and
    /// parallel tool execution alike emit one event per result.
    ///
    /// `usage` is the provider's `chat` return for the request that produced
    /// this message: `Some` on the assistant replies (the Stop and ToolCalls
    /// branches alike), `None` on tool results, which never earn tokens
    /// (ticket 02).
    Message {
        message: AgentMessage,
        usage: Option<TokenUsage>,
    },
    Stop(StopReason),
}

#[derive(Clone, Debug)]
pub struct RunConfig {
    /// Execute multiple tool calls from one response concurrently, or one at a
    /// time (appending results as we go). Defaults to `true`: a Tool batch is
    /// dispatched on scoped threads, its events stream in completion order and
    /// its results enter history in model order.
    pub parallel_tools: bool,
    /// Optional cap on the number of loop iterations (provider calls) in one run
    /// (ticket 08). `None` (the code default) means no cap — a coding agent
    /// runs until the model stops calling tools, the caller cancels, a phase
    /// aborts, or the provider errors. `Some(n)` stops the run under control at
    /// the iteration boundary once `n` iterations have run, returning
    /// [`StopReason::MaxIterations`].
    pub max_iterations: Option<usize>,
    /// Optional wall-clock budget for one run (ticket 08). `None` (the code
    /// default) means no timeout. `Some(d)` stops the run under control at the
    /// next loop boundary once `d` has elapsed since the run began, returning
    /// [`StopReason::Timeout`]. Honored at boundaries only (like cancel), never
    /// mid-request.
    pub timeout: Option<Duration>,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            parallel_tools: true,
            max_iterations: None,
            timeout: None,
        }
    }
}

fn dispatch(tools: &[Tool], tc: &ToolCall) -> Result<String, String> {
    match tools.iter().find(|t| t.spec.name == tc.name) {
        Some(t) => {
            let args: Value = serde_json::from_str(&tc.arguments)
                .map_err(|e| format!("invalid tool arguments for {}: {e}", tc.name))?;
            (t.run)(args)
        }
        None => Err(format!("unknown tool: {}", tc.name)),
    }
}

/// A live event sink for the agent loop: invoked for every [`AgentEvent`] as
/// it is produced. `Err` aborts the loop (used to propagate renderer errors).
pub type EventSink<'a> = &'a mut dyn FnMut(AgentEvent) -> Result<(), String>;

// --- seven-phase pipeline (ADR-0031) --------------------------------------
//
// ADR-0031 lifts slimbot's proven phase pipeline into the single loop as its
// extension seam: `pre_turn / pre_chat / after_chat / pre_tool / after_tool /
// post_iteration / after_turn`, threaded over an immutable [`RunSpec`] and a
// mutable [`RunState`]. The expand–contract move that added it alongside
// ADR-0015's three hooks has now closed: those hooks are retired, so this is
// the loop's only seam. A runner with no phase hooks registered behaves
// byte-for-byte like the pre-pipeline loop (the equivalence gate), because
// every phase call is guarded by whether its list is empty.

/// Flow control a non-tool phase hook returns: keep going, or cut the run
/// short (ADR-0031 D2). `Abort` skips the rest of the current iteration and
/// the remaining non-`after_turn` phases, then jumps to `after_turn` (which
/// always runs); it is not an error and closes the run like a normal stop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HookAction {
    /// Proceed to the next hook / phase.
    Continue,
    /// Stop the run at this boundary, carrying a human-readable reason.
    Abort { reason: String },
}

/// Flow control a `pre_tool` hook returns for one call (ADR-0031 D2): run it,
/// skip execution and supply the result the model will see, or abort the run.
/// A `Skip` still yields a paired tool result (every `tool_call` must be
/// answered on the next request), and — like a skipped call under the old
/// `before_tool` — is still visited by `after_tool`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolHookAction {
    /// Execute the tool.
    Execute,
    /// Do not execute; use this result instead. `Err` becomes the `Error: …`
    /// shape tool failures already use.
    Skip { result: Result<String, String> },
    /// Stop the run before this batch dispatches, carrying a reason.
    Abort { reason: String },
}

/// The immutable collaborator bundle a phase hook reads (ADR-0031 D2): the
/// run's tools, config, provider settings, cancel token and system prompt.
/// Built once per run from the [`AgentRunner`]'s inputs; hooks receive it by
/// `&` and steer through [`HookAction`] / [`RunState`] rather than mutating a
/// collaborator. Its reference fields are `Copy` handles with the runner's
/// lifetime, so holding a `RunSpec` does not borrow the runner.
#[derive(Clone)]
pub struct RunSpec<'a> {
    /// The tools this run may invoke.
    pub tools: &'a [Tool],
    /// Parallel vs serial tool dispatch.
    pub cfg: RunConfig,
    /// The provider-owned settings handed to every `chat` call.
    pub provider_config: &'a ProviderConfig,
    /// The caller's cancellation handle.
    pub cancel: &'a CancelToken,
    /// The system prompt (read-only here; the history it pairs with lives in
    /// [`RunState::messages`]).
    pub system: Message,
}

/// A minimal owned type map: one value per concrete type, letting phase hooks
/// carry typed state across the phases of a turn without the kernel knowing
/// what those types are. Backs [`RunState::extensions`].
#[derive(Default)]
pub struct Extensions {
    map: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

// `Box<dyn Any>` has no `Debug`, so a manual impl reports only the shape.
impl std::fmt::Debug for Extensions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Extensions")
            .field("len", &self.map.len())
            .finish()
    }
}

impl Extensions {
    /// Insert `value`, returning the previous value of the same type, if any.
    pub fn insert<T: Any + Send + Sync>(&mut self, value: T) -> Option<T> {
        self.map
            .insert(TypeId::of::<T>(), Box::new(value))
            .and_then(|b| b.downcast::<T>().ok())
            .map(|b| *b)
    }

    /// Borrow the value of type `T`, if present.
    pub fn get<T: Any + Send + Sync>(&self) -> Option<&T> {
        self.map
            .get(&TypeId::of::<T>())
            .and_then(|b| b.downcast_ref::<T>())
    }

    /// Mutably borrow the value of type `T`, if present.
    pub fn get_mut<T: Any + Send + Sync>(&mut self) -> Option<&mut T> {
        self.map
            .get_mut(&TypeId::of::<T>())
            .and_then(|b| b.downcast_mut::<T>())
    }

    /// Remove and return the value of type `T`, if present.
    pub fn remove<T: Any + Send + Sync>(&mut self) -> Option<T> {
        self.map
            .remove(&TypeId::of::<T>())
            .and_then(|b| b.downcast::<T>().ok())
            .map(|b| *b)
    }
}

/// Stashed in [`RunState::extensions`] when a phase aborts, carrying the
/// [`HookAction::Abort`] / [`ToolHookAction::Abort`] reason so an `after_turn`
/// hook can read why the run was cut short.
#[derive(Clone, Debug)]
pub struct AbortReason(pub String);

/// The mutable state a run threads through the phases of one turn (ADR-0031
/// D2): the message history so far, the loop counters, and a typed
/// cross-phase [`Extensions`] map. Hooks receive it by `&mut` and may rewrite
/// [`RunState::messages`] or stash state in `extensions`.
#[derive(Debug)]
pub struct RunState {
    /// The history, including the latest user message and every entry the run
    /// has pushed so far. The value the caller finally stores.
    pub messages: Vec<AgentMessage>,
    /// The provider-call (turn) count so far — the number reported in
    /// [`AgentEvent::Turn`] and handed to the old `turn_end` hook.
    pub turn: usize,
    /// Loop iterations completed. Mirrors [`RunState::turn`] today; kept
    /// separate so a guardrail (ticket 08) can cap iterations independent of
    /// the reported turn number.
    pub iterations: usize,
    /// The message count the run started with: `messages.len()` at entry, the
    /// history already including the latest user message. The rollback cursor
    /// (ticket 08): a bot default `after_turn` hook truncates
    /// [`RunState::messages`] back to this on an interrupted turn. The kernel
    /// never rolls back on its own, so the code path is byte-identical.
    pub initial_message_count: usize,
    /// Cross-phase typed state, empty at the start of each turn.
    pub extensions: Extensions,
}

/// The `pre_turn` / `pre_chat` / `post_iteration` callback shape: read the
/// collaborators, steer via [`HookAction`] and [`RunState`].
pub type GateHook<'a> = Box<dyn FnMut(&RunSpec, &mut RunState) -> Result<HookAction, String> + 'a>;
/// The `after_chat` callback shape: also handed the assistant message about to
/// enter history so it can rewrite it.
pub type AfterChatHook<'a> =
    Box<dyn FnMut(&RunSpec, &mut RunState, &mut AgentMessage) -> Result<HookAction, String> + 'a>;
/// The `pre_tool` callback shape: per call (index), deciding Execute/Skip/Abort.
pub type PreToolHook<'a> =
    Box<dyn FnMut(&RunSpec, &mut RunState, usize) -> Result<ToolHookAction, String> + 'a>;
/// The `after_tool` callback shape: per result (in completion order) with its
/// `ok` flag; may rewrite, cannot veto.
pub type AfterToolHook<'a> =
    Box<dyn FnMut(&RunSpec, &mut RunState, &mut AgentMessage, bool) -> Result<(), String> + 'a>;
/// The `after_turn` callback shape: once at run end (Completed / Cancelled /
/// Abort) with the stop reason; always runs, cannot veto.
pub type AfterTurnHook<'a> =
    Box<dyn FnMut(&RunSpec, &mut RunState, &StopReason) -> Result<(), String> + 'a>;

/// A `pre_turn` hook: once, before the loop begins the turn.
pub struct PreTurn<'a>(pub GateHook<'a>);

/// A `pre_chat` hook: before each provider request, after the cancel gate.
pub struct PreChat<'a>(pub GateHook<'a>);

/// An `after_chat` hook: after each provider response, with the assistant
/// message about to enter history (it may rewrite `asst`; `Abort` vetoes it and
/// drops it from the run).
pub struct AfterChat<'a>(pub AfterChatHook<'a>);

/// A `pre_tool` hook: once per call in model order, the whole batch before
/// anything dispatches (even in parallel mode). The batch's assistant message
/// is the last entry in [`RunState::messages`] at this point (no results pushed
/// yet), so a hook rewrites it there; `index` is the call being decided. It
/// returns a [`ToolHookAction`] per call.
pub struct PreTool<'a>(pub PreToolHook<'a>);

/// An `after_tool` hook: as each tool result is about to enter history (in
/// completion order), with the result message and the `ok` outcome flag. It
/// may rewrite the message; it does not veto (returns `Result<()>`).
pub struct AfterTool<'a>(pub AfterToolHook<'a>);

/// A `post_iteration` hook: after each tool batch has been applied, before the
/// loop turns again.
pub struct PostIteration<'a>(pub GateHook<'a>);

/// An `after_turn` hook: once when the run stops — on Completed, Cancelled and
/// Abort alike, never on an errored run — with the final history and stop
/// reason. It always runs (ADR-0031 D2), carrying rollback / consolidation /
/// terminal events.
pub struct AfterTurn<'a>(pub AfterTurnHook<'a>);

/// The seven-phase hook bundle (ADR-0031 D2), all lists empty by default. Each
/// phase runs its hooks in registration order; `pre_turn` / `pre_chat` /
/// `after_chat` / `post_iteration` return [`HookAction`], `pre_tool` returns
/// [`ToolHookAction`] per call, `after_tool` and `after_turn` return
/// `Result<()>`. Registering a bot-only hook leaves the code path untouched
/// when the lists are empty.
#[derive(Default)]
pub struct PhaseHooks<'a> {
    pub pre_turn: Vec<PreTurn<'a>>,
    pub pre_chat: Vec<PreChat<'a>>,
    pub after_chat: Vec<AfterChat<'a>>,
    pub pre_tool: Vec<PreTool<'a>>,
    pub after_tool: Vec<AfterTool<'a>>,
    pub post_iteration: Vec<PostIteration<'a>>,
    pub after_turn: Vec<AfterTurn<'a>>,
}

/// How a dispatched Tool batch ended, from [`AgentRunner::execute_tools`]:
/// applied cleanly, stopped by a cancel, or cut short by a `pre_tool` `Abort`.
/// `Aborted` never arises with no phase hooks, so the pre-pipeline path is
/// unchanged.
enum BatchSignal {
    Done,
    Cancelled,
    Aborted,
}

/// The agent loop as a value (ADR-0011 D1): one run's tools, config, cancel
/// token and event subscription, with [`AgentRunner::run`] driving the loop
/// over a message history. The event subscription is the sink every
/// [`AgentEvent`] flows into; `slimcode-app`'s shared turn runner builds one
/// runner per turn. The extension seam is [`AgentRunner::phases`]
/// (ADR-0031).
pub struct AgentRunner<'a> {
    /// The tools this run may invoke.
    pub tools: &'a [Tool],
    /// The run's configuration (parallel vs serial tool dispatch).
    pub cfg: RunConfig,
    /// The provider-owned settings handed to every `chat` call (ADR-0016): the
    /// runner borrows the resolved config and passes it across the seam, so
    /// the provider instance itself stays stateless.
    pub provider_config: &'a ProviderConfig,
    /// The caller's cancellation handle.
    pub cancel: &'a CancelToken,
    /// The live event subscription.
    pub on_event: EventSink<'a>,
    /// The seven-phase pipeline (ADR-0031): the loop's only extension seam
    /// (defaults to all-empty, in which case the loop is byte-identical to the
    /// pre-pipeline behavior).
    pub phases: PhaseHooks<'a>,
    /// The tracing collaborator (ADR-0031 D4): when set, the kernel opens a
    /// RAII span at each operand call (`iteration`, `provider.chat`,
    /// `tool.{name}`). `None` (the code preset's default) makes every span a
    /// silent no-op, leaving the run byte-identical to an untraced one.
    pub tracer: Option<Arc<dyn Tracer>>,
}

impl<'a> AgentRunner<'a> {
    pub fn new(
        tools: &'a [Tool],
        cfg: RunConfig,
        provider_config: &'a ProviderConfig,
        cancel: &'a CancelToken,
        on_event: EventSink<'a>,
    ) -> Self {
        Self {
            tools,
            cfg,
            provider_config,
            cancel,
            on_event,
            phases: PhaseHooks::default(),
            tracer: None,
        }
    }

    /// Open a span named `name` through the injected [`Tracer`], or return
    /// `None` when no tracer is wired in (the no-op path). The returned
    /// [`SpanRecorder`] closes the span when it drops, so the caller keeps it
    /// alive for exactly the guarded operation.
    fn span(&self, name: &str) -> Option<SpanRecorder> {
        self.tracer.as_ref().and_then(|tracer| tracer.start(name))
    }

    /// Drive one run over `messages` (already including the latest user
    /// message), forwarding every event to the subscription as it happens,
    /// and return the updated history plus the stop reason. A subscription
    /// error aborts the run. Cancellation is honored at every runner boundary:
    /// before a provider call, right after it errors (a provider error that
    /// coincides with a cancel is a silent Cancelled stop, not a propagated
    /// error), after its deltas were streamed (a cancel that landed during the
    /// request discards the half text — no assistant message enters history),
    /// and before/after each tool call. Partial history (already-pushed tool
    /// results) is returned as-is with [`StopReason::Cancelled`].
    ///
    /// The system prompt is passed separately and never part of the history
    /// (ADR-0012 D3); the runtime prepends it to `convert` before every
    /// provider request.
    pub fn run<P: Provider>(
        &mut self,
        provider: &mut P,
        system: &Message,
        messages: Vec<AgentMessage>,
    ) -> Result<(Vec<AgentMessage>, StopReason), String> {
        // The provider sees tools as a schema only: build the `ToolSpec` list
        // once per run (not per request) and hand it to every `chat` call
        // (ADR-0011 D1).
        let tool_specs: Vec<ToolSpec> = self.tools.iter().map(|t| t.spec.clone()).collect();

        // The immutable collaborator bundle (ADR-0031 D2), built from `Copy`
        // reference handles and owned clones so it borrows nothing from `self`:
        // a phase hook reads `&spec` while the loop mutates its own fields
        // (disjoint field borrows).
        let spec = RunSpec {
            tools: self.tools,
            cfg: self.cfg.clone(),
            provider_config: self.provider_config,
            cancel: self.cancel,
            system: system.clone(),
        };

        // The mutable run state threaded through the phases. The history the
        // caller finally stores lives here. The rollback cursor is the entry
        // count (ticket 08): a bot `after_turn` hook can truncate back to it.
        let initial_message_count = messages.len();
        let mut state = RunState {
            messages,
            turn: 0,
            iterations: 0,
            initial_message_count,
            extensions: Extensions::default(),
        };

        // Guardrail deadline (ticket 08): only read off the clock when a timeout
        // is configured, so the code path (`None`) never calls `Instant::now` and
        // stays byte-identical to the uncapped loop.
        let deadline = self.cfg.timeout.map(|d| Instant::now() + d);

        // Whether a phase `Abort` cut the run short (rather than a model Stop
        // or a cancel). Surfaced to the run-end `after_turn` via
        // `extensions`; the returned `StopReason` stays `Completed` because the
        // wire contract has no abort variant (ADR-0023).
        let mut aborted = false;

        // pre_turn: once, before the loop opens the turn. An `Abort` here skips
        // the loop entirely and jumps to `after_turn`. An empty list is a no-op
        // (the equivalence gate).
        for hook in self.phases.pre_turn.iter_mut() {
            match (hook.0)(&spec, &mut state)? {
                HookAction::Continue => {}
                HookAction::Abort { reason } => {
                    state.extensions.insert(AbortReason(reason));
                    aborted = true;
                    break;
                }
            }
        }

        // Unbounded loop: a coding agent runs until the model stops calling
        // tools (`FinishReason::Stop`), the caller cancels, a phase aborts, or
        // the provider errors. There is intentionally no iteration cap — ending
        // a run is the model's job (no tool calls ⇒ final answer). Each `break`
        // carries the [`StopReason`] for that exit.
        let stop = if aborted {
            StopReason::Completed
        } else {
            'run: loop {
                // Boundary: a cancel between iterations (or before the first
                // request) stops before any provider call is made.
                if self.cancel.is_cancelled() {
                    break 'run StopReason::Cancelled;
                }

                // Guardrails (ticket 08): controlled stops at the iteration
                // boundary, honored only when configured. With both `None` (the
                // code default) these branches never fire, leaving the loop
                // byte-identical to the uncapped one (the equivalence gate).
                if let Some(max) = self.cfg.max_iterations
                    && state.iterations >= max
                {
                    break 'run StopReason::MaxIterations;
                }
                if let Some(deadline) = deadline
                    && Instant::now() >= deadline
                {
                    break 'run StopReason::Timeout;
                }

                // pre_chat: before each provider request (after the cancel
                // gate). An `Abort` ends the run without making the call.
                {
                    let mut cut = false;
                    for hook in self.phases.pre_chat.iter_mut() {
                        match (hook.0)(&spec, &mut state)? {
                            HookAction::Continue => {}
                            HookAction::Abort { reason } => {
                                state.extensions.insert(AbortReason(reason));
                                cut = true;
                                break;
                            }
                        }
                    }
                    if cut {
                        break 'run StopReason::Completed;
                    }
                }

                state.iterations += 1;
                state.turn = state.iterations;
                let turn = state.turn;
                // iteration span: one RAII span per loop pass, closed when the
                // pass ends (break / return / fall-through all drop it).
                let mut iteration_span = self.span("iteration");
                if let Some(s) = iteration_span.as_mut() {
                    s.record("turn", &turn.to_string());
                }
                on_event_call(&mut self.on_event, AgentEvent::Turn { turn })?;

                // Deltas are surfaced as the provider produces them (ADR-0019):
                // the sink emits each one the moment the provider parses it off
                // the wire, and the same list is what `assemble` turns into the
                // assistant message below — nothing is rendered twice.
                let mut deltas: Vec<Delta> = Vec::new();
                // A subscription failure is fatal even when it coincides with a
                // cancel: this flag keeps it from being swallowed as a silent
                // cancelled stop (a broken frontend must surface its error).
                let mut sink_failed = false;
                // provider.chat span: opened around the request so its
                // latency / attributes land even on a failed transport (the
                // recorder drops on every exit path below). With no tracer the
                // recorder is a no-op and `chat_traced`'s default makes the call
                // byte-identical to `chat` (the equivalence gate).
                let mut chat_span = self.span("provider.chat");
                if let Some(s) = chat_span.as_mut() {
                    s.record("model", &self.provider_config.model);
                }
                let mut noop_recorder = SpanRecorder::noop();
                let recorder: &mut SpanRecorder = chat_span.as_mut().unwrap_or(&mut noop_recorder);
                let result = provider.chat_traced(
                    &convert(system, &state.messages),
                    &tool_specs,
                    self.provider_config,
                    self.cancel,
                    &mut |delta: Delta| {
                        if let Err(e) =
                            on_event_call(&mut self.on_event, AgentEvent::Stream(delta.clone()))
                        {
                            sink_failed = true;
                            return Err(e);
                        }
                        deltas.push(delta);
                        Ok(())
                    },
                    recorder,
                );
                let usage = match result {
                    Ok(usage) => usage,
                    // A provider error that landed together with a cancel (its
                    // interruptible read aborted) is a silent cancelled stop,
                    // not an error.
                    Err(_) if self.cancel.is_cancelled() && !sink_failed => {
                        break 'run StopReason::Cancelled;
                    }
                    Err(e) => return Err(e),
                };
                if let (Some(s), Some(u)) = (chat_span.as_mut(), usage.as_ref()) {
                    s.record("prompt_tokens", &u.prompt_tokens.to_string());
                    s.record("completion_tokens", &u.completion_tokens.to_string());
                }
                // A cancel that landed during the request discards the half
                // message: the streamed deltas stay on the transcript, but no
                // assistant message enters history. The request's usage is
                // discarded with it — a cancelled request earned no tokens
                // (ticket 02).
                if self.cancel.is_cancelled() {
                    break 'run StopReason::Cancelled;
                }
                let (text, tool_calls, reason) = assemble(&deltas);
                let mut wire = Message::text(Role::Assistant, text);
                wire.tool_calls = tool_calls.clone();
                let mut asst = AgentMessage::Llm(wire);

                // after_chat: the assistant message about to enter history is
                // offered to the hooks, which may rewrite it (model / log /
                // display all see one text, ADR-0015 semantics). An `Abort`
                // vetoes the response and ends the run without pushing it.
                {
                    let mut cut = false;
                    for hook in self.phases.after_chat.iter_mut() {
                        match (hook.0)(&spec, &mut state, &mut asst)? {
                            HookAction::Continue => {}
                            HookAction::Abort { reason } => {
                                state.extensions.insert(AbortReason(reason));
                                cut = true;
                                break;
                            }
                        }
                    }
                    if cut {
                        break 'run StopReason::Completed;
                    }
                }
                state.messages.push(asst.clone());

                match reason {
                    FinishReason::Stop => {
                        // Announce the history entry right after it is pushed,
                        // so the session layer persists the message at the
                        // moment it exists (ADR-0009 D2). The request's usage
                        // rides along (ticket 02).
                        on_event_call(
                            &mut self.on_event,
                            AgentEvent::Message {
                                message: asst,
                                usage,
                            },
                        )?;
                        break 'run StopReason::Completed;
                    }
                    FinishReason::ToolCalls => {
                        // The assistant-message event is emitted inside
                        // `execute_tools`, after the tool-phase hooks have run:
                        // the session log, the display and the next request all
                        // carry the (possibly rewritten) message (ADR-0015 D3).
                        // The request's usage rides along with it (ticket 02).
                        match self.execute_tools(&spec, &mut state, usage)? {
                            BatchSignal::Cancelled => break 'run StopReason::Cancelled,
                            BatchSignal::Aborted => {
                                break 'run StopReason::Completed;
                            }
                            BatchSignal::Done => {}
                        }

                        // post_iteration: after this turn's tool batch landed,
                        // before the loop turns again. An `Abort` ends the run.
                        {
                            let mut cut = false;
                            for hook in self.phases.post_iteration.iter_mut() {
                                match (hook.0)(&spec, &mut state)? {
                                    HookAction::Continue => {}
                                    HookAction::Abort { reason } => {
                                        state.extensions.insert(AbortReason(reason));
                                        cut = true;
                                        break;
                                    }
                                }
                            }
                            if cut {
                                break 'run StopReason::Completed;
                            }
                        }
                        // fall through to the next turn (tool results are in
                        // history)
                    }
                }
            }
        };
        // The run-end `after_turn` phase: the history it leaves is exactly what
        // the caller stores. It runs on Completed and Cancelled (and after an
        // Abort), before the stop event, never on the errored path (an `Err`
        // returned above before reaching here).
        for hook in self.phases.after_turn.iter_mut() {
            (hook.0)(&spec, &mut state, &stop)?;
        }
        on_event_call(&mut self.on_event, AgentEvent::Stop(stop.clone()))?;
        Ok((state.messages, stop))
    }

    /// Build the tool-result message for a finished call, run the
    /// `after_tool` phase on it, and emit its `ToolStart`/`ToolResult` events
    /// (ADR-0015 D3: the phase and the events both happen before the log
    /// append). Returns the message to be pushed to history.
    fn build_result(
        &mut self,
        spec: &RunSpec,
        state: &mut RunState,
        tc: &ToolCall,
        res: Result<String, String>,
    ) -> Result<AgentMessage, String> {
        let ok = res.is_ok();
        let content = match &res {
            Ok(body) => body.clone(),
            Err(err) => format!("Error: {err}"),
        };
        let mut msg = AgentMessage::tool_result(&tc.id, content);
        // after_tool phase (ADR-0031): the result message, offered to the
        // pipeline before it enters history and before its events (a no-op when
        // unset).
        for hook in self.phases.after_tool.iter_mut() {
            (hook.0)(spec, state, &mut msg, ok)?;
        }
        // ToolStart carries the (possibly rewritten) arguments the tool ran
        // with; ToolResult carries the outcome and the final text the UI will
        // render.
        on_event_call(
            &mut self.on_event,
            AgentEvent::ToolStart {
                tool_call_id: tc.id.clone(),
                name: tc.name.clone(),
                arguments: tc.arguments.clone(),
            },
        )?;
        let text = msg.text_content();
        on_event_call(
            &mut self.on_event,
            AgentEvent::ToolResult {
                tool_call_id: tc.id.clone(),
                name: tc.name.clone(),
                ok,
                result: text,
            },
        )?;
        Ok(msg)
    }

    /// Execute the Tool batch carried by the assistant message at
    /// `state.messages[asst_index]` and append its results to history, emitting
    /// events through the subscription. Serial mode dispatches and appends one
    /// call at a time (so later tools can observe earlier results in history);
    /// parallel mode runs every call on its own scoped thread, streams tool
    /// events in **completion order**, then appends every result to history in
    /// **model order** (the `tool_calls` order) so the Session log stays
    /// deterministic.
    ///
    /// The `pre_tool` phase runs for the whole batch first (model order, loop
    /// thread) before anything dispatches, so it can rewrite a call's arguments
    /// or Skip it; a skipped call yields its supplied result without executing,
    /// and still produces a result message, events and an `after_tool` visit
    /// (ADR-0031 D2). An `Abort` cuts the run before any dispatch: the batch is
    /// stripped from the assistant message and the message is announced, so
    /// history stays call/result-paired (ADR-0015 D4) and the session layer
    /// persists exactly what remains.
    ///
    /// Checks the cancel token before each dispatch (and after each result, so
    /// a tool that aborted itself mid-run — e.g. a cancelled bash child — does
    /// not push its aborted result). Returns [`BatchSignal::Cancelled`] when a
    /// cancel stopped the batch before it finished, [`BatchSignal::Aborted`] when
    /// a `pre_tool` hook aborted (before any dispatch), else
    /// [`BatchSignal::Done`].
    fn execute_tools(
        &mut self,
        spec: &RunSpec,
        state: &mut RunState,
        usage: Option<TokenUsage>,
    ) -> Result<BatchSignal, String> {
        // The assistant message holding this batch is the last element at
        // entry; serial finishes push tool results after it, so its index is
        // captured once and reused for every re-read.
        let asst_index = state
            .messages
            .len()
            .checked_sub(1)
            .ok_or_else(|| "execute_tools: no assistant message".to_string())?;
        let calls: Vec<ToolCall> = state.messages[asst_index].tool_calls().to_vec();
        let mut effective: Vec<ToolCall> = calls.clone();
        let mut decisions: Vec<ToolDecision> = vec![ToolDecision::Run; calls.len()];

        // pre_tool phase (ADR-0031 D2): once per call in model order, the whole
        // batch before anything dispatches (parallel included). It may rewrite
        // the batch (seen as the last history entry), Skip a call, or Abort the
        // run. An empty list is a no-op.
        if !self.phases.pre_tool.is_empty() {
            for index in 0..effective.len() {
                let mut action = ToolHookAction::Execute;
                for hook in self.phases.pre_tool.iter_mut() {
                    action = (hook.0)(spec, state, index)?;
                    if matches!(action, ToolHookAction::Abort { .. }) {
                        break;
                    }
                }
                // Re-read the (possibly rewritten) call so the rewrite is what
                // dispatches.
                effective[index] = state.messages[asst_index]
                    .tool_calls()
                    .get(index)
                    .cloned()
                    .ok_or_else(|| format!("pre_tool removed tool call {index}"))?;
                match action {
                    ToolHookAction::Execute => {}
                    ToolHookAction::Skip { result } => {
                        decisions[index] = ToolDecision::Skip(result)
                    }
                    ToolHookAction::Abort { reason } => {
                        state.extensions.insert(AbortReason(reason));
                        // The batch will never dispatch: strip the dangling
                        // `tool_calls` so history does not end on a call with
                        // no paired result (the same invariant the shape check
                        // below guards with an `Err`, ADR-0015 D4), then
                        // announce the assistant entry exactly as it remains.
                        if let AgentMessage::Llm(message) = &mut state.messages[asst_index] {
                            message.tool_calls.clear();
                        }
                        on_event_call(
                            &mut self.on_event,
                            AgentEvent::Message {
                                message: state.messages[asst_index].clone(),
                                usage,
                            },
                        )?;
                        return Ok(BatchSignal::Aborted);
                    }
                }
            }
        }

        // A phase hook may rewrite a call's arguments but not the batch's shape:
        // an added or removed `tool_calls` entry would leave a call unpaired with
        // a result on the next request (ADR-0015 D4). Removals are caught
        // per-index above; this catches additions.
        let final_calls = state.messages[asst_index].tool_calls().len();
        if final_calls != calls.len() {
            return Err(format!(
                "a tool phase restructured the batch: {} -> {final_calls} tool calls",
                calls.len()
            ));
        }
        // Announce the assistant history entry now that the phases have run:
        // the session layer and the display see exactly the message that
        // stays in history (ADR-0015 D3, ADR-0009 D2). Its request's usage
        // rides along (ticket 02).
        on_event_call(
            &mut self.on_event,
            AgentEvent::Message {
                message: state.messages[asst_index].clone(),
                usage,
            },
        )?;

        if spec.cfg.parallel_tools {
            // True concurrency: each call gets its own scoped thread; skipped
            // calls' results are known at phase-1 time, so they complete first
            // (model order among themselves), then the dispatched results flow
            // back over a channel in completion order.
            if self.cancel.is_cancelled() {
                return Ok(BatchSignal::Cancelled);
            }
            let mut results: Vec<Option<Result<String, String>>> =
                calls.iter().map(|_| None).collect();
            let mut completion_order: Vec<usize> = Vec::new();
            for (index, decision) in decisions.iter().enumerate() {
                if let ToolDecision::Skip(res) = decision {
                    results[index] = Some(res.clone());
                    completion_order.push(index);
                }
            }
            let shared_cancel = self.cancel.clone();
            let tools = self.tools;
            let tracer = self.tracer.clone();
            std::thread::scope(|scope| {
                let (tx, rx) = std::sync::mpsc::channel::<(usize, Result<String, String>)>();
                for (index, tc) in effective.iter().enumerate() {
                    if matches!(decisions[index], ToolDecision::Skip(_)) {
                        continue;
                    }
                    let tx = tx.clone();
                    let cancel = shared_cancel.clone();
                    let tracer = tracer.clone();
                    scope.spawn(move || {
                        // A call whose batch was cancelled before it started
                        // never dispatches (and so never reports).
                        if cancel.is_cancelled() {
                            return;
                        }
                        // tool.{name} span around the operand call.
                        let mut tool_span = tracer
                            .as_ref()
                            .and_then(|t| t.start(&format!("tool.{}", tc.name)));
                        let res = dispatch(tools, tc);
                        if let Some(s) = tool_span.as_mut() {
                            s.record("ok", if res.is_ok() { "true" } else { "false" });
                        }
                        let _ = tx.send((index, res));
                    });
                }
                drop(tx);
                for (index, res) in rx {
                    results[index] = Some(res);
                    completion_order.push(index);
                }
            });
            // A cancel anywhere in the batch discards the whole batch, so
            // nothing is emitted for it: the tool events and the history stay
            // consistent (the UI never shows a block the Session log did not
            // receive).
            if self.cancel.is_cancelled() {
                return Ok(BatchSignal::Cancelled);
            }
            // Tool events (and the after_tool hook) in completion order...
            let mut final_msgs: Vec<Option<AgentMessage>> = calls.iter().map(|_| None).collect();
            for &index in &completion_order {
                let res = results[index]
                    .take()
                    .expect("completion order carries a result");
                final_msgs[index] = Some(self.build_result(spec, state, &effective[index], res)?);
            }
            // ...and history entries in model order (index), keeping the
            // Session log deterministic regardless of which call finished
            // first. A call with no result (only possible if a tool resets the
            // cancel token mid-run) is skipped rather than panicking.
            for slot in final_msgs.iter_mut() {
                if let Some(msg) = slot.take() {
                    state.messages.push(msg.clone());
                    on_event_call(
                        &mut self.on_event,
                        AgentEvent::Message {
                            message: msg,
                            usage: None,
                        },
                    )?;
                }
            }
        } else {
            // Serial: dispatch, hook, emit and append one call at a time.
            for index in 0..calls.len() {
                if self.cancel.is_cancelled() {
                    return Ok(BatchSignal::Cancelled);
                }
                let res = match &decisions[index] {
                    ToolDecision::Run => {
                        // tool.{name} span around the operand call.
                        let mut tool_span = self.span(&format!("tool.{}", effective[index].name));
                        let res = dispatch(self.tools, &effective[index]);
                        if let Some(s) = tool_span.as_mut() {
                            s.record("ok", if res.is_ok() { "true" } else { "false" });
                        }
                        if self.cancel.is_cancelled() {
                            return Ok(BatchSignal::Cancelled);
                        }
                        res
                    }
                    ToolDecision::Skip(res) => res.clone(),
                };
                let msg = self.build_result(spec, state, &effective[index], res)?;
                state.messages.push(msg.clone());
                on_event_call(
                    &mut self.on_event,
                    AgentEvent::Message {
                        message: msg,
                        usage: None,
                    },
                )?;
            }
        }
        Ok(BatchSignal::Done)
    }
}

/// Invoke the event subscription once (a `&mut dyn FnMut` stored in a field).
fn on_event_call(sink: &mut EventSink<'_>, event: AgentEvent) -> Result<(), String> {
    sink(event)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    // --- helpers -----------------------------------------------------------

    fn r(t: &str) -> Delta {
        Delta::Reasoning(t.to_string())
    }
    fn t(t: &str) -> Delta {
        Delta::Text(t.to_string())
    }
    fn tc_start(index: usize, id: &str, name: &str) -> Delta {
        Delta::ToolCallStart {
            index,
            id: id.to_string(),
            name: name.to_string(),
        }
    }
    fn tc_args(index: usize, frag: &str) -> Delta {
        Delta::ToolCallArgs {
            index,
            fragment: frag.to_string(),
        }
    }
    fn done_stop() -> Delta {
        Delta::Done(FinishReason::Stop)
    }
    fn done_tools() -> Delta {
        Delta::Done(FinishReason::ToolCalls)
    }
    fn default_system() -> Message {
        Message::text(Role::System, "be helpful")
    }

    /// Scripted provider: pops the next delta sequence per call.
    struct FakeProvider {
        script: Vec<Vec<Delta>>,
        calls: usize,
        /// Optional: cancel the shared token before returning the Nth call's
        /// batch (1-based), simulating Esc arriving mid-request.
        cancel_on_call: Option<usize>,
        cancel: Option<CancelToken>,
        /// Per-call usage returned from `chat` (default zero; `with_usage`
        /// sets it) — the seam that lets tests assert the usage riding on the
        /// assistant `Message` events (ticket 02).
        usage: TokenUsage,
    }

    impl FakeProvider {
        fn new(script: Vec<Vec<Delta>>) -> Self {
            Self {
                script,
                calls: 0,
                cancel_on_call: None,
                cancel: None,
                usage: TokenUsage::default(),
            }
        }

        fn cancels_on(mut self, call: usize, token: &CancelToken) -> Self {
            self.cancel_on_call = Some(call);
            self.cancel = Some(token.clone());
            self
        }

        fn with_usage(mut self, usage: TokenUsage) -> Self {
            self.usage = usage;
            self
        }
    }

    impl Provider for FakeProvider {
        fn chat(
            &mut self,
            _messages: &[Message],
            _tools: &[ToolSpec],
            _config: &ProviderConfig,
            _cancel: &CancelToken,
            on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
        ) -> Result<Option<TokenUsage>, String> {
            self.calls += 1;
            if let (Some(cancel), Some(n)) = (&self.cancel, self.cancel_on_call)
                && self.calls == n
            {
                cancel.cancel();
            }
            let d = self.script.get(self.calls - 1).cloned().unwrap_or_default();
            // A scripted provider mirrors the live one (ADR-0019): deltas go
            // through the sink, in order, as the request produces them.
            for delta in d {
                on_delta(delta)?;
            }
            Ok(Some(self.usage))
        }
    }

    fn weather_tool() -> Tool {
        Tool::new(
            "get_weather",
            "Get current weather for a city",
            serde_json::json!({"type": "object"}),
            |args: Value| {
                let city = args.get("city").and_then(Value::as_str).unwrap_or("?");
                Ok(format!("{{\"city\": \"{city}\", \"temp\": \"25C\"}}"))
            },
        )
    }

    fn fragile_tool() -> Tool {
        Tool::new(
            "fragile_tool",
            "Fails for Tokyo",
            serde_json::json!({"type": "object"}),
            |args: Value| {
                let city = args.get("city").and_then(Value::as_str).unwrap_or("?");
                if city == "Tokyo" {
                    Err(format!("no weather station for {city}"))
                } else {
                    Ok(format!("{{\"city\": \"{city}\", \"temp\": \"18C\"}}"))
                }
            },
        )
    }

    /// A provider that keeps calling the `get_weather` tool forever, sleeping a
    /// little on each call. Only a guardrail (or cancel) can end a run against
    /// it — used by the timeout test (ticket 08).
    struct LoopingSleepProvider {
        calls: usize,
        sleep: Duration,
    }

    impl Provider for LoopingSleepProvider {
        fn chat(
            &mut self,
            _messages: &[Message],
            _tools: &[ToolSpec],
            _config: &ProviderConfig,
            _cancel: &CancelToken,
            on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
        ) -> Result<Option<TokenUsage>, String> {
            std::thread::sleep(self.sleep);
            self.calls += 1;
            on_delta(Delta::ToolCallStart {
                index: 0,
                id: format!("call_{}", self.calls),
                name: "get_weather".to_string(),
            })?;
            on_delta(Delta::ToolCallArgs {
                index: 0,
                fragment: "{\"city\": \"Beijing\"}".to_string(),
            })?;
            on_delta(Delta::Done(FinishReason::ToolCalls))?;
            Ok(None)
        }
    }

    /// The loop test harness with no phase hooks; events are collected so
    /// tests assert external behaviour only.
    ///
    /// The harness runs with a fixed test config: these loop tests exercise
    /// the agent loop, not config resolution (that lives in `slimcode-app`).
    fn run_with(
        provider: &mut impl Provider,
        tools: &[Tool],
        cfg: RunConfig,
        cancel: &CancelToken,
        system: &Message,
        messages: Vec<AgentMessage>,
    ) -> Result<(Vec<AgentMessage>, StopReason, Vec<AgentEvent>), String> {
        let config = ProviderConfig::new("test-key", "https://example.invalid/v1", "test-model");
        let mut events: Vec<AgentEvent> = Vec::new();
        let (messages, stop) = {
            let mut sink = |e: AgentEvent| {
                events.push(e);
                Ok(())
            };
            let mut runner = AgentRunner::new(tools, cfg, &config, cancel, &mut sink);
            runner.run(provider, system, messages)?
        };
        Ok((messages, stop, events))
    }

    /// The standard one-shot script: system "be helpful", user
    /// "weather in Beijing?", fresh cancel token.
    fn run(
        script: Vec<Vec<Delta>>,
        tools: Vec<Tool>,
        cfg: RunConfig,
    ) -> Result<(Vec<AgentMessage>, usize, StopReason, Vec<AgentEvent>), String> {
        let mut p = FakeProvider::new(script);
        let system = Message::text(Role::System, "be helpful");
        let messages = vec![AgentMessage::text(Role::User, "weather in Beijing?")];
        let (messages, stop, events) =
            run_with(&mut p, &tools, cfg, &CancelToken::new(), &system, messages)?;
        let turns = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::Turn { .. }))
            .count();
        Ok((messages, turns, stop, events))
    }

    // --- assemble ----------------------------------------------------------

    #[test]
    fn assemble_joins_text_and_ignores_reasoning() {
        let (text, tcs, reason) = assemble(&[r("think"), t("hello "), t("world"), done_stop()]);
        assert_eq!(text, "hello world");
        assert!(tcs.is_empty());
        assert_eq!(reason, FinishReason::Stop);
    }

    #[test]
    fn assemble_reconstructs_tool_calls_from_fragments() {
        let deltas = vec![
            tc_start(0, "call_1", "get_weather"),
            tc_args(0, "{\"city\": \"Beij"),
            tc_args(0, "ing\"}"),
            done_tools(),
        ];
        let (_, tcs, reason) = assemble(&deltas);
        assert_eq!(reason, FinishReason::ToolCalls);
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0].id, "call_1");
        assert_eq!(tcs[0].name, "get_weather");
        assert_eq!(tcs[0].arguments, "{\"city\": \"Beijing\"}");
    }

    #[test]
    fn assemble_handles_two_parallel_tool_calls() {
        let deltas = vec![
            tc_start(0, "call_1", "get_weather"),
            tc_args(0, "{\"city\": \"Beijing\"}"),
            tc_start(1, "call_2", "get_time"),
            tc_args(1, "{\"city\": \"Shanghai\"}"),
            done_tools(),
        ];
        let (_, tcs, _) = assemble(&deltas);
        assert_eq!(tcs.len(), 2);
        assert_eq!(tcs[0].name, "get_weather");
        assert_eq!(tcs[1].name, "get_time");
    }

    // --- loop behavior -----------------------------------------------------

    // --- guardrails (ticket 08) ---------------------------------------------

    /// A `max_iterations` cap stops the run under control after exactly that
    /// many provider calls, with a distinguishable stop reason.
    #[test]
    fn max_iterations_stops_the_run_under_control() {
        // A provider that keeps calling tools: only the cap can end the run.
        let call = vec![
            tc_start(0, "call_1", "get_weather"),
            tc_args(0, "{\"city\": \"Beijing\"}"),
            done_tools(),
        ];
        let script = vec![call; 8];
        let cfg = RunConfig {
            parallel_tools: true,
            max_iterations: Some(3),
            timeout: None,
        };
        let (messages, turns, stop, _) = run(script, vec![weather_tool()], cfg).unwrap();
        assert_eq!(stop, StopReason::MaxIterations);
        assert_eq!(turns, 3, "exactly max_iterations provider calls run");
        // 3 assistant tool-call messages + 3 tool results + the initial user.
        assert_eq!(messages.len(), 7);
    }

    /// With no cap set (the code default) the same never-stopping script is
    /// only ended by the model's final `Stop` — the loop stays uncapped.
    #[test]
    fn no_max_iterations_leaves_the_loop_uncapped() {
        let script = vec![
            vec![
                tc_start(0, "call_1", "get_weather"),
                tc_args(0, "{\"city\": \"Beijing\"}"),
                done_tools(),
            ],
            vec![t("done"), done_stop()],
        ];
        let (_, turns, stop, _) = run(script, vec![weather_tool()], RunConfig::default()).unwrap();
        assert_eq!(stop, StopReason::Completed);
        assert_eq!(turns, 2);
    }

    /// A wall-clock `timeout` stops the run under control at the next loop
    /// boundary once the budget has elapsed, with its own stop reason.
    #[test]
    fn timeout_stops_the_run_under_control() {
        let cfg = RunConfig {
            parallel_tools: true,
            max_iterations: None,
            timeout: Some(Duration::from_millis(10)),
        };
        let mut provider = LoopingSleepProvider {
            calls: 0,
            sleep: Duration::from_millis(30),
        };
        let system = Message::text(Role::System, "be helpful");
        let messages = vec![AgentMessage::text(Role::User, "go")];
        let (_, stop, events) = run_with(
            &mut provider,
            &[weather_tool()],
            cfg,
            &CancelToken::new(),
            &system,
            messages,
        )
        .unwrap();
        assert_eq!(stop, StopReason::Timeout);
        let turns = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::Turn { .. }))
            .count();
        // The first call (30ms) already overruns the 10ms budget, so the run
        // stops at the second boundary after exactly one turn.
        assert_eq!(turns, 1);
    }

    #[test]
    fn single_turn_no_tools_completes() {
        let (messages, turns, stop, _) = run(
            vec![vec![t("Beijing is sunny."), done_stop()]],
            vec![weather_tool()],
            RunConfig::default(),
        )
        .unwrap();
        assert_eq!(stop, StopReason::Completed);
        assert_eq!(turns, 1);
        assert_eq!(messages.len(), 2); // user + assistant (system is not history)
    }

    #[test]
    fn tool_call_then_answer_runs_two_turns() {
        let script = vec![
            vec![
                tc_start(0, "call_1", "get_weather"),
                tc_args(0, "{\"city\": \"Beijing\"}"),
                done_tools(),
            ],
            vec![t("Beijing is 25C."), done_stop()],
        ];
        let (messages, turns, stop, events) =
            run(script, vec![weather_tool()], RunConfig::default()).unwrap();
        assert_eq!(stop, StopReason::Completed);
        assert_eq!(turns, 2);
        // user + assistant(tool_calls) + tool + assistant(final): the
        // system prompt is not part of the history (ADR-0012 D3)
        assert_eq!(messages.len(), 4);
        // the tool message carries the result
        let tool_msg = messages.iter().find(|m| m.role() == &Role::Tool).unwrap();
        assert_eq!(tool_msg.tool_call_id(), Some("call_1"));
        assert!(tool_msg.text_content().contains("25C"));
        // events expose the full surface
        assert!(events.iter().any(
            |e| matches!(e, AgentEvent::ToolResult { name, ok: true, .. } if name == "get_weather")
        ));
    }

    // --- per-message events (ticket 02) ------------------------------------

    #[test]
    fn message_events_announce_every_history_entry() {
        // A tool-calling turn: the assembled assistant message and each tool
        // result must be announced after entering history (one event each),
        // in history order.
        let script = vec![
            vec![
                tc_start(0, "call_1", "get_weather"),
                tc_args(0, "{\"city\": \"Beijing\"}"),
                done_tools(),
            ],
            vec![t("Beijing is 25C."), done_stop()],
        ];
        let (messages, _, _, events) =
            run(script, vec![weather_tool()], RunConfig::default()).unwrap();
        let announced: Vec<&AgentMessage> = events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::Message { message, .. } => Some(message),
                _ => None,
            })
            .collect();
        // The announced messages are exactly the ones that entered history.
        assert_eq!(
            announced.iter().map(|m| (*m).clone()).collect::<Vec<_>>(),
            messages[1..]
        );
    }

    #[test]
    fn usage_rides_on_assistant_messages_only() {
        // Ticket 02: the provider's `chat` usage is attached to the assistant
        // Message events — the Stop and ToolCalls branches alike — and tool
        // results never carry one.
        let usage = TokenUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
            prompt_tokens_details: None,
        };
        let script = vec![
            vec![
                tc_start(0, "call_1", "get_weather"),
                tc_args(0, "{}"),
                done_tools(),
            ],
            vec![t("25C"), done_stop()],
        ];
        let mut p = FakeProvider::new(script).with_usage(usage);
        let tools = vec![weather_tool()];
        let system = Message::text(Role::System, "be helpful");
        let messages = vec![AgentMessage::text(Role::User, "weather in Beijing?")];
        let (_, _, events) = run_with(
            &mut p,
            &tools,
            RunConfig::default(),
            &CancelToken::new(),
            &system,
            messages,
        )
        .unwrap();
        let pairs: Vec<Option<TokenUsage>> = events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::Message { usage, .. } => Some(*usage),
                _ => None,
            })
            .collect();
        // The tool-call reply and the final answer carry the request usage; the
        // tool result in between carries none.
        assert_eq!(pairs, vec![Some(usage), None, Some(usage)]);
    }

    #[test]
    fn message_events_sit_after_stream_and_before_stop() {
        // The full event sequence for one tool-calling turn: the assistant
        // message event comes after its streamed deltas and before the tool
        // start; each tool result event follows its own ToolStart/ToolResult;
        // the stop is last.
        let script = vec![
            vec![
                t("checking "),
                tc_start(0, "call_1", "get_weather"),
                tc_args(0, "{}"),
                done_tools(),
            ],
            vec![t("done"), done_stop()],
        ];
        let (_, _, _, events) = run(script, vec![weather_tool()], RunConfig::default()).unwrap();
        let kinds: Vec<&str> = events
            .iter()
            .map(|e| match e {
                AgentEvent::Turn { .. } => "turn",
                AgentEvent::Stream(_) => "stream",
                AgentEvent::ToolStart { .. } => "tool-start",
                AgentEvent::ToolResult { .. } => "tool-result",
                AgentEvent::Message { message, .. } => {
                    if message.role() == &Role::Assistant {
                        "message-assistant"
                    } else {
                        "message-tool"
                    }
                }
                AgentEvent::Stop(_) => "stop",
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                "turn",
                "stream",
                "stream",
                "stream",
                "stream",
                "message-assistant",
                "tool-start",
                "tool-result",
                "message-tool",
                "turn",
                "stream",
                "stream",
                "message-assistant",
                "stop",
            ]
        );
    }

    #[test]
    fn deltas_reach_the_sink_before_the_provider_call_returns() {
        // ADR-0019: the provider pushes deltas into the runner's sink while its
        // request is still open, so a frontend renders a partial answer instead
        // of receiving the whole batch after the body ends. The fake sleeps
        // after its first delta and only then reports the request as over: the
        // sink must already have seen that delta.
        struct SlowProvider {
            returned: Arc<AtomicBool>,
        }
        impl Provider for SlowProvider {
            fn chat(
                &mut self,
                _m: &[Message],
                _t: &[ToolSpec],
                _cfg: &ProviderConfig,
                _c: &CancelToken,
                on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
            ) -> Result<Option<TokenUsage>, String> {
                on_delta(t("half "))?;
                std::thread::sleep(std::time::Duration::from_millis(150));
                self.returned.store(true, Ordering::SeqCst);
                on_delta(t("answer"))?;
                on_delta(done_stop())?;
                Ok(Some(TokenUsage::default()))
            }
        }

        let returned = Arc::new(AtomicBool::new(false));
        let mut provider = SlowProvider {
            returned: returned.clone(),
        };
        let config = ProviderConfig::new("test-key", "https://example.invalid/v1", "test-model");
        let cancel = CancelToken::new();
        let tools: [Tool; 0] = [];
        // `(fragment, had the provider returned yet?)` per streamed delta.
        let mut seen: Vec<(String, bool)> = Vec::new();
        {
            let mut sink = |e: AgentEvent| {
                if let AgentEvent::Stream(Delta::Text(text)) = &e {
                    seen.push((text.clone(), returned.load(Ordering::SeqCst)));
                }
                Ok(())
            };
            let mut runner =
                AgentRunner::new(&tools, RunConfig::default(), &config, &cancel, &mut sink);
            runner
                .run(
                    &mut provider,
                    &Message::text(Role::System, "sys"),
                    vec![AgentMessage::text(Role::User, "hi")],
                )
                .unwrap();
        }
        assert_eq!(
            seen,
            vec![("half ".to_string(), false), ("answer".to_string(), true)],
            "the first fragment must reach the sink before `chat` returns"
        );
    }

    #[test]
    fn tool_events_carry_the_tool_call_id() {
        // ToolStart/ToolResult must carry the call's id so renderers can pair
        // them even when the same tool is called several times in one batch
        // (parallel execution).
        let script = vec![
            vec![
                tc_start(0, "call_1", "get_weather"),
                tc_args(0, "{\"city\": \"Beijing\"}"),
                done_tools(),
            ],
            vec![t("Beijing is 25C."), done_stop()],
        ];
        let (_, _, _, events) = run(script, vec![weather_tool()], RunConfig::default()).unwrap();
        let start_id = events
            .iter()
            .find_map(|e| match e {
                AgentEvent::ToolStart {
                    tool_call_id, name, ..
                } if name == "get_weather" => Some(tool_call_id.clone()),
                _ => None,
            })
            .expect("tool start event");
        assert_eq!(start_id, "call_1");
        let result_id = events
            .iter()
            .find_map(|e| match e {
                AgentEvent::ToolResult {
                    tool_call_id, name, ..
                } if name == "get_weather" => Some(tool_call_id.clone()),
                _ => None,
            })
            .expect("tool result event");
        assert_eq!(result_id, "call_1");
    }

    #[test]
    fn parallel_tools_executes_all_calls() {
        let script = vec![
            vec![
                tc_start(0, "call_1", "get_weather"),
                tc_args(0, "{\"city\": \"Beijing\"}"),
                tc_start(1, "get_time_idx", "get_weather"),
                tc_args(1, "{\"city\": \"Shanghai\"}"),
                done_tools(),
            ],
            vec![t("both done"), done_stop()],
        ];
        let (messages, _, _, _) = run(
            script,
            vec![weather_tool()],
            RunConfig {
                parallel_tools: true,
                ..Default::default()
            },
        )
        .unwrap();
        let tool_msgs: Vec<_> = messages
            .iter()
            .filter(|m| m.role() == &Role::Tool)
            .collect();
        assert_eq!(
            tool_msgs.len(),
            2,
            "both parallel calls' results are in history"
        );
    }

    #[test]
    fn parallel_tools_run_concurrently() {
        // True concurrency's observable signal: two blocking calls overlap, so
        // the peak number of simultaneously running tools reaches 2. A
        // sequential dispatcher (the old "batched results" parallel path) can
        // never exceed a peak of 1.
        let running = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let peak = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let blocking_tool = |name: &'static str| {
            let running = running.clone();
            let peak = peak.clone();
            Tool::new(name, "blocks briefly", serde_json::json!({}), move |_| {
                let cur = running.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                peak.fetch_max(cur, std::sync::atomic::Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(60));
                running.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                Ok(format!("{name} done"))
            })
        };
        let script = vec![
            vec![
                tc_start(0, "call_0", "tool_a"),
                tc_args(0, "{}"),
                tc_start(1, "call_1", "tool_b"),
                tc_args(1, "{}"),
                done_tools(),
            ],
            vec![t("done"), done_stop()],
        ];
        let (_, _, stop, _) = run(
            script,
            vec![blocking_tool("tool_a"), blocking_tool("tool_b")],
            RunConfig {
                parallel_tools: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(stop, StopReason::Completed);
        let peak = peak.load(std::sync::atomic::Ordering::SeqCst);
        assert!(peak >= 2, "peak concurrency was {peak}, not concurrent");
    }

    #[test]
    fn parallel_history_follows_model_order_while_events_follow_completion_order() {
        // Index 1 finishes first (it signals index 0). History must still be
        // appended in model order, while the streamed events reflect real
        // completion order.
        let fast_done = Arc::new(AtomicBool::new(false));
        let signal = fast_done.clone();
        let fast = Tool::new(
            "fast_tool",
            "returns at once",
            serde_json::json!({}),
            move |_| {
                signal.store(true, Ordering::SeqCst);
                Ok("fast".to_string())
            },
        );
        let wait = fast_done.clone();
        let slow = Tool::new(
            "slow_tool",
            "waits for fast_tool",
            serde_json::json!({}),
            move |_| {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
                while !wait.load(Ordering::SeqCst) {
                    if std::time::Instant::now() > deadline {
                        return Err("slow_tool starved: calls did not overlap".to_string());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
                Ok("slow".to_string())
            },
        );
        let script = vec![
            vec![
                tc_start(0, "call_0", "slow_tool"),
                tc_args(0, "{}"),
                tc_start(1, "call_1", "fast_tool"),
                tc_args(1, "{}"),
                done_tools(),
            ],
            vec![t("done"), done_stop()],
        ];
        let (messages, _, _, events) = run(
            script,
            vec![slow, fast],
            RunConfig {
                parallel_tools: true,
                ..Default::default()
            },
        )
        .unwrap();

        // History (and thus the Session log) is deterministic: model order.
        let history_ids: Vec<&str> = messages
            .iter()
            .filter(|m| m.role() == &Role::Tool)
            .map(|m| m.tool_call_id().expect("tool result has an id"))
            .collect();
        assert_eq!(history_ids, vec!["call_0", "call_1"]);

        // Events stream in completion order: the fast call reports first.
        let result_ids: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::ToolResult { tool_call_id, .. } => Some(tool_call_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(result_ids, vec!["call_1", "call_0"]);
    }

    #[test]
    fn tool_error_surfaces_as_error_prefix_and_loop_continues() {
        let script = vec![
            vec![
                tc_start(0, "call_1", "fragile_tool"),
                tc_args(0, "{\"city\": \"Tokyo\"}"),
                done_tools(),
            ],
            vec![t("No station for Tokyo."), done_stop()],
        ];
        let (messages, _, stop, _) =
            run(script, vec![fragile_tool()], RunConfig::default()).unwrap();
        assert_eq!(stop, StopReason::Completed);
        let tool_msg = messages.iter().find(|m| m.role() == &Role::Tool).unwrap();
        assert!(
            tool_msg.text_content().starts_with("Error: "),
            "got: {}",
            tool_msg.text_content()
        );
        // the final assistant answer still comes through
        let last = messages.last().unwrap();
        assert_eq!(last.role(), &Role::Assistant);
        assert!(last.text_content().contains("Tokyo"));
    }

    #[test]
    fn runaway_loop_keeps_going_until_model_stops() {
        // A model that never stops calling tools keeps the loop alive; there is
        // no iteration cap. It ends only when the model finally returns no
        // tool calls (Completed), well past the old cap of 3.
        let mut script = Vec::new();
        for i in 0..5 {
            script.push(vec![
                tc_start(0, &format!("call_{i}"), "get_weather"),
                tc_args(0, "{\"city\": \"Beijing\"}"),
                done_tools(),
            ]);
        }
        script.push(vec![t("done"), done_stop()]);
        let (messages, turns, stop, _) =
            run(script, vec![weather_tool()], RunConfig::default()).unwrap();
        assert_eq!(stop, StopReason::Completed);
        assert_eq!(turns, 6);
        assert_eq!(messages.len(), 1 /*user*/ + 6 + 5);
    }

    #[test]
    fn run_continues_existing_history() {
        // A restored session already has an old assistant reply; the frontend
        // appends a new user message and continues from there. The system
        // prompt is passed separately and never part of the history
        // (ADR-0012 D3).
        let system = Message::text(Role::System, "be helpful");
        let history = vec![
            AgentMessage::text(Role::Assistant, "Earlier answer."),
            AgentMessage::text(Role::User, "Now: what is the weather?"),
        ];
        let script = vec![
            vec![
                tc_start(0, "call_1", "get_weather"),
                tc_args(0, "{\"city\": \"Beijing\"}"),
                done_tools(),
            ],
            vec![t("Beijing is 25C."), done_stop()],
        ];
        let mut p = FakeProvider::new(script);
        let tools = [weather_tool()];
        let (messages, stop, _) = run_with(
            &mut p,
            &tools,
            RunConfig::default(),
            &CancelToken::new(),
            &system,
            history.clone(),
        )
        .unwrap();
        assert_eq!(stop, StopReason::Completed);
        // old history preserved + assistant(tool_calls) + tool + assistant(final)
        assert_eq!(messages.len(), history.len() + 3);
        assert_eq!(messages[0].role(), &Role::Assistant);
        assert!(messages[1].text_content().contains("weather"));
        // provider received the system + full history on its first chat call
        assert_eq!(p.calls, 2);
    }

    #[test]
    fn provider_error_aborts_the_run() {
        struct ErrProvider;
        impl Provider for ErrProvider {
            fn chat(
                &mut self,
                _m: &[Message],
                _t: &[ToolSpec],
                _cfg: &ProviderConfig,
                _c: &CancelToken,
                _on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
            ) -> Result<Option<TokenUsage>, String> {
                Err("provider exploded".to_string())
            }
        }
        let mut p = ErrProvider;
        let tools: [Tool; 0] = [];
        let err = run_with(
            &mut p,
            &tools,
            RunConfig::default(),
            &CancelToken::new(),
            &Message::text(Role::System, "sys"),
            vec![AgentMessage::text(Role::User, "user")],
        )
        .unwrap_err();
        assert!(err.contains("provider exploded"));
    }

    // --- cancellation (ticket 07) -----------------------------------------

    #[test]
    fn cancel_token_defaults_false_flips_resets_and_shares() {
        let t = CancelToken::new();
        assert!(!t.is_cancelled());
        t.cancel();
        assert!(t.is_cancelled());
        t.reset();
        assert!(!t.is_cancelled());
        // Clones observe the same flag.
        let c = t.clone();
        t.cancel();
        assert!(c.is_cancelled());
        // Default is uncancelled.
        assert!(!CancelToken::default().is_cancelled());
    }

    #[test]
    fn cancel_before_first_chat_stops_without_calling_provider() {
        let cancel = CancelToken::new();
        cancel.cancel();
        let mut p = FakeProvider::new(vec![vec![t("never"), done_stop()]]);
        let tools: [Tool; 0] = [];
        let (messages, stop, events) = run_with(
            &mut p,
            &tools,
            RunConfig::default(),
            &cancel,
            &Message::text(Role::System, "sys"),
            vec![AgentMessage::text(Role::User, "user")],
        )
        .unwrap();
        assert_eq!(stop, StopReason::Cancelled);
        assert_eq!(p.calls, 0, "no provider call must be made");
        assert_eq!(messages.len(), 1, "history untouched (user only)");
        // The cancelled stop is emitted as an event, like every other stop.
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::Stop(StopReason::Cancelled)))
        );
    }

    #[test]
    fn cancel_during_request_streams_deltas_but_skips_history() {
        let cancel = CancelToken::new();
        // One-turn script: the provider arms the cancel before it returns its
        // batch, simulating Esc landing while the request was streaming.
        let mut p =
            FakeProvider::new(vec![vec![t("half text"), done_stop()]]).cancels_on(1, &cancel);
        let tools: [Tool; 0] = [];
        let (messages, stop, events) = run_with(
            &mut p,
            &tools,
            RunConfig::default(),
            &cancel,
            &Message::text(Role::System, "sys"),
            vec![AgentMessage::text(Role::User, "user")],
        )
        .unwrap();
        assert_eq!(stop, StopReason::Cancelled);
        // The half text was streamed out live (it stays on the transcript)...
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::Stream(Delta::Text(t)) if t == "half text"))
        );
        // ...but no assistant message enters history (no half-text message).
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn provider_error_while_cancel_set_is_a_silent_cancelled_stop() {
        let cancel = CancelToken::new();
        cancel.cancel();
        struct ErrProvider;
        impl Provider for ErrProvider {
            fn chat(
                &mut self,
                _m: &[Message],
                _t: &[ToolSpec],
                _cfg: &ProviderConfig,
                _c: &CancelToken,
                _on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
            ) -> Result<Option<TokenUsage>, String> {
                Err("request cancelled".to_string())
            }
        }
        let mut p = ErrProvider;
        let tools: [Tool; 0] = [];
        let (_, stop, _) = run_with(
            &mut p,
            &tools,
            RunConfig::default(),
            &cancel,
            &Message::text(Role::System, "sys"),
            vec![AgentMessage::text(Role::User, "user")],
        )
        .unwrap();
        assert_eq!(stop, StopReason::Cancelled);
    }

    #[test]
    fn cancel_after_turn_one_tools_keeps_pushed_tool_result() {
        let cancel = CancelToken::new();
        // Turn 1 calls the weather tool (its result is pushed); the provider
        // cancels while serving turn 2's request.
        let mut p = FakeProvider::new(vec![
            vec![
                tc_start(0, "call_1", "get_weather"),
                tc_args(0, "{\"city\": \"Beijing\"}"),
                done_tools(),
            ],
            vec![t("final answer"), done_stop()],
        ])
        .cancels_on(2, &cancel);
        let tools = [weather_tool()];
        let (messages, stop, events) = run_with(
            &mut p,
            &tools,
            RunConfig::default(),
            &cancel,
            &Message::text(Role::System, "sys"),
            vec![AgentMessage::text(Role::User, "user")],
        )
        .unwrap();
        assert_eq!(stop, StopReason::Cancelled);
        // The already-pushed tool result stays in history...
        let tool_msgs: Vec<_> = messages
            .iter()
            .filter(|m| m.role() == &Role::Tool)
            .collect();
        assert_eq!(tool_msgs.len(), 1);
        assert!(tool_msgs[0].text_content().contains("25C"));
        // ...but the aborted final assistant reply does not (half text rule).
        assert!(
            messages
                .iter()
                .all(|m| !m.text_content().contains("final answer")),
            "no assistant message from the cancelled turn in history"
        );
        // The final text was still streamed live.
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::Stream(Delta::Text(t)) if t == "final answer"))
        );
    }

    #[test]
    fn cancelled_parallel_batch_emits_no_tool_events() {
        // A cancel that lands during a parallel batch discards the whole batch:
        // neither history nor the rendered tool events may reflect it. The UI
        // must never show a completed block for a result the Session log never
        // received.
        let cancel = CancelToken::new();
        let ran = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let ran2 = ran.clone();
        let cancel2 = cancel.clone();
        let flipper = Tool::new(
            "flip_tool",
            "flips the token",
            serde_json::json!({}),
            move |_| {
                ran2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                cancel2.cancel();
                Ok("flipped".to_string())
            },
        );
        let script = vec![vec![
            tc_start(0, "call_0", "get_weather"),
            tc_args(0, "{}"),
            tc_start(1, "call_1", "flip_tool"),
            tc_args(1, "{}"),
            done_tools(),
        ]];
        let tools = [weather_tool(), flipper];
        let (messages, stop, events) = run_with(
            &mut FakeProvider::new(script),
            &tools,
            RunConfig {
                parallel_tools: true,
                ..Default::default()
            },
            &cancel,
            &Message::text(Role::System, "sys"),
            vec![AgentMessage::text(Role::User, "user")],
        )
        .unwrap();
        assert_eq!(stop, StopReason::Cancelled);
        assert!(messages.iter().all(|m| m.role() != &Role::Tool));
        assert!(
            events.iter().all(|e| !matches!(
                e,
                AgentEvent::ToolStart { .. } | AgentEvent::ToolResult { .. }
            )),
            "no tool events for a discarded batch: {:?}",
            events
        );
    }

    #[test]
    fn run_config_defaults_to_parallel_tools() {
        // Parallel tool calls are on by default; no configuration opts in.
        assert!(RunConfig::default().parallel_tools);
    }

    #[test]
    fn cancel_between_serial_tools_keeps_only_completed_results() {
        let cancel = CancelToken::new();
        // Tool 2 cancels the run when it executes; tool 3 must never dispatch.
        let ran: Arc<std::sync::atomic::AtomicUsize> = Default::default();
        let ran2 = ran.clone();
        let cancel2 = cancel.clone();
        let tool = Tool::new("cancel_tool", "cancels", serde_json::json!({}), move |_| {
            ran2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            cancel2.cancel();
            Ok("cancelled mid-run".to_string())
        });
        let never = Tool::new("never_tool", "never runs", serde_json::json!({}), |_| {
            Ok("should not run".to_string())
        });
        // One turn asks for three tools: weather (completed), cancel_tool
        // (sets the flag), never_tool (must not dispatch).
        let script = vec![vec![
            tc_start(0, "call_1", "get_weather"),
            tc_args(0, "{}"),
            tc_start(1, "call_2", "cancel_tool"),
            tc_args(1, "{}"),
            tc_start(2, "call_3", "never_tool"),
            tc_args(2, "{}"),
            done_tools(),
        ]];
        let tools = [weather_tool(), tool, never];
        let (messages, stop, _) = run_with(
            &mut FakeProvider::new(script),
            &tools,
            RunConfig {
                parallel_tools: false,
                ..Default::default()
            },
            &cancel,
            &Message::text(Role::System, "sys"),
            vec![AgentMessage::text(Role::User, "user")],
        )
        .unwrap();
        assert_eq!(stop, StopReason::Cancelled);
        // Tool 2 executed (and cancelled); tool 3 was skipped.
        assert_eq!(ran.load(std::sync::atomic::Ordering::SeqCst), 1);
        // Weather's completed result is in history; the cancelling tool's own
        // aborted result and the skipped tool's result are not.
        let tool_msgs: Vec<&AgentMessage> = messages
            .iter()
            .filter(|m| m.role() == &Role::Tool)
            .collect();
        assert_eq!(tool_msgs.len(), 1);
        assert!(tool_msgs[0].text_content().contains("25C"));
    }

    #[test]
    fn cancel_between_parallel_tools_stops_before_applying_the_batch() {
        let cancel = CancelToken::new();
        let ran: Arc<std::sync::atomic::AtomicUsize> = Default::default();
        let ran2 = ran.clone();
        let cancel2 = cancel.clone();
        let flipper = Tool::new(
            "flip_tool",
            "flips the token",
            serde_json::json!({}),
            move |_| {
                ran2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                cancel2.cancel();
                Ok("flipped".to_string())
            },
        );
        let script = vec![vec![
            tc_start(0, "call_1", "get_weather"),
            tc_args(0, "{}"),
            tc_start(1, "call_2", "flip_tool"),
            tc_args(1, "{}"),
            done_tools(),
        ]];
        let cfg = RunConfig {
            parallel_tools: true,
            ..Default::default()
        };
        let tools = [weather_tool(), flipper];
        let (messages, stop, _) = run_with(
            &mut FakeProvider::new(script),
            &tools,
            cfg,
            &cancel,
            &Message::text(Role::System, "sys"),
            vec![AgentMessage::text(Role::User, "user")],
        )
        .unwrap();
        assert_eq!(stop, StopReason::Cancelled);
        // The batch ran in parallel (both dispatched) but the cancel landed
        // before any result was applied: history has no tool messages.
        assert_eq!(ran.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(messages.iter().all(|m| m.role() != &Role::Tool));
    }

    #[test]
    fn unknown_tool_becomes_error_message() {
        let script = vec![
            vec![
                tc_start(0, "call_1", "nope"),
                tc_args(0, "{}"),
                done_tools(),
            ],
            vec![t("oops"), done_stop()],
        ];
        let (messages, _, stop, _) =
            run(script, vec![weather_tool()], RunConfig::default()).unwrap();
        assert_eq!(stop, StopReason::Completed);
        let tool_msg = messages.iter().find(|m| m.role() == &Role::Tool).unwrap();
        assert!(tool_msg.text_content().contains("unknown tool: nope"));
    }

    // --- pre_tool / after_tool / after_turn phase behaviour (ADR-0031) ------

    #[test]
    fn pre_tool_visits_the_whole_batch_before_any_dispatch_in_serial_mode() {
        // The whole batch is hooked (model order) before anything dispatches,
        // even in serial mode: each hook call must observe dispatch has not
        // happened yet.
        let dispatched = Arc::new(AtomicBool::new(false));
        let order = Arc::new(Mutex::new(Vec::<usize>::new()));
        let d = dispatched.clone();
        let flag = Tool::new(
            "flag_tool",
            "sets the flag",
            serde_json::json!({}),
            move |_| {
                d.store(true, Ordering::SeqCst);
                Ok("ran".to_string())
            },
        );
        let script = vec![vec![
            tc_start(0, "call_0", "flag_tool"),
            tc_args(0, "{}"),
            tc_start(1, "call_1", "flag_tool"),
            tc_args(1, "{}"),
            done_tools(),
        ]];
        let dp = dispatched.clone();
        let od = order.clone();
        let mut phases = PhaseHooks::default();
        phases
            .pre_tool
            .push(PreTool(Box::new(move |_s, _st, index| {
                assert!(
                    !dp.load(Ordering::SeqCst),
                    "dispatch happened before pre_tool({index})"
                );
                od.lock().unwrap().push(index);
                Ok(ToolHookAction::Execute)
            })));
        let tools = [flag];
        let (messages, stop, _) = run_with_phases(
            &mut FakeProvider::new(script),
            &tools,
            RunConfig {
                parallel_tools: false,
                ..Default::default()
            },
            &CancelToken::new(),
            &default_system(),
            vec![AgentMessage::text(Role::User, "user")],
            &mut phases,
        )
        .unwrap();
        assert_eq!(stop, StopReason::Completed);
        assert!(dispatched.load(Ordering::SeqCst), "the tool ran");
        assert_eq!(*order.lock().unwrap(), vec![0, 1]);
        assert_eq!(
            messages.iter().filter(|m| m.role() == &Role::Tool).count(),
            2,
            "both calls' results are in history"
        );
    }

    #[test]
    fn pre_tool_rewrites_arguments_the_tool_then_sees() {
        // The phase hook rewrites call 0's arguments; the tool must run with the
        // rewritten arguments, and the history / events carry the same text.
        let seen = Arc::new(Mutex::new(String::new()));
        let s = seen.clone();
        let echo = Tool::new(
            "echo",
            "echoes the city",
            serde_json::json!({}),
            move |args: Value| {
                let city = args
                    .get("city")
                    .and_then(Value::as_str)
                    .unwrap_or("?")
                    .to_string();
                *s.lock().unwrap() = city.clone();
                Ok(city)
            },
        );
        let script = vec![vec![
            tc_start(0, "call_0", "echo"),
            tc_args(0, "{\"city\": \"Shanghai\"}"),
            done_tools(),
        ]];
        let mut phases = PhaseHooks::default();
        phases.pre_tool.push(PreTool(Box::new(|_s, st, _index| {
            st.messages
                .last_mut()
                .unwrap()
                .llm_mut()
                .unwrap()
                .tool_calls[0]
                .arguments = "{\"city\": \"Beijing\"}".to_string();
            Ok(ToolHookAction::Execute)
        })));
        let tools = [echo];
        let (messages, _stop, events) = run_with_phases(
            &mut FakeProvider::new(script),
            &tools,
            RunConfig::default(),
            &CancelToken::new(),
            &default_system(),
            vec![AgentMessage::text(Role::User, "user")],
            &mut phases,
        )
        .unwrap();
        assert_eq!(*seen.lock().unwrap(), "Beijing");
        let tool_msg = messages.iter().find(|m| m.role() == &Role::Tool).unwrap();
        assert!(tool_msg.text_content().contains("Beijing"));
        assert!(
            events.iter().any(
                |e| matches!(e, AgentEvent::ToolStart { arguments, .. } if arguments.contains("Beijing"))
            ),
            "ToolStart carries the rewritten arguments"
        );
    }

    #[test]
    fn pre_tool_skip_err_produces_the_error_result_shape() {
        // A pre_tool `Skip(Err)` is told to the model as the `Error: …` shape
        // tool failures already use, and the after_tool phase sees `ok == false`.
        let ran = Arc::new(AtomicBool::new(false));
        let r = ran.clone();
        let tool = Tool::new(
            "never_run",
            "must not run",
            serde_json::json!({}),
            move |_| {
                r.store(true, Ordering::SeqCst);
                Ok("ran".to_string())
            },
        );
        let script = vec![vec![
            tc_start(0, "call_0", "never_run"),
            tc_args(0, "{}"),
            done_tools(),
        ]];
        let ok_seen = Arc::new(Mutex::new(None::<bool>));
        let o = ok_seen.clone();
        let mut phases = PhaseHooks::default();
        phases.pre_tool.push(PreTool(Box::new(|_s, _st, _i| {
            Ok(ToolHookAction::Skip {
                result: Err("not allowed".to_string()),
            })
        })));
        phases
            .after_tool
            .push(AfterTool(Box::new(move |_s, _st, _m, ok| {
                *o.lock().unwrap() = Some(ok);
                Ok(())
            })));
        let tools = [tool];
        let (messages, stop, _) = run_with_phases(
            &mut FakeProvider::new(script),
            &tools,
            RunConfig::default(),
            &CancelToken::new(),
            &default_system(),
            vec![AgentMessage::text(Role::User, "user")],
            &mut phases,
        )
        .unwrap();
        assert_eq!(stop, StopReason::Completed);
        assert!(!ran.load(Ordering::SeqCst));
        let tool_msg = messages.iter().find(|m| m.role() == &Role::Tool).unwrap();
        assert!(
            tool_msg.text_content().starts_with("Error: not allowed"),
            "got: {}",
            tool_msg.text_content()
        );
        assert_eq!(*ok_seen.lock().unwrap(), Some(false));
    }

    #[test]
    fn pre_tool_removing_a_call_errors() {
        // A phase hook may rewrite a call's arguments but not restructure the
        // batch: a vanished index is an error, not a panic.
        let mut phases = PhaseHooks::default();
        phases.pre_tool.push(PreTool(Box::new(|_s, st, _index| {
            st.messages
                .last_mut()
                .unwrap()
                .llm_mut()
                .unwrap()
                .tool_calls
                .clear();
            Ok(ToolHookAction::Execute)
        })));
        let script = vec![vec![
            tc_start(0, "call_0", "get_weather"),
            tc_args(0, "{}"),
            done_tools(),
        ]];
        let tools = [weather_tool()];
        let err = run_with_phases(
            &mut FakeProvider::new(script),
            &tools,
            RunConfig::default(),
            &CancelToken::new(),
            &default_system(),
            vec![AgentMessage::text(Role::User, "user")],
            &mut phases,
        )
        .unwrap_err();
        assert!(err.contains("pre_tool removed tool call 0"), "got: {err}");
    }

    #[test]
    fn pre_tool_adding_a_call_errors() {
        // Restructuring also covers additions: an added `tool_calls` entry
        // would leave a call unpaired with a result on the next request.
        let mut phases = PhaseHooks::default();
        phases.pre_tool.push(PreTool(Box::new(|_s, st, _index| {
            st.messages
                .last_mut()
                .unwrap()
                .llm_mut()
                .unwrap()
                .tool_calls
                .push(ToolCall {
                    id: "extra".to_string(),
                    name: "get_weather".to_string(),
                    arguments: "{}".to_string(),
                });
            Ok(ToolHookAction::Execute)
        })));
        let script = vec![vec![
            tc_start(0, "call_0", "get_weather"),
            tc_args(0, "{}"),
            done_tools(),
        ]];
        let tools = [weather_tool()];
        let err = run_with_phases(
            &mut FakeProvider::new(script),
            &tools,
            RunConfig::default(),
            &CancelToken::new(),
            &default_system(),
            vec![AgentMessage::text(Role::User, "user")],
            &mut phases,
        )
        .unwrap_err();
        assert!(
            err.contains("a tool phase restructured the batch: 1 -> 2 tool calls"),
            "got: {err}"
        );
    }

    #[test]
    fn pre_tool_rewrite_reaches_the_assistant_message_event() {
        // The assistant-message event is emitted after the `pre_tool` phase,
        // so the session log (persisted from events) and the returned history
        // carry the same rewritten arguments (ADR-0015 D3).
        let mut phases = PhaseHooks::default();
        phases.pre_tool.push(PreTool(Box::new(|_s, st, _index| {
            st.messages
                .last_mut()
                .unwrap()
                .llm_mut()
                .unwrap()
                .tool_calls[0]
                .arguments = "{\"city\": \"Beijing\"}".to_string();
            Ok(ToolHookAction::Execute)
        })));
        let script = vec![vec![
            tc_start(0, "call_0", "get_weather"),
            tc_args(0, "{\"city\": \"Shanghai\"}"),
            done_tools(),
        ]];
        let tools = [weather_tool()];
        let (messages, _stop, events) = run_with_phases(
            &mut FakeProvider::new(script),
            &tools,
            RunConfig::default(),
            &CancelToken::new(),
            &default_system(),
            vec![AgentMessage::text(Role::User, "user")],
            &mut phases,
        )
        .unwrap();
        // The announced assistant message is the post-hook one, matching what
        // stays in the returned history.
        let announced = events
            .iter()
            .find_map(|e| match e {
                AgentEvent::Message { message, .. } if message.role() == &Role::Assistant => {
                    Some(message.clone())
                }
                _ => None,
            })
            .expect("assistant message event");
        let in_history = messages
            .iter()
            .find(|m| m.role() == &Role::Assistant)
            .expect("assistant in history");
        assert_eq!(
            announced.tool_calls()[0].arguments,
            "{\"city\": \"Beijing\"}"
        );
        assert_eq!(
            in_history.tool_calls()[0].arguments,
            announced.tool_calls()[0].arguments
        );
    }

    #[test]
    fn pre_tool_phase_error_aborts_the_run() {
        // A phase hook's `Err` propagates verbatim and no Stop event is emitted.
        let mut phases = PhaseHooks::default();
        phases
            .pre_tool
            .push(PreTool(Box::new(|_s, _st, _index| Err("gate".to_string()))));
        let script = vec![vec![
            tc_start(0, "call_0", "get_weather"),
            tc_args(0, "{}"),
            done_tools(),
        ]];
        let tools = [weather_tool()];
        let err = run_with_phases(
            &mut FakeProvider::new(script),
            &tools,
            RunConfig::default(),
            &CancelToken::new(),
            &default_system(),
            vec![AgentMessage::text(Role::User, "user")],
            &mut phases,
        )
        .unwrap_err();
        assert_eq!(err, "gate");
    }

    #[test]
    fn after_tool_phase_receives_the_ok_flag() {
        // The after_tool phase sees `ok == false` for a tool that failed,
        // regardless of any rewrite.
        let script = vec![vec![
            tc_start(0, "call_1", "fragile_tool"),
            tc_args(0, "{\"city\": \"Tokyo\"}"),
            done_tools(),
        ]];
        let ok_seen = Arc::new(Mutex::new(None::<bool>));
        let o = ok_seen.clone();
        let mut phases = PhaseHooks::default();
        phases
            .after_tool
            .push(AfterTool(Box::new(move |_s, _st, _m, ok| {
                *o.lock().unwrap() = Some(ok);
                Ok(())
            })));
        let tools = [fragile_tool()];
        let (_, stop, _) = run_with_phases(
            &mut FakeProvider::new(script),
            &tools,
            RunConfig::default(),
            &CancelToken::new(),
            &default_system(),
            vec![AgentMessage::text(Role::User, "user")],
            &mut phases,
        )
        .unwrap();
        assert_eq!(stop, StopReason::Completed);
        assert_eq!(*ok_seen.lock().unwrap(), Some(false));
    }

    #[test]
    fn after_turn_runs_on_completed_with_the_final_history() {
        // after_turn runs on a completed turn with the final iteration count
        // and stop reason; whatever it leaves in the history is what the caller
        // stores.
        let script = vec![vec![t("done"), done_stop()]];
        let seen = Arc::new(Mutex::new(None::<(usize, StopReason)>));
        let s = seen.clone();
        let mut phases = PhaseHooks::default();
        phases
            .after_turn
            .push(AfterTurn(Box::new(move |_sp, st, stop| {
                *s.lock().unwrap() = Some((st.iterations, stop.clone()));
                st.messages.push(AgentMessage::text(Role::User, "sealed"));
                Ok(())
            })));
        let tools: [Tool; 0] = [];
        let (messages, stop, _) = run_with_phases(
            &mut FakeProvider::new(script),
            &tools,
            RunConfig::default(),
            &CancelToken::new(),
            &default_system(),
            vec![AgentMessage::text(Role::User, "user")],
            &mut phases,
        )
        .unwrap();
        assert_eq!(stop, StopReason::Completed);
        assert_eq!(*seen.lock().unwrap(), Some((1, StopReason::Completed)));
        assert_eq!(messages.last().unwrap().text_content(), "sealed");
    }

    #[test]
    fn after_turn_does_not_run_when_the_run_errors() {
        struct ErrProvider;
        impl Provider for ErrProvider {
            fn chat(
                &mut self,
                _m: &[Message],
                _t: &[ToolSpec],
                _cfg: &ProviderConfig,
                _c: &CancelToken,
                _on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
            ) -> Result<Option<TokenUsage>, String> {
                Err("boom".to_string())
            }
        }
        let called = Arc::new(AtomicBool::new(false));
        let c = called.clone();
        let mut phases = PhaseHooks::default();
        phases
            .after_turn
            .push(AfterTurn(Box::new(move |_s, _st, _stop| {
                c.store(true, Ordering::SeqCst);
                Ok(())
            })));
        let tools: [Tool; 0] = [];
        let err = run_with_phases(
            &mut ErrProvider,
            &tools,
            RunConfig::default(),
            &CancelToken::new(),
            &default_system(),
            vec![AgentMessage::text(Role::User, "user")],
            &mut phases,
        )
        .unwrap_err();
        assert!(err.contains("boom"));
        assert!(
            !called.load(Ordering::SeqCst),
            "after_turn must not run on an errored run"
        );
    }

    #[test]
    fn skipped_calls_report_before_dispatched_ones_in_parallel_mode() {
        // In parallel mode a pre_tool-skipped call's result is known before
        // dispatch, so it completes before dispatched calls: its ToolResult event
        // comes first even though the dispatched call finishes later.
        let slow = Tool::new("slow_tool", "sleeps briefly", serde_json::json!({}), |_| {
            std::thread::sleep(std::time::Duration::from_millis(50));
            Ok("slow".to_string())
        });
        let script = vec![vec![
            tc_start(0, "call_0", "slow_tool"),
            tc_args(0, "{}"),
            tc_start(1, "call_1", "slow_tool"),
            tc_args(1, "{}"),
            done_tools(),
        ]];
        let mut phases = PhaseHooks::default();
        phases.pre_tool.push(PreTool(Box::new(|_s, _st, index| {
            if index == 0 {
                Ok(ToolHookAction::Skip {
                    result: Ok("denied".to_string()),
                })
            } else {
                Ok(ToolHookAction::Execute)
            }
        })));
        let tools = [slow];
        let (messages, _stop, events) = run_with_phases(
            &mut FakeProvider::new(script),
            &tools,
            RunConfig {
                parallel_tools: true,
                ..Default::default()
            },
            &CancelToken::new(),
            &default_system(),
            vec![AgentMessage::text(Role::User, "user")],
            &mut phases,
        )
        .unwrap();
        let results: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::ToolResult { tool_call_id, .. } => Some(tool_call_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(results, vec!["call_0", "call_1"]);
        // History keeps model order too (skip first, dispatched second).
        let history: Vec<&str> = messages
            .iter()
            .filter(|m| m.role() == &Role::Tool)
            .map(|m| m.tool_call_id().unwrap())
            .collect();
        assert_eq!(history, vec!["call_0", "call_1"]);
    }

    // --- the FFI wire shape (ADR-0023) -------------------------------------

    #[test]
    fn stop_reason_serializes_to_the_ffi_wire_shape() {
        // The FFI `stop` / `turn_finished` events carry exactly these strings
        // (the spec's ABI contract section); `Serialize` + snake_case is
        // additive for that contract (ADR-0023).
        assert_eq!(
            serde_json::to_value(StopReason::Completed).unwrap(),
            serde_json::json!("completed")
        );
        assert_eq!(
            serde_json::to_value(StopReason::Cancelled).unwrap(),
            serde_json::json!("cancelled")
        );
    }

    // --- seven-phase pipeline (ADR-0031, engine ticket 02) -----------------

    /// The loop harness with a caller-supplied [`PhaseHooks`] bundle (and no
    /// old hooks). Events are collected so tests assert the external stream.
    #[allow(clippy::too_many_arguments)] // a test harness bundling run context
    fn run_with_phases<P: Provider>(
        provider: &mut P,
        tools: &[Tool],
        cfg: RunConfig,
        cancel: &CancelToken,
        system: &Message,
        messages: Vec<AgentMessage>,
        phases: &mut PhaseHooks<'static>,
    ) -> Result<(Vec<AgentMessage>, StopReason, Vec<AgentEvent>), String> {
        let config = ProviderConfig::new("test-key", "https://example.invalid/v1", "test-model");
        let mut events: Vec<AgentEvent> = Vec::new();
        // Scope the runner (and the sink borrowing `events`) so the borrow ends
        // before `events` is returned.
        let (msgs, stop) = {
            let mut sink = |e: AgentEvent| {
                events.push(e);
                Ok(())
            };
            let mut runner = AgentRunner::new(tools, cfg, &config, cancel, &mut sink);
            runner.phases = std::mem::take(phases);
            runner.run(provider, system, messages)?
        };
        Ok((msgs, stop, events))
    }

    /// The standard phase harness: no tools, one user message, fresh cancel.
    fn phases_one_shot(
        script: Vec<Vec<Delta>>,
        tools: Vec<Tool>,
        phases: &mut PhaseHooks<'static>,
    ) -> Result<(Vec<AgentMessage>, StopReason, Vec<AgentEvent>), String> {
        let mut p = FakeProvider::new(script);
        run_with_phases(
            &mut p,
            &tools,
            RunConfig::default(),
            &CancelToken::new(),
            &default_system(),
            vec![AgentMessage::text(Role::User, "user")],
            phases,
        )
    }

    /// A shared label recorder: each hook pushes one label.
    fn labels() -> Arc<Mutex<Vec<String>>> {
        Arc::new(Mutex::new(Vec::new()))
    }
    fn push(l: &Arc<Mutex<Vec<String>>>, s: &str) {
        l.lock().unwrap().push(s.to_string());
    }

    /// A phase provider-call counter helper: the turn that calls a tool once
    /// then answers.
    fn tool_script() -> Vec<Vec<Delta>> {
        vec![
            vec![
                tc_start(0, "call_0", "get_weather"),
                tc_args(0, "{\"city\": \"Beijing\"}"),
                done_tools(),
            ],
            vec![t("done"), done_stop()],
        ]
    }

    #[test]
    fn phase_pipeline_fires_in_registration_order_across_the_loop() {
        // One hook per phase, a tool-then-answer turn: the phases fire at their
        // documented points — pre_turn once, pre_chat/after_chat per provider
        // call, pre_tool/after_tool per call, post_iteration after the batch,
        // after_turn once at the end.
        let rec = labels();
        let mut phases = PhaseHooks::default();
        {
            let r = rec.clone();
            phases.pre_turn.push(PreTurn(Box::new(move |_s, _st| {
                push(&r, "pre_turn");
                Ok(HookAction::Continue)
            })));
        }
        {
            let r = rec.clone();
            phases.pre_chat.push(PreChat(Box::new(move |_s, _st| {
                push(&r, "pre_chat");
                Ok(HookAction::Continue)
            })));
        }
        {
            let r = rec.clone();
            phases
                .after_chat
                .push(AfterChat(Box::new(move |_s, _st, _a| {
                    push(&r, "after_chat");
                    Ok(HookAction::Continue)
                })));
        }
        {
            let r = rec.clone();
            phases.pre_tool.push(PreTool(Box::new(move |_s, _st, _i| {
                push(&r, "pre_tool");
                Ok(ToolHookAction::Execute)
            })));
        }
        {
            let r = rec.clone();
            phases
                .after_tool
                .push(AfterTool(Box::new(move |_s, _st, _m, _ok| {
                    push(&r, "after_tool");
                    Ok(())
                })));
        }
        {
            let r = rec.clone();
            phases
                .post_iteration
                .push(PostIteration(Box::new(move |_s, _st| {
                    push(&r, "post_iteration");
                    Ok(HookAction::Continue)
                })));
        }
        {
            let r = rec.clone();
            phases
                .after_turn
                .push(AfterTurn(Box::new(move |_s, _st, _stop| {
                    push(&r, "after_turn");
                    Ok(())
                })));
        }
        let (messages, stop, _) =
            phases_one_shot(tool_script(), vec![weather_tool()], &mut phases).unwrap();
        assert_eq!(stop, StopReason::Completed);
        assert_eq!(
            *rec.lock().unwrap(),
            vec![
                "pre_turn",
                "pre_chat",
                "after_chat",
                "pre_tool",
                "after_tool",
                "post_iteration",
                "pre_chat",
                "after_chat",
                "after_turn",
            ]
        );
        // The tool result still landed in history (phases did not disturb it).
        assert!(messages.iter().any(|m| m.role() == &Role::Tool));
    }

    #[test]
    fn pre_tool_visits_every_call_in_model_order_before_dispatch() {
        // Two calls in one batch (parallel mode): both pre_tool visits happen
        // in model index order before any tool dispatches.
        let dispatched = Arc::new(AtomicBool::new(false));
        let d = dispatched.clone();
        let flag = Tool::new(
            "flag_tool",
            "sets the flag",
            serde_json::json!({}),
            move |_| {
                d.store(true, Ordering::SeqCst);
                Ok("ran".to_string())
            },
        );
        let order = labels();
        let od = order.clone();
        let mut phases = PhaseHooks::default();
        phases
            .pre_tool
            .push(PreTool(Box::new(move |_s, _st, index| {
                od.lock().unwrap().push(format!("pre_tool{index}"));
                // The whole batch is hooked before anything runs, in parallel mode.
                assert!(!dispatched.load(Ordering::SeqCst));
                Ok(ToolHookAction::Execute)
            })));
        let script = vec![vec![
            tc_start(0, "call_0", "flag_tool"),
            tc_args(0, "{}"),
            tc_start(1, "call_1", "flag_tool"),
            tc_args(1, "{}"),
            done_tools(),
        ]];
        let mut p = FakeProvider::new(script);
        let tools = [flag];
        let (messages, stop, _) = run_with_phases(
            &mut p,
            &tools,
            RunConfig {
                parallel_tools: true,
                ..Default::default()
            },
            &CancelToken::new(),
            &default_system(),
            vec![AgentMessage::text(Role::User, "user")],
            &mut phases,
        )
        .unwrap();
        assert_eq!(stop, StopReason::Completed);
        assert_eq!(*order.lock().unwrap(), vec!["pre_tool0", "pre_tool1"]);
        assert_eq!(
            messages.iter().filter(|m| m.role() == &Role::Tool).count(),
            2
        );
    }

    #[test]
    fn after_chat_rewrites_the_message_history_and_events_carry() {
        // A phase hook rewrites the assistant message before it enters history:
        // the returned history and the announced event both carry the rewrite
        // (model / log / display see one text, ADR-0015 semantics).
        let mut phases = PhaseHooks::default();
        phases.after_chat.push(AfterChat(Box::new(|_s, _st, asst| {
            *asst = AgentMessage::text(Role::Assistant, "REWRITTEN");
            Ok(HookAction::Continue)
        })));
        let (messages, _stop, events) =
            phases_one_shot(vec![vec![t("orig"), done_stop()]], vec![], &mut phases).unwrap();
        let last = messages.last().unwrap();
        assert_eq!(last.text_content(), "REWRITTEN");
        assert!(events.iter().any(|e| matches!(
            e,
            AgentEvent::Message { message, .. } if message.text_content() == "REWRITTEN"
        )));
    }

    #[test]
    fn pre_tool_skip_still_produces_a_paired_result() {
        // A `Skip` never dispatches, but its supplied result enters history and
        // still produces ToolStart/ToolResult events, and `after_tool` is still
        // visited (the ADR-0015 skip semantics lifted onto pre_tool).
        let ran = Arc::new(AtomicBool::new(false));
        let r = ran.clone();
        let tool = Tool::new(
            "never_run",
            "must not run",
            serde_json::json!({}),
            move |_| {
                r.store(true, Ordering::SeqCst);
                Ok("ran".to_string())
            },
        );
        let visited = labels();
        let v = visited.clone();
        let mut phases = PhaseHooks::default();
        phases.pre_tool.push(PreTool(Box::new(|_s, _st, _i| {
            Ok(ToolHookAction::Skip {
                result: Ok("SKIPPED".to_string()),
            })
        })));
        phases
            .after_tool
            .push(AfterTool(Box::new(move |_s, _st, msg, _ok| {
                push(&v, msg.text_content().as_str());
                Ok(())
            })));
        let script = vec![vec![
            tc_start(0, "call_0", "never_run"),
            tc_args(0, "{}"),
            done_tools(),
        ]];
        let mut p = FakeProvider::new(script);
        let tools = [tool];
        let (messages, _stop, events) = run_with_phases(
            &mut p,
            &tools,
            RunConfig::default(),
            &CancelToken::new(),
            &default_system(),
            vec![AgentMessage::text(Role::User, "user")],
            &mut phases,
        )
        .unwrap();
        assert!(!ran.load(Ordering::SeqCst), "the tool must not run");
        let tool_msg = messages.iter().find(|m| m.role() == &Role::Tool).unwrap();
        assert!(tool_msg.text_content().contains("SKIPPED"));
        assert_eq!(*visited.lock().unwrap(), vec!["SKIPPED".to_string()]);
        assert!(events.iter().any(
            |e| matches!(e, AgentEvent::ToolStart { tool_call_id, .. } if tool_call_id == "call_0")
        ));
        assert!(events.iter().any(|e| matches!(
            e,
            AgentEvent::ToolResult { tool_call_id, ok: true, .. } if tool_call_id == "call_0"
        )));
    }

    #[test]
    fn pre_turn_abort_skips_the_provider_and_runs_after_turn() {
        // A pre_turn `Abort` prevents the loop from ever calling the provider,
        // yet after_turn still runs and the abort reason is stashed for it.
        let after_ran = Arc::new(AtomicBool::new(false));
        let a = after_ran.clone();
        let reason = Arc::new(Mutex::new(None::<String>));
        let rn = reason.clone();
        let mut phases = PhaseHooks::default();
        phases.pre_turn.push(PreTurn(Box::new(|_s, _st| {
            Ok(HookAction::Abort {
                reason: "nope".to_string(),
            })
        })));
        phases
            .after_turn
            .push(AfterTurn(Box::new(move |_s, st, _stop| {
                a.store(true, Ordering::SeqCst);
                if let Some(r) = st.extensions.get::<AbortReason>() {
                    *rn.lock().unwrap() = Some(r.0.clone());
                }
                Ok(())
            })));
        let mut p = FakeProvider::new(vec![vec![t("never"), done_stop()]]);
        let tools: [Tool; 0] = [];
        let (messages, stop, _) = run_with_phases(
            &mut p,
            &tools,
            RunConfig::default(),
            &CancelToken::new(),
            &default_system(),
            vec![AgentMessage::text(Role::User, "user")],
            &mut phases,
        )
        .unwrap();
        assert_eq!(p.calls, 0, "pre_turn abort must not call the provider");
        assert_eq!(stop, StopReason::Completed);
        assert_eq!(messages.len(), 1, "history untouched");
        assert!(
            after_ran.load(Ordering::SeqCst),
            "after_turn runs even on abort"
        );
        assert_eq!(*reason.lock().unwrap(), Some("nope".to_string()));
    }

    #[test]
    fn pre_chat_abort_stops_before_the_provider_call() {
        let mut phases = PhaseHooks::default();
        phases.pre_chat.push(PreChat(Box::new(|_s, _st| {
            Ok(HookAction::Abort {
                reason: "halt".to_string(),
            })
        })));
        let mut p = FakeProvider::new(vec![vec![t("never"), done_stop()]]);
        let tools: [Tool; 0] = [];
        let (_, stop, _) = run_with_phases(
            &mut p,
            &tools,
            RunConfig::default(),
            &CancelToken::new(),
            &default_system(),
            vec![AgentMessage::text(Role::User, "user")],
            &mut phases,
        )
        .unwrap();
        assert_eq!(p.calls, 0);
        assert_eq!(stop, StopReason::Completed);
    }

    #[test]
    fn after_chat_abort_drops_the_response_and_runs_after_turn() {
        // after_chat runs once the provider has produced a reply; an `Abort`
        // vetoes the response (it does not enter history) and ends the run.
        let after_ran = Arc::new(AtomicBool::new(false));
        let a = after_ran.clone();
        let mut phases = PhaseHooks::default();
        phases.after_chat.push(AfterChat(Box::new(|_s, _st, _a| {
            Ok(HookAction::Abort {
                reason: "veto".to_string(),
            })
        })));
        phases
            .after_turn
            .push(AfterTurn(Box::new(move |_s, _st, _stop| {
                a.store(true, Ordering::SeqCst);
                Ok(())
            })));
        let (messages, stop, _) =
            phases_one_shot(vec![vec![t("paid"), done_stop()]], vec![], &mut phases).unwrap();
        assert_eq!(stop, StopReason::Completed);
        // The assistant response was generated but vetoed: not in history.
        assert!(messages.iter().all(|m| m.text_content() != "paid"));
        assert!(after_ran.load(Ordering::SeqCst));
    }

    #[test]
    fn pre_tool_abort_skips_dispatch_and_runs_after_turn() {
        // A pre_tool `Abort` ends the run before the batch dispatches; the tool
        // never runs, and after_turn still fires.
        let ran = Arc::new(AtomicBool::new(false));
        let r = ran.clone();
        let tool = Tool::new(
            "never_run",
            "must not run",
            serde_json::json!({}),
            move |_| {
                r.store(true, Ordering::SeqCst);
                Ok("ran".to_string())
            },
        );
        let after_ran = Arc::new(AtomicBool::new(false));
        let a = after_ran.clone();
        let mut phases = PhaseHooks::default();
        phases.pre_tool.push(PreTool(Box::new(|_s, _st, _i| {
            Ok(ToolHookAction::Abort {
                reason: "denied".to_string(),
            })
        })));
        phases
            .after_turn
            .push(AfterTurn(Box::new(move |_s, _st, _stop| {
                a.store(true, Ordering::SeqCst);
                Ok(())
            })));
        let script = vec![vec![
            tc_start(0, "call_0", "never_run"),
            tc_args(0, "{}"),
            done_tools(),
        ]];
        let mut p = FakeProvider::new(script);
        let tools = [tool];
        let (messages, stop, events) = run_with_phases(
            &mut p,
            &tools,
            RunConfig::default(),
            &CancelToken::new(),
            &default_system(),
            vec![AgentMessage::text(Role::User, "user")],
            &mut phases,
        )
        .unwrap();
        assert_eq!(stop, StopReason::Completed);
        assert!(!ran.load(Ordering::SeqCst), "the tool must not dispatch");
        assert!(messages.iter().all(|m| m.role() != &Role::Tool));
        assert!(
            events.iter().all(|e| !matches!(
                e,
                AgentEvent::ToolStart { .. } | AgentEvent::ToolResult { .. }
            )),
            "no tool events for an aborted batch"
        );
        // The aborted batch must not dangle: the assistant message stays in
        // history with its `tool_calls` stripped (pairing invariant, ADR-0015
        // D4), and its Message event fires so the session layer persists
        // exactly what remains.
        let last = messages
            .last()
            .expect("history keeps the assistant message");
        assert_eq!(last.role(), &Role::Assistant);
        assert!(
            last.tool_calls().is_empty(),
            "an aborted batch leaves no unpaired tool calls"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::Message { .. })),
            "the aborted assistant entry is announced as a Message event"
        );
        assert!(after_ran.load(Ordering::SeqCst));
    }

    #[test]
    fn post_iteration_abort_ends_the_run_after_the_batch_applies() {
        // The tool batch lands, then post_iteration aborts before the next
        // provider call: the second turn never runs.
        let after_ran = Arc::new(AtomicBool::new(false));
        let a = after_ran.clone();
        let mut phases = PhaseHooks::default();
        phases
            .post_iteration
            .push(PostIteration(Box::new(|_s, _st| {
                Ok(HookAction::Abort {
                    reason: "budget".to_string(),
                })
            })));
        phases
            .after_turn
            .push(AfterTurn(Box::new(move |_s, _st, _stop| {
                a.store(true, Ordering::SeqCst);
                Ok(())
            })));
        let mut p = FakeProvider::new(tool_script());
        let tools = [weather_tool()];
        let (messages, stop, _) = run_with_phases(
            &mut p,
            &tools,
            RunConfig::default(),
            &CancelToken::new(),
            &default_system(),
            vec![AgentMessage::text(Role::User, "user")],
            &mut phases,
        )
        .unwrap();
        assert_eq!(stop, StopReason::Completed);
        assert_eq!(p.calls, 1, "the second turn must not run");
        // The first batch's result is in history.
        assert!(messages.iter().any(|m| m.role() == &Role::Tool));
        assert!(after_ran.load(Ordering::SeqCst));
    }

    #[test]
    fn phase_extensions_carry_typed_state_across_phases() {
        // A pre_chat hook stashes typed state; after_chat reads it and folds it
        // into the rewritten message — the cross-phase extension map works.
        struct Seed(String);
        let mut phases = PhaseHooks::default();
        phases.pre_chat.push(PreChat(Box::new(|_s, st| {
            st.extensions.insert(Seed("ctx".to_string()));
            Ok(HookAction::Continue)
        })));
        phases.after_chat.push(AfterChat(Box::new(|_s, st, asst| {
            let seed = st
                .extensions
                .get::<Seed>()
                .map(|s| s.0.clone())
                .unwrap_or_default();
            *asst = AgentMessage::text(Role::Assistant, format!("[{seed}]"));
            Ok(HookAction::Continue)
        })));
        let (messages, _, _) =
            phases_one_shot(vec![vec![t("x"), done_stop()]], vec![], &mut phases).unwrap();
        assert_eq!(messages.last().unwrap().text_content(), "[ctx]");
    }

    #[test]
    fn after_tool_phase_rewrites_the_result_message() {
        // The after_tool phase rewrites the result before it enters history and
        // before the events, so history and the ToolResult event agree.
        let mut phases = PhaseHooks::default();
        phases
            .after_tool
            .push(AfterTool(Box::new(|_s, _st, msg, _ok| {
                *msg = AgentMessage::tool_result(
                    msg.tool_call_id().unwrap_or_default(),
                    "redacted-phase",
                );
                Ok(())
            })));
        let script = vec![vec![
            tc_start(0, "call_0", "get_weather"),
            tc_args(0, "{}"),
            done_tools(),
        ]];
        let mut p = FakeProvider::new(script);
        let tools = [weather_tool()];
        let (messages, _, events) = run_with_phases(
            &mut p,
            &tools,
            RunConfig::default(),
            &CancelToken::new(),
            &default_system(),
            vec![AgentMessage::text(Role::User, "user")],
            &mut phases,
        )
        .unwrap();
        let tool_msg = messages.iter().find(|m| m.role() == &Role::Tool).unwrap();
        assert_eq!(tool_msg.text_content(), "redacted-phase");
        assert!(events.iter().any(
            |e| matches!(e, AgentEvent::ToolResult { result, .. } if result == "redacted-phase")
        ));
    }

    #[test]
    fn after_turn_runs_on_the_cancelled_path() {
        // `after_turn` runs whatever the way the turn closes — cancel included
        // (ADR-0031 D2). A pre-set cancel token stops before any provider call.
        let after_ran = Arc::new(AtomicBool::new(false));
        let a = after_ran.clone();
        let mut phases = PhaseHooks::default();
        phases
            .after_turn
            .push(AfterTurn(Box::new(move |_s, _st, stop| {
                assert_eq!(stop, &StopReason::Cancelled);
                a.store(true, Ordering::SeqCst);
                Ok(())
            })));
        let cancel = CancelToken::new();
        cancel.cancel();
        let mut p = FakeProvider::new(vec![vec![t("never"), done_stop()]]);
        let tools: [Tool; 0] = [];
        let (_, stop, _) = run_with_phases(
            &mut p,
            &tools,
            RunConfig::default(),
            &cancel,
            &default_system(),
            vec![AgentMessage::text(Role::User, "user")],
            &mut phases,
        )
        .unwrap();
        assert_eq!(stop, StopReason::Cancelled);
        assert!(
            after_ran.load(Ordering::SeqCst),
            "after_turn runs on cancel"
        );
    }
}
