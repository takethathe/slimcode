//! `BailianProvider` — implements the `slimcode-ai` `Provider` seam against the
//! Bailian OpenAI-compatible endpoint.
//!
//! Uses `reqwest::blocking` to keep the sync `Provider` trait seam; the async
//! boundary is entirely inside this crate. Streaming is used with
//! `stream_options.include_usage=true` (ticket 05 verified usage arrives in the
//! final chunk with `choices: []`); each SSE chunk maps to provider deltas.
//!
//! The request is read and framed **incrementally** (ADR-0019): every complete
//! SSE event is parsed the moment it reaches the socket and its deltas are
//! pushed straight into the `on_delta` sink, so the frontend renders an answer
//! while the endpoint is still generating it. Nothing accumulates the whole
//! body first — the reader is interruptible between chunks, and a cancelled
//! read simply drops the torn tail (whose deltas never counted).
//!
//! The provider instance is **stateless** (ADR-0016): construction only builds
//! the HTTP client, and every `chat` call reads its settings (model, base URL,
//! API key, cache flag) from the `&ProviderConfig` argument — so one instance
//! is reusable with different configs (tests, future config switching).

use std::io::Read;
use std::time::Duration;

use crate::config::ProviderConfig;
use crate::llm::{CancelToken, Delta, Provider, ToolSpec};
use crate::message::Message;
use crate::wire;
use crate::wire::{PromptTokensDetails, SseFramer, TokenUsage};

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

/// How an interruptible body read ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadEnd {
    /// The reader hit EOF: the body ended on its own terms.
    Complete,
    /// The cancel token flipped mid-read: the read stopped between chunks.
    Cancelled,
}

/// Read `reader` chunk by chunk, handing each chunk to `on_chunk` as it
/// arrives, and stop as soon as `cancel` is set (ADR-0019): an in-flight
/// request reacts within one socket chunk, and dropping the reader here drops
/// the connection so the running turn ends immediately. A `on_chunk` error
/// (a sink/renderer failure) aborts the read the same way and propagates
/// verbatim; I/O errors propagate as `Err`; `Interrupted` retries. A reader
/// that stalls is only bounded by the caller's own timeout — blocking reqwest
/// exposes no per-read timeout.
pub fn read_stream_interruptibly<R: Read>(
    mut reader: R,
    cancel: &CancelToken,
    on_chunk: &mut dyn FnMut(&[u8]) -> Result<(), String>,
) -> Result<ReadEnd, String> {
    let mut chunk = [0u8; 8192];
    loop {
        if cancel.is_cancelled() {
            return Ok(ReadEnd::Cancelled);
        }
        match reader.read(&mut chunk) {
            Ok(0) => return Ok(ReadEnd::Complete),
            Ok(n) => on_chunk(&chunk[..n])?,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(format!("read body failed: {e}")),
        }
    }
}

/// Feed one SSE event's deltas into the sink, recording the usage sample when
/// the event carries one (only the final chunk does). Shared by the live read
/// and the EOF flush so a single event is interpreted exactly one way
/// regardless of which chunk boundary it landed on.
fn emit_event(
    event: &str,
    usage: &mut Option<TokenUsage>,
    on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
) -> Result<(), String> {
    let Some(parsed) = wire::parse_event(event)? else {
        return Ok(());
    };
    if parsed.usage.is_some() {
        *usage = parsed.usage;
    }
    for delta in parsed.deltas {
        on_delta(delta)?;
    }
    Ok(())
}

/// Provider for the Bailian compatible-mode endpoint. Stateless: construction
/// only builds the HTTP client; the per-call settings ride the `&ProviderConfig`
/// argument of [`Provider::chat`] (ADR-0016).
pub struct BailianProvider {
    client: reqwest::blocking::Client,
    /// Token usage from the most recent `chat` call (None before any call or
    /// when the endpoint omitted it).
    pub last_usage: Option<TokenUsage>,
    /// Cumulative token usage across every `chat` call since construction
    /// (drives the frontend's end-of-run totals).
    pub total_usage: TokenUsage,
}

