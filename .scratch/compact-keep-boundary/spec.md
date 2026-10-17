# Compaction 持久化保留边界（Kept Boundary）

Status: ready-for-agent

## Problem Statement

slimcode 的 compaction 把一段旧历史浓缩成一条 `CompactSummary` checkpoint，保留最近 16k token 的近期消息（Kept tail）。但 checkpoint 落盘时是**追加**在那段 Kept tail 之后的，磁盘上的顺序是 `[…old, …kept, compact, …new]`；而内存里 `compact_messages` 产出的顺序是 `[compact, …kept]`。

当前 [`SessionStore::load`](../../../crates/app/src/session.rs) 用 `rposition(is_compact_summary)` 找到最新 checkpoint，然后 `drain(..index)` 把它之前的**一切**丢弃。因为 Kept tail 物理上排在 checkpoint 之前，它也被一并丢掉。结果是 ADR-0020 D2 记录的那个"已知代价"：

- 活动 session 里是 `[compact, kept…, new…]`；
- 一旦 `/session` 重新加载，恢复出来的是 `[compact, new…]`——**Kept tail 跨 resume 丢失**。

用户视角的问题：同一段对话，"正在用的时候"和"关掉再打开之后"喂给模型的内容不一样。恢复 session 后，模型看不到刚压缩时特意保留的那段精确近期上下文，只能靠摘要；footer 的 ContextUsage 与再压缩时机也随之偏小。

## Solution

采纳方案 (a)：让 checkpoint 自己携带一个**保留边界**。`CompactSummary` 新增 `kept_count`——本 checkpoint 保留了多少条紧邻它的近期消息。`load` 据此在恢复时**只丢弃 old、保留 Kept tail**，并把 checkpoint 提到 Kept tail 之前，重建出与活动态逐条一致的 `[compact, kept…, new…]`。

核心不变量（都在既有约束内，不推翻 ADR）：

- 日志仍是 append-only（ADR-0009）：只是 checkpoint 记录多一个字段，从不回写、不删除已有行。
- 记录仍不带消息 id（ADR-0009 D1）：边界用 `kept_count` 条数表达，不引入 pi 的 `firstKeptEntryId`。
- 边界应用仍落在 `load`（ADR-0020 D2）：`ContextBuilder::build()` / `inject_compaction` 保持"纯保位替换"不变。

## User Stories

1. 作为 TUI 用户，我把一个刚压缩过的 session 关掉再用 `/session` 打开后，希望它喂给模型的近期上下文和压缩当时一模一样，以便恢复不改变对话质量。
2. 作为用户，我希望 compaction 特意保留的"精确近期尾部"能跨 resume 存活，以便刚讨论完的细节不会因为重新加载而只剩摘要。
3. 作为 CLI one-shot 用户，我不受影响——one-shot 没有持久 session，从不触发压缩。
4. 作为用户，一个在本次改动之前生成的旧 session（其 checkpoint 没有 `kept_count`），重新加载时应保持今天的既有行为（丢弃 checkpoint 之前的一切），以便旧记录不会报错或被曲解。
5. 作为用户，同一 session 被压缩多次后，我恢复时仍希望看到的是"最新摘要 + 它保留的近期尾部 + 之后新消息"，旧摘要与被它们取代的记录都消失，以便历史干净且与活动态一致。
6. 作为开发者，我希望 checkpoint 记录新增的字段是纯 additive 的：新版读旧记录取默认、旧版读新记录忽略未知字段，以便不需要日志格式版本迁移。
7. 作为开发者，我希望 `kept_count` 由 `compact_messages` 在生成 checkpoint 时就确定并写入，`load` 只是消费它，以便"何时决定保留多少"与"如何恢复"分居两个职责清晰的层。
8. 作为开发者，恢复时的保留窗口边界保证落在一个完整 tool batch 之后（Kept tail 起点是 assistant、绝不从 tool result 起），以便存活历史里每个 `tool_call_id` 仍配对。
9. 作为开发者，我希望悬空 tool-batch 的内存修复发生在裁剪到保留窗口**之后**，以便不给"马上要被丢弃的 old 批次"凭空造 `Error: interrupted` 结果。
10. 作为 SDK 使用者（GUI / FFI 宿主），`compact_now` 与自动压缩路径都应让落盘的 checkpoint 带上正确的 `kept_count`，以便任意前端的恢复行为一致。
11. 作为 SDK 使用者，我希望 `AgentSession` 以 `with_session_id` 恢复一个刚被 `compact_now` 压缩过的 session 时，`messages()` 返回 `[compact, kept…, …]`，以便程序化消费者看到和活动态一致的历史。
12. 作为用户，恢复后 footer 的 ContextUsage 百分比可能比"只剩摘要"时更大（因为 Kept tail 被真正恢复了），我接受这一可见变化，以便显示口径和实时 session 对齐。
13. 作为维护者，我希望这次的决策（用 count 而非 id、先裁剪后修复、legacy 取 0）被记成一份新 ADR，并且 ADR-0020 那条"已知代价"被标注为已被取代，以免后来者重复"发现"这个问题或误改。

