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

cargo workspace，五个 crate（布局见 `.scratch/slimcode-v1` 的 map）：

| crate | 包名 | 职责 | 状态 |
| --- | --- | --- | --- |
| `crates/ai` | `slimcode-ai` | 统一 LLM provider 层（Provider trait + OpenAI-compatible/Bailian） | 起步（Bailian provider + wire 模型） |
| `crates/agent` | `slimcode-agent` | agent 运行时、工具、会话状态 | 起步（edit 引擎 + 运行时循环 + 消息模型） |
| `crates/commands` | `slimcode-commands` | 前端无关的 `/` 命令注册表与预测提示 | v1 新增（registry + suggest/find） |
| `crates/common` | `slimcode-common` | 前端无关的应用模块（配置解析 / 会话与输入历史持久化 / skills 发现与安装 / 上下文组装 / 七工具绑定） | v1 新增（自 cli 抽出） |
| `crates/cli` | `slimcode` | 二进制入口 + 终端前端（非交互 + REPL） | v1 完成（render/repl/main） |

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
- **配置**：`BailianConfig` 为纯 provider 数据（api key / base URL / model，保留
  `chat_completions_url()`）。四层优先级解析、env 变量名与默认值（`DEFAULT_BASE_URL` /
  `DEFAULT_MODEL`）的唯一 owner 是 `slimcode-common::config`（frontend overrides > env >
  `config.toml` > 默认值），产出 `BailianConfig`；ai 不再提供 `from_env`，消除与 cli 重复
  解析同一组 env/默认值的问题；
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

### crates/commands 命令注册表（`slimcode-commands`）

前端无关的 `/` 命令定义与预测逻辑（纯数据 + 纯函数，无 I/O、无依赖）：

- `Command`：规范名 `name`、别名 `aliases`、用法串 `usage`（含参数占位，如
  `/load <id>`）、描述 `description`、匹配种类 `kind`；
- `CommandKind::Exact`（按规范名/别名精确或前缀匹配）与
  `CommandKind::Numbered`（如 `/!N`：`name` 后跟数字序列，匹配 `/!3`）；
- `COMMANDS`：命令注册表的单一事实来源（`/help`、启动 banner、未知命令提示均由此生成）；
- `find(input)`：精确解析规范名或别名到命令；
- `suggest(input)`：按前缀预测匹配命令（`/` 单独列出全部，非 `/` 输入返回空）。

这里只登记**内置**命令；安装的 **skill** 是另一组动态 `/` 触发
（`slimcode-common::skills`），前端在预测部分 `/` 输入时把两者合并（见
crates/cli 的 `combined_suggestions`）。

### crates/common 前端无关应用模块（`slimcode-common`）

自 cli 抽出的前端无关 module，任何前端（当前 REPL、未来 TUI/Web）可直接复用，不依赖终端
二进制：

- `config`：四层优先级（frontend overrides > env > `config.toml` > 默认值）的单一 owner，
  并拥有 env 变量名（`ENV_*`）与默认值（`DEFAULT_BASE_URL` / `DEFAULT_MODEL`）常量；
  `resolve(file_toml, env, overrides)` 纯解析核心 + `load_from(path, overrides)` /
  `load_with_overrides(overrides)` I/O 包装，产出 `slimcode_ai::BailianConfig`；API key 只
  来自 `DASHSCOPE_API_KEY`；`slimcode_home()` 解析 `$SLIMCODE_HOME` / `~/.slimcode`；
- `session`：`SessionStore`（`~/.slimcode/sessions/<id>.json`），id
  `slimcode-<unix>-<pid>-<n>`、created_at RFC3339 UTC（无 chrono 依赖）、标题取首条
  用户消息截断 48 字符；
- `history`：`HistoryStore`（`~/.slimcode/history.json`，JSON 数组，上限 500 条丢最旧）
  记录 `input history`（仅普通 prompt，不含 `/` 命令），与会话 `message history` 严格区分
  （见 CONTEXT.md）；
