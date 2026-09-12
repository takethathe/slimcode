# 03: the Session stores only what the model was shown

**What to build:** A session's message history holds what the model actually saw — plus kinds it must
never see — and nothing derived. The system prompt is assembled fresh every turn instead of being
persisted, the log-only metadata moves out of the message payload, and a session written before this
change still loads.

**Blocked by:** 02

**Status:** resolved

- [x] `core::AgentMessage` is a serde-tagged enum (today only the LLM variant; non-LLM kinds such as
      a compaction summary slot into the same enum later) and
      `AgentMessage::to_llm(&self) -> Option<ai::Message>` is the only conversion: LLM variants
      return their message, session-only variants decide to transform or drop.
- [x] `ai::Message` is the wire shape only: it no longer carries `stop_reason` or `error`.
- [x] `convert(system: &ai::Message, history: &[AgentMessage]) -> Vec<ai::Message>` — system prefix
      plus `filter_map(to_llm)` — is applied before **every** provider request, so a mid-run
      compaction takes effect on the next turn.
- [x] `ContextBuilder::build()` returns the system message separately from the messages
      (`Context { system, messages }`); the system prompt never enters `Session::messages` and is
      never written to the log, and a turn therefore adds only its prompt message to history.
- [x] `Session::messages` is typed to hold `AgentMessage`s; a session-log message record carries
      `stop_reason`/`error` in the record envelope (omitted when absent), and a load **skips** a
      legacy `role: "system"` record without rewriting the file. No log version bump.
- [x] A cancelled or errored turn still closes on an assistant boundary (ADR-0009 D5), with its
      reason in the envelope.
- [x] Tests: session-log round-trip with and without the envelope, a legacy log containing a system
      record still loads, `to_llm` per variant, `convert` ordering, `ContextBuilder` returning
      system + messages, and every existing render/runner test updated to the new types.
- [x] One-shot output stays byte-identical and the TUI's `/load` still works.
- [x] `cargo test` green, `cargo fmt --all`, `cargo clippy --all-targets --all-features --
      -D warnings` clean.
- [x] Docs: the session model, session store and context-builder notes in `development.md`; the
      decisions are ADR-0012 plus the amendment notes on ADR-0009 and ADR-0004.

## Notes

- Domain terms (per `CONTEXT.md`): `Message`, `AgentMessage`, `Message history` (the system prompt
  is not part of it), `Session log` (system records are not written; `stop_reason`/`error` are record
  metadata), `Dangling tool batch`.
- Legacy compatibility is read-side only: no migration, no rewrite, no version bump.

## Comments

## Answer

已实现（ADR-0012）：`ai::Message` 缩成纯 wire（去掉 `stop_reason`/`error`）；`core::session`
新增 serde-tagged `AgentMessage`（今天只有 `Llm` 变体，`kind` tag）与 `to_llm(&self) -> Option<Message>`，
以及 `convert(system, history)`；core 的 loop 在每次 `chat` 前调用 `convert`，历史类型改为
`Vec<AgentMessage>`，`AgentEvent::Message` 也载 `AgentMessage`。`ContextBuilder::build()` 返回
`Context { system, messages }`（app 层 `Context`），system 每轮现组、不进 `Session::messages`、不写日志；
`run_turn` 收 `Context`（同时解决 8 参数 clippy 问题）。`MessageStopReason` 移到 `core::session`，
日志记录信封形如 `{"type":"message","message":{…},"stop_reason":…,"error":…}`（缺省省略），
`SessionStore::append_closing` 写收尾信封；`load` 跳过 legacy `role:"system"` 记录并兼容无 `kind`
的旧裸消息载荷，不重写文件、不升版本。TUI `submit_prompt`/`trigger_skill` 只把本轮 prompt 推入会话，
`close_turn` 走 `append_closing`。

测试：session-log 有/无信封两条 round-trip、legacy system 记录与 legacy 裸载荷加载、`to_llm`、
`convert` 顺序、`ContextBuilder` system/messages 分离、loop/runner/render 全部改到新类型。
one-shot 输出字节不变（TextRenderer 未动，tmux 冒烟两腿通过）。

验证：`cargo test --workspace` 全绿（ai 45 + core 67 + app 194 + cli 44 + commands 26 + tui 132 +
tmux 冒烟 2），`cargo fmt --all`，clippy 0 error / 0 warning。文档：development.md 会话模型 /
session store / context / runner / TUI 段落按 ADR-0012 改写；ADR-0009、ADR-0004 修订注记已存在。
