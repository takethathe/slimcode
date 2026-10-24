# GUI usage HUD — 右上角常驻用量

**Status:** done

## Problem Statement

GUI 把 TUI/CLI 共用的 `usage_summary` 文案搬进了瀑布流：wire 的 `usage` 事件是**每条 assistant 消息一条**，
一个带两次工具调用的 turn 会往对话里插三行

```
tokens: 3396 prompt (3334 cached, 98.2%) + 101 completion = 3497 total
```

这行文字挤在正文之间，读完即过期，下一次请求又被覆盖成新的一行。用户想扫一眼"这次花了多少、缓存还生效吗、
上下文还剩多少"时，得回头翻记录；而右上角本来就有 context 徽标那一块空闲位置。

## Solution

右上角变成常驻用量 HUD，瀑布流不再出现 usage 行：

```
↑3.4k  ↓101  CH98.2%  12.3%/200k
```

- `↑` 会话累计的 prompt（input）token
- `↓` 会话累计的 completion（output）token
- `CH` **最近一次请求**的缓存命中率
- `12.3%/200k` 现有的 context 用量徽标（阈值着色不变）

四项常驻，不随用量出现/消失而跳动；鼠标悬停给出缓存读/写计数与原来那句完整汇总。`/usage` 仍然可以敲，
但它变成"HUD 校准 + 闪一下"，不再往对话里追加任何东西。

参照 pi 的 footer stats 行（`packages/coding-agent/src/modes/interactive/components/footer.ts`：`↑ ↓ R W CH% pct/window`）。

## User Stories

1. 作为 GUI 用户，我想在右上角一眼看到本会话累计的 input / output token，这样我不用翻记录就知道这一轮对话花了多少
2. 作为 GUI 用户，我想看到缓存命中率，这样我能判断 prompt 缓存此刻是否生效
3. 作为 GUI 用户，我想让这四项固定在同一位置，这样我每次抬头都能用同一套视线路径找到它们
4. 作为 GUI 用户，我想让对话里不再出现 `tokens: … prompt (… cached, …%) + … completion = … total` 这种行，这样正文不被计量信息打断
5. 作为 GUI 用户，我想让一次带多次工具调用的 turn 不再插入多行用量（一次 turn 三条），这样转录不会越读越长
6. 作为 GUI 用户，我想让缓存命中率反映**最近一次**请求，这样缓存刚失效时我立刻看到它掉下来，而不是被历史平均掩盖
7. 作为 GUI 用户，我想看到缓存读 / 写计数与完整汇总的原文案（并且能看出 tooltip 里的百分比是累计、徽标是最近一次），这样需要精确数字时还能拿到
8. 作为 GUI 用户，我想在端点没返回 cache 详情时看到 `CH—`，这样我知道"未知"而不是误以为"命中 0%"
9. 作为 GUI 用户，我想在端点给了详情但没命中时看到 `CH0%`，这样我能区分"缓存没生效"与"不知道"
10. 作为 GUI 用户，我想在全新会话里看到 `↑0 ↓0 CH— 0%/200k`，这样 HUD 的位置与宽度从第一秒就稳定
11. 作为 GUI 用户，我想在收到 usage 事件时立刻看到累计数字变化，这样 turn 进行中的进度是实时的
12. 作为 GUI 用户，我想在 turn 结束、切换会话、压缩之后看到数字与宿主快照一致，这样本地累加不会漂移
13. 作为 GUI 用户，我想在恢复历史会话时看到累计从 0 开始（并知道这是已知边界），这样我不会把旧数据误当成本会话开销
14. 作为 GUI 用户，我想敲 `/usage` 时看到 HUD 闪一下，这样我知道命令被执行了，而不是"没反应"
15. 作为 GUI 用户，我想在 `/usage` 之后对话里不留任何痕迹，这样命令不污染转录
16. 作为 GUI 用户，我想在 800px 最小窗口 + 长标题时标题被截断而四枚徽标完整，这样窄窗也不丢状态
17. 作为 GUI 用户，我想在系统开启"减少动效"时不再看到脉冲动画，只看到静态描边，这样不会因动效不适
18. 作为 GUI 用户，我想让 context 徽标保持原来的阈值着色（≤70% 灰 / >70% 琥珀 / >90% 红），这样它表达的信息量不变
19. 作为 GUI 用户，我想在新建 / 恢复 / 删除会话后 HUD 立即归零或切换，这样它永远描述"当前会话"
20. 作为维护者，我想让 tooltip 的完整文案与 TUI/CLI 的 `usage_summary` 逐字一致，这样两端不会各说各话
21. 作为维护者，我想让 TUI / CLI / SDK / FFI 这次的代码一行不改，这样这个前端显示决策不牵连其它前端

