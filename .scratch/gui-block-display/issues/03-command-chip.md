# 03: 命令触发词的 Transcript chip（amber）

**What to build:** 任何以 `/{token}` 开头的用户消息落到转录后，其触发词（含斜杠）显示为 amber 的
Transcript chip，与 skill chip（teal）颜色相区分；普通 prompt 文本无 chip。被前端拦截的本地命令
（`/new` `/compact` `/help` `/exit` `/usage`）不进入对话历史，因此天然不产生 chip——作为回归护栏验证。

**Blocked by:** 02（chip 机制与 user 块判定顺序已建立）

**Status:** ready-for-agent

- [x] reducer：user 消息文本以 `/{token}` 起始且非 skill 块 → 产出 command-category 的 Transcript chip user 块，其余文本为正文
- [x] reducer：判定顺序为先 skill 块检测、后命令检测；不以 `/` 起始的普通文本无 chip
- [ ] 呈现：command chip 为 amber 圆角徽标、`font-mono`，与 teal skill chip 及 user 块靛蓝底三者均可读（手动验收）
- [x] 回归护栏：`/usage` 等本地拦截命令不经过 user 消息流，reducer 不为其产生 chip
- [x] reducer 单测覆盖：command chip 命中 / 普通文本不命中 / skill 优先于 command

## Comments
- 已实现：reducer 对以 `/{token}` 起始的非 skill 消息产出 amber command chip，判定顺序先 skill 后 command；本地拦截命令护栏以 `INTERCEPTED_COMMANDS` 集合断言落地。呈现待手动验收。
