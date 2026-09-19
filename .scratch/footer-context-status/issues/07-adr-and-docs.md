# 07: ADR-0020 修订 + CONTEXT.md 术语收尾

**What to build:** 记录本次 effort 确立的最终 seam 状态。ADR-0020 修订：窗口
128k→200k、触发从 92% 改为"剩 16k 空 context"、usage 事件流（`chat()` 返回值 →
`AgentEvent::Message` 携带 → 事件驱动累加 → footer context 渲染）的架构决策与
取舍（为何不用 `on_usage` 回调、为何不用 `Delta::Usage` 变体）。CONTEXT.md 更新
`Footer` 词条（新增 context 段）、`Compaction` 词条（200k + 剩 16k）、新增
`ContextUsage` 词条。相关 docs 同步。本 effort 的 spec 与全部 issue 状态置 resolved。

**Blocked by:** 01, 02, 03, 04, 05, 06 (docs describe the final seam state)

**Status:** resolved

- [ ] ADR-0020 记录窗口/触发/usage 事件流决策与取舍。
- [ ] CONTEXT.md 的 `Footer` / `Compaction` 词条更新，新增 `ContextUsage` 词条。
- [ ] 相关 docs 同步（开发文档、用户手册）。
- [ ] 本 effort 的 spec 与全部 issue 状态置 resolved。
- [ ] 全量测试通过；`cargo fmt --all` 与 clippy（`-D warnings`）干净。
