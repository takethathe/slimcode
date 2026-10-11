# 05: CLI：TUI 收敛为消费 SDK

**What to build:** TUI（`TuiSession`）的回合编排收敛为消费 `AgentSession`：当前会话对象（`/new` / `/session` 加载时替换、dispose 旧会话）、事件经 thread-local 槽桥接到库的 emit 通道（实时流式不阻塞）、`/compact` 走按需压缩、加载走凭 id 续会、Esc 仍杀 bash、配置冻结在启动值；`TuiAdapter` 重构为持有自身 live 视图的 Send+Sync 状态对象。TUI 外部行为不变（repair 计数提示移除与 skills 冻结两个小回退文档化）。

**Blocked by:** 01 (with_home + 可取消工具)、02 (with_session_id)、03 (compact_now)

**Status:** ready-for-agent

- [ ] `TuiSession` 内部状态为 `Inner { session: Arc<AgentSession>, adapter: Arc<Mutex<TuiAdapter>> }`：submit（worker）短暂取锁克隆 Arc 后 `prompt`（锁不跨回合）；cancel（frame）只 `abort()` 永不等待；`/new` / 加载仅在空闲时替换并 dispose 旧会话
- [ ] thread-local emit 槽：submit / `/compact` 设槽、监听器读槽调 emit、返回前清槽；监听器路由 Display → adapter render、Message → note_message（不渲染）、TurnFinished → no-op、Compacted → 渲染 usage + 压缩 notice（自动压缩路径）
- [ ] `/compact` 走 `compact_now()`：「compressing context…」提示 → 按返回值渲染 usage + 「context compacted: X → Y tokens」，`Err` 渲染错误行；与自动压缩渲染一致
- [ ] `/session` 加载走 `with_session_id`：历史/标题/branch 回放、usage 从零、当前行 no-op；repair 计数提示移除（文档化）
- [ ] `TuiAdapter` 重构：持有自身 live 视图（`Mutex<LiveUsageView>`）、`render(item, emit)`、`note_message`（Message 事件更新 live）；TUI 入口签名收窄（删 context_files/environment，加 home 供 with_home）
- [ ] 配置冻结：CLI 启动解析一次，把解析结果作为 base_url/model/api_key/cache 覆盖传给 SDK（footer model 与回合一致，`/new` 不漂移）
- [ ] 输入历史记录（`Prompt.record`）留在 CLI（prompt 之前）
- [ ] Fixture 改造：with_home + with_provider + with_tools 经公共 API 建会话（scratch 建 .git marker 使 project home 确定性解析、project_key 对齐 SDK 内部 store）；handler/render 测试全绿；TUI 行为不变

## Answer

已实现：TUI 收敛为消费 SDK 会话——thread-local emit 桥把 SDK 事件渲染进既有帧路径，`/compact` 走 `compact_now`、`/session` 走 `with_session_id`、配置每会话冻结；`TuiAdapter` 用 `Weak<AgentSession>` 订阅避免环、`Mutex<AdapterState>` 保持 Send+Sync。review 后修复了回合末自动压缩的陈旧 usage / footer（`669339c`：SDK 改为在会话状态恢复后再发 `TurnFinished` / `Compacted`）。已知小回退：repair 计数提示移除、skills 每会话冻结。

**Status:** resolved
