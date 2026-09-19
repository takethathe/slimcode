//! Shared turn runner (ADR-0004, spec §Implementation Decisions): drives the
//! existing agent loop through `slimcode-core`'s live event sink, mapping
//! every event through [`map_event`] and streaming it to a [`Renderer`] as the
//! loop runs. This is the single turn loop both frontends share, so their
//! behavior cannot drift.
//!
//! Errors stay `String`, matching the existing agent API. Token usage flows
//! through the message events (ticket 02): the runner forwards each request's
//! usage with the assistant `Message` event, and the sink renders one
//! [`DisplayItem::Usage`] per assistant message that carried usage (ticket
//! 03). Tool results never carry usage, so nothing is rendered for them.
//!
//! Per-message events (ADR-0009 D5): the loop announces every message that
//! enters history (assistant replies and each tool result). The runner
//! forwards those to a caller-provided sink (`on_message`) — never mapped to a
//! display item, never touching disk — so a session layer can persist each
//! message at the moment it exists.

use slimcode_core::agent::{
    AgentEvent, AgentMessage, AgentRunner, CancelToken, Provider, ProviderConfig, RunConfig,
    RunHooks, StopReason, TokenUsage, Tool,
};

use crate::context::Context;
use crate::render::{DisplayItem, Renderer, map_event};

/// The runner's per-message sink: called with each message that enters history
/// and the request's usage (`Some` on assistant replies, `None` on tool
/// results). Errors abort the loop like a renderer error does.
pub type MessageSink<'a> =
    &'a mut dyn FnMut(&AgentMessage, Option<&TokenUsage>) -> Result<(), String>;

/// Drive one agent turn over a [`Context`] (its system message plus the history
/// with this turn's prompt), streaming every event to `renderer` live as the
/// loop runs, forwarding each message that enters history to `on_message`
/// (with the request's usage, `None` on tool results), and returning the
/// updated message history plus the stop reason.
///
/// The context's system message is assembled per request (ADR-0012 D3) and
/// never enters the history; the runtime prepends it to `filter_map(to_llm)`
/// before every provider request.
///
/// `cancel` is threaded into the agent loop untouched: while it stays clear
/// the run behaves exactly as before; the moment it is set the run stops at
/// the next runner boundary with [`StopReason::Cancelled`] (the CLI one-shot
/// passes a token it never sets).
///
/// A provider error propagates as `Err(String)`; no partial history is
/// fabricated. `on_message` errors abort the loop the same way a renderer
/// error does.
#[allow(clippy::too_many_arguments)] // the turn's full run context crosses this seam
pub fn run_turn<P: Provider>(
    provider: &mut P,
    tools: &[Tool],
    context: Context,
    cfg: &RunConfig,
    provider_config: &ProviderConfig,
    cancel: &CancelToken,
    renderer: &mut dyn Renderer,
    on_message: MessageSink<'_>,
) -> Result<(Vec<AgentMessage>, StopReason), String> {
    run_turn_with_hooks(
        provider,
        tools,
        context,
        cfg,
        provider_config,
        cancel,
        renderer,
        on_message,
        RunHooks::default(),
    )
}

