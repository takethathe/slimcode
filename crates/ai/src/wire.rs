//! Wire model for the Bailian OpenAI-compatible endpoint, plus the mapping
//! between the agent's message model and the wire, and SSE parsing.
//!
//! Serde tolerance checklist (ticket 05 spike):
//! - models here do NOT use `deny_unknown_fields`, so unknown fields are
//!   automatically ignored (`reasoning_content`, `prompt_tokens_details`,
//!   `completion_tokens_details`, etc.);
//! - `usage` is `Option` because the `usage` key is present on every streaming
//!   chunk but `null` until the final one;
//! - `content` / `name` / `id` are `Option` because thinking-model and tool-call
//!   continuation chunks carry `""` / `null` there.

use serde::{Deserialize, Serialize};

use crate::llm::{Delta, FinishReason, ToolSpec};
use crate::message::{Message, Role};

// ---------------------------------------------------------------------------
// Response (streaming chunks)
// ---------------------------------------------------------------------------

#[derive(Deserialize, Debug, Default)]
pub struct WireChunk {
    #[serde(default)]
    pub choices: Vec<WireChoice>,
    #[serde(default)]
    pub usage: Option<TokenUsage>,
}

#[derive(Deserialize, Debug, Default)]
pub struct WireChoice {
    #[serde(default)]
    pub index: usize,
    #[serde(default)]
    pub delta: Option<WireDelta>,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
pub struct WireDelta {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub reasoning_content: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<WireToolCallDelta>,
}

#[derive(Deserialize, Debug, Default)]
pub struct WireToolCallDelta {
    #[serde(default)]
    pub index: usize,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub r#type: Option<String>,
    #[serde(default)]
    pub function: Option<WireFunctionDelta>,
}

#[derive(Deserialize, Debug, Default)]
pub struct WireFunctionDelta {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
}

/// Token usage reported by the endpoint (streamed with `include_usage=true`).
#[derive(Deserialize, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TokenUsage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
    /// Cache-hit accounting (`usage.prompt_tokens_details`). Absent when the
    /// endpoint omits it (cache off, unsupported model, no hit); accessors
    /// treat absence as 0 so callers never need to unwrap.
    #[serde(default)]
    pub prompt_tokens_details: Option<PromptTokensDetails>,
}

/// Cache-hit breakdown inside `usage.prompt_tokens_details`. Both fields are
/// optional on the wire; a missing field counts as 0.
#[derive(Deserialize, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PromptTokensDetails {
    /// Prompt tokens served from the cache (cache hits).
    #[serde(default)]
    pub cached_tokens: u64,
    /// Prompt tokens spent creating the cache entry.
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
}

impl TokenUsage {
    /// Cached (cache-hit) prompt tokens; `0` when the endpoint omitted the
    /// details (cache off, unsupported model, or no hit).
    pub fn cached_tokens(&self) -> u64 {
        self.prompt_tokens_details.map_or(0, |d| d.cached_tokens)
    }

    /// Prompt tokens spent creating the cache; `0` when details are absent.
    pub fn cache_creation_tokens(&self) -> u64 {
        self.prompt_tokens_details
            .map_or(0, |d| d.cache_creation_input_tokens)
    }
}

// ---------------------------------------------------------------------------
// Request
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct WireRequest<'a> {
    pub model: &'a str,
    pub messages: Vec<WireMessage<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<WireTool>>,
    /// Opt into several independent `tool_calls` in one response. Only
    /// meaningful — and only serialized — when `tools` is non-empty, so a
    /// plain-answer request keeps its pre-parallel byte shape.
    #[serde(skip_serializing_if = "is_false")]
    pub parallel_tool_calls: bool,
    pub stream: bool,
    pub stream_options: WireStreamOptions,
}

/// `skip_serializing_if` predicate: omit a `false` flag from the request.
fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Serialize)]
pub struct WireStreamOptions {
    pub include_usage: bool,
}

/// The `content` of a wire message: either a plain string (the pre-cache wire
/// shape, byte-identical) or a list of content blocks. `serde(untagged)` makes
/// the same field serialize as a JSON string or a JSON array depending on the
/// variant. Owned strings keep `message_to_wire` free of borrow gymnastics.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(untagged)]
pub enum WireContent {
    /// Plain string content (unchanged wire shape).
    Text(String),
    /// One text block carrying `cache_control` (explicit cache marker). Only
    /// emitted for the system message when caching is on.
    Blocks(Vec<WireContentBlock>),
}

