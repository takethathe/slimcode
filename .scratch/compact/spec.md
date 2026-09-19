# 自动压缩（Auto-Compaction）

Status: resolved

## Problem Statement

slimcode 的 Session 在长对话中会积累大量历史消息。当上下文窗口接近上限时，后续 LLM 请求可能因 token 超限被拒绝、产生截断错误，或浪费大量 tokens 在无关旧信息上。用户需要自动压缩机制来保持会话可用：将旧的对话内容浓缩为结构化摘要，释放上下文空间，同时保留关键决策和进展信息。

## Solution

实现与 pi 项目一致的 compaction 功能：当 Session 的消息历史超过阈值（约 92% 的 context window）时，在 turn 结束后自动触发压缩——调用 LLM 生成结构化摘要，替换被压缩的消息段，并将摘要作为新消息存入 Session.history。摘要在下次 build Context 时重新注入到发送给模型的消息序列中。

核心设计原则：
- slimcode 是纯线性 session（无分支导航），不需要 pi 的 branch summarization
- compacted messages 永远不进入 provider wire（通过 `to_llm` → `None` 拦截）
- 压缩结果通过 `CompactSummary` variant 存在 Session.history 中，build context 时格式化注入

## User Stories

1. 作为 TUI 用户，当我在多轮对话后继续提问时，如果历史过长会自动压缩旧消息，以便我不会遇到上下文溢出错误。
2. 作为用户，压缩后的会话仍然能正确理解之前讨论过的关键决策和代码变更，以便工作不被中断。
3. 作为用户，我可以通过 `/compact` 命令手动触发压缩，以便在需要时立即释放上下文空间。
4. 作为 CLI one-shot 用户，我不受影响——one-shot 没有持久 session，从不触发自动压缩。
5. 作为用户，压缩会在 turn 完成之后、返回结果之前发生，这样我看不到额外的 "正在压缩" 交互中断。
6. 作为用户，如果压缩调用 LLM 失败（网络错误、模型不可用），当前 turn 的结果不会被丢弃，而是保留完整历史重试下一个 turn。
7. 作为开发者，compact summary 只存在于 Session.history 和 `.jsonl` 日志中，不会污染发给模型的 wire message 列表。
8. 作为用户，压缩后的 session 加载（`/session` picker）能正确回放历史记录，包含所有压缩摘要。

## Implementation Decisions

### 1. 新增 CompactSummary Message Variant

给 `AgentMessage` enum 新增 `CompactSummary` 变体。它代表一次压缩操作的结果：一段结构化摘要文本、被压缩前的 token 数、以及前一个 compact entry（如果有，用于增量更新）。

`AgentMessage` 现有形状是带 tagged serde 的单变体 enum：
```rust
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentMessage {
    Llm(Message),
}
```

新增后变为双变体：
```rust
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentMessage {
    Llm(Message),
    CompactSummary {
        summary: String,
        tokens_before: usize,
        previous_summary: Option<String>,
    },
}
```

- `to_llm()`: Llm 返回 `Some(message)`，CompactSummary 返回 `None`（从 wire 中过滤掉）
- `role()`: Llm 返回内部 role；CompactSummary 可返回一个特殊值或不暴露（内部使用即可）
- `text_content()`: Llm 返回拼接文本；CompactSummary 返回 summary 字段
- `tool_calls()` / `tool_call_id()`: CompactSummary 返回空 slice / `None`

这保持向后兼容：`.jsonl` 日志中的未知 record type 会被跳过（这是现有行为，见 session.rs 的 parse_record），但因为我们用的是 `tag = "kind"` serde 反序列化，unknown kind 会解构失败并报错。需要确认 load 路径是否做 graceful fallback。实际上 slimcode 的 load 直接用 `serde_json::from_value` 解析每个 message，unknown kind 会导致 deserialize error——这个 behavior 跟 session-manager.ts 里的 skip unknown type 不同。不过对于 compact 这个新类型，我们只需要确保 load 路径能正确处理两种 known kinds。

### 2. Compaction Summary 注入到 Context

在 `ContextBuilder::build()` 中扫描 history：遇到 `CompactSummary` 时，将其格式化为一条 user message，插入到历史消息序列的对应位置。

注入格式遵循 pi 的模板：
```
The conversation history before this point was compacted into the following summary:

<summary>
{summary_text}
</summary>
```

注入点规则：
- CompactSummary 出现在历史的某个位置，表示在该点之前的所有内容已被压缩
- 注入时应保持原始顺序：先 CompactSummary（含摘要），再其后未被压缩的消息

具体实现：在 `build()` 中，对 self.history 做一次遍历，将 CompactSummary 转为 text message，其余 Llm 消息原样保留。

### 3. Turn-end Hook 触发 Auto-Compaction

