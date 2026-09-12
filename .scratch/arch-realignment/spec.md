# 架构成型：cli 总入口 + tui 图形库

Status: ready-for-agent

## Problem Statement

slimcode 是 6 个 crate 的 workspace，但分层与依赖方向和它想成为的东西相反：

- `crates/ai`（LLM provider）**反向依赖** `slimcode-agent`（`Provider` trait 与 `Message` 都在 agent 里），所以 provider 层无法独立存在；
- `crates/common` 同时装着应用服务（config / session / skills / context / tools / setup）与显示契约（`DisplayItem` / `map_event` / `Renderer` / `run_turn`），名字与内容不符；
- `crates/tui` 的 `terminal.rs`（844 行）直接 import config / session store / skills / history / context / provider，自己构建服务、执行 `/` 命令语义、写会话日志——它是**应用**而不是库，无法复用，也说不清"入口在哪"；
- 一个 `Message` 类型同时是 LLM wire 消息和会话存储单元，日志专用字段（`stop_reason`/`error`）挂在消息上，system prompt 被真的写进 session 与日志。

结果是："cli 是总入口、tui 是图形库"这条目标无法用依赖关系或测试表达出来。

## Solution

按 ADR-0011–0014 重新切分 crate，并让依赖方向可断言：

| crate | 目标职责（对外界面） | 依赖 |
| --- | --- | --- |
| `crates/ai` | `Message`(wire) / `Provider` / `ToolSpec` / `Delta` / `FinishReason` / `CancelToken` / `TokenUsage` / wire | 无 slimcode 依赖 |
| `crates/core` | `AgentEvent` / `AgentRunner`（loop + 可选闭包 hook 字段）/ `AgentMessage`(+`to_llm`) / `convert` / `Tool{spec,run}` / `RunConfig` / `StopReason` | → ai |
| `crates/app` | `DisplayItem` / `map_event` / `Renderer` / `usage_summary` / `ContextBuilder`→`Context{system,messages}` / SessionStore / history / skills / context_files / 七工具 / setup / `run_turn` | → ai, core, commands |
| `crates/commands` | `/` 注册表 + fuzzy（纯函数） | 无 |
| `crates/tui` | `RenderItem` / `Effect` / `App`(new/apply/draw/handle_key) / `run(terminal, app, handler)` / `UiHandler` / 组件 | **无 slimcode 依赖** |
| `crates/cli` | 唯一二进制 = 总入口：argv / 模式选择 / 配置 / 服务构建 / 命令语义 / 会话落盘 / `TextRenderer` / `TuiAdapter` / 补全与文案 | → 全部 |

- `crates/agent` → `crates/core`，`crates/common` → `crates/app`。
- `Provider` trait 与 LLM `Message` 移入 `ai`（依赖反转）；`core` 依赖 `ai`。
- 消息两层：`ai::Message`（wire）／`core::AgentMessage`（落 session，可含 compact 等非 LLM 变体），`to_llm` 转化或丢弃；system prompt 每轮现组、不进 session；`stop_reason`/`error` 移到日志记录信封。
- 显示链三层：`AgentEvent`（core）→ `DisplayItem`（app，共享映射，ADR-0004 保留）→ `RenderItem`（tui，由 cli 的 `TuiAdapter` 转换）。
- 进程生命周期（argv/raw mode/信号/panic/退出码）与应用生命周期（服务、命令语义、会话落盘、映射）全在 cli；tui 只保留 UI 机制（帧循环、输入轮询、tick、worker 线程、channel）。

详见 `docs/adr/0011..0014`、被修订的 `docs/adr/0004`（TUI 模式的 Renderer 在 cli 侧）与 `docs/adr/0009`（system 记录与 stop_reason 信封）。

## 本 spec 钉死的细节（ADR 未覆盖）

1. **`ToolSpec` 归属**：`Provider::chat(&[Message], &[ToolSpec], &CancelToken)` 里的工具参数是 `ai::ToolSpec { name, description, parameters }`（pi 的 `pi-ai` 同样拥有工具 schema）；`core::Tool { spec: ToolSpec, run }` 是带执行闭包的包装；loop 每次 run 开头构建一次 `Vec<ToolSpec>`，不在每次请求里克隆。
2. **`RenderItem` 变体集**：`Text` / `Reasoning` / `ToolStart` / `ToolResult` / `Notice` / `Error` / `UserPrompt` / `Usage(FooterUsage)` / `Skills(Vec<SkillInfo>)` / `Branch(Option<String>)` / `SessionChanged{id}`。`DisplayItem::Turn`、`DisplayItem::Stop` 由适配器丢弃。
3. **`FooterUsage` 的转换在 cli**：`impl From<&TokenUsage> for FooterUsage` 从 `tui::footer` 移到 cli（tui 不依赖 ai）。
4. **`App::new` 输入**：初态（session id、branch、skills、history）+ 注入的补全提供者；此后一切变更走 `apply(RenderItem)`。
5. **`usage_summary` 留在 app**（两个消费方都在 cli，但它是措辞而非状态）。
6. **不做日志版本升级**：system 记录与 envelope 字段都是记录级兼容（ADR-0009 D3 的宽容读）。

## Out of Scope

- 不新增任何功能：hooks 只摆 seam（默认 `None` 时行为与今天逐字节相同），不实现 compact / permission 语义。
- one-shot 文本输出保持字节不变（ADR-0004 的保证继续成立）。
- 不引入 async / tokio，不动 provider 的阻塞边界。
- 不迁移或删除遗留 `<id>.json`（配额驱逐照旧）。

## 验收

- 每个 S 步骤结束时：`cargo test` 全绿、`cargo fmt --all`、`cargo clippy --all-targets --all-features -- -D warnings` 0 error / 0 warning。
- S5 落地 `crates/cli/tests/architecture.rs`：读各 crate 的 `Cargo.toml` 断言依赖矩阵；断言 `crates/tui/Cargo.toml` 无任何 `slimcode-*` 依赖；断言 `crates/tui/src` 不出现 `SessionStore`/`SkillStore`/`Config`。
- 文档同步：`CONTEXT.md` 词条（本轮已写入）、`docs/development.md` 目标架构段（本轮已写入，S1–S5 逐步替换「现状」段）、`user-manual.md` 若出现"两个入口"的叙述需改。

## 迁移步骤（每个 issue 一个步骤）

| 步骤 | issue | 内容 |
| --- | --- | --- |
| S1 | `issues/01-rename-crates-and-invert-provider-seam.md` | 改名 + `Provider`/`Message`/`ToolSpec`/`Delta`/`FinishReason`/`CancelToken` 进 `ai` |
| S2 | `issues/02-two-layer-message-model.md` | `AgentMessage` + `to_llm` + `convert` + system 出 session + 日志信封 |
| S3 | `issues/03-tui-renderitem-and-cli-adapter.md` | `RenderItem` + `App::apply` + cli 的 `TuiAdapter` |
| S4 | `issues/04-tui-debusinessing-uihandler-seam.md` | 服务/命令语义入 cli + `UiHandler` seam + 补全注入 |
| S5 | `issues/05-dependency-matrix-test-and-docs.md` | 依赖矩阵测试 + docs 现状段删除/重写 |
