//! The FFI-owned flat event wire contract (spec D3, ticket 02).
//!
//! Every [`SdkEvent`] becomes one flat JSON object with a `kind` field, ready
//! for a host `switch (event.kind)`, exactly as `include/slimcode.h` documents
//! them. The mapping is exhaustive by construction — no `_` arm over
//! [`SdkEvent`] or [`DisplayItem`] — so a new display variant is a compile
//! error here instead of a silently dropped event (spec user story 41: a new
//! `DisplayItem` variant must make a test notice at once that the wire
//! contract was not updated).
//!
//! Payloads reuse the SDK's own serde shapes where spec D3 pinned them:
//! `message` is the persisted session-log shape ([`AgentMessage`]), `usage` is
//! the derived [`TokenUsage`] serialization (so an endpoint that omitted
//! `prompt_tokens_details` yields `"prompt_tokens_details":null`, never a
//! normalized zero object), and the stop kinds carry the snake_case
//! [`StopReason`].

use serde::Serialize;
use slimcode_sdk::{AgentMessage, DisplayItem, SdkEvent, StopReason, TokenUsage};

/// One event as the flat JSON object `include/slimcode.h` documents.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireEvent {
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
    /// The authoritative end of one prompt.
    TurnFinished { stop_reason: StopReason },
    /// A compaction replaced part of the history.
    Compacted { before: usize, after: usize },
}

impl From<&SdkEvent> for WireEvent {
    fn from(event: &SdkEvent) -> Self {
        match event {
            SdkEvent::Display(item) => Self::from(item),
            SdkEvent::Message { message, usage } => WireEvent::Message {
                message: message.clone(),
                usage: *usage,
            },
            SdkEvent::TurnFinished { stop_reason } => WireEvent::TurnFinished {
                stop_reason: stop_reason.clone(),
            },
            SdkEvent::Compacted { before, after } => WireEvent::Compacted {
                before: *before,
                after: *after,
            },
        }
    }
}

impl From<&DisplayItem> for WireEvent {
    fn from(item: &DisplayItem) -> Self {
        // Exhaustive on purpose: a new `DisplayItem` variant must break this
        // build so the wire contract (and the header) gets updated with it.
        match item {
            DisplayItem::Turn { turn } => WireEvent::Turn { turn: *turn },
            DisplayItem::Reasoning(text) => WireEvent::Reasoning { text: text.clone() },
            DisplayItem::Text(text) => WireEvent::Text { text: text.clone() },
            DisplayItem::ToolStart {
                tool_call_id,
                name,
                arguments,
            } => WireEvent::ToolStart {
                tool_call_id: tool_call_id.clone(),
                name: name.clone(),
                arguments: arguments.clone(),
            },
            DisplayItem::ToolResult {
                tool_call_id,
                name,
                ok,
                result,
            } => WireEvent::ToolResult {
                tool_call_id: tool_call_id.clone(),
                name: name.clone(),
                ok: *ok,
                result: result.clone(),
            },
            DisplayItem::Stop(reason) => WireEvent::Stop {
                reason: reason.clone(),
            },
            DisplayItem::Usage(usage) => WireEvent::Usage { usage: *usage },
        }
    }
}

/// Encode one SDK event as the flat JSON string the handle's queue carries.
///
/// The `Result` keeps the queue's listener path panic-free; the flat shapes
/// have no serialization failure mode (no non-string map keys, no fallible
/// custom impls), so `Err` is unreachable in practice.
pub fn encode(event: &SdkEvent) -> Result<String, serde_json::Error> {
    serde_json::to_string(&WireEvent::from(event))
}
