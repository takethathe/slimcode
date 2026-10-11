# 06: 回合后自动压缩

**What to build:** 完成回合且历史越过阈值时自动压缩（对齐 CLI/TUI 的回合后触发）：旧历史替换为会话内摘要，checkpoint 落盘且回放为压缩后视图，发出压缩事件。压缩失败保留原历史与本次 turn 结果，取消/失败回合不触发。

**Blocked by:** 02（会话对象与回合编排）

**Status:** resolved

- [x] 完成回合且历史越过阈值 → 自动压缩：messages 含 `CompactSummary`、日志回放为压缩后视图（load 按最新 checkpoint 丢弃其前记录）、发出 `SdkEvent::Compacted { before, after }`
- [x] 压缩失败 → 无 `Compacted` 事件、历史与本次 turn 结果原样保留（下一次 turn 看到完整历史）
- [x] 取消 / 失败回合不触发压缩
- [x] 压缩的 token 成本计入会话用量（对齐「usage 属于 live session」语义）
