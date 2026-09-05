//! PROTOTYPE for slimcode ticket 04 — agent runtime loop shape.
//!
//! Throwaway, zero assumptions. Explores the runtime against a *scripted fake
//! provider* (no real HTTP) to lock decisions on:
//!   - message / turn model (system/user/assistant/tool, content parts, tool_calls)
//!   - the tool loop (response with tool_calls -> execute -> append -> loop),
//!     serial vs parallel tool execution
//!   - stop conditions (no tool_calls, max_iterations, explicit stop)
//!   - streaming surface: how text + tool_call deltas are forwarded to the CLI
//!   - session state: in-memory shape + JSON serialization boundary
//!
//! Delta shapes deliberately mirror what the live Bailian spike (ticket 05)
//! observed: reasoning_content deltas first, then content; tool_call start
//! carries id/type/index/name, later fragments carry only index + arguments.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Message / turn model (also the session JSON boundary)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// A model-emitted tool invocation (assistant messages only).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Raw JSON string as the model emitted it; parsed only at execution time.
    pub arguments: String,
}

/// One content part of a message. v1 only uses `Text`; the parts abstraction
/// leaves room for image/tool parts later without changing the message shape.
/// Serialized shape mirrors pi's TextPart: `{"type":"text","text":"..."}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Part {
    Text { text: String },
}

/// One message in the conversation history.
///
/// JSON boundary decision: content lives in `parts` (a `Text` part is the
/// default/only kind for v1); `tool_calls` and `tool_call_id` are omitted from
/// the serialized session when absent, keeping non-tool messages compact.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub parts: Vec<Part>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    fn new(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            parts: vec![Part::Text { text: content.into() }],
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    fn tool_result(id: String, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            parts: vec![Part::Text { text: content.into() }],
            tool_calls: Vec::new(),
            tool_call_id: Some(id),
        }
    }
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

/// A registered tool. `run` is a plain function pointer — no captures, which is
/// enough for a throwaway (real engine will use Box<dyn Fn> / async).
pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: Value,
    pub run: fn(Value) -> Result<String, String>,
}

fn tool_get_weather(args: Value) -> Result<String, String> {
    let city = args.get("city").and_then(Value::as_str).unwrap_or("?").to_string();
    Ok(format!("{{\"city\": \"{city}\", \"temp\": \"25C\"}}"))
}

fn tool_get_time(args: Value) -> Result<String, String> {
    let city = args.get("city").and_then(Value::as_str).unwrap_or("?").to_string();
    Ok(format!("{{\"city\": \"{city}\", \"time\": \"14:05\"}}"))
}

fn tool_fragile(args: Value) -> Result<String, String> {
    let city = args.get("city").and_then(Value::as_str).unwrap_or("?");
    if city == "Tokyo" {
        Err(format!("no weather station for {city}"))
    } else {
        Ok(format!("{{\"city\": \"{city}\", \"temp\": \"18C\"}}"))
    }
}

