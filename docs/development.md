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

> **实施状态**：ADR-0011–0014（crate 分层与 cli 总入口 / 两层消息模型 / TUI 运行 seam / TUI 零依赖与 cli 侧适配器）已决策、**尚未实施**。迁移分六张 ticket（`.scratch/arch-realignment/issues/01..06`）。下面「目标架构」是设计基准（先读它）；「现状」描述当前代码，ticket 06 完成后删除并按新布局重写各模块段。

### 目标架构（ADR-0011–0014）

cargo workspace，六个 crate，唯一二进制 `slimcode`：

| crate | 包名 | 目标职责（对外界面） | 依赖 |
| --- | --- | --- | --- |
| `crates/ai` | `slimcode-ai` | LLM 层：`Message`（wire 消息）/ `Provider` / `ToolSpec` / `Delta` / `FinishReason` / `CancelToken` / `TokenUsage` / wire 模型 | 无 slimcode 依赖 |
| `crates/core` | `slimcode-core` | agent 运行时：`AgentEvent` / `AgentRunner`（loop + 可选闭包 hook 字段）/ `AgentMessage`(+`to_llm`) / `convert` / `Tool{spec,run}` / `RunConfig` / `StopReason` | → ai |
| `crates/app` | `slimcode-app` | 前端无关应用层：`DisplayItem` / `map_event` / `Renderer` / `usage_summary` / `ContextBuilder`→`Context{system,messages}` / 会话与输入历史持久化 / skills / context_files / 七工具 / setup / `run_turn` | → ai, core, commands |
| `crates/commands` | `slimcode-commands` | `/` 命令注册表 + fuzzy 预测（纯数据 + 纯函数，无 I/O） | 无 |
| `crates/tui` | `slimcode-tui` | 终端图形库：`RenderItem` / `Effect` / `App`(new/apply/draw/handle_key) / `run(terminal, app, handler)` / `UiHandler` / 组件（theme/markdown/toolcall/footer/text/git） | **无 slimcode 依赖** |
| `crates/cli` | `slimcode` | 唯一二进制 = 总入口：argv / 模式选择（one-shot 文本 vs 交互 TUI）/ 配置解析 / 服务构建 / 命令语义 / 会话落盘 / `TextRenderer` / `TuiAdapter` / 补全与文案 | → 全部 |

重命名：`crates/agent` → `crates/core`、`crates/common` → `crates/app`。关键依赖反转：`Provider` trait 与 LLM `Message` 由 `ai` 拥有（现状是 `ai` 反向依赖 `agent`）。

依赖方向由测试断言（ticket 06，`crates/cli/tests/architecture.rs`）：`ai` 无 slimcode 依赖；`core` → 仅 `ai`；`app` → `ai`/`core`/`commands`；`tui` 无 slimcode 依赖；`cli` → 全部；`tui` 源码不得出现 `SessionStore`/`SkillStore`/`Config`。

**现状（迁移前，ticket 06 后删除）**：

cargo workspace，六个 crate：

| crate | 包名 | 职责 | 状态 |
| --- | --- | --- | --- |
| `crates/ai` | `slimcode-ai` | 统一 LLM provider 层（Provider trait + OpenAI-compatible/Bailian） | 起步（Bailian provider + wire 模型） |
| `crates/agent` | `slimcode-agent` | agent 运行时、工具、会话状态 | 起步（edit 引擎 + 运行时循环 + 消息模型） |
| `crates/commands` | `slimcode-commands` | 前端无关的 `/` 命令注册表与预测提示 | v1 新增（registry + suggest/find） |
| `crates/common` | `slimcode-common` | 前端无关的应用模块（配置解析 / 会话与输入历史持久化 / skills 发现与安装 / 上下文组装 / 七工具绑定 / 共享渲染模型与 turn runner / setup seam） | v1 新增（自 cli 抽出 + TUI 共享 seam） |
| `crates/tui` | `slimcode-tui` | 交互式全屏 TUI（纯 App core + crossterm/ratatui 终端循环；theme/markdown/toolcall/footer/git 纯函数模块） | v1 完成（pi 对齐：header/blocks/layout/footer/status） |
| `crates/cli` | `slimcode` | 二进制入口 + 非交互 one-shot 前端（共享 runner + TextRenderer） | v1 完成（render/main） |

### crates/ai Bailian provider