Slimcode 已有 `RunHooks::turn_end` hook（ADR-0015）。TUI 的 `run_turn()` 在 `runner::run_turn_with_hooks()` 之后拿到最终的 `(messages, stop)`。auto-compaction 逻辑放在这里，而不是在 hook 内部：

- `run_turn()` 检查 `stop == Completed`（仅在成功完成的 turn 后压缩，取消/失败的 turn 不触发）
- 计算当前 messages 的预估 token 数
- 超过阈值则执行压缩
- 压缩完成后更新 `state.session.messages`

### 4. Token Estimation

简化方案：用 char/4 heuristic（pi 的做法）估算每个 message 的 token 数，累加后判断是否超过阈值。

```rust
fn estimate_message_tokens(msg: &AgentMessage) -> usize {
    let chars = match msg {
        AgentMessage::Llm(m) => m.text_content().chars().count(),
        AgentMessage::CompactSummary { summary, .. } => summary.chars().count(),
    };
    (chars + 3) / 4  // ceiling division
}

fn should_compact(messages: &[AgentMessage], context_window: usize) -> bool {
    let total_chars: usize = messages.iter().map(|m| estimated_char_count(m)).sum();
    let total_tokens = (total_chars + 3) / 4;
    total_tokens > context_window * 92 / 100
}
```

Context window 硬编码常量：`DEFAULT_CONTEXT_WINDOW = 128_000`（覆盖大多数主流模型）。

### 5. Compaction Execution

压缩函数独立于 runner，接收当前的 messages 和历史中的 CompactSummary，返回压缩后的 messages 列表：

```rust
/// Compact old messages into a structured summary.
/// Returns (new_messages, summary_generated).
/// `previous_summary` comes from the last CompactSummary in history, if any.
async fn compact_messages(
    messages: Vec<AgentMessage>,
    model_name: &str,
    api_key: &str,
    base_url: &str,
    previous_summary: Option<String>,
) -> Result<Vec<AgentMessage>, String>;
```

执行流程：
1. 将待压缩的消息（排除末尾最近的 ~8% 预留空间和现有的 CompactSummary）按顺序收集
2. 调用 LLM 生成结构化摘要（复用同一个 provider instance）
3. 移除被压缩的旧消息
4. 追加一条新的 `AgentMessage::CompactSummary`（含 summary + tokens_before）
5. 保留最近的消息不变

LLM 调用细节：
- 系统提示：固定字符串 "You are a context summarization assistant..."（pi 的 SUMMARIZATION_SYSTEM_PROMPT 精简版）
- User prompt：将被压缩的对话 serialize 为文本格式 + 结构化输出指令
- 如果 `previous_summary` 存在，合并更新而非从头生成（减少 LLM token 消耗）
- max_tokens 设为保守值（如 2048），避免摘要过长

**Prompt 结构**（完全跟随 pi 的模板）：
```
The messages above are a conversation to summarize. Create a structured context checkpoint summary that another LLM will use to continue the work.

Use this EXACT format:

## Goal
[What is the user trying to accomplish?]

## Constraints & Preferences
- [Any constraints or preferences mentioned]

## Progress
### Done
- [x] [Completed tasks]

### In Progress
- [ ] [Current work]

## Key Decisions
- **[Decision]**: [Brief rationale]

## Next Steps
1. [Ordered list of what should happen next]

## Critical Context
- [Data, examples, or references needed to continue]
```

Serialize 格式（每条消息一行）：
```
[User]: user message text
[Assistant]: assistant response
[Assistant tool calls]: read(path="src/main.rs")
[Tool result]: file contents...
```

### 6. Session Log 处理

`CompactSummary` 作为 `AgentMessage` 的一个 variant，序列化后会写入 `.jsonl` 日志（通过 `LogRecord::Message` → 现有 serde 路径）。`parse_record()` 使用 tagged serde 反序列化，`kind: "compact_summary"` 能正确解包。

Load 时：`parse_agent_message()` → `serde_json::from_value::<AgentMessage>()` → 自动识别 `"kind": "compact_summary"` → 恢复为 CompactSummary variant。

**实现修正**：日志是追加式的，compaction 写入的 checkpoint 落在被保留的 tail 之后，因此
naive replay 会把被替代的旧消息一起恢复出来。为使恢复的 Session 与模型看到的视图一致，
`SessionStore::load` 额外丢弃**最新一条 `compact_summary` 之前的所有 message 记录**
（日志字节不改写，ADR-0020 D2）。

### 7. `/compact` CLI Command

在 TUI 的 `command()` 方法中新增 `/compact` 子命令。行为：
- 显示一条 RenderItem::Notice "Compressing..."
- 调用与 auto-compaction 相同的 compact 逻辑
- 成功则更新 session.messages，显示 "Compacted N tokens → summary"
- 失败则显示错误但不丢失当前 turn 结果

