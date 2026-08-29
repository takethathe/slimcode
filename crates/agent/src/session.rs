//! Message / session data model, folded from the ticket 04 prototype.
//!
//! Shape locked by `.scratch/slimcode-v1` ticket 04:
//! - `Message { role, parts: Vec<Part>, tool_calls, tool_call_id }`
//! - `Part = Text { text }` (parts abstraction; Text is the v1 default/only kind),
//!   serialized as `{"type":"text","text":"..."}` (pi TextPart shape).
//! - `ToolCall { id, name, arguments }` — `arguments` keeps the raw JSON string
//!   the model emitted; it is parsed only at execution time.
//! - `Session { id, created_at, messages, title }` wraps the history for the
//!   CLI's `~/.slimcode/sessions/` JSON files.
//!
//! JSON boundary: `tool_calls` and `tool_call_id` are omitted when absent so
//! plain text messages stay compact; the whole graph round-trips losslessly.

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

/// One message in the conversation history.
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

/// Session metadata around a message history, ready for the CLI's
/// `~/.slimcode/sessions/` JSON files.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub created_at: String,
    pub messages: Vec<Message>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
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
    fn session_round_trips_losslessly() {
        let session = Session {
            id: "sess-1".to_string(),
            created_at: "2026-08-29T00:00:00Z".to_string(),
            title: None,
            messages: vec![
                Message::text(Role::System, "be helpful"),
                Message::text(Role::User, "weather?"),
                Message {
                    role: Role::Assistant,
                    parts: vec![Part::Text {
                        text: String::new(),
                    }],
                    tool_calls: vec![ToolCall {
                        id: "call_1".to_string(),
                        name: "get_weather".to_string(),
                        arguments: "{\"city\": \"Beijing\"}".to_string(),
                    }],
                    tool_call_id: None,
                },
                Message::tool_result("call_1", "{\"temp\": \"25C\"}"),
                Message::text(Role::Assistant, "Beijing is 25C."),
            ],
        };
        let json = serde_json::to_string(&session).unwrap();
        let back: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(session, back);
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
}