/// A text content block. `cache_control` marks the block as a cache-able
/// prefix for Bailian's explicit context caching.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct WireContentBlock {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub text: String,
    pub cache_control: WireCacheControl,
}

/// The explicit-cache marker (`{"type": "ephemeral"}`).
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct WireCacheControl {
    #[serde(rename = "type")]
    pub kind: &'static str,
}

#[derive(Serialize)]
pub struct WireMessage<'a> {
    pub role: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<WireContent>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<WireToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<&'a str>,
}

#[derive(Serialize)]
pub struct WireToolCall {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub id: String,
    pub function: WireFunction,
}

#[derive(Serialize)]
pub struct WireFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Serialize)]
pub struct WireTool {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: WireToolFunction,
}

#[derive(Serialize)]
pub struct WireToolFunction {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Mapping: agent model <-> wire
// ---------------------------------------------------------------------------

pub fn role_to_wire(role: &Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

/// Map one agent message to the wire shape. With `cache` on, the system
/// message's content is serialized as a one-block array carrying
/// `cache_control` so the stable "system prompt + tool definitions" prefix is
/// cached by the endpoint; every other message keeps the plain-string shape.
/// With `cache` off the bytes are identical to the pre-cache client.
pub fn message_to_wire(m: &Message, cache: bool) -> WireMessage<'_> {
    let role = role_to_wire(&m.role);
    let text = m.text_content();
    let has_tool_calls = !m.tool_calls.is_empty();
    let content = if role == "assistant" && has_tool_calls {
        // Tool-call assistant messages carry an empty content on the wire.
        Some(WireContent::Text(String::new()))
    } else if role == "tool" {
        // Tool messages must carry content on the wire; an empty result still
        // sends an empty string rather than omitting the field (400 risk).
        Some(WireContent::Text(text))
    } else if text.is_empty() {
        None
    } else if cache && role == "system" {
        // Explicit cache marker on the system prefix (only when enabled and
        // the text is non-empty).
        Some(WireContent::Blocks(vec![WireContentBlock {
            kind: "text",
            text,
            cache_control: WireCacheControl { kind: "ephemeral" },
        }]))
    } else {
        Some(WireContent::Text(text))
    };
    let tool_calls = m
        .tool_calls
        .iter()
        .map(|tc| WireToolCall {
            kind: "function",
            id: tc.id.clone(),
            function: WireFunction {
                name: tc.name.clone(),
                arguments: tc.arguments.clone(),
            },
        })
        .collect();
    WireMessage {
        role,
        content,
        tool_calls,
        tool_call_id: m.tool_call_id.as_deref(),
    }
}

pub fn tool_to_wire(t: &ToolSpec) -> WireTool {
    WireTool {
        kind: "function",
        function: WireToolFunction {
            name: t.name.clone(),
            description: t.description.clone(),
            parameters: t.parameters.clone(),
        },
    }
}

// ---------------------------------------------------------------------------
// SSE parsing + chunk -> Delta
// ---------------------------------------------------------------------------

/// Split an SSE body into its `data:` event payloads (joined across multi-line
/// `data:` sequences). `[DONE]` markers are included verbatim.
pub fn parse_sse_events(body: &str) -> Vec<String> {
    let mut events = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    for line in body.lines() {
        if let Some(data) = line.strip_prefix("data:") {
            cur.push(data.trim_start().to_string());
        } else if line.trim().is_empty() && !cur.is_empty() {
            events.push(cur.join("\n"));
            cur.clear();
        }
    }
    if !cur.is_empty() {
        events.push(cur.join("\n"));
    }
    events
}

/// Map one wire chunk's choices to agent deltas. Empty strings and nulls are
/// skipped; `finish_reason` maps to `Done`.
pub fn chunk_to_deltas(chunk: &WireChunk) -> Vec<Delta> {
    let mut out = Vec::new();
    for ch in &chunk.choices {
        if let Some(d) = &ch.delta {
            if let Some(rc) = &d.reasoning_content
                && !rc.is_empty()
            {
                out.push(Delta::Reasoning(rc.clone()));
            }
            if let Some(c) = &d.content
                && !c.is_empty()
            {
                out.push(Delta::Text(c.clone()));
            }
            for tc in &d.tool_calls {
                let f = tc.function.as_ref();
                let name = f.and_then(|f| f.name.clone());
                let args = f.and_then(|f| f.arguments.clone()).unwrap_or_default();
                if let Some(n) = name {
                    out.push(Delta::ToolCallStart {
                        index: tc.index,
                        id: tc.id.clone().unwrap_or_default(),
                        name: n,
                    });
                }
                if !args.is_empty() {
                    out.push(Delta::ToolCallArgs {
                        index: tc.index,
                        fragment: args,
                    });
                }
            }
        }
        if let Some(fr) = &ch.finish_reason {
            let finish = if fr == "tool_calls" {
                FinishReason::ToolCalls
            } else {
                FinishReason::Stop
            };
            out.push(Delta::Done(finish));
        }
    }
    out
}

/// Result of parsing a full SSE stream: the ordered deltas plus the token
/// usage carried by the final chunk (present when `include_usage` was set).
pub struct ParsedStream {
    pub deltas: Vec<Delta>,
    pub usage: Option<TokenUsage>,
}

/// Parse a full SSE stream body into an ordered list of agent deltas.
pub fn parse_stream(body: &str) -> Result<ParsedStream, String> {
    let mut deltas = Vec::new();
    let mut usage = None;
    for ev in parse_sse_events(body) {
        if ev == "[DONE]" {
            continue;
        }
        let chunk: WireChunk =
            serde_json::from_str(&ev).map_err(|e| format!("bad chunk: {e}: {ev}"))?;
        if chunk.usage.is_some() {
            usage = chunk.usage;
        }
        deltas.extend(chunk_to_deltas(&chunk));
    }
    Ok(ParsedStream { deltas, usage })
}

// ---------------------------------------------------------------------------
// Tests (fixtures mirror the live Bailian wire shapes captured in ticket 05)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{MessageStopReason, Role, ToolCall};

