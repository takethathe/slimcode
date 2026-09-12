# 02: the AI crate owns the LLM seam

**What to build:** Adding a provider no longer requires importing the agent runtime. The LLM seam —
`Provider`, the wire `Message`, `ToolSpec`, `Delta`, `FinishReason`, `CancelToken` — lives in
`slimcode-ai`, which depends on no other slimcode crate; `slimcode-core` owns the executable tool
and the runtime, and depends only on `ai`. Both frontends keep working exactly as before.

**Blocked by:** 01

**Status:** ready-for-agent

- [ ] `Provider::chat(&[Message], &[ToolSpec], &CancelToken) -> Result<Vec<Delta>, String>`:
      the provider sees tools as a schema only. A tool is
      `ToolSpec { name, description, parameters }`; `core` wraps it as a `Tool { spec, run }` whose
      `run` keeps today's `Fn(Value) -> Result<String, String> + Send + Sync` contract so a Tool
      batch still dispatches in parallel.
- [ ] The runtime builds one `Vec<ToolSpec>` per run (not per request) and assembles the assistant
      message from the delta stream as today.
- [ ] `slimcode-ai` has no slimcode dependency at all — including its `[dev-dependencies]`: the two
      `#[ignore]` live smoke tests read endpoint/model defaults from the AI crate's own config
      defaults instead of importing app-layer constants.
- [ ] Wire behaviour is unchanged: the serde tolerance list (unknown fields ignored; optional
      `usage`, `content`, `function.name`, `function.id`, `prompt_tokens_details`) and the
      `tool_call` fragment assembly rule are covered by the existing tests, which move with the
      code.
- [ ] One-shot and interactive runs behave identically (the tmux smoke suite stays green).
- [ ] `cargo test` green, `cargo fmt --all`, `cargo clippy --all-targets --all-features --
      -D warnings` clean.
- [ ] Docs: the AI section and the runtime section of `development.md` describe the new ownership;
      the ADR trail is ADR-0011 D1/D4.

## Notes

- Domain terms (per `CONTEXT.md`): `Provider`, `Message`, `Tool batch`. `AgentMessage` does not
  exist yet — it arrives in ticket 03.
- The smoke tests must still compile; a real key + network is still required to run them.

## Comments
