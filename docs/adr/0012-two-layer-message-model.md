# Two-layer message model: `ai::Message` (wire) and `core::AgentMessage` (session)

The single `Message` type in `slimcode-agent::session` today plays two roles at once: it is the LLM
wire message, and it is the unit a Session stores and replays. That conflation shows in two places:
`stop_reason`/`error` (ADR-0009 D5) are log-only fields carried on the message type and skipped on
the wire, and the system prompt is stored in `Session::messages` and written to the session log as a
`role: "system"` record — so "the session holds messages the model sees" is only accidentally true.
This ADR splits the roles.

## Decisions

### D1 — `ai::Message` is the wire message; `core::AgentMessage` is the stored one

`ai::Message` is one LLM-visible message (role + content parts + tool calls, serialized exactly as
the provider expects). `core::AgentMessage` is what a Session's message history holds: today the
user/assistant/tool messages, plus kinds the model must never see (the first planned one is a
compaction summary). `Session::messages` is a `Vec<AgentMessage>`.

This is pi's shape: `pi-ai` defines the LLM `Message`, `pi-agent-core` defines
`AgentMessage = Message | CustomAgentMessages` with `convertToLlm` between them.

### D2 — `AgentMessage::to_llm` is the only conversion, and it drops or transforms

`to_llm(&self) -> Option<ai::Message>` is a method on `AgentMessage`: the LLM variants return their
`Message`, session-only variants decide their own fate (a compaction summary becomes the summary
text the model should see, or nothing at all). `core`'s runner calls
`convert(system, history)` — prepending the system message to the `filter_map(to_llm)` result —
before **every** provider request, so a compaction performed mid-run takes effect on the next turn
without the loop knowing anything about it.

### D3 — The system prompt is assembled per request and never stored

`ContextBuilder::build()` returns `Context { system: ai::Message, messages: Vec<AgentMessage> }`.
The system message is rebuilt from live state (base prompt, environment, context files, skills)
every turn, so persisting it would store a derived value and make a session's portability depend on
the environment it was created in. `Session::messages` therefore contains no system message, and the
session log records none. `load` skips a legacy `role: "system"` message record (it is rebuilt on
the next turn); this is why no log version bump is needed — unknown-record tolerance already exists
(ADR-0009 D3).

### D4 — Log-only fields move to the record envelope

`stop_reason`/`error` leave the message type. A message record carries them next to the payload:
`{"type":"message","message":{…},"stop_reason":"aborted","error":"…"}` (both omitted when absent).
The message payload stays byte-identical to an ordinary wire-shaped message, and ADR-0009 D5's
"a failed or cancelled turn is closed on an assistant boundary" behaviour is unchanged — the closing
assistant message still exists, its reason now lives in the envelope.

## Considered Options

- **One message type in `ai`** (move today's superset into `ai` and let the session store it as-is)
  — rejected: `ai` would then own log-only fields and know about compaction; the provider layer
  would be shaped by storage concerns.
- **`AgentMessage` in `ai`, `Message` in `core`** (the literal reading of the original requirement)
  — rejected: it inverts the dependency (the provider layer must not import the agent runtime) and
  contradicts how pi splits the two types.
- **Keep the system prompt in the session** — rejected: it duplicates a value that is rebuilt every
  turn from live state (environment, context files, installed skills), and it makes every restored
  session carry the system prompt of a possibly different configuration.
- **Bump the session log version for D3/D4** — rejected: `load` already ignores unknown record kinds
  and skips bad records, and dropping a system record is a pure read-side rule. ADR-0009 is amended
  instead.

## Consequences

- ADR-0009 is amended: D2's system-message clause and D5's field placement are superseded; the rest
  (lazy creation, one record per message, lenient read, in-memory dangling-batch repair, quota)
  stands.
- `ContextBuilder`'s return type changes from `Vec<Message>` to `Context`; the TUI's
  "push the tail of the context into the session" trick disappears — the prompt is the only new
  message a turn adds.
- `Message history` in `CONTEXT.md` gains "the system prompt is not part of it"; `Session log`
  gains "system messages are not recorded".
- The one-shot CLI path is unaffected beyond the type names (it never persists a session).