实现 `agent::Provider` seam（ticket 04/05），栈与 serde 容忍清单自 ticket 01/02/05：

- **HTTP**：`reqwest 0.13` blocking（features `json` + `blocking` + `rustls`，`default-features=false`）。
  `Provider` trait 是同步 seam，真实阻塞边界收在 provider 内部，不引入 tokio；ticket 02 文档中 `stream` feature
  仅 async 路径需要。body 按 chunk 可中断读取（ticket 07：`read_body_interruptibly` 每块检查 `CancelToken`，
  中途取消返回 `Cancelled(partial)`，provider 对已收到的完整 SSE 事件做 salvage 解析，把已流式内容交回 runner；
  阻塞 reqwest 没有 per-read 超时——静默服务器仍由客户端 300s 整体超时兜底）；
- **请求**：`stream: true` + `stream_options.include_usage: true`（ticket 05 实测 usage 只在带 `choices: []` 的最终 chunk 出现）；
  工具用 `role: tool` 消息回传结果；声明了 tools 时额外带 `parallel_tool_calls: true`
  （默认开启，见 ADR-0010），`tools` 为空时该字段省略、请求字节与旧版一致；
- **显式上下文缓存**（llm-cache）：`BailianConfig.cache`（默认 `true`，`with_cache(bool)` 建造式 setter）。
  开启时 system 消息的 `content` 序列化为单元素块数组
  `[{"type":"text","text":"…","cache_control":{"type":"ephemeral"}}]`，把稳定前缀交给端点缓存；
  关闭时字节与未开启缓存的客户端完全一致。`message_to_wire(m, cache)` 只对 system（且文本非空）加标记，
  assistant 工具调用空 content、tool 必带 content、空文本省略等既有语义不变；
- **usage 缓存统计**（llm-cache）：`TokenUsage` 新增可选嵌套 `prompt_tokens_details`
  （`cached_tokens` / `cache_creation_input_tokens`，缺省视为 0；整块缺省为 None），
  访问器 `cached_tokens()` / `cache_creation_tokens()` 缺省返回 0；`accumulate_usage` 把两个缓存字段
  随 prompt/completion/total 一起并入 `total_usage`；汇总行措辞由
  `common::render::usage_summary` 共享（cli 与 TUI 各渲染点都消费它，两端不漂移）；
- **serde 容忍清单**（全部不设 `deny_unknown_fields`，未知字段自动忽略）：
  - `reasoning_content`：思考模型每个 chunk 都带，`Option<String>`；
  - `usage`：key 每 chunk 都在但多为 `null`，`Option<TokenUsage>`（解析后进入 `last_usage` / `total_usage`）；
  - `content`/`function.name`/`function.id`：可为 `''`/`null`（思考模型空内容、tool_call 续传 `name: null`）；
  - `prompt_tokens_details`：可选嵌套，缺省视为 0（未命中/未开缓存时端点可能不带该块）；其它
    `*_tokens_details` 等未知字段：直接忽略；
- **tool_call 拼接**：首片段带 `id`/`name`（`arguments: ""`）→ `ToolCallStart`，续传只有 `index`+`arguments` → `ToolCallArgs`，按 index 拼接；
- **配置**：`BailianConfig` 为纯 provider 数据（api key / base URL / model / cache，保留
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

消息/会话数据模型，折入自 ticket 04（ADR-0009 起扩展了日志专用字段）：

- `Message { role, parts: Vec<Part>, tool_calls, tool_call_id, stop_reason, error }`，
  role 序列化小写；`stop_reason`（`stop`/`tool_calls`/`error`/`aborted`）与 `error`
  是日志专用字段（`skip_serializing_if` 省略、不出现在 LLM wire 上），普通消息的
  序列化字节与旧 JSON 完全一致；
- `Part::Text { text }`（parts 抽象，v1 默认/唯一变体），JSON 形状 `{"type":"text","text":"..."}`；
- `ToolCall { id, name, arguments }`，`arguments` 存模型原始 JSON 串、执行时才 parse；
- `Session { id, created_at, messages, title }` 包一层元数据（为 `~/.slimcode/sessions/` 准备）；
- JSON 边界：`tool_calls`/`tool_call_id` 缺省时省略，整图无损 round-trip。

### crates/agent 运行时循环（`agent`）

