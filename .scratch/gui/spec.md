# slimcode GUI 前端（Tauri + React 桌面应用）

Status: ready-for-agent

来源：ADR-0024（Tauri Web GUI over the SDK）。设计经 interview 打磨定型，本文档按 to-spec 模板
定稿：Tauri v2 + 内嵌 `slimcode-sdk` + React/Vite/Tailwind 浅色现代 UI，`gui/` 独立于 workspace，
宿主逻辑收敛在一个可单测的纯 Rust 控制器上（测试 seam 已与用户确认）。

## Problem Statement

slimcode 目前有两个入口：CLI 二进制（one-shot + TUI，ADR-0022）与 Rust 程序化 `slimcode-sdk`
（ADR-0021）。两者都是终端界面。用户希望有一个**基于 Web 技术的图形界面**：浅色、现代、
桌面应用形态，能像 TUI 一样驱动 agent 会话——流式转录、工具调用、中止、会话恢复——但用
鼠标点击和浏览器级渲染代替终端键位。

Tauri 是自然载体：后端是 Rust，可直接**内嵌 `slimcode-sdk`**（不走 FFI——FFI 是给 Node/Python
宿主的，Tauri 后端本身就是 Rust）。难点不在集成（SDK 同步、单飞、事件同步送达已稳定），而在于：
SDK 事件如何桥接到 webview（`SdkEvent` 不能 `Serialize`，ADR-0023 D3）、会话/线程如何组织、
设置如何持久化而不污染 CLI/TUI 共享的 `config.toml`。

## Solution

新增**第三个前端**——GUI 前端：一个 Tauri v2 桌面应用，UI 是 Web 技术（React + Vite + Tailwind，
手写组件），运行在原生窗口的 webview 里。Rust 后端（**Tauri 宿主**）直接 path 依赖
`slimcode-sdk` 与 `slimcode-app`，内嵌 agent 能力，不 spawn `slimcode` 二进制、不经过 FFI。

应用启动时打开一个新的空会话（焦点在输入框）；用户输入 prompt 后，宿主在 worker 线程上跑一轮
同步 `AgentSession::prompt`，把每个 `SdkEvent` 经 GUI wire 事件桥编码为扁平 JSON，通过 Tauri
`Channel` 流式推给前端逐条渲染（文本追加、reasoning 弱化、工具卡片、用量行）。左侧栏列出
历史会话（来自与 CLI/TUI 共享的 `SessionStore`），可恢复（`with_session_id`）、可删除；
顶部标题栏显示会话标题、模型名与上下文用量；底部是输入框与发送/中止按钮；设置面板读写
GUI 专属的 `gui.toml`（作为 SDK overrides 传入，绝不写 `config.toml`）。

宿主逻辑（会话注册表、wire 映射、设置、SessionStore 操作）收敛在一个**纯 Rust 控制器**上，
Tauri command 只是它的薄壳转发层——所有可测逻辑不依赖 Tauri 运行时。

## User Stories

1. 作为用户，我想在 macOS 上打开一个桌面应用并直接输入 prompt，以便不用终端就能使用 slimcode。
2. 作为用户，我想在启动时得到一个空的、可立即输入的新会话，以便快速开始工作。
3. 作为用户，我想看到 prompt 后流式滚动的转录（文本片段实时追加），以便感知 agent 的进展。
4. 作为用户，我想让思考（reasoning）内容以弱化样式（浅灰小字、可折叠）显示，以便区分思考与正式回答。
5. 作为用户，我想让每次工具调用渲染成一张卡片（工具名 + 状态徽标，默认折叠参数与结果，点击展开），
   以便一眼看出 agent 在调什么工具、成没成功。
6. 作为用户，我想看到运行中的工具卡片高亮、成功的绿色、失败的红色，以便快速识别回合状态。
7. 作为用户，我想在回合结束时看到 token 用量行，以便了解每次回答的成本。
8. 作为用户，我想在发生自动压缩时看到可折叠的压缩提示，以便知道历史被摘要替换了。
9. 作为用户，我想随时点"中止"停掉正在跑的一轮（包括正在执行的 bash 工具），以便出错时立刻止损。
10. 作为用户，我想在回合运行期间界面保持响应（输入框、侧栏、中止按钮都可用），以便不会被"卡死"。
11. 作为用户，我想在左侧栏看到历史会话列表（标题 + 时间），以便找到之前的对话。
12. 作为用户，我想点击左侧栏的会话条目恢复它（历史完整、可继续 prompt），以便接着上次的工作。
13. 作为用户，我想恢复在 CLI/TUI 里创建的会话（同一个会话仓库），以便三端工作流互通。
14. 作为用户，我想删除不再需要的会话（带确认），以便清理空间、保持列表整洁。
15. 作为用户，我想在顶部看到当前会话标题、模型名与上下文用量（如 `45.3%/200k`），以便了解会话状态。
16. 作为用户，我想在设置面板改模型 / base URL / API key / 缓存开关，以便指向自己的 LLM 端点。
17. 作为用户，我想让 GUI 里改的设置只影响 GUI，以便不会悄悄改变 CLI/TUI 的行为。
18. 作为用户，我想让 API key 在磁盘上不是任何人都能读（写入后权限收紧），以便减少泄露面。
19. 作为用户，我想让界面是浅色、现代、可读性好的（白/浅灰底、indigo 强调、等宽代码字体），
    以便长时间使用不疲劳。
