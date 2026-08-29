//! Streaming renderer: maps `AgentEvent`s to terminal output.
//!
//! Two kinds of output: live text (printed as it streams, no trailing newline)
//! and structural lines (tool starts/results, stop markers). Raw
//! `ToolCallStart`/`ToolCallArgs`/`Done` deltas are suppressed — the loop's
//! aggregated `ToolStart`/`ToolResult` events carry the display content.

use slimcode_agent::agent::{AgentEvent, Delta, StopReason};
use slimcode_ai::TokenUsage;

/// A piece of rendered output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderText {
    pub text: String,
    /// `true` = print as-is and flush (live assistant text); `false` = a line
    /// with a trailing newline.
    pub streamed: bool,
}

/// Render a structural line (or `None` to print nothing).
pub fn render_event(e: &AgentEvent) -> Option<RenderText> {
    match e {
        AgentEvent::Turn { turn } => Some(RenderText {
            text: format!("── turn {turn} ──"),
            streamed: false,
        }),
        AgentEvent::Stream(Delta::Reasoning(t)) => Some(RenderText {
            text: format!("> {t}"),
            streamed: false,
        }),
        AgentEvent::Stream(Delta::Text(t)) => Some(RenderText {
            text: t.clone(),
            streamed: true,
        }),
        AgentEvent::Stream(Delta::ToolCallStart { .. })
        | AgentEvent::Stream(Delta::ToolCallArgs { .. })
        | AgentEvent::Stream(Delta::Done(_)) => None,
        AgentEvent::ToolStart { name, arguments } => Some(RenderText {
            text: format!("  ▶ {name} {arguments}"),
            streamed: false,
        }),
        AgentEvent::ToolResult { name, ok, result } => {
            let icon = if *ok { "✔" } else { "✖" };
            Some(RenderText {
                text: format!("  {icon} {name}: {result}"),
                streamed: false,
            })
        }
        AgentEvent::Stop(StopReason::Completed) => Some(RenderText {
            text: "✓ done".to_string(),
            streamed: false,
        }),
        AgentEvent::Stop(StopReason::MaxIterations) => Some(RenderText {
            text: "⚠ stopped: max iterations reached".to_string(),
            streamed: false,
        }),
    }
}

/// Render token usage as a summary line.
pub fn render_usage(u: &TokenUsage) -> String {
    format!(
        "tokens: {} prompt + {} completion = {} total",
        u.prompt_tokens, u.completion_tokens, u.total_tokens
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use slimcode_agent::agent::FinishReason;

    #[test]
    fn stream_text_is_marked_streamed() {
        let r = render_event(&AgentEvent::Stream(Delta::Text("hi".into()))).unwrap();
        assert!(r.streamed);
        assert_eq!(r.text, "hi");
    }

    #[test]
    fn reasoning_renders_as_prefixed_line() {
        let r = render_event(&AgentEvent::Stream(Delta::Reasoning("think".into()))).unwrap();
        assert!(!r.streamed);
        assert_eq!(r.text, "> think");
    }

    #[test]
    fn raw_tool_deltas_are_suppressed() {
        assert!(
            render_event(&AgentEvent::Stream(Delta::ToolCallStart {
                index: 0,
                id: "c1".into(),
                name: "read".into(),
            }))
            .is_none()
        );
        assert!(
            render_event(&AgentEvent::Stream(Delta::ToolCallArgs {
                index: 0,
                fragment: "{}".into(),
            }))
            .is_none()
        );
        assert!(render_event(&AgentEvent::Stream(Delta::Done(FinishReason::Stop))).is_none());
    }

    #[test]
    fn tool_events_render_name_and_body() {
        let start = render_event(&AgentEvent::ToolStart {
            name: "read".into(),
            arguments: "{\"path\": \"a.txt\"}".into(),
        })
        .unwrap();
        assert!(start.text.contains("▶ read"), "got: {}", start.text);

        let ok = render_event(&AgentEvent::ToolResult {
            name: "read".into(),
            ok: true,
            result: "hello".into(),
        })
        .unwrap();
        assert!(ok.text.contains("✔ read: hello"), "got: {}", ok.text);

        let err = render_event(&AgentEvent::ToolResult {
            name: "bash".into(),
            ok: false,
            result: "boom".into(),
        })
        .unwrap();
        assert!(err.text.contains("✖ bash: boom"), "got: {}", err.text);
    }

    #[test]
    fn stop_reasons_render() {
        let c = render_event(&AgentEvent::Stop(StopReason::Completed)).unwrap();
        assert!(c.text.contains("done"));
        let m = render_event(&AgentEvent::Stop(StopReason::MaxIterations)).unwrap();
        assert!(m.text.contains("max iterations"));
    }

    #[test]
    fn usage_line_sums_tokens() {
        let u = TokenUsage {
            prompt_tokens: 5,
            completion_tokens: 3,
            total_tokens: 8,
        };
        assert_eq!(
            render_usage(&u),
            "tokens: 5 prompt + 3 completion = 8 total"
        );
    }
}
