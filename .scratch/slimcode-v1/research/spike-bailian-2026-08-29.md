# Spike: Bailian compatible-mode 真实连通性验证

日期：2026-08-29
端点：`https://llm-aidtbmi5dynf7wum.cn-beijing.maas.aliyuncs.com/compatible-mode/v1`（用户提供，region MaaS 变体）
模型：`qwen3.7-plus-2026-05-26`（思考模型，reasoning_content 非空）
鉴权：`Authorization: Bearer $DASHSCOPE_API_KEY`（HTTP 200，认证通过）
原始记录：`.scratch/slimcode-v1/spike/01_nostream.json` … `05_parallel.json`（本地 scratch，不入库）

对应 ticket：[05-bailian-live-spike](../issues/05-bailian-live-spike.md)；为 01/02 的 Gaps 收尾。

---

## 1. 结论速览

| # | 验收点 | 结果 | 证据 |
|---|---|---|---|
| 1 | base URL + 鉴权 | ✅ 200，Bearer 生效 | 全部请求 |
| 2 | `stream_options.include_usage` 末块 usage | ✅ 仅末块（`choices: []`）返回 usage | 02/03 |
| 3 | 收尾 `data: [DONE]` | ✅ 最后一个 SSE 事件 | 02/03 |
| 4 | tools + stream tool_call 分片 | ✅ 首片 id/type/index/name，后续 arguments 分片，`index` 恒在 | 03 |
| 5 | 严格 serde 兼容性 | ⚠️ 见 §4 —— 有非标字段，须容忍 | 01–05 |
| 6 | 顺带：默认模型实测 | ✅ `qwen3.7-plus-2026-05-26` 正常返回；思考模型先 reasoning 后 content | 01–05 |
| 7 | 顺带：`parallel_tool_calls: true` | ✅ 生效，单回合 2 个并行 tool_call | 05 |
| 8 | 顺带：tool 结果回传多轮 | ✅ `assistant.tool_calls` + `role:tool` 回传后正常续答 | 04b |

---

## 2. 非流式补全（01）

请求：`{model, messages, max_tokens:20}`（无 stream，无 tools）

响应顶层键：`id, object, created, model, choices, usage` —— **无 `system_fingerprint`**。
`choice` 键：`index, message, finish_reason, logprobs`（`logprobs` 为 `null`，OpenAI 标准键）。
`message` 键：`role, reasoning_content, content`。

- `finish_reason: "stop"`，`content: "hello spike"`（忠实回显请求）。
- **`reasoning_content` 非空**（思考模型输出 950 字符思考过程）—— 非标字段，严格 serde 必须容忍。
- `usage`：`prompt_tokens, total_tokens, completion_tokens` + **非标** `prompt_tokens_details{cached_tokens, text_tokens}` 与 `completion_tokens_details{reasoning_tokens, text_tokens}`。
  - 注意：该模型 `completion_tokens = 226`，其中 `reasoning_tokens = 221`（思考 token 计入 completion_tokens）。

## 3. 流式补全（02 / 03）

### 3.1 普通流式（02，无 tools）

请求：`{..., stream:true, stream_options:{include_usage:true}}`

- 共 **255 个 SSE 事件**：254 个 `chat.completion.chunk` + **末位 `data: [DONE]`**。
- **usage 只在最后一个 payload（idx 253）返回**，且该块 **`choices: []`**（空数组）——与 ticket 01 论断一致。
  - 其余 253 个 chunk 的 `usage` 键均存在但为 **`null`** → 严格 serde 中 `usage` 必须是 `Option`。
- `finish_reason: "stop"` 在 idx 252（独立于 usage 块，其 delta 含 `content`+`reasoning_content`）。
- chunk 顶层键并集：`choices, created, id, model, object, usage`。
- delta 键并集：`content, reasoning_content, role`。
- **思考模型流式顺序**：首块 `role:assistant`（同时含空 content/reasoning_content）→ 连续 245 块 `reasoning_content`（思考阶段，无 content）→ idx 246 转折开始输出 `content`（6 块）→ finish → usage → `[DONE]`。
  - 给 UI/透传层的关键提示：思考内容与正文是**串行分两段**，不是交错。

### 3.2 工具流式（03，tools + stream）

请求：`{..., stream:true, tools:[get_weather], tool_choice:"auto"}`

