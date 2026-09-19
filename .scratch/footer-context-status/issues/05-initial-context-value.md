# 05: 启动/session 切换的初始 context 值

**What to build:** footer 从启动第一刻就有 context 信息。App 构造接收初始 context
（新 session → `0%/200k`）；加载已有 session 时按恢复的历史估算，并补发一次带
context 的 usage 渲染项；切换 session 后 footer 的 context 段重置为新 session 的
初始值，不显示上个 session 的残留数据。

**Blocked by:** 04 (needs ContextUsage and the context rendering in place)

**Status:** resolved

- [ ] 启动即渲染初始 context（`0%/200k`）。
- [ ] 加载已有 session 后 context% 按恢复历史估算并补发。
- [ ] 切换 session 后 context 段重置为新 session 初始值，无残留。
- [ ] 全量测试通过；`cargo fmt --all` 与 clippy（`-D warnings`）干净。
