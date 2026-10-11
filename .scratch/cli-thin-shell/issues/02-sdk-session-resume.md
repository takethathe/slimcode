# 02: SDK：with_session_id 凭 id 续会

**What to build:** SDK 会话可凭既有会话 id 从内部 store 宽容回放恢复（messages / title 就位、续写同一日志），宿主可恢复会话而不是每次新建；`persist: false` 时给明确报错（加载必须有 store）。CLI 未动。

**Blocked by:** None (can start immediately)

**Status:** ready-for-agent

- [ ] `AgentSessionOptions.with_session_id(id)`：创建时从内部 store 按 id 宽容回放既有会话（usage 从零，本就不落盘）
- [ ] `persist: false` 搭配 `with_session_id` 返回明确错误
- [ ] SDK 测试：续会回放 messages/title、续写同一日志可被 store 回放为追加后视图、`persist: false` 报错；既有测试全绿

## Answer

已实现：`AgentSessionOptions::with_session_id` 按 id 恢复会话，启动时把存档回放成消息与用量；`with_persist(false)` 下恢复不写回日志。测试覆盖恢复回放与 persist 守卫。

**Status:** resolved
