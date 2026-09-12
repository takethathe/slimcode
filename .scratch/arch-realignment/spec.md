# 架构成型：cli 总入口 + tui 图形库

Status: ready-for-agent

## Problem Statement

slimcode 的用户看不见这个问题，维护它的人躲不开：workspace 的六个 crate 里，分层与依赖方向和
它想成为的东西相反。

- LLM provider 层反向依赖 agent 运行时（`Provider` trait 与消息模型都在运行时里），所以"加一个
  provider"必须先读运行时；
- 前端无关的应用层同时装着应用服务与显示契约，名字（`common`）不说明任何事；
- 交互式 TUI 不是库而是应用：它自己解析配置、构造 provider、发现 skills、读写会话、执行 `/`
  命令语义——想复用它、想给它写测试、想说清"入口在哪"都做不到；
- 一个消息类型同时是发给模型的 wire 消息和会话存储单元，日志专用字段挂在消息上，system prompt
  被真的写进会话与日志。

后果是"cli 是总入口、tui 是图形库"这条目标只能靠口头约定，依赖里表达不出来，测试也断言不了。

## Solution

按 ADR-0011–0014 重切 crate 与依赖方向，并且让依赖方向由测试断言：

- 唯一二进制是 `slimcode`（CLI），它是总入口：argv、模式选择（one-shot 文本 / 交互 TUI）、配置
  解析、服务构建、命令语义、会话落盘、两个模式的显示适配器。
- TUI 是终端图形库：transcript 模型、组件、输入 reducer、帧循环、`Effect`（UI 意图）、
  `RenderItem`（显示词汇）、`UiHandler`（CLI 实现的回调）；不含 argv/配置/provider/会话/命令语义，
  **不依赖任何其他 slimcode crate**。
- LLM 层（`Message`/`Provider`/`ToolSpec`/`Delta`/`CancelToken`）归 `slimcode-ai`；
  agent 运行时（事件、loop、hooks、`AgentMessage`、`convert`、可执行工具）归 `slimcode-core`，
  只依赖 `ai`。
- 前端无关应用层（配置、会话持久化、输入历史、skills、上下文组装、七工具、显示契约）归
  `slimcode-app`：`DisplayItem` / `map_event` / `Renderer` / `run_turn` 保留，ADR-0004 的
  "一个映射、两个模式不漂移"继续成立。
- 消息两层：`ai::Message`（wire）与 `core::AgentMessage`（会话单元，含模型看不到的种类），
  `to_llm` 是唯一转换；system prompt 每轮现组、不进会话；日志专用字段进记录信封。
- 显示链三层：`AgentEvent`（core）→ `DisplayItem`（app，共享映射）→ `RenderItem`（tui，由 CLI 的
  适配器转换）。

对用户而言，这一次改动**没有可见行为变化**：one-shot 输出字节不变，TUI 的帧、交互与命令语义不变，
已有会话照常加载。

## User Stories

1. 作为 slimcode 用户，我希望 one-shot 输出在这次重构后逐字节不变，这样我的脚本与管道不会被破坏。
2. 作为 slimcode 用户，我希望 TUI 的 transcript、footer、补全弹框的外观与交互完全不变，这样这次内部
   重排对我不可见。
3. 作为 slimcode 用户，我希望新版写出的会话能被新版加载，这样我的会话连续性不丢。
4. 作为 slimcode 用户，我希望**改动之前**写出的会话（含 system 记录）仍能加载，这样升级不毁历史。
5. 作为 slimcode 用户，我希望被取消或出错的回合仍然以 assistant 消息收尾，这样恢复会话的行为是确定的。
6. 作为 slimcode 用户，我希望 `--help`、`slimcode config`、`--cwd/--model/--base-url/--api-key/--cache`
   的行为不变，这样我学过的用法不作废。
7. 作为 slimcode 用户，我希望 `/help /new /load /sessions /usage /history /skills /install-skill /!! /!N /exit`
   语义不变，这样肌肉记忆继续有效。
8. 作为 TUI 用户，我希望 Esc 取消在途回合、Ctrl+C/Ctrl+D 退出、Ctrl+J/Shift+Enter 换行都照旧，这样
   入口/退出规则不变。
