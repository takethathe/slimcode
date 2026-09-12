# 并行工具调用（parallel tool calls）

Status: ready-for-agent

## Problem Statement

slimcode 目前一次只期望模型返回一个工具调用，且执行是串行的：

- **请求侧**：请求不携带 `parallel_tool_calls` 参数（百炼默认 `false`），模型一次只返回一个
  `tool_calls` 条目。多文件读取、多地点查询这类相互独立的任务被迫拆成多轮串行往返，
  每轮都重新付一遍上下文开销，延迟高。
- **执行侧**：即使模型返回了多个 `tool_calls`，执行端也没有真正的并发——"并行"开关只把
  结果批量追加，调用本身仍顺序执行，总耗时等于各工具之和。
- **渲染侧**：工具块的配对靠"最后一个同名 pending 块"，同名工具被并行调用多次时必然错配，
  界面显示与实际结果张冠李戴。

## Solution

两端同时开启并行，默认生效、无需配置：

- **请求侧**：开启 `parallel_tool_calls`（仅当声明了 `tools` 时随请求发出，`tools` 为空时请求
  字节与现在完全一致）。模型据此可在一次响应中返回多个相互独立的 `tool_calls`。
- **执行侧**：一个 Tool batch 内的调用**真并发**执行——各自在独立线程上运行，总等待时间接近
  最慢的那个调用，而非各调用之和。
- **语义**：调用结果按模型返回顺序（`index`）进入 history，session log 因此确定、可复现；
  工具开始/结束事件按**完成顺序**流出，UI 先显示先完成的工具；事件携带 `tool_call_id`，
  渲染端按 id 配对，同名工具多次调用不再错配。
- **取消**：Esc 语义与串行一致——已进入 history 的结果保留，未进入的不追加；可取消的 bash
  调用在并发下照常终止进程组。

## User Stories

1. As a 用户, I want 模型一次响应返回多个独立工具调用, so that 多文件/多查询任务少走几轮、
   延迟更低。
2. As a 用户, I want 一个 Tool batch 内的调用真并发执行, so that 总等待时间接近最慢调用
   而非各调用之和。
3. As a 用户, I want 并行结果按模型返回顺序进入会话历史, so that session log 确定、可复现，
   加载后与执行时一致。
4. As a 用户, I want 工具事件按完成顺序显示, so that 先完成的工具先出现在界面上、反馈及时。
5. As a 用户, I want 同一工具被并行调用多次时每个调用独立成块且配对正确, so that 渲染不乱。
6. As a 用户, I want 并行执行时按 Esc 取消与串行一致（已完成的保留、未完成的不追加）,
   so that 取消行为不因并行而漂移。
7. As a 用户, I want 不新增任何配置项即可获得并行能力, so that 升级即生效、无需学习成本。
8. As a 用户, I want tools 为空的请求字节保持不变, so that 纯问答路径不受影响。

## Implementation Decisions

- **请求参数默认开启、不配置化**：`parallel_tool_calls: true` 是默认行为，不新增 config 项。
  仅当请求携带 `tools` 时序列化该字段；`tools` 为空时字段省略，请求字节与改动前一致
  （兼容未知端点，避免无谓参数）。
- **真并发用线程而非异步**：项目无 async runtime、`Tool.run` 是阻塞 I/O（文件、进程），
  用标准线程作用域（scoped threads）实现并发，零新依赖；并发度等于批次调用数
  （模型一次返回几个就开几个，实际通常 2–5 个）。
- **工具闭包约束放宽**：`Tool.run` 从 `Fn + Send` 放宽为 `Fn + Send + Sync`，使一个批次内
  多个线程可共享同一组工具只读调用；现有工具闭包捕获 `PathBuf` / `CancelToken` 等
  `Send + Sync` 数据，全部满足，无需改动任何工具实现。
- **结果顺序与事件顺序分离**：一个 Tool batch 内——
  - 工具结果按模型返回顺序（`tool_calls` 的 `index`）进入 history，并按此顺序发出
    逐消息事件（ADR-0009 的"消息进入 history 即持久化"语义不变，日志顺序确定）；
  - 工具开始/结束事件按**完成顺序**发出（先完成的先渲染，UI 反馈及时）。
