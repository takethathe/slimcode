//! The LLM wire message model, owned by `slimcode-ai`.
//!
//! Shape locked by `.scratch/slimcode-v1` ticket 04:
//! - `Message { role, parts: Vec<Part>, tool_calls, tool_call_id }`
//! - `Part = Text { text }` (parts abstraction; Text is the v1 default/only kind),
//!   serialized as `{"type":"text","text":"..."}` (pi TextPart shape).
//! - `ToolCall { id, name, arguments }` — `arguments` keeps the raw JSON string
//!   the model emitted; it is parsed only at execution time.
//!
//! JSON boundary: `tool_calls` and `tool_call_id` are omitted when absent so
//! plain text messages stay compact; `stop_reason` and `error` (the log
//! schema's turn-closing fields, ADR-0009 D5) are also omitted when absent, so
//! an ordinary message serializes byte-identically to before. The whole graph
//! round-trips losslessly.

use serde::{Deserialize, Serialize};

/// Message role. Lowercase on the wire (matches the OpenAI-compatible API).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// One content part of a message. v1 only has `Text`; the parts abstraction
/// leaves room for image/tool parts later without changing the message shape.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Part {
    Text { text: String },
}

/// A model-emitted tool invocation (assistant messages only).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Raw JSON arguments string as the model emitted it.
    pub arguments: String,
}

/// Why a message closed a turn (the session-log schema's `stop_reason` field,
/// ADR-0009 D5). `stop` / `tool_calls` mirror the provider finish reasons;
/// `error` / `aborted` mark a turn that failed or was cancelled, carried by the
/// assistant message that closes the log. Omitted on ordinary messages.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageStopReason {
    /// The model stopped with a final answer.
    Stop,
    /// The model requested tools.
    ToolCalls,
    /// The turn ended because of an error.
    Error,
    /// The turn was cancelled (Esc).
    Aborted,
}

/// One message in the conversation history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub parts: Vec<Part>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Why this message closed a turn (log schema, ADR-0009 D5). `None` on
    /// ordinary messages; wire-invisible (the provider mapping ignores it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<MessageStopReason>,
    /// Error text carried by a failure-closing message
    /// (`stop_reason == Some(MessageStopReason::Error)`). Wire-invisible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Message {
    /// Build a plain text message (`parts` = one `Text` part).
    pub fn text(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            parts: vec![Part::Text {
                text: content.into(),
            }],
            tool_calls: Vec::new(),
            tool_call_id: None,
            stop_reason: None,
            error: None,
        }
    }

    /// Build a tool-result message; the tool output goes in as `content`.
    pub fn tool_result(id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            parts: vec![Part::Text {
                text: content.into(),
            }],
            tool_calls: Vec::new(),
            tool_call_id: Some(id.into()),
            stop_reason: None,
            error: None,
        }
    }

    /// Concatenated text of all `Text` parts.
    pub fn text_content(&self) -> String {
        self.parts
            .iter()
            .map(|p| match p {
                Part::Text { text } => text.as_str(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_serialize_lowercase() {
        let m = Message::text(Role::System, "sys");
        let v: serde_json::Value = serde_json::to_value(&m).unwrap();
        assert_eq!(v["role"], "system");
        let m = Message::text(Role::Assistant, "hi");
        let v: serde_json::Value = serde_json::to_value(&m).unwrap();
        assert_eq!(v["role"], "assistant");
        let m = Message::tool_result("c1", "out");
        let v: serde_json::Value = serde_json::to_value(&m).unwrap();
        assert_eq!(v["role"], "tool");
    }

    #[test]
    fn text_part_serializes_with_type_tag() {
        let m = Message::text(Role::User, "hello");
        let v: serde_json::Value = serde_json::to_value(&m).unwrap();
        assert_eq!(
            v["parts"][0],
            serde_json::json!({"type": "text", "text": "hello"})
        );
    }

    #[test]
    fn empty_tool_fields_omitted() {
        let m = Message::text(Role::User, "hi");
        let v: serde_json::Value = serde_json::to_value(&m).unwrap();
        assert!(v.get("tool_calls").is_none());
        assert!(v.get("tool_call_id").is_none());
    }

    #[test]
    fn message_text_joins_text_parts() {
        let m = Message {
            role: Role::Assistant,
            parts: vec![
                Part::Text { text: "a".into() },
                Part::Text { text: "b".into() },
            ],
            tool_calls: vec![],
            tool_call_id: None,
            stop_reason: None,
            error: None,
        };
        assert_eq!(m.text_content(), "ab");
    }

    #[test]
    fn stop_reason_and_error_round_trip_and_are_omitted_when_absent() {
        let closing = Message {
            role: Role::Assistant,
            parts: vec![Part::Text {
                text: "The turn ended with an error: boom".into(),
            }],
            tool_calls: vec![],
            tool_call_id: None,
            stop_reason: Some(MessageStopReason::Error),
            error: Some("boom".to_string()),
        };
        let json = serde_json::to_string(&closing).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["stop_reason"], "error");
        assert_eq!(v["error"], "boom");
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back, closing);

        // Every stop reason value round-trips.
        for reason in [
            MessageStopReason::Stop,
            MessageStopReason::ToolCalls,
            MessageStopReason::Error,
            MessageStopReason::Aborted,
        ] {
            let mut m = Message::text(Role::Assistant, "x");
            m.stop_reason = Some(reason.clone());
            let back: Message = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
            assert_eq!(back.stop_reason, Some(reason));
        }
    }

    #[test]
    fn ordinary_message_serializes_without_new_fields() {
        // An ordinary message (both new fields None) keeps the exact byte
        // shape of the pre-log model: the new fields are omitted, not empty.
        let m = Message::text(Role::User, "hello");
        let v: serde_json::Value = serde_json::to_value(&m).unwrap();
        assert!(v.get("stop_reason").is_none());
        assert!(v.get("error").is_none());
        // Old serialized bytes (no new fields) still deserialize.
        let old = r#"{"role":"user","parts":[{"type":"text","text":"hello"}]}"#;
        let back: Message = serde_json::from_str(old).unwrap();
        assert_eq!(back, m);
    }
}
