# 06: 架构登记 — ADR-0027 + docs/index.md

**What to build:** 把本次 GUI 显示模型的关键决定固化为架构决策记录，供未来读者理解"为什么"。记录三条核心
决定及其权衡：① 前端从消息/工具结果文本检测 `<skill>` 块，而非在后端新增一条 skill 激活 wire 事件；
② 两条 skill 激活路径的非对称呈现（用户触发 = 带 chip 的 user 块；模型 read = slate 提示块）；
③ 为锁定 skill/chip 判定逻辑，在 GUI 引入 reducer 单测（对 ADR-0024 D7"v1 不引 Vitest"的有意反转）。

**Blocked by:** 05（实现落地后据实记录）

**Status:** ready-for-agent

- [x] 新增 `docs/adr/0027-gui-skill-active-block-and-transcript-chip.md`，含上述三条决定、考虑过的替代方案与后果
- [x] 在 `docs/index.md` 的 ADR 索引登记 0027
- [x] 术语与 `GLOSSARY.md` 的 **Skill active block** / **Transcript chip** 词条一致（已随本次会话更新）
- [x] ADR 引用来源 spec `.scratch/gui-block-display/spec.md`

## Comments
- 已实现：新增 `docs/adr/0027-gui-skill-active-block-and-transcript-chip.md`（D1 前端文本检测 / D2 非对称呈现 / D3 引入 Vitest 反转 ADR-0024 D7），并在 `docs/index.md` ADR 索引登记；术语对齐 GLOSSARY。