fn default_tools() -> BTreeMap<String, Tool> {
    let mut t = BTreeMap::new();
    t.insert(
        "get_weather".to_string(),
        Tool {
            name: "get_weather",
            description: "Get current weather for a city",
            parameters: serde_json::json!({"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]}),
            run: tool_get_weather,
        },
    );
    t.insert(
        "get_time".to_string(),
        Tool {
            name: "get_time",
            description: "Get current local time for a city",
            parameters: serde_json::json!({"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]}),
            run: tool_get_time,
        },
    );
    t.insert(
        "fragile_tool".to_string(),
        Tool {
            name: "fragile_tool",
            description: "Fails for some cities on purpose (prototype)",
            parameters: serde_json::json!({"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]}),
            run: tool_fragile,
        },
    );
    t
}

/// Session wrapper: metadata around the message history, ready for the CLI's
/// `~/.slimcode/sessions/` JSON files. The messages themselves round-trip
/// losslessly (prototype rule); metadata fields are settled by the CLI design.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub created_at: String,
    pub messages: Vec<Message>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

impl Session {
    fn new(id: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            id: id.into(),
            created_at: "2026-08-29T00:00:00Z".to_string(),
            messages,
            title: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Provider + streaming deltas (mirror ticket 05 wire shapes)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum FinishReason {
    Stop,
    ToolCalls,
}

/// One streaming delta from the provider — the unit surfaced to the CLI.
#[derive(Clone, Debug, PartialEq)]
pub enum Delta {
    /// Thinking tokens (qwen-style `reasoning_content`).
    Reasoning(String),
    /// Assistant text fragment.
    Text(String),
    /// First fragment of a tool call: carries id + name.
    ToolCallStart {
        index: usize,
        id: String,
        name: String,
    },
    /// Subsequent fragment: only arguments slice (id/name empty on the wire).
    ToolCallArgs {
        index: usize,
        fragment: String,
    },
    /// End-of-turn marker.
    Done(FinishReason),
}

/// Scripted fake provider: each `next_turn` pops the next pre-written delta
/// sequence. This lets us drive the loop through exact scenarios.
pub struct FakeProvider {
    script: Vec<Vec<Delta>>,
    calls: usize,
}

impl FakeProvider {
    fn new(script: Vec<Vec<Delta>>) -> Self {
        Self { script, calls: 0 }
    }

    fn next_turn(&mut self, _messages: &[Message]) -> Vec<Delta> {
        let d = self.script.get(self.calls).cloned().unwrap_or_default();
        self.calls += 1;
        d
    }
}

/// Reassemble an assistant message from a delta stream (what the runtime must do
/// regardless of which provider produced it).
fn assemble(deltas: &[Delta]) -> (String, Vec<ToolCall>, FinishReason) {
    let mut text = String::new();
    let mut tcs: BTreeMap<usize, ToolCall> = BTreeMap::new();
    let mut reason = FinishReason::Stop;
    for d in deltas {
        match d {
            Delta::Text(t) => text.push_str(t),
            Delta::Reasoning(_) => {}
            Delta::ToolCallStart { index, id, name } => {
                tcs.entry(*index)
                    .or_insert_with(|| ToolCall { id: id.clone(), name: name.clone(), arguments: String::new() });
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

// ---------------------------------------------------------------------------
// Agent run: events surfaced to the CLI + the run loop
// ---------------------------------------------------------------------------

/// Every observable thing the runtime emits during a run — the CLI renders these.
#[derive(Clone, Debug)]
pub enum AgentEvent {
    Turn { turn: usize },
    /// Raw delta, forwarded live so the CLI can stream text / a thinking line.
    Stream(Delta),
    ToolStart { name: String, arguments: String },
    ToolResult { name: String, ok: bool, result: String },
    Stop(StopReason),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StopReason {
    /// The model produced a final answer with no tool calls.
    Completed,
    /// Hit the iteration cap before the model finished.
    MaxIterations,
}

#[derive(Clone, Debug)]
pub struct RunConfig {
    pub max_iterations: usize,
    /// Execute multiple tool calls from one response concurrently, or one at a
    /// time (appending results as we go).
    pub parallel_tools: bool,
}

#[derive(Debug)]
pub struct RunResult {
    pub messages: Vec<Message>,
    pub iterations: usize,
    pub stop: StopReason,
    pub events: Vec<AgentEvent>,
}

/// Execute tool calls and append their results to `messages`.
fn execute_tools(
    tools: &BTreeMap<String, Tool>,
    calls: &[ToolCall],
    parallel: bool,
    events: &mut Vec<AgentEvent>,
    messages: &mut Vec<Message>,
) {
    // The semantic difference: in serial mode each result is appended before the
    // next call runs (so later tools could observe earlier results); in parallel
    // mode all calls run first and results are appended together. Both shapes
    // matter for tool loops that share state.
    if parallel {
        let results: Vec<(String, Result<String, String>)> = calls
            .iter()
            .map(|tc| (tc.id.clone(), dispatch(tools, tc)))
            .collect();
        for (id, res) in results {
            push_tool_event_and_message(tools, calls, &id, res, events, messages);
        }
    } else {
        for tc in calls {
            let res = dispatch(tools, tc);
            push_tool_event_and_message(tools, calls, &tc.id, res, events, messages);
        }
    }
}

fn dispatch(tools: &BTreeMap<String, Tool>, tc: &ToolCall) -> Result<String, String> {
    match tools.get(&tc.name) {
        Some(t) => {
            let args: Value =
                serde_json::from_str(&tc.arguments).unwrap_or(Value::Null);
            (t.run)(args)
        }
        None => Err(format!("unknown tool: {}", tc.name)),
    }
}

fn push_tool_event_and_message(
    _tools: &BTreeMap<String, Tool>,
    calls: &[ToolCall],
    id: &str,
    res: Result<String, String>,
    events: &mut Vec<AgentEvent>,
    messages: &mut Vec<Message>,
) {
    // find the call name for event reporting
    let name = calls
        .iter()
        .find(|c| c.id == id)
        .map(|c| c.name.clone())
        .unwrap_or_else(|| "?".to_string());
    let (ok, body) = match res {
        Ok(o) => (true, o),
        Err(e) => (false, e),
    };
    events.push(AgentEvent::ToolStart {
        name: name.clone(),
        arguments: "…".to_string(),
    });
    events.push(AgentEvent::ToolResult {
        name,
        ok,
        result: body.clone(),
    });
    let content = if ok { body } else { format!("Error: {body}") };
    messages.push(Message::tool_result(id.to_string(), content));
}

/// The runtime loop.
pub fn run_agent(
    provider: &mut FakeProvider,
    tools: &BTreeMap<String, Tool>,
    system: &str,
    user: &str,
    cfg: &RunConfig,
) -> RunResult {
    let mut messages = vec![
        Message::new(Role::System, system),
        Message::new(Role::User, user),
    ];
    let mut events: Vec<AgentEvent> = Vec::new();
    let mut stop = StopReason::Completed;
    let mut iterations = 0usize;

    for turn in 1..=cfg.max_iterations {
        iterations = turn;
        events.push(AgentEvent::Turn { turn });

        let deltas = provider.next_turn(&messages);
        for d in &deltas {
            events.push(AgentEvent::Stream(d.clone()));
        }
        let (text, tool_calls, reason) = assemble(&deltas);
        let mut asst = Message::new(Role::Assistant, text);
        asst.tool_calls = tool_calls.clone();
        messages.push(asst);

        match reason {
            FinishReason::Stop => {
                stop = StopReason::Completed;
                break;
            }
            FinishReason::ToolCalls => {
                execute_tools(tools, &tool_calls, cfg.parallel_tools, &mut events, &mut messages);
                // fall through to the next turn (tool results are now in history)
            }
        }
        if turn == cfg.max_iterations {
            stop = StopReason::MaxIterations;
        }
    }

    RunResult { messages, iterations, stop, events }
}

// ---------------------------------------------------------------------------
// Demo battery
// ---------------------------------------------------------------------------

fn scenario(title: &str, cfg: RunConfig, script: Vec<Vec<Delta>>) -> RunResult {
    let tools = default_tools();
    let mut provider = FakeProvider::new(script);
    println!("\n════════════════════════════════════════════════════════");
    println!("SCENARIO: {title}");
    println!("  config: max_iterations={}, parallel_tools={}", cfg.max_iterations, cfg.parallel_tools);
    let res = run_agent(
        &mut provider,
        &tools,
        "You are a helpful coding agent with tools.",
        "How is the weather in Beijing?",
        &cfg,
    );
    println!("  -> stop={:?}, iterations={}", res.stop, res.iterations);
    res
}

fn print_events(res: &RunResult) {
    for e in &res.events {
        match e {
            AgentEvent::Turn { turn } => println!("  --- turn {turn} ---"),
            AgentEvent::Stream(Delta::Reasoning(t)) => println!("  [thinking] {t}"),
            AgentEvent::Stream(Delta::Text(t)) => print!("  {t}"),
            AgentEvent::Stream(Delta::ToolCallStart { index, name, .. }) => {
                println!("  [tool_start] idx={index} name={name}")
            }
            AgentEvent::Stream(Delta::ToolCallArgs { .. }) => {}
            AgentEvent::Stream(Delta::Done(_)) => {}
            AgentEvent::ToolStart { name, .. } => println!("  >> tool: {name}"),
            AgentEvent::ToolResult { name, ok, result } => {
                println!("  << {name}: {} {}", if *ok { "ok" } else { "ERR" }, result)
            }
            AgentEvent::Stop(s) => println!("  [stop] {s:?}"),
        }
    }
    println!();
}

fn print_final(messages: &[Message]) {
    println!("  -- final assistant text --");
    for m in messages {
        if m.role == Role::Assistant {
            let tc = if m.tool_calls.is_empty() { String::new() } else { format!("  (tool_calls: {})", m.tool_calls.len()) };
            let txt: Vec<String> = m.parts.iter().filter_map(|p| match p { Part::Text { text } => Some(text.clone()), }).collect();
            println!("  assistant: {}{}", txt.join(""), tc);
        }
    }
}

/// Reusable delta builders that mirror the live wire format (ticket 05).
fn r(t: &str) -> Delta { Delta::Reasoning(t.to_string()) }
fn t(t: &str) -> Delta { Delta::Text(t.to_string()) }
fn tc_start(index: usize, id: &str, name: &str) -> Delta {
    Delta::ToolCallStart { index, id: id.to_string(), name: name.to_string() }
}
fn tc_args(index: usize, frag: &str) -> Delta {
    Delta::ToolCallArgs { index, fragment: frag.to_string() }
}

fn main() {
    println!("slimcode ticket 04 — agent runtime loop prototype");

    // 1. Single-turn direct answer (no tools).
    let s1 = scenario(
        "single-turn direct answer",
        RunConfig { max_iterations: 5, parallel_tools: false },
        vec![vec![r("This is trivial."), t("Beijing weather is sunny, 25C."), Delta::Done(FinishReason::Stop)]],
    );
    print_events(&s1);
    print_final(&s1.messages);

    // 2. One tool call, then the final answer (serial).
    let s2 = scenario(
        "one tool call then final answer",
        RunConfig { max_iterations: 5, parallel_tools: false },
        vec![
            vec![
                r("Need weather for Beijing."),
                tc_start(0, "call_1", "get_weather"),
                tc_args(0, "{\"city\": \"Beij"),
                tc_args(0, "ing\"}"),
                Delta::Done(FinishReason::ToolCalls),
            ],
            vec![t("Beijing is 25C."), Delta::Done(FinishReason::Stop)],
        ],
    );
    print_events(&s2);
    print_final(&s2.messages);

    // 3. Two tool calls from one response — parallel execution.
    let s3 = scenario(
        "two tool calls, PARALLEL execution",
        RunConfig { max_iterations: 5, parallel_tools: true },
        vec![
            vec![
                r("Need weather for two cities."),
                tc_start(0, "call_1", "get_weather"),
                tc_args(0, "{\"city\": \"Beijing\"}"),
                tc_start(1, "call_2", "get_time"),
                tc_args(1, "{\"city\": \"Shanghai\"}"),
                Delta::Done(FinishReason::ToolCalls),
            ],
            vec![t("Beijing: 25C. Shanghai: 14:05."), Delta::Done(FinishReason::Stop)],
        ],
    );
    print_events(&s3);

    // 4. Tool errors: a fragile tool fails, the model recovers.
    let s4 = scenario(
        "tool error, model recovers",
        RunConfig { max_iterations: 5, parallel_tools: false },
        vec![
            vec![
                r("Try the fragile tool on Tokyo."),
                tc_start(0, "call_1", "fragile_tool"),
                tc_args(0, "{\"city\": \"Tokyo\"}"),
                Delta::Done(FinishReason::ToolCalls),
            ],
            vec![t("I could not get Tokyo's weather — no station there."), Delta::Done(FinishReason::Stop)],
        ],
    );
    print_events(&s4);

    // 5. Runaway: the model keeps calling tools; the iteration cap stops it.
    let mut runaway = Vec::new();
    for i in 0..6 {
        runaway.push(vec![
            tc_start(0, &format!("call_{i}"), "get_weather"),
            tc_args(0, "{\"city\": \"Beijing\"}"),
            Delta::Done(FinishReason::ToolCalls),
        ]);
    }
    let s5 = scenario(
        "runaway tool loop hits max_iterations",
        RunConfig { max_iterations: 3, parallel_tools: false },
        runaway,
    );
    print_events(&s5);

    // 6. Streaming surface: the raw event sequence the CLI would render.
    println!("\n════════════════════════════════════════════════════════");
    println!("SCENARIO: streaming surface (event sequence as the CLI sees it)");
    let s6 = scenario(
        "streaming surface",
        RunConfig { max_iterations: 5, parallel_tools: false },
        vec![
            vec![
                r("Let me think…"),
                t("The answer is "),
                t("42"),
                Delta::Done(FinishReason::Stop),
            ],
        ],
    );
    for e in &s6.events {
        println!("  {:?}", e);
    }

    // 7. Session JSON boundary: a Session (metadata + messages) round-trips.
    println!("\n════════════════════════════════════════════════════════");
    println!("SCENARIO: session JSON boundary (Session wrapper + messages)");
    let session = Session::new("sess-1", s2.messages.clone());
    let json = serde_json::to_string_pretty(&session).expect("serialize");
    println!("{json}");
    let back: Session = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(&session, &back, "session JSON must round-trip losslessly");
    println!("  -> round-trip OK: {} messages preserved in {}", back.messages.len(), back.id);

    println!("\n════════════════════════════════════════════════════════");
    println!("done.");
}
