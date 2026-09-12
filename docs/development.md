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

### crate 分层（ADR-0011–0014）

cargo workspace，六个 crate，唯一二进制 `slimcode`：

| crate | 包名 | 职责（对外界面） | 依赖 |
| --- | --- | --- | --- |
| `crates/ai` | `slimcode-ai` | LLM 层：`Message`（wire 消息）/ `Provider` / `ToolSpec` / `Delta` / `FinishReason` / `CancelToken` / `TokenUsage` / wire 模型 | 无 slimcode 依赖 |
| `crates/core` | `slimcode-core` | agent 运行时：`AgentEvent` / `AgentRunner`（借用式 per-run 值：tools/cfg/`&ProviderConfig`/cancel/事件订阅，`run(provider, system, messages)`，ADR-0015 加可选 hook 字段）/ `AgentMessage`(+`to_llm`) / `convert` / `Tool{spec,run}` / `RunConfig` / `StopReason` | → ai |
| `crates/app` | `slimcode-app` | 前端无关应用层：`DisplayItem` / `map_event` / `Renderer` / `usage_summary` / `ContextBuilder`→`Context{system,messages}` / 会话与输入历史持久化 / skills / context_files / 七工具 / setup / `run_turn` | → ai, core, commands |
| `crates/commands` | `slimcode-commands` | `/` 命令注册表 + fuzzy 预测（纯数据 + 纯函数，无 I/O） | 无 |
| `crates/tui` | `slimcode-tui` | 终端图形库：`RenderItem` / `Effect` / `App`(new/apply/draw/handle_key) / `run(terminal, app, handler)` / `UiHandler` / 组件（theme/markdown/toolcall/footer/text/git） | **无 slimcode 依赖** |
| `crates/cli` | `slimcode` | 唯一二进制 = 总入口：argv / 模式选择（one-shot 文本 vs 交互 TUI）/ 配置解析 / 服务构建 / 命令语义 / 会话落盘 / `TextRenderer` / `TuiAdapter` / 补全与文案 | → 全部 |

重命名（ticket 01）：`crates/agent` → `crates/core`、`crates/common` → `crates/app`。关键依赖反转（ticket 02）：`Provider` trait 与 LLM `Message` 由 `ai` 拥有，`ai` 不再反向依赖运行时。

依赖方向由测试断言（ticket 06，`crates/cli/tests/architecture.rs`）：`ai` 无 slimcode 依赖；`core` → 仅 `ai`；`app` → `ai`/`core`/`commands`；`tui` 无 slimcode 依赖；`cli` → 全部；`tui` 源码不得出现 `SessionStore`/`SkillStore`/`Config`。

### crates/ai Bailian provider 与 LLM seam

`ai` 拥有 LLM seam（ADR-0011 D1）：wire `Message`（`message` 模块）、`Provider` / `ToolSpec` /
`Delta` / `FinishReason` / `CancelToken`（`llm` 模块）与 `TokenUsage` / wire 模型（`wire` 模块）。
`BailianProvider`（`provider` 模块）实现 `Provider` seam，栈与 serde 容忍清单自 ticket 01/02/05：

- **HTTP**：`reqwest 0.13` blocking（features `json` + `blocking` + `rustls`，`default-features=false`）。
  `Provider` trait 是同步 seam，真实阻塞边界收在 provider 内部，不引入 tokio；ticket 02 文档中 `stream` feature
  仅 async 路径需要。body 按 chunk 可中断读取（ticket 07：`read_body_interruptibly` 每块检查 `CancelToken`，
  中途取消返回 `Cancelled(partial)`，provider 对已收到的完整 SSE 事件做 salvage 解析，把已流式内容交回 runner；
  阻塞 reqwest 没有 per-read 超时——静默服务器仍由客户端 300s 整体超时兜底）；
