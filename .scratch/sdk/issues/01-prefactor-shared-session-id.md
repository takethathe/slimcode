# 01: 抽出共享的会话 id 生成（new_session_id）

**What to build:** 会话 id 的生成从会话存储中抽出为 app 层的共享自由函数，使持久化会话与内存会话（后续票）生成一致的 id，现有行为完全不变。这是后续内存模式的前置重构。

**Blocked by:** None (can start immediately)

**Status:** resolved

- [x] app 层新增自由函数 `new_session_id()`，产出 `slimcode-<unix>-<pid>-<n>` 形状的 id（与现 `SessionStore::new_id` 同方案，无 chrono 依赖、跨版本稳定）
- [x] `SessionStore::new_id` 改为委托该函数，id 输出逐字节不变
- [x] 既有 session store 测试与全仓测试全绿，无行为变化
