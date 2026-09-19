//! Session message model: `AgentMessage` (the unit a Session stores) plus the
//! `Session` wrapper and the `to_llm`/`convert` step to the wire model.
//!
//! Two layers (ADR-0012): `slimcode_ai::Message` is the LLM wire message;
//! `AgentMessage` is what a Session's history holds — the user / assistant /
//! tool messages plus the compaction summary the model must never see as such.
//! `AgentMessage::to_llm` is the only conversion point; the LLM variant returns
//! its message, the compaction summary drops itself.
//!
//! The system prompt is not part of a message history (ADR-0012 D3): it is
//! assembled fresh every turn and never stored. Log-only metadata
//! (`stop_reason` / `error`) lives in the session log's record envelope
//! (ADR-0012 D4), not on any message.

use serde::{Deserialize, Serialize};

use slimcode_ai::TokenUsage;

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
/// session-only kinds can join the same history: today the LLM kind plus the
/// compaction-summary kind.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentMessage {
    /// A message the model sees (and `to_llm` passes through).
    Llm(Message),
    /// A session-only compaction checkpoint: the structured summary that
    /// replaced an earlier span of history. The model never sees this variant
    /// itself — `ContextBuilder` injects `summary` as a `user` message at the
    /// variant's position — so `to_llm` drops it (ADR-0012 D2).
    CompactSummary {
        /// The structured summary text (`## Goal`, `## Progress`, ...).
        summary: String,
        /// Estimated tokens the history held before this compaction.
        tokens_before: usize,
        /// The summary this one supersedes, when it was produced by an
        /// incremental update rather than from scratch.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        previous_summary: Option<String>,
    },
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

    /// A compaction checkpoint convenience constructor.
    pub fn compact_summary(
        summary: impl Into<String>,
        tokens_before: usize,
        previous_summary: Option<String>,
    ) -> Self {
        Self::CompactSummary {
            summary: summary.into(),
            tokens_before,
            previous_summary,
        }
    }

    /// Whether this is a session-only compaction checkpoint.
    pub fn is_compact_summary(&self) -> bool {
        matches!(self, Self::CompactSummary { .. })
    }

    /// The only conversion to the wire model (ADR-0012 D2): the LLM variant
    /// returns its message; the compaction checkpoint returns `None` so it
    /// never reaches the provider directly.
    pub fn to_llm(&self) -> Option<Message> {
        match self {
            Self::Llm(message) => Some(message.clone()),
            Self::CompactSummary { .. } => None,
        }
    }

    /// The message role. A compaction checkpoint reports [`Role::User`]: that
    /// is the role its summary takes when injected into a request, and it is
    /// how a loaded log replays the entry as ordinary history.
    pub fn role(&self) -> &Role {
        match self {
            Self::Llm(message) => &message.role,
            Self::CompactSummary { .. } => &Role::User,
        }
    }

    /// Concatenated text: the wire message's text parts, or the summary.
    pub fn text_content(&self) -> String {
        match self {
            Self::Llm(message) => message.text_content(),
            Self::CompactSummary { summary, .. } => summary.clone(),
        }
    }

    /// The tool calls the message carries (assistant messages only; a
    /// compaction checkpoint carries none).
    pub fn tool_calls(&self) -> &[ToolCall] {
        match self {
            Self::Llm(message) => &message.tool_calls,
            Self::CompactSummary { .. } => &[],
        }
    }

    /// The tool call this result answers (tool messages only).
    pub fn tool_call_id(&self) -> Option<&str> {
        match self {
            Self::Llm(message) => message.tool_call_id.as_deref(),
            Self::CompactSummary { .. } => None,
        }
    }

    /// The single mutation entry point (ADR-0015 D3): run hooks rewrite the
    /// wire message a run is about to store. Only the LLM variant has a wire
    /// message to rewrite; a session-only variant (a compaction checkpoint)
    /// has none and returns `None`, so a hook cannot corrupt it.
    pub fn llm_mut(&mut self) -> Option<&mut Message> {
        match self {
            Self::Llm(message) => Some(message),
            Self::CompactSummary { .. } => None,
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
    /// The tokens this live session has spent, in memory only (ADR-0018 D4):
    /// the frontend accumulates the provider's per-turn delta into it and
    /// `/usage` and the footer read it. `serde(skip)` keeps it out of the log
    /// format, so a new or resumed session always starts at zero.
    #[serde(skip)]
    pub usage: TokenUsage,
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
    fn compact_summary_serializes_with_its_own_kind_tag() {
        let m = AgentMessage::compact_summary("## Goal\nfinish", 42, None);
        let v: serde_json::Value = serde_json::to_value(&m).unwrap();
        assert_eq!(v["kind"], "compact_summary");
        assert_eq!(v["summary"], "## Goal\nfinish");
        assert_eq!(v["tokens_before"], 42);
        // The absent previous summary is omitted (not serialized as null).
        assert!(v.get("previous_summary").is_none(), "{v}");
    }

    #[test]
    fn compact_summary_round_trips_losslessly() {
        for previous in [None, Some("## Goal\nolder".to_string())] {
            let m = AgentMessage::compact_summary("## Goal\nnewer", 1024, previous);
            let json = serde_json::to_string(&m).unwrap();
            let back: AgentMessage = serde_json::from_str(&json).unwrap();
            assert_eq!(back, m);
        }
    }

    #[test]
    fn to_llm_drops_the_compact_summary() {
        // The model never sees the checkpoint as such: the context builder
        // injects its summary as a user message instead (ADR-0012 D2).
        let m = AgentMessage::compact_summary("summary", 10, None);
        assert!(m.to_llm().is_none());
    }

    #[test]
    fn compact_summary_reports_user_role_and_its_summary_text() {
        let m = AgentMessage::compact_summary("the summary", 10, None);
        assert!(m.is_compact_summary());
        // It replays as a user-role history entry (the role its injection
        // takes), carrying the summary as its text.
        assert_eq!(m.role(), &Role::User);
        assert_eq!(m.text_content(), "the summary");
        assert!(m.tool_calls().is_empty());
        assert_eq!(m.tool_call_id(), None);
    }

    #[test]
    fn llm_variant_is_not_a_compact_summary() {
        assert!(!AgentMessage::text(Role::User, "hi").is_compact_summary());
    }

    #[test]
    fn llm_mut_refuses_the_compact_summary() {
        let mut m = AgentMessage::compact_summary("summary", 10, None);
        assert!(m.llm_mut().is_none());
        // A wire message is still rewritable, the hook seam's purpose.
        let mut llm = AgentMessage::text(Role::Assistant, "hi");
        llm.llm_mut().unwrap().parts = vec![Part::Text {
            text: "rewritten".to_string(),
        }];
        assert_eq!(llm.text_content(), "rewritten");
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
            usage: TokenUsage::default(),
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
                AgentMessage::compact_summary("## Goal\nweather", 128_000, None),
            ],
        };
        let json = serde_json::to_string(&session).unwrap();
        let back: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(session, back);
    }

    #[test]
    fn session_usage_never_reaches_the_log() {
        let session = Session {
            id: "sess-1".to_string(),
            created_at: "2026-08-29T00:00:00Z".to_string(),
            title: None,
            usage: TokenUsage {
                prompt_tokens: 7,
                completion_tokens: 3,
                total_tokens: 10,
                ..Default::default()
            },
            messages: vec![AgentMessage::text(Role::User, "hi")],
        };
        let json = serde_json::to_string(&session).unwrap();
        assert!(!json.contains("usage"), "{json}");
        // A deserialized session starts at zero: usage is not persisted.
        let back: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(back.usage, TokenUsage::default());
    }
}
