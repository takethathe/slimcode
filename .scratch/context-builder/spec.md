# Context builder — 上下文组装模块下沉 common

Status: ready-for-agent

## Problem Statement

slimcode 的上下文组装逻辑目前散落在 CLI 层，规则重复、容易漂移：

- `crates/cli/src/main.rs`：`BASE_SYSTEM_PROMPT` + `build_system_prompt(skills)`
  （基础 system + 可自动调用 skill 列表），非交互模式 `run_once` 手工拼
  `[system, user]`。
- `crates/cli/src/repl.rs`：`messages_for_prompt(messages, system, prompt)`
  （空历史插入 system，再追加 user）。
- `crates/common/src/skills.rs`：`skill_prompt(skill, arg)` 生成 skill 触发的
  user 消息，但触发路径又回到 CLI 手工拼消息。

两个入口（非交互 `run_once` 与 REPL `submit_prompt`）各自实现了一遍「system /
message history / user」的组装，skill 触发再绕一层。未来 TUI / Web 前端想复用
同一套组装规则时无处可依。

## Solution

把上下文组装收敛为 `slimcode-common` 里的一个前端无关 `context` 模块，对外提供
`ContextBuilder`：用流式 builder 方法按需放入组件（system 基础提示、skills 列表、
message history、user prompt 或 skill 触发），`build()` 返回可直接交给
`run_agent_from_messages` 的 `Vec<Message>`。基础系统提示词文本一并下沉到 common，
builder 提供默认 system。CLI 的两个入口改为消费同一个 builder，agent crate 不动。

## User Stories

1. 作为非交互用户，我提交一个 prompt 时系统按统一规则生成消息列表，以便与 REPL 行为一致。
2. 作为 REPL 用户，新会话第一轮自动插入 system 消息，以便 agent 拥有工具与工作准则的基础上下文。
3. 作为 REPL 用户，恢复已有会话后继续提交 prompt 时不重复插入 system 消息，以便消息历史不重复。
4. 作为 skill 作者，可自动调用的 skill（`disable-model-invocation: false`）描述被追加进 system 提示，以便 agent 能自动发现并选用。
5. 作为 skill 作者，`disable-model-invocation: true` 的 skill 描述不进 system 提示，以便它只能被显式 `/name` 触发。
6. 作为 REPL 用户，用 `/name` 触发 skill 时 skill 正文成为当轮 user 消息，以便按 skill 指令执行。
7. 作为前端开发者，我能自定义 system 基础提示，以便未来 TUI / Web 前端替换默认文案。
8. 作为前端开发者，我能从一段已有 message history 继续构建，以便继续一个进行中的会话。
9. 作为前端开发者，我能以流式 builder 方式按需放入组件，以便组件天然可选（无 skill、空 history）。
10. 作为前端开发者，`build()` 返回 `Vec<Message>`，以便直接交给 agent 运行时，无需二次转换。
11. 作为使用者，所有入口共享同一套组装规则，以便行为可预测、不再漂移。

## Implementation Decisions

- 新增模块 `slimcode-common::context`（单文件 `context.rs`），导出 `ContextBuilder`；
  在 `lib.rs` 注册 `pub mod context`。
- `DEFAULT_SYSTEM_PROMPT` 常量（原 CLI `BASE_SYSTEM_PROMPT` 的完整文本）下沉到该
  模块，作为 builder 的默认 system；`with_system` 可覆盖。
