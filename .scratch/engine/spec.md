# slimcode Engine：可移植的通用 agent 循环引擎（bot/code 双类型）

Status: ready-for-agent

> 上游调研：`/Users/takethat/Downloads/Work/slimbot/.scratch/slimcode-sdk-research/research.md`。
> 本 spec 来自 grill-with-docs 访谈，逐条决策见文末 Further Notes 与 ADR-0031。

## Problem Statement

slimcode 今天有两副面孔：一套**同步**的 coding-agent 产品（`AgentSession`：prompt 一轮、固定 `ContextBuilder`、硬编码 `RunConfig`、内置 JSONL 持久化与 turn-end compaction、四事件 `SdkEvent`），和一个把产品整套塞进宿主的 SDK 前端嵌入面（ADR-0021）。它的循环接缝只有 3 个 hook（`before_tool`/`after_tool`/`turn_end`），且不外露到 SDK。

同门的 slimbot（tokio 异步、多通道 bot 运行时）需要的是一个**只借循环骨架、其余行为全部自己控制**的引擎嵌入面：动态上下文（bootstrap 模板 + 记忆 + 每轮 runtime context）、七阶段 phase-hook 管线、自研 trace span、迭代/超时护栏、consolidation 记忆、跨会话队列与取消回滚。调研结论是：按 SDK 现状无法在不丢这些能力的前提下替换 slimbot 的 agent loop——差距不是"缺一两个参数"，而是定位（前端嵌入面 vs 引擎嵌入面）、执行模型与扩展点设计互不兼容。

我们真正想要的不是"让 slimbot 硬套 slimcode SDK"，而是：**把 slimcode 的循环内核升级成一个功能完备、bot/code 通吃的通用 agent 引擎**，其中一切外围行为（context 组装、持久化、压缩、trace、事件、治理）都通过标准 Event/Hook 接缝实现，默认实现只是"内置的那一份 hook"、不享任何特权。本轮范围只在 slimcode 内**把引擎备好**；slimbot 后续通过引入 slimcode SDK 采用它。

## Solution

在现有分层内引入一个**可移植 Engine**（不新增 crate，S1 选 a）：

- **循环内核 = 唯一不可换的内核**：把 `core::AgentRunner` 从 3-hook 扩展为**七阶段 phase-hook 管线**（抬用 slimbot 已验证的 `RunSpec`/`RunState`/`HookAction`/`ToolHookAction` 模型）。一处循环，前端不漂移（延续 ADR-0004 精神）。
- **三类扩展面**：
  - **Operand**（骨架直接调用、按值/句柄注入、天生同步 trait）：`Provider`、`Tool` 执行器，以及消息历史/取消 token/run 配置等骨架输入。
  - **Hook**（横切拦截：可改写 `AgentMessage`、否决/跳过、控制流 `Continue/Abort`）：七阶段挂载点。
  - **Event**（只读观察）：流式增量、消息入历史、turn 结束、compaction 发生、trace span 起止等，经**内部 message-bus** 投递。
- **默认不享特权**：把今天焊在 `prompt()` 里的产品行为降级为**内置默认 hook**——context 组装（environment + AGENTS.md context files + skills）、JSONL 持久化 + title 推断 + close_turn 收尾、turn-end auto-compaction、trace 埋点，全部由 Event/Hook 实现、可被宿主替换或关闭。
- **两个 preset**：`AgentSession` 退化为 **code 型 preset**（Engine + code 默认 hook bundle：七工具 + BailianProvider + AGENTS.md/skills + JSONL + turn-end compaction，不挂 bot 专属 hook，行为与今天的 slimcode 一致）；**bot 型 preset** = Engine + bot 默认 hook bundle（memory/runtime-ctx 注入、governance、persist+rollback、trigger_consolidation、fire_* → 通道广播、accumulate_usage）。
- **async 门面 + 同步内核**：内核保持同步、跑在**专用 OS 线程**；对外暴露 async `prompt`（future）与 runtime-agnostic 的 **`Stream`**（`futures-core::Stream`），**slimcode 不引入 tokio**。slimcode 的 code preset 用同步 provider/tool，零过桥；未来 slimbot 用宿主侧 `block_on` shim 把它的 async provider/tool 包成同步 Operand（桥只出现在宿主适配层，且运行在非 tokio 的专用循环线程上，合法）。
- **跨会话调度留在宿主**：Engine/Session 只保证单会话可取消/可回滚/产事件；同会话串行队列、多会话并发、channel 派发由 slimbot 保留。