折入自 ticket 04 原型，决策：

- 循环：模型带 `tool_calls` 的响应 → 执行工具 → 追加 `role: tool` 结果 → 循环，直到模型不再调工具；
- 停止：无 tool_calls → `Completed`（coding agent 无迭代上限，何时结束由模型决定）；
  取消（ticket 07）→ `StopReason::Cancelled`——`CancelToken`（`Arc<AtomicBool>`，`new`/`cancel`/
  `reset`/`is_cancelled`/`Clone`）由每个 run 入口携带，在每个 runner 边界检查：provider chat 之前、
  chat 返回 Err 时若 flag 置位视为静默 Cancelled 而非错误、已流出的 deltas 之后（半段文本不落 history）、
  每个工具派发前后（串行/并行）。已 push 的工具结果保留；
- 工具执行默认**并行**（ADR-0010）：一个 Tool batch 内的调用各在 scoped thread 上真并发，
  事件按完成顺序流出、结果按模型返回顺序（`tool_calls` 的 index）进 history；取消在批次应用前
  检查，已 push 的结果保留、未应用的不追加。`RunConfig.parallel_tools` 默认 `true`，
  串行路径保留给显式 `parallel_tools: false` 的场景（串行语义测试）；`Tool.run` 约束为 `Fn + Send + Sync`
  （工作线程共享同一组工具只读调用）；
- 工具事件（`ToolStart`/`ToolResult`）携带 `tool_call_id`，渲染端据此配对同名工具的多次调用；
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
- `suggest(input)`：按前缀预测匹配命令（`/` 单独列出全部，非 `/` 输入返回空）；
- `fuzzy`（ADR-0005）：通用子序列模糊匹配器 `fuzzy_match(query, text) -> Option<i64>`，
  **分数越低越优**——连续命中（每字符 −50）、词边界（`-_. /:` 前，−100）、精确匹配（−1000）加分，
  间隔（每字符 +20）与靠后位置（+1/索引）减分，启发式整数缩放自 pi 的 `fuzzy.ts`；
  纯函数、无依赖、可独立单测，供 `/` 补全弹框的候选排序使用。

这里只登记**内置**命令；安装的 **skill** 是另一组动态 `/` 触发
（`slimcode-common::skills`），前端在预测部分 `/` 输入时把两者合并
（`combined_suggestions`，同样在 `slimcode-common::skills`）。

### crates/common 前端无关应用模块（`slimcode-common`）

自 cli 抽出的前端无关 module，任何前端（one-shot CLI、TUI、未来 Web）可直接复用，不依赖终端
二进制：

- `config`：四层优先级（frontend overrides > env > `config.toml` > 默认值）的单一 owner，
  并拥有 env 变量名（`ENV_*`）与默认值（`DEFAULT_BASE_URL` / `DEFAULT_MODEL`）常量；
  `cache` 项（llm-cache）同样四层解析：`--cache`/`--no-cache` override（`Overrides.cache`）>
  `SLIMCODE_AI_CACHE`（`true`/`false`/`1`/`0`/`yes`/`no`/`on`/`off`，大小写不敏感，非法值启动报错
  指明变量名）> `config.toml [ai] cache` > 默认 `true`（默认开启）；与 `base_url`/`model`
  逐项独立回落；
  `resolve(file_toml, env, overrides)` 纯解析核心（返回 `(BailianConfig, ApiKeySource)`）
  + `load_app_config(overrides)` I/O 包装，产出 `AppConfig { provider, api_key_source,
  sessions_max_bytes }`；provider 携带 `ApiKeySource`（Cli | Env | File）标记；API key 三来源
  优先级 `--api-key`（`Overrides.api_key`）> `DASHSCOPE_API_KEY` > `[ai] api_key`
  （`FileAi.api_key`，serde optional），三者皆缺报错且措辞同时指向 env 与 config.toml；
  `slimcode config` 的纯函数核心也在此：`read_ai_fields(existing)`（读当前 `[ai]` 值作
  交互默认）+ `merge_config_toml(existing, answers)`（基于 `toml::Value` 的整文档合并：
  只覆盖非空填写项、空回答保留现有值、不碰 cache，未触碰的表/键保留，仅注释在
  TOML 重写中丢失），供 CLI 子命令消费；`slimcode_home()` 解析 `$SLIMCODE_HOME` /
  `~/.slimcode`；会话配额只走文件与默认两层：`[sessions] max_mb`（`FileSessions.max_mb`，
  默认 `DEFAULT_MAX_MB = 500` MiB，无 env/CLI 覆盖）→ `resolve_max_mb(file_toml)` +
  `max_mb_to_bytes(mb)` 换算为字节；
