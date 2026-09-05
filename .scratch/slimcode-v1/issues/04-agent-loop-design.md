# 04 — Agent 运行时循环形态

Type: prototype
Status: resolved

## Question

`crates/agent` 运行时的精确形态是什么？

用一个 cheap 的 fake-provider 原型锁定的决策：

- 消息/回合模型（system/user/assistant/tool 消息、content parts、tool_calls）
- 工具循环：模型带 tool_calls 的响应 → 执行工具 → 追加 tool 结果 → 循环；并行 vs 串行工具调用
- 停止条件：无 tool_calls、最大迭代次数、显式停止信号
- 流式：assistant 文本与 tool_call 增量如何上抛给 CLI
- 会话状态：内存形态 + JSON 序列化边界

产出一个针对 fake provider 的粗糙 Rust 原型，得到对运行时形态的反馈。

## Answer

（2026-08-29，原型 + HITL 拍板）原型：[prototypes/agent-loop/](../prototypes/agent-loop/)（throwaway，独立 crate；`cargo run` 跑 7 场景）。proposal：[prototypes/agent-loop/agent-loop-proposal.md](../prototypes/agent-loop/agent-loop-proposal.md)

锁定的运行时形态：

```
Message { role, parts: Vec<Part>, tool_calls: Vec<ToolCall>, tool_call_id: Option<String> }
Part = Text { text: String }          // 形状 {"type":"text","text":"..."}（pi TextPart 同形）
ToolCall { id, name, arguments: String }  // arguments 存原始 JSON，执行时才 parse
Session { id, created_at, messages, title: Option<String> }
Delta = Reasoning | Text | ToolCallStart{index,id,name} | ToolCallArgs{index,fragment} | Done
AgentEvent = Turn | Stream(Delta) | ToolStart | ToolResult | Stop(StopReason)
run_agent(provider, tools, system, user, RunConfig{max_iterations, parallel_tools})
```

已拍板决策：
1. **工具执行默认串行**（本地工具引擎安全）；`parallel_tools` 开关留给未来 IO 工具。
2. **parts 抽象**（默认/唯一变体 Text/String），消息 `parts: Vec<Part>`，`Message::new` 直接收文本。
3. **工具报错**：`Error: <msg>` 前缀作 `role: tool` 内容进 history，模型自然恢复。
4. **停止条件**：无 tool_calls → `Completed`；`max_iterations` → `MaxIterations`；用户中断由 CLI 层 cancellation。
5. **会话 JSON 包一层 Session**（id/created_at/messages/title），为 `~/.slimcode/sessions/` 准备。

下一步：把消息模型 + 循环结构折入 crates/agent（同 ticket 03 模式）。
