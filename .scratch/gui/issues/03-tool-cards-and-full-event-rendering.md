# 03: 工具卡片与完整事件渲染

**What to build:** 转录区把工具调用渲染成卡片：工具名 + 状态徽标（运行中高亮 / 成功绿 / 失败红），参数与结果默认折叠、点击展开；reasoning 内容以弱化样式显示并可折叠；回合末自动压缩（`compacted`）渲染为可折叠提示。wire 事件桥补全全部 kind 的显式载荷测试。

**Blocked by:** 02

**Status:** resolved

- [x] 前端工具卡片组件：`tool_start` / `tool_result` 配对渲染（工具名 + 状态徽标 + 可折叠参数/结果；运行中高亮、成功绿、失败红；无配对的 start 以运行中态挂起）
- [x] reasoning 渲染：弱化样式（浅灰小字）、可折叠
- [x] `compacted` 渲染：可折叠的压缩提示（before/after token 数）
- [x] 前端对 02 已编码但未渲染的其余 kind（reasoning / tool / compacted）接入转录状态机
- [x] wire 桥显式载荷测试：每个 kind（turn / reasoning / text / tool_start / tool_result / stop / usage / message / turn_finished / compacted）断言 kind 与载荷正确；穷尽性由编译保证
- [x] 手动验收：跑一个会调工具的回合，看到工具卡片折叠/展开、三态配色；reasoning 弱化显示；压缩发生时看到提示

**Gate:** `gui/` 内 `cargo test` 全绿（含 wire 全 kind 载荷测试）；手动走通「工具回合 → 卡片三态 → 折叠展开」。

## Answer

Implemented: `ToolCard` (name + status badge running/ok/error, collapsible arguments/result),
`ReasoningBlock` (dim, collapsible), `CompactNotice` (collapsible before→after token notice),
and all wire kinds (reasoning / tool / compacted) wired into the `reduceTranscript` state
machine. Every kind has an explicit payload test in `wire.rs` (11 tests); exhaustiveness is
compile-enforced.