9. 作为 TUI 用户，我希望流式文本与 spinner 在回合进行中继续刷新，这样长回合仍然"活着"。
10. 作为 TUI 用户，我希望 provider/runner 失败仍在 transcript 内联显示并交回输入框，这样坏回合不杀会话。
11. 作为 TUI 用户，我希望 `/skill:name`、未命中技能的提示、`/` 补全弹框仍然工作，这样技能发现不退化。
12. 作为 TUI 用户，我希望 `/load` 之后 transcript 与 session id 的呈现跟今天一致，这样我不会被串台困惑。
13. 作为 TUI 用户，我希望 `/usage` 的措辞与 one-shot 汇总一致，这样两个模式的数字说法不打架。
14. 作为未来的前端作者（Web/RPC），我希望有一个不依赖终端的应用层，这样我可以复用配置、会话、skills、
    上下文组装而不带上 TUI。
15. 作为未来的前端作者，我希望事件→显示的映射是共享的，这样我的前端不会与 one-shot 悄悄漂移。
16. 作为未来的前端作者，我希望 `run_turn` 只暴露事件与消息回调（不暴露显示类型），这样我可以用自己的
    渲染方式接入。
17. 作为终端 UI 贡献者，我希望 TUI crate 完全没有 slimcode 依赖，这样我可以把它当独立终端组件库开发、
    测试与复用。
18. 作为终端 UI 贡献者，我希望 transcript 的流式合并与工具配对规则留在 TUI 内（连同帧快照测试），这样
    显示逻辑仍是一个深模块。
19. 作为终端 UI 贡献者，我希望 TUI 只通过 `RenderItem` 接收显示指令，这样我不需要认识 agent 事件、
    token 用量类型或会话类型。
20. 作为终端 UI 贡献者，我希望补全候选由外部注入、`/help` 与"did you mean"文案由 CLI 提供，这样 TUI
    不必认识命令注册表与 skills 存储。
21. 作为 CLI 贡献者，我希望所有进程关注点（argv、raw mode、备用屏、信号、panic 恢复、退出码）都在 CLI
    crate，这样"谁拥有进程"只有一个答案。
22. 作为 CLI 贡献者，我希望所有应用关注点（服务构建、命令语义、会话落盘）也在 CLI crate，这样"总入口"
    不被切成两半。
23. 作为 CLI 贡献者，我希望 TUI 的入口接收调用方建好的 terminal 与一个 handler，这样我能在没有真终端的
    情况下驱动与测试它。
24. 作为 CLI 贡献者，我希望通过 `Effect` 应答用户意图、通过 emit 流式送出 `RenderItem`，这样 TUI 永远
    不需要知道配置、会话或 provider。
25. 作为 CLI 贡献者，我希望 worker 线程属于 TUI 的 UI 机制（帧循环不能被阻塞请求卡住），这样我不必在
    CLI 里重写 `FRAME_MS` 轮询与 channel。
26. 作为 provider 贡献者，我希望 LLM seam（`Provider`/`Message`/`ToolSpec`/`Delta`/`CancelToken`）归
    AI crate，这样我新增 provider 时不必 import agent 运行时。
27. 作为 provider 贡献者，我希望 wire 模型保持 serde 容忍（未知字段忽略、可选字段缺省），这样端点的小
    改动不会打断一次运行。
28. 作为 provider 贡献者，我希望工具对 provider 而言只是 schema（`ToolSpec`），这样"能执行什么"与
    "如何请求模型"彻底分开。
29. 作为 agent 运行时贡献者，我希望运行时只依赖 AI crate，这样 loop、事件与 hooks 与前端、存储解耦。
30. 作为 agent 运行时贡献者，我希望 hooks 只摆 seam（事件订阅 + 可选闭包字段，默认关闭时行为不变），
    这样之后的 compact/permission 是插桩而不是重写 loop。