20. 作为用户，我想让转录区自动跟随滚动到底部、向上滚动时暂停跟随，以便既不用手动拖到底、又能回看。
21. 作为用户，我想用 Enter 发送、Shift+Enter 换行，以便输入多行 prompt 时不会误发。
22. 作为用户，我想在设置里改了端点/模型后，下一次 prompt 就用新配置，以便不必重启应用。
23. 作为用户，我想在会话被 dispose / 删除后相关后台线程干净退出，以便应用退出时无泄漏。
24. 作为用户，我想在 GUI 不可用的动作（如重复提交）得到清晰的错误提示，以便知道哪里出了问题。
25. 作为开发者，我想让宿主的所有逻辑（状态机、wire 映射、设置）都能脱离 Tauri 运行时做单元测试，
    以便离线、快速、无 key 地回归。
26. 作为开发者，我想让 GUI 不拖累 workspace 的门禁与依赖矩阵，以便 agent 核心保持干净。

## Implementation Decisions

- **架构形状**（ADR-0024 D1–D4）：Tauri v2；Rust 后端（Tauri 宿主）path 依赖 `slimcode-sdk` 与
  `slimcode-app`（取 `SessionStore` / config / render 类型），不依赖任何终端 crate，不经过 FFI，
  不 spawn 二进制。前端 React + Vite + Tailwind，手写组件，不引组件库。
- **位置**（ADR-0024 D3）：顶层 `gui/` 目录，独立于 cargo workspace（自己的 Cargo.toml / lock 与
  package.json）；workspace 的 `cargo test` / clippy / `architecture.rs` 矩阵不受影响。
- **单活跃会话**（ADR-0024 D4）：宿主同一时间持有一个当前 `AgentSession`；`prompt` 时 spawn
  worker 线程跑同步单飞回合；`cancel` 只调 `abort()`（跨线程、只碰取消令牌、不等会话锁、连在途
  bash 一起杀）；`dispose` 时 join 线程并清空监听器。状态机：`Idle` / `Running` / `Disposed`；
  重复 `prompt` 报"turn already running"。
- **GUI wire 事件桥**：宿主自有的扁平枚举 `GuiWireEvent`，`encode(SdkEvent) -> GuiWireEvent` 穷尽
  match（无 `_` 分支），`DisplayItem` 新增变体即编译失败。每次 `prompt` 一个 Tauri v2
  `Channel<GuiWireEvent>`，worker 线程逐条 `send`；`SdkEvent` 永不出现在 IPC 上。事件表：
  `turn` / `reasoning` / `text` / `tool_start` / `tool_result` / `stop` / `usage` /
  `message` / `turn_finished` / `compacted`（kind 字段 + 载荷与 ADR-0023 的 FFI wire 同构，
  但枚举归 GUI 所有，不共享 import）。
- **纯 Rust 控制器（测试 seam）**：宿主逻辑收敛在 `AppController`——持有会话注册表 + 设置 +
  SessionStore，暴露 `new_session / resume / prompt(channel) / cancel / compact_now /
  list_sessions / delete_session / get_session / get_settings / set_settings`。Tauri command
  层只是转发薄壳，不承载逻辑。
- **设置**（ADR-0024 D5）：GUI 专属 `gui.toml`（slimcode_home 下），字段 `model` / `base_url` /
  `cache` / 可选 `api_key`；启动读出作为 SDK overrides（最高优先级）传入每个新会话；写入 api_key
  后文件权限收紧 0600；**绝不写 `config.toml`**。设置变更后，当前会话下一次 `prompt` 前重建
  overrides。
- **会话共享与恢复**（ADR-0024 D6）：与 CLI/TUI 共用同一 slimcode_home 的 JSONL 会话仓库；
  列表来自 `SessionStore.entries()`；恢复经 `with_session_id`（ADR-0022 D3）；删除 = 确认后移除
  JSONL 日志（当前会话先 dispose）。启动行为 = 新会话。
