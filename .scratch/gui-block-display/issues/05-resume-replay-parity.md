# 05: resume 回放一致性

**What to build:** 恢复一个历史会话时，转录与实时直播所见一致：以 `<skill>` 起始的用户消息回放成带 teal
chip 的 user 块；skill-serving 的 read 工具结果回放成 slate `Skill "x" activated` 块（替换其配对 read 卡）。
复用同一套 `parseSkillBlock` 检测，直播与回放共享规则。

**Blocked by:** 02、04（两种 skill 块形态都已定义并渲染）

**Status:** ready-for-agent

- [x] `blocksFromHistory`：role=user 且文本以 `<skill>` 起始 → 产出 skill-chip user 块（与 02 的直播产出等价）
- [x] `blocksFromHistory`：role=tool 的 skill-serving read 结果 → 替换其配对 read 卡为 `skill` 块（与 04 的直播产出等价）
- [x] reducer 单测：对同一批消息，回放产出的 Block 列表与"直播事件流入"产出的列表在 skill 块与 chip 上等价
- [ ] 手动验收：触发过 skill、跑过模型 read 激活的旧会话，重新打开后转录呈现与当时直播一致

## Comments
- 已实现：`blocksFromHistory` 复用同一 `parseSkillBlock` 与 read 闸门；单测断言回放 Block 形状与直播流入等价。待手动验收旧会话重开。
