//! Agent runtime loop, folded from the ticket 04 prototype.
//!
//! Shape locked by `.scratch/slimcode-v1` ticket 04:
//! - the loop is a value, [`AgentRunner`] (ADR-0011 D1): one run's tools,
//!   config, cancel token and event subscription, with `run` driving the loop;
//!   ADR-0015 adds the optional hook seam ([`RunHooks`]) on top of it;
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
use serde_json::Value;
use std::collections::BTreeMap;

// The LLM seam types are owned by `slimcode-ai` (ADR-0011 D1) and re-exported
// here so callers can keep importing them from the runtime module.
pub use slimcode_ai::config::ProviderConfig;
pub use slimcode_ai::llm::{CancelToken, Delta, FinishReason, Provider, ToolSpec};

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
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// The model produced a final answer with no tool calls.
    Completed,
    /// The caller requested a cancel (Esc in the TUI) at some boundary of the
    /// run. Whatever already streamed / was already applied stays; no error
    /// is implied and the stop renders nothing.
    Cancelled,
}

/// What a [`RunHooks::before_tool`] hook decides for one call: run it, or
/// skip execution and supply the result the model will see (ADR-0015 D4). A
/// skip still yields a tool result, because every `tool_call` must be paired
/// with one on the next provider request.
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
    Message(AgentMessage),
    Stop(StopReason),
}

#[derive(Clone, Debug)]
pub struct RunConfig {
    /// Execute multiple tool calls from one response concurrently, or one at a
    /// time (appending results as we go). Defaults to `true`: a Tool batch is
    /// dispatched on scoped threads, its events stream in completion order and
    /// its results enter history in model order.
    pub parallel_tools: bool,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            parallel_tools: true,
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

/// Signature of a [`RunHooks::before_tool`] hook: once per call in model
/// order, with the batch's assistant message to rewrite or skip.
pub type BeforeToolHook<'a> =
    Box<dyn FnMut(&mut AgentMessage, usize) -> Result<ToolDecision, String> + 'a>;

/// Signature of a [`RunHooks::after_tool`] hook: per finished call (skips
/// included), with the result message and the `ok` outcome flag.
pub type AfterToolHook<'a> = Box<dyn FnMut(&mut AgentMessage, bool) -> Result<(), String> + 'a>;

/// Signature of a [`RunHooks::turn_end`] hook: once at run end (Completed /
/// Cancelled), with the final history, turn count and stop reason.
pub type TurnEndHook<'a> =
    Box<dyn FnMut(&mut Vec<AgentMessage>, usize, &StopReason) -> Result<(), String> + 'a>;

/// The optional hook seam (ADR-0015): three callbacks at the loop's
/// boundaries, all of them unset by default. A hook receives the
/// [`AgentMessage`] that is about to enter history and may rewrite it — the
/// rewrite happens before the events and before the log append, so the model,
/// the session log and the display all see one text. `Err` from any hook
/// aborts the run like a provider or sink error.
#[derive(Default)]
pub struct RunHooks<'a> {
    /// Before a Tool batch is dispatched, once per call in model order (all
    /// of them before anything in the batch runs, even in parallel mode). The
    /// message is the batch's assistant message: the hook may rewrite a call's
    /// arguments or return a [`ToolDecision::Skip`]. It may not restructure
    /// the batch — a vanished call index is an error, not a panic.
    pub before_tool: Option<BeforeToolHook<'a>>,
    /// As each tool result is about to enter history, in completion order (a
    /// skipped call's supplied result is visited too). `ok` reports the tool
    /// outcome regardless of any rewrite.
    pub after_tool: Option<AfterToolHook<'a>>,
    /// Once when the run stops — on `Completed` and `Cancelled` alike, before
    /// the stop event — with the turn count, the stop reason and the run's
    /// final history (what it leaves there is what the caller stores). It
    /// does not run on an errored run.
    pub turn_end: Option<TurnEndHook<'a>>,
}