- **请求**：`stream: true` + `stream_options.include_usage: true`（ticket 05 实测 usage 只在带 `choices: []` 的最终 chunk 出现）；
  工具用 `role: tool` 消息回传结果；声明了 tools 时额外带 `parallel_tool_calls: true`
  （默认开启，见 ADR-0010），`tools` 为空时该字段省略、请求字节与旧版一致；
- **显式上下文缓存**（llm-cache + cache-last-message-mark）：开关位于 `ProviderConfig.cache`
  （默认 `true`，`with_cache(bool)` 建造式 setter，随 `chat` 跨 seam 传递，见 ADR-0016）。
  开启时 system 消息与自尾部扫描到的最后一条「非空文本的 user/assistant/tool」消息都把 `content`
  序列化为单元素块数组 `[{"type":"text","text":"…","cache_control":{"type":"ephemeral"}}]`，
  让「system 前缀」与「完整对话前缀」都交给端点缓存；尾部空文本（assistant 工具调用 / 空 tool 结果）
  跳过并向前找。百炼只在数组形态 content 上接受 `cache_control`，且按 content 块匹配前缀，因此开启缓存时
  **所有非空文本消息一律用数组形态**（唯一差异是是否带 mark），message 从「末尾带 mark」变成「历史不带
  mark」时字节除 mark 外不变、前缀匹配不破（ADR-0016 D6）。关闭缓存时所有消息回到旧的字符串形态，字节与未开启
  缓存的客户端完全一致。`message_to_wire(m, cache)` 处理单条，公开的 `messages_to_wire(msgs, cache)`
  负责尾部扫描并 mark 最后一条可缓存消息；assistant 工具调用空 content、tool 必带 content、空文本省略
  等既有语义不变；工具定义不加 mark（`cache_control` 只加在 content）；
- **usage 缓存统计**（llm-cache）：`TokenUsage` 新增可选嵌套 `prompt_tokens_details`
  （`cached_tokens` / `cache_creation_input_tokens`，缺省视为 0；整块缺省为 None），
  访问器 `cached_tokens()` / `cache_creation_tokens()` 缺省返回 0；`accumulate_usage` 把两个缓存字段
  随 prompt/completion/total 一起并入 `total_usage`；汇总行措辞由
  `app::render::usage_summary` 共享（cli 与 TUI 各渲染点都消费它，两端不漂移）；
- **serde 容忍清单**（全部不设 `deny_unknown_fields`，未知字段自动忽略）：
  - `reasoning_content`：思考模型每个 chunk 都带，`Option<String>`；
  - `usage`：key 每 chunk 都在但多为 `null`，`Option<TokenUsage>`（解析后进入 `last_usage` / `total_usage`）；
  - `content`/`function.name`/`function.id`：可为 `''`/`null`（思考模型空内容、tool_call 续传 `name: null`）；
  - `prompt_tokens_details`：可选嵌套，缺省视为 0（未命中/未开缓存时端点可能不带该块）；其它
    `*_tokens_details` 等未知字段：直接忽略；
- **tool_call 拼接**：首片段带 `id`/`name`（`arguments: ""`）→ `ToolCallStart`，续传只有 `index`+`arguments` → `ToolCallArgs`，按 index 拼接；
- **配置**：`ProviderConfig` 为纯 provider 数据（api key / base URL / model / cache，保留
  `chat_completions_url()`）。端点默认值（`DEFAULT_BASE_URL` / `DEFAULT_MODEL`）由本 crate 拥有，
  与 provider 放在一起；四层优先级解析与 env 变量名的唯一 owner 是 `slimcode-app::config`
  （frontend overrides > env > `config.toml` > 默认值），它 re-export 这两个默认值再产出
  `ProviderConfig`；ai 不提供 `from_env`、不读 env/文件，因此本 crate 无任何 slimcode 依赖
  （含 `[dev-dependencies]`：两个 live 冒烟测试自己读 env 名、用本 crate 的默认值）；
