# 04: 会话列表 / 恢复 / 删除

**What to build:** 左侧栏列出历史会话（标题 + 时间，来自与 CLI/TUI 共享的会话仓库），点击条目恢复该会话（历史以消息块渲染、可继续 prompt），删除按钮确认后移除会话（当前会话先 dispose）。切换会话时前端全量加载历史与用量。

**Blocked by:** 02

**Status:** resolved

- [x] 宿主：`list_sessions` 读共享会话仓库的条目（标题 + 时间）；`resume_session` 用 `with_session_id` 恢复（`persist` 默认 true）；`delete_session` 确认后移除日志（当前会话先 dispose）；`get_session` 返回历史消息 / 标题 / 用量供前端全量加载
- [x] 左侧栏 UI：会话列表（标题 + 时间）、点击恢复、新建会话按钮（切换时旧会话 dispose）、删除（带确认）
- [x] 前端消息历史渲染：恢复后历史以用户/助手消息块渲染，可继续 prompt 追加新回合
- [x] Rust 单测（scratch home）：列表/恢复/删除的数据行为（含 CLI/TUI 写入的会话可见、恢复后历史完整、删除后列表消失）
- [x] 手动验收：CLI/TUI 里建一个会话 → GUI 列表可见 → 恢复 → 历史完整可继续 → 删除（确认后消失）；新建会话切换正确

**Gate:** `gui/` 内 `cargo test` 全绿；手动走通「列表 → 恢复 → 继续 → 删除」全流程。

## Answer

Implemented: `list_sessions`/resume/delete/get_session over the shared `SessionStore`
(project-scoped, same as CLI/TUI); resume via `with_session_id`; delete disposes the current
session and removes the JSONL log. Sidebar lists title + time with inline delete confirmation;
resuming reloads the full history through `blocksFromHistory` and continues prompting.
Unit tests cover list/resume/delete data behavior against scratch homes.