## Implementation Decisions

### 1. `CompactSummary` 新增 `kept_count` 字段

- 位置：`AgentMessage::CompactSummary`（`slimcode-core` 的 session 消息模型）。
- 字段：`kept_count: usize`，语义为"本 checkpoint 保留的、紧邻它之前的非-checkpoint 消息条数"。
- serde：`#[serde(default)]`，因此 legacy 记录反序列化为 `0`（等价于今天的"丢弃 checkpoint 之前一切"）；新记录写入真实值。这是纯 additive 变更，未知字段被默认忽略，旧读取者读新记录不受影响。
- `compact_summary` 构造器与 checkpoint 的产生处随之多带一个 `kept_count` 入参；其余访问器（`to_llm`、`role`、`is_compact_summary`、`text_content` 等）不变——checkpoint 仍是 session-only、report `Role::User`、`to_llm` 返回 `None`。

### 2. `compact_messages` 计算并写入 `kept_count`

- 保留尾部即结果里 checkpoint 之后的那段（`kept` = 过滤掉 checkpoint 后的 `messages[start..]`）。
- 产出 `[CompactSummary { kept_count = kept.len(), … }, …kept]`。
- 边界情形"最新单条就超过保留预算"→ `kept` 为空 → `kept_count = 0` 且合法（"全总结、零保留"），与新 checkpoint 恒为内存首位的事实一致（`kept_start` 返回 `0` 会先报错，故上一个 checkpoint 必落入被总结区间，永不出现在 Kept tail 内）。

### 3. `load` 的 checkpoint 边界重建（取代旧 `drain`）

- 重放记录（含坏行跳过、撕裂尾 seal）后，定位**最新** checkpoint 及其 `kept_count = N`。
- 从最新 checkpoint 位置**向前**收集 `N` 条**非-checkpoint** 消息作为 Kept tail，途中**跳过**更旧的 checkpoint（多次压缩时旧 checkpoint 物理夹在 kept 与 new 之间，必须跳过，见 ADR-0025 权衡）；收集满 `N` 即止。
- 更早的一切记录（含所有更旧 checkpoint）丢弃。
- 重建为 `[最新 checkpoint] ++ Kept tail ++ [最新 checkpoint 之后的记录]`，即把 checkpoint 提到 Kept tail 之前，产出与活动内存逐条一致的顺序。
- `ContextBuilder::build()` / `inject_compaction` 不改：仍是保位替换，据此把 checkpoint 就地渲染成一条 `user` 总结，模型读到 `[summary, kept…, new…]`。

### 4. 修复与裁剪的顺序

- 悬空 tool-batch 的内存修复（`close_batch`）作用在裁剪到保留窗口**之后**的存活历史上，避免为将被丢弃的 old 批次凭空补 `Error: interrupted` 结果。
- 保留窗口起点由 `compact_messages` 侧保证不落在 tool result 上，故存活历史自带配对，修复只是兜底。

### 5. 触发/估算层无需改动，但接受其后果

