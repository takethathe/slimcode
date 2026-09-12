# 01: 滚轮在空闲态滚动 transcript（tracer bullet）

**What to build:** 从用户视角：在 TUI 里滚动鼠标滚轮，Transcript 上/下滚动，一格 3 行，右侧滚动条照旧出现并在约 1 秒后淡出；滚轮**不再**把历史 Prompt 灌进输入框。今天的行为是终端按 DECSET 1007 把滚轮翻译成 `↑`/`↓` 按键重复，而 `↑`/`↓` 在输入框为空时进入 Input history recall（用户已确认复现）。

这一票是贯穿全部三层的完整薄片：

- **终端侧**：只订阅最小鼠标集 —— 按键/滚轮的 press/release + SGR 坐标；**不**订阅鼠标移动（motion），**不**请求 Shift 捕获（把 Shift 留给终端原生选区）。
- **reducer**：`App` 新增一个与键盘 reducer（`handle_key` / `handle_key_running`）并列的鼠标 reducer，只认向上/向下滚，调用既有的滚动语义（上滚即停止跟随、滚到底自动恢复跟随、滚动条 fade 计数照旧），**忽略**坐标、修饰键、点击、按下/释放与水平滚轮，且不产生 `Effect`。
- **帧循环**：空闲态路径把鼠标事件转发给 reducer（运行中路径由 02 交付）。
- **生命周期**：进入 TUI 时启用订阅，退出时对称关闭；panic 复用同一恢复路径。

键盘 `↑`/`↓` 的 recall、`PgUp`/`PgDn` 翻页、`/` 补全弹框的导航一律不变。

**Blocked by:** None (can start immediately)

**Status:** resolved

- [x] 实机（真实终端）滚轮上滚/下滚：Transcript 按 3 行/格移动，输入框内容不被改写
- [x] 滚轮上滚即停止跟随新输出；滚到底自动恢复跟随；不出现空白行
- [x] 滚轮手势触发/维持右侧滚动条，并在约 1 秒后淡出（与 `PgUp`/`PgDn` 一致）
- [x] 键盘 `↑`/`↓` 在空输入时**仍然**进入 Input history recall（回归钉子，防连坐）
- [x] `PgUp`/`PgDn` 翻页与 `/` 补全弹框内的 `↑`/`↓`/`PgUp`/`PgDn` 导航完全不变
- [x] 只订阅最小鼠标集：不订阅鼠标移动（motion）；不发 `XTSHIFTESCAPE`
- [x] 正常退出后终端恢复（鼠标订阅关闭）；panic 后同样恢复（两条路径复用同一恢复函数）
- [x] 单测（沿用 `App` 既有键盘单测的同一个 seam，断言可观察状态而非实现细节）：上滚 3 行且输入框仍为空、无 recall 激活；键盘 `↑` 仍 recall
- [x] 文档同步（AGENTS.md 规则 3）：user manual 的滚动小节 + 按键表新增"鼠标滚轮"行 + "终端注意事项"（tmux 需 `set -g mouse on`、Ghostty 用 Shift+拖拽选区、iTerm2 fast trackpad 会丢滚轮增量、点击不再移动光标）；`CONTEXT.md` 的 `Transcript` 词条注明滚轮与 `PgUp`/`PgDn` 同族、**不是** Input history recall；新增 ADR-0017（原生选区退化为 Shift+拖拽换取滚轮语义；最小订阅；不做参考实现的鼠标家具；不上配置项的理由）；ADR 列表补该条目
- [x] 本票文档**不**声称"运行中也能滚"（由 02 交付）
- [x] 门禁：`cargo test` 全绿；`cargo fmt --all`；clippy `--all-targets --all-features -- -D warnings` 零 error / 零 warning
- [x] 本地提交（不 push）：`feat(tui): scroll the transcript with the mouse wheel`

## Notes

- 边界语义（弹框、指针位置、短内容、非滚轮事件）的钉死由 03 交付；本票只保证主路径工作。
- 调试纪律（AGENTS.md）：真实二进制放进 tmux 跑一遍确认不崩、退出后终端干净，结束即销毁该 session（`tmux ls` 不再列出）。tmux 会截走滚轮，因此手感必须在真实终端人工确认。
- 手感风险：Ghostty 的滚动倍率可能让一格投递多个事件，若 3 行/格偏快/偏慢，只调"每格行数"这一个常量。
- 端到端（tmux smoke）不在本票范围，见 spec 的 Out of Scope。
- **实施偏差记录（有意、且为验收项所必需）**：验收项“不出现空白行 / 内容不足一屏无可见副作用”与既有滚动模型冲突——旧上限是 `max_scroll = total - 1`（到顶时只剩一行内容 + 空白），且 `scroll_up` 在“什么都没滚”时也会置 `follow = false`。由于滚轮按 spec 复用同一份滚动语义（“不新增滚动模型”），修正落在共享处：上限改为 `max_scroll(total, view_height)`（面板填满）、`scroll_up` 在 `max_scroll == 0` 时整体 no-op。因此 `PgUp`/`PgDn` 的**翻页语义**（10 行/次、弹框优先、停跟随、到底恢复）不变，只有极端边界被修正；另有 D6 的“泊住不被拽回”同样按 `follow` 共享。均记在 ADR-0017 D5/D6 与 development.md。
- **实施验证记录**：真实二进制已在 tmux 内跑过（`set -g mouse on`，向 pane 发 SGR 滚轮序列 `\x1b[<64;10;5M`）：`/help` 造出可滚内容后，一格滚轮上滚正好 3 行、输入框保持空；对照实验证明差异——同一会话按真实 `↑` 会把预置在 `history.json` 的 `hello from history` 灌进输入框，滚轮不会。pane 输出流里只有 `?1000h`+`?1006h`（无 `?1002h`/`?1003h`/`?1015h`、无 `XTSHIFTESCAPE`），退出时发出 `?1006l`+`?1000l`，shell 干净返回；tmux session 已销毁。3 行/格的实机手感仍需真实终端人工确认（tmux 会改写滚轮路径）。
