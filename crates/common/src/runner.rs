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

use slimcode_agent::agent::{Message, Provider, RunConfig, Tool};

use crate::render::{Renderer, map_event};

/// Drive one agent turn over `messages`, streaming every event to `renderer`
/// live as the loop runs, and return the updated message history.
///
/// A provider error propagates as `Err(String)`; no partial history is
/// fabricated.
pub fn run_turn<P: Provider>(
    provider: &mut P,
    tools: &[Tool],
    messages: Vec<Message>,
    cfg: &RunConfig,
    renderer: &mut dyn Renderer,
) -> Result<Vec<Message>, String> {
    slimcode_agent::agent::run_agent_from_messages_sink(provider, tools, messages, cfg, &mut |e| {
        if let Some(item) = map_event(&e) {
            renderer.render(&item)?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::DisplayItem;
    use serde_json::Value;
    use slimcode_agent::agent::{Delta, FinishReason, StopReason};
    use slimcode_agent::session::{Message, Role};

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
        fn chat(&mut self, _messages: &[Message], _tools: &[Tool]) -> Result<Vec<Delta>, String> {
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
        let updated = run_turn(
            &mut provider,
            &[weather_tool()],
            messages,
            &RunConfig::default(),
            &mut renderer,
        )
        .unwrap();

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
        let updated = run_turn(
            &mut provider,
            &[weather_tool()],
            messages.clone(),
            &RunConfig::default(),
            &mut renderer,
        )
        .unwrap();

        let mut provider2 = FakeProvider::new(script);
        let result = slimcode_agent::agent::run_agent_from_messages(
            &mut provider2,
            &[weather_tool()],
            messages,
            &RunConfig::default(),
        )
        .unwrap();
        assert_eq!(updated, result.messages);
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
        run_turn(
            &mut provider,
            &[weather_tool()],
            messages,
            &RunConfig::default(),
            &mut renderer,
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
    fn provider_error_propagates() {
        struct ErrProvider;
        impl Provider for ErrProvider {
            fn chat(&mut self, _m: &[Message], _t: &[Tool]) -> Result<Vec<Delta>, String> {
                Err("provider exploded".to_string())
            }
        }
        let mut provider = ErrProvider;
        let mut renderer = RecordingRenderer::new();
        let err = run_turn(
            &mut provider,
            &[],
            vec![Message::text(Role::User, "hi")],
            &RunConfig::default(),
            &mut renderer,
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
        let err = run_turn(
            &mut provider,
            &[],
            vec![Message::text(Role::User, "hi")],
            &RunConfig::default(),
            &mut renderer,
        )
        .unwrap_err();
        assert!(err.contains("renderer exploded"), "err: {err}");
    }
}
