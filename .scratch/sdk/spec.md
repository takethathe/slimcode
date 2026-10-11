# slimcode SDK（进程内嵌入会话 API）

Status: resolved

## Problem Statement

slimcode 的 coding-agent 能力（prompt 一轮 agent 循环、流式事件、七工具执行、会话持久化、skills / context files / 自动压缩）目前只能通过 CLI 二进制触达：one-shot 文本模式与交互式 TUI。其它 Rust 程序无法在进程内嵌入 slimcode；未来的 Node / Python 宿主更没有入口。参照 pi 项目的 SDK 方案（`@earendil-works/pi-coding-agent` 的 `createAgentSession` / `AgentSession`：CLI 是 SDK 的薄壳），slimcode 需要一个程序化、会话中心的 SDK crate，把 app 层已有的前端无关构件以公共 API 暴露出来，并为后续 FFI 模块（Node/Python 对接）与 CLI 薄壳化铺路。

## Solution

新增 workspace crate `slimcode-sdk`：`create_agent_session(AgentSessionOptions) -> Result<AgentSession, String>`，返回一个会话对象，提供

- `prompt(&self, text)`：同步跑一轮 agent turn（复用 `app::runner::run_turn_with_hooks`），返回 `StopReason`；
- `subscribe(&self, listener) -> Unsubscribe`：订阅 `SdkEvent` 事件流（`Display(DisplayItem)` + 会话生命周期事件）；
- `abort(&self)` / `dispose(&self)`：跨线程取消 / 释放；
- 只读访问器：`messages` / `session_id` / `usage` / `title`。

会话两种持久化模式：默认复用现有 `SessionStore`（项目分区 JSONL 日志，与 TUI `/session` picker 完全兼容），`persist: false` 时纯内存、不写任何文件。配置与资源组装对齐 CLI 启动路径：显式选项（`cwd` / `base_url` / `model` / `api_key` / `provider` / `tools` / `skills`…）优先，缺省回落现有四层配置解析与默认七工具、AGENTS.md 上下文文件、skills 发现、环境信息。SDK 不解释 `/` 命令（那是前端语义），prompt 按纯文本处理。

后续工作（FFI 部分已实现，spec 见 `.scratch/ffi/spec.md`）：`slimcode-ffi` cdylib 模块（纯 C ABI，不透明句柄，供 Node/Python 薄绑定）；CLI 重构为 SDK 的消费者（pi 的薄壳模式）。

## User Stories