- `skills`：`Skill` 模型 + `SkillStore`（前端无关）：`SKILL.md` 的 YAML 风格
  frontmatter（`name` / `description` / `disable-model-invocation`）解析、
  user（`<home>/skills/`）与 project（`<cwd>/.slimcode/skills/`）两 scope 的
  发现（同名时 project 优先）、`install`（目录或单文件源，落为
  `<scope>/skills/<name>/SKILL.md`）、纯函数 `find_skill` / `suggest_skills` /
  `skill_prompt`；`disable-model-invocation: true` 的 skill 不进系统提示词，
  只通过显式 `/name` 触发；`Skill` 携带 `dir`（发现/安装时确定），
  `skill_prompt` 把它注入触发消息，供模型解析正文里的相对路径；
- `context`：`ContextBuilder`（前端无关）把一轮 prompt 的上下文组装收敛为单一
  入口：基础系统提示（默认 `DEFAULT_SYSTEM_PROMPT` 或 `with_system` 覆盖）+
  可自动调用 skill 广告（`with_skills`，build 时过滤 `disable-model-invocation`）+
  可选 message history（`with_history`，非空不重复插 system）+ user prompt（
  `with_user_prompt`）或 skill 触发（`with_skill`，复用 `skill_prompt`）；
  `build()` 返回可直接交给 `run_agent_from_messages` 的 `Vec<Message>`，缺
  user 时报错；空 history 前置一条 system 消息；
- `tools`：把七工具 factory 绑定到启动 `cwd`。

### crates/cli 二进制（`slimcode`）

两种模式，I/O 与逻辑分离（`run(args, out)` 便于测试）：

- **非交互**：`slimcode "<prompt>"`（可 `--cwd <dir>`、`--model <model>`、
  `--base-url <url>`）经共享 `ContextBuilder` 组装消息列表（新会话首轮前置系统
  提示并广告可自动调用 skill），跑一轮七工具循环、流式渲染事件、
  打印 token 用量并保存会话；
- **REPL**：`slimcode` 进入行式循环，`/` 命令控制（`/help /new /load <id> /sessions
  /usage /save /history /!! /!N /exit`），每轮自动保存会话。

模块：

- `render`：`AgentEvent` → 终端输出（流式文本 / 结构行 / 用量汇总），原始
  tool_call delta 与 `Done` 事件被抑制；
- `repl`：行式循环；每轮通过共享的 `ContextBuilder` 组装消息列表：**新会话**
  首轮自动前置系统提示并广告可自动调用的 skill，恢复的会话历史已含系统消息，
  不重复插 system（`disable-model-invocation: true` 的 skill 描述**不**进系统
  提示）；基础提示与 skill 段落均以 markdown 结构呈现（基础提示：`## Tools` /
  `## Working style` 小节，工具名反引号包裹、工作准则为 bullet 列表；skill
  段落：`## Available skills` 标题 + 反引号包裹 `/name` 的 bullet 列表）；
  `/load` 经 `SessionStore::load` 恢复历史；
  输入历史 `/history`（最近 20 条、最新在前、带编号）/`/!!`/`/!N` 重跑（verbatim、
  作为新一轮 prompt、不再写入历史）；多行 prompt 用行尾 `\` 续行、空行或非 `\` 行
  提交（无 readline 依赖、无 raw mode，见 ADR-0001）；纯函数 `is_continuation` /
  `strip_continuation` / `accumulate` / `parse_replay` / `resolve_replay_index` /
  `render_history` 承接测试，共享依赖收在 `ReplCtx`；`/help` 与启动 banner 由
  `slimcode-commands::COMMANDS` 生成，未知 `/` 命令用 `suggest` 给出 `did you
  mean` 预测提示；
- **skills 集成**：`ReplCtx` 持有 `SkillStore`；`/skills` 列出已安装 skill（含
  scope 与 manual-only 标记），`/install-skill <path> --user|--project` 安装
  （先 `inspect` 校验源与 name，拒绝与内置命令重名），`/name` 精确命中时把
  skill 正文（含其目录，便于解析相对路径）作为用户消息跑一轮（不写入 input
  history）；未知 `/` 命令的预测提示由 `combined_suggestions` 合并内置命令
  与 skill（`suggest_skills`）；
- 前端无关的 `config` / `session` / `history` / `skills` / `context` / `tools` 已
  移入 `slimcode-common`（见上节），cli 只消费它们，不再各自实现。

agent crate 的 `agent` 模块 `pub use session::{Message, Role, ToolCall}`，CLI 统一从
`slimcode_agent::agent` 引用消息类型。
