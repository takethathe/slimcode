# Agent 运行时循环形态 — 原型 proposal

对应：[ticket 04](../issues/04-agent-loop-design.md) · 原型代码：`src/main.rs`（throwaway，独立 crate，不属 workspace）

## 原型回答的问题

`crates/agent` 运行时的精确形态。用一个 scripted fake provider（无真实 HTTP，delta 形状镜像 ticket 05 实测 wire 格式）把循环推过 7 个场景，得到以下观察。

## 场景实测结果

| # | 场景 | 结果 |
|---|---|---|
| 1 | 单回合直答（无工具） | `Completed`，1 轮 |
| 2 | 单次工具调用 → 续答 | `Completed`，2 轮；fragmented arguments 正确拼接 `{"city": "Beijing"}` |
| 3 | 一回合 2 个 tool_call，**并行**执行 | 两调用都执行、结果一并 append，2 轮完成 |
| 4 | 工具报错 → 模型恢复 | `ERR` 以 `Error: …` 前缀进 history，模型给出兜底答复 |
| 5 | 死循环（模型不停调工具） | **`MaxIterations` 硬停**，3 轮止住 |
| 6 | 流式透传 | 事件序列 `Turn → Stream(Reasoning/Text/ToolCallStart/ToolCallArgs/Done)` |
| 7 | 会话 JSON 边界 | 5 条消息序列化 → 反序列化 **无损 round-trip**；`tool_calls`/`tool_call_id` 缺省时省略 |

## 原型锁定的结构（建议折入 crates/agent）

```
Message { role: System|User|Assistant|Tool,
          parts: Vec<Part>,          // v1: 只有 Text{text}，形状 {"type":"text","text":"..."}
          tool_calls: Vec<ToolCall>, tool_call_id: Option<String> }
Part = Text { text: String }        // parts 抽象，默认 Text/String（已折入原型）
ToolCall { id, name, arguments: String /* raw JSON, 执行时才 parse */ }

Session { id, created_at, messages: Vec<Message>, title: Option<String> }  // 已折入原型

Delta = Reasoning(String) | Text(String)
      | ToolCallStart { index, id, name }
      | ToolCallArgs { index, fragment } | Done(FinishReason)

AgentEvent = Turn{..} | Stream(Delta) | ToolStart{..} | ToolResult{..} | Stop(StopReason)

run_agent(provider, tools, system, user, RunConfig{max_iterations, parallel_tools})
  -> RunResult { messages, iterations, stop, events }
```

## 已拍板的决策（HITL 2026-08-29）

1. **并行 vs 串行**：v1 默认**串行**（本地工具天然串行安全）；`parallel_tools` 开关留给未来 IO 密集工具。
2. **content parts**：采用 **parts 抽象**（`Part::Text` 为默认/唯一变体），消息用 `parts: Vec<Part>`；纯 String 是默认构造（`Message::new` 直接给文本）。
3. **工具报错格式**：`Error: <msg>` 前缀作为 `role: tool` 内容进 history，让模型自然恢复。
4. **停止条件**：无 tool_calls → `Completed`；达到 `max_iterations` → `MaxIterations`（运行时唯一硬保险）；用户 Ctrl-C 中断由 CLI 层做 cancellation（drop run）。
5. **会话 JSON**：包一层 `Session { id, created_at, messages, title }` 元数据，为 `~/.slimcode/sessions/` 准备；字段细节待 cli 设计。

## 结论

决策已拍板（见上「已拍板的决策」），写入 ticket 04 的 Answer。下一步：把消息模型/循环结构折入 crates/agent（与 ticket 03 原型折入 edit 引擎同模式）。
