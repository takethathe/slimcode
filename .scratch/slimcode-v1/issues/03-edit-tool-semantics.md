# 03 — Edit 工具语义

Type: prototype
Status: resolved

## Question

slimcode v1 的 `edit` 工具应如何行为？

参考 pi 的 edit 工具：oldText/newText 精确替换、单次唯一匹配、空白敏感、零/多匹配报错、diff 预览。备选：apply_patch（unified diff hunk）、模糊匹配。

要定下的问题：v1 锁定哪套语义？以及边界行为 —— 无匹配、多匹配、空替换、超大文件 —— 各自的错误/反馈长什么样？

## Answer

决策已锁定（用户选 **C + 其余 8 条照抄 pi**）。

**v1 edit 语义**：
1. 输入 `{ path, edits: [{oldText,newText}] }`，一次调用可含多个**不相交** edit
2. 所有 edit 匹配**原始文件**（非增量）；按 offset 逆序应用，偏移稳定
3. 每个 `oldText` 必须**恰好出现 1 次**；0 次 → not-found，>1 次 → 不唯一（带出现次数）
4. 匹配**不得重叠** → 报错并提示合并
5. `oldText` 为空 → 报错
6. no-op（替换后内容相同）→ 报错
7. 行尾 CRLF/LF 检测 + 归一/恢复；BOM 剥离/恢复
8. 成功返回：替换块数 + 带行号 diff + 首个变更行

**fuzzy 兜底 = 选项 C（轻量）**：exact 优先；找不到时仅做**每行 trim_end** 归一重试；命中后按行回映射，**未触碰行保留原始字节**。不做 NFKC/智能引号/破折号折叠（现代编辑器存 ASCII，真实场景少；「模型带出行尾/多余空白」才是高发错误）。

原型（资产，`cargo run` 复现）：`prototypes/edit-semantics/`（含 `edit-semantics-proposal.md`）。决定已折入 `crates/agent/src/tools/edit.rs`（见后续实现）。