- `ContextBuilder` 为流式 builder（组件可选），字段：`system: Option<String>`、
  `skills: Vec<Skill>`、`history: Vec<Message>`、`user: Option<String>`。API：
  - `ContextBuilder::new()`：默认 system 取 `DEFAULT_SYSTEM_PROMPT`。
  - `with_system(impl Into<String>) -> Self`：覆盖基础 system。
  - `with_skills(&[Skill]) -> Self`：存放 skill 列表（build 时过滤掉
    `disable_model_invocation: true` 的项）。
  - `with_history(Vec<Message>) -> Self`：传入 message history。
  - `with_user_prompt(impl Into<String>) -> Self`：设置当轮 user 消息。
  - `with_skill(&Skill, Option<&str>) -> Self`：复用 `skills::skill_prompt` 生成
    user 消息，不重复实现。
  - `build(self) -> Result<Vec<Message>, String>`：未设置 user 时返回错误；否则
    组装并返回消息列表。
- `build()` 语义（与现有 `messages_for_prompt` 对齐）：
  - 组装最终 system 文本 = 基础 system（默认或 `with_system` 覆盖）+ skills 段落
    （`## Skills` + 每 skill 一行 `- name: description [Read from <file>]` bullet，仅含
    `disable_model_invocation: false` 的 skill；markdown 结构与现状一致）。
  - `history` 为空 → 前置 `Role::System` 消息；`history` 非空 → 不重复插入 system。
  - 末尾追加 `Role::User` 消息（user prompt 或 skill 触发内容）。
- CLI 改动（仅 cli，不动 agent crate）：
  - 删除 `main.rs` 的 `BASE_SYSTEM_PROMPT` / `build_system_prompt`，`run_once`
    改为 `ContextBuilder::new().with_skills(&skills).with_user_prompt(&prompt).build()`。
  - 删除 `repl.rs` 的 `messages_for_prompt`，`submit_prompt` / `submit_skill`
    改为同一 builder（`submit_skill` 走 `with_skill`）。
- `slimcode-agent::agent::run_agent` 保持不动：agent crate 不依赖 common，避免
  依赖环；builder 是前端上下文组装层，放 common、由 cli 消费。

## Testing Decisions

- 唯一新 seam 是 `ContextBuilder::build()`：单元测试直接对 `build()` 输出断言，
  不断言 builder 内部字段（外部行为优先）。
- 单元测试在 `slimcode-common::context` 覆盖：默认 system、`with_system` 覆盖、
  skills 广告与 `disable_model_invocation` 过滤、skills 段落 markdown 结构、
  空 history 插入 system、非空 history 不重复插入、`with_user_prompt`、
  `with_skill`（含 `arg` 参数）、缺 user 时 `build()` 报错。
- CLI 集成测试沿用现有 seam，不新增：repl 的脚本化 `run`（新会话插 system、
  续会话不重复、`/name` 触发分派）与 main 的 `run`（非交互入口）。原
  `messages_for_prompt` / `build_system_prompt` 的单测迁移到 `context` 模块。
- Prior art：`common::skills` 的临时目录端到端测试；`.scratch/slimcode-v1` spec
  06 的脚本化 `run` 测试（temp dir + captured output + `127.0.0.1:9` 拒绝连接
  证明触发走了 prompt turn）。

## Out of Scope

- `input history`（仍属 `common::history`，与上下文组装无关）。
- `slimcode-agent` crate 的任何改动（`run_agent` / `Provider` / 会话模型不动）。
- 把 skill 暴露为模型可调用的 tool。
- provider / wire 层改动。
- 新增 ADR（本次为整合/移动既有逻辑，可逆且不惊讶，不满足 ADR 三条件）。

## Further Notes

- 术语（见 `CONTEXT.md`，本特性需补词条）：`Context`（组装后的消息列表）不等于
  `message history`（`session.messages`）也不等于 `input history`（REPL 输入历史）。
  代码、测试、文档统一用 `context` 指代 builder 产物，`message history` 指代
  会话消息历史。
- 文档同步：`docs/development.md`（common 模块表 + cli 消费方式）、
  `docs/explanation.md`（核心概念）、`CONTEXT.md`（词条）。
- 验收：`cargo test` 全绿；`cargo fmt --all`；`cargo clippy --all-targets
  --all-features --message-format=json -- -D warnings` 0 error / 0 warning。