## User Stories

1. 作为 slimcode 维护者，我想有一个明确的"循环内核"边界（唯一不可换的部分），以便其余能力都能挂在公开接缝上、而非焊死在 `prompt()` 里。
2. 作为库消费者，我想通过一次构造选择 **code 型 preset**，以便获得与今天 slimcode CLI/TUI/GUI/FFI 完全一致的 agent 行为（回归不破）。
3. 作为库消费者，我想通过构造选择 **bot 型 hook bundle**，以便让同一引擎跑出带记忆、runtime-context、治理、consolidation、trace 的 bot 行为。
4. 作为宿主（未来的 slimbot），我想注入自定义 `Context`（含每轮动态片段），以便实现 bootstrap 模板 + 记忆 + runtime context 组装。
5. 作为宿主，我想在七个阶段各注册 0..N 个 hook，以便把治理、持久化、事件、压缩等全部编排成我自己的管线。
6. 作为宿主，我想让 pre_tool hook 返回 `Execute/Skip/Abort`、post/after hook 返回 `Continue/Abort`，以便在循环边界做流程控制（跳过某工具、整轮中止）。
7. 作为宿主，我想 hook 能改写即将进历史的 `AgentMessage`，以便模型、会话日志与显示三处看到同一份文本（延续现有 hook 特权语义）。
8. 作为宿主，我想订阅引擎的**事件流**（流式文本/推理/工具起止/每消息 usage/停止标记/turn 结束/压缩发生），以便展示、埋点或转发。
9. 作为宿主，我想事件经**有界 message-bus** 投递并带背压，以便一个慢订阅者（如 SSE）不会拖住循环内核线程。
10. 作为宿主，我想在 turn 结束后读到"已把取出的 provider/tools/session 放回"的权威终态事件（`TurnFinished` 后跟可选 `Compacted`），以便读取会话状态时看到本轮结果而非 turn 前快照（保持现有 SDK 顺序契约）。
11. 作为 bot 宿主，我想让 **trace** 作为刚需不退化：注入 `Tracer` 组件、由骨架内建产出 `iteration` / `provider.chat` / `tool.{name}` 三类 span，以便还原 provider 请求级 span（延迟、token、请求体 recorder）。
12. 作为 bot 宿主，我想 provider 拿到 `SpanRecorder` 以便即使传输失败也能把请求体属性落在 `provider.chat` span 上（对齐 slimbot ADR-0001 自研 span 模型）。
13. 作为 bot 宿主，我想把持久化做成一个可替换的默认 hook（而非 SDK 内建硬编码），以便换成我自己的会话模型或关掉它。
14. 作为 bot 宿主，我想把 compaction 做成 turn-end hook，以便换成 consolidation（写 memory + history.jsonl）或关闭。
15. 作为 bot 宿主，我想配置 `max_iterations` 与超时护栏，以便长循环可控（引擎默认仍可不设上限，供 code 型使用）。
16. 作为 bot 宿主，我想 per-run 记录 `initial_message_count` 并在取消/失败时回滚到该游标，以便会话状态一致（跨会话队列仍由我自己持有）。
17. 作为宿主，我想引擎保证单会话 single-flight（同一会话一次只跑一轮），以便并发语义可预期。
18. 作为 async 宿主，我想以 `prompt`-future + `Stream` 消费引擎而**不被迫引入 tokio 到 slimcode**，以便 slimcode 保持 runtime-agnostic、我在自己运行时里驱动。
19. 作为 async 宿主，我想在专用循环线程上用 `block_on` shim 把我的 async provider/tool 适配成同步 Operand，以便复用我现有的异步 provider/tool。
20. 作为 slimcode code 型消费者，我想同步 provider/tool 完全不过桥，以便行为与性能与今天一致。
21. 作为 FFI 宿主（Node/Python），我想 C ABI 仍是同步窄接口、不驱动循环，以便 async 门面改动不波及 FFI ABI。
22. 作为 GUI 宿主，我想继续用 `AgentSession`（现为 code preset over Engine）而不改渲染/事件接线，以便 GUI 不随本轮引擎改造被迫 async 化。
23. 作为测试作者，我想只通过 **Engine 公共 API** 注入脚本化 provider + 自定义 hook bundle 来验证全部行为，以便测试不依赖实现细节。
24. 作为测试作者，我想用 `TestTraceBackend` 断言 span 起止与属性，以便验证 trace 刚需不丢失。
25. 作为测试作者，我想用 `futures::executor::block_on` 驱动 async 门面并拉 `Stream`，以便在无 tokio 的前提下测异步边界。
26. 作为 slimcode 维护者，我想把默认 hook 组织成"code preset / bot preset 两份 bundle"，以便两类型共享同一引擎、差异只在挂哪些 hook。
27. 作为 slimcode 维护者，我想七阶段内核仍是一处循环（扩展 `AgentRunner`，不新写第二套 loop），以便行为不漂移。
28. 作为规划者，我想本轮只交付"slimcode 内备好引擎 + code preset 回归"，以便 slimbot 引入是独立可控的后续步骤。
29. 作为库消费者，我想 `ProviderConfig` 的可用参数（如 temperature）在需要时可透传，以便多 provider 配置不因接缝缺字段而退化。
30. 作为维护者，我想跨会话调度明确留在宿主、不进引擎公开面，以便 code 型不被迫吃一套它用不上的调度器。
31. 作为维护者，我想 code preset 重构后与今天的 slimcode 行为**逐点等效**（事件序列/落盘/压缩/取消/技能拦截），以便迁移是纯结构改造、不破任何现有前端。
32. 作为维护者，我想用端到端**集成测试**把"迁移前基线 vs 迁移后 code preset"的外部输出钉死为一致，以便任何行为漂移在合并前就被拦下。

