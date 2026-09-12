# 02: 请求开启并行 + 真并发执行 + 默认生效

**What to build:** 请求在声明 `tools` 时携带 `parallel_tool_calls: true`（`tools` 为空时请求字节不变）；一个 Tool batch 内的调用在独立线程上**真并发**执行；工具结果按模型返回顺序进入 history、事件按完成顺序流出；取消契约与串行一致；默认开启、无配置项。完成后，模型一次返回多个独立工具调用即被真并发执行，总等待时间接近最慢调用。

**Blocked by:** 01

**Status:** resolved

- [ ] 请求侧：`tools` 非空时序列化 `parallel_tool_calls: true`；`tools` 为空时字段省略、字节与改动前一致
- [ ] 工具闭包约束放宽为可跨线程共享（现有工具实现无需改动即满足）
- [ ] 并行分支改为 scoped threads 真并发；并发峰值测试 ≥ 2（可观察行为，非实现细节）
- [ ] history 中的工具结果按模型返回顺序（index）追加；逐消息事件随 history 顺序发出
- [ ] 工具开始/结束事件按完成顺序发出
- [ ] 取消契约保持：cancel 后已进入 history 的结果保留、未进入的不追加；已取消的调用不再派发
- [ ] `RunConfig` 默认开启并行；依赖串行语义的既有测试显式声明串行路径、断言不变
- [ ] 共享 turn runner 行为测试：并行批次下 history 顺序与事件流顺序在共享 seam 上成立
- [ ] 全量测试绿

## Answer

已实现：`WireRequest.parallel_tool_calls`（tools 非空时序列化 `true`，为空省略）；`Tool.run` 放宽为
`Fn + Send + Sync`；并行分支改 `std::thread::scope` 真并发（结果经 channel 按完成顺序回报，事件即时流出），
history 按 model 顺序、事件按完成顺序；取消契约不变；`RunConfig` 默认并行、串行路径保留。测试：并发峰值 ≥2、
history/事件顺序分离（agent + common）、wire 序列化。
