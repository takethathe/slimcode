# slimcode 说明文档

> 本文件记录项目背景、核心概念、设计动机与原理。随代码变更同步维护。

## 项目背景

slimcode 是一个 Rust 编码 agent CLI：一条 prompt 在目标目录内自主执行
read / write / edit / bash / grep / find / ls 工具循环直到完成，流式输出、
token 用量统计、会话可保存/恢复（详见 `docs/development.md`）。有两个前端：
带 prompt 时跑非交互 one-shot；不带 prompt 时进入全屏 TUI（ADR-0003），两者共用
同一套渲染模型与 turn runner（ADR-0004）。

## 核心概念

### Context（构建后的消息列表）

一次 agent 运行需要一组按序排列的 `Message`（system → user → assistant →
tool → …）。这组消息在交给运行时之前，由前端无关的 `ContextBuilder`（
`slimcode-common::context`）组装：基础系统提示（默认 `DEFAULT_SYSTEM_PROMPT`
或 `with_system` 覆盖）+ 可自动调用 skill 的广告段落 + 可选 message history
+ 当轮 user prompt 或 skill 触发。`build()` 返回的 `Vec<Message>` 可直接交给
`run_agent_from_messages`，无需二次转换。

关键区分（见 `CONTEXT.md` 词条）：**Context** 是一次构建的产物；**message
history**（`session.messages`）是会话中已发生的消息，可被当作 Context 的
`with_history` 输入继续构建；**input history**（前端输入历史）与上下文组装
无关。

组装规则（所有前端入口共享，行为可预测、不漂移）：

- 新会话（空 history）首轮前置一条 system 消息；恢复的会话 history 已含
  system，不重复插入。
- 只有 `disable-model-invocation: false` 的 skill 描述进入 system 的
  `## Available skills` 段落；`true` 的 skill 仅通过显式 `/name` 触发。
- `/name` 触发时，skill 正文与 skill 目录一并成为当轮 user 消息，模型无需
  再 read `SKILL.md`，直接按正文里的相对路径引用辅助文件。
- 当轮 user 消息必须存在（`with_user_prompt` 或 `with_skill`），否则
  `build()` 报错，而不是静默产出没有 user 消息的 turn。

### DisplayItem / Renderer / run_turn（前端无关的渲染与运行 seam）

渲染与 turn 执行收敛为 `slimcode-common` 的前端无关 seam（ADR-0004）：

- **DisplayItem**：前端无关的显示单元（turn 标记、流式文本片段、思考行、工具
  开始/结果、停止标记、token 用量），由共享纯映射 `map_event(AgentEvent) ->
  Option<DisplayItem>` 从 agent 事件产出；
- **Renderer**：每前端只实现一个 `Renderer` trait 消费 DisplayItem——CLI 是
  文本行（`TextRenderer`），TUI 是 widget 状态（App 的 `Renderer` impl）；
- **run_turn**：共享 turn runner 逐事件流式回调 `&mut dyn Renderer`，返回更新后的
  消息历史，CLI 与 TUI 共用同一 turn 循环。

token 用量保持前端关注点：运行结束后前端读取其具体 provider 的 total usage，
喂一条 `DisplayItem::Usage` 给自己的 renderer。

## 设计动机

上下文组装曾散落在 CLI 两个入口（非交互 `run_once` 与交互前端的 `submit_prompt`）
各自实现一遍「system / message history / user」规则，skill 触发再绕一层，规则
重复、容易漂移。把组装收敛为 `slimcode-common::context` 的单一 `ContextBuilder`
后，one-shot 入口与 TUI 消费同一套规则，未来 Web 前端也可直接复用。

同样地，渲染与 turn 执行曾由 CLI 独占：事件→输出的映射和 turn 循环只在
`crates/cli` 里，TUI 若另写一套必然漂移。把 `map_event` / `DisplayItem` /
`Renderer` / `run_turn` 上收到 `slimcode-common`（ADR-0004）后，CLI 与 TUI
共享同一份事件到显示的映射与同一个 turn 循环，每个前端只实现自己的 `Renderer`，
行为可预测、不漂移。

**为什么 TUI 替代 REPL**（ADR-0003）：行式 REPL 无法区分 Shift+Enter 与 Enter，
多行输入只能靠 `\` 续行（ADR-0001 的取舍）；raw mode 一旦开启（TUI 的必然），
Shift+Enter 与 Enter 可区分，Enter 提交、Shift+Enter 换行，交互自然得多。同时
TUI 提供滚动 transcript、`PgUp`/`PgDn` 翻页、输入历史 recall（`↑`/`↓`）与状态行，
可维护性也更好——ratatui 的 widget 模型 + `TestBackend` 让 UI 逻辑可做帧缓冲
测试（spec：好的测试断言帧缓冲，而非内部状态）。代价是交互式前端引入
crossterm/ratatui 依赖（仅交互式前端，ADR-0001 的无依赖立场对交互前端让位）；
不带 prompt 且非 TTY 时打印明确错误并以非零码退出。
