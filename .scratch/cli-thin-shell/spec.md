# CLI 薄壳化（one-shot + TUI 编排收敛为消费 slimcode-sdk）

Status: resolved

## Problem Statement

slimcode 的回合编排（prompt 记录、标题推断、best-effort 落盘、失败/取消收尾消息、回合后自动压缩、skill read 拦截）现在**存在两份**：SDK（`AgentSession::prompt`，ADR-0021 已实现）镜像了一份，CLI 的 one-shot（`run_once`）与 TUI（`TuiSession::run_turn` / `run_compaction` / `close_turn`）各持有一份直接驱动 `app::runner::run_turn_with_hooks` 的重复实现。任何编排行为修改（例如收尾消息文案、压缩阈值、skill 拦截规则）都要改三处，行为漂移是常态。ADR-0021 把「CLI 薄壳化」记为明确后续工作：CLI 只留 argv / 模式选择 / 终端 / 命令语义，回合编排收敛为消费 `slimcode-sdk`。同时 SDK 面缺 TUI 需要的两个能力（按需压缩、按 id 续会），且 SDK 默认工具不可取消（`abort()` 杀不死 bash 子进程）——薄壳化正好一并补上。

## Solution

CLI（one-shot 与 TUI）的回合编排全部收敛为消费 `slimcode-sdk`：one-shot 用 `persist: false` 的 `AgentSession`（行为不变，ADR-0009 D5），TUI 的 `TuiSession` 持有 `AgentSession` 作为当前会话对象（`/new` / `/session` 加载时替换），事件经 `subscribe` 同步送达并桥接到现有渲染通道（实时流式不阻塞）。SDK 面按需小扩展：`with_home`（测试/宿主 pin 住 home）、`with_session_id`（凭 id 续会）、`compact_now`（按需强制压缩）、默认七工具改为绑定会话自己的 CancelToken（`abort` 连 bash 子进程一起杀）。CLI 只留 argv / 模式选择 / 终端 / `/` 命令语义 / 输入历史 / 会话 picker 列表 / 显示适配（TextRenderer 与 TUI adapter），以及会话对象管理（创建、切换、dispose 旧会话）。依赖方向：cli → {ai, core, app, commands, tui, sdk}，架构依赖断言相应翻转。外部行为不变：one-shot 不落盘、Esc 仍杀 bash、`/compact` 与自动压缩的渲染一致、加载会话仍回放历史与标题。两条罕见路径的小回退被接受并文档化（见 Implementation Decisions）。

## User Stories