- **UI 布局与设计语言**（ADR-0024 D2）：左侧会话栏（列表 + 新建按钮 + 设置入口）+ 主区（顶部
  标题栏：会话标题 / 模型名 / 上下文用量徽标；中部转录区；底部多行输入框 + 发送/中止按钮）。
  浅色现代：slate 灰阶底、indigo 强调色、细边框、6–8px 圆角、柔和阴影、系统 UI 字体 +
  等宽字体（代码与转录文本）、语义状态色（成功绿 / 失败红 / 运行中强调色）。
- **转录渲染规则**：`text` 片段追加到当前 assistant 块；`reasoning` 弱化可折叠；`tool_start` /
  `tool_result` 配对成卡片（默认折叠参数/结果）；`stop` 后渲染用量行；`compacted` 渲染为可折叠
  提示；`message` 驱动侧栏与标题栏刷新。自动跟随滚动，向上滚动暂停跟随（对齐 TUI Transcript
  心智，ADR-0017）。输入快捷键 Enter 发送 / Shift+Enter 换行。
- **命名**（ADR-0024 D8）：Rust crate 与 Tauri 标识符 `slimcode-gui`，窗口/包显示名
  "slimcode GUI"。
- **配置解析**：模型/base_url/api_key/cache 若在 gui.toml 缺省，回落 SDK 四层解析
  （overrides > env > config.toml > 默认值），与 CLI/FFI 行为一致。

## Testing Decisions

- **测什么**：只测外部可观察行为，不测实现细节——会话状态机的合法/非法迁移、单飞互斥、
  cancel 路径、wire 事件映射的载荷正确性、设置文件的读写与权限、会话列表/恢复/删除的
  数据行为。Tauri command 薄壳不单独测（无逻辑）。
- **测哪些模块**：`AppController`（状态机、生命周期、设置 overrides 组装、列表/删除）、
  wire 映射纯函数（每个 `SdkEvent` 变体 → 正确 kind 与载荷，穷尽性由编译保证）、
  settings（`gui.toml` 写入 api_key 后权限 0600、缺省回落）。
- **用什么 seam**：`slimcode-sdk` 注入 seam（`with_provider(FakeProvider)` + `with_tools` +
  `with_home(scratch)`）跑所有"一轮 turn / 会话生命周期 / 恢复"的行为测试；`SessionStore` seam
  测列表/删除；新的 `AppController` seam 把宿主逻辑与 Tauri 解耦。前端 React 手动验收（v1 不
  上 Vitest）。
- **先例**（prior art）：`crates/sdk/tests/`（FakeProvider + 临时 home + 离线无 key 的行为测试）、
  `crates/ffi/tests/`（wire 映射的穷尽性测试模式）、CLI picker 对 `SessionStore.entries()` 的
  用法。GUI 测试跑在 `gui/` 项目自己的门禁下（`cargo fmt` / `clippy -D warnings` / `cargo test`），
  不进 workspace。

## Out of Scope

- **并发多会话标签页**（v2；控制器与注册表已为它留好接口）。
- **`/` 命令语义、输入历史**（前端语义，ADR-0022 D4；v1 的 prompt 按纯文本处理）。
- **技能浏览器 / 上下文文件编辑器 / 会话重命名**。
- **macOS Keychain**（api_key v1 明文 + 0600，Keychain 为 v2 增强）。
- **Windows / Linux 打包**（v1 只保证 macOS；Tauri 跨平台，后续顺手出包）。
- **前端 Vitest / 组件测试**（手动验收；Rust 测试覆盖逻辑）。
- **多窗口 / 系统托盘 / 全局快捷键**。
- **修改 `config.toml`、`slimcode config` 的 GUI 化**。

## Further Notes

- 设计定稿于 interview（grill-with-docs 三轮），全部决策记录在 **ADR-0024**（D1–D8 +
  Considered Options + Consequences）。
- GLOSSARY 已同步：`Frontend` 修订为泛化定义，新增 `GUI 前端` / `Tauri 宿主` / `gui.toml` /
  `GUI wire 事件` 四个条目；`docs/index.md` 与 `docs/development.md` 已列出本规格与 ADR。
- 实现按 repo 改动流程推进（需求 → 计划 → TDD → 测试 → 文档 → review → 提交），门禁在
  `gui/` 项目内跑；Rust 侧测试先行（wire 映射 → 控制器状态机 → 设置），随后搭 Tauri 骨架
  与 React UI，最后手动验收清单逐条过。
- 术语：本文档中的"宿主 / 控制器 / wire 事件 / gui.toml"均按 GLOSSARY 定义使用；遇到与
  既有语言冲突的表述以 GLOSSARY 为准。
