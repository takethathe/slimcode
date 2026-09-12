# 02: 运行中滚轮同样滚动 transcript

**What to build:** 从用户视角：一轮 agent 正在跑的时候滚动滚轮，Transcript 照常上/下滚动（3 行/格、滚动条淡出、上滚停跟随、到底恢复跟随都与空闲态一致），而且**不取消**这一轮、不影响在途的 LLM 请求或工具调用。今天运行中的输入只认 `Esc`（取消）、`Ctrl+C`/`Ctrl+D`（运行结束后退出），滚轮事件被静默丢掉。

实现落点是帧循环的"运行中"轮询路径：它现在只把按键交给运行中的键盘 reducer，需要同样把鼠标事件转发给 01 交付的鼠标 reducer；运行中的既有按键语义（`Esc` 取消、`Ctrl+C`/`Ctrl+D` 标记运行后退出）一个字都不改。

**Blocked by:** 01 — 滚轮在空闲态滚动 transcript（tracer bullet）

**Status:** resolved

- [x] agent 运行中滚轮滚动 Transcript，3 行/格，行为与空闲态一致（含滚动条出现与淡出）
- [x] 运行中滚轮不取消这一轮，不打断在途请求或工具调用（滚轮只是视图操作）
- [x] 运行中 `Esc` 仍然取消当前轮；`Ctrl+C`/`Ctrl+D` 仍然标记"运行结束后退出"
- [x] 单测（同一 `App` seam）：运行中状态下滚轮改变滚动位置；既有"运行中按键"单测保持不变
- [x] 文档补一句"运行中也能滚"（user manual 的滚动小节；development.md 的 `App` 方法/输入路径清单补上鼠标 reducer 与两条转发路径）
- [x] 门禁：`cargo test` 全绿；`cargo fmt --all`；clippy `--all-targets --all-features -- -D warnings` 零 error / 零 warning
- [x] 本地提交（不 push）：`feat(tui): keep the wheel scrolling while a turn runs`

## Notes

- 失败模式是"滚轮在运行中静默失效"，所以本票的验收必须在**一轮真实运行中**手动做一次（tmux 里起一轮 mock 或真实请求都可，仅用于确认不崩与终端干净），手感仍在真实终端确认。
- 边界语义（弹框、指针位置、短内容、非滚轮事件）由 03 交付。
- **实施补充（超出原票字面、但为“行为与空闲态一致”所必需）**：既有 `App::apply` 对**每个** item 都调用 `reset_view()`，即上滚后下一条流式 delta 就会把视口拽回底部 —— 那样运行中滚轮回看实际不可用。因此 `apply` 改为只在 `follow` 时重新锚定；上滚泊住（`follow == false`）时把本次追加的行数加到 `scroll` 上，让窗口停在原内容（滚轮与 `PgUp`/`PgDn` 同样受益）。见 ADR-0017 D6；`/new`/`/load` 仍整体 `reset_view`。
- **实施验证记录**：真实二进制 + 本地 mock SSE（0.5s 一个 chunk）在 tmux 里跑过一轮：第 2s（运行中）发两格滚轮，窗口上移 6 行且输入框仍为空、spinner 继续动（未被取消）；随后 2.5s 内又有 4 个 chunk 落地，`capture-pane` 与滚轮后那一帧**逐行相同**（唯一差异是运行结束 spinner 消失）——证明上滚泊住的视口不被新输出拽走；再发 12 格下滚回到最新内容。退出后 shell 干净，tmux session 已销毁。
