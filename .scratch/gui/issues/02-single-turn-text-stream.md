# 02: 新会话 + 单回合文本流（tracer bullet）

**What to build:** 应用启动即有一个空白会话、输入框聚焦；用户输入 prompt 后 Enter 发送，宿主在 worker 线程上跑一轮 `AgentSession::prompt`，`SdkEvent` 经 GUI wire 事件桥编码、经 Tauri Channel 流式推给前端，文本片段逐段追加到转录区并自动跟随滚动；回合结束（`stop`）后渲染 token 用量行；中止按钮随时 `abort()`（返回 Cancelled，UI 全程可响应）。这是贯穿「宿主控制器 + wire 桥 + worker 线程 + 前端转录」全链路的第一条可用路径。

**Blocked by:** 01

**Status:** resolved

- [x] `AppController` 雏形（纯 Rust，不依赖 Tauri 运行时）：会话注册表持有一个当前 `AgentSession`，状态机 Idle / Running / Disposed；`new_session` 用设置 overrides 创建会话；`prompt` spawn worker 线程跑同步单飞回合，重复提交报 "turn already running"；`cancel` 只调 `abort()`（跨线程、不等会话锁）；`dispose` join 线程并清空监听器
- [x] GUI wire 事件桥：穷尽 match 把 `SdkEvent` / `DisplayItem` 映射为扁平事件（无 `_` 分支，`DisplayItem` 新增变体即编译失败）；本切片前端消费文本路径（文本 / stop / 用量 / 消息 / 回合结束），其余 kind 先不渲染但编码可用
- [x] 设置读取：无 `gui.toml` 时回落默认值组装 overrides；有则读 `model` / `base_url` / `api_key` / `cache`（写入面板留给 05）
- [x] Tauri command 面薄壳转发：新会话 / prompt（带 Channel）/ cancel / 获取会话快照；command 层不承载逻辑
- [x] 前端转录视图：输入框（Enter 发送 / Shift+Enter 换行）+ 转录区（文本流式追加、自动跟随滚动、向上滚动暂停跟随）+ 发送/中止按钮（运行中禁用发送、启用中止）
- [x] Rust 单测（FakeProvider + `with_home(scratch)`，离线无 key）：完整回合的事件顺序、单飞互斥、cancel → Cancelled 且 partial 文本不进历史、dispose 后 prompt 报错、wire 文本路径编码正确
- [x] 手动验收：启动 → 空白会话聚焦输入框 → 输入 prompt 回车 → 流式文本逐段出现 → 中止按钮可随时停（返回 Cancelled）→ 停后渲染用量行

**Gate:** `gui/` 内 `cargo test` 全绿、fmt/clippy 干净；手动走通「prompt → 流式转录 → 中止 → 用量行」。

## Answer

Implemented: `AppController` (Idle/Running/Disposed, worker-thread single-flight prompt,
cancel→abort, dispose joins), the exhaustive `GuiWireEvent` bridge, per-prompt Tauri
`Channel<GuiWireEvent>`, and the frontend transcript with text streaming, auto-follow scroll,
Enter/Shift+Enter input and a Stop button. Unit tests cover full-turn event order, single-flight
mutex, cancel→Cancelled with partial text excluded from history, and dispose rejection.

### 实现补记：第 11 个 kind `error`

wire 桥在 spec 事件表（10 个 kind）之外多了一个 GUI 自有的 `error` kind：worker 在线程里跑同步
turn 时，失败无法像 FFI 那样通过同步返回值 `-1` + `last_error` 报出（Tauri `Channel` 是单向流），
所以失败以 `GuiWireEvent::Error { message }` 作为该轮事件流的收尾送达，前端据此离开 running 态并
显示错误。它是 GUI 宿主的 API 形状需求（ADR-0024 D4 的“宿主自有 wire 枚举”允许），不改变
`SdkEvent` → kind 的穷尽映射（`error` 不由任何 `SdkEvent` 产生）。