    const TEXT_STREAM: &str = concat!(
        "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",",
        "\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"reasoning_content\":\"Let me think...\",\"content\":\"\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",",
        "\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"\",\"content\":\"Beijing is \"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",",
        "\"choices\":[{\"index\":0,\"delta\":{\"content\":\"25C.\"},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );

    const TOOL_STREAM: &str = concat!(
        "data: {\"id\":\"c2\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",",
        "\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\",\"reasoning_content\":\"\",",
        "\"tool_calls\":[{\"id\":\"call_1\",\"type\":\"function\",\"index\":0,",
        "\"function\":{\"name\":\"get_weather\",\"arguments\":\"\"}}]},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"c2\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",",
        "\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,",
        "\"function\":{\"name\":null,\"arguments\":\"{\\\"city\\\": \\\"Bei\"}}]},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"c2\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",",
        "\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,",
        "\"function\":{\"name\":null,\"arguments\":\"jing\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n",
    );

    #[test]
    fn parse_sse_events_splits_and_keeps_done() {
        let evs = parse_sse_events(TEXT_STREAM);
        assert_eq!(evs.len(), 4);
        assert_eq!(evs[3], "[DONE]");
    }

    #[test]
    fn parse_stream_orders_reasoning_then_text_then_done() {
        let deltas = parse_stream(TEXT_STREAM).unwrap().deltas;
        let kinds: Vec<&str> = deltas
            .iter()
            .map(|d| match d {
                Delta::Reasoning(_) => "r",
                Delta::Text(_) => "t",
                Delta::Done(_) => "d",
                _ => "?",
            })
            .collect();
        assert_eq!(kinds, vec!["r", "t", "t", "d"]);
        match &deltas[0] {
            Delta::Reasoning(s) => assert_eq!(s, "Let me think..."),
            _ => panic!("first delta should be reasoning"),
        }
        let text: String = deltas
            .iter()
            .filter_map(|d| match d {
                Delta::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "Beijing is 25C.");
        assert!(matches!(
            deltas.last(),
            Some(Delta::Done(FinishReason::Stop))
        ));
    }

    #[test]
    fn parse_stream_assembles_tool_call_fragments() {
        let deltas = parse_stream(TOOL_STREAM).unwrap().deltas;
        let mut start: Option<(usize, &str, &str)> = None;
        let mut args = String::new();
        let mut done = false;
        for d in &deltas {
            match d {
                Delta::ToolCallStart { index, id, name } => {
                    start = Some((*index, id, name));
                }
                Delta::ToolCallArgs { fragment, .. } => args.push_str(fragment),
                Delta::Done(FinishReason::ToolCalls) => done = true,
                _ => {}
            }
        }
        let (idx, id, name) = start.expect("tool_call start present");
        assert_eq!(idx, 0);
        assert_eq!(id, "call_1");
        assert_eq!(name, "get_weather");
        assert_eq!(args, "{\"city\": \"Beijing\"}");
        assert!(done, "tool_calls finish reason surfaced as Done(ToolCalls)");
    }

    #[test]
    fn parse_stream_tolerates_unknown_fields_and_null_usage() {
        // usage key present but null; reasoning_content unknown-but-expected
        let body = concat!(
            "data: {\"id\":\"c\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",",
            "\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}],\"usage\":null}\n\n",
            "data: [DONE]\n\n",
        );
        let deltas = parse_stream(body).unwrap().deltas;
        assert_eq!(deltas.len(), 1);
        assert!(matches!(deltas[0], Delta::Text(ref t) if t == "hi"));
    }

    #[test]
    fn parse_stream_ignores_final_usage_chunk_with_empty_choices() {
        let body = concat!(
            "data: {\"id\":\"c\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",",
            "\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"id\":\"c\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",",
            "\"choices\":[],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":3,\"total_tokens\":8}}\n\n",
            "data: [DONE]\n\n",
        );
        let deltas = parse_stream(body).unwrap().deltas;
        assert_eq!(deltas.len(), 2); // text + done; usage chunk contributes nothing
        assert!(matches!(
            deltas.last(),
            Some(Delta::Done(FinishReason::Stop))
        ));
    }

    #[test]
    fn parse_stream_captures_final_usage() {
        // `include_usage` puts prompt/completion/total tokens in the final
        // chunk; parse_stream must surface them alongside the deltas.
        let body = concat!(
            "data: {\"id\":\"c\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",",
            "\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"id\":\"c\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",",
            "\"choices\":[],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":3,\"total_tokens\":8}}\n\n",
            "data: [DONE]\n\n",
        );
        let parsed = parse_stream(body).unwrap();
        let usage = parsed.usage.expect("usage captured from final chunk");
        assert_eq!(usage.prompt_tokens, 5);
        assert_eq!(usage.completion_tokens, 3);
        assert_eq!(usage.total_tokens, 8);
    }

    #[test]
    fn parse_stream_captures_cache_usage_details() {
        // The final chunk carries `usage.prompt_tokens_details` with the
        // cache-hit and cache-creation token counts; parse_stream must
        // surface them and the accessors must return them.
        let body = concat!(
            "data: {\"id\":\"c\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",",
            "\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"id\":\"c\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",",
            "\"choices\":[],\"usage\":{\"prompt_tokens\":3019,\"completion_tokens\":104,\"total_tokens\":3123,",
            "\"prompt_tokens_details\":{\"cached_tokens\":2048,\"cache_creation_input_tokens\":1605}}}\n\n",
            "data: [DONE]\n\n",
        );
        let parsed = parse_stream(body).unwrap();
        let usage = parsed.usage.expect("usage captured from final chunk");
        assert_eq!(usage.prompt_tokens, 3019);
        assert_eq!(usage.completion_tokens, 104);
        assert_eq!(usage.total_tokens, 3123);
        let details = usage.prompt_tokens_details.expect("details parsed");
        assert_eq!(details.cached_tokens, 2048);
        assert_eq!(details.cache_creation_input_tokens, 1605);
        assert_eq!(usage.cached_tokens(), 2048);
        assert_eq!(usage.cache_creation_tokens(), 1605);
    }

    #[test]
    fn cache_accessors_return_zero_when_details_absent() {
        // No `prompt_tokens_details` in the payload (cache off / unsupported
        // model): the accessors must return 0, never panic.
        let body = concat!(
            "data: {\"id\":\"c\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",",
            "\"choices\":[],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":3,\"total_tokens\":8}}\n\n",
            "data: [DONE]\n\n",
        );
        let parsed = parse_stream(body).unwrap();
        let usage = parsed.usage.expect("usage captured");
        assert!(usage.prompt_tokens_details.is_none());
        assert_eq!(usage.cached_tokens(), 0);
        assert_eq!(usage.cache_creation_tokens(), 0);
    }

    #[test]
    fn cache_accessors_return_zero_for_default_usage() {
        // A zeroed TokenUsage (e.g. the provider's initial total) reports no
        // cache activity.
        let usage = TokenUsage::default();
        assert_eq!(usage.cached_tokens(), 0);
        assert_eq!(usage.cache_creation_tokens(), 0);
    }

    #[test]
    fn parse_stream_null_usage_yields_none() {
        // `usage:null` on a chunk must not fabricate a zeroed usage.
        let body = concat!(
            "data: {\"id\":\"c\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",",
            "\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}],\"usage\":null}\n\n",
            "data: [DONE]\n\n",
        );
        let parsed = parse_stream(body).unwrap();
        assert!(parsed.usage.is_none());
    }

    #[test]
    fn message_to_wire_text_messages() {
        let m = Message::text(Role::User, "hello");
        let w = message_to_wire(&m, false);
        assert_eq!(w.role, "user");
        assert_eq!(w.content, Some(WireContent::Text("hello".to_string())));
        assert!(w.tool_calls.is_empty());
        assert!(w.tool_call_id.is_none());
    }

    #[test]
    fn message_to_wire_assistant_tool_calls() {
        let mut m = Message::text(Role::Assistant, "");
        m.tool_calls = vec![ToolCall {
            id: "call_1".to_string(),
            name: "get_weather".to_string(),
            arguments: "{\"city\":\"Beijing\"}".to_string(),
        }];
        let w = message_to_wire(&m, true);
        assert_eq!(w.role, "assistant");
        assert_eq!(w.content, Some(WireContent::Text(String::new()))); // empty content, not omitted
        assert_eq!(w.tool_calls.len(), 1);
        assert_eq!(w.tool_calls[0].id, "call_1");
        assert_eq!(w.tool_calls[0].function.name, "get_weather");
        assert_eq!(w.tool_calls[0].function.arguments, "{\"city\":\"Beijing\"}");
    }

    #[test]
    fn message_to_wire_tool_role() {
        let m = Message::tool_result("call_1", "{\"temp\":\"25C\"}");
        let w = message_to_wire(&m, false);
        assert_eq!(w.role, "tool");
        assert_eq!(
            w.content,
            Some(WireContent::Text("{\"temp\":\"25C\"}".to_string()))
        );
        assert_eq!(w.tool_call_id, Some("call_1"));
    }

    #[test]
    fn message_to_wire_ignores_log_only_fields() {
        // `stop_reason` and `error` are log-schema fields (ADR-0009 D5): they
        // must never leak onto the provider wire. The failure-closing assistant
        // message still goes out as plain assistant text.
        let mut m = Message::text(Role::Assistant, "The turn ended with an error: boom");
        m.stop_reason = Some(MessageStopReason::Error);
        m.error = Some("boom".to_string());
        let w = message_to_wire(&m, false);
        assert_eq!(w.role, "assistant");
        assert_eq!(
            w.content,
            Some(WireContent::Text(
                "The turn ended with an error: boom".to_string()
            ))
        );
        let json = serde_json::to_value(&w).unwrap();
        assert!(json.get("stop_reason").is_none());
        assert!(json.get("error").is_none());
    }

    #[test]
    fn tool_to_wire_shapes_function() {
        let t = ToolSpec::new(
            "get_weather",
            "Get current weather for a city",
            serde_json::json!({"type": "object"}),
        );
        let w = tool_to_wire(&t);
        assert_eq!(w.kind, "function");
        assert_eq!(w.function.name, "get_weather");
        assert_eq!(w.function.description, "Get current weather for a city");
        assert_eq!(w.function.parameters["type"], "object");
    }

    #[test]
    fn parse_stream_tolerates_null_delta_finish_chunk() {
        // Some endpoints send the finish_reason on a chunk whose `delta` is
        // explicitly null rather than `{}`; that must still produce Done.
        let body = concat!(
            "data: {\"id\":\"c\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",",
            "\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"c\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",",
            "\"choices\":[{\"index\":0,\"delta\":null,\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        let deltas = parse_stream(body).unwrap().deltas;
        assert_eq!(deltas.len(), 2); // text + done (null delta contributes nothing)
        assert!(matches!(
            deltas.last(),
            Some(Delta::Done(FinishReason::Stop))
        ));
    }

    #[test]
    fn parse_stream_keeps_arguments_when_name_and_args_share_chunk() {
        // If a tool-call fragment carries both name and non-empty arguments
        // (some endpoints), both the start and the args must be emitted.
        let body = concat!(
            "data: {\"id\":\"c\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",",
            "\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"id\":\"call_1\",\"type\":\"function\",\"index\":0,",
            "\"function\":{\"name\":\"get_weather\",\"arguments\":\"{}\"}}]},\"finish_reason\":null}]}\n\n",
            "data: [DONE]\n\n",
        );
        let deltas = parse_stream(body).unwrap().deltas;
        let has_start = deltas
            .iter()
            .any(|d| matches!(d, Delta::ToolCallStart { name, .. } if name == "get_weather"));
        let args: String = deltas
            .iter()
            .filter_map(|d| match d {
                Delta::ToolCallArgs { fragment, .. } => Some(fragment.as_str()),
                _ => None,
            })
            .collect();
        assert!(has_start, "start delta present");
        assert_eq!(args, "{}");
    }

    #[test]
    fn message_to_wire_tool_role_always_has_content() {
        // An empty tool result must still send content (empty string) rather
        // than omit it — OpenAI-compatible endpoints require it on tool
        // messages and may reject otherwise.
        let m = Message::tool_result("call_1", "");
        let w = message_to_wire(&m, false);
        assert_eq!(w.role, "tool");
        assert_eq!(w.content, Some(WireContent::Text(String::new())));
    }

    // --- explicit cache marker (llm-cache ticket 03) ----------------------

    #[test]
    fn message_to_wire_system_with_cache_emits_block_array() {
        // cache=true + system message → content serializes as a one-block
        // array carrying `type=text`, the verbatim text and
        // `cache_control.type=ephemeral`.
        let m = Message::text(Role::System, "You are slimcode.");
        let w = message_to_wire(&m, true);
        let json = serde_json::to_string(&w).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let blocks = v["content"].as_array().expect("content is an array");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[0]["text"], "You are slimcode.");
        assert_eq!(blocks[0]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn message_to_wire_system_without_cache_stays_plain_string() {
        // cache=false + system message → byte-identical plain string.
        let m = Message::text(Role::System, "You are slimcode.");
        let w = message_to_wire(&m, false);
        assert_eq!(
            w.content,
            Some(WireContent::Text("You are slimcode.".to_string()))
        );
        let json = serde_json::to_string(&w).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["content"], "You are slimcode.");
    }

    #[test]
    fn message_to_wire_non_system_ignores_cache_flag() {
        // cache=true only ever touches the system message; user messages stay
        // plain strings.
        let m = Message::text(Role::User, "hello");
        let w = message_to_wire(&m, true);
        assert_eq!(w.content, Some(WireContent::Text("hello".to_string())));
        let json = serde_json::to_string(&w).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["content"], "hello");
    }

    #[test]
    fn message_to_wire_empty_system_with_cache_omits_content() {
        // An empty system message must not become a cache block; it keeps the
        // existing omit-content behavior.
        let m = Message::text(Role::System, "");
        let w = message_to_wire(&m, true);
        assert_eq!(w.content, None);
    }

    #[test]
    fn message_to_wire_cache_off_keeps_byte_shape_of_other_roles() {
        // Regression: assistant tool-call empty content, tool messages always
        // carry content, and empty text omission all hold with cache on.
        let m = Message::tool_result("call_1", "");
        let w = message_to_wire(&m, true);
        assert_eq!(w.content, Some(WireContent::Text(String::new())));
        let m = Message::text(Role::User, "");
        let w = message_to_wire(&m, true);
        assert_eq!(w.content, None);
    }

    // --- parallel tool calls (parallel-tool-calls ticket 02) --------------

    /// Build a request with the given tools and parallel flag for the
    /// serialization tests.
    fn request_with_tools<'a>(
        tools: Option<Vec<WireTool>>,
        parallel_tool_calls: bool,
    ) -> WireRequest<'a> {
        WireRequest {
            model: "m",
            messages: Vec::new(),
            tools,
            parallel_tool_calls,
            stream: true,
            stream_options: WireStreamOptions {
                include_usage: true,
            },
        }
    }

    #[test]
    fn request_serializes_parallel_tool_calls_when_tools_are_present() {
        // With tools declared, the request opts into parallel tool calls so
        // the model may return several independent calls in one response.
        let tool = ToolSpec::new(
            "get_weather",
            "Get current weather for a city",
            serde_json::json!({"type": "object"}),
        );
        let req = request_with_tools(Some(vec![tool_to_wire(&tool)]), true);
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["parallel_tool_calls"], true);
        assert!(v.get("tools").is_some());
    }

    #[test]
    fn request_omits_parallel_tool_calls_without_tools() {
        // No tools → the flag is omitted entirely: a plain-answer request
        // keeps the exact byte shape it had before parallel support.
        let req = request_with_tools(None, false);
        let v = serde_json::to_value(&req).unwrap();
        assert!(v.get("parallel_tool_calls").is_none());
        assert!(v.get("tools").is_none());
    }
}