1. 作为 Rust 库消费者，我想用一次调用 `create_agent_session(options)` 创建会话，以便在我的程序里嵌入 slimcode，而不必启动 CLI 子进程。
2. 作为 Rust 库消费者，我想调用 `prompt(text)` 跑完一整轮 agent turn 并拿到 `StopReason`，以便知道回合是完成还是被取消。
3. 作为 Rust 库消费者，我想在 prompt 之前 `subscribe(listener)`，以便实时收到流式文本、思考行、工具开始/结果与用量事件。
4. 作为 Rust 库消费者，我想用返回的 `Unsubscribe` 停止接收事件，以便管理监听器生命周期（例如 UI 组件卸载）。
5. 作为库消费者，我想在每条消息进入历史时收到 `SdkEvent::Message { message, usage }`，以便按消息展示或转发、并跟踪 token 开销。
6. 作为库消费者，我想在 prompt 结束时收到最后一个 `SdkEvent::TurnFinished { stop_reason }`，以便知道包括回合后处理（自动压缩）在内的整轮已结束。
7. 作为库消费者，我想在默认持久化模式下让会话落成现有 JSONL 会话日志，以便之后用 TUI 的 `/session` picker 加载同一个会话。
8. 作为库消费者，我想在 `persist: false` 的内存模式下不写任何文件，以便在无磁盘写入要求的宿主里嵌入 slimcode。
9. 作为库消费者，我想从另一个线程调用 `abort()` 取消正在跑的 prompt，以便实现超时、UI 取消等控制。
10. 作为库消费者，我想调用 `dispose()` 释放会话并让后续 prompt 报错，以便干净地清理资源。
11. 作为库消费者，我想在选项里显式传 `base_url` / `model` / `api_key`，以便指向自建端点而不依赖 `~/.slimcode/config.toml`。
12. 作为库消费者，我想在未传配置时回落现有四层配置解析（overrides > env > config.toml > 默认值），以便零配置起步。
13. 作为库消费者，我想注入自己的 `Provider` 与工具集，以便 mock LLM、替换端点或定制工具（这也是测试与未来 FFI 宿主的能力需求）。
14. 作为库消费者，我想默认获得绑定到 `cwd` 的七工具（read/write/edit/bash/grep/find/ls），以便 prompt 能实际操作目标目录。
15. 作为库消费者，我想让 SDK 会话像 CLI 一样发现并注入 AGENTS.md 上下文文件与环境信息，以便会话行为与 CLI 不漂移。
16. 作为库消费者，我想让 SDK 会话像 CLI 一样发现 skills 并在系统提示中广告、把模型对 `SKILL.md` 的 read 拦截为 skill 激活块，以便 SDK 与 CLI 的 skill 行为一致。
17. 作为库消费者，我想在完成回合且历史越过阈值时自动压缩（对齐 TUI 的回合后触发），以便长会话持续可用。
18. 作为库消费者，我想在失败回合中拿到 `Err`、会话日志以 assistant 边界收尾（保留错误收尾消息），以便日志保持可回放。
19. 作为库消费者，我想在取消回合中拿到 `Cancelled`、已流出的 partial 文本只经 `Display` 事件交付（不进历史）、日志以 `aborted` 收尾消息关闭，以便会话状态一致、不悬在工具批次上。
20. 作为库消费者，我想在回合进行中再次调用 prompt 时被明确拒绝，以便会话保持单飞（single-flight）。
21. 作为库消费者，我想在 prompt 期间事件在调用线程同步送达监听器，以便无需异步运行时即可使用。
22. 作为测试作者，我想只通过 `AgentSession` 公共 API 注入脚本化 provider 来测 SDK 全部行为，以便测试不依赖实现细节、与仓库现有 handler-test 风格一致。
23. 作为未来 Node/Python 宿主，我想让 SDK 能力最终能以独立编译的 cdylib 模块（纯 C ABI、不透明句柄 + 事件流）暴露，以便用 FFI 从其它语言驱动 slimcode（后续工作）。
24. 作为未来 CLI 维护者，我想有一份 CLI 薄壳化（CLI 消费 SDK）的路线文档，以便行为收敛到单一实现（后续工作）。
25. 作为库消费者，我想 `slimcode-sdk` 是纯 Rust crate、无终端依赖（不依赖 tui/crossterm/ratatui），以便非终端宿主（服务、CI 脚本、web 后端）可以依赖它。
26. 作为库消费者，我想让 prompt 文本按纯文本处理、不解释 `/` 命令，以便命令语义继续是前端关注点、SDK 不复制命令注册表。
27. 作为库消费者，我想读取会话的 `messages` / `session_id` / `usage` / `title`，以便在 turn 之间检查状态或做展示。
28. 作为库消费者，我想在压缩失败时保留原历史与本次 turn 结果（静默、不丢数据），以便失败不影响已完成的工作。

## Implementation Decisions

### 1. 新 crate `slimcode-sdk`

workspace 新增成员，包名 `slimcode-sdk`，依赖 `slimcode-app`（→ `ai` / `core` / `commands`）；**不依赖** `tui`、`cli` 与任何终端 crate。`crates/cli/tests/architecture.rs` 的依赖断言矩阵扩展一条：`sdk → app(→ ai/core/commands)`。

### 2. 会话服务组装对齐 CLI 启动路径

`create_agent_session` 内部复用 app 层既有的组装件（与 `cli/main.rs` 启动路径同一套，不新写发现逻辑）：

