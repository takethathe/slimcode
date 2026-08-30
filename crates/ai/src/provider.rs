//! `BailianProvider` — implements the agent `Provider` seam against the Bailian
//! (阿里云百炼) OpenAI-compatible endpoint.
//!
//! Uses `reqwest::blocking` to keep the sync `Provider` trait seam (ticket 04);
//! the async boundary is entirely inside this crate. Streaming is used with
//! `stream_options.include_usage=true` (ticket 05 verified usage arrives in the
//! final chunk with `choices: []`); each SSE chunk maps to agent deltas.

use std::time::Duration;

use crate::config::BailianConfig;
use crate::wire;
use crate::wire::TokenUsage;
use slimcode_agent::agent::{Delta, Provider, Tool};
use slimcode_agent::session::Message;

/// Sum a usage sample into an accumulator (pure, unit-testable).
fn accumulate_usage(total: &mut TokenUsage, sample: TokenUsage) {
    total.prompt_tokens += sample.prompt_tokens;
    total.completion_tokens += sample.completion_tokens;
    total.total_tokens += sample.total_tokens;
}

/// Provider for the Bailian compatible-mode endpoint.
pub struct BailianProvider {
    config: BailianConfig,
    client: reqwest::blocking::Client,
    /// Token usage from the most recent `chat` call (None before any call or
    /// when the endpoint omitted it).
    pub last_usage: Option<TokenUsage>,
    /// Cumulative token usage across every `chat` call since construction
    /// (drives the frontend's end-of-run totals).
    pub total_usage: TokenUsage,
}

impl BailianProvider {
    /// Build a provider from an explicit config.
    pub fn new(config: BailianConfig) -> Result<Self, String> {
        let client = reqwest::blocking::Client::builder()
            // A stalled connection must not hang the synchronous agent loop
            // indefinitely. Generous enough for long thinking-streams.
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(300))
            .build()
            .map_err(|e| format!("failed to build HTTP client: {e}"))?;
        Ok(Self {
            config,
            client,
            last_usage: None,
            total_usage: TokenUsage::default(),
        })
    }
}

impl Provider for BailianProvider {
    fn chat(&mut self, messages: &[Message], tools: &[Tool]) -> Result<Vec<Delta>, String> {
        let req = wire::WireRequest {
            model: &self.config.model,
            messages: messages.iter().map(wire::message_to_wire).collect(),
            tools: if tools.is_empty() {
                None
            } else {
                Some(tools.iter().map(wire::tool_to_wire).collect())
            },
            stream: true,
            stream_options: wire::WireStreamOptions {
                include_usage: true,
            },
        };

        let resp = self
            .client
            .post(self.config.chat_completions_url())
            .bearer_auth(&self.config.api_key)
            .json(&req)
            .send()
            .map_err(|e| format!("request failed: {e}"))?;

        let status = resp.status();
        let body = resp.text().map_err(|e| format!("read body failed: {e}"))?;
        if !status.is_success() {
            return Err(format!("Bailian API error {status}: {body}"));
        }
        let parsed = wire::parse_stream(&body)?;
        self.last_usage = parsed.usage;
        if let Some(u) = parsed.usage {
            accumulate_usage(&mut self.total_usage, u);
        }
        Ok(parsed.deltas)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slimcode_agent::agent::FinishReason;
    use slimcode_agent::session::Role;
    use slimcode_common::config::{
        DEFAULT_BASE_URL, DEFAULT_MODEL, ENV_API_KEY, ENV_BASE_URL, ENV_MODEL,
    };

    /// Test-only: build a provider from the live environment. The real
    /// resolution owner is `slimcode-common::config`; this helper keeps the
    /// live smoke tests working by referencing the shared constants instead of
    /// re-inlining them.
    fn provider_from_env() -> Result<BailianProvider, String> {
        let api_key =
            std::env::var(ENV_API_KEY).map_err(|_| format!("{ENV_API_KEY} is not set"))?;
        let base_url = std::env::var(ENV_BASE_URL).unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());
        let model = std::env::var(ENV_MODEL).unwrap_or_else(|_| DEFAULT_MODEL.to_string());
        BailianProvider::new(BailianConfig::new(api_key, base_url, model))
    }

    #[test]
    fn accumulate_usage_sums_across_calls() {
        let mut total = TokenUsage::default();
        accumulate_usage(
            &mut total,
            TokenUsage {
                prompt_tokens: 5,
                completion_tokens: 3,
                total_tokens: 8,
            },
        );
        accumulate_usage(
            &mut total,
            TokenUsage {
                prompt_tokens: 10,
                completion_tokens: 7,
                total_tokens: 17,
            },
        );
        assert_eq!(total.prompt_tokens, 15);
        assert_eq!(total.completion_tokens, 10);
        assert_eq!(total.total_tokens, 25);
    }

    /// Live smoke test — requires `DASHSCOPE_API_KEY` (and optionally
    /// `SLIMCODE_AI_BASE_URL` / `SLIMCODE_AI_MODEL`). Skipped by default.
    ///
    /// Test-only env lookup: the production config seam is
    /// `slimcode-common::config`, which this crate deliberately does not depend
    /// on, so the live test reads the same variables inline.
    #[test]
    #[ignore = "requires live DASHSCOPE_API_KEY and network access"]
    fn live_chat_returns_text_and_done() {
        let mut p = provider_from_env().expect("env config");
        let msgs = vec![Message::text(Role::User, "Reply with exactly: pong")];
        let deltas = p.chat(&msgs, &[]).expect("chat succeeds");
        assert!(
            deltas
                .iter()
                .any(|d| matches!(d, Delta::Text(t) if !t.is_empty())),
            "expected some text deltas, got: {deltas:?}"
        );
        assert!(
            deltas.iter().any(|d| matches!(d, Delta::Done(_))),
            "expected a Done marker"
        );
    }

    /// Live tool-call check — verifies request-side tool serialization against
    /// the real endpoint. Skipped by default.
    #[test]
    #[ignore = "requires live DASHSCOPE_API_KEY and network access"]
    fn live_chat_calls_tool() {
        let mut p = provider_from_env().expect("env config");
        let tool = Tool::new(
            "get_weather",
            "Get the current weather for a city",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "city": {"type": "string", "description": "City name"}
                },
                "required": ["city"]
            }),
            |args| Ok(format!("weather for {args}")),
        );
        let msgs = vec![Message::text(
            Role::User,
            "What is the weather in Beijing? Use the get_weather tool.",
        )];
        let deltas = p.chat(&msgs, &[tool]).expect("chat succeeds");
        assert!(
            deltas
                .iter()
                .any(|d| matches!(d, Delta::ToolCallStart { name, .. } if name == "get_weather")),
            "expected a get_weather tool call, got: {deltas:?}"
        );
        assert!(
            deltas
                .iter()
                .any(|d| matches!(d, Delta::Done(FinishReason::ToolCalls))),
            "expected Done(ToolCalls), got: {deltas:?}"
        );
    }
}
