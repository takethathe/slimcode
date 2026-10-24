# 03: 文档、ADR 与术语

**What to build:** 把这次 GUI 与 TUI 的有意分叉写进仓库的既有文档体系：一条 ADR 记录"四项常驻、`CH` 取最近
一次请求、放弃瀑布流 usage 行、`/usage` 退化为校准与脉冲、恢复会话后归零的已知边界"及其取舍；术语表新增
**Usage HUD**；用户手册的 GUI 段落与实际行为对齐（不再描述 usage 行、`/usage` 改为反馈）；ADR 索引追加新条目。

**Blocked by:** 01、02（文档描述的是两张票落地后的最终行为）

**Status:** done

- [x] 新 ADR 沿用 `docs/adr/` 既有格式（首行标题即结论、`Source spec:` 行、`## Decisions` 分条）并说明
      与 TUI/CLI 累计口径的分叉理由
- [x] `GLOSSARY.md` 新增 **Usage HUD**，并标注 `usage_summary` 现在只服务 TUI/CLI
- [x] `docs/user-manual.md` 的 GUI 描述与新行为一致，`grep -rn "usage" docs/user-manual.md` 无残留旧说法
- [x] `docs/index.md` 的 ADR 列表包含新条目
- [x] 新 ADR 把"恢复后归零"指向 `.scratch/session-usage-persistence/issues/01-persist-usage-across-resume.md`
