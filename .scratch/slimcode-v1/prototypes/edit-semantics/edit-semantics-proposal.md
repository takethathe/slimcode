# Edit 工具语义 — 候选方案（v1）

> Ticket 03 的 prototype 产物。先跑 `cargo run`（demo 电池）感受行为，再就「fuzzy 兜底是否进 v1」给反应。
> 参考实现：pi `packages/coding-agent/src/core/tools/edit.ts` + `edit-diff.ts`（已通读）。

## 建议锁定的 v1 语义（除 Q 外全部建议照抄 pi）

| # | 语义 | 决策 |
|---|---|---|
| 1 | 输入 `{ path, edits: [{oldText, newText}] }`，一次调用可含**多个不相交 edit** | ✅ 照抄 |
| 2 | 所有 edit 都**匹配原始文件**（非增量）；替换按 offset 逆序应用，偏移稳定 | ✅ 照抄 |
| 3 | 每个 `oldText` 必须**恰好出现 1 次**；0 次 → not-found 错误，>1 次 → 不唯一错误（带出现次数） | ✅ 照抄 |
| 4 | 匹配结果**不得重叠**；重叠 → 报错并提示合并 | ✅ 照抄 |
| 5 | `oldText` 为空 → 报错 | ✅ 照抄 |
| 6 | 替换后内容与原文相同（no-op）→ 报错 | ✅ 照抄 |
| 7 | 行尾：检测 CRLF/LF，先归一为 LF 匹配，再恢复原行尾；BOM 剥离/恢复 | ✅ 照抄 |
| 8 | 成功返回：替换块数 + 面向展示的带行号 diff + unified patch + 首个变更行 | ✅ 照抄（diff 可简化） |

## Q — 核心决策：exact 之外要不要 fuzzy 兜底？

pi 的做法：**exact 优先，找不到时**对全文做归一（每行去尾空白 + NFKC + 智能引号/破折号/特殊空格折成 ASCII），在归一空间重试。命中后为了不破坏未触碰行的原始字节，还要把替换映射回原文（`applyReplacementsPreservingUnchangedLines`，一套额外机制）。

prototype 演示了三种情况（demo case 8/9）：
- 模型给的 `oldText` 带**智能引号**（“hello” vs "hello"）：exact 必挂，fuzzy 能救。
- 模型给的 `oldText` 带**行尾空白**：exact 必挂，fuzzy 能救。

**选项**：
- **A. 纯 exact**：最简单、行为最可预测；模型必须逐字节给对 oldText（含空白/换行），错一个字就失败重试。实现省掉整块归一+回映射机制。
- **B. pi 完整 fuzzy**：最稳，模型出错容忍度高；但引入归一化 + `applyReplacementsPreservingUnchangedLines` 回映射机制（v1 工程量最大）。
- **C. 轻量 fuzzy（推荐）**：只做**行尾空白容错**（每行 `trim_end`）这一种归一，不做 NFKC/引号/破折号折叠。理由：真实文件里的 Unicode 引号/破折号场景少见（现代编辑器存 ASCII），而「模型带出尾随空格/多余空格」是高发错误；`trim_end` 的归一不改变字符，回映射机制可大幅简化（按行对齐，未触碰行保留原文）。

## 边界行为（所有选项下一致，prototype 已演示）

- 无匹配 / 多匹配 / 空 oldText / 重叠 / no-op → 各自报错文案（见 demo case 2–6）。
- 超大文件：`oldText` 匹配是 O(n) 线性扫描，无指数风险；不设长度上限（pi 亦无）。

## 你只需要回答

1. fuzzy 兜底选 **A / B / C**？（我推荐 C）
2. 其余 8 条语义照抄 pi，有没有要改的？（默认无）
