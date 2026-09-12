# 03: the Session stores only what the model was shown

**What to build:** A session's message history holds what the model actually saw — plus kinds it must
never see — and nothing derived. The system prompt is assembled fresh every turn instead of being
persisted, the log-only metadata moves out of the message payload, and a session written before this
change still loads.

**Blocked by:** 02

**Status:** ready-for-agent

- [ ] `core::AgentMessage` is a serde-tagged enum (today only the LLM variant; non-LLM kinds such as
      a compaction summary slot into the same enum later) and
      `AgentMessage::to_llm(&self) -> Option<ai::Message>` is the only conversion: LLM variants
      return their message, session-only variants decide to transform or drop.
- [ ] `ai::Message` is the wire shape only: it no longer carries `stop_reason` or `error`.
- [ ] `convert(system: &ai::Message, history: &[AgentMessage]) -> Vec<ai::Message>` — system prefix
      plus `filter_map(to_llm)` — is applied before **every** provider request, so a mid-run
      compaction takes effect on the next turn.
- [ ] `ContextBuilder::build()` returns the system message separately from the messages
      (`Context { system, messages }`); the system prompt never enters `Session::messages` and is
      never written to the log, and a turn therefore adds only its prompt message to history.
- [ ] `Session::messages` is typed to hold `AgentMessage`s; a session-log message record carries
      `stop_reason`/`error` in the record envelope (omitted when absent), and a load **skips** a
      legacy `role: "system"` record without rewriting the file. No log version bump.
- [ ] A cancelled or errored turn still closes on an assistant boundary (ADR-0009 D5), with its
      reason in the envelope.
- [ ] Tests: session-log round-trip with and without the envelope, a legacy log containing a system
      record still loads, `to_llm` per variant, `convert` ordering, `ContextBuilder` returning
      system + messages, and every existing render/runner test updated to the new types.
- [ ] One-shot output stays byte-identical and the TUI's `/load` still works.
- [ ] `cargo test` green, `cargo fmt --all`, `cargo clippy --all-targets --all-features --
      -D warnings` clean.
- [ ] Docs: the session model, session store and context-builder notes in `development.md`; the
      decisions are ADR-0012 plus the amendment notes on ADR-0009 and ADR-0004.

## Notes

- Domain terms (per `CONTEXT.md`): `Message`, `AgentMessage`, `Message history` (the system prompt
  is not part of it), `Session log` (system records are not written; `stop_reason`/`error` are record
  metadata), `Dangling tool batch`.
- Legacy compatibility is read-side only: no migration, no rewrite, no version bump.

## Comments
