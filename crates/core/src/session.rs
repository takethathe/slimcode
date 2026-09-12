//! Session message model: `AgentMessage` (the unit a Session stores) plus the
//! `Session` wrapper and the `to_llm`/`convert` step to the wire model.
//!
//! Two layers (ADR-0012): `slimcode_ai::Message` is the LLM wire message;
//! `AgentMessage` is what a Session's history holds — today the user /
//! assistant / tool messages, plus kinds the model must never see (the first
//! planned one is a compaction summary). `AgentMessage::to_llm` is the only
//! conversion point; the LLM variant returns its message, session-only
//! variants decide to transform or drop.
//!
//! The system prompt is not part of a message history (ADR-0012 D3): it is
//! assembled fresh every turn and never stored. Log-only metadata
//! (`stop_reason` / `error`) lives in the session log's record envelope
//! (ADR-0012 D4), not on any message.

use serde::{Deserialize, Serialize};

pub use slimcode_ai::message::{Message, Part, Role, ToolCall};

/// Why a turn closed, recorded in a session-log message record's envelope
/// (ADR-0009 D5, ADR-0012 D4). `stop` / `tool_calls` mirror the provider
/// finish reasons; `error` / `aborted` mark a turn that failed or was
/// cancelled. Never part of a message payload.
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

/// One message in a Session's history (ADR-0012 D1). A serde-tagged enum so
/// session-only kinds can join the same history later; today only the LLM
/// kind exists.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentMessage {
    /// A message the model sees (and `to_llm` passes through).
    Llm(Message),
}

impl AgentMessage {
    /// Wrap a wire message as the LLM variant.
    pub fn llm(message: Message) -> Self {
        Self::Llm(message)
    }

    /// A plain text message convenience constructor.
    pub fn text(role: Role, content: impl Into<String>) -> Self {
        Self::Llm(Message::text(role, content))
    }

    /// A tool-result message convenience constructor.
    pub fn tool_result(id: impl Into<String>, content: impl Into<String>) -> Self {
        Self::Llm(Message::tool_result(id, content))
    }

    /// The only conversion to the wire model (ADR-0012 D2): the LLM variant
    /// returns its message; a session-only variant would decide its own fate
    /// (`None` drops it).
    pub fn to_llm(&self) -> Option<Message> {
        match self {
            Self::Llm(message) => Some(message.clone()),
        }
    }

    /// The message role.
    pub fn role(&self) -> &Role {
        match self {
            Self::Llm(message) => &message.role,
        }
    }

    /// Concatenated text of the message's text parts.
    pub fn text_content(&self) -> String {
        match self {
            Self::Llm(message) => message.text_content(),
        }
    }

    /// The tool calls the message carries (assistant messages only).
    pub fn tool_calls(&self) -> &[ToolCall] {
        match self {
            Self::Llm(message) => &message.tool_calls,
        }
    }

    /// The tool call this result answers (tool messages only).
    pub fn tool_call_id(&self) -> Option<&str> {
        match self {
            Self::Llm(message) => message.tool_call_id.as_deref(),
        }
    }
}

/// Prepending the system message to the `to_llm`-converted history is the only
/// way a request is assembled (ADR-0012 D2). `core`'s loop calls this before
/// **every** provider request, so a mid-run compaction takes effect on the
/// next turn.
pub fn convert(system: &Message, history: &[AgentMessage]) -> Vec<Message> {
    let mut out = Vec::with_capacity(history.len() + 1);
    out.push(system.clone());
    out.extend(history.iter().filter_map(AgentMessage::to_llm));
    out
}

/// Session metadata around a message history, ready for the session store.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub created_at: String,
    pub messages: Vec<AgentMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llm_variant_serializes_with_a_kind_tag() {
        let m = AgentMessage::text(Role::User, "hello");
        let v: serde_json::Value = serde_json::to_value(&m).unwrap();
        assert_eq!(v["kind"], "llm");
        assert_eq!(v["role"], "user");
        assert_eq!(v["parts"][0]["text"], "hello");
    }

    #[test]
    fn to_llm_passes_the_wire_message_through() {
        let m = AgentMessage::text(Role::User, "hello");
        let wire = m.to_llm().unwrap();
        assert_eq!(wire.role, Role::User);
        assert_eq!(wire.text_content(), "hello");
    }

    #[test]
    fn convert_prefixes_the_system_message_in_history_order() {
        let system = Message::text(Role::System, "be helpful");
        let history = vec![
            AgentMessage::text(Role::User, "hi"),
            AgentMessage::text(Role::Assistant, "hello"),
            AgentMessage::text(Role::User, "again"),
        ];
        let wire = convert(&system, &history);
        assert_eq!(wire.len(), 4);
        assert_eq!(wire[0].role, Role::System);
        assert_eq!(wire[0].text_content(), "be helpful");
        assert_eq!(wire[1].text_content(), "hi");
        assert_eq!(wire[2].text_content(), "hello");
        assert_eq!(wire[3].text_content(), "again");
    }

    #[test]
    fn session_round_trips_losslessly() {
        let session = Session {
            id: "sess-1".to_string(),
            created_at: "2026-08-29T00:00:00Z".to_string(),
            title: None,
            messages: vec![
                AgentMessage::text(Role::User, "weather?"),
                AgentMessage::Llm(Message {
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
                }),
                AgentMessage::tool_result("call_1", "{\"temp\": \"25C\"}"),
                AgentMessage::text(Role::Assistant, "Beijing is 25C."),
            ],
        };
        let json = serde_json::to_string(&session).unwrap();
        let back: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(session, back);
    }
}
