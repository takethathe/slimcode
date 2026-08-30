//! Frontend-agnostic renderer seam (ADR-0004): a `DisplayItem` enum, a pure
//! `map_event` function turning an [`AgentEvent`] into an optional display
//! unit, and a `Renderer` trait every frontend implements.
//!
//! The CLI's `TextRenderer` and the TUI's widget state both consume
//! `DisplayItem`s, so the event-to-display mapping lives here once and the two
//! frontends cannot drift. Text fragments stay faithful to the event: no
//! trimming, no re-wording (the CLI's byte-identical-output guarantee depends
//! on it, see ticket 03).

use slimcode_agent::agent::{AgentEvent, Delta, StopReason};
use slimcode_ai::TokenUsage;

/// A frontend-agnostic display unit consumed by a `Renderer`.
///
/// Produced from an [`AgentEvent`] by [`map_event`] (or handed straight to the
/// renderer for the frontend-owned [`DisplayItem::Usage`] entry).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisplayItem {
    /// A new turn of the agent loop started.
    Turn { turn: usize },
    /// A reasoning line (thinking tokens, qwen-style `reasoning_content`).
    Reasoning(String),
    /// A streamed assistant text fragment (no trailing newline implied).
    Text(String),
    /// A tool invocation started, with the raw JSON arguments string.
    ToolStart { name: String, arguments: String },
    /// A tool finished: `ok` distinguishes success from failure.
    ToolResult {
        name: String,
        ok: bool,
        result: String,
    },
    /// The run ended.
    Stop(StopReason),
    /// Token usage after a run. Frontend-owned: never produced by `map_event`;
    /// the frontend reads its concrete provider's total usage and feeds this
    /// to its renderer itself (spec §Implementation Decisions).
    Usage(TokenUsage),
}

/// Map an agent event to an optional display unit.
///
/// Raw tool-call deltas (`ToolCallStart` / `ToolCallArgs` / `Done`) are
/// suppressed — the loop's aggregated `ToolStart` / `ToolResult` events carry
/// the display content. Every other event kind maps to its own `DisplayItem`,
/// faithful to the event payload.
pub fn map_event(e: &AgentEvent) -> Option<DisplayItem> {
    match e {
        AgentEvent::Turn { turn } => Some(DisplayItem::Turn { turn: *turn }),
        AgentEvent::Stream(Delta::Reasoning(t)) => Some(DisplayItem::Reasoning(t.clone())),
        AgentEvent::Stream(Delta::Text(t)) => Some(DisplayItem::Text(t.clone())),
        AgentEvent::Stream(Delta::ToolCallStart { .. })
        | AgentEvent::Stream(Delta::ToolCallArgs { .. })
        | AgentEvent::Stream(Delta::Done(_)) => None,
        AgentEvent::ToolStart { name, arguments } => Some(DisplayItem::ToolStart {
            name: name.clone(),
            arguments: arguments.clone(),
        }),
        AgentEvent::ToolResult { name, ok, result } => Some(DisplayItem::ToolResult {
            name: name.clone(),
            ok: *ok,
            result: result.clone(),
        }),
        AgentEvent::Stop(reason) => Some(DisplayItem::Stop(reason.clone())),
    }
}

/// The per-frontend component that turns `DisplayItem`s into frontend output —
/// text lines for the CLI, widget state for the TUI (CONTEXT.md: Renderer).
///
/// Errors stay `String`, matching the existing agent error style (ADR-0004).
pub trait Renderer {
    /// Render one display item, appending to this frontend's output.
    fn render(&mut self, item: &DisplayItem) -> Result<(), String>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use slimcode_agent::agent::FinishReason;

    fn turn(n: usize) -> AgentEvent {
        AgentEvent::Turn { turn: n }
    }
    fn reasoning(t: &str) -> AgentEvent {
        AgentEvent::Stream(Delta::Reasoning(t.to_string()))
    }
    fn text(t: &str) -> AgentEvent {
        AgentEvent::Stream(Delta::Text(t.to_string()))
    }
    fn tool_start(name: &str) -> AgentEvent {
        AgentEvent::ToolStart {
            name: name.to_string(),
            arguments: "{}".to_string(),
        }
    }
    fn tool_result(name: &str, ok: bool) -> AgentEvent {
        AgentEvent::ToolResult {
            name: name.to_string(),
            ok,
            result: "out".to_string(),
        }
    }
    fn stop(reason: StopReason) -> AgentEvent {
        AgentEvent::Stop(reason)
    }

