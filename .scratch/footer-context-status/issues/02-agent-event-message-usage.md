# 02: AgentEvent::Message 携带 usage

**What to build:** 事件层表达"usage 属于 assistant 消息"。`AgentEvent::Message`
变体携带 `usage: Option<TokenUsage>`。runner 把 01 暂存的 usage 附到本次请求对应
的 assistant 消息事件上：正常结束（Stop）的最终消息带；工具循环里发出的 assistant
消息带；tool result 消息**不带**（usage 只属于 assistant 消息）。取消路径无 usage。
app 层 sink 解开新变体（`on_message` 签名本票暂不变，usage 暂不消费——消费在 03）。

**Blocked by:** 01 (needs the chat() return carrying usage first)

**Status:** resolved

- [ ] `AgentEvent::Message` 变体携带 `usage: Option<TokenUsage>`。
- [ ] Stop 分支：最终 assistant 消息事件携带本次请求的 usage。
- [ ] ToolCalls 分支：assistant 消息事件携带 usage；tool result 消息事件不带。
- [ ] 取消的请求不产生携带 usage 的 Message 事件。
- [ ] app 层 sink 适配新变体，`on_message` 签名不变、usage 暂不消费。
- [ ] 全量测试通过；`cargo fmt --all` 与 clippy（`-D warnings`）干净。