- **无状态 provider + config seam**（cache-last-message-mark）：`BailianProvider::new()` 只构建 HTTP client，
  不接收配置；`Provider::chat(messages, tools, config: &ProviderConfig, cancel)` 每次调用从入参读取
  model / base URL / api key / cache（ADR-0016）。同一实例可配不同 config 复用（测试、未来配置切换）；
  两个 `#[ignore]` 冒烟测试（文本 + 工具调用）需真实 key + 网络，默认跳过，一次性手动验证已通过。

### crates/core 工具

- `tools::edit`：`edit` 工具引擎（纯函数，无文件 I/O）。语义锁定自 `.scratch/slimcode-v1` ticket 03：
  - 一次调用多个**不相交** edit，全部匹配**原始**内容（非增量），按 offset 逆序应用；
  - 每个 `oldText` 必须**恰好出现 1 次**（0 → not-found，>1 → not-unique，均报错）；
  - 空 `oldText`、匹配重叠、no-op（内容未变）均报错；
  - 行尾 CRLF/LF 检测与恢复、BOM 剥离/恢复；
  - **轻量 fuzzy（选项 C）**：exact 优先，找不到时仅做每行 `trim_end` 归一重试（不做 NFKC/智能引号折叠），命中后按行回映射、未触碰行保留原始字节；
  - 成功返回 `{ new_content, replaced_blocks, diff, first_changed_line }`（替换块数、带行号 diff、首个变更行）。

### crates/core 会话模型（`session`，ADR-0012）

两层消息模型（ADR-0012 D1/D2）：`slimcode-ai` 拥有 wire `Message`（role + parts + tool_calls +
tool_call_id，已不含日志专用字段），`core` 拥有会话单元 `AgentMessage` 与唯一转换：

- `AgentMessage` 是 serde-tagged enum（`{"kind":"llm",…}`，今天只有 LLM 变体；compact 摘要等
  会话专有种类后续加入同一 enum）；`AgentMessage::to_llm(&self) -> Option<ai::Message>` 是唯一转换，
  LLM 变体返回其消息，会话专有变体自己决定“转化或丢弃”；
- `convert(system: &ai::Message, history: &[AgentMessage]) -> Vec<ai::Message>` = system 前缀 +
  `filter_map(to_llm)`，在**每次** provider 请求前调用（ADR-0012 D2），因此 run 中途的 compact
  下一轮自然生效；
- system prompt 每轮现组、不进会话也不进日志（ADR-0012 D3）；`Session::messages: Vec<AgentMessage>`
  一回合只新增一条 prompt 消息；
- `MessageStopReason`（`stop`/`tool_calls`/`error`/`aborted`）是日志专用类型，住在记录信封里
  （ADR-0012 D4），不在消息载荷上；旧日志的 `role:"system"` 记录在 `load` 时跳过（不重写文件、不升版本），
  旧格式的裸消息载荷（无 `kind` tag、日志字段内联）也仍可加载。

### crates/core 运行时循环（`core`）

循环是一个值：`AgentRunner`（借用式 per-run：tools / `RunConfig` / `&ProviderConfig` / `CancelToken` / 事件订阅，
`run(provider, system, messages)` 驱动，返回更新后的 history 与 stop reason；ADR-0011 D1，
provider config seam 见 ADR-0016，hook seam 见 ADR-0015）。折入自 ticket 04 原型，决策：

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
- 工具对 provider 只是 schema：`Provider::chat(&[Message], &[ToolSpec], &ProviderConfig, &CancelToken)`
  （ADR-0011 D1，config 入参见 ADR-0016）；
  `core::Tool { spec: ToolSpec, run }` 是带执行闭包的包装，`Tool::new(name, description, parameters, run)`
  照旧；loop 在每次 run 开头构建一次 `Vec<ToolSpec>`（非每请求）并交给所有 `chat` 调用；
- 工具事件（`ToolStart`/`ToolResult`）携带 `tool_call_id`，渲染端据此配对同名工具的多次调用；
- 历史是 `Vec<AgentMessage>`；每条进历史的都是 LLM 变体（assistant 回复、tool 结果），
  每次 `chat` 前用 `convert(system, &messages)` 重组 wire 消息列表；