- 配置：`app::config::load_app_config(Overrides { base_url, model, cache, api_key })`——显式选项（`base_url` / `model` / `api_key` / `cache`）覆盖 env > `config.toml` > 默认值；产出 `ProviderConfig`。
- 家目录与项目定位：`app::config::slimcode_home()`、`app::context_files::resolve_project_home(cwd, user_home)`、`project_key(project_home)`。
- 会话存储：`SessionStore::new(home/sessions, project_key, project_home)`（仅持久化模式）；启动时 `cleanup_empty()` 尽力而为。
- 上下文文件与环境：`load_context_files(home, cwd)` + `Environment { os, global_home, project_home }`（与 CLI 逐字段一致）。
- Skills：`SkillStore::new(home, cwd)` + `list()`（发现失败不阻塞创建，对齐 CLI）。

### 3. 公共 API 形状（schema 决策，来自访谈确认）

```rust
pub struct AgentSessionOptions {
    pub cwd: Option<PathBuf>,          // 默认 process::current_dir()
    pub persist: bool,                 // 默认 true（SessionStore）；false = 纯内存
    pub base_url: Option<String>,      // 显式覆盖，缺省回落四层解析
    pub model: Option<String>,
    pub api_key: Option<String>,
    pub cache: Option<bool>,
    pub provider: Option<Box<dyn Provider + Send>>, // 默认 BailianProvider；测试/FFI 注入点
    pub tools: Option<Vec<Tool>>,      // 默认七工具（绑定 cwd）
    pub skills: Option<Vec<Skill>>,    // 默认从 user/project 两 scope 发现
}

pub enum SdkEvent {
    Display(DisplayItem),              // 流式显示单元（复用 app::render::DisplayItem）
    Message { message: AgentMessage, usage: Option<TokenUsage> }, // 每条进历史的消息（含用户 prompt）；assistant 带 usage，用户 prompt 与工具结果为 None
    TurnFinished { stop_reason: StopReason }, // 一次 prompt 的权威结束信号
    Compacted { before: usize, after: usize }, // 自动压缩发生（回合后）
}

pub struct AgentSession { /* 见决策 4 */ }
pub type Unsubscribe = Box<dyn FnOnce() + Send + 'static>;

impl AgentSession {
    pub fn prompt(&self, text: &str) -> Result<StopReason, String>;
    pub fn subscribe(&self, listener: impl Fn(SdkEvent) + Send + Sync + 'static) -> Unsubscribe; // + Sync：emit 用 Arc 快照在锁外调用监听器（见决策 4）
    pub fn abort(&self);
    pub fn dispose(&self);
    pub fn messages(&self) -> Vec<AgentMessage>;
    pub fn session_id(&self) -> &str;
    pub fn title(&self) -> Option<String>;
    pub fn usage(&self) -> TokenUsage;
}
```

- 选项构造采用 builder 风格（`AgentSessionOptions::new(cwd)` + `.with_persist(false)` 等），缺省字段不强制传。
- 错误统一 `Result<_, String>`（对齐现有 agent/app 错误风格，ADR-0004）。
- `title()` 返回 owned `String`：标题存在会话状态里，而状态在 `Mutex<Inner>` 之后，无法借出 `&str`。
- 空 / 全空白 `prompt` 立即返回 `Err("prompt text must not be empty")`，不建日志、不触发回合。
- 内存模式的会话 id 用与 `SessionStore::new_id` 相同的 `slimcode-<unix>-<pid>-<n>` 方案：把 id 生成从 `SessionStore::new_id` 抽成 app 层自由函数 `new_session_id()`，SDK 与 store 共用（app 层小改动，行为不变）。

### 4. `AgentSession` 内部状态与并发模型

对齐 `TuiSession` 的 take/put 模式（ADR-0013 D2 的运行时 seam）：