### 8. API Key / Model 传递

compaction 需要一个 LLM call。TuiSession 已有 `config: ProviderConfig` 字段携带 model/base_url/api_key。runner.rs 的 `run_turn()` 也接收这些参数。compaction 可以直接复用同一套 config。

## Testing Decisions

### Module 1: `core::session` — CompactSummary variant
- **测试什么**：CompactSummary 能正确序列化/反序列化；`to_llm()` 返回 None；`text_content()` 返回 summary
- **参照物**：`crates/core/src/session.rs` 现有的 `session_round_trips_losslessly` test
- **验证方式**：serde round-trip + method dispatch assertions

### Module 2: `app::context` — CompactSummary injection
- **测试什么**：`ContextBuilder::build()` 将 CompactSummary 转为 user text message 注入
- **参照物**：`crates/app/src/context.rs` 现有 tests（empty_history_seeds_only_the_user_message 等）
- **验证方式**：构建 context 后检查 `context.messages` 是否包含注入的摘要 text

### Module 3: `app::runner` — Token estimation + threshold check
- **测试什么**：estimate_tokens 对不同角色消息的估算精度；should_compact 的阈值判定
- **参照物**：`crates/app/src/runner.rs` 现有 test suite（scripted FakeProvider pattern）
- **验证方式**：给定一组已知长度的 message，验证 total_tokens > threshold → true/false

### Module 4: `app::runner` — Compaction execution
- **测试什么**：compact 函数接收历史消息 + previous_summary，返回新的 messages 列表（含 CompactSummary）
- **参照物**：需要模拟 LLM call——用一个 FakeProvider 返回固定的摘要文本
- **验证方式**：
  - 输入 10 条消息 → 输出 1 条 CompactSummary + 末尾保留的消息
  - 有 previous_summary → 输出摘要包含新旧信息
  - 无 previous_summary → 输出全新的结构化摘要

### Module 5: `cli::tui` — Turn-end auto-compaction trigger
- **测试什么**：turn 完成后若超阈值则自动压缩；turn 取消/失败不触发
- **参照物**：`crates/cli/src/tui.rs` 现有 `submit_streams_a_turn_and_persists_every_message` 等 handler tests
- **验证方式**：FakeProvider 返回足够长的消息使 history 超标 → submit 后检查 session.messages 含 CompactSummary

### Module 6: `cli::tui` — `/compact` command
- **测试什么**：手动触发压缩，正确处理成功/失败路径
- **参照物**：同 tui.rs handler test pattern

### Module 7: Load round-trip
- **测试什么**：含 CompactSummary 的 session 能被完整 load
- **参照物**：`crates/cli/src/tui.rs` 的 `load_session_replays_the_history_into_the_transcript` test
- **验证方式**：创建含 CompactSummary 的 session → 保存 → load → 验证 CompactSummary 正确恢复

## Out of Scope

- **Branch summarization**：slimcode 是纯线性 session（无 fork/branch），不需要 pi 的 branch-summary 逻辑
- **File operation tracking**：compaction 不提取/追踪文件操作（read/write/edit），这与 pi 的功能有差距但对 v1 够用
- **Streaming compaction summary generation**：compaction LLM call 是同步阻塞的（通过 BailianProvider），不流式输出摘要
- **Configurable thresholds**：阈值硬编码为 context_window × 92%，不做配置项
- **Partial compaction**：不保留"最近 N 条消息 + 旧消息压缩"的策略——所有超阈值的旧消息一起压缩为一整段摘要
- **Compaction in one-shot mode**：CLI 非交互式模式不触发任何 compaction（无持久 session）

## Further Notes

- **术语对齐 CONTEXT.md**：新增词条 `CompactSummary`（一种 session-only 消息变体，压缩结果的容器，不对模型可见）、`compaction`（将旧对话内容浓缩为结构化摘要的操作）。
- **为什么不用 existing hooks 做 compaction**：`before_tool` hook 太早（工具还没跑完），`after_tool` hook 每工具都触发。`turn_end` 是语义正确的边界——整个 turn 结束，所有消息已稳定在 history 中。
- **与 agent-runner-hooks 的关系**：ticket 02 (`02-run-hooks.md`) 已经建立了 RunHooks 体系。compaction 走 turn_end，不破坏现有 hook 边界。
- **与 session-jsonl 的关系**：ticket 01 (`01-session-log-format.md`) 定义了 typed record 模式。CompactSummary 作为 `AgentMessage` 的新 variant，通过现有的 tag-based serde 序列化和解析，无缝融入 `.jsonl` 格式。
- **为什么 CompactSummary 不是 `custom` 类型**：pi 用 declaration merging 扩展 CustomAgentMessages。slimcode 用 Rust enum variant，更类型安全且无需 module augmentation。
