# 06: 顶部栏：会话标题 / 模型名 / 上下文用量（+ 可选 compact_now）

**What to build:** 主区顶部标题栏显示当前会话的推断标题、模型名与上下文用量（如 `45.3%/200k`，复用 app 层估算器与窗口，与 CLI footer 一致）；`message` 事件驱动这些值刷新。可选：`compact_now` 透传命令 + 按钮（手动压缩，事件渲染已在 03 就绪）。

**Blocked by:** 02

**Status:** resolved

- [x] 宿主：暴露会话标题访问器（推断标题）、模型名（overrides 或配置解析结果）、上下文用量（app 层估算器 + 窗口，与 CLI footer 同源）；`message` 事件后刷新这些值
- [x] 前端顶部标题栏：会话标题（无则占位）、模型名、上下文用量徽标（按阈值配色：≤70% 常规 / >70% 黄 / >90% 红）
- [x] 可选 `compact_now`：命令透传 + 前端按钮，触发后渲染 `compacted` 提示（渲染已由 03 提供）
- [x] Rust 单测：title / usage 访问器在回合后返回正确值；上下文估算与 CLI 同源
- [x] 手动验收：prompt 后顶部栏标题/模型/用量正确更新；用量超阈值时配色变化

**Gate:** `gui/` 内 `cargo test` 全绿；手动确认顶部栏三要素随回合更新。

## Answer

Implemented: `TopBar` shows the inferred session title, resolved model, and the context-usage
badge (`45.3%/200k`) using the app-layer estimator/window (same source as the CLI footer) with
threshold colors (≤70% normal / >70% warn / >90% critical); `message` events refresh it.
The optional `compact_now` command + button are wired, rendering the `compacted` notice.
Unit tests assert title/model/context values after turns and that the estimate matches the CLI
estimator.