/// The agent loop as a value (ADR-0011 D1): one run's tools, config, cancel
/// token and event subscription, with [`AgentRunner::run`] driving the loop
/// over a message history. The event subscription is the sink every
/// [`AgentEvent`] flows into; `slimcode-app`'s shared turn runner builds one
/// runner per turn. The optional hook seam is [`AgentRunner::hooks`]
/// (ADR-0015).
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
    /// The optional hook seam (defaults to all unset).
    pub hooks: RunHooks<'a>,
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
            hooks: RunHooks::default(),
        }
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
        mut messages: Vec<AgentMessage>,
    ) -> Result<(Vec<AgentMessage>, StopReason), String> {
        let mut iterations = 0usize;

        // The provider sees tools as a schema only: build the `ToolSpec` list
        // once per run (not per request) and hand it to every `chat` call
        // (ADR-0011 D1).
        let tool_specs: Vec<ToolSpec> = self.tools.iter().map(|t| t.spec.clone()).collect();

        // Unbounded loop: a coding agent runs until the model stops calling
        // tools (`FinishReason::Stop`), the caller cancels, or the provider
        // errors. There is intentionally no iteration cap — ending a run is
        // the model's job (no tool calls ⇒ final answer). Each `break` carries
        // the [`StopReason`] for that exit.
        let stop = 'run: loop {
            // Boundary: a cancel between iterations (or before the first
            // request) stops before any provider call is made.
            if self.cancel.is_cancelled() {
                break 'run StopReason::Cancelled;
            }
            iterations += 1;
            let turn = iterations;
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
            let result = provider.chat(
                &convert(system, &messages),
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
            );
            match result {
                Ok(()) => {}
                // A provider error that landed together with a cancel (its
                // interruptible read aborted) is a silent cancelled stop, not
                // an error.
                Err(_) if self.cancel.is_cancelled() && !sink_failed => {
                    break 'run StopReason::Cancelled;
                }
                Err(e) => return Err(e),
            }
            // A cancel that landed during the request discards the half
            // message: the streamed deltas stay on the transcript, but no
            // assistant message enters history.
            if self.cancel.is_cancelled() {
                break 'run StopReason::Cancelled;
            }
            let (text, tool_calls, reason) = assemble(&deltas);
            let mut asst = Message::text(Role::Assistant, text);
            asst.tool_calls = tool_calls.clone();
            let asst = AgentMessage::Llm(asst);
            messages.push(asst.clone());

            match reason {
                FinishReason::Stop => {
                    // Announce the history entry right after it is pushed, so
                    // the session layer persists the message at the moment it
                    // exists (ADR-0009 D2).
                    on_event_call(&mut self.on_event, AgentEvent::Message(asst))?;
                    break 'run StopReason::Completed;
                }
                FinishReason::ToolCalls => {
                    // The assistant-message event is emitted inside
                    // `execute_tools`, after the `before_tool` hooks have run:
                    // the session log, the display and the next request all
                    // carry the (possibly rewritten) message (ADR-0015 D3).
                    let cancelled = self.execute_tools(&mut messages)?;
                    if cancelled {
                        break 'run StopReason::Cancelled;
                    }
                    // fall through to the next turn (tool results are in
                    // history)
                }
            }
        };

        // The run-end hook (ADR-0015): the history it leaves here is exactly
        // what the caller stores. Runs on Completed and Cancelled, before the
        // stop event; never on the errored path.
        if let Some(hook) = self.hooks.turn_end.as_mut() {
            hook(&mut messages, iterations, &stop)?;
        }
        on_event_call(&mut self.on_event, AgentEvent::Stop(stop.clone()))?;
        Ok((messages, stop))
    }

    /// Build the tool-result message for a finished call, run the
    /// `after_tool` hook on it, and emit its `ToolStart`/`ToolResult` events
    /// (ADR-0015 D3: the hook and the events both happen before the log
    /// append). Returns the message to be pushed to history.
    fn build_result(
        &mut self,
        tc: &ToolCall,
        res: Result<String, String>,
    ) -> Result<AgentMessage, String> {
        let ok = res.is_ok();
        let content = match &res {
            Ok(body) => body.clone(),
            Err(err) => format!("Error: {err}"),
        };
        let mut msg = AgentMessage::tool_result(&tc.id, content);
        if let Some(hook) = self.hooks.after_tool.as_mut() {
            hook(&mut msg, ok)?;
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
    /// `messages[asst_index]` and append its results to history, emitting
    /// events through the subscription. Serial mode dispatches and appends one
    /// call at a time (so later tools can observe earlier results in history);
    /// parallel mode runs every call on its own scoped thread, streams tool
    /// events in **completion order**, then appends every result to history in
    /// **model order** (the `tool_calls` order) so the Session log stays
    /// deterministic.
    ///
    /// The `before_tool` hook runs for the whole batch first (model order,
    /// loop thread, before anything dispatches), so it can rewrite a call's
    /// arguments or skip it; a skipped call yields its supplied result without
    /// executing, and still produces a result message, events and an
    /// `after_tool` visit (ADR-0015 D4).
    ///
    /// Checks the cancel token before each dispatch (and after each result, so
    /// a tool that aborted itself mid-run — e.g. a cancelled bash child — does
    /// not push its aborted result). Returns `Ok(true)` when a cancel stopped
    /// the batch before it finished (results already pushed stay; the caller
    /// stops with [`StopReason::Cancelled`]).
    fn execute_tools(&mut self, messages: &mut Vec<AgentMessage>) -> Result<bool, String> {
        // The assistant message holding this batch is the last element at
        // entry; serial finishes push tool results after it, so its index is
        // captured once and reused for every `before_tool` re-read.
        let asst_index = messages
            .len()
            .checked_sub(1)
            .ok_or_else(|| "execute_tools: no assistant message".to_string())?;
        let calls: Vec<ToolCall> = messages[asst_index].tool_calls().to_vec();
        let mut effective: Vec<ToolCall> = calls.clone();
        let mut decisions: Vec<ToolDecision> = Vec::with_capacity(calls.len());

        // Phase 1 — `before_tool` for the whole batch (model order, loop
        // thread) before any dispatch. The effective calls are re-read from
        // the assistant message after each hook call, so a rewrite of a call's
        // arguments is what gets dispatched.
        if let Some(hook) = self.hooks.before_tool.as_mut() {
            for (index, eff) in effective.iter_mut().enumerate() {
                let asst = messages
                    .get_mut(asst_index)
                    .ok_or_else(|| "before_tool: assistant message vanished".to_string())?;
                let decision = hook(asst, index)?;
                *eff = asst
                    .tool_calls()
                    .get(index)
                    .cloned()
                    .ok_or_else(|| format!("before_tool removed tool call {index}"))?;
                decisions.push(decision);
            }
        } else {
            decisions = calls.iter().map(|_| ToolDecision::Run).collect();
        }

        // A `before_tool` hook may rewrite a call's arguments but not the
        // batch's shape: an added or removed `tool_calls` entry would leave a
        // call unpaired with a result on the next request (ADR-0015 D4).
        // Removals are caught per-index above; this catches additions.
        let final_calls = messages[asst_index].tool_calls().len();
        if final_calls != calls.len() {
            return Err(format!(
                "before_tool restructured the batch: {} -> {final_calls} tool calls",
                calls.len()
            ));
        }
        // Announce the assistant history entry now that the hooks have run:
        // the session layer and the display see exactly the message that
        // stays in history (ADR-0015 D3, ADR-0009 D2).
        on_event_call(
            &mut self.on_event,
            AgentEvent::Message(messages[asst_index].clone()),
        )?;

        if self.cfg.parallel_tools {
            // True concurrency: each call gets its own scoped thread; skipped
            // calls' results are known at phase-1 time, so they complete first
            // (model order among themselves), then the dispatched results flow
            // back over a channel in completion order.
            if self.cancel.is_cancelled() {
                return Ok(true);
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
            std::thread::scope(|scope| {
                let (tx, rx) = std::sync::mpsc::channel::<(usize, Result<String, String>)>();
                for (index, tc) in effective.iter().enumerate() {
                    if matches!(decisions[index], ToolDecision::Skip(_)) {
                        continue;
                    }
                    let tx = tx.clone();
                    let cancel = shared_cancel.clone();
                    scope.spawn(move || {
                        // A call whose batch was cancelled before it started
                        // never dispatches (and so never reports).
                        if cancel.is_cancelled() {
                            return;
                        }
                        let res = dispatch(tools, tc);
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
                return Ok(true);
            }
            // Tool events (and the after_tool hook) in completion order...
            let mut final_msgs: Vec<Option<AgentMessage>> = calls.iter().map(|_| None).collect();
            for &index in &completion_order {
                let res = results[index]
                    .take()
                    .expect("completion order carries a result");
                final_msgs[index] = Some(self.build_result(&effective[index], res)?);
            }
            // ...and history entries in model order (index), keeping the
            // Session log deterministic regardless of which call finished
            // first. A call with no result (only possible if a tool resets the
            // cancel token mid-run) is skipped rather than panicking.
            for slot in final_msgs.iter_mut() {
                if let Some(msg) = slot.take() {
                    messages.push(msg.clone());
                    on_event_call(&mut self.on_event, AgentEvent::Message(msg))?;
                }
            }
        } else {
            // Serial: dispatch, hook, emit and append one call at a time.
            for index in 0..calls.len() {
                if self.cancel.is_cancelled() {
                    return Ok(true);
                }
                let res = match &decisions[index] {
                    ToolDecision::Run => {
                        let res = dispatch(self.tools, &effective[index]);
                        if self.cancel.is_cancelled() {
                            return Ok(true);
                        }
                        res
                    }
                    ToolDecision::Skip(res) => res.clone(),
                };
                let msg = self.build_result(&effective[index], res)?;
                messages.push(msg.clone());
                on_event_call(&mut self.on_event, AgentEvent::Message(msg))?;
            }
        }
        Ok(false)
    }
}

/// Invoke the event subscription once (a `&mut dyn FnMut` stored in a field).
fn on_event_call(sink: &mut EventSink<'_>, event: AgentEvent) -> Result<(), String> {
    sink(event)
}

#[cfg(test)]
// Test hooks are built iteratively (`let mut hooks = default; hooks.x = …`)
// because a struct literal with three optional closures is unreadable.
#[allow(clippy::field_reassign_with_default)]
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
    }

    impl FakeProvider {
        fn new(script: Vec<Vec<Delta>>) -> Self {
            Self {
                script,
                calls: 0,
                cancel_on_call: None,
                cancel: None,
            }
        }

        fn cancels_on(mut self, call: usize, token: &CancelToken) -> Self {
            self.cancel_on_call = Some(call);
            self.cancel = Some(token.clone());
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
        ) -> Result<(), String> {
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
            Ok(())
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

    /// The loop test harness with caller-supplied hooks; events are collected
    /// into a caller-owned Vec (passed in so the sink's borrow ends when this
    /// helper returns). The caller's `hooks` is moved into the runner (left
    /// default after), so recorders inside the hooks stay inspectable via
    /// shared handles.
    ///
    /// The harness runs with a fixed test config: these loop tests exercise
    /// the agent loop, not config resolution (that lives in `slimcode-app`).
    #[allow(clippy::too_many_arguments)] // a test harness bundling run context
    fn run_with_hooks_collect<P: Provider>(
        provider: &mut P,
        tools: &[Tool],
        cfg: RunConfig,
        cancel: &CancelToken,
        system: &Message,
        messages: Vec<AgentMessage>,
        hooks: &mut RunHooks<'static>,
        events: &mut Vec<AgentEvent>,
    ) -> Result<(Vec<AgentMessage>, StopReason), String> {
        let config = ProviderConfig::new("test-key", "https://example.invalid/v1", "test-model");
        let mut sink = |e: AgentEvent| {
            events.push(e);
            Ok(())
        };
        let mut runner = AgentRunner::new(tools, cfg, &config, cancel, &mut sink);
        runner.hooks = std::mem::take(hooks);
        runner.run(provider, system, messages)
    }

    /// The loop test harness with caller-supplied hooks and a collecting event
    /// sink. Returns the run's observables so tests assert external behaviour
    /// only.
    fn run_with_hooks<P: Provider>(
        provider: &mut P,
        tools: &[Tool],
        cfg: RunConfig,
        cancel: &CancelToken,
        system: &Message,
        messages: Vec<AgentMessage>,
        hooks: &mut RunHooks<'static>,
    ) -> Result<(Vec<AgentMessage>, StopReason, Vec<AgentEvent>), String> {
        let mut events: Vec<AgentEvent> = Vec::new();
        let (messages, stop) = run_with_hooks_collect(
            provider,
            tools,
            cfg,
            cancel,
            system,
            messages,
            hooks,
            &mut events,
        )?;
        Ok((messages, stop, events))
    }

    /// The same harness with no hooks set.
    fn run_with(
        provider: &mut impl Provider,
        tools: &[Tool],
        cfg: RunConfig,
        cancel: &CancelToken,
        system: &Message,
        messages: Vec<AgentMessage>,
    ) -> Result<(Vec<AgentMessage>, StopReason, Vec<AgentEvent>), String> {
        let mut hooks = RunHooks::default();
        run_with_hooks(provider, tools, cfg, cancel, system, messages, &mut hooks)
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
                AgentEvent::Message(m) => Some(m),
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
                AgentEvent::Message(m) => {
                    if m.role() == &Role::Assistant {
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
            ) -> Result<(), String> {
                on_delta(t("half "))?;
                std::thread::sleep(std::time::Duration::from_millis(150));
                self.returned.store(true, Ordering::SeqCst);
                on_delta(t("answer"))?;
                on_delta(done_stop())
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
            ) -> Result<(), String> {
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
            ) -> Result<(), String> {
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

    // --- Run hooks (ADR-0015) ----------------------------------------------

    #[test]
    fn before_tool_runs_for_the_whole_batch_before_any_dispatch() {
        // The whole batch is hooked (model order) before anything dispatches,
        // even in parallel mode: each hook call must observe dispatch has not
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
        let mut hooks = RunHooks::default();
        hooks.before_tool = Some(Box::new(move |_asst: &mut AgentMessage, index: usize| {
            assert!(
                !dp.load(Ordering::SeqCst),
                "dispatch happened before before_tool({index})"
            );
            od.lock().unwrap().push(index);
            Ok(ToolDecision::Run)
        }));
        let tools = [flag];
        let (messages, stop, _) = run_with_hooks(
            &mut FakeProvider::new(script),
            &tools,
            RunConfig::default(),
            &CancelToken::new(),
            &default_system(),
            vec![AgentMessage::text(Role::User, "user")],
            &mut hooks,
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
    fn before_tool_rewrites_arguments_the_tool_then_sees() {
        // The hook rewrites call 0's arguments; the tool must run with the
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
        let mut hooks = RunHooks::default();
        hooks.before_tool = Some(Box::new(|asst: &mut AgentMessage, _index: usize| {
            asst.llm_mut().unwrap().tool_calls[0].arguments = "{\"city\": \"Beijing\"}".to_string();
            Ok(ToolDecision::Run)
        }));
        let tools = [echo];
        let (messages, _stop, events) = run_with_hooks(
            &mut FakeProvider::new(script),
            &tools,
            RunConfig::default(),
            &CancelToken::new(),
            &default_system(),
            vec![AgentMessage::text(Role::User, "user")],
            &mut hooks,
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
    fn skip_supplies_the_result_without_running_the_tool() {
        // A Skip(Ok) call never dispatches; its supplied result enters history
        // and still renders as a normal paired block (ToolStart/ToolResult)
        // and still visits after_tool.
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
        let visited = Arc::new(Mutex::new(Vec::<(String, bool)>::new()));
        let v = visited.clone();
        let mut hooks = RunHooks::default();
        hooks.before_tool = Some(Box::new(|_asst: &mut AgentMessage, _index: usize| {
            Ok(ToolDecision::Skip(Ok("denied".to_string())))
        }));
        hooks.after_tool = Some(Box::new(move |msg: &mut AgentMessage, ok: bool| {
            v.lock()
                .unwrap()
                .push((msg.tool_call_id().unwrap().to_string(), ok));
            Ok(())
        }));
        let tools = [tool];
        let (messages, stop, events) = run_with_hooks(
            &mut FakeProvider::new(script),
            &tools,
            RunConfig::default(),
            &CancelToken::new(),
            &default_system(),
            vec![AgentMessage::text(Role::User, "user")],
            &mut hooks,
        )
        .unwrap();
        assert_eq!(stop, StopReason::Completed);
        assert!(!ran.load(Ordering::SeqCst), "the tool must not run");
        assert_eq!(*visited.lock().unwrap(), vec![("call_0".to_string(), true)]);
        let tool_msgs: Vec<_> = messages
            .iter()
            .filter(|m| m.role() == &Role::Tool)
            .collect();
        assert_eq!(tool_msgs.len(), 1);
        assert!(tool_msgs[0].text_content().contains("denied"));
        assert!(events.iter().any(
            |e| matches!(e, AgentEvent::ToolStart { tool_call_id, .. } if tool_call_id == "call_0")
        ));
        assert!(events.iter().any(|e| matches!(
            e,
            AgentEvent::ToolResult { tool_call_id, ok: true, .. } if tool_call_id == "call_0"
        )));
    }

    #[test]
    fn skip_err_produces_the_error_result_shape() {
        // A Skip(Err) is told to the model as the `Error: …` shape tool
        // failures already use, and after_tool sees `ok == false`.
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
        let mut hooks = RunHooks::default();
        hooks.before_tool = Some(Box::new(|_asst: &mut AgentMessage, _index: usize| {
            Ok(ToolDecision::Skip(Err("not allowed".to_string())))
        }));
        hooks.after_tool = Some(Box::new(move |_msg: &mut AgentMessage, ok: bool| {
            *o.lock().unwrap() = Some(ok);
            Ok(())
        }));
        let tools = [tool];
        let (messages, stop, _) = run_with_hooks(
            &mut FakeProvider::new(script),
            &tools,
            RunConfig::default(),
            &CancelToken::new(),
            &default_system(),
            vec![AgentMessage::text(Role::User, "user")],
            &mut hooks,
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
    fn after_tool_rewrite_is_what_history_and_events_carry() {
        // The hook rewrites the result message; both the returned history and
        // the ToolResult event carry the rewrite (the display and the log see
        // one text).
        let script = vec![vec![
            tc_start(0, "call_0", "get_weather"),
            tc_args(0, "{}"),
            done_tools(),
        ]];
        let mut hooks = RunHooks::default();
        hooks.after_tool = Some(Box::new(|msg: &mut AgentMessage, _ok: bool| {
            let id = msg.tool_call_id().map(str::to_string);
            let mut wire = Message::text(Role::Tool, "redacted");
            wire.tool_call_id = id;
            *msg = AgentMessage::llm(wire);
            Ok(())
        }));
        let tools = [weather_tool()];
        let (messages, _stop, events) = run_with_hooks(
            &mut FakeProvider::new(script),
            &tools,
            RunConfig::default(),
            &CancelToken::new(),
            &default_system(),
            vec![AgentMessage::text(Role::User, "user")],
            &mut hooks,
        )
        .unwrap();
        let tool_msg = messages.iter().find(|m| m.role() == &Role::Tool).unwrap();
        assert_eq!(tool_msg.text_content(), "redacted");
        assert!(
            events.iter().any(
                |e| matches!(e, AgentEvent::ToolResult { result, .. } if result == "redacted")
            )
        );
    }

    #[test]
    fn before_tool_removing_a_call_errors() {
        // A hook may rewrite a call's arguments but not restructure the batch:
        // a vanished index is an error, not a panic.
        let mut hooks = RunHooks::default();
        hooks.before_tool = Some(Box::new(|asst: &mut AgentMessage, _index: usize| {
            asst.llm_mut().unwrap().tool_calls.clear();
            Ok(ToolDecision::Run)
        }));
        let script = vec![vec![
            tc_start(0, "call_0", "get_weather"),
            tc_args(0, "{}"),
            done_tools(),
        ]];
        let tools = [weather_tool()];
        let err = run_with_hooks(
            &mut FakeProvider::new(script),
            &tools,
            RunConfig::default(),
            &CancelToken::new(),
            &default_system(),
            vec![AgentMessage::text(Role::User, "user")],
            &mut hooks,
        )
        .unwrap_err();
        assert!(
            err.contains("before_tool removed tool call 0"),
            "got: {err}"
        );
    }

    #[test]
    fn before_tool_adding_a_call_errors() {
        // Restructuring also covers additions: an added `tool_calls` entry
        // would leave a call unpaired with a result on the next request.
        let mut hooks = RunHooks::default();
        hooks.before_tool = Some(Box::new(|asst: &mut AgentMessage, _index: usize| {
            asst.llm_mut().unwrap().tool_calls.push(ToolCall {
                id: "extra".to_string(),
                name: "get_weather".to_string(),
                arguments: "{}".to_string(),
            });
            Ok(ToolDecision::Run)
        }));
        let script = vec![vec![
            tc_start(0, "call_0", "get_weather"),
            tc_args(0, "{}"),
            done_tools(),
        ]];
        let tools = [weather_tool()];
        let err = run_with_hooks(
            &mut FakeProvider::new(script),
            &tools,
            RunConfig::default(),
            &CancelToken::new(),
            &default_system(),
            vec![AgentMessage::text(Role::User, "user")],
            &mut hooks,
        )
        .unwrap_err();
        assert!(
            err.contains("before_tool restructured the batch: 1 -> 2 tool calls"),
            "got: {err}"
        );
    }

    #[test]
    fn before_tool_rewrite_reaches_the_assistant_message_event() {
        // The assistant-message event is emitted after the `before_tool`
        // hooks, so the session log (persisted from events) and the returned
        // history carry the same rewritten arguments (ADR-0015 D3).
        let mut hooks = RunHooks::default();
        hooks.before_tool = Some(Box::new(|asst: &mut AgentMessage, _index: usize| {
            asst.llm_mut().unwrap().tool_calls[0].arguments = "{\"city\": \"Beijing\"}".to_string();
            Ok(ToolDecision::Run)
        }));
        let script = vec![vec![
            tc_start(0, "call_0", "get_weather"),
            tc_args(0, "{\"city\": \"Shanghai\"}"),
            done_tools(),
        ]];
        let tools = [weather_tool()];
        let (messages, _stop, events) = run_with_hooks(
            &mut FakeProvider::new(script),
            &tools,
            RunConfig::default(),
            &CancelToken::new(),
            &default_system(),
            vec![AgentMessage::text(Role::User, "user")],
            &mut hooks,
        )
        .unwrap();
        // The announced assistant message is the post-hook one, matching what
        // stays in the returned history.
        let announced = events
            .iter()
            .find_map(|e| match e {
                AgentEvent::Message(m) if m.role() == &Role::Assistant => Some(m.clone()),
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
    fn hook_error_aborts_the_run() {
        // A hook's `Err` propagates verbatim and no Stop event is emitted.
        let mut hooks = RunHooks::default();
        hooks.before_tool = Some(Box::new(|_asst: &mut AgentMessage, _index: usize| {
            Err("gate".to_string())
        }));
        let script = vec![vec![
            tc_start(0, "call_0", "get_weather"),
            tc_args(0, "{}"),
            done_tools(),
        ]];
        let tools = [weather_tool()];
        let err = run_with_hooks(
            &mut FakeProvider::new(script),
            &tools,
            RunConfig::default(),
            &CancelToken::new(),
            &default_system(),
            vec![AgentMessage::text(Role::User, "user")],
            &mut hooks,
        )
        .unwrap_err();
        assert_eq!(err, "gate");
    }

    #[test]
    fn after_tool_receives_the_ok_flag() {
        // after_tool sees `ok == false` for a tool that failed, regardless of
        // any rewrite.
        let script = vec![vec![
            tc_start(0, "call_1", "fragile_tool"),
            tc_args(0, "{\"city\": \"Tokyo\"}"),
            done_tools(),
        ]];
        let ok_seen = Arc::new(Mutex::new(None::<bool>));
        let o = ok_seen.clone();
        let mut hooks = RunHooks::default();
        hooks.after_tool = Some(Box::new(move |_msg: &mut AgentMessage, ok: bool| {
            *o.lock().unwrap() = Some(ok);
            Ok(())
        }));
        let tools = [fragile_tool()];
        let (_, stop, _) = run_with_hooks(
            &mut FakeProvider::new(script),
            &tools,
            RunConfig::default(),
            &CancelToken::new(),
            &default_system(),
            vec![AgentMessage::text(Role::User, "user")],
            &mut hooks,
        )
        .unwrap();
        assert_eq!(stop, StopReason::Completed);
        assert_eq!(*ok_seen.lock().unwrap(), Some(false));
    }

    #[test]
    fn turn_end_runs_on_completed_with_the_final_history() {
        let script = vec![vec![t("done"), done_stop()]];
        let seen = Arc::new(Mutex::new(None::<(usize, StopReason)>));
        let s = seen.clone();
        let mut hooks = RunHooks::default();
        hooks.turn_end = Some(Box::new(
            move |msgs: &mut Vec<AgentMessage>, turns: usize, stop: &StopReason| {
                *s.lock().unwrap() = Some((turns, stop.clone()));
                msgs.push(AgentMessage::text(Role::User, "sealed"));
                Ok(())
            },
        ));
        let tools: [Tool; 0] = [];
        let (messages, stop, _) = run_with_hooks(
            &mut FakeProvider::new(script),
            &tools,
            RunConfig::default(),
            &CancelToken::new(),
            &default_system(),
            vec![AgentMessage::text(Role::User, "user")],
            &mut hooks,
        )
        .unwrap();
        assert_eq!(stop, StopReason::Completed);
        assert_eq!(*seen.lock().unwrap(), Some((1, StopReason::Completed)));
        assert_eq!(messages.last().unwrap().text_content(), "sealed");
    }

    #[test]
    fn turn_end_runs_on_cancelled() {
        let cancel = CancelToken::new();
        cancel.cancel();
        let seen = Arc::new(Mutex::new(None::<(usize, StopReason)>));
        let s = seen.clone();
        let mut hooks = RunHooks::default();
        hooks.turn_end = Some(Box::new(
            move |_msgs: &mut Vec<AgentMessage>, turns: usize, stop: &StopReason| {
                *s.lock().unwrap() = Some((turns, stop.clone()));
                Ok(())
            },
        ));
        let tools: [Tool; 0] = [];
        let (_, stop, _) = run_with_hooks(
            &mut FakeProvider::new(vec![vec![t("never"), done_stop()]]),
            &tools,
            RunConfig::default(),
            &cancel,
            &default_system(),
            vec![AgentMessage::text(Role::User, "user")],
            &mut hooks,
        )
        .unwrap();
        assert_eq!(stop, StopReason::Cancelled);
        assert_eq!(*seen.lock().unwrap(), Some((0, StopReason::Cancelled)));
    }

    #[test]
    fn turn_end_does_not_run_when_the_run_errors() {
        struct ErrProvider;
        impl Provider for ErrProvider {
            fn chat(
                &mut self,
                _m: &[Message],
                _t: &[ToolSpec],
                _cfg: &ProviderConfig,
                _c: &CancelToken,
                _on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
            ) -> Result<(), String> {
                Err("boom".to_string())
            }
        }
        let called = Arc::new(AtomicBool::new(false));
        let c = called.clone();
        let mut hooks = RunHooks::default();
        hooks.turn_end = Some(Box::new(
            move |_msgs: &mut Vec<AgentMessage>, _turns: usize, _stop: &StopReason| {
                c.store(true, Ordering::SeqCst);
                Ok(())
            },
        ));
        let tools: [Tool; 0] = [];
        let err = run_with_hooks(
            &mut ErrProvider,
            &tools,
            RunConfig::default(),
            &CancelToken::new(),
            &default_system(),
            vec![AgentMessage::text(Role::User, "user")],
            &mut hooks,
        )
        .unwrap_err();
        assert!(err.contains("boom"));
        assert!(
            !called.load(Ordering::SeqCst),
            "turn_end must not run on an errored run"
        );
    }

    #[test]
    fn skipped_calls_report_before_dispatched_ones_in_parallel_mode() {
        // In parallel mode a skipped call's result is known at phase-1 time,
        // so it completes before dispatched calls: its ToolResult event comes
        // first even though the dispatched call finishes later.
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
        let mut hooks = RunHooks::default();
        hooks.before_tool = Some(Box::new(|_asst: &mut AgentMessage, index: usize| {
            if index == 0 {
                Ok(ToolDecision::Skip(Ok("denied".to_string())))
            } else {
                Ok(ToolDecision::Run)
            }
        }));
        let tools = [slow];
        let (messages, _stop, events) = run_with_hooks(
            &mut FakeProvider::new(script),
            &tools,
            RunConfig {
                parallel_tools: true,
            },
            &CancelToken::new(),
            &default_system(),
            vec![AgentMessage::text(Role::User, "user")],
            &mut hooks,
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
}
