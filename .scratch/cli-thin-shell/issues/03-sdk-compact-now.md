# 03: SDK：compact_now 按需压缩

**What to build:** SDK 会话可强制压缩历史（无视自动压缩阈值），与 prompt 单飞互斥；成功同步发 `SdkEvent::Compacted { before, after }` 并返回 `(before, after)`，压缩 token 成本计入会话 usage；失败带 provider 错误详情且历史不动。宿主/CLI 的按需压缩能力。

**Blocked by:** None (can start immediately)

**Status:** ready-for-agent

- [ ] `AgentSession::compact_now()`：强制压缩（无视阈值）、与 prompt 单飞（take/put；回合中调用报「a turn is already running」）、dispose 后报错
- [ ] 成功：历史替换为 CompactSummary checkpoint（best-effort 落盘）、发 `Compacted` 事件、返回 `(before, after)`、压缩成本计入 usage
- [ ] 失败：返回带 provider 错误详情的 `Err`、历史与本次回合结果不动、不发事件；无可压缩内容时返回明确错误
- [ ] 内部自动压缩路径重构为带 force 参数 + `Result<Option<…>, String>`，自动路径失败保持静默
- [ ] SDK 测试：小型历史强制压缩成功（事件 + checkpoint + 返回）、无可压缩内容报错、回合中调用被拒（单飞）、summary 失败带错且历史不动、usage 计入；既有测试全绿

## Answer

已实现：`compact_now` 跳过阈值强制按需压缩，成功后同步发出 `SdkEvent::Compacted { before, after }` 并返回 token 变化；失败与边界情形（disposed、并发回合、失败不改历史）均有测试。

**Status:** resolved
