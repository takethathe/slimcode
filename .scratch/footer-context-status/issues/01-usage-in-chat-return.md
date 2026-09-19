# 01: usage 随 chat() 返回值流出

**What to build:** Provider seam 的 `chat()` 直接返回本次请求的 usage——它是 LLM
response 的一部分（SSE 末 chunk），不需要额外的回调机制。返回类型从"仅错误"变为
"usage 或 None"：正常 EOF 且响应含 usage 时返回 `Some(usage)`；取消的请求返回
`None`（没完成的响应不产生 usage，也不计入任何统计）；HTTP 错误保持 `Err`。三个
测试替身 provider 同步适配新签名，runner 的调用点适配为接住 usage（本票只暂存、
不向事件传播——传播在 02）。本票是数据流链的地基：usage 首次跨出 provider 边界。

**Blocked by:** None (can start immediately)

**Status:** resolved

- [ ] `chat()` 成功且响应含 usage → 返回 `Some(usage)`。
- [ ] 取消的请求 → `Ok(None)`，不产生 usage、不累加任何统计。
- [ ] HTTP 错误 → `Err`（无 usage）。
- [ ] usage 不进内容 delta 流（内容流/`assemble` 不受影响）。
- [ ] 三个测试替身 provider 适配新签名；runner 调用点接住 usage（暂存，本票不传播）。
- [ ] 全量测试通过；`cargo fmt --all` 与 clippy（`-D warnings`）干净。
