# TUI: 鼠标滚轮滚动 transcript

Status: resolved

## Problem Statement

在 TUI 里滚鼠标，用户期望的是"滚动屏幕上的内容"，实际发生的却是 **Input history recall**：

- TUI 占着 alt screen，且从不订阅鼠标事件，于是终端按 **DECSET 1007（alternate scroll）** 把滚轮翻译成 `↑`/`↓` 按键；
- 而 `↑`/`↓` 在输入框为空时进入 Input history recall，把历史 Prompt 灌进输入框（用户已确认：看到历史输入出现在输入框中，并伴随输出滚动）；
- 一次滚轮手势变成多次按键重复，界面抖动、输入框被覆写；
- Transcript 只能靠 `PgUp`/`PgDn` 翻页，滚轮对查看内容完全没用。

也就是说，`↑`/`↓` 这个"键盘语义"的复用，恰好让滚轮做了用户最不想要的事。

## Solution

从用户视角：

- **滚轮直接滚 Transcript**，与 `PgUp`/`PgDn` 属于同一族手势，一格 3 行；
- 上滚即停止跟随、滚到底自动恢复跟随、右侧滚动条照旧出现并在约 1 秒后淡出；
- **一轮运行中也能滚**（滚轮不打断在途请求，也不是取消）；
- `/` 补全弹框打开时，滚轮仍然只滚 Transcript（弹框由 `↑`/`↓`/`PgUp`/`PgDn` 导航）；
- 指针在哪都无所谓：滚轮永远滚 Transcript，不因为指针压在输入框或 footer 上而失效；
- 滚轮**永不**进入 Input history recall —— `↑`/`↓` 的键盘语义一个字都没变。

两条已知代价（会在文档里写明）：

- 订阅滚轮意味着终端把点击/拖拽交给 TUI，**普通拖拽不再触发原生选区**；Ghostty 下 **Shift+拖拽**仍是原生选区（`mouse-shift-capture` 默认 `false`，Shift 不下发给 app），另有 `mouse-reporting`/`toggle_mouse_reporting` 作为整体逃生门；
- tmux 下需 `set -g mouse on`，否则 tmux 截走滚轮、事件到不了 app。

## User Stories

1. 作为 TUI 用户，我想用鼠标滚轮回看 Transcript 的历史内容，这样我不必把手移回键盘按 `PgUp`。
2. 作为 TUI 用户，我想滚轮一格只移动少量行（3 行），这样我能看清自己滚过了什么。
3. 作为 TUI 用户，我想滚轮**不要**把历史 Prompt 灌进输入框，这样我正在编辑的内容不会被覆写。
4. 作为 TUI 用户，我想滚轮**不要**让界面抖动（一次滚轮 ≠ 一串按键重复），这样阅读时不会丢焦点。
5. 作为 TUI 用户，我想滚轮滚到顶/底就停住（不滚出空白），这样视图不会出现无意义的空屏。
6. 作为 TUI 用户，我想滚轮上滚后屏幕停在原处（不自动追新输出），这样我能安心阅读历史。
7. 作为 TUI 用户，我想滚轮回到底部时自动恢复跟随新输出，这样我不必再按一次键回到末尾。
8. 作为 TUI 用户，我想每次滚动都看到右侧滚动条拇指，并在约 1 秒后淡出，这样我知道自己在 Transcript 的哪个位置。
9. 作为 TUI 用户，我想在 agent 正在跑一轮的时候也能滚轮回看，这样我不用等这一轮结束。
10. 作为 TUI 用户，我想在 agent 正在跑的时候滚轮**不会**取消这一轮，这样回看内容不会有副作用。
11. 作为 TUI 用户，我想 `/` 补全弹框打开时滚轮仍然滚 Transcript，这样我浏览历史内容不会被弹框抢走滚动手势。
12. 作为 TUI 用户，我想指针停在输入框、footer 或任意位置时滚轮行为一致，这样我不必先"瞄准"Transcript 再滚。
13. 作为 TUI 用户，我想水平滚轮、鼠标点击、按下与释放**不改变任何状态**（不移动光标、不展开工具块、不选候选），这样误触不会破坏我的会话状态。
14. 作为 TUI 用户，我想键盘 `↑`/`↓` 的历史 recall 行为完全不变，这样滚轮的改动不会牵连我已经熟悉的键盘操作。
15. 作为 TUI 用户，我想 `PgUp`/`PgDn` 的翻页行为完全不变，这样两套滚动手势可以并存。
16. 作为 TUI 用户，我想 Transcript 内容不足一屏时滚轮没有可见副作用，这样短会话看起来不会"怪怪的"。
17. 作为 TUI 用户，我想退出 TUI 后终端完全恢复（鼠标模式关闭、别的程序拿回鼠标），这样我的 shell 不会粘住鼠标状态。
18. 作为 TUI 用户，我想 TUI 崩溃/panic 后终端同样完全恢复，这样我不需要 `reset` 才能继续用终端。
19. 作为 TUI 用户，我想滚轮只影响视图、不产生任何"效果/动作"（不提交 turn、不跑命令、不改 Session），这样滚轮永远是一个安全操作。
20. 作为 TUI 用户，我想在 Ghostty 里按住 **Shift** 拖拽仍能原生选区复制，这样我在没有自带复制功能的情况下依然能复制文本。
21. 作为 TUI 用户，我想 user manual 明确写出 Shift+拖拽、tmux `set -g mouse on`、以及"点击不再移动光标"这些代价，这样我遇到时不会以为是 bug。
22. 作为 TUI 用户，我想 user manual 的按键表里出现"鼠标滚轮"这一行，这样我不用读源码才知道它可用。
23. 作为维护者，我想这次改动只订阅滚轮、不订阅鼠标移动，这样事件循环不会被 motion 事件淹没。
24. 作为维护者，我想这次改动不引入配置项和加速修饰键，这样"3 行/格"是一个可以在实机上直接调常量解决的事。
25. 作为维护者，我想 ADR 记录"用原生选区退化换取滚轮语义"这个取舍，这样未来有人问"为什么复制要按 Shift"时有据可查。
26. 作为维护者，我想 `CONTEXT.md` 里 `Transcript` 的定义包含"滚轮与 PgUp/PgDn 同族、不是 Input history recall"，这样"滚动"和"历史"两个概念不会再次混淆。

