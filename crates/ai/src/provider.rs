//! `BailianProvider` — implements the `slimcode-ai` `Provider` seam against the
//! Bailian (阿里云百炼) OpenAI-compatible endpoint.
//!
//! Uses `reqwest::blocking` to keep the sync `Provider` trait seam; the async
//! boundary is entirely inside this crate. Streaming is used with
//! `stream_options.include_usage=true` (ticket 05 verified usage arrives in the
//! final chunk with `choices: []`); each SSE chunk maps to provider deltas.

use std::io::Read;
use std::time::Duration;

use crate::config::BailianConfig;
use crate::llm::{CancelToken, Delta, Provider, ToolSpec};
use crate::message::Message;
use crate::wire;
use crate::wire::{PromptTokensDetails, TokenUsage};

/// Sum a usage sample into an accumulator (pure, unit-testable). Cache-hit
/// details (`prompt_tokens_details`) accumulate alongside the three headline
/// fields; a sample without details leaves existing totals untouched.
fn accumulate_usage(total: &mut TokenUsage, sample: TokenUsage) {
    total.prompt_tokens += sample.prompt_tokens;
    total.completion_tokens += sample.completion_tokens;
    total.total_tokens += sample.total_tokens;
    if let Some(details) = sample.prompt_tokens_details {
        let acc = total
            .prompt_tokens_details
            .get_or_insert_with(PromptTokensDetails::default);
        acc.cached_tokens += details.cached_tokens;
        acc.cache_creation_input_tokens += details.cache_creation_input_tokens;
    }
}

/// What an interruptible body read ended with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadOutcome {
    /// The reader hit EOF: the full body.
    Complete(Vec<u8>),
    /// The cancel token flipped while reading (an in-flight request aborted):
    /// carries the bytes received before the cancel so the provider can
    /// salvage whatever already streamed.
    Cancelled(Vec<u8>),
}

/// Read `reader` to EOF in chunks, checking `cancel` between chunks (ticket
/// 07): as soon as the flag is set the read stops and reports
/// [`ReadOutcome::Cancelled`], dropping the connection so the running turn
/// can end immediately. I/O errors propagate as `Err`; `Interrupted` retries.
/// A reader that stalls is only bounded by the caller's own timeout — a
/// flowing stream reacts within one socket chunk.
pub fn read_body_interruptibly<R: Read>(
    mut reader: R,
    cancel: &CancelToken,
) -> Result<ReadOutcome, String> {
    let mut body = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        if cancel.is_cancelled() {
            return Ok(ReadOutcome::Cancelled(body));
        }
        match reader.read(&mut chunk) {
            Ok(0) => return Ok(ReadOutcome::Complete(body)),
            Ok(n) => body.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(format!("read body failed: {e}")),
        }
    }
}