- 工具报错以 `Error: …` 前缀作 `role: tool` 内容进 history，模型自然恢复；
- `Provider` trait 归 `slimcode-ai` 拥有（同步、无 async 依赖，真实 provider 内部处理阻塞边界），
  `core` 只 re-export 它的 seam 类型；运行时只依赖 `ai`（ADR-0011 D1/D4）；
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
（`slimcode-app::skills`），同样是前端无关的纯函数：CLI 在启动时把命令表与 skills 快照
合成注入 TUI 的 `/` 候选池（`app::skills::complete`，泛型自 `SkillView`：name/description
只读视图，`Skill` 与任何前端快照各实现一次），did-you-mean 文案走
`combined_suggestions`（同模块）；TUI 自己既不读注册表也不读 skills store（ADR-0014 D3）。

### crates/app 前端无关应用模块（`slimcode-app`）

自 cli 抽出的前端无关 module，任何前端（one-shot CLI、TUI、未来 Web）可直接复用，不依赖终端
二进制：

- `config`：四层优先级（frontend overrides > env > `config.toml` > 默认值）的单一 owner，
  并拥有 env 变量名（`ENV_*`）与默认值（`DEFAULT_BASE_URL` / `DEFAULT_MODEL`）常量；
  `cache` 项（llm-cache）同样四层解析：`--cache`/`--no-cache` override（`Overrides.cache`）>
  `SLIMCODE_AI_CACHE`（`true`/`false`/`1`/`0`/`yes`/`no`/`on`/`off`，大小写不敏感，非法值启动报错
  指明变量名）> `config.toml [ai] cache` > 默认 `true`（默认开启）；与 `base_url`/`model`
  逐项独立回落；
  `resolve(file_toml, env, overrides)` 纯解析核心（返回 `(ProviderConfig, ApiKeySource)`）
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
  backlog，此后每条一行的追加，之前为 no-op）、`append_closing`（失败/取消回合的收尾 assistant，
  `stop_reason`/`error` 写在记录信封：`{"type":"message","message":{…},"stop_reason":…,"error":…}`，
  缺省省略）、`append_title`、宽容的 `load`（未知/损坏记录跳过并计数、旧格式裸消息载荷与
  `role:"system"` 遗留记录均兼容（后者跳过，ADR-0012 D3）、残缺尾行丢弃并补换行、悬空的工具调用批次在内存补
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
  可选 message history（`with_history(Vec<AgentMessage>)`，history 不含 system）+ user prompt
  （`with_user_prompt`）。skill 触发的消息文本由 CLI 渲染（`skills::skill_prompt`，先用
  `context::skill_loaded_in` 对照 history 判断该 skill 是否已注入过，决定重复 body 还是
  “already loaded” 提示），再作为普通 user message 传入 —— 命令与 skill 语义都在 CLI
  （ADR-0013 D3）；
  `build()` 返回 `Context { system: ai::Message, messages: Vec<AgentMessage> }`（ADR-0012 D3）：
  system 由现组状态（基础提示、环境、context files、skills）合成，与 messages 分开返回，
  缺 user 时报错；一个回合只向 history 新增一条 prompt 消息；
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
- `runner`：共享 turn runner `run_turn(provider, tools, context: Context, &RunConfig, &ProviderConfig,
  &CancelToken, &mut dyn Renderer, &mut dyn FnMut(&AgentMessage))
  -> Result<(Vec<AgentMessage>, StopReason), String>`（ADR-0012 D3 改为收 `Context`，ADR-0009 D5 加
  `on_message` 回调与 `StopReason` 返回值，ADR-0016 加 `&ProviderConfig`）：逐事件流式回调渲染器，每条进历史的消息（assistant 回复、
  每个工具结果）同步回调一次 sink（TUI 借此实时追加日志），返回更新后的消息历史与
  终止原因（`Completed`/`Cancelled`）。cli 与 tui 共用同一 turn 循环，行为不漂移。
