//! The LLM wire message model, owned by `slimcode-ai`.
//!
//! Shape locked by `.scratch/slimcode-v1` ticket 04:
//! - `Message { role, parts: Vec<Part>, tool_calls, tool_call_id }`
//! - `Part = Text { text }` (parts abstraction; Text is the v1 default/only kind),
//!   serialized as `{"type":"text","text":"..."}` (pi TextPart shape).
//! - `ToolCall { id, name, arguments }` — `arguments` keeps the raw JSON string
//!   the model emitted; it is parsed only at execution time.
//!
//! This is the wire shape only (ADR-0012 D1): the log-only `stop_reason` /
//! `error` fields live in the session log's record envelope (`slimcode-app`),
//! never on the message. JSON boundary: `tool_calls` and `tool_call_id` are
//! omitted when absent so plain text messages stay compact, and the whole
//! graph round-trips losslessly.

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

/// One LLM-visible message: the wire shape the provider sees.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub parts: Vec<Part>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
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
        };
        assert_eq!(m.text_content(), "ab");
    }

    #[test]
    fn message_serializes_without_log_only_fields() {
        // The wire message has no `stop_reason`/`error` (ADR-0012 D4): those
        // live in the session log record envelope. The payload keeps the exact
        // byte shape of the pre-log model.
        let m = Message::text(Role::User, "hello");
        let v: serde_json::Value = serde_json::to_value(&m).unwrap();
        assert!(v.get("stop_reason").is_none());
        assert!(v.get("error").is_none());
        // Old serialized bytes (no new fields) still deserialize.
        let old = r#"{"role":"user","parts":[{"type":"text","text":"hello"}]}"#;
        let back: Message = serde_json::from_str(old).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn message_tolerates_legacy_log_only_fields() {
        // A session written before the split carried `stop_reason`/`error` on
        // the message; reading it as a wire message ignores them.
        let legacy = r#"{"role":"assistant","parts":[{"type":"text","text":"x"}],"stop_reason":"error","error":"boom"}"#;
        let m: Message = serde_json::from_str(legacy).unwrap();
        assert_eq!(m.role, Role::Assistant);
        assert_eq!(m.text_content(), "x");
    }
}
