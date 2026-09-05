# 02 — Rust OpenAI-compatible 栈选型

Type: research
Status: resolved

## Question

`crates/ai` 实现「OpenAI-compatible 的 chat provider + 流式 + tool_calls」应选哪套 Rust 栈？从一手来源（crates.io / docs.rs 的 crate 文档与源码）查证：

- `async-openai`：是否支持自定义 base_url（对接 Bailian 这类 OpenAI-compatible 服务）？流式？请求/响应中的 `tool_calls`？usage 上报？
- 手写 `reqwest` + SSE 解析（`reqwest-eventsource` / `eventsource-stream` / 手动 `bytes_stream`）：工程量与风险。
- HTTP client（reqwest + rustls）、JSON（serde）、SSE crate 的选型。
- 给出一套推荐方案，服务于一个 slim 的 `Provider` trait（含流式与 tool_calls）。

## Answer

Research 完成（subagent，docs.rs/crates.io 一手来源）。全文：`research/rust-openai-compatible-stack.md`。研究版本：async-openai 0.41.3 / reqwest-eventsource 0.6.0 / eventsource-stream 0.2.3。

关键结论：
- **async-openai 四需求全支持**（`OpenAIConfig::with_api_base` 自定义 base_url、`Chat::create_stream` SSE 流、tool_calls 请求/响应含流式分片、`CompletionUsage` + `include_usage`），但重：内部 `_api` feature 拉入 reqwest 0.13 + eventsource-stream + tower + tokio-stream + secrecy + derive_builder + macros。**留作 fallback**。
- **推荐（slim 手写）**：`reqwest 0.13`（default-features=false，features = json + stream + rustls）+ `eventsource-stream 0.2.3`（传输无关 SSE 分帧，async-openai 内部同款，依赖极小）+ `serde`/`serde_json` + futures-core/tokio。
- **reqwest-eventsource**：仅当接受 reqwest 0.12 锁定 + 内置自动重试（对话流不想要）才选。
- **难点不在 SSE 分帧而在 tool_call delta 累积**：按 `index` 拼接 `arguments` 分片，`finish_reason` 时 JSON 解析，需校验非法 JSON —— 无论哪条路线都一样。
- **Provider trait 建议形态**：`ProviderEvent { TextDelta, ToolCallDelta{index,id,name,arguments_fragment}, Finish{reason}, Usage }` + `ChatProvider::complete/stream`（见调研全文 §Q4 的签名草稿）。
- **残留待验证**（见 05 任务 ticket）：Bailian 是否真发 `[DONE]`、是否认 `include_usage`、额外字段是否破坏严格反序列化 —— 需真实 key 的 live spike。

决议：02 关闭；栈选型喂给 crates/ai；live spike 立为任务 ticket 05。