- `setup`：`setup(cwd) -> (BailianProvider, Vec<Tool>)` 共享 seam，cli 与
  tui 用同一套 provider + 工具构造，两端不会各自实现而漂移；provider 无状态（ADR-0016），
  故构造不再收 config，`&ProviderConfig` 由各前端在每轮 turn 传入。

### crates/tui 终端库（`slimcode-tui`）

`crates/tui` 是**终端库**（ADR-0003 / ADR-0013）：它被 CLI **进入**，不自己跑起来。
CLI 拥有进程与应用生命周期；本 crate 拥有纯 `App` 状态机与帧循环，且**不依赖任何
其他 `slimcode-*` crate**（含 dev-dependency，见 ticket 06 的矩阵测试）。模块：
`theme`（pi dark.json 词法转的只读 token 表：`Token::color()` 前景 / `BgToken::color()`
背景）、`text`（折行/截断/宽度，CJK 双宽）、`markdown`（pulldown-cmark → 样式
span，代码围栏行映射等）、`toolcall`（内置工具紧凑调用标题 composer，pi `format*Call`
移植：`CallPart` 纯函数 + 逐工具单测）、`footer`（pi `footer.ts` 的 `formatTokens` /
`formatCwdForFooter` / stats 纯函数移植）、`git`（`terminal_title` / `current_branch`
纯包装）、`render`（TUI 自己的显示词汇 `RenderItem`，ADR-0014 D1）、`handler`
（运行 seam：`UiHandler` / `Prompt` / `TurnReport` / `ControlFlow` /
`CompletionProvider` / `CompletionItem`，ADR-0013 D1）、`app`（纯 reducer + draw）、
`run`（帧循环 + worker-thread runner；`lib.rs` 把它导出为 `slimcode_tui::run`）。
分层：

- **自有显示词汇（ADR-0014 D1/D2）**：TUI 不消费 `app` 的 `DisplayItem`。`RenderItem`
  载 agent 流输出（`Text` / `Reasoning` / `ToolStart` / `ToolResult`）与 CLI 自有的状态
  （`Notice` / `Error` / `UserPrompt` / `Usage(FooterUsage)` / `Branch` /
  `SessionChanged`），不含任何 `ai`/`core`/`app` 类型；`App::apply(RenderItem)` 是唯一
  入口，transcript 合并与工具配对规则仍是私有实现。三套词汇两层转换：`AgentEvent`
  （core）→ `DisplayItem`（app，共享 `map_event`）→ `RenderItem`（tui）；中间的适配器
  属于 **cli**（`cli/src/render.rs::TuiAdapter`，`Renderer` 实现），在 turn 的 worker
  线程上把每条 `DisplayItem` 转成 `RenderItem` 交给库的 emit 回调；`Turn` 与 `Stop` 被
  丢弃（transcript 无 turn 标记、stop 不渲染）。`FooterUsage` 只保留平凡构造函数，token
  用量换算在 CLI 侧完成，故 TUI 源码不出现 provider 的用量类型。`app` 的显示契约
  （`DisplayItem` / `map_event` / `Renderer` / `usage_summary` / 共享 runner）保持原样。
- **运行 seam（ADR-0013 D1/D2）**：库入口是 `tui::run(terminal, app, handler)`——
  terminal 与 app 由 CLI 构造好，`handler` 是 CLI 实现的 `UiHandler`：

  ```rust
  pub trait UiHandler: Sync {
      fn on_effect(&mut self, effect: Effect, emit: &mut dyn FnMut(RenderItem)) -> ControlFlow;
      fn submit(&self, prompt: Prompt, emit: &mut dyn FnMut(RenderItem)) -> Result<TurnReport, String>;
      fn cancel(&self);
  }
  ```

  `on_effect` 在 UI 线程回答 reducer 的每个意图；`ControlFlow` 是 crate 自有的
  `Continue` / `Submit(Prompt)` / `Quit`（std 的 `ControlFlow` 载不了“请库跑一轮”），
  `Submit` 让**库**把该 turn 放到 worker 线程上跑 `submit`；`cancel` 由帧循环在 Esc 时
  调用，所以 `submit`/`cancel` 取 `&self`（`Sync`，turn 期间从 UI 线程与 worker 并发可达；
  CLI 用互斥量满足它）。库负责通道、`event::poll(80ms)` 帧定时器、排空、`draw`、
  `app.tick()` 与 scoped worker；raw mode / alternate screen / 终端标题 / panic hook /
  退出码全在 CLI。