31. 作为 agent 运行时贡献者，我希望运行时对磁盘零 I/O，这样持久化永远属于应用层。
32. 作为应用层贡献者，我希望会话只存"模型看到的东西"加上明确标注的非 LLM 种类，这样会话可跨环境移植。
33. 作为应用层贡献者，我希望 `AgentMessage` 与 `Message` 是两个类型、只有 `to_llm` 一个转换点，这样
    "模型看到了什么"可被审查。
34. 作为应用层贡献者，我希望 system prompt 每轮现组，这样 context files / skills / 环境的变化下一轮就生效，
    而不必改写历史。
35. 作为应用层贡献者，我希望日志专用元数据（终止原因、错误）在记录信封里，这样消息载荷保持 wire 形状。
36. 作为应用层贡献者，我希望显示契约留在应用层，这样两种呈现模式共享同一个事件映射。
37. 作为应用层贡献者，我希望会话日志保持追加式与宽容读，这样崩溃最多丢一个回合，一行坏数据不会让会话
    不可加载。
38. 作为维护者，我希望允许的依赖方向由测试断言，这样一次顺手 `use` 不能悄悄把 crate 重新粘住。
39. 作为维护者，我希望 crate 名字描述其内容（`core`/`app`），这样新人不必先读文档才能找到运行时。
40. 作为维护者，我希望迁移分成每步都保持测试全绿的多个提交，这样我能逐步 review 与二分定位。
41. 作为维护者，我希望这次改动不夹带任何新功能，这样 diff 可以作为"纯重排"审阅。
42. 作为维护者，我希望文档（ADR、CONTEXT.md、development.md）写明目标与"尚未实施"状态，这样人和 agent
    不会对着过时的布局写代码。
43. 作为 agent，我希望每个迁移步骤是一张有验收标准的 ticket，这样我能独立完成一步并知道何时算完成。
44. 作为 agent，我希望目标架构在代码搬动之前就写下来，这样我不必从 import 反推边界。
45. 作为 reviewer，我希望显示链为什么有三套词汇被记录下来，这样我不会把它"简化"掉。
46. 作为 reviewer，我希望 TUI 不得依赖应用层的原因被记录下来，这样我不会为了图方便把依赖加回来。
47. 作为 reviewer，我希望残余风险（两个显示模式可能漂移、TUI 内可能残留应用关注点）被记录，这样我会
    持续盯着它们。

## Implementation Decisions

### 目标 crate 与依赖矩阵

| crate | 对外界面 | 依赖 |
| --- | --- | --- |
| `slimcode-ai` | `Message`(wire) / `Provider` / `ToolSpec` / `Delta` / `FinishReason` / `CancelToken` / `TokenUsage` / wire 模型 | 无 slimcode 依赖 |
| `slimcode-core` | `AgentEvent` / `AgentRunner`（loop + 可选闭包 hook 字段）/ `AgentMessage`(+`to_llm`) / `convert` / `Tool{spec,run}` / `RunConfig` / `StopReason` | `ai` |
| `slimcode-app` | `DisplayItem` / `map_event` / `Renderer` / `usage_summary` / `ContextBuilder`→`Context{system,messages}` / 会话与输入历史持久化 / skills / context_files / 七工具 / setup / `run_turn` | `ai`, `core`, `commands` |
| `slimcode-commands` | `/` 注册表 + fuzzy 预测（纯数据 + 纯函数） | 无 |
| `slimcode-tui` | `RenderItem` / `Effect` / `App`(new/apply/draw/handle_key) / `run(terminal, app, handler)` / `UiHandler` / 组件 | **无 slimcode 依赖** |
| `slimcode`（bin） | 总入口：argv / 模式选择 / 配置 / 服务构建 / 命令语义 / 会话落盘 / `TextRenderer` / `TuiAdapter` / 补全与文案 | 全部 |

重命名：`slimcode-agent` → `slimcode-core`、`slimcode-common` → `slimcode-app`。关键依赖反转：
`Provider` trait 与 LLM `Message` 由 `ai` 拥有。

### 消息模型（ADR-0012）

- `ai::Message` 是唯一的 wire 消息（role + content parts + tool calls + tool_call_id），不再携带
  日志专用字段。