- `session`：项目分区的 `SessionStore`（`~/.slimcode/sessions/<project-key>/<id>.jsonl`，
  ADR-0009 追加式 JSONL 日志），project key = project home 的 basename + 全路径
  FNV-1a hash 前 12 hex（`project_key()`，无外部依赖、跨版本稳定）；id
  `slimcode-<unix>-<pid>-<n>`、created_at RFC3339 UTC（无 chrono 依赖）、标题取首条
  用户消息截断 48 字符；`append`（首个 assistant 消息时独占建日志：头 + title + 全量
  backlog，此后每条一行的追加，之前为 no-op）、`append_title`、宽容的 `load`
  （未知/损坏记录跳过并计数、残缺尾行丢弃并补换行、悬空的工具调用批次在内存补
  `Error: interrupted`，磁盘字节不改写）、`list` 只作用于当前项目子目录；每次追加后
  `evict_over_quota` 以 mtime 最旧优先删到配额一半（`.jsonl` 与遗留 `.json` 一起计数，
  `DEFAULT_MAX_BYTES`，`with_max_bytes` 覆盖，跳过当前 session，删空目录，尽力而为），
  `cleanup_empty` 在启动时静默清理当前项目内重放不到任何 assistant 消息的 `.jsonl`；
- `history`：`HistoryStore`（`~/.slimcode/history.json`，JSON 数组，上限 500 条丢最旧）
  记录 `input history`（仅普通 prompt，不含 `/` 命令），与会话 `message history` 严格区分
  （见 CONTEXT.md）；
- `skills`：`Skill` 模型 + `SkillStore`（前端无关）：`SKILL.md` 的 YAML 风格
  frontmatter（`name` / `description` / `disable-model-invocation`）解析、
  user（`<home>/skills/`）与 project（`<cwd>/.slimcode/skills/`）两 scope 的**递归深搜**
  发现（任意深度含 `SKILL.md` 的目录都是 skill，分类目录逐层穿透；skill 目录本身不
  下钻；同 scope 同名时最浅目录优先、路径排序保证确定性；同名时 project 优先）、
  `install`（目录或单文件源，落为
  `<scope>/skills/<name>/SKILL.md`）、纯函数 `find_skill` / `suggest_skills` /
  `skill_prompt` / `format_skills_for_prompt` / `normalize_skill_trigger`；`disable-model-invocation: true` 的
  skill 不进系统提示词，只通过显式 `/skill:name` 触发；`Skill` 携带 `dir`（发现/安装时
  确定）与 `file`（`SKILL.md` 文件路径，用于 `<location>`）；`skill_prompt` 以 pi 风格
  `<skill name location>` XML 块把正文注入触发消息并附 `References are relative to
  <dir>.` 行，`already_loaded` 时正文换成去重提示；`format_skills_for_prompt` 产出
  `## Skills` markdown 广告索引（每 skill 一行 `- name: description [Read from
  <file>]`，含“按名字/描述匹配即用，或显式 `/{name}` 引用”的说明）；
  `normalize_skill_trigger` 把裸 prompt 开头的 `/skill:name` 改写为 `/{name}`（one-shot CLI
  无命令解析，避免 `/skill:` 前缀原样进 LLM）；`find_skill` / `suggest_skills` 兼容 `/skill:name`
  与裸 `/name` 两种拼写，补全池统一用 `/skill:name`；
