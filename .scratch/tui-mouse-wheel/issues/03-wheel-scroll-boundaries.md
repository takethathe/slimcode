# 03: 锁定滚轮的边界语义（弹框 / 指针 / 短内容 / 非滚轮事件）

**What to build:** 从用户视角，把滚轮的边界行为钉死，避免它们日后被"顺手改得像 `PgUp`/`PgDn` 一样"而悄悄回归：

- `/` 补全弹框打开时，滚轮**仍然只滚 Transcript**（候选高亮不动）—— 即刻意**不**复制 `PgUp`/`PgDn` 的"弹框优先"分支；
- 指针压在输入框、footer 或 Transcript 上，滚轮结果一致（坐标被忽略；不按"指针下区域"分派）；
- Transcript 内容不足一屏时，滚轮不产生任何可见副作用（不留空白、不动滚动条）；
- 点击、按下/释放、水平滚轮**零状态变化**（不移动输入框光标、不展开/折叠工具块、不改补全候选）。

这一票没有新的生产代码路径（01 的 reducer 已按上述语义实现），交付物是**测试 + 文档**：每条边界都要有一条断言它的单测，以及一句写进文档的说明。若某条断言不成立，本票负责修生产代码使其成立。

**Blocked by:** 01 — 滚轮在空闲态滚动 transcript（tracer bullet）

**Status:** resolved

- [x] 单测：`/` 补全弹框打开时滚轮改变滚动位置，且候选高亮（选中项）不变
- [x] 单测：鼠标事件带有落在输入框 / footer 上的坐标时，滚轮结果与坐标落在 Transcript 上完全一致
- [x] 单测：Transcript 内容不足一屏时滚轮后滚动位置、可视窗口、滚动条可见性都不变
- [x] 单测：点击、按下、释放、水平滚轮事件后，滚动位置、输入框文本、recall 状态、工具块展开状态均不变
- [x] 文档说明这些边界是有意的（development.md 的鼠标 reducer 小节，或 ADR-0017 的相应决定条目），并明确"滚轮 ≠ `PgUp`/`PgDn` 的作用域优先级"
- [x] 门禁：`cargo test` 全绿；`cargo fmt --all`；clippy `--all-targets --all-features -- -D warnings` 零 error / 零 warning
- [x] 本地提交（不 push）：`test(tui): pin the wheel's scroll boundaries`

## Notes

- 本票是"验证薄片"：它自己可验证（每条边界都能在测试里演示），但它不新增用户可见能力 —— 若粒度上你想省一张票，可并入 01；保持独立是因为这些边界最容易在后续改动中被无意破坏。
- **实施记录**：四条断言全部在 01/02 交付的 reducer 上成立，未再改生产代码；四条单测加在 `crates/tui/src/app.rs` 的 “mouse wheel（tickets 01–03）” 组（`wheel_is_a_no_op_when_the_transcript_fits_the_pane` / `wheel_scrolls_the_transcript_while_the_completion_popup_is_open` / `wheel_output_does_not_depend_on_the_pointer_position` / `non_wheel_mouse_events_change_no_state`）。短内容那条顺带钉住了 `max_scroll(total, view)` 的“面板填满”上限（ADR-0017 D5）。