- `core::AgentMessage` 是会话消息单元（今天只有 LLM 变体，compact 等非 LLM 种类后续加入同一 enum），
  带 serde tag；`AgentMessage::to_llm(&self) -> Option<ai::Message>` 是唯一转换点，非 LLM 种类自己
  决定"转化或丢弃"。
- `core::convert(system: &ai::Message, history: &[AgentMessage]) -> Vec<ai::Message>`：system 前缀 +
  `filter_map(to_llm)`，在**每次** provider 请求前调用（因此 run 中途的 compact 下一轮自然生效）。
- `ContextBuilder::build()` 返回 `Context { system: ai::Message, messages: Vec<AgentMessage> }`；
  system prompt 由现组状态（基础提示、环境、context files、skills）合成，**不进会话**，一个回合只新增
  一条 prompt 消息。
- `Session::messages: Vec<AgentMessage>`；日志记录形如
  `{"type":"message","message":{…},"stop_reason":…,"error":…}`（缺省省略）。`load` 跳过
  `role: "system"` 的遗留记录（下一轮重建），不改写文件，不升日志版本（沿用 ADR-0009 D3 的宽容读）。
- 取消/出错的回合仍以 assistant 消息收尾（ADR-0009 D5），原因值改在信封里。

### 运行时与 hooks（ADR-0011）

- `Provider::chat(&[Message], &[ToolSpec], &CancelToken)`：工具对 provider 只是 schema；
  `core::Tool { spec: ToolSpec, run }` 是带执行闭包的包装；loop 每次 run 开头构建一次
  `Vec<ToolSpec>`。
- `AgentRunner` 从自由函数变为结构体：**借用式 per-run 值** —— 持有本轮的 `tools`、
  `RunConfig`、`CancelToken`、事件订阅（`on_event` sink）与 `RunHooks`；`run(provider, system,
  messages)` 每轮接收 provider、system 与 history。三个自由函数（`run_agent` /
  `run_agent_from_messages` / `run_agent_from_messages_sink`）删除，调用点改为"构造 runner + `run()`"。
- `RunHooks` 三个可选闭包字段，默认全 `None`（此时行为与今天逐字节相同），载荷都是
  `AgentMessage`：
  `before_tool(&mut AgentMessage, call_index) -> Result<ToolDecision, String>` —— 整批 call 在
  **任何 dispatch 之前**按 model 序回调；`ToolDecision::Skip(Result<String, String>)` 表示不执行、
  直接用给定结果（仍然产生一条 tool result，保持 call↔result 一一对应）；
  `after_tool(&mut AgentMessage, ok: bool) -> Result<(), String>` —— 每条 result 进 history 与
  发事件之前回调，**完成序**（被短路的 call 也算，排在完成序最前、彼此 model 序）；
  `turn_end(&mut Vec<AgentMessage>, turns: usize, &StopReason) -> Result<(), String>` ——
  `Completed`/`Cancelled` 收尾时回调一次，在 `AgentEvent::Stop` 之前。
  钩子**可以改写**拿到的消息（新增 `AgentMessage::llm_mut`），`Err` 中止整轮；改写在事件与落盘
  之前发生，所以界面、日志、下一轮请求看到的是同一份（ADR-0015）。
  **对"只摆 seam，不实现新语义"的修订**：本票未覆盖该条、review 时发现，随后与用户逐条裁定为
  "可改写的介入 seam"，记录在 ADR-0015。
- `AgentEvent::Message` 的载荷是 `AgentMessage`；运行时不做磁盘 I/O。

### 显示链（ADR-0004 保留 + ADR-0014）

- `DisplayItem` / `map_event` / `Renderer` / `usage_summary` / `run_turn` 留在应用层不变。
- 新增 `tui::RenderItem`，变体：`Text` / `Reasoning` / `ToolStart` / `ToolResult` / `Notice` /
  `Error` / `UserPrompt` / `Usage(FooterUsage)` / `Branch(Option<String>)` /
  `SessionChanged{id}`。`DisplayItem::Turn` 与 `DisplayItem::Stop` 由适配器丢弃。
  **实施偏差（ticket 05）**：`Skills(Vec<SkillInfo>)` 变体（与 `SkillInfo` / `SkillScope`）被删除 ——
  补全候选由注入的 `CompletionProvider` 提供、`/skills` 文本由 CLI 以 `Notice` 产出，TUI 不再持有
  任何 skill 状态；ADR-0014 D1 已加修订注记。