## Implementation Decisions

### 1. 分层落点（S1 = a，不新增 crate）

- **七阶段循环内核**在 `core`：扩展 `core::AgentRunner`，把现有 3-hook 接缝扩为七阶段管线（仍是唯一循环）。
- **默认 hook 组件 + 引擎公开接缝 + async 门面**在 `app`（本就是前端无关引擎层）。
- **`slimcode-sdk` 的 `AgentSession`** 退化为 code preset over Engine；CLI/TUI/GUI/FFI 继续消费 `AgentSession`，本轮不强制它们改用新接缝。
- `crates/cli/tests/architecture.rs` 的依赖断言相应微调（不新增 crate、不改分层方向，只允许接缝类型在既有依赖边内流动）。

### 2. 七阶段管线（抬用 slimbot 模型，schema 决策）

阶段与流控（`RunSpec` 不可变协作者 + `RunState` 可变、跨阶段 `extensions` type-map）：

```text
phases:   pre_turn  pre_chat  after_chat  pre_tool  after_tool  post_iteration  after_turn
flow:     HookAction      = Continue | Abort{ reason }              # 非工具阶段
          ToolHookAction  = Execute | Skip{ result } | Abort{ reason } # pre_tool
          after_turn      -> Result<()>                              # 恒运行，含取消
hook sig: async-free & 同步: fn(&RunSpec, &mut RunState[, &ToolCall| &result]) -> Result<Action>
```

- `pre_chat`/`after_chat` 承载 provider 调用前后（响应处理、should_exit 判定、治理）。
- `pre_tool`（每 call、模型顺序、并行模式下批次全部先跑）决定执行/跳过/中止；`after_tool`（完成顺序）改写结果入历史。
- `post_iteration` 每轮工具批次后；`after_turn` 无论成败/取消恒运行，承载 rollback、consolidation 触发、终态事件。
- 与现有 `before_tool`/`after_tool`/`turn_end` 三 hook 的关系：三 hook 被七阶段吸收/泛化；skill read 拦截迁移到 `pre_tool` 的 `Skip`。

