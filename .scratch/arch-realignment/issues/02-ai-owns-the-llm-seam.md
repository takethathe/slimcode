# 02: the AI crate owns the LLM seam

**What to build:** Adding a provider no longer requires importing the agent runtime. The LLM seam —
`Provider`, the wire `Message`, `ToolSpec`, `Delta`, `FinishReason`, `CancelToken` — lives in
`slimcode-ai`, which depends on no other slimcode crate; `slimcode-core` owns the executable tool
and the runtime, and depends only on `ai`. Both frontends keep working exactly as before.

**Blocked by:** 01

**Status:** resolved

- [x] `Provider::chat(&[Message], &[ToolSpec], &CancelToken) -> Result<Vec<Delta>, String>`:
      the provider sees tools as a schema only. A tool is
      `ToolSpec { name, description, parameters }`; `core` wraps it as a `Tool { spec, run }` whose
      `run` keeps today's `Fn(Value) -> Result<String, String> + Send + Sync` contract so a Tool
      batch still dispatches in parallel.
- [x] The runtime builds one `Vec<ToolSpec>` per run (not per request) and assembles the assistant
      message from the delta stream as today.
- [x] `slimcode-ai` has no slimcode dependency at all — including its `[dev-dependencies]`: the two
      `#[ignore]` live smoke tests read endpoint/model defaults from the AI crate's own config
      defaults instead of importing app-layer constants.
- [x] Wire behaviour is unchanged: the serde tolerance list (unknown fields ignored; optional
      `usage`, `content`, `function.name`, `function.id`, `prompt_tokens_details`) and the
      `tool_call` fragment assembly rule are covered by the existing tests, which move with the
      code.
- [x] One-shot and interactive runs behave identically (the tmux smoke suite stays green).
- [x] `cargo test` green, `cargo fmt --all`, `cargo clippy --all-targets --all-features --
      -D warnings` clean.
- [x] Docs: the AI section and the runtime section of `development.md` describe the new ownership;
      the ADR trail is ADR-0011 D1/D4.

## Notes

- Domain terms (per `CONTEXT.md`): `Provider`, `Message`, `Tool batch`. `AgentMessage` does not
  exist yet — it arrives in ticket 03.
- The smoke tests must still compile; a real key + network is still required to run them.

## Comments

## Answer

已实现：新增 `slimcode-ai::message`（`Message`/`Role`/`Part`/`ToolCall`/`MessageStopReason`）与
`slimcode-ai::llm`（`Provider`/`ToolSpec`/`Delta`/`FinishReason`/`CancelToken`），
`Provider::chat(&[Message], &[ToolSpec], &CancelToken)`；`slimcode-core` 的 `Tool` 改为
`Tool { spec: ToolSpec, run }`（`Tool::new` 签名不变），loop 在每次 run 开头构建一次
`Vec<ToolSpec>`。`crates/ai` 的 `[dependencies]` 与 `[dev-dependencies]` 均不再含任何 slimcode
crate（`cargo tree -p slimcode-ai` 只列自身）；两个 `#[ignore]` live 冒烟测试改读 env 名 +
`ai::config` 自有的 `DEFAULT_BASE_URL`/`DEFAULT_MODEL`。端点默认值移入 `ai::config`，`app::config`
`pub use` 之，避免两处常量。

偏差：`core::session` 保留 `Session` 并 re-export `ai::message` 的消息类型，`core::agent` re-export
`ai::llm` 的 seam 类型，使后续 ticket 的导入路径渐进迁移（ticket 03 会改到 `ai::Message` 与
`core::AgentMessage`）。`Tool` 的字段名 `name`/`description`/`parameters` 收进 `spec`，测试断言同步。

验证：`cargo test --workspace` 全绿（ai 45 + core 64 + app 191 + cli 44 + commands 26 + tui 132 +
tmux 冒烟 2），`cargo fmt --all` 已应用，clippy 0 error / 0 warning。
