# 05 — Bailian 连通性 spike

Type: task
Status: resolved

## Question

写 crates/ai 前，用真实 DASHSCOPE_API_KEY 对百炼 compatible-mode 端点做一次 live spike，确认 wire 行为（01/02 的残留疑问）：

- base URL 与鉴权如 01 所述（POST `/compatible-mode/v1/chat/completions`，`Authorization: Bearer $DASHSCOPE_API_KEY`）
- `stream_options={"include_usage":true}` 是否真的在末块（`choices:[]`）返回 usage
- 是否发出收尾 `data: [DONE]`
- `tools` + `stream:true` 的 tool_call 分片格式（首片 id/name、arguments 分片、`index` 键）
- 响应中是否存在让严格 serde 反序列化失败的额外/缺省字段
- （顺带）默认模型 qwen-plus 的实际返回

产出：把 curl 记录与结论落到 `research/spike-bailian-<date>.md`，并入 01/02 答案的 gap 清单。

HITL 前提：需要用户提供 DASHSCOPE_API_KEY（或 .env / ~/.slimcode/config.toml）。

## Answer

（2026-08-29，用户提供 region MaaS base_url + `qwen3.7-plus-2026-05-26` 思考模型）live spike 全部通过，报告：[research/spike-bailian-2026-08-29.md](../research/spike-bailian-2026-08-29.md)

- base URL + `Authorization: Bearer` 鉴权：✅ 200。
- `stream_options={"include_usage":true}`：✅ usage 只在**末块**（`choices:[]`）返回；其余块 usage 键为 `null`（须 `Option`）。
- 收尾 `data: [DONE]`：✅ 最后一个 SSE 事件。
- tools + stream tool_call 分片：✅ 首片带 id/type/index/name（arguments 空串），后续仅 index + arguments 碎片；`index` 恒在；续片 name=null、id=""。
- 严格 serde：⚠️ 有非标字段需容忍 —— `message.reasoning_content`、`usage.*_tokens_details`、tool_call 消息 `content:''`、续片 `name:null`；无 `system_fingerprint`。
- 默认模型顺带：✅ 思考模型先 reasoning 后 content 流式；`parallel_tool_calls:true` 生效（单回合 2 并行调用）；tool 结果回传多轮可用。

01/02 的 Gaps 已收尾（raw tool-call SSE 分片、system_fingerprint 存在性、tools+stream 行为、usage 末块）均有实测记录。