### 3. 三类扩展面与"默认不享特权"

- Operand：`Provider`、`Tool`（**同步 trait**），骨架直接调用；消息历史、cancel token、`RunConfig`（新增可配 `max_iterations`/超时；默认仍可不设上限供 code 型）作骨架输入。
- Hook：七阶段挂载点；可改写 `AgentMessage`（保持现有"模型/日志/显示看到同一份文本"语义）。
- Event：只读事件流，经内部有界 channel（message-bus）投递，带背压、吸收突发；`AgentEvent`/`DisplayItem` 复用现有事件词汇。
- **内置默认 hook**（可替换/可关）：`build_context`（environment + context files + skills + history + prompt）、`persist_assistant`/`persist_tool_message`/`write_user_message` + title + `close_turn`、`trigger_compaction`（turn-end、checkpoint 语义）、`fire_*` 事件。
- **bot 专属、可选 hook/collaborator**（code 型不挂即无感）：memory 注入、per-turn runtime-ctx、governance（drop_orphan/backfill/microcompact/snip）、rollback 游标、channel 广播、consolidation。

### 4. Trace（刚需、不退化）

- `Tracer` 作为注入协作者；**骨架在操作数调用处内建 RAII span**：`iteration` / `provider.chat` / `tool.{name}`，`Tracer=None`（或无 backend）时 no-op。
- `SpanRecorder` 透传进 `Provider` 操作数，使请求体属性在传输失败时仍落在 `provider.chat` span（对齐 slimbot `TraceBackend`/`SpanRecorder` 与 slimbot ADR-0001 自研 span 模型）。
- 事件流不承载 provider 请求级 span；trace 不走"纯 hook 还原"路线（会丢 recorder 与失败记录能力）。

### 5. async 门面 + 同步内核（桥接）

- 门面把同步内核放到**专用 OS 线程**运行；对外提供 async `prompt`（future）与 **runtime-agnostic `Stream`**（`futures-core::Stream`）作为事件输出，经内部 channel 衔接。
- **slimcode 全栈不引入 tokio**；facade runtime-agnostic，宿主在自己运行时驱动；slimcode 测试用 `futures::executor::block_on`。
- Operand 保持同步 trait：async provider/tool 由**宿主侧 `block_on` shim** 在专用循环线程上适配（该线程非 tokio worker，`block_on` 合法）；slimcode code preset 用同步 provider，零过桥。
- **顺序契约**：turn 终态事件（`TurnFinished` → 可选 `Compacted`）必须在取出的 provider/tools/session 放回之后投递，bus 不得让订阅者读到 turn 前快照（延续 SDK 现有语义）。

### 6. 单会话边界

- 引擎/Session 层：单会话、可取消（`CancelToken`）、可回滚（`initial_message_count` 游标经 hook 表达）、产事件、single-flight。
- **不进引擎公开面**：跨会话并发调度、同会话串行队列、背压派发、多 AgentSession 生命周期——留在宿主（slimbot 保留 SessionRunner/MessageBus）。

### 7. 逻辑等效保证（迁移不破行为）

- **等效范围 = 公共可观测面**，不含内部实现：code preset 迁移后，事件序列与相对顺序、`StopReason`、落盘字节与会话日志回放、usage 累加、title 推断、close_turn 收尾、turn-end compaction 触发与 checkpoint、skill read 拦截文本，必须与今天 `AgentSession` / `run_turn_with_hooks` 的现有实现**逐点一致**。七阶段重排、hook 拆分、context 组装移到默认 hook 等都是**内部实现**，不得改变外部结果。
- **差量/金标准基线**：以迁移前实现为基线，把其外部输出（有序事件流 + 返回值 + 落盘产物字节）固化为一组脚本化输入（同种 FakeProvider + 同种工具/回合）上的期望；迁移后引擎跑**同一组输入**须产出**逐事件/逐字节一致**的输出。
- **有意分歧须显式豁免**：若内部改动导致事件微序等外部可观测变化，必须在 ADR/spec 记录并证明外部不可观测等价或属既定改进，否则视为回归、不予合并。
- **bot 型接缝是纯增量**：就位 bot 专属 hook/operand/collaborator 不得改动 code preset 走的默认路径（code 型不挂即无感），以保证等效基线不被新增接缝污染。

