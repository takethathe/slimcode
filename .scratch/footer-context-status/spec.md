# Footer context status（footer 状态栏显示 context window 信息）

Status: resolved

## Problem Statement

TUI 的 Footer 目前显示 token 统计（`↑in ↓out Rcache Wcache CH%`）和右对齐的模型名，但看不到当前上下文窗口的占用情况——用户无法判断会话距离自动压缩还有多远，也无法感知每个请求对 context 的消耗。同时 provider 的 usage 只在 turn 结束时汇总一次，footer 的 token 统计和 context 占用在整个 turn 进行中都不更新。

## Solution

参照 pi 项目的 footer 实现，在 footer 的 stats 行新增 context 段 `45.3%/200k`（估算 token 占 context window 的百分比 + 窗口大小），按 pi 的阈值分级着色（>90% 红、>70% 黄、否则 dim）。context% 在**每条 assistant 消息完成时**随 usage 信息更新（pi 的 message_end 语义），compaction 完成后、session 切换时也更新。同时把压缩触发语义从"超过 92% 百分比"改为"剩余 context 不足 16k 时压缩"，窗口默认值从 128k 调整为 200k。

## User Stories

1. 作为 TUI 用户，我想在 footer 看到当前 context 占用百分比（如 `45.3%`），以便了解会话距离压缩/溢出还有多少空间。
2. 作为 TUI 用户，我想看到 context 窗口大小（如 `200k`），以便理解百分比的分母。
3. 作为 TUI 用户，当 context 占用超过 70% 时，我想看到警告色（黄），以便提前感知接近阈值。
4. 作为 TUI 用户，当 context 占用超过 90% 时，我想看到错误色（红），以便知道马上要自动压缩了。
5. 作为 TUI 用户，在 turn 进行中（工具循环）我想看到 token 统计和 context% 随每条 assistant 响应实时更新，以便了解每个请求对上下文的消耗。
6. 作为 TUI 用户，启动一个新 session 时，我想立即看到初始 context 占用（`0%/200k`），以便 footer 从第一刻就有 context 信息。
7. 作为 TUI 用户，加载一个已有 session 时，我想看到按恢复历史估算的 context 占用，以便知道该 session 还剩下多少空间。
8. 作为 TUI 用户，自动压缩完成后，我想看到 context% 立刻回落（反映压缩后的新历史），以便确认压缩生效。
9. 作为 TUI 用户，手动 `/compact` 后，我想看到 context% 更新，以便确认手动压缩生效。
10. 作为 TUI 用户，切换 session 时，footer 的 context 段应重置并显示新 session 的初始值，以便不会显示上个 session 的残留数据。
11. 作为 TUI 用户，窄终端下 context 段应随 stats 一起截断，以便 footer 在窄终端不溢出。
12. 作为开发者，usage 信息应随 assistant message 传递（而不是独立的 usage 事件流），以便保持"usage 属于消息"的语义并简化数据流。
13. 作为开发者，provider 的 `chat()` 应直接返回 usage（它本来就在 LLM response 里），以便不需要额外的回调机制。
14. 作为开发者，取消的请求不应产生 usage 信息（也不应累加 `session.usage`），以便 footer 统计只反映真正完成的响应。
15. 作为用户，压缩阈值从"92% 百分比"改为"剩余不足 16k 空 context"（窗口 200k），以便触发语义更直观。
16. 作为开发者，footer 显示的 context% 与压缩触发的估算同源（同一估算器、同一窗口），以便显示和实际行为一致。

## Implementation Decisions

### 1. Provider 层：`chat()` 返回 usage

`Provider::chat()` 的返回类型从 `Result<(), String>` 改为 `Result<Option<TokenUsage>, String>`：

- Bailian 实现：SSE 流 EOF 且非取消时返回 `Ok(Some(usage))`；取消路径返回 `Ok(None)`；HTTP 错误返回 `Err`。
- `last_usage` / `total_usage` 字段保留（provider 自记账，测试在用），但 footer 显示不再依赖 `total_usage`。
- 理由：usage 是 LLM response 的一部分（SSE 末 chunk，`choices:[]` + usage），直接随返回值出来最直接——**否决** `on_usage` 回调（多余的通知机制）和 `Delta::Usage` 变体（污染内容流，`assemble` 需改动）。