- 内部状态 `Arc<Mutex<Inner>>`，`Inner { session: Session, provider: Option<Box<dyn Provider + Send>>, tools: Vec<Tool>, cancel: CancelToken, disposed: bool }`。
- `prompt(&self)`：锁内 `take()` 出 provider + session + tools（拿不到 = 已有回合在跑 → `Err("a turn is already running")`），`cancel.reset()`，**锁外**跑完整个 turn，结束后 put 回。因此 `prompt` 可跨线程单飞、且回合期间 `abort(&self)`（只碰 `CancelToken`）永不等待锁。
- 监听器列表独立于 Inner：`Arc<Mutex<Vec<(usize, Arc<dyn Fn(SdkEvent) + Send + Sync>)>>>`；事件在 turn 的调用线程**同步**送达每个监听器（订阅顺序）。`emit` 在锁内快照每个监听器的 `Arc`、在锁外调用，所以监听器回调内可以调用自己的 `Unsubscribe`（std mutex 不可重入，锁内调用会死锁）；监听器 panic 被 `catch_unwind` **隔离**（跳过该监听器，其它监听器与回合继续，会话保持可用）。
- `abort(&self)` 置位 `CancelToken`（`Arc<AtomicBool>`，跨线程安全，复用现有取消语义：下一 runner 边界 / 下一 socket chunk 中止）。
- `dispose(&self)`：置 `disposed`、清空监听器、置位 cancel；此后 `prompt` 报错。

### 5. `prompt()` 回合编排（对齐 `TuiSession::run_turn`）

一次 `prompt(text)` 的步骤（复用 `app::runner::run_turn_with_hooks`，不新写循环）：

1. `ContextBuilder::new().with_environment(env).with_context_files(&files).with_skills(&skills).with_history(session.messages.clone()).with_user_prompt(text).build()`（无 prompt 时 build 报错，SDK 传 text 恒非空）。
2. prompt 消息（context.messages 最后一条）push 进 `session.messages` 并（持久化模式）`store.append`；标题为 None 时 `infer_title` 并 `append_title`（对齐 ADR-0009 D1）。
3. 以 `run_turn_with_hooks` 驱动：
   - **Renderer**：SDK 自己的 `Renderer` 实现，把每个 `DisplayItem` 转发为 `SdkEvent::Display`（含 `Usage`，对齐 runner 的"每条 assistant 消息播一次 Usage"语义）；永不返回 Err。
   - **on_message sink**：每条进历史的消息 → 镜像进 `session.messages` + `SdkEvent::Message { message, usage }` +（持久化）`store.append`（append 失败收集为副作用记录，不中断 turn，对齐 TuiSession 的 best-effort）+ `usage` 累进 `session.usage`（`None` 为工具结果，跳过）。
   - **RunHooks::before_tool**：对齐 CLI——对 `read` 工具命中 skill 自身 `SKILL.md` 的调用，用 `skill_for_read_args` / `skill_loaded_in` / `skill_prompt` 拦截为 `ToolDecision::Skip(Ok(skill_prompt(...)))`（skill 激活块，已加载时给 already-loaded 提示）。SDK 无 skill 时 hooks 不生效。
4. 回合后处理（对齐 `TuiSession::run_compaction`）：
   - `Completed` 且 `compaction::should_compact(&messages)`：`compact_messages(provider, &config, &cancel, &messages)` → 成功则替换 messages、持久化 checkpoint（`store.append`，load 时按最新 checkpoint 丢弃其前记录，ADR-0020 D2）、usage 计入压缩成本、发 `SdkEvent::Compacted { before, after }`；失败则保留原历史（本轮结果不丢），静默（文档化）。
   - `Cancelled`：若本轮已产出消息或日志已存在，`store.append_closing` 写 `Aborted` 收尾消息（ADR-0009 D3）；已流出的 partial 文本只经 `Display` 事件交付，不进历史。
   - `Err(e)`：同样 `append_closing` 写 `Error` 收尾，返回 `Err(e)`。
5. 返回 `StopReason`。

