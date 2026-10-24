# 01: 右上角出现 Usage HUD

**What to build:** 跑完一轮对话后，窗口右上角（context 徽标左侧）同时显示四项用量：累计输入、累计输出、
最近一次请求的缓存命中率、context 占比——`↑3396 ↓101 CH98.2% 12.3%/200k`。四项常驻，全新会话也显示
`↑0 ↓0 CH— 0%/200k`，宽度不随用量跳动；鼠标悬停给出缓存读/写计数与完整的累计汇总文案。数字随
assistant 消息的 usage 事件即时更新，并在 turn 结束、切换会话时与宿主快照对齐。转录此时**仍**显示旧的
usage 行（下一张票才删），所以这一张只加不改。

**Blocked by:** None (can start immediately)

**Status:** done

- [x] 一轮带缓存的请求后，四枚徽标显示 `↑`/`↓`/`CH`/`ctx` 的实际数值，`CH` 是最近一次请求的命中率（spec D2）
- [x] 空会话显示 `↑0 ↓0 CH— 0%/200k`；端点未给 cache 详情时为 `CH—`，给了详情但未命中时为 `CH0%`（spec D4）
- [x] 悬停提示为 `latest CH… · R… W… · tokens: … prompt (… cached, …%) + … completion = … total`，
      摘要句与 TUI/CLI 的 `usage_summary` 逐字一致（spec D1/D8）
- [x] 累计值在 usage 事件上即时累加，并在快照刷新时被 `session.usage()` 覆盖校准，不漂移、不吞数（spec D5）
- [x] 新建 / 恢复 / 删除会话后 HUD 立刻描述当前会话（恢复会话时如实为 0，spec D3）
- [x] 纯函数单测覆盖：累计合并与详情缺失、命中率"未知 vs 0%"、`98.2%` 舍入、summary 与 tooltip 文案
- [x] jsdom DOM 单测覆盖：空态四段文案、`CH—` 与 `CH0%` 可区分、长标题下四枚徽标仍可读