- **`/` 补全来自注入的 provider（ADR-0014 D3）**：reducer 每次按键刷新弹框，但候选池
  由 CLI 在 `App::new` 时注入（`CompletionProvider::complete(&self, input) ->
  Vec<CompletionItem>`，`CompletionItem` 是 TUI 自己的类型）。CLI 的实现由
  `slimcode-commands` 的命令表 + skills store 拼成，所以 TUI 既无命令注册表也无 skills
  store；`/help` 文案、未知命令的 did-you-mean 与 skill 名解析都由 CLI 生成并以
  `RenderItem` 到达。
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
  `spinner_frame`）、注入的 `completions` provider、全局 `tool_output_expanded`、
  `version`。`handle_key` / `handle_key_running` / `tick()`（推进 spinner 帧、递减滚动条
  淡出计数）是纯 reducer；`Effect` 只有五个变体——`SubmitPrompt(Prompt)`（打字的 prompt
  或 recall 重跑，`Prompt.record` 决定是否写输入历史）、`Command { name, arg }`、`Quit` /
  `QuitAfterTurn` / `CancelRunning`——**reducer 不解析命令语义**：`/` 开头的输入原样变成
  `Command`，含义（`/help` / `/new` / `/load` / `/sessions` / `/usage` / `/history` /
  `/skills` / `/install-skill` / `/exit` / `/!!` / `/!N` / skill 触发 / 未知命令）全由
  CLI 在 `on_effect` 里决定（ADR-0013 D3）。`draw` 用 ratatui `TestBackend` 做帧缓冲测试
  （spec：好测试断言**帧缓冲**而非内部状态）。布局是 ADR-0007 D4 四区 dock：
  `[transcript(Min0) | popup(0|n) | input | footer(2)]`；补全弹框（0 行时收起）位于输入框
  正上方，开合只吃 transcript、不移动输入框；编辑器为无左右竖线/圆角的上下两条全宽 `─`
  横线，边框色蓝色（`border`）闲置 / 青色（`borderAccent`）运行中，右缘滚动条 thumb。
  输入历史 recall：输入框为空时 `↑`/`↓` 进入（最新一条开始），`Enter` 把选中的历史 prompt
  作为新一轮重跑（不再写入历史）；每轮提交时追加（不查重，同 `HistoryStore::append`）。
- **Footer / 状态指示器（ADR-0006 D5/D6）**：两行 dim footer 由纯函数拼装——第一行
  `~/cwd (branch) • session`（`footer::format_cwd_for_footer`：只在词法上位于 `$HOME`
  内时缩写为 `~` / `~/rel`），第二行 `stats_line`（`↑in ↓out Rcache WcacheWrite
  CH{pct}%`，零值省略；`format_tokens` 与 pi 同表：<1000 原样、<10k `x.xk`、<1M 取整
  `xk`、<10M `x.xM`、否则取整 `M`），模型名右对齐，宽度不足时右侧截断。footer 的用量由
  CLI 每轮以 `RenderItem::Usage` 喂入（换算在 CLI 侧）。运行中把 `⠋ Working...`
  （braille 帧、80ms 一帧）嵌入输入框上边框左侧，整行用运行色（borderAccent 青）渲染，
  空闲恢复纯 `─` 上边框（不再占独立状态行）。