事件顺序（一次 prompt）：`Message`（用户 prompt）→ 流式 `Display` 事件 → 每消息 `Message`（assistant / 工具结果）→ `Display(Stop)` → `TurnFinished { stop_reason }` →（仅发生压缩时）`Compacted`。

### 6. 持久化 vs 内存模式

- `persist: true`：如上，全部走 `SessionStore`（项目分区、追加式 JSONL、配额清理、与 `/session` picker 兼容）。
- `persist: false`：不构建 SessionStore；`session.messages` / `usage` / `title` 全在内存；id 用共享 `new_session_id()`；`append` / `append_closing` 全部跳过。两种模式下回合编排逻辑唯一（持久化点是分支），行为不漂移。

### 7. 不做的事（v1 边界）

- 不解释 `/` 命令、不接输入历史（前端关注点）。
- 不做多会话管理（pi 的 `AgentSessionRuntime`：new/switch/fork/import 留作后续）。
- 不引入异步运行时；SDK 同步，需要非阻塞的宿主自己起线程。
- 不做 FFI（见后续工作）。

## Testing Decisions

### 唯一的测试 seam：`AgentSession` 公共 API

SDK 的全部测试都写在 `AgentSession` 公共 API 层：经 `AgentSessionOptions.provider` 注入脚本化 `FakeProvider`（对齐 `cli/tui.rs` tests 与 `app/runner.rs` tests 的既有先例：scripted delta 批次 + per-call usage + `total_usage()`），真实 `run_turn_with_hooks`、真实（或内存）SessionStore，断言外部行为——事件序列、stop reason、落盘字节、无文件行为、跨线程取消。SDK crate 不暴露任何测试专用入口。

**什么构成好测试**：只断言外部行为（订阅者收到的事件、prompt 返回值、日志文件内容/存在性、并发与取消语义），不断言内部状态或实现细节。

**被测试模块**：`crates/sdk`（针对公共 API 的集成测试；provider 注入即 mock seam）。

**测试先例**：`crates/cli/src/tui.rs` 的 handler tests（FakeProvider 驱动公共入口 + 记录 emit 输出 + 断言落盘）；`crates/app/src/runner.rs` 的 FakeProvider/RecordingRenderer 模式；`crates/app/src/session.rs` 的 store 行为测试。

**测试清单**（行为级）：

1. prompt 流式产出有序 `Display` 事件并返回 `Completed`（含工具回合：Turn/Reasoning/Text/ToolStart/ToolResult/Stop）。
2. 每条进历史的消息产生 `SdkEvent::Message`，assistant 消息带 usage、工具结果 usage 为 `None`。
3. 一次 prompt 的收尾事件是 `TurnFinished { stop_reason }`（Completed / Cancelled 两路）；发生压缩时其后还有一个 `Compacted`（压缩是回合后处理，见决策 5 步骤 4）。
4. 持久化模式：回合后 `<sessions>/<project-key>/<id>.jsonl` 存在、可被 `SessionStore::load` 回放、内容与 `messages` 一致（含 prompt 消息、标题记录）。
5. 内存模式：全程不产生任何文件（临时目录断言为空）。
6. 自动压缩：完成回合且历史越过阈值 → 收到 `Compacted`、messages 含 `CompactSummary`、日志回放为压缩后视图；压缩失败 → 无 `Compacted`、历史不变、turn 结果保留。
7. 取消回合：provider 取消 token → 返回 `Cancelled`、已流出的 partial 文本经 `Display` 事件交付（不进历史）、日志以 `aborted` 收尾（或未产出时无文件）。
8. 失败回合：provider 报错 → `Err` 传播、日志以 error 收尾消息、先前的消息仍在。
9. 跨线程 abort：在另一个线程 `abort()` 一个运行中的 prompt → 尽快以 `Cancelled` 结束。
10. 单飞：一个回合进行中再次 `prompt` → `Err("a turn is already running")`。
11. dispose 后 prompt → 报错；订阅者在 dispose 后不再收到事件。
12. 显式 `base_url`/`model`/`api_key` 经 `chat` seam 传给 provider（对齐 `provider_receives_the_config_crossing_the_chat_seam` 先例）。
13. provider / tools 注入生效（FakeProvider + 自定义工具被执行）。
14. skill read 拦截：模型 read 某 skill 的 `SKILL.md` → 工具结果为 skill 激活块文本（对齐 CLI 行为）。
15. unsubscribe 后监听器不再收到事件。

