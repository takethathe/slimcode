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

cargo workspace，六个 crate：

| crate | 包名 | 职责 | 状态 |
| --- | --- | --- | --- |
| `crates/ai` | `slimcode-ai` | 统一 LLM provider 层（Provider trait + OpenAI-compatible/Bailian） | 起步（Bailian provider + wire 模型） |
| `crates/agent` | `slimcode-agent` | agent 运行时、工具、会话状态 | 起步（edit 引擎 + 运行时循环 + 消息模型） |
| `crates/commands` | `slimcode-commands` | 前端无关的 `/` 命令注册表与预测提示 | v1 新增（registry + suggest/find） |
| `crates/common` | `slimcode-common` | 前端无关的应用模块（配置解析 / 会话与输入历史持久化 / skills 发现与安装 / 上下文组装 / 七工具绑定 / 共享渲染模型与 turn runner / setup seam） | v1 新增（自 cli 抽出 + TUI 共享 seam） |
| `crates/tui` | `slimcode-tui` | 交互式全屏 TUI（纯 App core + crossterm/ratatui 终端循环） | v1 完成（app/terminal） |
| `crates/cli` | `slimcode` | 二进制入口 + 非交互 one-shot 前端（共享 runner + TextRenderer） | v1 完成（render/main） |

### crates/ai Bailian provider

实现 `agent::Provider` seam（ticket 04/05），栈与 serde 容忍清单自 ticket 01/02/05：

- **HTTP**：`reqwest 0.13` blocking（features `json` + `blocking` + `rustls`，`default-features=false`）。
  `Provider` trait 是同步 seam，真实阻塞边界收在 provider 内部，不引入 tokio；ticket 02 文档中 `stream` feature
  仅 async 路径需要，blocking 下用 `resp.text()` 一次取回整段 SSE 再解析；
- **请求**：`stream: true` + `stream_options.include_usage: true`（ticket 05 实测 usage 只在带 `choices: []` 的最终 chunk 出现）；
  工具用 `role: tool` 消息回传结果；
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

消息/会话数据模型，折入自 ticket 04：

- `Message { role, parts: Vec<Part>, tool_calls, tool_call_id }`，role 序列化小写；
- `Part::Text { text }`（parts 抽象，v1 默认/唯一变体），JSON 形状 `{"type":"text","text":"..."}`；
- `ToolCall { id, name, arguments }`，`arguments` 存模型原始 JSON 串、执行时才 parse；
- `Session { id, created_at, messages, title }` 包一层元数据（为 `~/.slimcode/sessions/` 准备）；
- JSON 边界：`tool_calls`/`tool_call_id` 缺省时省略，整图无损 round-trip。

### crates/agent 运行时循环（`agent`）

折入自 ticket 04 原型，决策：

- 循环：模型带 `tool_calls` 的响应 → 执行工具 → 追加 `role: tool` 结果 → 循环，直到模型不再调工具；
- 停止：无 tool_calls → `Completed`；`max_iterations` → `MaxIterations`（运行时唯一硬保险；运行中用户中断不在范围内，见 crates/tui 一节）；
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
  命令在前、skill 在后；非 `/` 输入返回空）；候选 `value` 是命令的裸拼写（`/save`、`/resume`）
  与 skill 的规范触发 `/skill:name`，**不含** `usage` 的参数占位符，提交时经
  `find`/`find_skill` 可解析；
- `context`：`ContextBuilder`（前端无关）把一轮 prompt 的上下文组装收敛为单一
  入口：基础系统提示（默认 `DEFAULT_SYSTEM_PROMPT` 或 `with_system` 覆盖）+
  可自动调用 skill 广告（`with_skills`，build 时经 `format_skills_for_prompt` 过滤
  `disable-model-invocation`，产出 `## Skills` markdown 索引）+
  可选 message history（`with_history`，非空不重复插 system）+ user prompt（
  `with_user_prompt`）或 skill 触发（`with_skill`，build 时扫描 history 中是否已有
  `<skill name="..."` 标记来决定是否去重，再调用 `skill_prompt`）；
  `build()` 返回可直接交给 `run_agent_from_messages` 的 `Vec<Message>`，缺
  user 时报错；空 history 前置一条 system 消息；
- `tools`：把七工具 factory 绑定到启动 `cwd`。
- `render`（ADR-0004）：前端无关的显示模型。`DisplayItem` 是渲染单元（turn 标记 /
  流式文本片段 / 思考行 / 工具开始与结果 / 停止标记 / token 用量）；
  `map_event(AgentEvent) -> Option<DisplayItem>` 是事件→显示单元的共享纯映射；
  `usage_summary(TokenUsage) -> String` 是 token 用量汇总行（含 `({cached} cached, {pct}%)`
  与缓存命中百分比）的共享措辞，cli 汇总与 TUI `/usage` 都消费它；
  `Renderer` trait 消费 `DisplayItem`，每个前端只实现自己的渲染器（cli 的文本行、
  tui 的 widget 状态）。