## Implementation Decisions

**归属与边界**

- **滚动权归 app**（保持 alt screen）。终端侧不参与滚动：不退回 main screen / scrollback 模式。
- **只借用参考实现（pi）的"滚轮滚视口"这一条**，不借它 fullscreen 的鼠标家具（见 Out of Scope）。
- 不新增配置项、不做 `Alt+滚轮` 加速；步长是一个与 `PgUp`/`PgDn` 的 10 行并列的常量，取 **3 行/格**。

**终端订阅（最小集）**

- 订阅只开最小集：**`?1000h`**（按键/滚轮的 press/release）+ **`?1006h`**（SGR 坐标），退出时对称关闭。
- **不用** crossterm 现成的一把梭鼠标开关：它会附加 `1002h`/`1003h`/`1015h`，等于订阅鼠标移动事件（motion 洪水），而我们不实现任何 motion 语义。
- **不发** `XTSHIFTESCAPE`：把 Shift 留给终端原生选区（Ghostty 的默认行为）。
- 终端生命周期归 CLI 层（ADR-0013 的划分）：进入 TUI 时启用，`restore_terminal()` 里关闭；panic hook 复用同一个恢复函数，因此异常路径也对称关闭。

**reducer 接口**

- `slimcode-tui` 的 `App` 新增一个与 `handle_key`/`handle_key_running` 并列的鼠标 reducer，形状为"传入一个鼠标事件、无返回值"（滚轮不产生 `Effect`）。
- 它只认 `ScrollUp`/`ScrollDown`，各调用既有滚动语义 3 行；**坐标、修饰键、点击、按下/释放、水平滚轮一律忽略**。
- 复用既有滚动语义，不新增滚动模型：上滚即停止跟随、滚到底自动恢复跟随、滚动条 fade 计数照旧。
- 帧循环的**两条**路径都转发鼠标事件：空闲态的主循环与"一轮运行中"的轮询循环。（后者目前只处理 `Ctrl+C`/`Ctrl+D`/`Esc`，滚轮与它们互不干扰。）

**文档**

- 新增 **ADR-0017**：记录本次取舍 —— 订阅滚轮后原生选区退化为 Shift+拖拽；记录"只订阅滚轮、不订阅 motion"与"不做 pi 鼠标家具"两个决定，以及为什么不用配置项。
- `CONTEXT.md` 的 `Transcript` 词条补一句：滚轮与 `PgUp`/`PgDn` 同属 Transcript 滚动手势，**不是** Input history recall。
- `user-manual.md`：滚动小节 + 按键表新增"鼠标滚轮"行 + 新增"终端注意事项"（tmux `set -g mouse on`、Ghostty Shift+拖拽 / `mouse-reporting`、iTerm2 fast trackpad、点击不再移动光标）。
- `explanation.md`、`development.md`（`App` 的方法清单）、`docs/index.md`（ADR 列表）同步。