- `/` 补全（ADR-0005）：`CompletionItem { value, description }` + `complete(input,
  skills) -> Vec<CompletionItem>`——把 `slimcode-commands` 的每个命令拼写（规范名 + 别名）
  与每个已安装 skill 合成候选池，用 `fuzzy::fuzzy_match` 模糊排序（裸 `/` 按注册表顺序列全部，
  命令在前、skill 在后；非 `/` 输入返回空）；候选 `value` 是命令的裸拼写（`/usage`、`/resume`）
  与 skill 的规范触发 `/skill:name`，**不含** `usage` 的参数占位符，提交时经
  `find`/`find_skill` 可解析；skill 只按**裸名字**参与模糊打分（`/skill:` 前缀是纯拼写，
  若一起打分会让 `s`/`k`/`i`/`l` 等前缀字母命中所有 skill、并淹没名字自身的边界奖励），
  输入里的 `/skill:name` 触发拼写也会先剥掉 `skill:` 前缀再按名字匹配；弹框高亮：用户未
  用 `↑`/`↓`/`PgUp`/`PgDn` 移动时，每个按键后高亮自动跟随重排后的最佳候选，只有手动移动过
  才在后续输入里粘住当前选中值（直到该值被过滤掉或弹框关闭）；
- `context`：`ContextBuilder`（前端无关）把一轮 prompt 的上下文组装收敛为单一
  入口：基础系统提示（默认 `DEFAULT_SYSTEM_PROMPT` 或 `with_system` 覆盖）+
  可选环境信息（`with_environment`，注入 `## Environment` markdown 章节：OS /
  global home / project home，位于基础提示之后、上下文文件之前；不调用则
  整个章节省略，默认提示词保持逐字节不变）+
  可自动调用 skill 广告（`with_skills`，build 时经 `format_skills_for_prompt` 过滤
  `disable-model-invocation`，产出 `## Skills` markdown 索引）+
  可选 message history（`with_history`，非空不重复插 system）+ user prompt（
  `with_user_prompt`）或 skill 触发（`with_skill`，build 时扫描 history 中是否已有
  `<skill name="..."` 标记来决定是否去重，再调用 `skill_prompt`）；
  `build()` 返回可直接交给 `run_agent_from_messages` 的 `Vec<Message>`，缺
  user 时报错；空 history 前置一条 system 消息；
- `context_files`：`AGENTS.md` 上下文文件的发现与渲染（对齐 pi 的项目上下文加载）：
  先读全局 `<home>/AGENTS.md`（scope `global`）；project 只判定两个位置——cwd 自身
  与 **git 仓库根**（最近的含 `.git` 条目的祖先目录，`.git` 可以是目录或
  `gitdir:` 文件标记，兼容 worktree/submodule），按 git 根在前、cwd 在后的顺序
  （scope `project`），按规范化路径去重（cwd 嵌套在 home 下时全局文件不再重复
  作为 project）。`format_context_files` 把它们渲染成 `## Project context` markdown
  章节（对齐 `## Skills` / `## Tools` 的标题层级），每个文件一个
  `<project_instructions path scope>` XML 块包裹内容（`scope="global|project"`
  标注意图，XML 块隔离内容，防止 AGENTS.md 内部的 `#` 标题/列表与外层 markdown
  冲突），段首声明项目要求可覆盖全局要求；没有可注入的 AGENTS.md 时整个章节
  省略，system 与未启用该功能时逐字节一致；注入位置在基础 system 之后、
  `## Skills` 索引之前；发现逻辑无失败路径（文件缺失即返回空列表）。
  同模块的 `resolve_project_home(cwd, user_home)` 供环境信息复用同一套 git 根
  发现：取最近含 `.git` 的祖先，无则回退到 OS 用户主目录（`$HOME`），再无则回退
  到 cwd 自身。
- `tools`：把七工具 factory 绑定到启动 `cwd`。
- `render`（ADR-0004）：前端无关的显示模型。`DisplayItem` 是渲染单元（turn 标记 /
  流式文本片段 / 思考行 / 工具开始与结果 / 停止标记 / token 用量）；
  `map_event(AgentEvent) -> Option<DisplayItem>` 是事件→显示单元的共享纯映射；
  `usage_summary(TokenUsage) -> String` 是 token 用量汇总行（含 `({cached} cached, {pct}%)`
  与缓存命中百分比）的共享措辞，cli 汇总与 TUI `/usage` 都消费它；
  `Renderer` trait 消费 `DisplayItem`，每个前端只实现自己的渲染器（cli 的文本行、
  tui 的 widget 状态）。
- `runner`：共享 turn runner `run_turn(provider, tools, messages, &RunConfig,
  &mut dyn Renderer, &mut dyn FnMut(&Message))
  -> Result<(Vec<Message>, StopReason), String>`（ADR-0009 D5 加 `on_message` 回调与
  `StopReason` 返回值）：逐事件流式回调渲染器，每条进历史的消息（assistant 回复、
  每个工具结果）同步回调一次 sink（TUI 借此实时追加日志），返回更新后的消息历史与
  终止原因（`Completed`/`Cancelled`）。cli 与 tui 共用同一 turn 循环，行为不漂移。
