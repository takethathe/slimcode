# 04: Skill active block（模型 read 激活路径）— slate 提示块原位替换 read 卡

**What to build:** 当模型自己 `read` 某 skill 的 `SKILL.md` 来激活它时（SDK 已把该 read 的工具结果替换为
`<skill>` 块），GUI 不再显示那张普通 read 工具卡，而是把其**原位替换**成一块 slate 底 + accent 色条的提示：
`Skill "<name>" activated`。同一件事不呈现两遍。这是模型激活路径的专用块（用户触发路径走 02 的 user 块，
两者非对称）。

**Blocked by:** 02（复用 parseSkillBlock 与 skill 块判定）

**Status:** ready-for-agent

- [x] Block 模型新增 `skill` 提示块变体（承载 name，渲染为 `Skill "<name>" activated`）
- [x] reducer：一次 read 的 `tool_result` 文本以 `<skill>` 起始 → 把其配对的 tool 块原位替换为 `skill` 块，转录里不再残留该 read 卡
- [x] reducer：非 skill 的普通 read 结果仍维持现有工具卡配对，不受影响
- [x] reducer 单测：`tool_start(read)` + 以 `<skill>` 起始的结果 → 结果为 `skill` 块而非 `tool` 块；普通 read 结果仍为 `tool` 块
- [ ] 呈现：`Skill "x" activated` 用 slate 底 + 左侧 accent 色条，明显区别于 user 块靛蓝（手动验收）
- [ ] 手动验收：让模型读一个已安装 skill 的 `SKILL.md` → 转录出现该 skill 激活提示，看不到重复的 read 工具卡

## Comments
- 已实现：新增 `skill` 块变体；`tool_result` 分支仅当配对工具为 `read` 且结果以 `<skill>` 起始时原位替换 read 卡为 slate 提示块（对齐 CLI renderer）。单测覆盖 read/非-read/普通 read。呈现待手动验收。