### 2. Core 层：`AgentEvent::Message` 携带 usage

`AgentEvent::Message` 变体从单值改为携带 usage：

```rust
Message { message: AgentMessage, usage: Option<TokenUsage> }
```

- AgentRunner 把每次 `chat()` 返回的 usage 暂存，附到本次请求对应的 assistant 消息事件上：
  - `FinishReason::Stop`：最终 assistant 消息携带 usage。
  - `FinishReason::ToolCalls`：`execute_tools` 里发出的 assistant 消息携带 usage；tool result 消息**不带**（usage 只属于 assistant 消息）。
- 取消路径：provider 返回 `Ok(None)` → 无 usage 附上；取消的请求不进入历史。

### 3. App 层：`run_turn_with_hooks` 的 sink 转发 usage

- sink 的 Message 分支：先调 `on_message(&message, usage)`（消息进历史），若有 usage 再 `renderer.render(DisplayItem::Usage(usage))`。
- `on_message` hook 签名从 `FnMut(&AgentMessage)` 改为 `FnMut(&AgentMessage, Option<&TokenUsage>)`。
- `DisplayItem::Usage` 的语义更新：不再只是 frontend-owned，也来自事件流（注释同步更新）。

### 4. CLI 层：`session.usage` 累加 + context 计算

- `on_message` hook 里：`session.messages.push(msg)` 之后 `session.usage = session.usage.saturating_add(usage)`。
- **删除** `run_turn` 末尾的 `usage_before` / `total_usage().saturating_sub` diff 汇总 emit（事件驱动取代它）。
- `run_compaction` 保留 emit（compact 替换消息后 context% 变了，必须重发，且改为带新 context）。
- TuiAdapter 增加 session 访问能力（持有 CLI 的 inner session 引用）：收到 `DisplayItem::Usage` 时读 `session.messages` → 调 app 层估算器 → 组装 `ContextUsage` → emit `RenderItem::Usage { usage, context }`。由于 `on_message` 先于 render 调用，此时 `session.messages` **已含刚进历史的这条消息**——context% 无滞后（与 pi 的 message_end 语义一致）。
- `App::new` 增加 context 参数（启动即显示初始 context%）；new/load session 路径在 SessionChanged 后补发带 context 的 Usage。

### 5. TUI 层：StatusLine + footer 渲染

- `StatusLine` 增加 `context: ContextUsage` 字段。
- 新结构 `ContextUsage { percent: f64, window: u64 }`（无 Option——slimcode 常数窗口 + 自己预估，总可估；无 pi 的 `?` 未知态）。
- `RenderItem::Usage` payload 从 `Usage(FooterUsage)` 改为 `Usage { usage: FooterUsage, context: Option<ContextUsage> }`（Option 用于 SessionChanged 后、首次 Usage 前的短暂过渡，期间 footer 不显示 context 段）。
- footer 纯函数层新增 `context_part(percent, window) -> (String, ContextLevel)`，`ContextLevel { Normal | Warn | Critical }`，阈值照抄 pi：`>90` Critical、`>70` Warn、否则 Normal。纯函数只算文本 + 等级，颜色映射留在渲染层（ADR-0006 D5 的纯函数定位不破）。
- `render_footer` 的 line2 从单 span 变多 span：dim(stats) + 按等级着色的 context 段 + dim(padding + model)。

### 6. Compaction：窗口 + 触发语义

- `DEFAULT_CONTEXT_WINDOW` 从 128_000 改为 200_000。
- 触发条件从 `estimate > window * 92 / 100` 改为 **`estimate >= window - MIN_CONTEXT_REMAINING`**，`MIN_CONTEXT_REMAINING = 16_000`（剩 ≤16k 空 context 时压缩；200k 下即 estimate ≥ 184k 触发）。
- `COMPACT_THRESHOLD_PERCENT` 移除；`KEEP_RECENT_PERCENT = 8%` 保持（200k 下保留 16k 最近历史）。
- footer 的 context 分子与压缩触发同源：`estimate_total_tokens(session.messages)` / `estimated_context_window()`。

