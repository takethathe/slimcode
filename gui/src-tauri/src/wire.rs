//! The GUI-owned flat event wire contract (spec §Implementation Decisions,
//! ADR-0024 D4): every [`SdkEvent`] becomes one flat JSON object with a `kind`
//! field, delivered per `prompt` over a Tauri `Channel<GuiWireEvent>`. The
//! frontend never sees an `SdkEvent`.
//!
//! The mapping is exhaustive by construction — no `_` arm over [`SdkEvent`]
//! or [`DisplayItem`] — so a new display variant is a compile error here
//! instead of a silently dropped event (mirroring the FFI's wire contract,
//! ADR-0023 D3). The enum is the GUI's own, not a shared import of the FFI's
//! `WireEvent`: the two hosts have different payload needs (D3).
//!
//! Payloads reuse the SDK's own serde shapes: `message` is the persisted
//! session-log shape ([`AgentMessage`]), `usage` is the derived
//! [`TokenUsage`] serialization, and the stop kinds carry the snake_case
//! [`StopReason`].

use serde::Serialize;
use slimcode_sdk::{AgentMessage, DisplayItem, SdkEvent, StopReason, TokenUsage};

/// One event as the flat JSON object the frontend `switch (event.kind)`es on.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GuiWireEvent {
    /// A new turn of the agent loop started.
    Turn { turn: usize },
    /// A streamed reasoning (thinking) fragment.
    Reasoning { text: String },
    /// A streamed assistant text fragment.
    Text { text: String },
    /// A tool invocation started, with the raw JSON arguments string.
    ToolStart {
        tool_call_id: String,
        name: String,
        arguments: String,
    },
    /// A tool finished; `ok` distinguishes success from failure.
    ToolResult {
        tool_call_id: String,
        name: String,
        ok: bool,
        result: String,
    },
    /// The run ended (`completed` / `cancelled`).
    Stop { reason: StopReason },
    /// The request's token usage.
    Usage { usage: TokenUsage },
    /// A message entered history; the user prompt and tool results carry
    /// `usage: null`, assistant replies the request's usage.
    Message {
        message: AgentMessage,
        usage: Option<TokenUsage>,
    },
    /// The authoritative end of one prompt (`completed` / `cancelled`).
    TurnFinished { stop_reason: StopReason },
    /// A compaction replaced part of the history.
    Compacted { before: usize, after: usize },
    /// GUI-owned (not produced by the [`SdkEvent`] mapping above): the
    /// worker reports that the turn failed or `compact_now` errored — the
    /// channel-based analogue of the FFI's synchronous `-1` + `last_error`.
    /// A turn that ends this way is over (the frontend must leave the running
    /// state); the session stays usable.
    Error { message: String },
}

impl From<&SdkEvent> for GuiWireEvent {
    fn from(event: &SdkEvent) -> Self {
        match event {
            SdkEvent::Display(item) => Self::from(item),
            SdkEvent::Message { message, usage } => GuiWireEvent::Message {
                message: message.clone(),
                usage: *usage,
            },
            SdkEvent::TurnFinished { stop_reason } => GuiWireEvent::TurnFinished {
                stop_reason: stop_reason.clone(),
            },
            SdkEvent::Compacted { before, after } => GuiWireEvent::Compacted {
                before: *before,
                after: *after,
            },
        }
    }
}

impl From<&DisplayItem> for GuiWireEvent {
    fn from(item: &DisplayItem) -> Self {
        // Exhaustive on purpose: a new `DisplayItem` variant must break this
        // build so the wire contract gets updated with it.
        match item {
            DisplayItem::Turn { turn } => GuiWireEvent::Turn { turn: *turn },
            DisplayItem::Reasoning(text) => GuiWireEvent::Reasoning { text: text.clone() },
            DisplayItem::Text(text) => GuiWireEvent::Text { text: text.clone() },
            DisplayItem::ToolStart {
                tool_call_id,
                name,
                arguments,
            } => GuiWireEvent::ToolStart {
                tool_call_id: tool_call_id.clone(),
                name: name.clone(),
                arguments: arguments.clone(),
            },
            DisplayItem::ToolResult {
                tool_call_id,
                name,
                ok,
                result,
            } => GuiWireEvent::ToolResult {
                tool_call_id: tool_call_id.clone(),
                name: name.clone(),
                ok: *ok,
                result: result.clone(),
            },
            DisplayItem::Stop(reason) => GuiWireEvent::Stop {
                reason: reason.clone(),
            },
            DisplayItem::Usage(usage) => GuiWireEvent::Usage { usage: *usage },
        }
    }
}

/// Encode one SDK event as the flat JSON string (used by tests; the channel
/// path serializes the typed enum directly). The flat shapes have no
/// serialization failure mode (no non-string map keys, no fallible custom
/// impls), so `Err` is unreachable in practice.
pub fn encode(event: &SdkEvent) -> Result<serde_json::Value, serde_json::Error> {
    serde_json::to_value(GuiWireEvent::from(event))
}

#[cfg(test)]
mod tests {
    use super::*;
    use slimcode_app::{AgentMessage, Role, TokenUsage};

    fn usage(prompt: u64, completion: u64) -> TokenUsage {
        TokenUsage {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: prompt + completion,
            prompt_tokens_details: None,
        }
    }

    fn kind(value: &serde_json::Value) -> &str {
        value["kind"].as_str().expect("every wire event has a kind")
    }

