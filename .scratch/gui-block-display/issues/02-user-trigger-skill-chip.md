# 02: Skill active block（用户触发路径）— parseSkillBlock + Vitest 地基 + skill chip

**What to build:** 用户用 `/skill:name`（或 `/{name}`）触发一个 skill 后，GUI 转录显示**用户自己那行触发**——
把 skill 名（含斜杠，如 `/grill`）做成 teal 的 Transcript chip，后跟任务参数——而**不是** backend 回传的整段
`<skill …>` XML 块。无任务参数时，也显示那个仅含 chip 的 user 块，让这次触发不会凭空消失。这是本功能的
tracer-bullet 头刀，端到端可 demo。同时为 reducer seam 建立 Vitest 测试地基。

**Blocked by:** 01（chip 落进重排后的 user 块）

**Status:** ready-for-agent

- [x] `gui/ui` 引入 Vitest：dev 依赖 + `test` 脚本 + 最小配置，纯 reducer 测试可运行
- [x] 新增纯函数 `parseSkillBlock(text)`：仅当 text **以** `<skill name="…"` 起始时解析出 `(name, 块内内容)`，否则返回空；
      尾部任务参数留在块外（reducer 单测锁定）
- [x] reducer：以 `<skill>` 起始的 user 消息 → 产出一个 skill-category 的 Transcript chip user 块（name + 尾部参数，不含 XML）
- [x] reducer：无任务参数时仍产出仅含 skill chip 的 user 块
- [x] reducer：`<skill>` 出现在句中（非起始）不当作 skill 触发
- [ ] 呈现：user 块里 skill chip 为 teal 圆角徽标、`font-mono`，包住含斜杠的触发词（手动验收）
- [ ] 手动验收：`/skill:name` 触发一个已安装 skill → 转录显示带 teal chip 的 user 块，看不到 `<skill>` XML

## Comments
- 已实现：引入 Vitest（devDep + `test` 脚本 + 锁文件同步）；`parseSkillBlock` 镜像 `parse_skill_block`（仅起始匹配，`</skill>` 内联不漏尾）；reducer 以 `<skill>` 起始的 user 消息产出 teal skill chip + 尾部参数。14→17 单测覆盖。呈现待手动验收。
