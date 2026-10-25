//! Engine trace-span tests (`.scratch/engine` ticket 05, spec Testing
//! Decisions item 7): drive a turn through the public
//! [`slimcode_app::engine::Engine`] seam with an injected [`Tracer`] backend and
//! pin the three kernel-opened operand-call spans — `iteration`,
//! `provider.chat`, `tool.{name}` — their start/end (each appears exactly once
//! in the log, opened then closed on the recorder's drop) and their attributes.
//!
//! Also pins the two guarantees the seam owes the code preset:
//! - no tracer (`Tracer: None`) is a silent no-op that never errors, and the run
//!   still completes (the byte-equivalence gate is the `equivalence_baseline`
//!   snapshot; here we assert it merely does not blow up);
//! - a provider that records onto its `provider.chat` recorder *before* the
//!   transport fails still lands those attributes on the span (RAII drop runs on
//!   the error path, ADR-0031 D4).

use std::sync::{Arc, Mutex};

use slimcode_app::context::Context;
use slimcode_app::render::{DisplayItem, Renderer};
use slimcode_app::{
    AgentMessage, CancelToken, Delta, Engine, FinishReason, Message, Provider, ProviderConfig,
    Role, SpanRecorder, TokenUsage, Tool, ToolSpec, Tracer,
};

/// One closed span: its name and the attributes recorded on it.
type ClosedSpan = (String, Vec<(String, String)>);

/// A [`Tracer`] backend that collects every closed span: its name and the
/// attributes recorded on it. `start` hands back a recorder whose `on_end`
/// pushes onto the shared log — so a span lands here exactly once its RAII
/// recorder drops.
#[derive(Clone)]
struct TraceBackend {
    spans: Arc<Mutex<Vec<ClosedSpan>>>,
}

impl TraceBackend {
    fn new() -> Self {
        Self {
            spans: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Every closed span, in end order.
    fn spans(&self) -> Vec<(String, Vec<(String, String)>)> {
        self.spans.lock().unwrap().clone()
    }

    /// The attributes recorded on the (single) span named `name`, or `None` if
    /// no such span closed.
    fn attrs(&self, name: &str) -> Option<Vec<(String, String)>> {
        self.spans()
            .into_iter()
            .find(|(n, _)| n == name)
            .map(|(_, a)| a)
    }

    /// How many closed spans carry `name`.
    fn count(&self, name: &str) -> usize {
        self.spans().iter().filter(|(n, _)| n == name).count()
    }
}

impl Tracer for TraceBackend {
    fn start(&self, name: &str) -> Option<SpanRecorder> {
        let log = self.spans.clone();
        Some(SpanRecorder::start(name, move |n, attrs| {
            log.lock().unwrap().push((n.to_string(), attrs));
        }))
    }
}

/// Scripted provider: pops the next delta batch per call, then reports a fixed
/// usage so the `provider.chat` span picks up its token attributes.
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
    fn chat(
        &mut self,
        _messages: &[Message],
        _tools: &[ToolSpec],
        _config: &ProviderConfig,
        _cancel: &CancelToken,
        on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
    ) -> Result<Option<TokenUsage>, String> {
        let batch = self.script.get(self.calls).cloned().unwrap_or_default();
        self.calls += 1;
        for d in batch {
            on_delta(d)?;
        }
        Ok(Some(TokenUsage {
            prompt_tokens: 10,
            completion_tokens: 3,
            total_tokens: 13,
            ..Default::default()
        }))
    }
}

/// A provider that overrides the traced seam: it records a request-body
/// attribute onto the `provider.chat` recorder, then fails the transport —
/// exercising the drop-on-error flush the trace seam promises (ADR-0031 D4).
struct FailingProvider;

impl Provider for FailingProvider {
    fn chat(
        &mut self,
        _messages: &[Message],
        _tools: &[ToolSpec],
        _config: &ProviderConfig,
        _cancel: &CancelToken,
        _on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
    ) -> Result<Option<TokenUsage>, String> {
        Err("transport down".to_string())
    }

