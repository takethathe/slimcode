# 03: session.usage 事件驱动累加

**What to build:** footer 的 token 统计更新频率从"turn 末汇总一次"细化为"每条
assistant 消息"。`on_message` hook 签名接收 usage；CLI 在消息进历史时把 usage
累加进 `session.usage`（saturating_add）；删除 turn 末基于 provider 累计值的 diff
快照汇总——`session.usage` 只由事件驱动（每次 assistant 消息累加一次，多请求
turn 正确累计，取消的请求不产生 usage 故不累加）。每条 assistant 消息完成后 emit
一次 usage 渲染项（footer 的 token 统计随之实时更新）。compaction 完成后的重发
保留（context 补全在 04）。

**Blocked by:** 02 (needs the Message event carrying usage)

**Status:** resolved

- [ ] `on_message` hook 收到本次请求的 usage。
- [ ] `session.usage` 每条 assistant 消息累加一次；多请求 turn 累计正确（不重不丢）。
- [ ] turn 末 diff 快照汇总逻辑删除，usage 统计行为不回归。
- [ ] 取消的请求不累加 `session.usage`。
- [ ] 工具循环：每条 assistant 消息完成 emit 一次 usage 渲染项。
- [ ] 全量测试通过；`cargo fmt --all` 与 clippy（`-D warnings`）干净。
