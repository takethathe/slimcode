# 03: 内存会话模式（persist: false）

**What to build:** 会话的纯内存模式：`persist: false` 时全程不写任何文件，messages / title / usage 只活在内存，id 与持久化模式同源。回合编排与持久化模式共用同一代码路径，仅持久化点分支，两种模式行为不漂移。

**Blocked by:** 01（共享 id 生成）、02（会话对象与回合编排）

**Status:** resolved

- [x] `persist: false` 创建不触碰磁盘的会话，id 来自共享 `new_session_id()`，不构建会话存储
- [x] 内存模式下 prompt 行为与持久化模式一致（事件序列、StopReason、messages/usage/title 更新），测试断言两种模式事件序列相同
- [x] 测试断言整个回合期间临时目录零文件产生