    #[test]
    fn turn_maps_to_turn_marker() {
        assert_eq!(map_event(&turn(2)), Some(DisplayItem::Turn { turn: 2 }));
    }

    #[test]
    fn reasoning_maps_to_reasoning_line() {
        assert_eq!(
            map_event(&reasoning("think")),
            Some(DisplayItem::Reasoning("think".to_string()))
        );
    }

    #[test]
    fn text_maps_to_faithful_text_fragment() {
        // No trimming, no re-wording: the fragment is byte-identical.
        assert_eq!(
            map_event(&text("hello ")),
            Some(DisplayItem::Text("hello ".to_string()))
        );
        assert_eq!(
            map_event(&text("  padded\n")),
            Some(DisplayItem::Text("  padded\n".to_string()))
        );
    }

    #[test]
    fn raw_tool_deltas_are_suppressed() {
        assert!(
            map_event(&AgentEvent::Stream(Delta::ToolCallStart {
                index: 0,
                id: "c1".into(),
                name: "read".into(),
            }))
            .is_none()
        );
        assert!(
            map_event(&AgentEvent::Stream(Delta::ToolCallArgs {
                index: 0,
                fragment: "{}".into(),
            }))
            .is_none()
        );
        assert!(map_event(&AgentEvent::Stream(Delta::Done(FinishReason::Stop))).is_none());
        assert!(map_event(&AgentEvent::Stream(Delta::Done(FinishReason::ToolCalls))).is_none());
    }

    #[test]
    fn tool_events_map_to_their_own_items() {
        assert_eq!(
            map_event(&tool_start("read")),
            Some(DisplayItem::ToolStart {
                name: "read".to_string(),
                arguments: "{}".to_string(),
            })
        );
        assert_eq!(
            map_event(&tool_result("bash", true)),
            Some(DisplayItem::ToolResult {
                name: "bash".to_string(),
                ok: true,
                result: "out".to_string(),
            })
        );
        assert_eq!(
            map_event(&tool_result("bash", false)),
            Some(DisplayItem::ToolResult {
                name: "bash".to_string(),
                ok: false,
                result: "out".to_string(),
            })
        );
    }

    #[test]
    fn stop_maps_to_stop_marker() {
        assert_eq!(
            map_event(&stop(StopReason::Completed)),
            Some(DisplayItem::Stop(StopReason::Completed))
        );
        assert_eq!(
            map_event(&stop(StopReason::MaxIterations)),
            Some(DisplayItem::Stop(StopReason::MaxIterations))
        );
    }

    #[test]
    fn mapped_order_for_a_small_stream() {
        let events = vec![
            turn(1),
            reasoning("Let me think"),
            text("answer "),
            text("fragment"),
            AgentEvent::Stream(Delta::ToolCallStart {
                index: 0,
                id: "c1".into(),
                name: "read".into(),
            }),
            AgentEvent::Stream(Delta::ToolCallArgs {
                index: 0,
                fragment: "{}".into(),
            }),
            AgentEvent::Stream(Delta::Done(FinishReason::Stop)),
            tool_start("read"),
            tool_result("read", true),
            stop(StopReason::Completed),
        ];
        let mapped: Vec<DisplayItem> = events.iter().filter_map(map_event).collect();
        assert_eq!(
            mapped,
            vec![
                DisplayItem::Turn { turn: 1 },
                DisplayItem::Reasoning("Let me think".to_string()),
                DisplayItem::Text("answer ".to_string()),
                DisplayItem::Text("fragment".to_string()),
                DisplayItem::ToolStart {
                    name: "read".to_string(),
                    arguments: "{}".to_string(),
                },
                DisplayItem::ToolResult {
                    name: "read".to_string(),
                    ok: true,
                    result: "out".to_string(),
                },
                DisplayItem::Stop(StopReason::Completed),
            ]
        );
    }
}
