# 04: footer 渲染 context 段

**What to build:** 用户能在 footer 看到 context 占用。新增 `ContextUsage { percent,
window }` 结构（无 Option——slimcode 常数窗口 + 自己预估，总可估，无 pi 的 `?`
未知态）；usage 渲染项 payload 加宽为"usage + context"（context 为 Option，覆盖
SessionChanged 后的短暂过渡）；TuiAdapter 在收到 usage 时读 session 消息历史 →
估算 token → 组装 ContextUsage（估算与压缩触发同源）→ 传给 footer；`StatusLine`
加 context 字段。footer 纯函数层新增 context 段组装（percent, window → 文本 + 等级
`Normal/Warn/Critical`，阈值照抄 pi：>90 Critical、>70 Warn、否则 Normal）。渲染层
把 stats 行改为多 span：dim 统计 + 按等级着色的 context 段 + dim 模型名；窄终端下
context 段随统计一起截断不溢出。

**Blocked by:** 03 (needs the per-message usage event mechanism)

**Status:** resolved

- [ ] `ContextUsage { percent, window }` 结构存在。
- [ ] usage 渲染项携带 `context: Option<ContextUsage>`。
- [ ] TuiAdapter 读 session 历史估算 context%（与压缩触发同一估算器/窗口）。
- [ ] context 段纯函数：阈值 70/90 边界、200k 格式化正确。
- [ ] footer 渲染 `45.3%/200k`；>90 红、>70 黄、否则 dim（仅 context 段着色，stats/model 保持 dim）。
- [ ] 窄终端下 context 段截断不溢出。
- [ ] 全量测试通过；`cargo fmt --all` 与 clippy（`-D warnings`）干净。
