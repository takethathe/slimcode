# 01: 让累计 usage 跨会话存活

**Status:** needs-triage

**背景（来自 GUI usage HUD 讨论）:** GUI 的右上角 HUD 要显示 input / output / cached rate / context usage。其中前两项取会话累计 usage。当前
`Session.usage` 是 `#[serde(skip)]` 的内存字段（`crates/core/src/session.rs`），恢复会话时 replay 只还原 messages 与 title，
**usage 从 0 开始**——于是 `/session` 恢复后 HUD 显示 `↑0 ↓0`，直到新一轮请求。pi 的做法是从会话日志里的全部 entries
重算 totals，所以它恢复后数字仍在。

**What to build:** 让恢复后的会话累计 usage 与恢复前一致，覆盖 TUI footer、CLI 摘要、GUI HUD、SDK `usage()` 与 FFI `usage_json`
所有读这个累计值的地方。

**Open questions（triage 时要定）:**

- 日志形态：assistant `message` 条目携带该请求的 usage，还是新增独立的 `usage` 条目？
- 老日志兼容：缺少 usage 的条目按 0 计（数字偏小但不报错）是否可接受？
- compaction summary 条目的 usage 是否也计入累计？
- `cache_creation_input_tokens` / `cached_tokens` 的详情要不要一并持久化（GUI 的 cached rate 需要它）？

**验收标准（草案）:**

- 跑一轮后恢复会话，四个前端的累计数字与恢复前一致
- 老格式日志能被 lenient replay 读入，usage 从可得的条目累加，不 panic
- `crates/core/src/session.rs` 的 `session_usage_never_reaches_the_log` 测试按新契约改写
