# 03: code 型行为落到各自接缝（skill 拦截=phase hook，context/持久化=operand+sink）

**What to build:** 让一轮 code prompt 端到端走七阶段管线，并把今天焊在 `AgentSession::prompt` 里的 code 型产品行为落到各自贴合的接缝：跨切的否决/改写做成内置默认 phase hook（`pre_tool` 的 skill read 拦截——命中 skill 自身 `SKILL.md` 用 Skip 供激活块；回合末回滚走 `after_turn`）；context 装配（environment + AGENTS.md context files + skills + history + prompt）与消息入历史/持久化（write_user / persist_assistant / persist_tool_message + title 推断 + close_turn 收尾）归循环骨架 + operand / turn seam（message sink），但每个 hook 都拿到可变的 `RunState`，宿主可注册 hook 改写 `messages` 覆盖这份默认维护——slimcode 自身只用默认。`AgentSession` 的对外签名与事件词汇不变——它是 code preset（Engine + 这份默认 bundle）；CLI/TUI/GUI/FFI 继续消费 `AgentSession`、不受影响。

**Blocked by:** 01（金标准基线）、02（七阶段管线）

**Status:** resolved

- [x] 跨切否决/改写以默认 phase hook 实现、可被替换或关闭、不吃内部特权（skill 拦截走 `pre_tool`、回滚走 `after_turn`）；context 装配与持久化+title+close_turn 由循环骨架 + operand / turn seam（message sink）承接，宿主可用 hook 改写 `messages` 覆盖默认（slimcode 只用默认）
- [x] 跑工单 01 的基线：事件序列 / JSONL 字节与回放 / usage / title / stop reason / skill 激活块文本 **逐点等效**（spec 清单 13–15、17–18）
- [x] AgentSession 对外 API（prompt/subscribe/abort/dispose/messages/session_id/title/usage）与 SdkEvent 词汇未变
- [x] sdk/app/cli 现有集成测试全绿；crates/cli 的 architecture.rs 依赖矩阵仍成立（接缝类型在既有 app→core/ai 边内流动，未新增 crate）