- 共 47 事件：46 payload + `[DONE]` 末位。
- **tool_call 分片（6 块）**：
  - 首块（idx 38）：`delta.tool_calls: [{ "id":"call_bfcb...", "type":"function", "index":0, "function":{"name":"get_weather","arguments":""} }]` —— **id + type + index + name 都在首片**，arguments 为空串。
  - 续片（idx 39–42）：`id:""`、`name:null`，只带 `index:0` + `function.arguments` 碎片：`'{"city": '` `'"Beijing'` `'"'` `'}'` —— 需按 `index` 拼接、`arguments` 是 JSON 片段直到收尾才合法。
  - **`index` 键在每个 tool_call 分片恒在**（含首片 `0`）。
- `finish_reason: "tool_calls"` 在 idx 44（空 delta）。
- usage 末块 idx 45，`choices: []`。
- delta 键并集：`content, reasoning_content, role, tool_calls`。
- serde 要点：续片 `function.name` 为 `null`、`id` 为空串 → 客户端 `name: Option<String>`、`id` 允许空。

### 3.3 非流式工具调用（04）

- 完整形状：`message.tool_calls: [{id, type:"function", function:{name, arguments}, index:0}]`。
- **`content` 为 `''` 空串而非 `null`**（含 tool_call 的 assistant 消息）→ serde 里 `content: Option<String>`（可空串）。
- `finish_reason: "tool_calls"`。

### 3.4 工具结果回传多轮（04b）

回传 `assistant.tool_calls` + `role:"tool", tool_call_id, content` → 模型正常续答 `"The current temperature in Shanghai is 25°C."`，`finish_reason: "stop"`。**工具循环可用。**

### 3.5 并行工具（05）

- 显式 `parallel_tool_calls: true` **被接受**（200），单回合返回 **2 个** `get_weather` tool_call（Beijing + Shanghai），`finish_reason: "tool_calls"`。
- 修正 01 的表述：不是“该模型不支持并行”，而是默认不并行（OpenAI 语义 `parallel_tool_calls` 默认 false 即串行/单调用）；置 true 后可用。

## 4. serde 严格性清单（给 crates/ai 客户端）

顶层（非流式与流式一致）：`id, object, created, model, choices, usage`。
chunk（流式）：同上 6 键；**`usage` 键每块都出现（多数为 `null`）→ `usage: Option<Usage>`**。

必须建模或容忍的非标字段：
1. `message.reasoning_content: Option<String>`（思考模型，非流式 `message` 与流式 `delta` 均出现）。
2. `usage.prompt_tokens_details` / `usage.completion_tokens_details`（含 `cached_tokens`、`text_tokens`、`reasoning_tokens`）。
3. `delta.tool_calls[].function.name: Option<String>`（续片为 null）。
4. `message.content` 在 tool_call 消息为 `''`（可空串），普通消息为字符串，思考模型非空。
5. 并行时 `message.tool_calls` 数组长度 > 1；`index` 键在流式分片恒存在。

未发现 `system_fingerprint` 键（此前 01 的 Gaps 悬疑已解决：**本端点响应中不存在该键**）。

## 5. 对 crates/ai 的落地建议

- 客户端顶层严格字段 + `#[serde(default)]` 容忍未知字段（至少不 `deny_unknown_fields`）。
- 流式拼接：按 `choices[0].delta` 合并 —— `reasoning_content` 与 `content` 各自累计；`tool_calls` 按 `index` 分组，`id` 取首个非空、`name` 取首个非空、`arguments` 逐片拼接。
- usage 只在 `include_usage:true` 时于末块 `choices:[]` 出现；客户端以“收到 `[DONE]` 或 usage 非空块”判定流结束。
- 思考模型 token 计入 `completion_tokens`（含 `reasoning_tokens`），做用量展示时留意。

## 6. 待办/残留

- `qwen3.7-plus-2026-05-26` 是思考模型（reasoning_content）；ticket 01 建议的默认 `qwen-plus` 未在本次 spike 覆盖，非思考模型路径（无 reasoning_content）暂未实测。
- 错误码分类（model-not-found、参数错）未在 spike 中触发，留待客户端错误处理实现时补测。
- 本 spike 用用户提供的 region MaaS base_url；`https://dashscope.aliyuncs.com/compatible-mode/v1` 默认路径未实测（无该 key 的区域权限）。
