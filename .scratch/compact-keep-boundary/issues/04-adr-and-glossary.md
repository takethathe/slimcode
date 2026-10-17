# 04: ADR-0025 + GLOSSARY（Kept tail / CompactSummary / Compaction）

**What to build:** 把"compaction 在 checkpoint 里持久化一个保留边界、让 Kept tail 跨 resume 存活"这一决策留痕，并让术语表与代码现状一致。记录为何用条数（`kept_count`）而非消息 id 或读时重算，以及由此带来的可见后果。

**Blocked by:** 01, 02, 03（描述合入后的最终形状）

**Status:** done

- [x] 新开 `docs/adr/0025-compaction-persists-a-keep-boundary.md`：记录 `kept_count` 边界、`load` 的重建规则（含嵌套跳过旧 checkpoint）、先裁剪后修复、legacy 取 `0`；含 Considered Options（消息 id / pi 的 `firstKeptEntryId`、读时按 token 重算——均被拒）与 Consequences（恢复历史变大 → footer ContextUsage 与再压缩时机口径对齐；append-only 与"记录无 id"两条不变量守住）。
- [x] 在 `docs/adr/0020-compaction-summary.md` 顶部加 `> Amended by ADR-0025` 指针，标注 D2 中"近期保留尾部写在 checkpoint 之前、resume 不存活"的已知代价已被取代；其余 D1/D3/D4/D5/D6 不变（沿用 0009↔0012 的 amend 惯例）。
- [x] `GLOSSARY.md` 更新 `CompactSummary`（补 `kept_count`、把"丢弃 checkpoint 之前一切"改为"保留紧邻的 `kept_count` 条非-checkpoint 记录"）与 `Compaction`（关联 Kept tail 现跨 resume 存活）。
- [x] `GLOSSARY.md` 新增术语 **Kept tail**（compaction 保留、并被 checkpoint 的 `kept_count` 精确标定的那段近期连续消息；_Avoid_: keep boundary 指位置不指消息、recent window）。
- [x] `docs/index.md` 若有 ADR 索引，补上 0025 条目。
- [x] 文档改动与 01/02/03 已合入的字段命名、行为逐条核对一致。