    fn chat_traced(
        &mut self,
        _messages: &[Message],
        _tools: &[ToolSpec],
        _config: &ProviderConfig,
        _cancel: &CancelToken,
        _on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
        recorder: &mut SpanRecorder,
    ) -> Result<Option<TokenUsage>, String> {
        // Record before the call dies, exactly as a real provider serializing a
        // request would; the recorder still drops (and flushes) on the Err path.
        recorder.record("request_body", "{\"messages\":[…]}");
        Err("transport down".to_string())
    }
}

/// Null renderer: the trace tests observe spans, not the display stream.
struct Null;

impl Renderer for Null {
    fn render(&mut self, _item: &DisplayItem) -> Result<(), String> {
        Ok(())
    }
}

fn cfg() -> ProviderConfig {
    ProviderConfig::new("k", "https://example.invalid/v1", "m")
}

fn probe_tool() -> Tool {
    Tool::new("probe", "probe", serde_json::json!({}), |_| {
        Ok("probed".to_string())
    })
}

fn context(prompt: &str) -> Context {
    Context {
        system: Message::text(Role::System, "sys"),
        messages: vec![AgentMessage::text(Role::User, prompt)],
    }
}

fn t(s: &str) -> Delta {
    Delta::Text(s.to_string())
}
fn tool_call(i: usize, id: &str, name: &str) -> Delta {
    Delta::ToolCallStart {
        index: i,
        id: id.into(),
        name: name.into(),
    }
}
fn tool_args(i: usize, f: &str) -> Delta {
    Delta::ToolCallArgs {
        index: i,
        fragment: f.into(),
    }
}
fn stop() -> Delta {
    Delta::Done(FinishReason::Stop)
}
fn tools_done() -> Delta {
    Delta::Done(FinishReason::ToolCalls)
}

/// A tool-calling turn then a final answer drives every span kind the kernel
/// opens: two `iteration` passes, two `provider.chat` calls, one `tool.probe`.
#[test]
fn engine_opens_the_three_operand_spans_with_attributes() {
    let backend = TraceBackend::new();
    let mut provider: Box<dyn Provider + Send> = Box::new(FakeProvider::new(vec![
        vec![
            t("checking "),
            tool_call(0, "call_1", "probe"),
            tool_args(0, "{}"),
            tools_done(),
        ],
        vec![t("done"), stop()],
    ]));

    let config = cfg();
    let tools = [probe_tool()];
    let mut engine =
        Engine::new(&mut provider, &tools, &config).with_tracer(Arc::new(backend.clone()));
    let result = engine.run_turn(
        context("weather?"),
        &CancelToken::new(),
        &mut Null,
        &mut |_, _| Ok(()),
    );
    assert!(result.is_ok(), "engine turn failed: {result:?}");

    // Each span kind closed with its expected attributes.
    let iterations = backend.count("iteration");
    let chats = backend.count("provider.chat");
    let probes = backend.count("tool.probe");
    assert_eq!(iterations, 2, "one iteration span per loop pass");
    assert_eq!(chats, 2, "one provider.chat span per LLM call");
    assert_eq!(probes, 1, "one tool.probe span for the dispatched call");

    let iter_attrs = backend.attrs("iteration").expect("iteration span");
    assert!(
        iter_attrs.iter().any(|(k, _)| k == "turn"),
        "iteration span carries the turn attribute: {iter_attrs:?}"
    );

    let chat_attrs = backend.attrs("provider.chat").expect("provider.chat span");
    assert!(
        chat_attrs.iter().any(|(k, v)| k == "model" && v == "m"),
        "provider.chat span carries the model: {chat_attrs:?}"
    );
    assert!(
        chat_attrs
            .iter()
            .any(|(k, v)| k == "prompt_tokens" && v == "10"),
        "provider.chat span carries the reported usage: {chat_attrs:?}"
    );

    let tool_attrs = backend.attrs("tool.probe").expect("tool.probe span");
    assert!(
        tool_attrs.iter().any(|(k, v)| k == "ok" && v == "true"),
        "tool.probe span records a successful dispatch: {tool_attrs:?}"
    );
}

/// With no tracer wired in, every kernel span is a silent no-op: the turn runs
/// to completion and no panic or error surfaces from the tracing path.
#[test]
fn tracer_none_is_a_quiet_no_op() {
    let mut provider: Box<dyn Provider + Send> =
        Box::new(FakeProvider::new(vec![vec![t("hello"), stop()]]));

    // No `.with_tracer(..)`: the default Engine carries `Tracer: None`.
    let config = cfg();
    let mut engine = Engine::new(&mut provider, &[], &config);
    let result = engine.run_turn(
        context("hi"),
        &CancelToken::new(),
        &mut Null,
        &mut |_, _| Ok(()),
    );
    assert!(
        result.is_ok(),
        "untracked turn must still complete: {result:?}"
    );
}

/// A failed transport still lands the attributes a provider recorded on its
/// `provider.chat` recorder before the call died (RAII drop flushes on the error
/// path, spec item 7 / ADR-0031 D4).
#[test]
fn provider_attributes_survive_a_failed_transport() {
    let backend = TraceBackend::new();
    let mut provider: Box<dyn Provider + Send> = Box::new(FailingProvider);

    let config = cfg();
    let mut engine =
        Engine::new(&mut provider, &[], &config).with_tracer(Arc::new(backend.clone()));
    let result = engine.run_turn(
        context("go"),
        &CancelToken::new(),
        &mut Null,
        &mut |_, _| Ok(()),
    );
    assert!(
        result.is_err(),
        "the failing provider surfaces a turn error"
    );

    let chat_attrs = backend
        .attrs("provider.chat")
        .expect("provider.chat span closed even on failure");
    assert!(
        chat_attrs.iter().any(|(k, _)| k == "request_body"),
        "the recorded request-body attribute still landed on the span: {chat_attrs:?}"
    );
}