- `should_compact` / `estimate_total_tokens` / footer 的 `ContextUsage` 都基于"恢复后的 `session.messages`"计算，代码本身不变。
- 后果：恢复 session 因 Kept tail 被真正保留，估算 token 变大、再压缩时机比"只剩摘要"时更早——这是刻意为之的对称性，写入 ADR 的 Consequences。
- legacy（`kept_count = 0`）session 恢复后仍是小历史，不受影响。

## Testing Decisions

好的测试只断言**外部行为**（落盘记录的形状 + 恢复出来的消息序列），不断言内部实现细节。全部复用**已有 seam**，不新增 seam：

- **主 seam — `slimcode_app::session::SessionStore::load`**（`crates/app/src/session.rs` 的单元测试，已有 `write_raw`/`msg_line` 等原始日志构造工具）：
  - 原始日志 `[…old…, kept…, compact(kept_count=2), after…]` → 断言 load 还原为 `[compact, kept…, after…]`，Kept tail 存活。
  - legacy checkpoint（无 `kept_count` 字段的裸 JSON 记录）→ 断言等价 `kept_count=0`，行为与今天一致（丢弃之前一切）。
  - 嵌套压缩：磁盘含两个 checkpoint，第二个的 Kept tail 跨过第一个 checkpoint → 断言 load 跳过旧 checkpoint、正确收集保留条数、只留最新 checkpoint 在首位。
  - 保留窗口起点是 assistant（其后紧跟它 own 的 tool result 全部在 Kept tail 内）→ 断言存活历史每个 `tool_call_id` 配对，且修复发生在裁剪后。
- **产生 seam — `slimcode_app::compaction::compact_messages`**（已有 `ScriptedProvider` 测试）：
  - 断言 checkpoint 的 `kept_count == 保留尾部长度`；"最新单条即超预算"时 `kept_count == 0`。
  - 现有 `compact_messages_replaces_the_old_span_and_keeps_the_tail` 等测试相应扩展一条 `kept_count` 断言。
- **端到端对称 seam — `crates/sdk/tests/auto_compaction.rs`**（已有 seam）：
  - 压缩后 `session.messages` 与"落盘 + 重新 load"出来的 `messages` 相等（live == reload 对称，正是本次要消除的不对称）。
- 先序参考：`crates/app/src/compaction.rs` 的 `ScriptedProvider` 单元测试、`crates/app/src/session.rs` 里 `load_restores_a_compaction_checkpoint` / `load_keeps_records_after_the_newest_checkpoint_only` 这类"写原始日志、断言恢复序列"的 load 测试。

## Out of Scope

- 消息稳定 id / pi 的 `firstKeptEntryId` 机制（被 `kept_count` 取代，明确不引入）。
- 回写/裁剪磁盘以物理删除 old 记录（违 ADR-0009 append-only，明确不做）。
- 日志格式版本 bump 与旧 `.json`/旧 checkpoint 的迁移（旧记录按 `kept_count=0` 就地兼容，不迁移）。
- compaction 可配置阈值、流式摘要、split-turn 摘要、one-shot 模式压缩（沿用 ADR-0020 D5，仍排除）。
- 分支式 session 的保留边界（slimcode 线性 session，无此需求）。
- FFI / GUI wire 契约文档为 `kept_count` 单独点名（additive 字段，默认忽略即可，除非后续要求）。

## Further Notes

- 落地顺序遵循改动流程：先写 ADR-0025「Compaction persists a keep boundary」并取代 ADR-0020 D2 的"已知代价"子句（在 ADR-0020 顶部加 `> Amended by ADR-0025` 指针，沿用 0009↔0012 惯例）；同步更新 `GLOSSARY.md` 的 `CompactSummary`、`Compaction`，并新增 `Kept tail` 术语。
- 本 spec 是 `.scratch/compact/spec.md`（已 resolved）的后续增量：只改"保留边界如何跨 resume 存活"这一点，不触碰估算、序列化格式、摘要 prompt 等既有决策。
- 关键事实（讨论中已跑通）：`kept_start` 返回 `0` 会先触发 "nothing to compact" 错误，因此新 checkpoint 恒为内存首位、上一个 checkpoint 必落入被总结区间；磁盘上旧 checkpoint 会物理夹在 Kept tail 与 new 之间，这正是"向前收集需跳过更旧 checkpoint"的根因。