- `setup`：`setup(cwd, config) -> (BailianProvider, Vec<Tool>)` 共享 seam，cli 与
  tui 用同一套 provider + 工具构造，两端不会各自实现而漂移。

### crates/tui 交互式 TUI（`slimcode-tui`）

`crates/tui` 提供交互式全屏 TUI（ADR-0003），替代行式 REPL。模块：
`theme`（pi dark.json 词法转的只读 token 表：`Token::color()` 前景 / `BgToken::color()`
背景）、`text`（折行/截断/宽度，CJK 双宽）、`markdown`（pulldown-cmark → 样式
span，代码围栏行映射等）、`toolcall`（内置工具紧凑调用标题 composer，pi `format*Call`
移植：`CallPart` 纯函数 + 逐工具单测）、`footer`（pi `footer.ts` 的 `formatTokens` /
`formatCwdForFooter` /
stats 纯函数移植）、`git`（`terminal_title` / `current_branch` 纯包装）、`app`（纯
reducer + draw）、`terminal`（薄壳 + worker-thread runner）。分层：

- **主题（ADR-0006 D1）**：唯一风格来源是 pi `dark.json` 的逐字十六进制；TUI 渲染只
  引用 token（`Token` 前景 / `BgToken` 背景），不出现裸颜色。现有测试把每个 token
  的 hex 钉死，换肤只需改一处表。
- **块感知 transcript（ADR-0006 D2）**：App 持有 `Vec<Entry>`（`Header` /
  `UserPrompt` / `Assistant` / `Thinking` / `Tool` / `Notice` / `Error`），不再是
  扁平的逐 kind 行；流式文本/思考相邻片段仍按“同 kind 合并”规则拼进同一条目。
  用户消息是 `userMessageBg` 整块背景的 boxed markdown；assistant 文本/思考按
  pi markdown token 渲染（thinking italic 灰）；工具调用由 start/result 配成单个
  状态色块（pending 深灰 / success 暗绿 / error 暗红背景，色带横贯整行宽度；内置工具的
  头部是 `toolcall` 模块产出的紧凑调用标题 —— `read <path>:<range>` / `ls <path>` /
  `grep /pattern/ in <scope>` / `$ command` 等，不再显示 JSON 参数区；未知工具保留
  bold 名 + pretty JSON 兜底；标题下方灰色输出，超过 `TOOL_PREVIEW_LINES=10` 折叠为
  `… (N more lines, Ctrl+O to expand)`，`Ctrl+O` 全局展开）；启动头部是 transcript 顶部的
  `Header` 条目（bold accent `slimcode` + dim ` v<version>` + 一行 dim 快捷键提示）；
  错误红字、notice dim；**不**渲染 turn 标记 / `done` 行 / 每轮用量行。`/new`、
  `/load` 清空后重建会话视图。`/` 补全弹框是 SelectList 裸行样式（无边框、无标题，顶部一条
  全宽 `─` 分隔线（border 色）把弹框与上方 transcript 隔开；选中行
  `→` + accent、无反色，描述 muted，滚动标记 `(i/n)` muted），显示在输入框正上方。
- **纯 App core（`app`）**：前端无关、无 I/O 的 reducer。持有 transcript、输入框、
  `history`（input history 快照）、`recall` 态、`completion`、`scroll` /
  `scrollbar_ticks`（auto 模式滚动条：出现后 ~1s 淡出，与 scroll 位置无关）、
  `status`（`cwd` / `session_id` / `branch` / `usage: FooterUsage` / `running` /
  `spinner_frame`）、全局 `tool_output_expanded`、`version`。`handle_key` /
  `handle_key_running` / `tick()`（推进 spinner 帧、递减滚动条淡出计数）是纯
  reducer；`Effect` 枚举（Submit / ReplayPrompt / ReplayHistory / NewSession /
  LoadSession / ShowUsage / Quit / QuitAfterTurn / …）由终端循环兑现。`draw` 用
  ratatui `TestBackend` 做帧缓冲测试（spec：好测试断言**帧缓冲**而非内部状态）。
  布局是 ADR-0007 D4 四区 dock：`[transcript(Min0) | popup(0|n) | input |
  footer(2)]`；补全弹框（0 行时收起）位于输入框正上方，开合只吃 transcript、不移动输入框；
  编辑器为无左右竖线/圆角的上下两条全宽 `─` 横线，边框色蓝色（`border`）闲置 / 青色
  （`borderAccent`）运行中，右缘滚动条 thumb。命令解析经
  `slimcode_commands::find` + skill 触发 + 编号重跑（`/!N`）。输入历史 recall：输入框
  为空时 `↑`/`↓` 进入（最新一条开始），`Enter` 把选中的历史 prompt 作为新一轮重跑
  （不再写入历史）；每轮提交时追加（不查重，同 `HistoryStore::append`）。
