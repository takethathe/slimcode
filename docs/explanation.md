# slimcode 说明文档

> 本文件记录项目背景、核心概念、设计动机与原理。随代码变更同步维护。

## 项目背景

slimcode 是一个 Rust 编码 agent CLI：一条 prompt 在目标目录内自主执行
read / write / edit / bash / grep / find / ls 工具循环直到完成，流式输出、
token 用量统计、会话可保存/恢复（详见 `docs/development.md`）。

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
`with_history` 输入继续构建；**input history**（REPL 输入历史）与上下文组装
无关。

组装规则（所有前端入口共享，行为可预测、不漂移）：

- 新会话（空 history）首轮前置一条 system 消息；恢复的会话 history 已含
  system，不重复插入。
- 只有 `disable-model-invocation: false` 的 skill 描述进入 system 的
  `## Available skills` 段落；`true` 的 skill 仅通过显式 `/name` 触发。
- 当轮 user 消息必须存在（`with_user_prompt` 或 `with_skill`），否则
  `build()` 报错，而不是静默产出没有 user 消息的 turn。

## 设计动机

上下文组装曾散落在 CLI 两个入口（非交互 `run_once` 与 REPL `submit_prompt`）
各自实现一遍「system / message history / user」规则，skill 触发再绕一层，规则
重复、容易漂移。把组装收敛为 `slimcode-common::context` 的单一 `ContextBuilder`
后，非交互入口与 REPL 消费同一套规则，未来 TUI / Web 前端也可直接复用。