## Testing Decisions

测试原则：只测外部行为（emitted RenderItems、渲染结果、返回类型），不测实现细节。沿用各 crate 现有的测试模式，不新增 crate 级 seam。

### Seam 1 — `cli::tui` handler 测试（端到端验收，用户故事级）

- **测什么**：`submit()` 用脚本化 FakeProvider（delta 序列 + 每 call 返回 usage）→ 断言 emit 出的 `RenderItem::Usage` 序列（每条 assistant 消息一个，含 context）；`session.usage` 正确累加；取消的请求不产生 usage。
- **参照**：`submit_streams_a_turn_and_persists_every_message`、`usage_reports_the_sessions_totals`。

### Seam 2 — `tui::app` render 测试

- **测什么**：`App::new(…, context)` + `apply(RenderItem::Usage { usage, context })` → footer line2 渲染 `45.3%/200k`；颜色分级（>90 红 / >70 黄 / 否则 dim）；窄终端截断行为。
- **参照**：`footer_renders_two_dim_lines`、`usage_item_fills_the_footer_stats`。

### Seam 3 — `tui::footer` 纯函数测试

- **测什么**：`context_part` 阈值边界（70/90）、`format_tokens(200_000)` = `"200k"`、ContextLevel 映射。
- **参照**：`stats_parts_omit_zeros_and_hit_rate`。

### Seam 4 — `core::agent` runner 测试

- **测什么**：`chat()` 返回 `Ok(Some(usage))` → `AgentEvent::Message` 携带 usage（Stop 与 ToolCalls 两分支）；取消时无 usage。
- **参照**：runner 现有 scripted FakeProvider 测试。

### Seam 5 — `app::compaction` 测试

- **测什么**：`DEFAULT_CONTEXT_WINDOW = 200_000`；`should_compact` 边界（estimate ≥ 184k 触发、< 184k 不触发；"剩 16k"语义）。
- **参照**：`context_window_is_the_128k_constant`（改名并改断言）、现有 `should_compact` 阈值测试。

### Seam 6 — `ai::provider` 测试

- **测什么**：`chat()` 返回 `Ok(Some(usage))`（EOF 正常）/ `Ok(None)`（取消）/ `Err`（HTTP 错误）；usage 不进 Delta 流。
- **参照**：`chat_finishes_the_tail_event_and_records_usage`。

### 明确不加

- `tui_smoke.rs` 冒烟测试不加 footer context 断言（保持最小）。

## Out of Scope

- **cost 显示**（`$X.XXX`）：slimcode 无成本跟踪，不做。
- **extension status 行**：slimcode 无扩展系统，不做。
- **thinking level / provider 前缀**：slimcode 无这些概念，不做。
- **`(auto)` 后缀**：compaction 无条件自动，恒显示等于噪声，不做。
- **pi 的 `?` 未知态**：slimcode 常数窗口 + 自己预估，总可估，不做。
- **实时流式 context 更新**（每个 text fragment）：usage 只在 response 完成时已知，不做。
- **`on_usage` 回调**：已否决（usage 随 `chat()` 返回值出来）。

## Further Notes

- **术语同步**：CONTEXT.md 的 `Footer` 词条需更新（新增 context 段）；`Compaction` 词条更新（"剩 16k 空"触发、200k 窗口）；新增 `ContextUsage` 词条。
- **ADR-0020 修订**：窗口 128k→200k、触发从 92% 改为"剩 16k"、usage 事件流（跨 4 个 crate 的接口变更，难逆转，有真实权衡——独立 Usage 事件 vs 附在消息上）。
- **与 ADR-0018 D4 的关系**：footer 仍读 `session.usage`，数据源不变，但更新时机从"turn 末"细化为"每条 assistant 消息"。
- **与 ADR-0014 D2 的关系**：TuiAdapter 从纯 mapper 变为可读 session（为算 context%），职责描述需小幅更新。
- **与 ADR-0019 的关系**：usage 走 `chat()` 返回值而非 delta 流；"usage 在末 chunk"的 wire 事实不变，只是传出方式从内部字段改为返回值。
- **时序优势**：`on_message` 先 push 消息再算 context → context% 已含刚完成的响应，无滞后。
