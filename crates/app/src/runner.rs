//! Shared turn runner (ADR-0004, spec §Implementation Decisions): drives the
//! existing agent loop through `slimcode-agent`'s live event sink, mapping
//! every event through [`map_event`] and streaming it to a [`Renderer`] as the
//! loop runs. This is the single turn loop both frontends share, so their
//! behavior cannot drift.
//!
//! Errors stay `String`, matching the existing agent API. Token usage is NOT
//! rendered here — it stays a frontend concern: after the run the frontend
//! reads its concrete provider's total usage and feeds a
//! [`DisplayItem::Usage`] to its renderer itself (spec §Implementation
//! Decisions).
//!
//! Per-message events (ADR-0009 D5): the loop announces every message that
//! enters history (assistant replies and each tool result). The runner
//! forwards those to a caller-provided sink (`on_message`) — never mapped to a
//! display item, never touching disk — so a session layer can persist each
//! message at the moment it exists.

use slimcode_core::agent::{
    AgentEvent, CancelToken, Message, Provider, RunConfig, StopReason, Tool,
};

use crate::render::{Renderer, map_event};

/// Drive one agent turn over `messages`, streaming every event to `renderer`
/// live as the loop runs, forwarding each message that enters history to
/// `on_message`, and returning the updated message history plus the stop
/// reason.
///
/// `cancel` is threaded into the agent loop untouched: while it stays clear
/// the run behaves exactly as before; the moment it is set the run stops at
/// the next runner boundary with [`StopReason::Cancelled`] (the CLI one-shot
/// passes a token it never sets).
///
/// A provider error propagates as `Err(String)`; no partial history is
/// fabricated. `on_message` errors abort the loop the same way a renderer
/// error does.
pub fn run_turn<P: Provider>(
    provider: &mut P,
    tools: &[Tool],
    messages: Vec<Message>,
    cfg: &RunConfig,
    cancel: &CancelToken,
    renderer: &mut dyn Renderer,
    on_message: &mut dyn FnMut(&Message) -> Result<(), String>,
) -> Result<(Vec<Message>, StopReason), String> {
    slimcode_core::agent::run_agent_from_messages_sink(
        provider,
        tools,
        messages,
        cfg,
        cancel,
        &mut |e| {
            match &e {
                AgentEvent::Message(m) => on_message(m)?,
                _ => {
                    if let Some(item) = map_event(&e) {
                        renderer.render(&item)?;
                    }
                }
            }
            Ok(())
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::DisplayItem;
    use serde_json::Value;
    use slimcode_core::agent::{Delta, FinishReason, StopReason};
    use slimcode_core::session::{Message, Role};

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

    /// Scripted provider: pops the next delta sequence per call (agent prior
    /// art).
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
            _tools: &[Tool],
            _cancel: &CancelToken,
        ) -> Result<Vec<Delta>, String> {
            let d = self.script.get(self.calls).cloned().unwrap_or_default();
            self.calls += 1;
            Ok(d)
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
    fn noop_sink() -> impl FnMut(&Message) -> Result<(), String> {
        |_| Ok(())
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
        let messages = vec![
            Message::text(Role::System, "be helpful"),
            Message::text(Role::User, "weather?"),
        ];
        let cancel = CancelToken::new();
        let (updated, stop) = run_turn(
            &mut provider,
            &[weather_tool()],
            messages,
            &RunConfig::default(),
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
        // No usage item: token usage is a frontend concern.
        assert!(
            renderer
                .items
                .iter()
                .all(|i| !matches!(i, DisplayItem::Usage(_)))
        );

        // Returned history: system + user + assistant(tool_calls) + tool + assistant.
        assert_eq!(updated.len(), 5);
        assert!(updated.iter().any(|m| m.role == Role::Tool));
        assert_eq!(updated.last().unwrap().role, Role::Assistant);
        assert!(updated.last().unwrap().text_content().contains("25C"));
    }

    #[test]
    fn returned_history_matches_agent_semantics() {
        // The returned history must equal what the existing agent loop would
        // produce (RunResult::messages) for the same script.
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
        let messages = vec![
            Message::text(Role::System, "be helpful"),
            Message::text(Role::User, "weather?"),
        ];
        let cancel = CancelToken::new();
        let (updated, stop) = run_turn(
            &mut provider,
            &[weather_tool()],
            messages.clone(),
            &RunConfig::default(),
            &cancel,
            &mut renderer,
            &mut noop_sink(),
        )
        .unwrap();

        let mut provider2 = FakeProvider::new(script);
        let result = slimcode_core::agent::run_agent_from_messages(
            &mut provider2,
            &[weather_tool()],
            messages,
            &RunConfig::default(),
            &CancelToken::new(),
        )
        .unwrap();
        assert_eq!(updated, result.messages);
        assert_eq!(stop, result.stop);
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
        let messages = vec![Message::text(Role::User, "go")];
        let cancel = CancelToken::new();
        run_turn(
            &mut provider,
            &[weather_tool()],
            messages,
            &RunConfig::default(),
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
            vec![Message::text(Role::User, "go")],
            &RunConfig::default(),
            &cancel,
            &mut renderer,
            &mut |m| {
                if m.role == Role::Tool {
                    seen.push(m.tool_call_id.clone().unwrap_or_default());
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
            .filter(|m| m.role == Role::Tool)
            .map(|m| m.tool_call_id.as_deref().expect("tool result id"))
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
    fn provider_error_propagates() {
        struct ErrProvider;
        impl Provider for ErrProvider {
            fn chat(
                &mut self,
                _m: &[Message],
                _t: &[Tool],
                _c: &CancelToken,
            ) -> Result<Vec<Delta>, String> {
                Err("provider exploded".to_string())
            }
        }
        let mut provider = ErrProvider;
        let mut renderer = RecordingRenderer::new();
        let cancel = CancelToken::new();
        let err = run_turn(
            &mut provider,
            &[],
            vec![Message::text(Role::User, "hi")],
            &RunConfig::default(),
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
            vec![Message::text(Role::User, "hi")],
            &RunConfig::default(),
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
            vec![Message::text(Role::User, "hi")],
            &RunConfig::default(),
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
        let messages = vec![
            Message::text(Role::System, "be helpful"),
            Message::text(Role::User, "weather?"),
        ];
        let cancel = CancelToken::new();
        let mut seen: Vec<Message> = Vec::new();
        let (updated, _) = run_turn(
            &mut provider,
            &[weather_tool()],
            messages,
            &RunConfig::default(),
            &cancel,
            &mut renderer,
            &mut |m| {
                seen.push(m.clone());
                Ok(())
            },
        )
        .unwrap();
        // The sink saw assistant(tool_calls), tool, then the final assistant —
        // exactly the messages that entered history after the turn's input.
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[0].role, Role::Assistant);
        assert_eq!(seen[0].tool_calls.len(), 1);
        assert_eq!(seen[0].tool_calls[0].id, "call_1");
        assert_eq!(seen[1].role, Role::Tool);
        assert_eq!(seen[1].tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(seen[2].role, Role::Assistant);
        assert_eq!(seen, updated[2..]);
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
            vec![Message::text(Role::User, "hi")],
            &RunConfig::default(),
            &cancel,
            &mut renderer,
            &mut |_| Err("sink exploded".to_string()),
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
            t: &[Tool],
            _c: &CancelToken,
        ) -> Result<Vec<Delta>, String> {
            self.calls += 1;
            if self.calls == 1 {
                self.cancel.cancel();
            }
            self.inner.chat(m, t, &self.cancel)
        }
    }
}
