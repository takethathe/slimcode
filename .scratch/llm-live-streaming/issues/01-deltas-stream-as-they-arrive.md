# llm-live-streaming

**What to build:** the LLM response must reach a frontend **as it is generated**:
every SSE event the endpoint writes is parsed and rendered while the request is
still open, in the one-shot CLI and the TUI alike. Cancel keeps what already
streamed, drops the torn tail, and records no usage.

**Why now:** the wire was streaming and both frontends rendered fragments, but
`Provider::chat` returned `Result<Vec<Delta>, String>`, which forced a
read-to-EOF-then-parse body read: a long answer appeared as one instant block
after the endpoint finished. Diagnosed with a local SSE endpoint (events every
500ms): the frontend saw nothing for the whole body, then all five fragments at
once.

**Decision:** see ADR-0019 (`docs/adr/0019-deltas-stream-through-the-provider-seam.md`).

- [x] `Provider::chat` carries `on_delta: &mut dyn FnMut(Delta) -> Result<(), String>`
      and returns `Result<(), String>` — one delivery path, no double render.
- [x] `wire::SseFramer` frames SSE incrementally; `parse_sse_events`/`parse_stream`
      are re-expressed on top of it so live and whole-body parsing cannot drift.
- [x] The provider reads and parses chunk by chunk, pushing each complete event's
      deltas into the sink; a cancel drops the torn tail and records no usage.
- [x] `read_body_interruptibly`, `ReadOutcome` and `trim_partial_sse_tail` are gone
      (nothing to salvage once deltas are delivered as they arrive).
- [x] The runner accumulates the sink's deltas for `assemble`; a sink failure is
      reported even when a cancel lands at the same moment.
- [x] Regression tests: live delivery against a local SSE server, framer vs
      whole-body parity for every chunk size, and delta-before-`chat`-returns in
      the runtime loop.
- [x] Docs updated (ADR-0019, ADR-0006 D6a / ADR-0016 pointers, `development.md`,
      `docs/index.md`), clippy clean, workspace tests green.

## Answer

Root cause: the seam, not the transport. `reqwest::blocking::Response` hands out
bytes per socket chunk (probed: reads at 407/812/1217ms for events sent at
400/800/1200ms), but the provider accumulated the whole body and parsed it once,
so the caller could not observe a delta before EOF.

Fix: `Provider::chat` takes a delta sink and reports nothing back to render; the
provider frames SSE incrementally (`SseFramer`) and pushes each event's deltas the
moment they complete; the runner forwards them live and accumulates the same list
for `assemble`. The buffered-read salvage machinery exists only to serve the old
shape and is deleted.

Verified: one-shot CLI prints one fragment per server chunk (`Hello` at +0.46s …
`stream.` at +2.48s, previously all at +2.54s); the TUI's transcript grows across
captures while the spinner keeps animating (`Hello` → `Hello from` → … ),
previously one capture with the whole sentence after ~3.6s.
