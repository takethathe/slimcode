# 07: async 门面（prompt-future + runtime-agnostic Stream）

**What to build:** 在同步内核之上提供 Engine 的异步门面：内核跑在一条**专用 OS 线程**，对外暴露非阻塞的 `prompt`（future）与从工单 06 总线喂出的 **runtime-agnostic 事件 `Stream`**（`futures-core::Stream`）。slimcode 全栈**不引入 tokio**；宿主在自己运行时里驱动门面。Operand 保持同步 trait——code preset 的同步 provider/tool 零过桥；文档化宿主侧 `block_on` shim 约定（把 async provider/tool 适配成同步 Operand，运行在该非 tokio 专用循环线程上，合法）。

**Blocked by:** 06

**Status:** resolved

- [x] 用 `futures::executor::block_on` 驱动 `prompt` + `StreamExt::next` 拉事件，能跑完一整轮；事件顺序与同步内核一致（spec 清单 10）
- [x] 终态事件在状态 put-back 之后进入 `Stream`
- [x] 依赖断言/构建证明 slimcode（core/app/sdk）**无 tokio 依赖**；门面 runtime-agnostic
- [x] code preset 同步路径不过桥（provider/tool 为同步 trait）；提供 async→sync shim 的文档化契约与一个用 mini-runtime 模拟的过桥测试（spec 清单 11）
- [x] 不影响 FFI C-ABI（同步窄接口不驱动循环）与 GUI（继续用 AgentSession）