impl BailianProvider {
    /// Build the HTTP client. Takes no config — the provider is stateless
    /// (ADR-0016): every `chat` call receives its settings as an argument.
    pub fn new() -> Result<Self, String> {
        let client = reqwest::blocking::Client::builder()
            // A stalled connection must not hang the synchronous agent loop
            // indefinitely. Generous enough for long thinking-streams.
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(300))
            .build()
            .map_err(|e| format!("failed to build HTTP client: {e}"))?;
        Ok(Self {
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
        config: &ProviderConfig,
        cancel: &CancelToken,
        on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
    ) -> Result<(), String> {
        let req = wire::WireRequest {
            model: &config.model,
            messages: wire::messages_to_wire(messages, config.cache),
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
            .post(config.chat_completions_url())
            .bearer_auth(&config.api_key)
            .json(&req)
            .send()
            .map_err(|e| format!("request failed: {e}"))?;

        let status = resp.status();
        // An unsuccessful response carries no deltas: read its body (still
        // interruptibly — a cancel wins over the error text) and report it.
        if !status.is_success() {
            let mut body = Vec::new();
            let end = read_stream_interruptibly(resp, cancel, &mut |bytes| {
                body.extend_from_slice(bytes);
                Ok(())
            })?;
            if end == ReadEnd::Cancelled {
                return Err("request cancelled".to_string());
            }
            return Err(format!(
                "Bailian API error {status}: {}",
                String::from_utf8_lossy(&body)
            ));
        }

        // A successful response is an SSE stream: frame and parse it as it
        // arrives and hand every delta to the sink immediately (ADR-0019), so
        // the caller renders the answer while the endpoint is still writing
        // it. Blocking reqwest exposes no per-read timeout, so a silent server
        // stays bounded by the client timeout (300s) while a flowing stream
        // reacts to an Esc cancel within one socket chunk.
        let mut framer = SseFramer::new();
        // The last usage sample seen; recorded only when the body ends on its
        // own terms (a cancelled request never earned its tokens).
        let mut usage: Option<TokenUsage> = None;
        let end = {
            let mut handle = |bytes: &[u8]| -> Result<(), String> {
                for event in framer.push(bytes) {
                    emit_event(&event, &mut usage, &mut *on_delta)?;
                }
                Ok(())
            };
            read_stream_interruptibly(resp, cancel, &mut handle)?
        };
        if end == ReadEnd::Cancelled {
            // The deltas that already arrived streamed out live; the torn tail
            // (and any usage the final chunk would have carried) is dropped,
            // and the runner sees the cancel token and ends the turn cancelled.
            return Ok(());
        }
        // EOF: an event whose blank line never landed still counts, matching
        // the whole-body parser (`SseFramer::finish`).
        for event in framer.finish() {
            emit_event(&event, &mut usage, &mut *on_delta)?;
        }
        self.last_usage = usage;
        if let Some(u) = usage {
            accumulate_usage(&mut self.total_usage, u);
        }
        Ok(())
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

    // --- read_stream_interruptibly ----------------------------------------

    /// Collect every chunk the read hands over, in order.
    fn chunks_of(reader: impl Read, cancel: &CancelToken) -> (ReadEnd, Vec<u8>) {
        let mut seen = Vec::new();
        let end = read_stream_interruptibly(reader, cancel, &mut |bytes| {
            seen.extend_from_slice(bytes);
            Ok(())
        })
        .unwrap();
        (end, seen)
    }

    #[test]
    fn read_stream_hands_every_chunk_over_and_completes() {
        let cancel = CancelToken::new();
        let (end, seen) = chunks_of(Cursor::new(b"hello\nworld"), &cancel);
        assert_eq!(end, ReadEnd::Complete);
        assert_eq!(seen, b"hello\nworld");
    }

    #[test]
    fn read_stream_completes_empty_reader() {
        let cancel = CancelToken::new();
        let (end, seen) = chunks_of(Cursor::new(b""), &cancel);
        assert_eq!(end, ReadEnd::Complete);
        assert!(seen.is_empty());
    }

    #[test]
    fn read_stream_cancels_without_reading_when_flag_is_already_set() {
        let cancel = CancelToken::new();
        cancel.cancel();
        let reader = ChunkedReader::new("1234567890", 4);
        let (end, seen) = chunks_of(reader, &cancel);
        assert_eq!(end, ReadEnd::Cancelled);
        assert!(seen.is_empty(), "nothing may be read after a cancel");
    }

    #[test]
    fn read_stream_stops_between_chunks_when_flag_flips() {
        let cancel = CancelToken::new();
        // Chunks of 4: the flag flips as the second read begins. The read in
        // flight may still deliver its chunk, but the loop stops before any
        // further read — everything already handed over was streamed out.
        let reader = CancelOnRead {
            inner: ChunkedReader::new("abcdefghij", 4),
            flip_on: 2,
            cancel: cancel.clone(),
        };
        let (end, seen) = chunks_of(reader, &cancel);
        assert_eq!(end, ReadEnd::Cancelled);
        assert_eq!(seen, b"abcdefgh");
    }

    #[test]
    fn read_stream_propagates_io_errors() {
        struct Boom;
        impl Read for Boom {
            fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("socket gone"))
            }
        }
        let cancel = CancelToken::new();
        let err = read_stream_interruptibly(Boom, &cancel, &mut |_| Ok(())).unwrap_err();
        assert!(err.contains("socket gone"), "err: {err}");
    }

    #[test]
    fn read_stream_propagates_a_sink_failure_and_stops_reading() {
        // A frontend that cannot render any more aborts the request: the error
        // travels out verbatim (the runner must surface it, not swallow it as
        // a silent cancelled stop).
        let cancel = CancelToken::new();
        let reader = ChunkedReader::new("abcdefghij", 4);
        let mut seen = 0usize;
        let err = read_stream_interruptibly(reader, &cancel, &mut |bytes| {
            seen += bytes.len();
            Err("renderer exploded".to_string())
        })
        .unwrap_err();
        assert!(err.contains("renderer exploded"), "err: {err}");
        assert_eq!(seen, 4, "the first chunk was delivered before the abort");
    }

    // --- live streaming (ADR-0019) ----------------------------------------

    /// A one-shot SSE server on a random local port: writes each event after
    /// its own delay, then `[DONE]` and EOF. Returns its base URL.
    fn spawn_sse_server(events: Vec<(Duration, String)>) -> String {
        use std::io::{Read as _, Write as _};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut req = [0u8; 8192];
            let _ = sock.read(&mut req);
            let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                        Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
            sock.write_all(head.as_bytes()).unwrap();
            for (delay, event) in events {
                std::thread::sleep(delay);
                let body = format!("data: {event}\n\n");
                sock.write_all(format!("{:x}\r\n", body.len()).as_bytes())
                    .unwrap();
                sock.write_all(body.as_bytes()).unwrap();
                sock.write_all(b"\r\n").unwrap();
                sock.flush().unwrap();
            }
            let done = b"data: [DONE]\n\n";
            sock.write_all(format!("{:x}\r\n", done.len()).as_bytes())
                .unwrap();
            sock.write_all(done).unwrap();
            sock.write_all(b"\r\n0\r\n\r\n").unwrap();
            sock.flush().unwrap();
        });
        base
    }

    /// One text-content chunk, in the live wire shape.
    fn text_event(text: &str) -> String {
        format!(
            "{{\"id\":\"c\",\"object\":\"chat.completion.chunk\",\"choices\":\
             [{{\"index\":0,\"delta\":{{\"content\":\"{text}\"}},\"finish_reason\":null}}]}}"
        )
    }

    /// Chunk one arrives immediately, chunk two 500ms later: a provider that
    /// frames the stream as it reads hands the first delta over at once, while
    /// one that reads the body to EOF first (the pre-ADR-0019 shape) cannot
    /// deliver anything before the gap is over.
    #[test]
    fn chat_delivers_deltas_while_the_response_is_still_streaming() {
        let base = spawn_sse_server(vec![
            (Duration::ZERO, text_event("first")),
            (Duration::from_millis(500), text_event("second")),
        ]);
        let mut provider = BailianProvider::new().unwrap();
        let config = ProviderConfig::new("test-key", base, "test-model");
        let messages = vec![Message::text(Role::User, "hi")];

        let started = std::time::Instant::now();
        let mut arrivals: Vec<(Duration, Delta)> = Vec::new();
        provider
            .chat(&messages, &[], &config, &CancelToken::new(), &mut |delta| {
                arrivals.push((started.elapsed(), delta));
                Ok(())
            })
            .expect("chat succeeds");

        let texts: Vec<&str> = arrivals
            .iter()
            .filter_map(|(_, d)| match d {
                Delta::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["first", "second"]);
        assert!(
            arrivals[0].0 < Duration::from_millis(250),
            "the first delta must reach the sink before the second chunk is sent, \
             but arrived at {:?}",
            arrivals[0].0
        );
    }

    #[test]
    fn chat_finishes_the_tail_event_and_records_usage() {
        // The usage chunk is the last event before `[DONE]`; both must be
        // handled after EOF (finish), exactly like the whole-body parser.
        let base = spawn_sse_server(vec![
            (Duration::ZERO, text_event("answer")),
            (
                Duration::ZERO,
                "{\"id\":\"c\",\"object\":\"chat.completion.chunk\",\"choices\":[],\
             \"usage\":{\"prompt_tokens\":7,\"completion_tokens\":2,\"total_tokens\":9}}"
                    .to_string(),
            ),
        ]);
        let mut provider = BailianProvider::new().unwrap();
        let config = ProviderConfig::new("test-key", base, "test-model");
        let mut texts = String::new();
        provider
            .chat(
                &[Message::text(Role::User, "hi")],
                &[],
                &config,
                &CancelToken::new(),
                &mut |delta| {
                    if let Delta::Text(t) = delta {
                        texts.push_str(&t);
                    }
                    Ok(())
                },
            )
            .expect("chat succeeds");
        assert_eq!(texts, "answer");
        assert_eq!(provider.last_usage.map(|u| u.total_tokens), Some(9));
        assert_eq!(provider.total_usage().total_tokens, 9);
        assert!(provider.total_usage().cached_tokens() == 0);
    }

    #[test]
    fn chat_reports_an_http_error_body() {
        use std::io::{Read as _, Write as _};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut req = [0u8; 8192];
            let _ = sock.read(&mut req);
            let body = "{\"error\":\"bad key\"}";
            let head = format!(
                "HTTP/1.1 401 Unauthorized\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            sock.write_all(head.as_bytes()).unwrap();
            sock.write_all(body.as_bytes()).unwrap();
        });

        let mut provider = BailianProvider::new().unwrap();
        let config = ProviderConfig::new("test-key", base, "test-model");
        let mut delivered = 0usize;
        let err = provider
            .chat(
                &[Message::text(Role::User, "hi")],
                &[],
                &config,
                &CancelToken::new(),
                &mut |_| {
                    delivered += 1;
                    Ok(())
                },
            )
            .unwrap_err();
        assert!(err.contains("401"), "err: {err}");
        assert!(err.contains("bad key"), "err: {err}");
        assert_eq!(delivered, 0, "an error body carries no deltas");
    }

    // --- cancel mid-stream: what already streamed stays out ---------------

    /// A two-event SSE body (each event a reasoning/content chunk).
    const SSE: &str = concat!(
        "data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"content\":\"first \"}}]}\n\n",
        "data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"content\":\"second\"}}]}\n\n",
    );

    #[test]
    fn a_cancelled_read_keeps_the_deltas_that_already_streamed() {
        // A single read delivers the whole (event-complete) stream while Esc
        // lands: the read stops before any further chunk, and every event that
        // already arrived was pushed to the sink (a torn tail simply never
        // reaches `finish`).
        let cancel = CancelToken::new();
        let reader = CancelOnRead {
            inner: ChunkedReader::new(SSE, SSE.len()),
            flip_on: 1,
            cancel: cancel.clone(),
        };
        let mut framer = SseFramer::new();
        let mut texts = String::new();
        let end = read_stream_interruptibly(reader, &cancel, &mut |bytes| {
            for event in framer.push(bytes) {
                let parsed = wire::parse_event(&event)?.expect("not [DONE]");
                for delta in parsed.deltas {
                    if let Delta::Text(t) = delta {
                        texts.push_str(&t);
                    }
                }
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(end, ReadEnd::Cancelled);
        assert_eq!(texts, "first second");
        // The tail is dropped, not flushed: a torn event must not be parsed.
        assert!(framer.finish().is_empty());
    }

    // --- usage accumulation + live smoke (existing suite) ------------------

    /// Test-only: build a provider from the live environment. The real
    /// resolution owner is `slimcode-app::config`; this helper keeps the live
    /// smoke tests working by reading the same env var names and the AI
    /// crate's own endpoint defaults (this crate depends on no slimcode crate).
    fn provider_from_env() -> Result<(BailianProvider, ProviderConfig), String> {
        const ENV_API_KEY: &str = "DASHSCOPE_API_KEY";
        const ENV_BASE_URL: &str = "SLIMCODE_AI_BASE_URL";
        const ENV_MODEL: &str = "SLIMCODE_AI_MODEL";
        let api_key =
            std::env::var(ENV_API_KEY).map_err(|_| format!("{ENV_API_KEY} is not set"))?;
        let base_url = std::env::var(ENV_BASE_URL)
            .unwrap_or_else(|_| crate::config::DEFAULT_BASE_URL.to_string());
        let model =
            std::env::var(ENV_MODEL).unwrap_or_else(|_| crate::config::DEFAULT_MODEL.to_string());
        Ok((
            BailianProvider::new()?,
            ProviderConfig::new(api_key, base_url, model),
        ))
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
        let (mut p, config) = provider_from_env().expect("env config");
        let msgs = vec![Message::text(Role::User, "Reply with exactly: pong")];
        let mut deltas: Vec<Delta> = Vec::new();
        p.chat(&msgs, &[], &config, &CancelToken::new(), &mut |d| {
            deltas.push(d);
            Ok(())
        })
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
        let (mut p, config) = provider_from_env().expect("env config");
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
        let deltas = {
            let mut collected: Vec<Delta> = Vec::new();
            p.chat(&msgs, &[tool], &config, &CancelToken::new(), &mut |d| {
                collected.push(d);
                Ok(())
            })
            .expect("chat succeeds");
            collected
        };
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