- **worker-thread turn runner（ADR-0006 D6/D6a，ADR-0013 D2）**：`run` 在提交一轮时建
  `mpsc::channel::<RenderItem>()`，用 `thread::scope` 起 worker 跑
  `handler.submit(prompt, &mut emit)`（`emit` 就是往通道 send 的闭包）；UI 线程留帧循环
  `event::poll(80ms)` 当帧定时器，poll 事件 + 排空通道（`app.apply`）+ `draw` +
  `app.tick()`，spinner 因此边 HTTP 等待边动画。运行中：裸 `Esc` → `handler.cancel()`
  （CLI 持有每轮 `CancelToken`，`submit` 开头 `reset()`；worker 在下一 runner 边界 /
  下个 socket chunk 中止在途请求，并杀掉 bash 子进程组；以 `StopReason::Cancelled` 静默
  结束、已流式内容保留、不进历史）；Ctrl+C / Ctrl+D → `Effect::QuitAfterTurn`，其它按键
  忽略。`handle.is_finished()` 门控 join，任何路径都先 join 再让 CLI 归还 provider/工具
  （不变量：provider 总被归还）。turn 的收尾全在 CLI：会话持久化为**每条消息实时追加**
  （ADR-0009 D5），`on_message` sink 把进入历史的每条消息同时推进 session 并
  `store.append`（追加失败收集为 notice、不打断 turn）；失败/取消且本轮已产出过消息时
  用 `append_closing` 追加一条带 `stop_reason`（`error`/`aborted`，写在记录信封）与短文本
  的 assistant 消息收尾，保证日志不悬在工具批次上；一轮在首个 assistant 前失败则不产生
  任何文件。`submit` 返回 `Err` 时库把它渲染成一行错误（CLI 已先关好日志）。
- **测试 seam**：决定逻辑都在 `app` 纯 core 与 `footer`/`git` 纯函数里（帧缓冲测试、
  纯单测）；`run` 只有原始 I/O + 通道搬移。CLI 侧有 handler 测试（脚本化 provider +
  记录 emit 的闭包：命令语义、skill 解析、重跑、会话落盘、失败收尾、并发拒绝、cancel
  重置）与表驱动适配器测试（覆盖每个 `DisplayItem` 变体，含被丢弃的 `Turn`/`Stop`）；
  tmux 冒烟在 `crates/cli/tests/tui_smoke.rs`（无 tmux 自动跳过）：对本地 mock SSE 服务器
  起真终端，capture-pane 断言头部/色块 prompt/markdown 思考/工具块/spinner 动画（已嵌入
  上边框）/footer 两行/补全弹框/滚动/改尺寸 dock 固定/OSC 0 标题/Ctrl+C 退出/Esc 中途取消
  （spinner 消失、已流式 partial 文本保留、无错误文本、下一 prompt 正常运行）。
- **one-shot 与 TUI 不漂移**：两端的 `DisplayItem` 映射、`usage_summary` 措辞与
  `ContextBuilder` 上下文组装仍来自 `app`；one-shot 从一开始就不经 TUI crate。

### crates/cli 唯一二进制 = 总入口（`slimcode`）

唯一的二进制入口（ADR-0011 D3）：argv / 模式选择 / 配置解析 / 服务构建 / 命令语义 /
会话落盘 / 两个显示适配器（one-shot 文本与 TUI）全在这里；TUI 只是它进入的一个库。I/O 与
逻辑分离（`run(args, out, tty)` 便于测试），按启动规则分派（`std::io::IsTerminal` 判定
stdout 是否 TTY）：

- `--help` / `-h`：打印用法后退出；
- `slimcode config`：第一个参数为 `config` 时特判为子命令（先于 prompt 解析）→
  `config_cmd` 模块（交互壳 + 写回）：stdin/stdout 行输入逐项询问缺失的
  model / base_url / api_key（已有值显示为默认、回车保留），合并核心是
  `app::config::merge_config_toml` 纯函数，写回后（Unix）若含 api_key 则
  chmod 600 并打印文件路径；非 TTY / 多余参数报错；不处理 cache（保持手动编辑）；
