# llm-live-streaming

Stream the model's answer to the frontend while it is being generated, instead of
handing the whole answer over once the HTTP body ends.

## Problem

Both frontends already render fragments as `DisplayItem`s arrive, and the wire is
already an SSE stream with `stream: true`. But `Provider::chat` returned
`Result<Vec<Delta>, String>`, so the provider had to read the response body to EOF,
parse the whole string once, and only then return a finished vector. The frontend
therefore could not display anything until generation had stopped — the spinner
`⠋ Working...` for the entire answer, then the text in one jump.

## Outcome

Deltas cross the `Provider::chat` seam through a sink and are forwarded to the
renderer the moment each SSE event completes. A cancel keeps what already
streamed, drops the torn tail and records no usage; a complete read records usage
and assembles the assistant message exactly as before. Frontends, session
persistence and history semantics are unchanged.

## Acceptance

- A response whose events are spread over time is rendered fragment by fragment,
  in the one-shot CLI and in the TUI, while the request is still open.
- Ordering, assembly, cancellation, usage accounting and error propagation match
  the previous behaviour (pinned by the existing test suites).
- No whole-body read or re-parse of already-parsed text remains on the response
  path.

## Tickets

- `issues/01-deltas-stream-as-they-arrive.md` — the seam change, the incremental
  framer, the removal of the salvage machinery, and the regression tests.

## Decisions

- ADR-0019 (`docs/adr/0019-deltas-stream-through-the-provider-seam.md`).
