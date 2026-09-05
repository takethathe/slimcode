# slimcode v1 — Wayfinding Map

## Destination

一个能实际干活的 Rust 编码 agent CLI：一条 prompt 在目标目录内自主执行 read / write / edit / bash / grep / find / ls 工具循环直到完成，流式输出、token 用量统计、session 可保存/恢复。v1 验收 = `cargo test` 全绿 + clippy 零警告。

## Notes

- 领域：Rust 重写 pi 的 coding agent；参考仓库 `~/Downloads/Work/pi`（packages/ai、agent、coding-agent）。
- 布局：cargo workspace，三个 crate —— `crates/ai`（统一 LLM + Provider trait + 流式）、`crates/agent`（运行时 + 工具 + 会话）、`crates/cli`（二进制 slimcode）。
- 第一家 provider：阿里云百炼（Bailian），走 OpenAI-compatible 协议，置于 Provider trait 之后。
- 交互：非交互 `slimcode "<prompt>"` + 简单行式 REPL。
- 会话：JSON 文件，`~/.slimcode/sessions/*.json`。
- 配置：`~/.slimcode/config.toml` + env 覆盖。
- 会话必读 skills：tdd（AGENTS.md 强制 red-green-refactor）、domain-modeling、grilling、prototype。
- 工程纪律：cargo fmt + clippy 零警告方可提交；docs/ 同步；只本地提交。
- 验收清单（v1「到了」）：① 一条 prompt 启动并在指定 cwd 自主循环执行七工具 ✅；② LLM 流式输出 ✅；③ token 用量统计 ✅；④ session 保存/恢复 ✅（`/save` + REPL `/load`）；⑤ cargo test 全绿 + clippy 零警告 ✅（cli 33 / agent 48 / ai 17 tests，含 2 ignored live）。

## Decisions so far

- [01 — Bailian OpenAI-compatible API 面](issues/01-bailian-openai-compatible-api.md)：百炼走 OpenAI-compatible —— `/compatible-mode/v1` 端点、`Authorization: Bearer $DASHSCOPE_API_KEY`（region-bound）、请求/响应/工具调用/SSE 贴合 OpenAI；15 处需特判（非标 body 参数、parallel_tool_calls 默认 false、usage 仅末块）；默认模型推荐 qwen-plus。调研：[research/bailian-openai-compatible-api.md](research/bailian-openai-compatible-api.md)
- [02 — Rust OpenAI-compatible 栈选型](issues/02-rust-openai-compatible-stack.md)：crates/ai 用 slim 手写栈 —— reqwest 0.13（rustls+json+stream）+ eventsource-stream 0.2.3 + serde/serde_json；async-openai 留作 fallback；tool_call delta 按 index 拼接后 JSON 解析。调研：[research/rust-openai-compatible-stack.md](research/rust-openai-compatible-stack.md)
- [03 — Edit 工具语义](issues/03-edit-tool-semantics.md)：v1 锁定 pi 式语义（多 edit 匹配原文、唯一性强制、重叠/空/no-op 报错、CRLF/BOM 保留、返回 diff+首行）+ **轻量 fuzzy（选项 C）**：仅每行 trim_end 归一，命中后按行回映射、未触碰行保留原始字节，不做 NFKC/引号折叠。原型：[prototypes/edit-semantics/](prototypes/edit-semantics/)，已折入 crates/agent 的 edit 引擎
- [05 — Bailian 连通性 spike](issues/05-bailian-live-spike.md)：live 实测通过（region MaaS base_url + `qwen3.7-plus-2026-05-26` 思考模型）—— usage 仅末块（`choices:[]`）且每块 usage 键为 null（须 Option）、`[DONE]` 收尾、tool_call 分片首片带 id/type/index/name、严格 serde 须容忍 `reasoning_content`/`usage.*_tokens_details`/tool 消息 `content:''`/续片 `name:null`、无 `system_fingerprint`、`parallel_tool_calls:true` 生效、工具结果回传多轮可用。spike：[research/spike-bailian-2026-08-29.md](research/spike-bailian-2026-08-29.md)
- [04 — Agent 运行时循环形态](issues/04-agent-loop-design.md)：锁 runtime 形态 —— `Message{role, parts: Vec<Part>, tool_calls, tool_call_id}`（parts 抽象，默认 `Part::Text`）、`ToolCall{id,name,arguments 原串}`、`Session{id,created_at,messages,title}`；循环 `run_agent(provider,tools,system,user,RunConfig{max_iterations,parallel_tools})`，事件 `Turn/Stream/ToolStart/ToolResult/Stop`；决策=工具执行默认串行、`Error:` 前缀作 tool 结果、停止=无 tool_calls/max_iterations/CLI 层中断、会话包一层 Session。原型：[prototypes/agent-loop/](prototypes/agent-loop/)，已折入 crates/agent
- [crates/ai — Bailian provider 实现](issues/)：实现 `agent::Provider` seam —— reqwest 0.13 blocking（`json`+`blocking`+`rustls`，无 tokio，阻塞边界收在 provider 内），`stream:true` + `include_usage`，serde 全量容忍清单（不设 `deny_unknown_fields`；`reasoning_content`/`usage` Option/`content:''`/`name:null`），tool_call 分片按 index 拼接；配置 `DASHSCOPE_API_KEY`（必填）+ `SLIMCODE_AI_BASE_URL`/`SLIMCODE_AI_MODEL` 覆盖；文本+工具两条 live 冒烟通过。当前 3 crate 状态：agent 48 tests、ai 17 tests（含 2 ignored live）全绿，clippy 0 警告
- [crates/cli — 二进制入口](issues/)：非交互 `slimcode "<prompt>"`（`--cwd`）+ 行式 REPL。模块 `config`（env > config.toml > 默认值，key 只来自 env）、`render`（事件→终端，流式文本/结构行/用量）、`session`（`SessionStore`，RFC3339 UTC 无 chrono）、`repl`（`/load` 恢复、新会话自动前置系统提示、每轮自动保存）、`tools`（七工具绑 cwd）。agent 模块 `pub use session::{Message,Role,ToolCall}` 供 CLI 引用。

## Not yet specified

- 已细化并落地（cli 设计）✅：Session 元数据（id `slimcode-<unix>-<pid>-<n>`、created_at RFC3339 UTC、title 取首条用户消息截断 48 字符）、config.toml schema 与 env 覆盖规则、token 用量 CLI 展示、系统提示词、行式 REPL 的 `/` 命令集与恢复。
- [06 — REPL 输入历史与多行 prompt](issues/06-repl-input-history-and-multiline.md)：落地 `input history`（`HistoryStore`，`<home>/history.json` 500 条）与 `multi-line prompt`（行尾 `\` 续行）。无依赖交互 `/history`（20 条、最新在前、1=最新）`/!!` `/!N`（verbatim 重跑、不重复入史、错误仅打印）；`classify` 保持纯逐行，续行态收在 `run()` 的 `pending` + 纯函数 `accumulate`；共享依赖收 `ReplCtx`。Shift+Enter 不可行见 ADR-0001。
- 额外 provider（Anthropic 原生、Google 等，超出第一家 Bailian 的部分）。

## Out of scope

- TUI（差量渲染终端 UI）——Q2 划出
- 远程会话（protocol / client / server，CBOR）——Q2 划出
- telemetry、evals——Q2 划出
- 容器化 / 沙箱（Docker / Gondolin / OpenShell）——不在 v1 目的地内