- CLI 实现 `TuiAdapter`（`Renderer`），在 worker 线程上把 `DisplayItem` 转成 `RenderItem`；CLI 自有的
  状态（notice/error/session 变更/skills/branch/usage/user prompt）也以 `RenderItem` 发出——TUI 只有
  一条输入通道。
- `tui::FooterUsage` 提供朴素构造函数；`From<&TokenUsage>` 的转换移到 CLI（TUI 不出现 `ai` 类型）。
- `App::apply(RenderItem)` 取代 `impl Renderer for App`；流式合并、工具配对、状态色与 `Entry` 保持
  TUI 私有。

### 运行 seam（ADR-0013）

- `tui::run(terminal, app, handler)`；raw mode / 备用屏 / 终端标题 / panic hook / 信号 / 退出码在 CLI。
- `UiHandler`：`on_effect(&mut self, Effect, emit) -> ControlFlow`、
  `submit(&mut self, Prompt, emit) -> Result<TurnReport, String>`、`cancel(&mut self)`。
- 帧循环（crossterm 轮询 ~80ms + tick + draw + channel 排空）与 scoped worker 线程留在库内；channel
  元素是 `RenderItem`。
- `App::new` 接收初态（session id、branch、skills、history）与注入的补全提供者；此后一切变更走
  `apply(RenderItem)`。新会话/载入会话、`/help`、"did you mean"、skill 名解析由 CLI 应答并发出
  `RenderItem`。

### 迁移顺序（六张 ticket，见本目录 `issues/01..06`）

1. **01 改名**（`slimcode-agent`→`slimcode-core`、`slimcode-common`→`slimcode-app`）——纯机械 prefactor，
   一次提交全绿，后续所有票都在最终名字上工作。
2. **02 AI 拥有 LLM seam**（`Provider`/`Message`/`ToolSpec`/`Delta`/`FinishReason`/`CancelToken` 进 `ai`，
   `Tool{spec,run}` 留 core，`ai` 的依赖（含 dev）清零）。
3. **03 Session 只存模型看过的东西**（`AgentMessage`/`to_llm`/`convert`、system 出会话、日志信封、
   遗留 system 记录跳过）。
4. **04 TUI 讲自己的显示词汇**（`RenderItem` + `App::apply` + CLI 的 `TuiAdapter`，channel 改载
   `RenderItem`，帧循环暂不动）。
5. **05 TUI 变成 CLI 进入的库**（`run(terminal, app, handler)` + `UiHandler`、服务与命令语义入 CLI、
   终端生命周期入 CLI、补全注入、TUI 依赖清零）。
6. **06 分层被测试钉死 + 文档只说这一版**（依赖矩阵双向断言 + TUI 源码黑名单 + 现状段删除）。

```
01 ──┬─ 02 ── 03 ──┬─ 05 ── 06
     └─ 04 ────────┘
```

每一步都必须：`cargo test` 全绿、`cargo fmt --all`、`cargo clippy --all-targets --all-features --
-D warnings` 0 error / 0 warning，并同步文档。

## Testing Decisions

**好测试的标准**：只断言外部行为——依赖图、公开类型形状、帧快照、文本输出字节、会话日志形状、
`Effect`→`RenderItem` 的映射——不断言内部结构。迁移的硬性要求是**不削弱现有覆盖**：每一步结束后现有
测试套件（frame 快照、session round-trip、loop 脚本测试、tmux 冒烟）都必须仍然存在且通过，只允许把
断言从旧类型改到新类型。

**Seam（尽量少、尽量高）**：

1. **crate 依赖矩阵**（最高 seam，唯一真正新增的测试面）：读各 crate 的 manifest，断言
   `ai` 无 slimcode 依赖、`core` 仅 `ai`、`app` 为 `ai`/`core`/`commands`、`commands` 无依赖、
   `tui` 无 slimcode 依赖、CLI 依赖全部；双向断言（多一条边也要失败）。同一测试内再断言 TUI 源码不出现
   `SessionStore`/`SkillStore`/`Config`。写完后要临时加一条依赖验证测试确实会红，再撤销。