## Testing Decisions

- **好测试的标准**：只断言外部行为 —— 输入（鼠标事件或按键）→ 可观察结果（`scroll` 偏移、渲染出的可视窗口、输入框文本、recall 是否激活、滚动条是否可见）。不断言私有函数、不按调用顺序断言实现细节。
- **唯一的 seam（沿用既有、取最高点）**：`App` 的纯 reducer。既有 `handle_key`/`handle_key_running` 单测用的是同一个 seam（prior art：`slimcode-tui` 内 `#[cfg(test)]` 单测 + ratatui `TestBackend` 帧断言）。不新增第二个 seam，不做 CLI 层桩。
- **用例**（基于该 seam）：
  1. 滚轮上滚 3 行，且输入框仍为空、无 recall 激活（本次 bug 的回归钉子）；
  2. 键盘 `↑` 在空输入时**仍然** recall（防止连坐的回归钉子）；
  3. 滚轮下滚到底部恢复跟随（`scroll` 归零、可视窗口落在末尾、不出现空白）；
  4. 内容不足一屏时滚轮无可见副作用；
  5. `/` 补全弹框打开时滚轮仍滚 Transcript，且候选高亮不变；
  6. 指针坐标覆盖在输入框/footer 上，结果与覆盖在 Transcript 上一致（证明坐标被忽略）；
  7. 运行中状态下滚轮照常改 `scroll`；
  8. 点击、按下/释放、水平滚轮不改变 `scroll`、输入框与 recall 状态。
- **渲染层不加新测试**：既有帧断言已覆盖滚动窗口与滚动条可见性（同文件既有测试即 prior art）。
- **端到端（tmux）本次不加**：现有 smoke harness 用默认 tmux socket 且不设 `mouse on`，tmux 会截走滚轮；要在不污染用户 tmux 全局配置的前提下断言，需要改 harness（见 Out of Scope）。滚轮手感由人工在真实终端确认。
- **提交门禁**：`cargo test` 全绿、`cargo fmt --all`、`cargo clippy --all-targets --all-features --message-format=json -- -D warnings` 零 error / 零 warning。

## Out of Scope

- **用鼠标选择文本 + 复制**（参考实现那套：拖拽选区、复制到剪贴板、双击选词、链接点击、"Jump to latest" 提示、transcript 搜索、滚动条拖拽）。本次明确不做 —— 因此不自己重写选择，原生选区退化为 Shift+拖拽（写入文档）。
- 点击定位输入框光标、点击展开/折叠工具块。
- `Alt+滚轮` 加速、步长配置项、参考实现的 `wheelScrollLines` 语义（其缺省为 1 行/格）。
- main screen（终端 scrollback）模式：滚轮原生可用，但会删掉 app 侧滚动模型，并让 `Ctrl+O` 全局展开、resize 重排、`/load` 回放整个 Session、滚动条一起退化 —— 属另一条大改。
- tmux 冒烟 harness 改造（专用 socket + `set -g mouse on`）与滚轮的 tmux 端到端断言。
- 已上滚时的底部提示（"Jump to latest"）。
- 其它终端（iTerm2 / VTE / Terminal.app）的逐一手感校准；文档记录已知坑即可。

## Further Notes

- **事实依据**：Ghostty 1.3.1（`TERM=xterm-ghostty`）实现 DEC 1007（`mouse_alternate_scroll`），这是滚轮变成 `↑`/`↓` 的路径；`↑`/`↓` 在空输入时进入 Input history recall 是 bug 的落点；`PgUp`/`PgDn` 已有滚动语义与滚动条 fade 可直接复用。
- **手感风险**：Ghostty 的 `mouse-scroll-multiplier`（默认偏 `precision:0.1,discrete:3`）可能让一格滚轮投递多个事件，从而使 3 行/格偏快；实机确认后只调那一个常量，不改结构。
- **终端侧逃生门**（写进 user-manual）：Ghostty `mouse-reporting = false` 或 `toggle_mouse_reporting` 键位可整体关掉 app 的鼠标捕获；`mouse-shift-capture` 保持默认 `false` 才有 Shift+拖拽选区；iTerm2 的 fast-trackpad 会丢滚轮增量（参考实现文档推荐关闭）；tmux 需 `set -g mouse on`。
- **调试纪律**（AGENTS.md）：真实二进制放进 tmux 跑一遍验证不崩与退出后终端干净，结束即销毁该 session；但 tmux 会改写滚轮路径，手感必须在真实终端人工确认。