## Implementation Decisions

- **D1 — 四项，明细进 tooltip。** HUD 只放 `↑ / ↓ / CH / ctx`；`R`（cache read）、`W`（cache write）与
  完整 `usage_summary` 文案进悬停提示。理由：右上角是常驻 HUD，六枚数字过载；完整信息有悬停兜底。
  tooltip 以 `latest CH…` 开头标注徽标那枚数字的口径，因为摘要句里的百分比是**会话累计**（如
  `latest CH0% · R3.3k W0 · tokens: 3396 prompt (3334 cached, 88.3%) + …`）——同一枚 `CH` 在两处
  口径不同，不能让读者以为是同一个数。
- **D2 — `CH` 取最近一次请求。** 与 pi 一致（`cached / prompt`，一位小数、去尾 `.0`）。slimcode 的 TUI footer
  与 `/usage` 用的是**会话累计**比率，两者口径不同且有意保留：GUI HUD 回答"缓存现在还生效吗"，TUI 回答"整体省了多少"。
- **D3 — 累计值不落盘，恢复后归零。** `Session.usage` 是 `#[serde(skip)]`，恢复会话时 replay 只还原 messages 与
  title，因此 HUD 如实显示 0，直到新请求。跨会话累计是独立议题，见
  `.scratch/session-usage-persistence/issues/01-persist-usage-across-resume.md`。
- **D4 — 四项常显，零值也显示。** 空态 `↑0 ↓0 CH— 0%/200k`。`CH—` 表示"端点没给 cache 详情"或"prompt 为 0"
  （未知），`CH0%` 表示"给了详情但没命中"。宽度不随用量跳动。
- **D5 — 数据源与校准。** `CH` 只能来自 wire `usage` 事件（快照里没有"最近一次"）。累计值在事件上本地累加，
  并在每次刷新宿主快照时用 `session.usage()` 覆盖校准；`turn_finished` / `message` / 新建 / 恢复 / 删除会话
  已经都会触发刷新，不需要新触发点。`SdkEvent` 的顺序保证校准点看到的是含本轮的累计值，不会吞数。
- **D6 — 删渲染、留事件。** 转录 block 模型去掉 usage 成员与追加入口，渲染组件整体删除；`GuiWireEvent::Usage`
  与 Rust 侧 `DisplayItem::Usage` 保留，职责从"显示一行"改为"喂 HUD"。Rust 零改动。
- **D7 — `/usage` 保留但退化为校准 + 反馈。** 仍由前端拦截（不发给模型），行为 = 拉一次快照校准 + HUD 一次性
  ring 脉冲（约 600ms）；`prefers-reduced-motion: reduce` 时退化为静态描边。不追加 transcript block。
- **D8 — tooltip 文案前端镜像。** TS 侧镜像 `usage_summary`，用抄 Rust `usage_summary_*` 与
  `cache_hit_percent` 用例的单测挡漂移；不把展示文案塞进 wire 契约（否则每条 assistant 消息都要重复携带同一句模板）。
  前缀 `latest CH{pct}` 由前端拼接，属 GUI 自己的标注，不进镜像断言。