/// [`run_turn`] with the ADR-0015 hook seam wired in: the caller supplies the
/// hooks (e.g. a skill-aware `read` interception), the runner owns them for
/// the run. [`run_turn`] is a thin wrapper passing no hooks, so existing
/// callers and tests keep working unchanged.
#[allow(clippy::too_many_arguments)] // the turn's full run context crosses this seam
pub fn run_turn_with_hooks<P: Provider>(
    provider: &mut P,
    tools: &[Tool],
    context: Context,
    cfg: &RunConfig,
    provider_config: &ProviderConfig,
    cancel: &CancelToken,
    renderer: &mut dyn Renderer,
    on_message: MessageSink<'_>,
    hooks: RunHooks<'_>,
) -> Result<(Vec<AgentMessage>, StopReason), String> {
    let Context { system, messages } = context;
    let mut sink = |e: AgentEvent| {
        match &e {
            AgentEvent::Message { message, usage } => {
                on_message(message, usage.as_ref())?;
                // One Usage display item per assistant message that carried
                // usage; tool results carry none, so nothing renders for them
                // (ticket 03).
                if let Some(u) = usage {
                    renderer.render(&DisplayItem::Usage(*u))?;
                }
            }
            _ => {
                if let Some(item) = map_event(&e) {
                    renderer.render(&item)?;
                }
            }
        }
        Ok(())
    };
    let mut runner = AgentRunner::new(tools, cfg.clone(), provider_config, cancel, &mut sink);
    runner.hooks = hooks;
    runner.run(provider, &system, messages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::DisplayItem;
    use serde_json::Value;
    use slimcode_core::agent::{Delta, FinishReason, StopReason, ToolDecision, ToolSpec};
    use slimcode_core::session::{AgentMessage, Message, Role};

    /// A fixed provider config for the turn-runner tests: these tests exercise
    /// event mapping and cancellation, not config resolution (that lives in
    /// `crate::config`).
    fn test_config() -> ProviderConfig {
        ProviderConfig::new("test-key", "https://example.invalid/v1", "test-model")
    }

    // --- helpers (the agent crate's scripted FakeProvider pattern) ----------

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
    fn text(t: &str) -> Delta {
        Delta::Text(t.to_string())
    }
    fn reasoning(t: &str) -> Delta {
        Delta::Reasoning(t.to_string())
    }
    fn system() -> Message {
        Message::text(Role::System, "be helpful")
    }
    fn user(text: &str) -> AgentMessage {
        AgentMessage::text(Role::User, text)
    }
    fn context(messages: Vec<AgentMessage>) -> Context {
        Context {
            system: system(),
            messages,
        }
    }

    /// Scripted provider: pops the next delta sequence per call (agent prior
    /// art). By default `chat` returns `Ok(None)` (no usage); `with_usage`
    /// makes every call carry one, the seam that lets tests assert the
    /// usage-forwarding sink behavior (ticket 03).
    struct FakeProvider {
        script: Vec<Vec<Delta>>,
        calls: usize,
        usage: Option<TokenUsage>,
    }

    impl FakeProvider {
        fn new(script: Vec<Vec<Delta>>) -> Self {
            Self {
                script,
                calls: 0,
                usage: None,
            }
        }

        fn with_usage(mut self, usage: TokenUsage) -> Self {
            self.usage = Some(usage);
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
            let d = self.script.get(self.calls).cloned().unwrap_or_default();
            self.calls += 1;
            // A scripted provider mirrors the live one (ADR-0019): deltas go
            // through the sink, in order, as the request produces them.
            for delta in d {
                on_delta(delta)?;
            }
            Ok(self.usage)
        }
    }

    /// Recording renderer: collects every display item it receives.
    struct RecordingRenderer {
        items: Vec<DisplayItem>,
        fail_on: Option<String>,
    }

    impl RecordingRenderer {
        fn new() -> Self {
            Self {
                items: Vec::new(),
                fail_on: None,
            }
        }
    }

    impl Renderer for RecordingRenderer {
        fn render(&mut self, item: &DisplayItem) -> Result<(), String> {
            if let Some(needle) = &self.fail_on
                && matches!(item, DisplayItem::Text(t) if t.contains(needle))
            {
                return Err("renderer exploded".to_string());
            }
            self.items.push(item.clone());
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

    /// A sink that accepts every message (no persistence in these tests).
    fn noop_sink() -> impl FnMut(&AgentMessage, Option<&TokenUsage>) -> Result<(), String> {
        |_, _| Ok(())
    }

    /// Drive the agent loop directly (the runner the shared `run_turn` uses),
    /// returning the updated history, the stop reason and the collected events.
    fn drive_agent_runner(
        provider: &mut impl Provider,
        tools: &[Tool],
        cfg: RunConfig,
        cancel: &CancelToken,
        system: &Message,
        messages: Vec<AgentMessage>,
    ) -> Result<(Vec<AgentMessage>, StopReason, Vec<AgentEvent>), String> {
        let mut events: Vec<AgentEvent> = Vec::new();
        // The sink's borrow of `events` ends when the collect helper returns,
        // so the Vec can be moved out afterwards (the runner's drop glue
        // would keep the borrow alive otherwise).
        let (messages, stop) = drive_agent_runner_collect(
            provider,
            tools,
            cfg,
            cancel,
            system,
            messages,
            &mut events,
        )?;
        Ok((messages, stop, events))
    }

    /// The same loop with a caller-owned event collector.
    fn drive_agent_runner_collect(
        provider: &mut impl Provider,
        tools: &[Tool],
        cfg: RunConfig,
        cancel: &CancelToken,
        system: &Message,
        messages: Vec<AgentMessage>,
        events: &mut Vec<AgentEvent>,
    ) -> Result<(Vec<AgentMessage>, StopReason), String> {
        let config = ProviderConfig::new("test-key", "https://example.invalid/v1", "test-model");
        let mut sink = |e: AgentEvent| {
            events.push(e);
            Ok(())
        };
        let mut runner = AgentRunner::new(tools, cfg, &config, cancel, &mut sink);
        runner.run(provider, system, messages)
    }

    // --- behavior -----------------------------------------------------------

    #[test]
    fn streams_ordered_display_items_text_reasoning_tool_stop() {
        let script = vec![
            vec![
                reasoning("Let me check"),
                text("Checking "),
                tc_start(0, "call_1", "get_weather"),
                tc_args(0, "{\"city\": \"Beijing\"}"),
                done_tools(),
            ],
            vec![text("Beijing is 25C."), done_stop()],
        ];
        let mut provider = FakeProvider::new(script);
        let mut renderer = RecordingRenderer::new();
        let messages = vec![user("weather?")];
        let cancel = CancelToken::new();
        let (updated, stop) = run_turn(
            &mut provider,
            &[weather_tool()],
            context(messages),
            &RunConfig::default(),
            &test_config(),
            &cancel,
            &mut renderer,
            &mut noop_sink(),
        )
        .unwrap();
        assert_eq!(stop, StopReason::Completed);

        // Raw tool deltas are suppressed; streamed text and structural lines
        // arrive in order, live.
        let kinds: Vec<&str> = renderer
            .items
            .iter()
            .map(|i| match i {
                DisplayItem::Turn { .. } => "turn",
                DisplayItem::Reasoning(_) => "reasoning",
                DisplayItem::Text(_) => "text",
                DisplayItem::ToolStart { .. } => "tool-start",
                DisplayItem::ToolResult { .. } => "tool-result",
                DisplayItem::Stop(_) => "stop",
                DisplayItem::Usage(_) => "usage",
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                "turn",
                "reasoning",
                "text",
                "tool-start",
                "tool-result",
                "turn",
                "text",
                "stop"
            ]
        );
        // Text fragments are faithful to the stream.
        let texts: Vec<&str> = renderer
            .items
            .iter()
            .filter_map(|i| match i {
                DisplayItem::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["Checking ", "Beijing is 25C."]);
        // The tool result carries its ok flag.
        assert!(renderer.items.iter().any(
            |i| matches!(i, DisplayItem::ToolResult { name, ok: true, .. } if name == "get_weather")
        ));
        assert!(
            renderer
                .items
                .iter()
                .any(|i| matches!(i, DisplayItem::Stop(StopReason::Completed)))
        );
        // No usage item: the default scripted provider returns no usage, so
        // nothing renders (ticket 03 — usage rides the message events only
        // when the provider reports it).
        assert!(
            renderer
                .items
                .iter()
                .all(|i| !matches!(i, DisplayItem::Usage(_)))
        );

        // Returned history: user + assistant(tool_calls) + tool + assistant
        // (the system prompt is not part of it, ADR-0012 D3).
        assert_eq!(updated.len(), 4);
        assert!(updated.iter().any(|m| m.role() == &Role::Tool));
        assert_eq!(updated.last().unwrap().role(), &Role::Assistant);
        assert!(updated.last().unwrap().text_content().contains("25C"));
    }

    #[test]
    fn usage_flows_through_the_message_events() {
        // Ticket 03: a provider that returns per-request usage — the sink
        // sees it with each assistant Message event and renders one Usage
        // display item per assistant message (tool results carry none, so
        // nothing renders for them).
        let usage = TokenUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
            prompt_tokens_details: None,
        };
        let script = vec![
            vec![
                text("checking "),
                tc_start(0, "call_1", "get_weather"),
                tc_args(0, "{}"),
                done_tools(),
            ],
            vec![text("25C"), done_stop()],
        ];
        let mut provider = FakeProvider::new(script).with_usage(usage);
        let mut renderer = RecordingRenderer::new();
        let cancel = CancelToken::new();
        let mut usages: Vec<TokenUsage> = Vec::new();
        let (_, stop) = run_turn(
            &mut provider,
            &[weather_tool()],
            context(vec![user("weather?")]),
            &RunConfig::default(),
            &test_config(),
            &cancel,
            &mut renderer,
            &mut |_, u| {
                if let Some(u) = u {
                    usages.push(*u);
                }
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(stop, StopReason::Completed);
        // The sink saw one usage per assistant message (the tool-call reply
        // and the final answer); the tool result carried none.
        assert_eq!(usages, vec![usage, usage]);
        // One Usage display item per assistant message.
        let rendered_usages = renderer
            .items
            .iter()
            .filter_map(|i| match i {
                DisplayItem::Usage(u) => Some(*u),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(rendered_usages, vec![usage, usage]);
    }

    #[test]
    fn usage_is_discarded_on_a_cancelled_request() {
        // A cancel that lands mid-request discards the request's usage: no
        // assistant message enters history, so the sink sees nothing and no
        // Usage item renders (ticket 02).
        let usage = TokenUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
            prompt_tokens_details: None,
        };
        let cancel = CancelToken::new();
        let mut provider =
            FakeProvider::new(vec![vec![text("partial"), done_stop()]]).with_usage(usage);
        let mut wrapped = CancelFirstProvider {
            inner: &mut provider,
            calls: 0,
            cancel: cancel.clone(),
        };
        let mut renderer = RecordingRenderer::new();
        let mut seen: usize = 0;
        let (_, stop) = run_turn(
            &mut wrapped,
            &[],
            context(vec![user("hi")]),
            &RunConfig::default(),
            &test_config(),
            &cancel,
            &mut renderer,
            &mut |_, u| {
                if u.is_some() {
                    seen += 1;
                }
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(stop, StopReason::Cancelled);
        assert_eq!(seen, 0);
        assert!(
            !renderer
                .items
                .iter()
                .any(|i| matches!(i, DisplayItem::Usage(_)))
        );
    }

    #[test]
    fn returned_history_matches_agent_semantics() {
        // The returned history must equal what the agent loop produces for
        // the same script: an AgentRunner-driven run.
        let script = vec![
            vec![
                tc_start(0, "call_1", "get_weather"),
                tc_args(0, "{\"city\": \"Beijing\"}"),
                done_tools(),
            ],
            vec![text("Beijing is 25C."), done_stop()],
        ];
        let mut provider = FakeProvider::new(script.clone());
        let mut renderer = RecordingRenderer::new();
        let messages = vec![user("weather?")];
        let cancel = CancelToken::new();
        let (updated, stop) = run_turn(
            &mut provider,
            &[weather_tool()],
            context(messages.clone()),
            &RunConfig::default(),
            &test_config(),
            &cancel,
            &mut renderer,
            &mut noop_sink(),
        )
        .unwrap();

        let mut provider2 = FakeProvider::new(script);
        let tools = [weather_tool()];
        let (updated2, stop2, _) = drive_agent_runner(
            &mut provider2,
            &tools,
            RunConfig::default(),
            &CancelToken::new(),
            &system(),
            messages,
        )
        .unwrap();
        assert_eq!(updated, updated2);
        assert_eq!(stop, stop2);
    }

    #[test]
    fn raw_tool_deltas_suppressed() {
        let script = vec![vec![
            tc_start(0, "call_1", "get_weather"),
            tc_args(0, "{}"),
            done_tools(),
        ]];
        let mut provider = FakeProvider::new(script);
        let mut renderer = RecordingRenderer::new();
        let messages = vec![user("go")];
        let cancel = CancelToken::new();
        run_turn(
            &mut provider,
            &[weather_tool()],
            context(messages),
            &RunConfig::default(),
            &test_config(),
            &cancel,
            &mut renderer,
            &mut noop_sink(),
        )
        .unwrap();
        assert!(
            renderer
                .items
                .iter()
                .all(|i| !matches!(i, DisplayItem::Text(t) if t.contains("call"))),
            "no raw tool deltas leaked: {:?}",
            renderer.items
        );
    }

    #[test]
    fn before_tool_hook_can_skip_a_call_and_supply_its_result() {
        // A `before_tool` hook may return `Skip`, replacing what the model
        // sees (and what enters history) without running the tool. The hook
        // signature is the run_turn_with_hooks seam; run_turn passes none.
        let script = vec![vec![
            tc_start(0, "call_1", "get_weather"),
            tc_args(0, "{}"),
            done_tools(),
        ]];
        let mut provider = FakeProvider::new(script);
        let mut renderer = RecordingRenderer::new();
        let messages = vec![user("weather?")];
        let cancel = CancelToken::new();
        let mut hook = |msg: &mut AgentMessage, index: usize| {
            let call_name = msg.tool_calls()[index].name.clone();
            assert_eq!(call_name, "get_weather");
            Ok(ToolDecision::Skip(Ok("SKIPPED_WEATHER".to_string())))
        };
        let hooks = RunHooks {
            before_tool: Some(Box::new(&mut hook)),
            ..Default::default()
        };
        let (updated, stop) = run_turn_with_hooks(
            &mut provider,
            &[weather_tool()],
            context(messages),
            &RunConfig::default(),
            &test_config(),
            &cancel,
            &mut renderer,
            &mut noop_sink(),
            hooks,
        )
        .unwrap();
        assert_eq!(stop, StopReason::Completed);
        // The skipped call's result entered history in place of the tool's
        // real output; the tool never ran.
        assert!(
            updated
                .iter()
                .any(|m| m.text_content() == "SKIPPED_WEATHER")
        );
    }

    #[test]
    fn parallel_batch_streams_events_in_completion_order_and_sinks_history_in_model_order() {
        // The shared seam both frontends drive: the renderer (UI) must see a
        // parallel batch's tool events in completion order, while the
        // on_message sink (session persistence) and the returned history must
        // see the results in model order.
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::{Duration, Instant};

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
            "waits for the fast call",
            serde_json::json!({}),
            move |_| {
                let deadline = Instant::now() + Duration::from_secs(2);
                while !wait.load(Ordering::SeqCst) {
                    if Instant::now() > deadline {
                        return Err("slow_tool starved".to_string());
                    }
                    std::thread::sleep(Duration::from_millis(2));
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
            vec![text("done"), done_stop()],
        ];
        let mut provider = FakeProvider::new(script);
        let mut renderer = RecordingRenderer::new();
        let cancel = CancelToken::new();
        let mut seen: Vec<String> = Vec::new();
        let (updated, stop) = run_turn(
            &mut provider,
            &[slow, fast],
            context(vec![user("go")]),
            &RunConfig::default(),
            &test_config(),
            &cancel,
            &mut renderer,
            &mut |m, _| {
                if m.role() == &Role::Tool {
                    seen.push(m.tool_call_id().unwrap_or_default().to_string());
                }
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(stop, StopReason::Completed);
        // Sink (and history): model order — slow_tool (call_0) first.
        assert_eq!(seen, vec!["call_0", "call_1"]);
        let history_ids: Vec<&str> = updated
            .iter()
            .filter(|m| m.role() == &Role::Tool)
            .map(|m| m.tool_call_id().expect("tool result id"))
            .collect();
        assert_eq!(history_ids, vec!["call_0", "call_1"]);
        // Renderer: completion order — fast_tool (call_1) first.
        let result_ids: Vec<&str> = renderer
            .items
            .iter()
            .filter_map(|i| match i {
                DisplayItem::ToolResult { tool_call_id, .. } => Some(tool_call_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(result_ids, vec!["call_1", "call_0"]);
    }

    #[test]
    fn provider_receives_the_config_crossing_the_chat_seam() {
        // ADR-0016: the resolved config (model, base URL, cache flag) travels
        // across the `chat` seam, not through the provider instance — a
        // stateless provider must see exactly what the caller resolved.
        struct CapturingProvider {
            seen: Option<(String, String, bool)>,
        }
        impl Provider for CapturingProvider {
            fn chat(
                &mut self,
                _m: &[Message],
                _t: &[ToolSpec],
                config: &ProviderConfig,
                _c: &CancelToken,
                on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
            ) -> Result<Option<TokenUsage>, String> {
                self.seen = Some((config.model.clone(), config.base_url.clone(), config.cache));
                on_delta(text("ok"))?;
                on_delta(done_stop())?;
                Ok(None)
            }
        }
        let mut provider = CapturingProvider { seen: None };
        let mut renderer = RecordingRenderer::new();
        let cancel = CancelToken::new();
        let config = ProviderConfig::new("k", "https://chosen.example.com/v1", "chosen-model")
            .with_cache(false);
        run_turn(
            &mut provider,
            &[],
            context(vec![user("hi")]),
            &RunConfig::default(),
            &config,
            &cancel,
            &mut renderer,
            &mut noop_sink(),
        )
        .unwrap();
        assert_eq!(
            provider.seen,
            Some((
                "chosen-model".to_string(),
                "https://chosen.example.com/v1".to_string(),
                false,
            ))
        );
    }

    #[test]
    fn provider_error_propagates() {
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
        let mut provider = ErrProvider;
        let mut renderer = RecordingRenderer::new();
        let cancel = CancelToken::new();
        let err = run_turn(
            &mut provider,
            &[],
            context(vec![user("hi")]),
            &RunConfig::default(),
            &test_config(),
            &cancel,
            &mut renderer,
            &mut noop_sink(),
        )
        .unwrap_err();
        assert!(err.contains("provider exploded"), "err: {err}");
    }

    #[test]
    fn renderer_error_propagates() {
        let script = vec![vec![text("boom town"), done_stop()]];
        let mut provider = FakeProvider::new(script);
        let mut renderer = RecordingRenderer::new();
        renderer.fail_on = Some("boom".to_string());
        let cancel = CancelToken::new();
        let err = run_turn(
            &mut provider,
            &[],
            context(vec![user("hi")]),
            &RunConfig::default(),
            &test_config(),
            &cancel,
            &mut renderer,
            &mut noop_sink(),
        )
        .unwrap_err();
        assert!(err.contains("renderer exploded"), "err: {err}");
    }

    #[test]
    fn cancel_flip_mid_run_stops_the_stream_with_cancelled() {
        // A provider whose first chat cancels the token (Esc while the request
        // is in flight) before returning its batch: the shared runner streams
        // the deltas that arrived, then stops with Stop(Cancelled); the
        // cancel token itself is passed through untouched otherwise.
        let cancel = CancelToken::new();
        let mut provider = FakeProvider::new(vec![vec![text("partial"), done_stop()]]);
        let mut wrapped = CancelFirstProvider {
            inner: &mut provider,
            calls: 0,
            cancel: cancel.clone(),
        };
        let mut renderer = RecordingRenderer::new();
        let (updated, stop) = run_turn(
            &mut wrapped,
            &[],
            context(vec![user("hi")]),
            &RunConfig::default(),
            &test_config(),
            &cancel,
            &mut renderer,
            &mut noop_sink(),
        )
        .unwrap();
        assert_eq!(stop, StopReason::Cancelled);
        // The stream ends with Stop(Cancelled); the partial text of the
        // aborted request was still streamed live.
        assert!(
            renderer
                .items
                .iter()
                .any(|i| matches!(i, DisplayItem::Stop(StopReason::Cancelled)))
        );
        assert!(
            renderer
                .items
                .iter()
                .any(|i| matches!(i, DisplayItem::Text(t) if t == "partial"))
        );
        let stop_at = renderer
            .items
            .iter()
            .position(|i| matches!(i, DisplayItem::Stop(_)))
            .unwrap();
        assert_eq!(
            &renderer.items[stop_at..],
            [DisplayItem::Stop(StopReason::Cancelled)]
        );
        // History rule: no assistant message from the cancelled request enters
        // the returned message list.
        assert!(
            updated
                .iter()
                .all(|m| !m.text_content().contains("partial"))
        );
        assert_eq!(updated.len(), 1);
    }

    #[test]
    fn on_message_sink_sees_every_history_entry_in_order() {
        // The runner forwards each message that enters history (the assembled
        // assistant reply and every tool result) to the sink, in history
        // order, alongside the live stream.
        let script = vec![
            vec![
                text("checking "),
                tc_start(0, "call_1", "get_weather"),
                tc_args(0, "{}"),
                done_tools(),
            ],
            vec![text("25C"), done_stop()],
        ];
        let mut provider = FakeProvider::new(script);
        let mut renderer = RecordingRenderer::new();
        let messages = vec![user("weather?")];
        let cancel = CancelToken::new();
        let mut seen: Vec<AgentMessage> = Vec::new();
        let (updated, _) = run_turn(
            &mut provider,
            &[weather_tool()],
            context(messages),
            &RunConfig::default(),
            &test_config(),
            &cancel,
            &mut renderer,
            &mut |m, _| {
                seen.push(m.clone());
                Ok(())
            },
        )
        .unwrap();
        // The sink saw assistant(tool_calls), tool, then the final assistant —
        // exactly the messages that entered history after the turn's input.
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[0].role(), &Role::Assistant);
        assert_eq!(seen[0].tool_calls().len(), 1);
        assert_eq!(seen[0].tool_calls()[0].id, "call_1");
        assert_eq!(seen[1].role(), &Role::Tool);
        assert_eq!(seen[1].tool_call_id(), Some("call_1"));
        assert_eq!(seen[2].role(), &Role::Assistant);
        assert_eq!(seen, updated[1..]);
    }

    #[test]
    fn on_message_sink_error_aborts_the_run() {
        let script = vec![vec![text("hi"), done_stop()]];
        let mut provider = FakeProvider::new(script);
        let mut renderer = RecordingRenderer::new();
        let cancel = CancelToken::new();
        let err = run_turn(
            &mut provider,
            &[],
            context(vec![user("hi")]),
            &RunConfig::default(),
            &test_config(),
            &cancel,
            &mut renderer,
            &mut |_, _| Err("sink exploded".to_string()),
        )
        .unwrap_err();
        assert!(err.contains("sink exploded"), "err: {err}");
    }

    /// Cancels the shared token on its first chat call, then delegates.
    struct CancelFirstProvider<'a> {
        inner: &'a mut FakeProvider,
        calls: usize,
        cancel: CancelToken,
    }

    impl Provider for CancelFirstProvider<'_> {
        fn chat(
            &mut self,
            m: &[Message],
            t: &[ToolSpec],
            config: &ProviderConfig,
            _c: &CancelToken,
            on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
        ) -> Result<Option<TokenUsage>, String> {
            self.calls += 1;
            if self.calls == 1 {
                self.cancel.cancel();
            }
            self.inner.chat(m, t, config, &self.cancel, on_delta)
        }
    }
}