1. 作为 CLI 维护者，我想让 one-shot 的回合编排走 `AgentSession::prompt`（`persist: false`），以便 CLI 不再重复实现 turn 循环，行为收敛到单一实现。
2. 作为 CLI 维护者，我想让 TUI 的回合编排走 `AgentSession::prompt`，以便 `TuiSession::run_turn` / `run_compaction` / `close_turn` 从 CLI 删除，编排逻辑只有 SDK 一份。
3. 作为 CLI 维护者，我想让 one-shot 保持从不落盘，以便 ADR-0009 D5 的行为不变（one-shot 无交互会话工作流、不产生会话文件）。
4. 作为 TUI 用户，我想 Esc 取消仍能终止正在跑的 bash 子进程，以便取消语义不因薄壳化回归（ticket 07 行为）。
5. 作为 SDK 消费者，我想在回合外调用 `compact_now()` 强制压缩，以便不越过阈值也能手动压缩（对齐 `/compact` 语义）。
6. 作为 SDK 消费者，我想在压缩失败时从 `compact_now()` 拿到带 provider 错误详情的 `Err`，以便展示具体失败原因。
7. 作为 SDK 消费者，我想 `compact_now()` 成功后收到与自动压缩同构的 `SdkEvent::Compacted { before, after }`，以便两种路径统一处理。
8. 作为 SDK 消费者，我想在回合进行中调用 `compact_now()` 被明确拒绝，以便单飞语义覆盖压缩（与 prompt 互斥）。
9. 作为 SDK 消费者，我想凭会话 id 创建续会（`with_session_id`），以便宿主恢复既有会话而不是每次新建。
10. 作为 SDK 消费者，我想 `with_session_id` 从内部 store 宽容回放既有会话，以便与 TUI `/session` picker 的加载语义一致。
11. 作为 SDK 消费者，我想 `with_session_id` 搭配 `persist: false` 时得到明确报错，以便不会出现「要加载却无 store」的静默错误。
12. 作为 SDK 消费者 / 测试作者，我想 `with_home` pin 住 slimcode home，以便控制会话存储与 skills/上下文文件发现的位置，而不依赖进程级 env。
13. 作为 SDK 消费者，我想 `abort()` / `dispose()` 能终止运行中的 bash 子进程，以便取消语义对嵌入宿主也完整（默认工具绑定会话自己的 token）。
14. 作为 CLI 维护者，我想配置冻结在启动时的解析值，以便 `/new` 不会静默换 model 导致 footer 与回合不一致（经 overrides 传给 SDK，overrides 优先级最高复现冻结值）。
15. 作为 TUI 用户，我想 `/compact` 与回合后自动压缩渲染一致（usage + 「context compacted: X → Y tokens」），以便两种压缩路径显示一致。
16. 作为 TUI 用户，我想 `/session` 加载会话后仍回放历史、显示标题与 branch、usage 从零开始，以便加载行为与现状一致（repair 计数提示除外，见决策）。
17. 作为 TUI 用户，我想选择当前会话行是 no-op，以便不会丢掉内存中领先于日志的历史（ADR-0018 D3 保持）。
18. 作为 TUI 维护者，我想 worker 线程的 `submit` 与 frame 线程的 `cancel` 共享同一个 `AgentSession`（Arc），以便取消永不等待会话锁（SDK take/put 天然支持）。
19. 作为 TUI 维护者，我想 SDK 事件在 prompt 线程同步送达时桥接到库的 emit 通道（thread-local 槽），以便实时流式不阻塞、渲染顺序不丢失。
20. 作为 TUI 维护者，我想 `/new` / 加载只在 worker 空闲时替换当前 `AgentSession`（dispose 旧会话），以便不会出现替换与回合的竞争（库在回合中忽略普通按键，天然保证）。
21. 作为 CLI 维护者，我想 `slimcode config` 子命令与 `/` 命令语义留在 CLI，以便前端语义不进入 SDK（ADR-0013 D3）。
22. 作为依赖维护者，我想 `cli` 的依赖断言翻转（cli → 含 sdk），以便架构边界由测试锁定（architecture.rs）。
23. 作为 CLI 维护者，我想薄壳化后把 `app::setup`（若确无其它消费者）删除，以便不留死代码。
24. 作为测试作者，我想 SDK 的编排行为测试只走 `AgentSession` 公共 API（provider 注入），以便测试不依赖实现细节、与既有 sdk/tests 风格一致。
25. 作为测试作者，我想 CLI 的 handler 测试经 `AgentSession`（`with_provider` + `with_home`）驱动，以便测试反映薄壳后的真实结构。
26. 作为文档读者，我想有一份 ADR-0022 记录薄壳化决策（依赖反转、SDK 扩展、行为权衡），以便未来维护者理解「为什么 CLI 不直接调 app」。
27. 作为术语读者，我想 CONTEXT.md 的 `CLI` / `SDK` 词条同步薄壳后的现实，以便词汇表不与代码矛盾。

## Implementation Decisions

### 1. SDK 扩展：`with_home` / `with_session_id` / `compact_now` / 默认工具可取消

`AgentSessionOptions` 增加：

- `with_home(PathBuf)`：pin 住 slimcode home（会话存储根、skills 与上下文文件发现、`Environment.global_home` 的来源）。不传时维持现状（`config::slimcode_home()`）。生产路径行为不变；CLI 与测试用它避免二次解析/环境依赖。
- `with_session_id(String)`：创建会话时从内部 store 按 id 宽容回放既有 Session（消息/标题/usage；usage 本就不落盘，加载后从零）。`persist: false` 时直接报错（加载必须有 store）。
- `AgentSession::compact_now()`：强制压缩（无视阈值），与 prompt 单飞（take/put；回合中调用报「a turn is already running」；dispose 后报错），成功同步发 `SdkEvent::Compacted { before, after }` 并返回 `Result<(usize, usize), String>`（失败返回带 provider 错误详情的 `Err`，历史不动），压缩 token 成本计入 `session.usage`（ADR-0018 D4）。内部 `run_compaction` 重构为带 `force` 参数、返回 `Result<Option<(before, after)>, String>`：自动路径失败静默（保持），`compact_now` 传播错误。

默认七工具从 `build_tools(cwd)` 改为 `build_tools_with_cancel(cwd, &session_token)`——工具持有会话自己的 CancelToken（`Arc<AtomicBool>` 克隆共享，prompt 开头 reset、abort/dispose 置位），`abort()` 因此连 bash 子进程一起杀（修掉 SDK 缺口）。`with_tools` 注入不受影响（宿主可注入自定义/不可取消工具）。

### 2. one-shot 收敛（`persist: false`）

