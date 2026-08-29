# slimcode 开发文档

> 本文件记录架构、设计决策、构建与测试方法。随代码变更同步维护。

## 构建

```bash
cargo build
```

## 测试

```bash
cargo test
```

## 格式化与静态检查

```bash
cargo fmt --all
# 可选：验证格式化状态（不强制）
cargo fmt --all --check
cargo clippy --all-targets --all-features --message-format=json -- -D warnings
```

## 架构

cargo workspace，三个 crate（布局见 `.scratch/slimcode-v1` 的 map）：

| crate | 包名 | 职责 | 状态 |
| --- | --- | --- | --- |
| `crates/ai` | `slimcode-ai` | 统一 LLM provider 层（Provider trait + OpenAI-compatible/Bailian） | 起步（Bailian provider + wire 模型） |
| `crates/agent` | `slimcode-agent` | agent 运行时、工具、会话状态 | 起步（edit 引擎 + 运行时循环 + 消息模型） |
| `crates/cli` | `slimcode` | 二进制入口（非交互 + REPL） | v1 完成（配置/渲染/会话/REPL + 七工具绑定） |

### crates/ai Bailian provider

实现 `agent::Provider` seam（ticket 04/05），栈与 serde 容忍清单自 ticket 01/02/05：

- **HTTP**：`reqwest 0.13` blocking（features `json` + `blocking` + `rustls`，`default-features=false`）。
  `Provider` trait 是同步 seam，真实阻塞边界收在 provider 内部，不引入 tokio；ticket 02 文档中 `stream` feature
  仅 async 路径需要，blocking 下用 `resp.text()` 一次取回整段 SSE 再解析；
- **请求**：`stream: true` + `stream_options.include_usage: true`（ticket 05 实测 usage 只在带 `choices: []` 的最终 chunk 出现）；
  工具用 `role: tool` 消息回传结果；
- **serde 容忍清单**（全部不设 `deny_unknown_fields`，未知字段自动忽略）：
  - `reasoning_content`：思考模型每个 chunk 都带，`Option<String>`；
  - `usage`：key 每 chunk 都在但多为 `null`，`Option<WireUsage>`（当前解析后丢弃，token 记账留给 CLI）；
  - `content`/`function.name`/`function.id`：可为 `''`/`null`（思考模型空内容、tool_call 续传 `name: null`）；
  - `*_tokens_details` 等未知字段：直接忽略；
- **tool_call 拼接**：首片段带 `id`/`name`（`arguments: ""`）→ `ToolCallStart`，续传只有 `index`+`arguments` → `ToolCallArgs`，按 index 拼接；
- **配置**：`DASHSCOPE_API_KEY`（必填）、`SLIMCODE_AI_BASE_URL`（默认 `https://dashscope.aliyuncs.com/compatible-mode/v1`）、
  `SLIMCODE_AI_MODEL`（默认 `qwen-plus`）；`BailianConfig::from_env()` / `BailianProvider::from_env()`；
- 两个 `#[ignore]` 冒烟测试（文本 + 工具调用）需真实 key + 网络，默认跳过，一次性手动验证已通过。

### crates/agent 工具

- `tools::edit`：`edit` 工具引擎（纯函数，无文件 I/O）。语义锁定自 `.scratch/slimcode-v1` ticket 03：
  - 一次调用多个**不相交** edit，全部匹配**原始**内容（非增量），按 offset 逆序应用；
  - 每个 `oldText` 必须**恰好出现 1 次**（0 → not-found，>1 → not-unique，均报错）；
  - 空 `oldText`、匹配重叠、no-op（内容未变）均报错；
  - 行尾 CRLF/LF 检测与恢复、BOM 剥离/恢复；
  - **轻量 fuzzy（选项 C）**：exact 优先，找不到时仅做每行 `trim_end` 归一重试（不做 NFKC/智能引号折叠），命中后按行回映射、未触碰行保留原始字节；
  - 成功返回 `{ new_content, replaced_blocks, diff, first_changed_line }`（替换块数、带行号 diff、首个变更行）。

### crates/agent 会话模型（`session`）

消息/会话数据模型，折入自 ticket 04：

- `Message { role, parts: Vec<Part>, tool_calls, tool_call_id }`，role 序列化小写；
- `Part::Text { text }`（parts 抽象，v1 默认/唯一变体），JSON 形状 `{"type":"text","text":"..."}`；
- `ToolCall { id, name, arguments }`，`arguments` 存模型原始 JSON 串、执行时才 parse；
- `Session { id, created_at, messages, title }` 包一层元数据（为 `~/.slimcode/sessions/` 准备）；
- JSON 边界：`tool_calls`/`tool_call_id` 缺省时省略，整图无损 round-trip。

### crates/agent 运行时循环（`agent`）

折入自 ticket 04 原型，决策：

- 循环：模型带 `tool_calls` 的响应 → 执行工具 → 追加 `role: tool` 结果 → 循环，直到模型不再调工具；
- 停止：无 tool_calls → `Completed`；`max_iterations` → `MaxIterations`（运行时唯一硬保险；用户中断由 CLI 层做 cancellation）；
- 工具执行默认**串行**（本地工具引擎安全），`RunConfig.parallel_tools` 开关留给未来 IO 工具；
- 工具报错以 `Error: …` 前缀作 `role: tool` 内容进 history，模型自然恢复；
- `Provider` trait 是 crates/ai 已实现的 seam（当前同步、无 async 依赖，真实 provider 内部处理阻塞边界）；
- 流式 delta（`Reasoning`/`Text`/`ToolCallStart`/`ToolCallArgs`/`Done`）镜像 ticket 05 实测 wire 形状，`assemble` 负责拼接。

### crates/cli 二进制（`slimcode`）

两种模式，I/O 与逻辑分离（`run(args, out)` 便于测试）：

- **非交互**：`slimcode "<prompt>"`（可 `--cwd <dir>`）跑一轮七工具循环、流式渲染事件、
  打印 token 用量并保存会话；
- **REPL**：`slimcode` 进入行式循环，`/` 命令控制（`/help /new /load <id> /sessions
  /usage /save /exit`），每轮自动保存会话。

模块：

- `config`：`config.toml`（非敏感）< env 覆盖 < 默认值；API key 只来自
  `DASHSCOPE_API_KEY`；`load()` 委托 `load_from(path)` 复用文件加载逻辑；
- `render`：`AgentEvent` → 终端输出（流式文本 / 结构行 / 用量汇总），原始
  tool_call delta 与 `Done` 事件被抑制；
- `session`：`SessionStore`（`~/.slimcode/sessions/<id>.json`），id
  `slimcode-<unix>-<pid>-<n>`、created_at RFC3339 UTC（无 chrono 依赖）、标题取首条
  用户消息截断 48 字符；
- `repl`：行式循环；`messages_for_prompt` 在**新会话**首轮前置系统提示（恢复的
  会话历史已含系统消息，不重复），`/load` 经 `SessionStore::load` 恢复历史；
- `tools`：把七工具 factory 绑定到启动 `cwd`。

agent crate 的 `agent` 模块 `pub use session::{Message, Role, ToolCall}`，CLI 统一从
`slimcode_agent::agent` 引用消息类型。
