# Deltas reach the frontend as they arrive: the provider seam carries a delta sink

A long answer never appeared piece by piece in the TUI: the transcript stayed on
`⠋ Working...` and then the whole text landed at once, after the endpoint had
finished. The wire was streaming (`stream: true`), the reader was chunked, and the
frontend rendered every fragment it was handed — but `Provider::chat` returned
`Result<Vec<Delta>, String>`, so the provider had to read the body to EOF, parse it
as one string, and hand the caller a completed vector. Nothing downstream could
observe a delta earlier than that vector, however fast the transport streamed.

Measured against a local SSE endpoint that writes one event every 500ms: the CLI
printed `Hello`, ` from`, ` a`, ` fake`, ` stream.` all at +2.5s (one fragment per
event only *after* the body ended), and the TUI showed the finished sentence in a
single capture after ~3.6s of spinner. The transport was never the problem:
`reqwest::blocking::Response` as a `Read` hands out bytes per chunk (probed:
reads at 407ms / 812ms / 1217ms for events sent at 400/800/1200ms). The seam was.

## Decisions

### D1 — `Provider::chat` takes a delta sink and returns nothing to render

```rust
fn chat(
    &mut self,
    messages: &[Message],
    tools: &[ToolSpec],
    config: &ProviderConfig,
    cancel: &CancelToken,
    on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
) -> Result<(), String>;
```

The provider pushes each delta into `on_delta` the moment it is complete off the
wire; the caller renders it right there. Returning `Vec<Delta>` alongside would
create two delivery paths and a double-render hazard (does the caller emit the
vector, or the sink?), so the vector is gone: the caller accumulates what the sink
delivered, which is exactly what `assemble` turns into the assistant message. One
delivery, one order, no "don't re-emit the returned list" rule to remember.

The sink returns `Result<(), String>` because a frontend can fail mid-answer (a
renderer error, a session-log write that must abort the turn): the error aborts the
request and propagates verbatim, rather than being swallowed by a provider that has
nothing to report it through.

### D2 — The provider frames and parses SSE incrementally

`wire::SseFramer` is the incremental half of the existing framing rules: feed raw
bytes, get each event's `data:` payload back when its terminating blank line lands.
It buffers by bytes, so a chunk boundary inside a line, inside an event or inside a
multi-byte UTF-8 character is harmless; `parse_sse_events` (the whole-body parser
the existing tests pin) is now that same framer run over the whole body, so the two
paths cannot drift. `parse_event` interprets one payload and is shared by both.

The provider reads the response chunk by chunk (`read_stream_interruptibly`), pushes
each received chunk through the framer and hands the resulting events to `on_delta`.
`finish()` flushes the tail at EOF, so an event whose blank line never arrived still
counts — byte-identical to the old whole-body parse.

### D3 — A cancelled read drops the torn tail; the salvage machinery is deleted

Cancellation was built around the buffered read: stop reading, keep the partial
`Vec<u8>`, trim it to the last `\n\n` (`trim_partial_sse_tail`), re-parse it
leniently, and return whatever deltas survived. With incremental framing there is
nothing to salvage — the complete events already reached the sink as they arrived,
and the torn tail is simply never passed to `finish()`. `read_body_interruptibly`,
`ReadOutcome` and `trim_partial_sse_tail` are removed; the user-visible contract is
unchanged (Esc keeps what already streamed, discards the half message, records no
usage) and the code no longer re-parses text it has already parsed once.

### D4 — A sink failure is fatal even when a cancel lands at the same moment

The runner used to emit deltas *after* `chat` returned, so a renderer error was
reported before the following cancel check. Now the error surfaces from inside the
sink during the request, where the existing `Err(_) if cancel.is_cancelled()`
arm could swallow it as a silent cancelled stop. The runner tracks whether the
error came from its own sink (`sink_failed`) and lets that case through as a real
error; a provider error alongside a cancel stays a silent cancelled stop.

### D5 — Usage is still recorded only when the body ends on its own terms

The usage sample rides the final chunk. It is captured while streaming but assigned
to `last_usage` / `total_usage` only after a complete read, and skipped on a
cancelled one — the behaviour the buffered path had by returning early.

> **Extended by ADR-0020 D6**: the same captured sample is now also the `chat`
> return value (`Ok(Some(usage))`; `Ok(None)` on cancel), so it leaves the
> provider with the response rather than only through the internal accounting
> fields. Nothing about *when* it is captured changed.

## Consequences

- Every provider — the real one and each test fake — must push its deltas through
  the sink. A fake that returns them without pushing now fails the runner's
  ordering tests instead of silently rendering nothing.
- `Provider::chat` takes five arguments; ADR-0016 D3's config seam is unchanged in
  substance (its "four arguments" sentence is superseded here), and this ADR
  supersedes ADR-0006 D6a's description of the cancel-time salvage parse.
- `Provider::chat` also returns `Result<Option<TokenUsage>, String>` (ADR-0020 D6),
  so every provider implementation must return its request's usage rather than `()`.
- A response that dies mid-answer now shows the text that already arrived *plus*
  the error, where it previously showed only the error. Partial output on a torn
  stream is the point of streaming; the assistant message still does not enter
  history on a failure.
- The tests that matter: `crates/ai` pins live delivery against a local SSE server
  whose second event trails the first by 500ms (the first delta must reach the sink
  before it is sent), the framer is pinned against `parse_sse_events` for every
  chunk size, and `crates/core` pins that a delta reaches the sink before `chat`
  returns. The tmux smoke test's spinner loop reads the status line before it stops
  on the final answer, because the answer now arrives while the turn is still
  running.
