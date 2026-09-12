# 02 (S2): two-layer message model (`ai::Message` / `core::AgentMessage`)

**What to build:** ADR-0012. Split the message model into the LLM wire message (`ai::Message`) and
the stored message (`core::AgentMessage`), add the conversion seam, stop storing the system prompt
in a Session, and move `stop_reason`/`error` into the session-log record envelope.

**Blocked by:** 01

**Status:** ready-for-agent

- [ ] `core::AgentMessage` is an enum with a serde tag: today only `Message(ai::Message)`, documented
      as the place non-LLM kinds (a compaction summary) will slot in. Add
      `AgentMessage::to_llm(&self) -> Option<ai::Message>` (LLM variants return their message;
      session-only variants decide their own fate).
- [ ] `ai::Message` loses `stop_reason`/`error`; it is the wire shape only (role + parts + tool
      calls + tool_call_id).
- [ ] `core` gains `convert(system: &ai::Message, history: &[AgentMessage]) -> Vec<ai::Message>`
      (system prefix + `filter_map(to_llm)`). The loop calls it before **every** provider request,
      so mid-run compaction takes effect on the next turn.
- [ ] `ContextBuilder::build() -> Context { system: ai::Message, messages: Vec<AgentMessage> }`; the
      system prompt is assembled from live state every turn and never pushed into
      `Session::messages`. The TUI/CLI no longer slice the built list to find "this turn's new
      messages": the prompt is the only message a turn adds.
- [ ] `Session::messages: Vec<AgentMessage>`; `SessionStore` writes
      `{"type":"message","message":{…},"stop_reason":…,"error":…}` (both omitted when absent) and
      reads the envelope back. `load` **skips** a `role: "system"` message record (rebuilt next
      turn) and never rewrites the file. Legacy envelope-less records still load.
- [ ] Cancelled/errored turns still close on an assistant boundary (ADR-0009 D5): the closing
      assistant message exists, its reason lives in the envelope.
- [ ] Tests: session-store round-trip (envelope present/absent, legacy system record skipped),
      `to_llm` per variant, `convert` ordering, `ContextBuilder` returning `system` + messages,
      and every existing render/runner test updated to the new types.

- [ ] `cargo test` green, `cargo fmt --all`, `cargo clippy --all-targets --all-features -- -D warnings`
      clean.

## Notes

- Domain terms (per `CONTEXT.md`, already updated): `Message`, `AgentMessage`, `Message history`
  (system prompt is not part of it), `Session log` (system records not written; `stop_reason`/`error`
  are record metadata).
- Do not bump the session log version: unknown-record tolerance plus the read-side skip is the whole
  compatibility story.
- Docs: amend the `docs/development.md` 现状 sections for the session model / session store / context
  builder, and `docs/development.md`'s ai section (`Provider` owns the wire model). Decisions:
  `docs/adr/0012-two-layer-message-model.md`, plus the amendments in ADR-0004/ADR-0009.

## Comments