`run_once` 改为创建 `AgentSession`（`persist: false` + 冻结配置 overrides + skills 快照）→ `subscribe` 转发 `Display` 事件到 `TextRenderer` → `prompt`。技能触发归一化（前导 `/skill:name` / `/{name}` 注入 skill 激活块）留在 CLI（前端语义）。`TextRenderer` 改为持有 `Box<dyn Write>`（监听器要求 `Send + Sync`），渲染状态跨事件持续。不落盘行为不变（ADR-0009 D5）。

### 3. TUI 收敛（`TuiSession` 持 `AgentSession`）

`TuiSession` 内部状态改为 `Inner { session: Arc<AgentSession>, adapter: Arc<Mutex<TuiAdapter>> }`：

- `submit`（worker 线程）：短暂取锁克隆 `Arc`，释放锁后 `agent.prompt(text)`（锁不跨回合）→ `cancel`（frame 线程）短暂取锁调 `abort()`，永不等待（SDK take/put 保证）。
- `/new` 与 `/session` 加载只在 worker 空闲时发生（库在回合中忽略普通按键）：dispose 旧会话（顺带清空其监听器）、创建/加载新会话、换入 Inner、`SessionChanged` + usage + branch 等渲染照旧。
- 事件桥接：SDK 事件在 prompt 线程（= worker）同步送达，而库的 `emit` 只在 `submit` 期间有效 → **thread-local 槽**：submit / command 设槽、监听器读槽调 emit、返回前清槽（每次触发事件的线程自己设槽；`compact_now` 在 frame 线程触发，CLI 的 `/compact` 用返回值渲染，监听器处理自动压缩的 `Compacted` 事件）。
- `TuiAdapter` 重构为 Send+Sync 状态对象：持有自身 live 视图（`Mutex<LiveUsageView>`），`render(item, emit)` 把 emit 从字段改为按调用传入，新增 `note_message`（Message 事件更新 live：messages push + usage 累加）。监听器路由：`Display` → adapter render；`Message` → adapter note_message（不渲染，流式 Display 已覆盖）；`TurnFinished` → no-op；`Compacted` → 渲染 Usage + 「context compacted」Notice（自动压缩路径）。
- `/compact`（frame 线程）：渲染「compressing context…」→ `compact_now()` → 按返回值渲染 Usage + Notice，`Err` 渲染错误行。
- 加载会话：`with_session_id` 建新会话（SDK 内 load）；**repair 计数提示（skipped/repaired）移除**——仅损坏日志的罕见路径，文档化回退。标题、历史回放、branch、usage 从零照旧。
- 输入历史记录（`Prompt.record`）留在 CLI（submit 内、prompt 之前）。
- `run()` 入口签名收窄：`context_files` / `environment` 由 SDK 内部发现/构建，从 CLI 删除；`config` 保留（footer model 名）；`store`（picker 列表）/ `history` / `skills`（命令语义）保留；新增 `home`（传给 `with_home`）。

### 4. 配置冻结

CLI 启动时解析一次 config（chmod 警告、footer model 名、列表 store 配额），把**解析结果**作为 `with_base_url` / `with_model` / `with_api_key` / `with_cache` 覆盖传给 SDK——SDK 四层解析里 overrides 优先级最高，逐字段复现冻结值 → `/new` / 加载不会漂移。`max_tokens` 不进选项（CLI 的 config 恒为 None，无损失）。`sessions_max_mb` 是 config.toml 单独表、不在 overrides 里：运行中改 max_mb 时 SDK store 与 CLI 列表 store 配额短暂不一致——尽力而为，接受。

### 5. 依赖与架构断言

`cli` 新增依赖 `slimcode-sdk`：cli → {ai, core, app, commands, tui, sdk}。`app` 依赖保留给前端关注点（picker 列表 `SessionStore`、skills 命令/补全 `SkillStore`、config 编辑、渲染映射）——薄壳 ≠ 无 app 依赖，SDK 不代管前端语义。`crates/cli/tests/architecture.rs` 的 `cli_depends_on_every_crate_except_sdk` 断言改为含 sdk。`app::setup`（one-shot 用 `setup`、TUI 用 `setup_with_cancel` 的旧入口）薄壳化后若无其它消费者则删除。

### 6. 已知小回退（文档化）

- `/session` 加载的 repair 计数提示（`skipped N unreadable record(s)` / `repaired N interrupted tool call(s)`）移除（仅损坏/撕裂日志出现）。
- `/install-skill` 后新安装的 skill 在下一个会话（`/new` 或重启）才被 SDK 广告与 read 拦截（skills 在 `create_agent_session` 时冻结）；CLI 的 `/skills` 列表与 `/` 补全立即生效（走 CLI 自己的 SkillStore）。
- 配置在启动值冻结（`/new` 不重读 config.toml）——与现状一致，是特性不是回退。

