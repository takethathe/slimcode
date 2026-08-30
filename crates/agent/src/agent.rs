//! Agent runtime loop, folded from the ticket 04 prototype.
//!
//! Shape locked by `.scratch/slimcode-v1` ticket 04:
//! - loop: model response with `tool_calls` → execute tools → append tool
//!   results → loop, until the model stops calling tools;
//! - stop conditions: no tool_calls → `Completed`; iteration cap →
//!   `MaxIterations` (the runtime's only hard safety; user interrupt is a CLI
//!   layer concern);
//! - tool execution defaults to **serial** (local tool engines are naturally
//!   serial and share no concurrent state); `parallel_tools` is a switch
//!   reserved for future I/O-heavy tools;
//! - tool errors are surfaced to the model as a `role: tool` message prefixed
//!   `Error: …` so it can recover naturally;
//! - deltas mirror the live Bailian wire shape (ticket 05): `reasoning` before
//!   `content`; a tool call starts with `ToolCallStart` (id + name) followed by
//!   `ToolCallArgs` fragments; `assemble` re-joins them.
//!
//! The `Provider` trait is the seam crates/ai will implement against the real
//! Bailian/OpenAI-compatible endpoint. It is synchronous for now so the loop
//! stays dependency-free and testable; the async boundary lives inside the
//! provider implementation (a blocking call / runtime handle).

pub use crate::session::{Message, Role, ToolCall};
use serde_json::Value;
use std::collections::BTreeMap;

/// Why a turn of generation ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FinishReason {
    Stop,
    ToolCalls,
}

/// One streaming delta from the provider — the unit surfaced to the CLI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Delta {
    /// Thinking tokens (qwen-style `reasoning_content`).
    Reasoning(String),
    /// Assistant text fragment.
    Text(String),
    /// First fragment of a tool call: carries `id` + `name` (and `index`).
    ToolCallStart {
        index: usize,
        id: String,
        name: String,
    },
    /// Subsequent fragment: only an arguments slice (id/name are empty on the
    /// wire for these).
    ToolCallArgs { index: usize, fragment: String },
    /// End-of-turn marker.
    Done(FinishReason),
}

/// A registered tool the agent can invoke.
pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    /// Runs the tool against parsed JSON arguments. `Err` becomes an
    /// `Error: …` tool message in history.
    pub run: Box<dyn Fn(Value) -> Result<String, String>>,
}

impl Tool {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
        run: impl Fn(Value) -> Result<String, String> + 'static,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
            run: Box::new(run),
        }
    }
}

/// The provider seam. crates/ai implements this against the real endpoint.
pub trait Provider {
    /// One turn of generation over `messages` with `tools` available.
    /// Returns the raw delta stream for this turn.
    fn chat(&mut self, messages: &[Message], tools: &[Tool]) -> Result<Vec<Delta>, String>;
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
    /// Hit the iteration cap before the model finished.
    MaxIterations,
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
        name: String,
        arguments: String,
    },
    ToolResult {
        name: String,
        ok: bool,
        result: String,
    },
    Stop(StopReason),
}

#[derive(Clone, Debug)]
pub struct RunConfig {
    pub max_iterations: usize,
    /// Execute multiple tool calls from one response concurrently, or one at a
    /// time (appending results as we go).
    pub parallel_tools: bool,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            max_iterations: 10,
            parallel_tools: false,
        }
    }
}

#[derive(Debug)]
pub struct RunResult {
    pub messages: Vec<Message>,
    pub iterations: usize,
    pub stop: StopReason,
    pub events: Vec<AgentEvent>,
}