- **Footer / 状态指示器（ADR-0006 D5/D6）**：两行 dim footer 由纯函数拼装——第一行
  `~/cwd (branch) • session`（`footer::format_cwd_for_footer`：只在词法上位于 `$HOME`
  内时缩写为 `~` / `~/rel`），第二行 `stats_line`（`↑in ↓out Rcache WcacheWrite
  CH{pct}%`，零值省略；`format_tokens` 与 pi 同表：<1000 原样、<10k `x.xk`、<1M 取整
  `xk`、<10M `x.xM`、否则取整 `M`），模型名右对齐，宽度不足时右侧截断。运行中把
  `⠋ Working...`（braille 帧、80ms 一帧）嵌入输入框上边框左侧，整行用运行色
  （borderAccent 青）渲染，空闲恢复纯 `─` 上边框（不再占独立状态行）。
- **worker-thread turn runner（ADR-0006 D6/D6a）**：`terminal::run` 先 `setup_with_cancel`
  构造 provider + 可取消工具集（再进 raw mode / alternate screen），设终端标题（OSC 0
  `slimcode - <session> - <cwd 目录名>`，`/new` `/load` 时更新），并尽力
  `git branch --show-current` 喂 footer。提交后把 `BailianProvider`（`Option`
  take/restore）+ `Vec<Tool>`（`mem::take`，`Tool::run` 已加宽为 `Box<dyn Fn(...) +
  Send + Sync>`）移入 worker `thread::spawn` 跑共享 `run_turn`，经 mpsc `ChannelRenderer`
  把 `DisplayItem` 流回 UI；UI 循环 `event::poll(80ms)` 同时当帧定时器，poll 事件 + 排空
  通道 + `draw` + `app.tick()`，spinner 因此边 HTTP 等待边动画。运行中：裸 `Esc` →
  `Effect::CancelRunning`（`Tui` 持有每轮 `CancelToken`，`drive_turn` 开头 `reset()`，
  Esc 时 `cancel()`；worker 在下一 runner 边界 / 下个 socket chunk 中止在途请求，并杀掉
  bash 子进程组；以 `StopReason::Cancelled` 静默结束、已流式内容保留、不进历史）；
  Ctrl+C / Ctrl+D → `Effect::QuitAfterTurn`，其它按键忽略。`handle.is_finished()` 门控
  join，任何路径都先 join 再 restore provider/工具（不变量：provider 总被归还）。turn
  结束后 worker 返回会话克隆（每轮起点 + 本轮进入历史的消息），TUI 采纳；turn 报错内联进
  transcript 并回到输入框。会话持久化改为**每条消息实时追加**（ADR-0009 D5）：worker 的
  `on_message` sink 把进入历史的每条消息同时推进 session 克隆并 `store.append`（追加失败
  收集为 notice、不打断 turn）；TUI 在 `submit_prompt`/`trigger_skill` 把本轮新消息
  （首轮 system + user）先进历史并 append（首个 assistant 前是 no-op、不建文件）；失败/
  取消且本轮已 append 过时，`close_turn` 追加一条带 `stop_reason`（`error`/`aborted`）与
  短文本的 assistant 消息收尾，保证日志不悬在工具批次上；一轮在首个 assistant 前失败则
  不产生任何文件。`/save` 已随整文件保存一并移除。TUI 用 `setup_with_cancel`
  （bash 为可取消变体，进程组 SIGKILL、~50ms 轮询）；CLI one-shot 用普通 `setup` +
  从不置位的 token。