## Out of Scope

- **FFI 模块（`slimcode-ffi`，cdylib + 纯 C ABI）**：本次不做，见「后续工作」。
- **CLI 重构为 SDK 消费者（薄壳模式）**：本次不做，见「后续工作」。
- **多会话管理**（pi 的 `AgentSessionRuntime`：`newSession` / `switchSession` / `fork` / `importFromJsonl` 等价物）：v1 每次 `create_agent_session` 一个会话，需要新会话就再建一个。
- **`/` 命令语义与输入历史**：前端关注点，SDK prompt 是纯文本。
- **异步 / 非阻塞 API**：SDK 同步；宿主自行起线程。
- **会话分支 / fork / 导入导出**。
- **config.toml 的交互式编辑、`slimcode config` 子命令**（CLI 已有）。

## 后续工作（follow-up，另行文档化）

1. **`slimcode-ffi` cdylib 模块**：纯 C ABI（`extern "C"` + 不透明句柄），暴露 create/prompt/subscribe（事件经回调或 wait+drain）/abort/dispose + last-error；产物 `.so` / `.dylib` / `.dll`，Node（koffi/node-ffi）与 Python（ctypes/cffi）各写薄绑定；napi-rs / PyO3 包装可按需叠加。依赖 `slimcode-sdk`。
2. **CLI 薄壳化**：one-shot 与 TUI 的编排逻辑（`TuiSession::run_turn` 等）收敛为消费 `slimcode-sdk`，CLI 只留 argv / 模式选择 / 终端 / 命令语义；`architecture.rs` 依赖断言相应调整。

## Further Notes

- **术语（写入 CONTEXT.md）**：`SDK`（程序化会话 API crate，CLI 之外的消费面）、`AgentSession`（SDK 的会话对象：prompt/subscribe/abort/dispose）、`SdkEvent`（SDK 事件载荷：Display + Message + TurnFinished + Compacted）、`事件订阅`（subscribe → Unsubscribe）、`FFI 模块`（后续的 C ABI cdylib）。注意与既有 `Session`（CLI/TUI 的会话单元）区分：`AgentSession` 是其 SDK 侧的程序化等价物，复用同一个 `Session` 数据结构。
- **ADR**：补一条记录「sdk + ffi 分层（`sdk` rlib 纯 Rust、`ffi` cdylib 薄 C ABI）、CLI 薄壳化留待后续、SDK 复用 app 层不新增 turn 循环」；与 ADR-0011 的 crate 分层与依赖断言直接相关。
- **docs 同步**：`docs/development.md` 增加 `slimcode-sdk` crate 一节（表格、依赖方向、职责）；`docs/index.md` 的 ADR 摘要与新文档条目同步。
- **对齐 pi 的映射**：`create_agent_session` ↔ pi `createAgentSession`；`AgentSession.prompt/subscribe/abort/dispose` ↔ pi `AgentSession` 同名方法（pi 的 `steer`/`followUp`/`waitForIdle` 因同步模型不必要）；`SdkEvent::TurnFinished` ↔ pi `agent_settled` 的"不再自动继续"权威信号；`persist: false` ↔ pi `SessionManager.inMemory()`；`Provider` 注入 ↔ pi `modelRuntime` 注入。slimcode 是同步架构，不照搬 pi 的 async。
- **性能/线程说明**：监听器在 turn 线程同步送达，若消费者需要跨线程投递（如未来 FFI），由消费方自行经 channel/队列搬运，SDK 不内置。