## Testing Decisions

**什么构成好测试**：只断言外部行为——事件流序列与顺序、终态 hook 结果、prompt 返回值/stop reason、落盘字节/存在性、span 起止与属性、hook 流控（Skip/Abort/Execute）生效、取消/回滚语义——不断言内部状态或实现细节。

**主 seam（唯一高 seam）：Engine 公共 API**。经构造选择 preset（code）或自定义 hook bundle + 注入脚本化 `FakeProvider` + 注入 tools，跑一轮，在 seam 上断言全部外部行为；trace 用 `TestTraceBackend`；持久化用真实/内存 `SessionStore`。

**次 seam（新增、必测）：async 门面**。用 `futures::executor::block_on` 驱动 `prompt`-future 并 `StreamExt::next` 拉事件，断言跨线程事件顺序与"终态事件在 put-back 之后"；断言 slimcode 不出现 tokio 依赖。

**被测试模块**：`core`（七阶段管线流控与 hook 改写）、`app`（默认 hook bundle、trace、async 门面）、`sdk`（code preset 回归 = 今天的 AgentSession 行为）。

**测试先例**：`app::runner` 的 FakeProvider/RecordingRenderer；`cli/tui.rs` handler tests（FakeProvider 驱动公共入口 + 记录输出 + 断言落盘）；`sdk` 现有 AgentSession 集成测试；slimbot `runner/tests.rs`（七阶段 + 默认 hook 复刻行为）与 `trace/tests.rs`（backend 断言）作跨类型对齐参考。

**测试清单（行为级）**：

1. code preset 一轮：流式 `Event` 有序产出 + 返回 `Completed`，与今天 `AgentSession` 事件序列一致（回归）。
2. 自定义 hook bundle：`pre_tool=Skip` 使工具不执行但仍产配对工具结果；`Abort` 使整轮跳到 after_turn。
3. hook 改写 `AgentMessage`：模型请求、会话日志、事件流三处文本一致。
4. 默认 context hook 可被替换：注入自定义 system/每轮片段生效。
5. 默认持久化 hook 可关：`persist:false` 全程不产文件；开启时 JSONL 可被 `SessionStore::load` 回放。
6. turn-end compaction hook：越过阈值产 `Compacted` 且日志按最新 checkpoint 回放；失败保留原历史与本轮结果；换成 consolidation hook 时行为改变。
7. trace：注入 `TestTraceBackend` → 收到 `iteration`/`provider.chat`/`tool.{name}` span 起止与属性；`Tracer=None` 时 no-op 且不报错；传输失败时 recorder 属性仍落在 `provider.chat`。
8. 取消：`CancelToken` 置位 → `Cancelled`；partial 文本只经事件流、不进历史；日志以 aborted 收尾；回滚到 `initial_message_count`。
9. single-flight：同会话一轮进行中再次驱动 → 明确错误。
10. async 门面：`block_on` 驱动 `prompt` + 拉 `Stream`，事件顺序与同步内核一致；终态事件在状态 put-back 之后。
11. operand 过桥 shim：把一个 async provider 包成同步 Operand 后能正常完成一轮（用测试内 mini-runtime 模拟宿主 `block_on`）。
12. FFI/GUI 面不变：`AgentSession`（code preset）对外签名与事件词汇不破坏现有 C ABI / GUI wire 事件。

**等效性集成测试（迁移护栏，端到端、测外部行为非实现细节）**：写在 `crates/sdk` 与 `crates/app` 的 `tests/` 集成层，用**真实引擎 + 脚本化 FakeProvider + 真实或内存 `SessionStore`**（不 mock 循环本身），对同一组输入比对"迁移前基线 vs 迁移后 code preset"的外部输出。基线在迁移开始前先固化（red 阶段跑通基线，green 阶段让新引擎匹配）。

