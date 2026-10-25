# 06: 事件走有界 message-bus 投递

**What to build:** 把引擎的只读事件投递从"turn 线程同步回调监听器"改为经一条**内部有界 message-bus**（channel）：流式增量、每消息入历史、turn 结束、compaction 发生、span 起止等作为事件发布到总线；一个慢订阅者不会拖住 Loop skeleton 所在线程，突发被容量吸收、超容量按背压处理。严格保持现有顺序契约——一次 prompt 的终态事件（`TurnFinished` → 可选 `Compacted`）仍在取出的 provider/tools/session 状态 put-back 之后投递，订阅者在其中读会话看到的是本轮终态而非 turn 前快照。

**Blocked by:** 05

**Status:** resolved

- [x] 事件经有界总线投递、具备背压；慢/阻塞订阅者不 stall 内核线程（并发测试证明）
- [x] 终态事件在状态 put-back 之后送达，顺序契约测试通过（延续 SDK 现有语义）
- [x] panicking 订阅者被隔离，不中断回合、不影响其他订阅者（延续现有 catch_unwind 行为）
- [x] 跑工单 01 基线：事件序列与相对顺序 **逐点等效**（spec 清单 13）；单飞语义不变