## Testing Decisions

**好测试**：只断言外部行为（事件序列、返回的 StopReason / 压缩前后 token、落盘字节/存在性、并发与取消语义、渲染输出），不断言内部状态或实现细节。SDK 与 CLI 的编排行为测试分别落在各自 crate，不互相穿透。

**被测模块**：

- `crates/sdk`（公共 API 层，provider 注入即 mock seam）：
  - `with_home`：会话与落盘落在 pin 住的 home 下（store 路径、skills/上下文文件发现位置）。
  - `with_session_id`：续会回放既有 messages/title、续写同一日志、`persist: false` 报错。
  - `compact_now`：小型历史强制压缩成功（`Compacted` 事件 + messages 含 CompactSummary + 返回 (before, after)）、无内容可压缩时返回带详情的 Err、回合进行中调用被拒（单飞）、压缩 token 成本计入 usage、压缩失败带 provider 错误详情且历史不动。
  - 默认工具可取消：以行为级验证（注入的脚本化 provider 驱动）；可取消工具的构造属 app 层既有测试覆盖。
- `crates/cli`：
  - `tui.rs` handler tests：Fixture 改用 `with_home(scratch)` + `with_provider(FakeProvider)` + `with_tools` 经公共 API 建会话（scratch/proj 建 `.git` marker 使 project home 确定性解析到 cwd 本身、`project_key(cwd)` 对齐 SDK 内部 store），断言命令语义、渲染输出、picker 列表、落盘。
  - `render.rs` tests：适配 `TextRenderer` 持有 `Box<dyn Write>` 与 `TuiAdapter` 新签名（`render(item, emit)`）。
  - `main.rs` 测试与 `tui_smoke.rs`：保持（real-binary 冒烟不因薄壳化失效）。

**先例**：`crates/sdk/tests/*`（公共 API + 脚本化 FakeProvider，`sdk/tests/common` 提供 scratch/home_guard/BlockingProvider）；`crates/cli/src/tui.rs` handler tests；`crates/app/src/runner.rs` 的 FakeProvider/RecordingRenderer 模式。

## Out of Scope

- **`slimcode-ffi`**（cdylib + C ABI）：另立后续工作，与本 spec 无关。
- **多会话管理**（pi 的 `AgentSessionRuntime`：new/switch/fork/import）：TUI 只需一个当前会话，`/new`/加载 = 替换 `AgentSession`，不引入运行时。
- **`/` 命令语义与输入历史的 SDK 化**：前端关注点（ADR-0013 D3），CLI 保留。
- **异步 / 非阻塞 API**：SDK 同步；事件经 thread-local 槽桥接，不引入运行时。
- **one-shot 落盘行为改变**：维持 ADR-0009 D5（`persist: false`）。
- **repair 计数提示的保留**：接受移除（决策 6）。
- **skills 发现的新鲜度**：接受冻结到会话创建时（决策 6）。
- **config 交互式编辑（`slimcode config`）**：CLI 已有，不动。

## Further Notes

- **术语（同步 CONTEXT.md）**：`CLI` 词条改为「唯一二进制 = 总入口，消费 `slimcode-sdk`：argv / 模式选择 / 终端 / `/` 命令语义 / 输入历史 / 会话 picker 列表 / 显示适配（TextRenderer / TUI adapter）/ 会话对象管理（创建、切换、dispose `AgentSession`）」——现词条「owns config resolution, service construction, session persistence」与薄壳现实冲突。`SDK` 词条末尾「A future … CLI-thin-shell refactor are follow-up work (ADR-0021)」改为已实现。新词条/更新：`compact_now` 即按需压缩（对齐 `Compaction` 词条）；`with_session_id` 即「按 id 续会」。
- **ADR-0022**「CLI thin-shell over the SDK」：依赖方向反转（cli → sdk）、SDK 面扩展、行为权衡（repair 提示移除、skills 冻结）三条都够格——hard to reverse、surprising without context、真实 trade-off。
- **对齐 pi**：CLI 消费 SDK = pi 的 thin-shell；`compact_now` ↔ pi 手动 compaction 等价物；`with_session_id` ↔ pi 的 load-from-log 等价物；thread-local emit 桥接是 slimcode 同步模型的适配（pi 是 async，无此问题）。
- **性能/线程说明**：事件同步送达 = 零拷贝转发；thread-local 槽只在触发事件的线程设/读，回合期间 frame 线程不触碰（库在回合中忽略普通按键），无竞争。