13. 等效-事件序列：含工具回合的脚本 provider，在迁移前后产出**完全有序**的事件流（`Display`/`Message`/`TurnFinished`/`Compacted` 的内容与相对顺序逐项一致）。
14. 等效-持久化：同一 prompt 序列下 JSONL 会话日志**逐记录字节一致**，可被 `SessionStore::load` 回放为相同 messages/title。
15. 等效-usage / title / stop reason：累计 usage、title 推断结果、每回合 `StopReason` 与基线一致。
16. 等效-compaction：越过阈值的同一历史，迁移前后产出相同 `CompactSummary` checkpoint 与 `Compacted`（before/after token 一致或差值等价）；压缩失败两路同样保留原历史与本轮结果。
17. 等效-cancel / error：取消与 provider 报错两路的收尾消息、"partial 文本只经事件不进历史"、日志 aborted/error 收尾与基线一致。
18. 等效-skill 拦截：模型 `read` 某 skill 的 `SKILL.md` 得到的激活块文本与基线**逐字一致**（含 already-loaded 提示分支）。
19. 等效-纯增量：新增任一 bot 专属 hook/collaborator 后，重跑上述 13–18 全部仍通过（证明 code preset 默认路径未被污染）。

## Out of Scope

- **slimbot 采用**：slimbot 废弃自有 `AgentRunner`、改为依赖 slimcode SDK——后续，不在本轮。
- **跨仓库依赖与版本**：slimbot→slimcode 的 path/crates.io 依赖、中立第三仓——本轮不做。
- **跨会话调度器**进引擎公开面：明确留宿主。
- **把 `core`/`app` 内核改 async**：内核保持同步；async 只在门面。
- **CLI/TUI/GUI/FFI 全量迁到新接缝**：它们继续走 `AgentSession`（code preset）；增量迁移另立工作。
- **`/` 命令语义与输入历史**：仍是前端关注点，引擎不解释。
- **FFI C-ABI async 化**：FFI 保持同步窄接口。

## Further Notes

- **术语（写入 `GLOSSARY.md`）**：`Engine`（可移植循环引擎 = 不可换内核 + Event/Hook/Operand 三面）、`Phase hook`（七阶段横切接缝）、`RunSpec`/`RunState`（注入协作者 / 跨阶段可变状态）、`HookAction`/`ToolHookAction`（流控）、`Operand`（骨架直调的同步 Provider/Tool）、`Preset`（code/bot = 引擎 + 一份默认 hook bundle）、`Async 门面`（runtime-agnostic prompt/Stream + 专用循环线程）、`Event bus`（有界背压的事件投递）。
- **ADR**：新增 `docs/adr/0031-portable-engine-seven-phase-hook-pipeline.md`，记录本决策与 §4-A/B/C 取舍（同步内核 + async 门面、抬用 slimbot 七阶段管线为公开接缝、默认 hook 不享特权、trace/持久化/压缩皆接缝、跨会话留宿主、slimbot 后采）。
- **docs 同步**：`docs/index.md` 增 ADR-0031 摘要；`docs/development.md` 说明 `app` 引擎接缝与 async 门面、trace 组件；与 ADR-0021 的关系在 ADR 中点明（AgentSession 由"前端嵌入面"改为"code preset over Engine"，SDK 定位不变但底层多出一层可移植引擎）。
- **对齐 slimbot 的源**：七阶段与流控取自 slimbot `src/runner/traits.rs`、`src/runner/types.rs`（`RunSpec`/`RunState`/`HookAction`/`ToolHookAction`）；trace 取自 `src/trace/mod.rs`、`src/trace/span.rs`、ADR-0001；async provider 形状参考 `src/provider/mod.rs`（仅作宿主侧 shim 目标，slimcode 内核不采用 async trait）。
- **回归优先级**：先保证 code preset = 今天 slimcode 行为（AgentSession 事件/持久化/压缩/skill 拦截不变），再就位 bot 型所需接缝；bot 型完整端到端验证待 slimbot 引入轮次。