- **模块边界。**
  - 新增一个**纯函数用量模块**：累计、最近一次命中率、百分比/命中率格式化、summary 与 tooltip 文案。无 React 依赖，可单测。
  - `TopBar` 组件：接收累计 usage、最近一次 usage、脉冲计数，渲染四枚徽标与 tooltip。
  - 转录 block 模型：删去 usage 成员，reducer 对 `usage` 事件保留空分支（switch 穷尽性仍要求覆盖该 kind）。
  - App 外壳：持有累计/最近一次/脉冲三个状态，在事件刷新与快照刷新两处更新，`/usage` 改为校准 + 脉冲。
  - 样式表：新增脉冲 keyframes 与 reduced-motion 退化。
- **接口契约（不变）**：`GuiWireEvent::Usage { usage }` 的 JSON 形状、`SessionSnapshot.usage` 字段、`/usage` 的拦截
  集合都保持不变；变化只发生在 GUI 前端如何消费它们。
- **数值格式**：token 走现有 `formatTokens`（`3.4k` / `10k`）；命中率与 context 百分比都是一位小数、去尾 `.0`。

## Testing Decisions

- 好的测试只断言**外部可见行为**：格式化函数的返回值、DOM 里用户能读到的文本、reducer 产出的 block 形状。
  不断言 Tailwind class、组件树结构或内部状态名。
- **纯函数单测**（Vitest，新用量模块）：累计合并（含详情缺失保持 null）、命中率"未知 vs 0%"的区分、
  `98.2%` 舍入、summary 文案与 Rust `usage_summary_*` 用例逐字一致、tooltip 文案。Prior art：
  `transcript.test.ts` 对 reducer 的纯逻辑断言。
- **reducer 单测**（Vitest，转录 block 模型）：`usage` 事件 reduce 后 block 数量与内容不变——把"不显示"这个新契约
  反过来固化。Prior art：同文件的 tool/skill/compaction 用例。
- **渲染层 DOM 单测**（Vitest + jsdom + Testing Library，`TopBar`）：空态四段文案齐全、`CH—` 与 `CH0%` 可区分、
  长标题下四枚徽标仍可读。Prior art：`Markdown.test.tsx`（文件级 `@vitest-environment jsdom`）。
- **App 接线**不做组件集成测试，由上面两层 + 一次 devshot 覆盖；这一层是状态搬运，断言成本高于收益。
- **运行时验收**：devshot 截图四张——新会话空态、一轮带缓存后、恢复会话（归零）、800px 最小宽 + 长标题。
  协议见 `docs/development.md` 的 GUI 一节（只存在于 debug 构建）。
- **Rust 侧**：无改动，`cargo test` 保持全绿即可；`crates/app`、`crates/tui` 的既有 usage 测试是这次的对照组。

## Out of Scope

- 让累计 usage 跨 resume 存活（日志格式改动）——见 `.scratch/session-usage-persistence/issues/01-persist-usage-across-resume.md`。
- TUI footer / CLI 摘要 / `/usage` 在 TUI 的口径与排版（它们继续用会话累计）。
- 成本（`$`）显示：slimcode 的 `TokenUsage` 没有计价字段。
- auto-compaction 的 `(auto)` 指示（pi 有，这次不加，避免与同一行的 Compact 按钮混淆）。
- `R` / `W` 作为独立徽标、每轮增量的展示、历史图表。
- 把 `formatTokens` 统一成与 TUI 逐字一致（GUI 的 `3000 → 3k`、TUI 的 `3000 → 3.0k` 属既有小差异）。

## Further Notes

- 参照实现：`pi/packages/coding-agent/src/modes/interactive/components/footer.ts`（stats 行组装、零值省略、
  `CH` 取最近一次、context 阈值着色）。pi 省略零值是因为它挤在终端的单行 footer 里；GUI 空间固定，所以选择常显。
- 术语：**Usage HUD**（见 `GLOSSARY.md`），避免用 usage line / usage bar / token gauge。
- 架构记录：ADR-0029（本次分叉与放弃瀑布流行的理由）。
- 相关 ADR：ADR-0020（usage 事件流与 context 估算）、ADR-0024（GUI 前端）、ADR-0026（GUI 命令集与 `/usage` 拦截）。
- 票（按依赖顺序）：`issues/01-usage-hud.md` → `issues/02-drop-transcript-usage-rows.md` →
  `issues/03-docs-adr-and-glossary.md`。
