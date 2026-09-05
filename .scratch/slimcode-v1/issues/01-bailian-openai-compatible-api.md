# 01 — Bailian OpenAI-compatible API 面

Type: research
Status: resolved

## Question

阿里云百炼（Bailian / DashScope / Model Studio）的 OpenAI-compatible chat 接口，精确 HTTP 面是什么？从一手来源（阿里云官方帮助文档、DashScope 文档、Model Studio 文档）查证并逐条引用：

- chat completions 的 base URL
- 鉴权：header 名 + 凭证来源（`DASHSCOPE_API_KEY` 还是 OpenAI 风格 `Authorization: Bearer`）
- chat completions 请求体，含 `tools` / `tool_calls` 参数
- 响应体：`message`、`tool_calls`、`usage`（token 字段）
- SSE 流式事件格式（chunk 结构、`finish_reason`、末块 usage）
- Bailian 上可用的模型 id（qwen 系列在 `model` 字段的合法取值）
- 与 OpenAI 规范的偏差，Rust provider 需要特判的点

## Answer

Research 完成（subagent，一手来源全引用）。全文：`research/bailian-openai-compatible-api.md`。

关键结论：
- **端点**：`POST https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions`（默认可用，无需 WorkspaceId）；生产可用区域 MaaS 域 `https://{WorkspaceId}.<region>.maas.aliyuncs.com/compatible-mode/v1/...`。base URL 以 `/compatible-mode/v1` 结尾（非 `/v1`）。
- **鉴权**：`Authorization: Bearer $DASHSCOPE_API_KEY`；key 与区域绑定（区域不匹配返回 401 `invalid_api_key`）。
- **请求/响应/工具/流式大体贴合 OpenAI**：`tools`/`tool_choice`/`parallel_tool_calls`（默认 **false**）、`tool` 角色 + `tool_call_id`、usage = prompt/completion/total_tokens；SSE 每块 `data: {json}`（`object: chat.completion.chunk`），末块 `choices:[]` + usage（需 `stream_options={"include_usage":true}`），收尾 `data: [DONE]`。
- **流式工具调用**：函数名首片到达，`arguments` 按 `index` 键分片拼接，结束再 JSON 解析。
- **模型**：qwen3.8-max / qwen-plus / qwen-max / qwen-turbo / qwen-flash / qwen3-coder-* 等；默认推荐 **qwen-plus**（1M ctx、FC、thinking、结构化输出齐备）。
- **需特判的 15 处偏差**（完整清单见调研全文）：非标顶层 body 参数（`enable_thinking`、`tool_stream`、`top_k`、`repetition_penalty`…）、thinking 模型流 `reasoning_content`、`system_fingerprint` 不可靠、`stop` 数组不能混 token_ids 与字符串、GLM 需 `tool_stream:true`、`qvq-max`/Qwen-Audio 不支持 OpenAI 兼容等 → **serde 类型必须容错未知字段**。

决议：01 关闭；结论直接喂给 crates/ai（消息/工具/SSE chunk 模型）与配置默认值（base_url + DASHSCOPE_API_KEY + 默认模型 qwen-plus）。
