# 04: turn-end compaction 默认 hook 化 + 收缩旧 3-hook 接缝（contract）

**What to build:** 把 turn-end auto-compaction（Completed 回合越阈值 → CompactSummary checkpoint 替换旧段 + `Compacted` 事件 + usage 计入压缩成本 + 失败保留原历史）从 `AgentSession::prompt` 里改写成默认 after_turn hook；同时收缩工单 02 保留的旧 3-hook 接缝（`before_tool`/`after_tool`/`turn_end`）——把最后依赖旧接缝的调用方迁到对应阶段后删除旧接缝，全仓编译绿。`compact_now`（on-demand 强制压缩）经同一压缩组件走通、仍发同一 `Compacted` 事件。

**Blocked by:** 03

**Status:** resolved

- [x] auto-compaction 与 compact_now 均以默认 hook/组件路径实现，可被替换为 consolidation 或关闭
- [x] 跑工单 01 基线：compaction 两路（越阈值产 checkpoint+`Compacted`、before/after token 一致；失败保留原历史与本轮结果）**逐点等效**（spec 清单 16）
- [x] 旧 `before_tool`/`after_tool`/`turn_end` 三 hook 接缝删除，无残留调用方
- [x] 全仓 `cargo build` + `cargo test` 绿；sdk 等效性集成测试全绿
