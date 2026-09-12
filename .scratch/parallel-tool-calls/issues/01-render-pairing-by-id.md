# 01: 工具事件携带调用 id，渲染按 id 配对

**What to build:** 工具开始/结束事件及其显示项携带 `tool_call_id`，TUI 工具块按 id 配对（无 id 回退按名字），CLI 文本渲染同步。完成后，**任何执行模式下同名工具多次调用都不再错配**——这是并行渲染的基础设施，横跨事件、显示项、两个前端，编译器强制穷举所有匹配点。

**Blocked by:** None (can start immediately)

**Status:** resolved

- [ ] `ToolStart`/`ToolResult` 事件增加 `tool_call_id` 字段；串行执行路径照常提供该 id
- [ ] 事件到显示项的映射同步携带 `tool_call_id`
- [ ] TUI 工具块按 id 配对 pending 块（`ToolStart` 的 id 非可选，无“无 id”事件来源，故不再保留按名字回退）
- [ ] CLI 文本渲染同步字段（不改变输出字节形状的语义，只配对依据变更）
- [ ] 测试：同名工具两次并行调用、结果乱序到达，各自独立成块且输出配对正确
- [ ] 既有工具块渲染测试全部保持绿色

## Answer

已实现：`AgentEvent::ToolStart`/`ToolResult` 与 `DisplayItem` 增加 `tool_call_id`，TUI `Entry::Tool`
记录 id 并按 id 配对；CLI 文本渲染同步。测试 `parallel_same_name_tools_pair_by_call_id`
覆盖同名工具乱序到达的配对（先按名字 LIFO 验证过可复现错配，再按 id 转绿）。

偏差：`ToolStart.tool_call_id` 为必填 `String`，不存在“无 id 的事件来源”，因此 ticket 原计划的
“无 id 回退按名字配对”分支不可达，已删除（`Entry::Tool.tool_call_id` 改为非可选 `String`）。