2. **`RenderItem` + `App::apply`**（TUI 的显示词汇）：沿用 ratatui `TestBackend` 帧快照的做法，逐变体
   断言 transcript/footer 的可见结果（含流式合并与工具配对）。
3. **CLI 的 `DisplayItem` → `RenderItem` 适配器**：表驱动单测，逐 `DisplayItem` 变体断言输出（含两个被
   丢弃的变体）；这是"两个模式不漂移"的新防线。
4. **CLI 的 `UiHandler`**：fake handler 录制 `Effect` 与 emitted `RenderItem`，替代今天只能在真终端里
   验证的终端循环集成测试。
5. **沿用不动的高价值既有 seam**：`TextRenderer` 输出字节（one-shot 与 ADR-0004 的保证）、
   loop 的脚本化 `FakeProvider`、`run_turn` 的录制型 renderer、`SessionStore` 的
   round-trip/宽容读/信封/遗留 system 记录跳过、`ContextBuilder` 的组装结果、`skills` 与 `commands` 的
   纯函数测试、`crates/cli/tests/tui_smoke.rs` 的 tmux 端到端（覆盖两腿：文本与 TUI）。

**被测试的模块**：`ai`（wire 容忍、live 冒烟 `#[ignore]`）、`core`（loop、`to_llm`/`convert`、
`assemble`）、`app`（`map_event`、session store、context builder、skills、`run_turn`）、`tui`（`App`
快照、组件纯函数）、CLI（`TextRenderer`、`TuiAdapter`、`UiHandler`+fake、`config` 子命令、架构测试）。

**先例（prior art）**：`crates/agent`/`crates/common` 里的脚本化 `FakeProvider` 与录制 renderer；
`crates/tui/src/app.rs` 的 `TestBackend` 帧快照；`crates/common/src/session.rs` 的日志 round-trip 与
宽容读测试；`crates/commands` 的纯函数表单测试；`crates/cli/tests/tui_smoke.rs` 的真二进制 tmux 冒烟。

## Out of Scope

- 不新增任何功能：hooks 只摆 seam（默认 `None` 时行为不变），不实现 compact / permission / 新命令。
- one-shot 文本输出保持字节不变。
- 不引入 async/tokio，不动 provider 的阻塞边界与超时。
- 不升会话日志版本，不迁移或删除遗留 `<id>.json`（配额驱逐照旧）。
- 不新增 crate，不做全仓格式化，不改用户手册/配置文档里的既有行为描述（只改"两个入口"这类分层叙述）。
- 不实现 Web/RPC 前端；本 spec 只为它留下不被终端绑死的应用层。

## Further Notes

- 决策记录：ADR-0011（分层与总入口）、ADR-0012（两层消息模型）、ADR-0013（TUI 运行 seam）、
  ADR-0014（TUI 零依赖与 cli 侧适配器）；ADR-0004 与 ADR-0009 各有修订注记。
- 术语以 `CONTEXT.md` 为准（`Frontend` / `CLI` / `TUI` / `DisplayItem` / `RenderItem` / `Renderer` /
  `Message` / `AgentMessage` / `Entry` / `Effect` / `Session` / `Session log`）。
- 已知代价（有意接受）：显示链有三套词汇（`AgentEvent` → `DisplayItem` → `RenderItem`），这是 TUI 零
  依赖的代价；两个显示模式的共享只剩 `map_event` 与 `usage_summary`，其余映射各自独立，需靠适配器表
  驱动测试守着。
- 已知残余风险：TUI 源码里可能出现本次断言之外的应用关注点泄漏；`app` 层未来的新服务可能被顺手
  import 进 TUI。二者的防线分别是 review 与依赖矩阵测试。
- 实施前先写文档、后动代码的顺序已执行：目标架构与"尚未实施"状态已在 `docs/development.md` 标注，
  ticket 06 负责删除现状段。