- `runner`：共享 turn runner `run_turn(provider, tools, messages, &RunConfig,
  &mut dyn Renderer) -> Result<Vec<Message>, String>`，逐事件流式回调渲染器，返回
  更新后的消息历史。cli 与 tui 共用同一 turn 循环，行为不漂移。
- `setup`：`setup(cwd, config) -> (BailianProvider, Vec<Tool>)` 共享 seam，cli 与
  tui 用同一套 provider + 工具构造，两端不会各自实现而漂移。

### crates/tui 交互式 TUI（`slimcode-tui`）

`crates/tui` 提供交互式全屏 TUI（ADR-0003），替代行式 REPL。分两层：

- **纯 App core（`app`）**：前端无关、无 I/O 的 reducer。持有 `transcript`
  （`DisplayItem` 序列）、输入框、`history`（input history 快照，最旧在前）、
  `recall` 态、`status`（当前会话 id / notice / error）。`handle_key(KeyEvent) ->
  Option<Effect>` 是纯 reducer，把按键映射为副作用声明；`Effect` 枚举
  （Submit / ReplayPrompt / ReplayHistory / NewSession / LoadSession / Quit / …）
  由终端循环兑现（session / history / skills / quit）。`draw(Frame)` 经 `Renderer`
  实现把状态渲染到 ratatui `Frame`，用 `TestBackend` 做帧缓冲测试（spec：好的测试
  断言**帧缓冲**，而非内部状态）；长行在纯 core 里按面板宽度预折行（`wrap_to_width`，
  CJK 双宽字符按 2 列计），滚动/跟随的行数数学因此与渲染一致、内容不截断。命令解析经
  `slimcode_commands::find` + skill
  触发 + 编号重跑（`/!N`），未知 `/` 命令给出 `did you mean` 提示；`/new` / `/load`
  会清空 transcript 再重建会话视图。输入历史 recall：输入框为空时按 `↑`/`↓` 进入
  （从最新一条开始），`Enter` 把选中的历史 prompt 作为新一轮重跑（不再写入历史）；
  记录在每轮提交时追加（不查重，同 `HistoryStore::append`）。
- **`/` 补全弹框（ADR-0005）**：`App::completion: Option<Completion>`（`items` /
  `selected` / `offset`，`Completion::clamp_offset` 让选中项始终落在滚动窗口内）。
  条件：整条输入为单行、以 `/` 开头且 `/` 后无空白（输入空格进入参数段即关闭弹框）；
  每次按键后 `refresh_completion` 重算候选（`common::skills::complete`），已选中值仍是
  候选时保留选择，否则回落到最佳匹配（index 0）。按键：`↑`/`↓` 循环导航、`PgUp`/`PgDn`
  翻页（弹框打开时优先于 transcript 滚动）、`Tab` 上屏（写入裸拼写 + 尾部空格、光标
  落空格后、不提交）、`Enter` 展开选中项后直接提交执行、`Esc` 取消并保留文本。弹框在
  input 与 status 之间动态扩展高度渲染（`render_completion`，ratatui `List`，选中行
  `→` + 反色，最多 `COMPLETION_VISIBLE=5` 行，超出显示滚动窗口标记）。
- **终端循环（`terminal`）**：薄壳。`run(cwd, config, store, history, skills)` 先经
  `common::setup::setup` 构造 provider + 工具（**在**进入 raw mode / alternate
  screen 之前，API-key/配置错误在普通终端上浮现），再 `enable_raw_mode` +
  `EnterAlternateScreen`，泵 crossterm 事件、喂给 App reducer、兑现 `Effect`，
  每事件 `draw`（ratatui 自动 autoresize），退出路径统一 `restore_terminal()`。

**同步 provider 的运行时行为**：provider 当前是同步 seam（真实阻塞边界收在
provider 内部）。一轮提交后循环**不再轮询按键**，同步阻塞在共享 `run_turn` 内；
`LiveRenderer` 把每个流式 `DisplayItem` 追加进 transcript（相邻的流式文本/思考片段合并为同一条目）并立即重绘，实现边跑边
显示。turn 结束自动保存会话；turn 报错内联进 transcript 并回到输入框。运行中的
Ctrl+C 字节被缓冲，turn 结束后退出。中断运行中的 turn 明确不在范围内（见 spec
Further Notes）。

### crates/cli 二进制（`slimcode`）

二进制入口 + 非交互 one-shot 前端，I/O 与逻辑分离（`run(args, out, tty)` 便于测试），
按启动规则分派（`std::io::IsTerminal` 判定 stdout 是否 TTY）：

- `--help` / `-h`：打印用法后退出；
- **one-shot**：`slimcode "<prompt>"`（可 `--cwd <dir>`、`--model <model>`、
  `--base-url <url>`）经共享 `ContextBuilder` 组装消息列表（新会话首轮前置系统
  提示并广告可自动调用 skill；开头的 `/skill:name` 会先被 `normalize_skill_trigger`
  改写为 `/{name}`，因为 one-shot 没有命令解析器），经共享 `run_turn` 跑一轮七工具循环、流式渲染事件、
  打印 token 用量并保存会话；
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