/// Trim `body` to the last complete SSE event boundary (ticket 07): a
/// cancelled read may cut a `data:` event mid-line; the torn tail cannot
/// parse, so it is dropped before salvaging the partial stream.
fn trim_partial_sse_tail(body: &str) -> &str {
    if body.is_empty() {
        return body;
    }
    if body.ends_with("\n\n") {
        return body;
    }
    match body.rfind("\n\n") {
        Some(pos) => &body[..pos + 2],
        None => "",
    }
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
    /// The provider's running totals: the CLI reads them for the footer stats
    /// line and the `/usage` notice (ADR-0004, ADR-0013).
    fn total_usage(&self) -> TokenUsage {
        self.total_usage
    }

    fn chat(
        &mut self,
        messages: &[Message],
        tools: &[ToolSpec],
        cancel: &CancelToken,
    ) -> Result<Vec<Delta>, String> {
        let req = wire::WireRequest {
            model: &self.config.model,
            messages: messages
                .iter()
                .map(|m| wire::message_to_wire(m, self.config.cache))
                .collect(),
            tools: if tools.is_empty() {
                None
            } else {
                Some(tools.iter().map(wire::tool_to_wire).collect())
            },
            // Parallel tool calls are on by default: the model may answer an
            // independent multi-tool request in one response. The flag rides
            // only when tools are declared (never on a plain-answer request).
            parallel_tool_calls: !tools.is_empty(),
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
        // Chunked body read (ticket 07): blocking reqwest exposes no per-read
        // timeout, so a silent server stays bounded by the client timeout
        // (300s); an Esc cancel aborts the read within one socket chunk on a
        // flowing stream.
        let body = match read_body_interruptibly(resp, cancel)? {
            ReadOutcome::Complete(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            ReadOutcome::Cancelled(partial) => {
                // The request was aborted mid-stream. Salvage whatever already
                // arrived: parse the partial body (dropping a torn final SSE
                // event) so the runner can surface the text that streamed in;
                // when nothing usable arrived, report the abort (the runner
                // maps it to a silent cancelled stop).
                let partial = String::from_utf8_lossy(&partial).into_owned();
                let trimmed = trim_partial_sse_tail(&partial);
                if let Ok(parsed) = wire::parse_stream(trimmed)
                    && !parsed.deltas.is_empty()
                {
                    return Ok(parsed.deltas);
                }
                return Err("request cancelled".to_string());
            }
        };
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
    use crate::llm::FinishReason;
    use crate::message::Role;
    use std::io::Cursor;

    /// A reader handing out at most `chunk_size` bytes per `read` call.
    struct ChunkedReader {
        data: Vec<u8>,
        pos: usize,
        chunk_size: usize,
        reads: usize,
    }

    impl ChunkedReader {
        fn new(data: &str, chunk_size: usize) -> Self {
            Self {
                data: data.as_bytes().to_vec(),
                pos: 0,
                chunk_size,
                reads: 0,
            }
        }
    }

    impl Read for ChunkedReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.reads += 1;
            let n = self.chunk_size.min(self.data.len() - self.pos);
            buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    /// A reader that cancels the token when its Nth `read` call starts,
    /// simulating Esc arriving while a chunk is in flight.
    struct CancelOnRead {
        inner: ChunkedReader,
        flip_on: usize,
        cancel: CancelToken,
    }

    impl Read for CancelOnRead {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.inner.reads += 1;
            if self.inner.reads == self.flip_on {
                self.cancel.cancel();
            }
            let n = self
                .inner
                .chunk_size
                .min(self.inner.data.len() - self.inner.pos);
            buf[..n].copy_from_slice(&self.inner.data[self.inner.pos..self.inner.pos + n]);
            self.inner.pos += n;
            Ok(n)
        }
    }

    // --- read_body_interruptibly ------------------------------------------

    #[test]
    fn read_body_reads_to_eof_and_completes() {
        let cancel = CancelToken::new();
        let outcome = read_body_interruptibly(Cursor::new(b"hello\nworld"), &cancel).unwrap();
        assert_eq!(outcome, ReadOutcome::Complete(b"hello\nworld".to_vec()));
    }

    #[test]
    fn read_body_completes_empty_reader() {
        let cancel = CancelToken::new();
        let outcome = read_body_interruptibly(Cursor::new(b""), &cancel).unwrap();
        assert_eq!(outcome, ReadOutcome::Complete(Vec::new()));
    }

    #[test]
    fn read_body_cancels_without_reading_when_flag_is_already_set() {
        let cancel = CancelToken::new();
        cancel.cancel();
        let reader = ChunkedReader::new("1234567890", 4);
        let outcome = read_body_interruptibly(reader, &cancel).unwrap();
        assert_eq!(outcome, ReadOutcome::Cancelled(Vec::new()));
    }

    #[test]
    fn read_body_stops_between_chunks_when_flag_flips() {
        let cancel = CancelToken::new();
        // Chunks of 4: the flag flips as the second read begins. The read in
        // flight may still deliver its chunk, but the loop stops before any
        // further read — the partial body is the data received so far.
        let reader = CancelOnRead {
            inner: ChunkedReader::new("abcdefghij", 4),
            flip_on: 2,
            cancel: cancel.clone(),
        };
        let outcome = read_body_interruptibly(reader, &cancel).unwrap();
        assert_eq!(outcome, ReadOutcome::Cancelled(b"abcdefgh".to_vec()));
    }

    #[test]
    fn read_body_propagates_io_errors() {
        struct Boom;
        impl Read for Boom {
            fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("socket gone"))
            }
        }
        let cancel = CancelToken::new();
        let err = read_body_interruptibly(Boom, &cancel).unwrap_err();
        assert!(err.contains("socket gone"), "err: {err}");
    }

    // --- trim_partial_sse_tail + salvage -----------------------------------

    /// A two-event SSE body (each event a reasoning/content chunk).
    const SSE: &str = concat!(
        "data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"content\":\"first \"}}]}\n\n",
        "data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"content\":\"second\"}}]}\n\n",
    );

    #[test]
    fn trim_keeps_a_body_ending_on_an_event_boundary() {
        assert_eq!(trim_partial_sse_tail(SSE), SSE);
        assert_eq!(trim_partial_sse_tail(""), "");
    }

    #[test]
    fn trim_drops_a_torn_final_event() {
        // Cut mid-second-event: only the first complete event survives.
        let torn = format!("{}data: {{chunk", SSE);
        assert_eq!(trim_partial_sse_tail(&torn), SSE);
        // Only a torn event (no complete event yet): nothing survives.
        assert_eq!(trim_partial_sse_tail("data: {partial"), "");
    }

    #[test]
    fn partial_stream_salvages_deltas_received_before_the_cancel() {
        // A single read delivers the whole (event-complete) stream while Esc
        // lands: the read stops before any further chunk and the salvage
        // parse keeps every event that already arrived.
        let cancel = CancelToken::new();
        let reader = CancelOnRead {
            inner: ChunkedReader::new(SSE, SSE.len()),
            flip_on: 1,
            cancel: cancel.clone(),
        };
        let ReadOutcome::Cancelled(partial) = read_body_interruptibly(reader, &cancel).unwrap()
        else {
            panic!("expected a cancel");
        };
        let partial = String::from_utf8_lossy(&partial).into_owned();
        let trimmed = trim_partial_sse_tail(&partial);
        let parsed = wire::parse_stream(trimmed).unwrap();
        let text: String = parsed
            .deltas
            .iter()
            .filter_map(|d| match d {
                Delta::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "first second");
    }

    // --- usage accumulation + live smoke (existing suite) ------------------

    /// Test-only: build a provider from the live environment. The real
    /// resolution owner is `slimcode-app::config`; this helper keeps the live
    /// smoke tests working by reading the same env var names and the AI
    /// crate's own endpoint defaults (this crate depends on no slimcode crate).
    fn provider_from_env() -> Result<BailianProvider, String> {
        const ENV_API_KEY: &str = "DASHSCOPE_API_KEY";
        const ENV_BASE_URL: &str = "SLIMCODE_AI_BASE_URL";
        const ENV_MODEL: &str = "SLIMCODE_AI_MODEL";
        let api_key =
            std::env::var(ENV_API_KEY).map_err(|_| format!("{ENV_API_KEY} is not set"))?;
        let base_url = std::env::var(ENV_BASE_URL)
            .unwrap_or_else(|_| crate::config::DEFAULT_BASE_URL.to_string());
        let model =
            std::env::var(ENV_MODEL).unwrap_or_else(|_| crate::config::DEFAULT_MODEL.to_string());
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
                ..Default::default()
            },
        );
        accumulate_usage(
            &mut total,
            TokenUsage {
                prompt_tokens: 10,
                completion_tokens: 7,
                total_tokens: 17,
                ..Default::default()
            },
        );
        assert_eq!(total.prompt_tokens, 15);
        assert_eq!(total.completion_tokens, 10);
        assert_eq!(total.total_tokens, 25);
    }

    #[test]
    fn accumulate_usage_sums_cache_details_across_calls() {
        let mut total = TokenUsage::default();
        accumulate_usage(
            &mut total,
            TokenUsage {
                prompt_tokens: 100,
                completion_tokens: 10,
                total_tokens: 110,
                prompt_tokens_details: Some(PromptTokensDetails {
                    cached_tokens: 60,
                    cache_creation_input_tokens: 40,
                }),
            },
        );
        // A sample without details must not clobber the accumulated counts.
        accumulate_usage(
            &mut total,
            TokenUsage {
                prompt_tokens: 200,
                completion_tokens: 20,
                total_tokens: 220,
                ..Default::default()
            },
        );
        accumulate_usage(
            &mut total,
            TokenUsage {
                prompt_tokens: 300,
                completion_tokens: 30,
                total_tokens: 330,
                prompt_tokens_details: Some(PromptTokensDetails {
                    cached_tokens: 90,
                    cache_creation_input_tokens: 10,
                }),
            },
        );
        assert_eq!(total.prompt_tokens, 600);
        assert_eq!(total.completion_tokens, 60);
        assert_eq!(total.total_tokens, 660);
        assert_eq!(total.cached_tokens(), 150);
        assert_eq!(total.cache_creation_tokens(), 50);
    }

    /// Live smoke test — requires `DASHSCOPE_API_KEY` (and optionally
    /// `SLIMCODE_AI_BASE_URL` / `SLIMCODE_AI_MODEL`). Skipped by default.
    ///
    /// Test-only env lookup: this crate deliberately depends on no slimcode
    /// crate, so the live test reads the same variables inline.
    #[test]
    #[ignore = "requires live DASHSCOPE_API_KEY and network access"]
    fn live_chat_returns_text_and_done() {
        let mut p = provider_from_env().expect("env config");
        let msgs = vec![Message::text(Role::User, "Reply with exactly: pong")];
        let deltas = p
            .chat(&msgs, &[], &CancelToken::new())
            .expect("chat succeeds");
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
        let tool = ToolSpec::new(
            "get_weather",
            "Get the current weather for a city",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "city": {"type": "string", "description": "City name"}
                },
                "required": ["city"]
            }),
        );
        let msgs = vec![Message::text(
            Role::User,
            "What is the weather in Beijing? Use the get_weather tool.",
        )];
        let deltas = p
            .chat(&msgs, &[tool], &CancelToken::new())
            .expect("chat succeeds");
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