fn dispatch(tools: &[Tool], tc: &ToolCall) -> Result<String, String> {
    match tools.iter().find(|t| t.name == tc.name) {
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
type EventSink<'a> = &'a mut dyn FnMut(AgentEvent) -> Result<(), String>;

/// Append one tool result to history, emitting ToolStart/ToolResult events
/// through the sink. `tc` is the tool call this result belongs to.
fn push_tool_result(
    tc: &ToolCall,
    res: Result<String, String>,
    on_event: EventSink<'_>,
    messages: &mut Vec<Message>,
) -> Result<(), String> {
    let (ok, body) = match res {
        Ok(o) => (true, o),
        Err(e) => (false, e),
    };
    on_event(AgentEvent::ToolStart {
        name: tc.name.clone(),
        arguments: tc.arguments.clone(),
    })?;
    on_event(AgentEvent::ToolResult {
        name: tc.name.clone(),
        ok,
        result: body.clone(),
    })?;
    let content = if ok { body } else { format!("Error: {body}") };
    messages.push(Message::tool_result(&tc.id, content));
    Ok(())
}

/// Execute tool calls and append their results to `messages`, emitting events
/// through the sink. In serial mode each result is appended before the next
/// call runs (so later tools can observe earlier results in history); in
/// parallel mode all calls run first and results are appended together.
fn execute_tools(
    tools: &[Tool],
    calls: &[ToolCall],
    parallel: bool,
    on_event: EventSink<'_>,
    messages: &mut Vec<Message>,
) -> Result<(), String> {
    if parallel {
        // Run every call first, then append all results together.
        let results: Vec<(&ToolCall, Result<String, String>)> =
            calls.iter().map(|tc| (tc, dispatch(tools, tc))).collect();
        for (tc, res) in results {
            push_tool_result(tc, res, on_event, messages)?;
        }
    } else {
        // Serial: dispatch and append one call at a time.
        for tc in calls {
            let res = dispatch(tools, tc);
            push_tool_result(tc, res, on_event, messages)?;
        }
    }
    Ok(())
}

/// The agent loop over a freshly built `[system, user]` history — the simple
/// one-shot entry point (used by the non-interactive CLI mode).
pub fn run_agent<P: Provider>(
    provider: &mut P,
    tools: &[Tool],
    system: &str,
    user: &str,
    cfg: &RunConfig,
) -> Result<RunResult, String> {
    let messages = vec![
        Message::text(Role::System, system),
        Message::text(Role::User, user),
    ];
    run_agent_from_messages(provider, tools, messages, cfg)
}

/// The shared loop: drives one turn over `messages`, forwarding every event to
/// `on_event` as it happens, and returns `(updated history, iterations, stop)`.
/// A sink error (e.g. a renderer failure) aborts the run.
fn run_loop<P: Provider>(
    provider: &mut P,
    tools: &[Tool],
    mut messages: Vec<Message>,
    cfg: &RunConfig,
    on_event: EventSink<'_>,
) -> Result<(Vec<Message>, usize, StopReason), String> {
    let mut stop = StopReason::Completed;
    let mut iterations = 0usize;

    for turn in 1..=cfg.max_iterations {
        iterations = turn;
        on_event(AgentEvent::Turn { turn })?;

        let deltas = provider.chat(&messages, tools)?;
        for d in &deltas {
            on_event(AgentEvent::Stream(d.clone()))?;
        }
        let (text, tool_calls, reason) = assemble(&deltas);
        let mut asst = Message::text(Role::Assistant, text);
        asst.tool_calls = tool_calls.clone();
        messages.push(asst);

        match reason {
            FinishReason::Stop => {
                stop = StopReason::Completed;
                break;
            }
            FinishReason::ToolCalls => {
                execute_tools(
                    tools,
                    &tool_calls,
                    cfg.parallel_tools,
                    on_event,
                    &mut messages,
                )?;
                // fall through to the next turn (tool results are in history)
            }
        }
        if turn == cfg.max_iterations {
            stop = StopReason::MaxIterations;
        }
    }

    on_event(AgentEvent::Stop(stop.clone()))?;
    Ok((messages, iterations, stop))
}

/// The agent loop over an existing message history (already including the
/// latest user message), streaming every event to `on_event` live and
/// returning the updated message history. Used by `slimcode-common`'s shared
/// runner to render live through a `Renderer`.
pub fn run_agent_from_messages_sink<P: Provider>(
    provider: &mut P,
    tools: &[Tool],
    messages: Vec<Message>,
    cfg: &RunConfig,
    on_event: EventSink<'_>,
) -> Result<Vec<Message>, String> {
    let (messages, _, _) = run_loop(provider, tools, messages, cfg, on_event)?;
    Ok(messages)
}

/// The agent loop over an existing message history (already including the
/// latest user message). Used by an interactive frontend to continue a
/// restored session: assistant replies and tool results are appended to
/// `messages`, so the caller replaces its stored history with
/// `RunResult::messages`.
pub fn run_agent_from_messages<P: Provider>(
    provider: &mut P,
    tools: &[Tool],
    messages: Vec<Message>,
    cfg: &RunConfig,
) -> Result<RunResult, String> {
    let mut events: Vec<AgentEvent> = Vec::new();
    let (messages, iterations, stop) = run_loop(provider, tools, messages, cfg, &mut |e| {
        events.push(e);
        Ok(())
    })?;
    Ok(RunResult {
        messages,
        iterations,
        stop,
        events,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Scripted provider: pops the next delta sequence per call.
    struct FakeProvider {
        script: Vec<Vec<Delta>>,
        calls: usize,
    }

    impl FakeProvider {
        fn new(script: Vec<Vec<Delta>>) -> Self {
            Self { script, calls: 0 }
        }
    }

    impl Provider for FakeProvider {
        fn chat(&mut self, _messages: &[Message], _tools: &[Tool]) -> Result<Vec<Delta>, String> {
            let d = self.script.get(self.calls).cloned().unwrap_or_default();
            self.calls += 1;
            Ok(d)
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

    fn run(
        script: Vec<Vec<Delta>>,
        tools: Vec<Tool>,
        cfg: &RunConfig,
    ) -> Result<RunResult, String> {
        let mut p = FakeProvider::new(script);
        run_agent(&mut p, &tools, "be helpful", "weather in Beijing?", cfg)
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
        let res = run(
            vec![vec![t("Beijing is sunny."), done_stop()]],
            vec![weather_tool()],
            &RunConfig::default(),
        )
        .unwrap();
        assert_eq!(res.stop, StopReason::Completed);
        assert_eq!(res.iterations, 1);
        assert_eq!(res.messages.len(), 3); // system + user + assistant
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
        let res = run(script, vec![weather_tool()], &RunConfig::default()).unwrap();
        assert_eq!(res.stop, StopReason::Completed);
        assert_eq!(res.iterations, 2);
        // system + user + assistant(tool_calls) + tool + assistant(final)
        assert_eq!(res.messages.len(), 5);
        // the tool message carries the result
        let tool_msg = res.messages.iter().find(|m| m.role == Role::Tool).unwrap();
        assert_eq!(tool_msg.tool_call_id.as_deref(), Some("call_1"));
        assert!(tool_msg.text_content().contains("25C"));
        // events expose the full surface
        assert!(res.events.iter().any(
            |e| matches!(e, AgentEvent::ToolResult { name, ok: true, .. } if name == "get_weather")
        ));
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
        let res = run(
            script,
            vec![weather_tool()],
            &RunConfig {
                max_iterations: 5,
                parallel_tools: true,
            },
        )
        .unwrap();
        let tool_msgs: Vec<_> = res
            .messages
            .iter()
            .filter(|m| m.role == Role::Tool)
            .collect();
        assert_eq!(
            tool_msgs.len(),
            2,
            "both parallel calls' results are in history"
        );
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
        let res = run(script, vec![fragile_tool()], &RunConfig::default()).unwrap();
        assert_eq!(res.stop, StopReason::Completed);
        let tool_msg = res.messages.iter().find(|m| m.role == Role::Tool).unwrap();
        assert!(
            tool_msg.text_content().starts_with("Error: "),
            "got: {}",
            tool_msg.text_content()
        );
        // the final assistant answer still comes through
        let last = res.messages.last().unwrap();
        assert_eq!(last.role, Role::Assistant);
        assert!(last.text_content().contains("Tokyo"));
    }

    #[test]
    fn runaway_loop_stops_at_max_iterations() {
        let mut script = Vec::new();
        for i in 0..5 {
            script.push(vec![
                tc_start(0, &format!("call_{i}"), "get_weather"),
                tc_args(0, "{\"city\": \"Beijing\"}"),
                done_tools(),
            ]);
        }
        let res = run(
            script,
            vec![weather_tool()],
            &RunConfig {
                max_iterations: 3,
                parallel_tools: false,
            },
        )
        .unwrap();
        assert_eq!(res.stop, StopReason::MaxIterations);
        assert_eq!(res.iterations, 3);
    }

    #[test]
    fn run_agent_from_messages_continues_existing_history() {
        // A restored session already has a system + an old assistant reply;
        // the frontend appends a new user message and continues from there.
        let history = vec![
            Message::text(Role::System, "be helpful"),
            Message::text(Role::Assistant, "Earlier answer."),
            Message::text(Role::User, "Now: what is the weather?"),
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
        let res = run_agent_from_messages(
            &mut p,
            &[weather_tool()],
            history.clone(),
            &RunConfig::default(),
        )
        .unwrap();
        assert_eq!(res.stop, StopReason::Completed);
        // old history preserved + assistant(tool_calls) + tool + assistant(final)
        assert_eq!(res.messages.len(), history.len() + 3);
        assert_eq!(res.messages[0].role, Role::System);
        assert_eq!(res.messages[1].role, Role::Assistant);
        assert!(res.messages[2].text_content().contains("weather"));
        // provider received the full history on its first chat call
        assert_eq!(p.calls, 2);
    }

    #[test]
    fn provider_error_aborts_the_run() {
        struct ErrProvider;
        impl Provider for ErrProvider {
            fn chat(&mut self, _m: &[Message], _t: &[Tool]) -> Result<Vec<Delta>, String> {
                Err("provider exploded".to_string())
            }
        }
        let mut p = ErrProvider;
        let err = run_agent(&mut p, &[], "sys", "user", &RunConfig::default()).unwrap_err();
        assert!(err.contains("provider exploded"));
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
        let res = run(script, vec![weather_tool()], &RunConfig::default()).unwrap();
        let tool_msg = res.messages.iter().find(|m| m.role == Role::Tool).unwrap();
        assert!(tool_msg.text_content().contains("unknown tool: nope"));
    }
}
