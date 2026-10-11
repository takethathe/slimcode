# 04: 生命周期：abort / dispose / 单飞

**What to build:** 会话的运行时控制：另一个线程可取消正在跑的回合、dispose 释放会话并拒绝后续 prompt、并发 prompt 被明确拒绝。取消不等待会话锁（take/put 模式，对齐 CLI 的运行时 seam）。

**Blocked by:** 02（会话对象与回合编排）

**Status:** resolved

- [x] 另一线程调用 `abort()` 使运行中的 prompt 尽快以 `StopReason::Cancelled` 结束，已流出的部分内容保留、不进历史
- [x] 回合期间 `abort()` 不等待会话锁（取消只碰共享取消令牌）
- [x] `dispose()` 后 prompt 报错、监听器被清空、不再送达任何事件
- [x] 一个回合进行中再次 `prompt` 返回「a turn is already running」错误（单飞）
