# 05: 公开 Engine preset 接缝 + 内建 trace span

**What to build:** 把 Engine 作为可移植引擎的公开构造面立起来：宿主一次构造即可选 preset（code 默认 bundle 或自带一份 hook bundle），并可注入 Operand（Provider/Tool）、Tracer、guardrails 配置等协作者。同时落地 trace 作为**刚需、不退化**接缝：Loop skeleton 在操作数调用处内建 RAII span（iteration / provider.chat / tool.{name}），`Tracer` 作注入组件、无 backend 时 no-op；`SpanRecorder` 透传进 Provider，使请求体属性即使传输失败仍落在 `provider.chat` span。为后续 bot preset（memory/runtime-ctx/governance/rollback/consolidation/通道广播）预留挂载点，但本票不实现 bot 语义。

**Blocked by:** 04

**Status:** resolved

- [x] 存在公开的 Engine 构造面：可选 preset / 注入 hook bundle / 注入 Operand 与 Tracer（不新增 crate；接缝在 app 层公开）
- [x] code preset 仅通过该公开面装配（吃默认 hook），与内部实现解耦
- [x] 注入 `TestTraceBackend` → 收到三类 span 起止与属性；`Tracer=None`/无 backend → no-op 且不报错；传输失败时 recorder 属性仍落 `provider.chat`（spec 清单 7）
- [x] Provider 接缝加 `SpanRecorder`（可选）不破坏 code 型：跑工单 01 基线仍**逐点等效**、现有 provider 测试全绿
