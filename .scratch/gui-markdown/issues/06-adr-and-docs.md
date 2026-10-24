# 06: 决策记录与文档同步（ADR-0028 + Heading marker 词条）

**What to build:** 把这一批改动里"看不出但必须记住"的决定写成文档：为什么 GUI 引真解析器、为什么
GUI 保留标题 `#` 而 TUI 不保留、为什么为了一个链接点得动就接 opener 插件、以及为什么渲染层第一次
进了自动测试。同时把用户可见行为写进用户手册，把前端门禁口径的更新写进开发文档。

**Blocked by:** 02, 03, 05（文档描述的是它们落地后的行为）。

**Status:** ready-for-agent

- [x] 新增 ADR-0028：解析器与 GFM 选型、标题保留 `#` 并有意偏离 TUI/pi、opener 插件引入、
      渲染层首次进单测（含取舍与代价）
- [x] `docs/index.md` 的 ADR 索引登记 ADR-0028
- [x] `GLOSSARY.md` 新增词条 **Heading marker**（标题前保留的 `#`×N 前缀，层级的第一信号）
- [x] `docs/user-manual.md` 的「桌面 GUI」章节说明 assistant 正文按 markdown 渲染（标题保留 `#`、
      代码块带语言标签、表格、链接在系统浏览器打开）
- [x] `docs/development.md` 的 gui 一节更新前端依赖与前端门禁：渲染层不再"仅手动验收"，
      DOM 断言随 `npm test` 跑

## Comments
- 已实现（f418259）：ADR-0028 含 D1–D6（解析器选型 / 标题 marker / 字体与代码块 / opener 最小权限 /
  GFM 任务与脚注降级 / 渲染层 DOM 测试），`docs/index.md` 已登记，`GLOSSARY.md` 新增 **Heading marker**，
  用户手册与开发文档（含前端门禁口径）已同步。
