//! Session metadata around a message history.
//!
//! The wire `Message` model lives in `slimcode-ai` (ADR-0011 D1); this module
//! keeps the session wrapper and re-exports the message types for callers that
//! used to find them here.
//!
//! `Session { id, created_at, messages, title }` wraps the history for the
//! CLI's `~/.slimcode/sessions/` files.

use serde::{Deserialize, Serialize};

pub use slimcode_ai::message::{Message, MessageStopReason, Part, Role, ToolCall};

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
                    stop_reason: None,
                    error: None,
                },
                Message::tool_result("call_1", "{\"temp\": \"25C\"}"),
                Message::text(Role::Assistant, "Beijing is 25C."),
            ],
        };
        let json = serde_json::to_string(&session).unwrap();
        let back: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(session, back);
    }
}