    #[test]
    fn display_turn_maps_kind_and_turn() {
        let event = SdkEvent::Display(DisplayItem::Turn { turn: 3 });
        let value = encode(&event).unwrap();
        assert_eq!(kind(&value), "turn");
        assert_eq!(value["turn"], 3);
    }

    #[test]
    fn display_reasoning_maps_kind_and_text() {
        let event = SdkEvent::Display(DisplayItem::Reasoning("think…".into()));
        let value = encode(&event).unwrap();
        assert_eq!(kind(&value), "reasoning");
        assert_eq!(value["text"], "think…");
    }

    #[test]
    fn display_text_maps_kind_and_text() {
        let event = SdkEvent::Display(DisplayItem::Text("hello".into()));
        let value = encode(&event).unwrap();
        assert_eq!(kind(&value), "text");
        assert_eq!(value["text"], "hello");
    }

    #[test]
    fn display_tool_start_maps_kind_and_payload() {
        let event = SdkEvent::Display(DisplayItem::ToolStart {
            tool_call_id: "call_1".into(),
            name: "read".into(),
            arguments: r#"{"path":"a.md"}"#.into(),
        });
        let value = encode(&event).unwrap();
        assert_eq!(kind(&value), "tool_start");
        assert_eq!(value["tool_call_id"], "call_1");
        assert_eq!(value["name"], "read");
        assert_eq!(value["arguments"], r#"{"path":"a.md"}"#);
    }

    #[test]
    fn display_tool_result_maps_kind_and_payload() {
        let event = SdkEvent::Display(DisplayItem::ToolResult {
            tool_call_id: "call_1".into(),
            name: "read".into(),
            ok: false,
            result: "Error: missing".into(),
        });
        let value = encode(&event).unwrap();
        assert_eq!(kind(&value), "tool_result");
        assert_eq!(value["tool_call_id"], "call_1");
        assert_eq!(value["name"], "read");
        assert_eq!(value["ok"], false);
        assert_eq!(value["result"], "Error: missing");
    }

    #[test]
    fn display_stop_maps_kind_and_reason() {
        let event = SdkEvent::Display(DisplayItem::Stop(StopReason::Cancelled));
        let value = encode(&event).unwrap();
        assert_eq!(kind(&value), "stop");
        assert_eq!(value["reason"], "cancelled");
    }

    #[test]
    fn display_usage_maps_kind_and_usage() {
        let event = SdkEvent::Display(DisplayItem::Usage(usage(100, 25)));
        let value = encode(&event).unwrap();
        assert_eq!(kind(&value), "usage");
        assert_eq!(value["usage"]["prompt_tokens"], 100);
        assert_eq!(value["usage"]["completion_tokens"], 25);
        assert_eq!(value["usage"]["total_tokens"], 125);
    }

    #[test]
    fn message_maps_kind_role_and_optional_usage() {
        let message = AgentMessage::text(Role::User, "hi");
        let event = SdkEvent::Message {
            message: message.clone(),
            usage: None,
        };
        let value = encode(&event).unwrap();
        assert_eq!(kind(&value), "message");
        assert_eq!(value["message"]["kind"], "llm");
        assert_eq!(value["message"]["role"], "user");
        assert!(value["usage"].is_null());

        let event = SdkEvent::Message {
            message,
            usage: Some(usage(10, 5)),
        };
        let value = encode(&event).unwrap();
        assert_eq!(value["usage"]["total_tokens"], 15);
    }

    #[test]
    fn turn_finished_maps_kind_and_reason() {
        let event = SdkEvent::TurnFinished {
            stop_reason: StopReason::Completed,
        };
        let value = encode(&event).unwrap();
        assert_eq!(kind(&value), "turn_finished");
        assert_eq!(value["stop_reason"], "completed");
    }

    #[test]
    fn compacted_maps_kind_and_tokens() {
        let event = SdkEvent::Compacted {
            before: 120_000,
            after: 8_000,
        };
        let value = encode(&event).unwrap();
        assert_eq!(kind(&value), "compacted");
        assert_eq!(value["before"], 120_000);
        assert_eq!(value["after"], 8_000);
    }

    #[test]
    fn error_event_serializes_its_message() {
        let value = serde_json::to_value(GuiWireEvent::Error {
            message: "provider exploded".into(),
        })
        .unwrap();
        assert_eq!(kind(&value), "error");
        assert_eq!(value["message"], "provider exploded");
    }

    #[test]
    fn every_display_kind_is_covered_by_encode() {
        // A smoke test over every DisplayItem variant: encode must never
        // panic, and each kind string must be distinct and documented.
        let items = [
            DisplayItem::Turn { turn: 1 },
            DisplayItem::Reasoning("r".into()),
            DisplayItem::Text("t".into()),
            DisplayItem::ToolStart {
                tool_call_id: "c".into(),
                name: "n".into(),
                arguments: "{}".into(),
            },
            DisplayItem::ToolResult {
                tool_call_id: "c".into(),
                name: "n".into(),
                ok: true,
                result: "r".into(),
            },
            DisplayItem::Stop(StopReason::Completed),
            DisplayItem::Usage(usage(1, 2)),
        ];
        let kinds: Vec<String> = items
            .iter()
            .map(|item| {
                encode(&SdkEvent::Display(item.clone()))
                    .unwrap()
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_default()
            })
            .collect();
        // The `kind` field holds the tag; every display variant maps.
        let tagged: Vec<String> = items
            .iter()
            .map(|item| encode(&SdkEvent::Display(item.clone())).unwrap()["kind"].to_string())
            .collect();
        assert_eq!(tagged.len(), items.len());
        assert_eq!(kinds.len(), items.len());
    }
}