- **测试 seam**：决定逻辑都在 `app` 纯 core 与 `footer`/`git` 纯函数里（帧缓冲测试、
  纯单测）；`terminal` 只有原始 I/O + 通道搬移。worker 通道有端到端测试（脚本化
  provider + 通道录制渲染器断言有序 `DisplayItem` 流与最终结果）；tmux 冒烟在
  `crates/cli/tests/tui_smoke.rs`（无 tmux 自动跳过）：对本地 mock SSE 服务器起真终端，
  capture-pane 断言头部/色块 prompt/markdown 思考/工具块/spinner 动画（已嵌入上边框）/footer 两行/补全
  弹框/滚动/改尺寸 dock 固定/OSC 0 标题/Ctrl+C 退出/Esc 中途取消（spinner 消失、已流式
  partial 文本保留、无错误文本、下一 prompt 正常运行）。
- **CLI 并行不变**：one-shot 前端字节不变地复用 `common`（`render::map_event` 共享；
  TUI 的 `DisplayItem::Usage` 在前端侧消费、绝不出自 `map_event`，/usage 汇总措辞与
  CLI 共用 `usage_summary`）。

### crates/cli 二进制（`slimcode`）

二进制入口 + 非交互 one-shot 前端，I/O 与逻辑分离（`run(args, out, tty)` 便于测试），
按启动规则分派（`std::io::IsTerminal` 判定 stdout 是否 TTY）：

- `--help` / `-h`：打印用法后退出；
- `slimcode config`：第一个参数为 `config` 时特判为子命令（先于 prompt 解析）→
  `config_cmd` 模块（交互壳 + 写回）：stdin/stdout 行输入逐项询问缺失的
  model / base_url / api_key（已有值显示为默认、回车保留），合并核心是
  `common::config::merge_config_toml` 纯函数，写回后（Unix）若含 api_key 则
  chmod 600 并打印文件路径；非 TTY / 多余参数报错；不处理 cache（保持手动编辑）；
- **one-shot**：`slimcode "<prompt>"`（可 `--cwd <dir>`、`--model <model>`、
  `--base-url <url>`、`--api-key <key>`）经共享 `ContextBuilder` 组装消息列表（新会话首轮前置系统
  提示并广告可自动调用 skill；开头的 `/skill:name` 会先被 `normalize_skill_trigger`
  改写为 `/{name}`，因为 one-shot 没有命令解析器），经共享 `run_turn` 跑一轮七工具循环、流式渲染事件、
  打印 token 用量；**不落盘会话**（ADR-0009 D5：无 `/load`/`/sessions` 工作流，与 TUI
  「首个 assistant 前失败不建文件」规则一致；`main` 里的启动 `cleanup_empty` 仍执行）；
- **chmod 提示**：`load_app_config` 之后，若 `ApiKeySource::File` 且（Unix）
  `config.toml` 权限 `mode & 0o077 != 0`，stderr 打印 `chmod 600 <path>` 提示（one-shot
  与 TUI 共用此打印点，TUI 进 alternate screen 前已打过）；非 Unix 跳过；
- **无 prompt + TTY**：交给 `slimcode_tui::terminal::run` 启动全屏 TUI（见上节）；
- **无 prompt + 非 TTY**：在配置解析前就以明确错误退出（非零退出码）。

模块：

- `render`：`TextRenderer`（`Renderer` trait 的文本实现）——把共享 `DisplayItem` 流（流式文本 / 流式思考 / 结构行 / 用量汇总）渲染为终端输出，原始 tool_call delta 与
  `Done` 事件被抑制；流式文本与思考（带 `> ` 前缀）按 delta 拼接、不逐 delta 换行，换行只来自内容本身的 `\n`，结构行（工具开始/结果、停止标记、turn 标记）总是另起一行；事件→DisplayItem 的映射是共享的 `common::render::map_event`，
  cli 不再各自实现（见 ADR-0004）；
- provider + 工具构造经 `common::setup::setup` 与 TUI 共享，两端不会漂移。

交互能力（历史 recall、`/` 命令、skills、`/new`、`/load`、`/exit`）已整体移入
TUI（`slimcode-tui`，见上节），行式 REPL 已移除（见 ADR-0003）。

agent crate 的 `agent` 模块 `pub use session::{Message, Role, ToolCall}`，CLI 统一从
`slimcode_agent::agent` 引用消息类型。
