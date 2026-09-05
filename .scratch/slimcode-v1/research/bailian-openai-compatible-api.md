# Research: Alibaba Cloud Bailian (百炼 / DashScope / Model Studio) OpenAI-Compatible Chat Completions API

> Scope: exact HTTP surface of the OpenAI-compatible chat completions endpoint (`/compatible-mode/v1/chat/completions`).
> Every claim cites the official Alibaba Cloud help pages listed under [Key reference URLs](#key-reference-urls).

## Summary

Bailian exposes an OpenAI-compatible chat completions API at `https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions` (legacy China endpoint) and at per-region "business workspace" MaaS domains such as `https://{WorkspaceId}.cn-beijing.maas.aliyuncs.com/compatible-mode/v1/chat/completions`, which Alibaba now recommends migrating to. Auth is `Authorization: Bearer <key>` where the key comes from the `DASHSCOPE_API_KEY` environment variable (created in the Bailian console and **region-bound**). The request/response JSON, tool-calling (`tools` / `tool_calls` / `tool` role), usage fields (`prompt_tokens`/`completion_tokens`/`total_tokens`) and SSE shape (`data: {...}` chunks, `object:"chat.completion.chunk"`, final usage chunk with empty `choices`, `data: [DONE]`) closely follow OpenAI, but a Rust provider must special-case several non-standard extras and defaults (see §7).

## Findings

### 1. Base URL / endpoint

- OpenAI-SDK style `BASE_URL` (must end in `/compatible-mode/v1`, **without** `/chat/completions`):
  - 华北2（北京）: `https://{WorkspaceId}.cn-beijing.maas.aliyuncs.com/compatible-mode/v1`
  - 美国（弗吉尼亚）: `https://dashscope-us.aliyuncs.com/compatible-mode/v1`
  - 新加坡: `https://{WorkspaceId}.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1`
  - 日本（东京）: `https://{WorkspaceId}.ap-northeast-1.maas.aliyuncs.com/compatible-mode/v1`
- Full HTTP endpoint (POST): append `/chat/completions`, e.g. `POST https://{WorkspaceId}.cn-beijing.maas.aliyuncs.com/compatible-mode/v1/chat/completions`. [Source](https://help.aliyun.com/zh/model-studio/compatibility-of-openai-with-dashscope)
- Alibaba explicitly recommends migrating from the legacy domains to the workspace-specific MaaS domains for 北京 and 新加坡 (`https://{WorkspaceId}.cn-beijing.maas.aliyuncs.com` and `https://{WorkspaceId}.ap-southeast-1.maas.aliyuncs.com`); the old domains `https://dashscope.aliyuncs.com` (China) and `https://dashscope-intl.aliyuncs.com` (Singapore) "仍可正常使用" (still work normally). `{WorkspaceId}` is shown in the Bailian console → 业务空间详情. [Source](https://help.aliyun.com/zh/model-studio/compatibility-of-openai-with-dashscope)
- **Recommendation for a default**: `https://dashscope.aliyuncs.com/compatible-mode/v1` (China-station legacy) works **without a `WorkspaceId`** and is the safe conventional default for a generic OpenAI-compatible provider; for production Beijing/Singapore callers, Alibaba recommends the `{WorkspaceId}` MaaS domain. The endpoint must match the region of the API key (§2 / §7.2). [Source](https://help.aliyun.com/zh/model-studio/compatibility-of-openai-with-dashscope)
- The endpoint is OpenAI-compatible only for the supported model families; `Qwen-Audio` does **not** support the OpenAI-compatible protocol (DashScope protocol only). [Source](https://help.aliyun.com/zh/model-studio/compatibility-of-openai-with-dashscope), [Source](https://help.aliyun.com/zh/model-studio/qwen-api-via-openai-chat-completions)

### 2. Auth

- Header: `Authorization: Bearer $DASHSCOPE_API_KEY` (plus `Content-Type: application/json`), shown in all curl examples, e.g. `curl -X POST ... -H "Authorization: Bearer $DASHSCOPE_API_KEY" -H "Content-Type: application/json"`. [Source](https://help.aliyun.com/zh/model-studio/compatibility-of-openai-with-dashscope)
- Credential source: the **Bailian (百炼) API Key**, conventionally read from the `DASHSCOPE_API_KEY` environment variable (docs show `export DASHSCOPE_API_KEY='<YOUR_API_KEY>'` on Linux/macOS, `setx DASHSCOPE_API_KEY "<YOUR_API_KEY>"` on Windows). [Source](https://help.aliyun.com/zh/model-studio/get-api-key)
- The key is created in the Bailian console API-Key management page (https://bailian.console.aliyun.com/?tab=model#/api-key). [Source](https://help.aliyun.com/zh/model-studio/compatibility-of-openai-with-dashscope)
- **Keys are region-bound**: calling a region's base_url requires a key created in that same region; a Beijing key used against the Virginia endpoint returns HTTP 401, message `Incorrect API key provided`, code `invalid_api_key` (this means region mismatch, not an invalid/expired key). [Source](https://help.aliyun.com/zh/model-studio/compatibility-of-openai-with-dashscope)

### 3. Request body (chat completions)

Core fields (parameter reference): [Source](https://help.aliyun.com/zh/model-studio/qwen-api-via-openai-chat-completions), [Source](https://help.aliyun.com/zh/model-studio/compatibility-of-openai-with-dashscope)

- `model` (string, required) — see §6.
- `messages` (array, required) — `{"role":..., "content":...}` in order. Roles: `system` (optional, generally first), `user` (required; `content` may be a string, or an array for multimodal: `type` ∈ `text`/`image_url`/`input_audio`/`video`/`video_url`), `assistant` (optional; `content` may be empty when `tool_calls` present; supports a `tool_calls` array for round-tripping), `tool` (tool result: `content` **must be a string** + `tool_call_id`).
  - Older constraint from the compatibility page: only `messages[0]` may be `system`, user/assistant should alternate, last element should be `user`. [Source](https://help.aliyun.com/zh/model-studio/compatibility-of-openai-with-dashscope)
- `stream` (bool, default false) — enable SSE. `stream_options` (`{"include_usage":true}`) puts usage in the **final** chunk. [Source](https://help.aliyun.com/zh/model-studio/qwen-api-via-openai-chat-completions), [Source](https://help.aliyun.com/zh/model-studio/stream)
- Sampling/control: `temperature` [0,2), `top_p` (0,1], `top_k` (non-standard), `presence_penalty` [-2,2], `repetition_penalty` (non-standard), `seed` ([0, 2^31−1]), `stop` (string or array; **cannot mix token_ids and strings** in one array), `n` (1–4; older doc: only `qwen-plus`; newer doc: only Qwen3 non-thinking + `qwen-plus-character`; forced to 1 when `tools` is passed). [Source](https://help.aliyun.com/zh/model-studio/qwen-api-via-openai-chat-completions), [Source](https://help.aliyun.com/zh/model-studio/compatibility-of-openai-with-dashscope)
- Length limits: `max_tokens` (soon deprecated; for most models = answer tokens only; for some third-party models includes chain-of-thought) and `max_completion_tokens` (total = reasoning + answer; recommended for thinking models; ≤10-token variance). [Source](https://help.aliyun.com/zh/model-studio/qwen-api-via-openai-chat-completions)
- Structured output: `response_format` = `{"type":"text"}` (default) or `{"type":"json_object"}` (must instruct the model to output JSON in the prompt, else error). [Source](https://help.aliyun.com/zh/model-studio/qwen-api-via-openai-chat-completions)
- Function calling — `tools` (array) of `{"type":"function","function":{"name","description","parameters"}}` where `parameters` is a JSON Schema object (type/properties/required; arrays need `items`). `tool_choice` = `"auto"` (default) | `"none"` | `"required"` | `{"type":"function","function":{"name":...}}`. `parallel_tool_calls` (bool, **default `false`**). [Source](https://help.aliyun.com/zh/model-studio/qwen-api-via-openai-chat-completions), [Source](https://help.aliyun.com/zh/model-studio/qwen-function-calling)
- Non-OpenAI extras (put at the **top level of the HTTP JSON body**; OpenAI SDKs pass them via `extra_body`): `enable_thinking`, `thinking_budget`, `reasoning_effort`, `enable_search`, `search_options` (`forced_search`, `search_strategy`), `tool_stream` (default false; streams complex tool args), `top_k`, `repetition_penalty`, `enable_code_interpreter`, `modalities`/`audio` (Qwen-Omni), `skill` (qwen-doc-turbo), `preserve_thinking`, `clear_thinking` (GLM), `thinking` (MiniMax/MiniMax-M3). [Source](https://help.aliyun.com/zh/model-studio/qwen-api-via-openai-chat-completions)
- Header extension `X-DashScope-DataInspection` (value `{"input":"cip","output":"cip"}`) for stricter content-safety inspection. [Source](https://help.aliyun.com/zh/model-studio/qwen-api-via-openai-chat-completions)

### 4. Response body (non-streaming)

Non-streaming response is a single OpenAI-shaped JSON object, e.g.: [Source](https://help.aliyun.com/zh/model-studio/compatibility-of-openai-with-dashscope), [Source](https://help.aliyun.com/zh/model-studio/qwen-api-via-openai-chat-completions)

```json
{
  "id": "chatcmpl-xxx",
  "object": "chat.completion",
  "created": 1716430652,
  "model": "qwen3.8-max",
  "choices": [
    {
      "index": 0,
      "finish_reason": "stop",
      "logprobs": null,
      "message": {
        "role": "assistant",
        "content": "我是来自阿里云的超大规模预训练模型，我叫千问。",
        "function_call": null,
        "tool_calls": null
      }
    }
  ],
  "system_fingerprint": null,
  "usage": { "prompt_tokens": 22, "completion_tokens": 18, "total_tokens": 40 }
}
```

- `choices[i].message` — `role` (fixed `assistant`), `content` (string), `tool_calls` (nullable array). `choices[i].finish_reason` ∈ `null` (generating) / `stop` (stop condition) / `length` (max length hit). `usage` = `prompt_tokens` / `completion_tokens` / `total_tokens`. `system_fingerprint`: docs say "当前暂时不支持，返回为空字符串" (not supported, returns empty string) — though examples show `null`. [Source](https://help.aliyun.com/zh/model-studio/compatibility-of-openai-with-dashscope)
- When the model decides to call a tool, `message.tool_calls` is populated (OpenAI format): [Source](https://help.aliyun.com/zh/model-studio/qwen-function-calling)

```json
{
  "content": "",
  "refusal": null,
  "role": "assistant",
  "audio": null,
  "function_call": null,
  "tool_calls": [
    {
      "id": "call_6596dafa2a6a46f7a217da",
      "function": { "arguments": "{\"location\": \"上海\"}", "name": "get_current_weather" },
      "type": "function",
      "index": 0
    }
  ]
}
```

- Tool result fed back as `{"role":"tool","tool_call_id":"<id>","content":"<string>"}` (content must be a string; keep the assistant tool-call message in history). With `parallel_tool_calls:true` the `tool_calls` array carries all requested calls (multiple ids). [Source](https://help.aliyun.com/zh/model-studio/qwen-function-calling)
- Error response body (HTTP error) — OpenAI-like envelope: [Source](https://help.aliyun.com/zh/model-studio/compatibility-of-openai-with-dashscope)

```json
{ "error": { "message": "Invalid API-key provided.", "type": "invalid_request_error", "param": null, "code": "invalid_api_key" } }
```

### 5. SSE streaming format

- Streaming is SSE: each chunk is a `data: {json}` line; the stream ends with `data: [DONE]`. Chunks carry `"object":"chat.completion.chunk"` and `id`/`model`/`created`. Delta carries `content` (+ `role` on the first chunk) and `finish_reason` (`null` while generating, `"stop"` on the final content chunk). [Source](https://help.aliyun.com/zh/model-studio/stream), [Source](https://help.aliyun.com/zh/model-studio/compatibility-of-openai-with-dashscope)

```
data: {"choices":[{"delta":{"content":"","role":"assistant"},"index":0,"logprobs":null,"finish_reason":null}],"object":"chat.completion.chunk","usage":null,"created":1726132850,"system_fingerprint":null,"model":"qwen-plus","id":"chatcmpl-428b414f-..."}
data: {"choices":[{"finish_reason":null,"delta":{"content":"我是"},"index":0,"logprobs":null}],...}
...
data: {"choices":[{"finish_reason":"stop","delta":{"content":""},"index":0,"logprobs":null}],...}
data: {"choices":[],"object":"chat.completion.chunk","usage":{"prompt_tokens":22,"completion_tokens":17,"total_tokens":39},...}
data: [DONE]
```

- **Usage arrives in the final chunk**: with `stream_options={"include_usage":true}`, the last chunk has **empty `choices`:[]** and a populated `usage`. Without `include_usage`, no usage is returned at all (chunks show `"usage":null`). [Source](https://help.aliyun.com/zh/model-studio/stream), [Source](https://help.aliyun.com/zh/model-studio/qwen-api-via-openai-chat-completions)
- Thinking models stream a `reasoning_content` delta **before** `content`; phase detection: `reasoning_content != null` → thinking, `content != null` → answering, both null → same phase as previous chunk. [Source](https://help.aliyun.com/zh/model-studio/stream)
- Streaming tool calls: with `stream=true` + `tools`, the function **name** arrives in the first tool delta and `arguments` are streamed as fragmented JSON strings across chunks; the provider must concatenate `delta.tool_calls[i].function.arguments` keyed by `index`. (For complex array/object-typed parameters, non-fragmented emission is the default; `tool_stream=true` enables fragmented streaming.) [Source](https://help.aliyun.com/zh/model-studio/qwen-function-calling)
- Version note: one older page's input-param table states `tools` 暂时无法与 `stream=True` 同时使用 ("tools cannot currently be used with stream=True"), but the current function-calling and chat-completions pages explicitly document and demonstrate tools + stream together (with `tool_stream`). Treat as a version difference; current behavior supports streaming tool calls. [Source](https://help.aliyun.com/zh/model-studio/compatibility-of-openai-with-dashscope), [Source](https://help.aliyun.com/zh/model-studio/qwen-function-calling)
- Some models are **stream-only** (non-streaming may time out): Qwen3 open-source, QwQ (commercial + open), QVQ, Qwen-Omni. Non-streaming max timeout ≥ 300 s; on timeout the service interrupts and returns already-generated content rather than an error. [Source](https://help.aliyun.com/zh/model-studio/stream), [Source](https://help.aliyun.com/zh/model-studio/qwen-api-via-openai-chat-completions)

### 6. Model ids (valid in the `model` field)

Supported families on the OpenAI-compatible endpoint: Qwen LLMs (commercial + open-source), Qwen-VL, Qwen-Coder, Qwen-Omni, Qwen-Math, plus third-party DeepSeek (阿里云直供 / 硅基流动直供 / 快手万擎直供), Kimi, GLM, MiniMax. Third-party 直供 models are China-station (Beijing) only and require activating the service in the console first. Qwen-Audio is excluded. [Source](https://help.aliyun.com/zh/model-studio/compatibility-of-openai-with-dashscope)

Text-generation model ids (recommended + legacy), with context / thinking / function-calling / structured-output: [Source](https://help.aliyun.com/zh/model-studio/text-generation-model/)

| Model id (examples) | Context | Thinking | Function Calling | Structured output |
|---|---|---|---|---|
| `qwen3.8-max` | 1M | ✔ | ✔ | ✔ |
| `qwen3.7-plus` (+snapshots) | 1M | ✔ | ✔ | ✔ |
| `qwen3.8-flash` | 1M | ✔ | ✔ | ✔ |
| `qwen3.7-flash` | 1M | ✔ | ✔ | ✔ |
| `deepseek-v4-pro` / `deepseek-v4-flash` | 1M | ✔ | ✔ | ✘ |
| `glm-5.2` | 1M | ✔ | ✔ | ✔ |
| `kimi-k2.7-code` | 256k | ✔ | ✔ | ✘ |
| `MiniMax-M3` | 192k | ✔ | ✔ | ✘ |
| `xiaomi/mimo-v2.5-pro` | 1M | ✔ | ✔ | ✔ |
| `qwen-plus` (+snapshots) | 1M | ✔ | ✔ | ✔ |
| `qwen-max` | 32k | ✘ | ✔ | ✔ |
| `qwen-flash` (+snapshots) | 1M | ✔ | ✔ | ✔ |
| `qwen-turbo` (+snapshots) | 128k | ✔ | ✔ | ✔ |
| `qwen3-max` | 256k | ✔ | ✔ | ✔ |
| `qwen3-coder-plus` / `qwen3-coder-flash` | 1M | ✔ | ✔ | ✔ |
| `qwen-long` / `qwen-long-latest` | 10M | ✘ | ✘ | ✔ |
| `qwq-plus` | 128k | ✔ | ✔ | ✘ |
| `qvq-max` | 128k | ✔ | ✘ (FC) | ✘ |
| `qwen-plus-character` | 32k | ✘ | ✘ | ✘ |
| `qwen-mt-plus/turbo/flash/lite` | 16k | ✘ | ✘ | ✘ |
| `qwen-vl-plus` / `qwen-vl-max` | — | — | ✔ (VL/Omni series) | — |

Notes:
- `qwen-long` does **not** support Function Calling (it is the 10M-context document model; it does support structured output). [Source](https://help.aliyun.com/zh/model-studio/text-generation-model/)
- Function Calling supported set (text): Qwen Max/Plus/Flash/Coder/Turbo series, Qwen3.x / 3.5 / 3.6 / 2.5 / 3.8 open-source series; multimodal: Qwen3-VL-Plus/Flash, Qwen3.5-Omni-*, Qwen3-Omni-Flash, Qwen3-VL open-source; realtime: Qwen-Audio-3.0-Realtime-*; plus DeepSeek / GLM / Kimi / MiniMax families. **GLM models require `tool_stream:true`** in the request or they will not return `tool_calls`. [Source](https://help.aliyun.com/zh/model-studio/qwen-function-calling)
- `qvq-max` does not support OpenAI-compatible HTTP calls (returns HTTP 400 `current user api does not support http call`). [Source](https://help.aliyun.com/zh/model-studio/compatibility-of-openai-with-dashscope)
- The full billing-grade model list is maintained in the console 模型广场 (model market) — treat the table as illustrative, not exhaustive. [Source](https://help.aliyun.com/zh/model-studio/getting-started/models), [Source](https://help.aliyun.com/zh/model-studio/text-generation-model/)

### 7. Deviations / special-cases a Rust provider must handle

1. **Path prefix `/compatible-mode/v1`** — base URL is not `/v1` but `/compatible-mode/v1`; endpoint `/compatible-mode/v1/chat/completions`. [Source](https://help.aliyun.com/zh/model-studio/compatibility-of-openai-with-dashscope)
2. **Region-bound API keys** — 401 `invalid_api_key` / `Incorrect API key provided` if key region ≠ endpoint region; surface this as key/endpoint mismatch, not a bad key. [Source](https://help.aliyun.com/zh/model-studio/compatibility-of-openai-with-dashscope)
3. **`system_fingerprint` not really supported** — docs say it returns an empty string (examples show `null`); don't rely on it for config detection. [Source](https://help.aliyun.com/zh/model-studio/compatibility-of-openai-with-dashscope)
4. **`parallel_tool_calls` defaults to `false`** (OpenAI defaults to `true`) — parallel tool calls only happen when opted in. [Source](https://help.aliyun.com/zh/model-studio/qwen-api-via-openai-chat-completions)
5. **`tool_choice` extensions**: `"required"` is documented (must drop `tool_choice` when summarizing tool results); forced-function `{"type":"function","function":{"name":...}}` is supported but not by thinking-mode models. [Source](https://help.aliyun.com/zh/model-studio/qwen-function-calling), [Source](https://help.aliyun.com/zh/model-studio/qwen-api-via-openai-chat-completions)
6. **GLM function calling requires `tool_stream:true`** or `tool_calls` are never returned. [Source](https://help.aliyun.com/zh/model-studio/qwen-function-calling)
7. **Non-standard top-level body params** (`enable_thinking`, `thinking_budget`, `reasoning_effort`, `enable_search`, `search_options`, `tool_stream`, `top_k`, `repetition_penalty`, `enable_code_interpreter`, `modalities`, `audio`, `skill`, `preserve_thinking`, `clear_thinking`, `thinking`) — an OpenAI-compatible client must allow passthrough of unknown body fields. [Source](https://help.aliyun.com/zh/model-studio/qwen-api-via-openai-chat-completions)
8. **`reasoning_content`** delta/message field for thinking models is not in the OpenAI schema; a strict deserializer must tolerate unknown fields. [Source](https://help.aliyun.com/zh/model-studio/stream)
9. **Usage in streaming is opt-in and only in the final chunk** (`choices:[]` + `usage`); without `include_usage` chunks carry `"usage":null`. A provider must not assume per-chunk usage. [Source](https://help.aliyun.com/zh/model-studio/stream)
10. **`max_tokens` vs `max_completion_tokens`** — `max_tokens` is being deprecated in favor of `max_completion_tokens` (includes chain-of-thought) for thinking models. [Source](https://help.aliyun.com/zh/model-studio/qwen-api-via-openai-chat-completions)
11. **`stop` arrays cannot mix token_ids and strings** in the same array. [Source](https://help.aliyun.com/zh/model-studio/qwen-api-via-openai-chat-completions), [Source](https://help.aliyun.com/zh/model-studio/compatibility-of-openai-with-dashscope)
12. **`tools` + `stream`**: historical docs forbade it; current docs support streaming tool calls with `tool_stream` semantics for complex args. [Source](https://help.aliyun.com/zh/model-studio/compatibility-of-openai-with-dashscope), [Source](https://help.aliyun.com/zh/model-studio/qwen-function-calling)
13. **`n` support is narrow and version-dependent** (older doc: only `qwen-plus`; newer doc: Qwen3 non-thinking + `qwen-plus-character`); forced to 1 when `tools` is passed. [Source](https://help.aliyun.com/zh/model-studio/compatibility-of-openai-with-dashscope), [Source](https://help.aliyun.com/zh/model-studio/qwen-api-via-openai-chat-completions)
14. **Heterogeneous model/feature matrix** — not every model supports every feature (`qwen-long` no FC; `qvq-max` no OpenAI-compatible HTTP; Qwen-Audio not OpenAI-compatible; some models stream-only). Don't assume uniform capability. [Source](https://help.aliyun.com/zh/model-studio/text-generation-model/), [Source](https://help.aliyun.com/zh/model-studio/compatibility-of-openai-with-dashscope)
15. **Error envelope** matches OpenAI (`error.message/type/param/code`); documented status codes: 400 invalid request, 401 invalid API key, 429 rate-limit/quota, 500 server error, 503 overloaded (retryable). [Source](https://help.aliyun.com/zh/model-studio/compatibility-of-openai-with-dashscope)

## Key reference URLs

| # | Page | URL | Used for |
|---|---|---|---|
| 1 | OpenAI兼容-Chat | https://help.aliyun.com/zh/model-studio/compatibility-of-openai-with-dashscope | base URLs, MaaS domains, auth header, region-bound keys, request/response params, error envelope, status codes, model families, OpenAI SDK examples |
| 2 | OpenAI兼容-Chat (chat completions) | https://help.aliyun.com/zh/model-studio/qwen-api-via-openai-chat-completions | authoritative request-body reference: messages/tool_calls/tool, stream_options, tools/tool_choice/parallel_tool_calls, non-standard extras, max_completion_tokens |
| 3 | 流式输出 | https://help.aliyun.com/zh/model-studio/stream | SSE chunk shape, final usage chunk, `[DONE]`, reasoning_content streaming, stream-only models, timeout |
| 4 | Function Calling | https://help.aliyun.com/zh/model-studio/qwen-function-calling | tools JSON schema, tool_calls shape, tool role/tool_call_id, parallel + forced tool calling, streaming tool-call fragmentation, GLM tool_stream requirement |
| 5 | 文本生成 | https://help.aliyun.com/zh/model-studio/text-generation-model/ | model id list with context/thinking/FC/structured-output matrix |
| 6 | 获取与配置 API Key | https://help.aliyun.com/zh/model-studio/get-api-key | DASHSCOPE_API_KEY env-var setup |
| 7 | 选择模型 | https://help.aliyun.com/zh/model-studio/getting-started/models | model catalog entry point (model market links) |

- Dropped: none of the 7 primary URLs were dropped; all were used. (OpenAI-SDK repr snippets on the function-calling page were used only as corroborating evidence for chunk shape, not as a normative spec.)

## Gaps

> 标注 ✅ 的条目已被 [spike-bailian-2026-08-29.md](spike-bailian-2026-08-29.md)（ticket 05）实测收尾。

- **Exhaustive model catalog** is not stable in docs — the definitive, billing-grade model id + capability list lives in the Bailian console 模型广场 (model market), not in a static doc; tables above are illustrative. Suggested next step: query the model-market page / pricing API for a canonical list.
- ✅ **Raw JSON of a streaming tool-call SSE chunk** — live curl capture (`tools` + `stream:true`) confirmed: first fragment carries `id`/`type`/`index`/`name` (arguments `""`), later fragments carry only `index` + `function.arguments` slices with `name:null`/`id:""`; `index` is present on every fragment.
- ✅ **`system_fingerprint`** — a live probe shows the key is **absent** from responses on this endpoint (non-stream and streaming).
- ✅ **`tools` + `stream`** — works on `qwen3.7-plus-2026-05-26` (Beijing MaaS region): streaming tool_call deltas and non-stream assembled tool_calls both confirmed; `parallel_tool_calls:true` yields 2 parallel calls in one turn.
- ✅ **`stream_options.include_usage`** — usage arrives only in the final chunk with `choices: []`; the `usage` key exists on every chunk as `null` otherwise.
- **Error-code catalog** (e.g. model-not-found `invalid_model`, parameter errors) lives in the separate 错误码 (error-code) page, outside the provided source set — worth a follow-up fetch for a complete retry/classification table.