- **事件与显示项携带 `tool_call_id`**：工具开始/结束事件及其对应的显示项增加
  `tool_call_id` 字段；渲染端按 id 配对 pending 块（实现时确认不存在无 id 的事件来源，
  故不保留按名字回退）。
- **取消契约保持**：cancel 在批次应用前检查——已进入 history 的结果保留，未进入的不追加；
  线程内 dispatch 前检查 cancel，已取消则不再派发；可取消 bash 照常 kill 进程组。
- **`RunConfig` 默认值改为开启**：并行开关默认 `true`；依赖串行语义的既有测试显式声明
  串行路径。

## Testing Decisions

- **好测试的标准**：只测外部可观察行为，不测实现细节。真并发用"并发峰值"（并发执行期间
  同时在跑的调用数峰值 ≥ 2）验证——这是"真的在同时执行"的唯一可观察信号；顺序语义断言
  history 与事件流的可观察顺序；配对断言渲染输出内容。不测试线程数、不测试调度细节。
- **测试模块（4 个现有 seam，全部复用既有测试模式，无新 seam）**：
  - `slimcode-agent` agent loop 测试（脚本化 `FakeProvider` 模式，先例：`assemble_handles_two_parallel_tool_calls`、
    `cancel_between_parallel_tools_stops_before_applying_the_batch`）——真并发峰值、结果按
    index 进 history、事件按完成顺序、事件带 `tool_call_id`、取消契约。
  - `slimcode-ai` wire 序列化测试（先例：`message_to_wire_*` 系列）——`tools` 非空时
    `parallel_tool_calls: true`，`tools` 为空时字段省略。
  - `slimcode-common` 共享 turn runner 测试（脚本化 `FakeProvider` + `RecordingRenderer` +
    `on_message` sink 模式，先例：`on_message_sink_sees_every_history_entry_in_order`）——
    并行批次下 history 顺序（sink 断言）与事件流顺序（renderer 断言）在共享 seam 上成立。
  - `slimcode-tui` App 渲染测试（`seeded_app` + `render_buffer` 模式，先例：
    `tool_start_pairs_with_result_into_one_block`）——同名工具两次并行调用、结果乱序到达，
    按 `tool_call_id` 配对正确。
- **受影响测试**：`RunConfig::default()` 改为并行后，依赖串行语义的既有 agent 测试显式
  声明 `parallel_tools: false`，断言不变。

## Out of Scope

- **思考模型（thinking model）的兼容性兜底**：默认开启、不按模型降级。官方文档未声明
  思考模型不支持并行；风险（若有）记录于 ADR，作为后续问题的已知项。
- **工具参数的流式输出开关**（`tool_stream`）：现有工具参数流式组装已覆盖分片场景，
  该参数只影响复杂参数的一次性/流式输出策略，与本次无关。
- **并发上限/背压**：模型一次返回几个就并发几个，不设上限（实际数量通常 2–5）。
- **写类工具的并发防护**：`edit`/`write`/`bash` 与读工具一样可并发；写冲突（同一文件的
  读-改-写竞态）由模型负责任地使用并行能力规避，执行端不设锁（全并行，用户已拍板）。
- **配置项**：不新增任何配置项（用户已明确"默认打开，不需要进行配置"）。

## Further Notes

- 默认模型为 `qwen-plus`（非思考模型），默认开启并行是安全的。
- 现有"逻辑并行"（顺序 dispatch + 结果批量追加）路径被真并发替换；串行路径保留
  （单调用批次、显式串行测试场景）。
- 领域词汇（CONTEXT.md）：**Tool batch**（执行单元，结果按 index 进 history、事件按完成序）、
  **Parallel tool call**（请求侧能力）。Dangling tool batch 的修复语义不受影响——
  并行下加载时按 index 补 `Error: interrupted` 结果依然成立。
- 文档同步：CONTEXT.md（术语，已更新）、docs/development.md（agent 运行时循环与工具章节）、
  docs/index.md（ADR 索引）、新增 ADR-0010。