- **one-shot**：`slimcode "<prompt>"`（可 `--cwd <dir>`、`--model <model>`、
  `--base-url <url>`、`--api-key <key>`）经共享 `ContextBuilder` 组装 `Context`（system 单独
  返回、每轮现组并广告可自动调用 skill；开头的 `/skill:name` 会先被 `normalize_skill_trigger`
  改写为 `/{name}`，因为 one-shot 没有命令解析器），经共享 `run_turn` 跑一轮七工具循环、流式渲染事件、
  打印 token 用量；**不落盘会话**（ADR-0009 D5：无 `/load`/`/sessions` 工作流，与 TUI
  「首个 assistant 前失败不建文件」规则一致；`main` 里的启动 `cleanup_empty` 仍执行）；
- **chmod 提示**：`load_app_config` 之后，若 `ApiKeySource::File` 且（Unix）
  `config.toml` 权限 `mode & 0o077 != 0`，stderr 打印 `chmod 600 <path>` 提示（one-shot
  与 TUI 共用此打印点，TUI 进 alternate screen 前已打过）；非 Unix 跳过；
- **无 prompt + TTY**：交给 `tui::run`（`cli/src/tui.rs`）启动全屏 TUI（见上节）：
  构造 provider + 工具、建 `App` 与 `TuiSession` handler、进 raw mode/alternate screen、
  设标题、跑 `slimcode_tui::run`、退出前恢复终端；
- **无 prompt + 非 TTY**：在配置解析前就以明确错误退出（非零退出码）。

模块：

- `tui`：TUI 入口与 handler（ADR-0013）。`run` 拥有进程生命周期（raw mode / alternate
  screen / 终端标题 / panic hook / 退出码）；`TuiSession` 实现 `UiHandler`，拥有 provider
  与工具（`Mutex` 里，turn 期间 take 出来跑、结束后归还）、session store、input history、
  skills、context files、environment，并实现**全部命令语义**（`/help` / `/new` / `/load` /
  `/sessions` / `/usage` / `/history` / `/skills` / `/install-skill` / `/exit` / `/!!` /
  `/!N` / skill 触发 / 未知命令 + did-you-mean）、每轮的上下文组装与会话落盘、失败/取消
  收尾。`CliCompletions` 是注入 TUI 的 `/` 候选 provider（命令表 + skills 快照，
  `/install-skill` 后就地刷新）。
- `render`：两个 `Renderer` 实现。`TextRenderer` 把共享 `DisplayItem` 流（流式文本 / 流式思考 / 结构行 / 用量汇总）渲染为终端输出，原始 tool_call delta 与
  `Done` 事件被抑制；流式文本与思考（带 `> ` 前缀）按 delta 拼接、不逐 delta 换行，换行只来自内容本身的 `\n`，结构行（工具开始/结果、停止标记、turn 标记）总是另起一行；事件→DisplayItem 的映射是共享的 `app::render::map_event`。
  `TuiAdapter`（ADR-0014 D2）持有库给的 emit 回调，在 turn 的 worker 线程上把每条
  `DisplayItem` 转为 `slimcode_tui::render::RenderItem`；`Turn`/`Stop` 丢弃，`Usage` 经
  `to_footer_usage` 完成 `TokenUsage → FooterUsage` 换算（表驱动单测覆盖每个变体）。
  `DisplayItem` / `Renderer` / `map_event` / `usage_summary` / `run_turn` 仍全在 `app`
  （ADR-0004 未变）。
- provider + 工具构造经 `app::setup::setup` 与 TUI 共享，两端不会漂移。

交互能力（历史 recall、`/` 命令、skills、`/new`、`/load`、`/exit`）已整体移入
TUI（`slimcode-tui`，见上节），行式 REPL 已移除（见 ADR-0003）。

core crate 的 `core` 模块 `pub use session::{Message, Role, ToolCall}`，CLI 统一从
`slimcode_core::agent` 引用消息类型。
