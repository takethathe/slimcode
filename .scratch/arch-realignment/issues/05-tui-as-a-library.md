# 05: the TUI becomes a library the CLI enters

**What to build:** `slimcode` owns the process and the application; the TUI is a terminal library
that renders and takes input. Starting the binary and running a one-shot prompt behave exactly as
before, but every service, command semantic and session write now happens in the CLI, and the TUI
crate depends on no other slimcode crate.

**Blocked by:** 03, 04

**Status:** resolved

- [x] Entry point is `tui::run(terminal, app, handler)`: the caller builds the terminal, and the
      handler the CLI implements answers the reducer:

      ```rust
      trait UiHandler {
          fn on_effect(&mut self, effect: Effect, emit: &mut dyn FnMut(RenderItem)) -> ControlFlow;
          fn submit(&mut self, prompt: Prompt, emit: &mut dyn FnMut(RenderItem)) -> Result<TurnReport, String>;
          fn cancel(&mut self);
      }
      ```

- [x] Raw mode, the alternate screen, the terminal title, the panic hook, signal handling and the
      exit code live in the CLI; the frame loop (input polling, tick, draw, channel drain) and the
      scoped worker thread stay in the library.
- [x] Provider/tool construction, the session store, input history, skills, context files,
      environment, the turn loop's session writes, cancelled/errored turn closing, skill installation
      and the semantics of every `/` command (`/help /new /load /sessions /usage /history /skills
      /install-skill /!! /!N /exit`) live in the CLI. The reducer turns text into an `Effect` and
      nothing else.
- [x] `/help` text, the unknown-command "did you mean" notice and skill-name resolution are produced
      by the CLI and arrive as `RenderItem`s; command/skill completion candidates come from a
      provider injected at construction, so the TUI imports no command registry and no skills store.
- [x] `crates/tui` lists no `slimcode-*` dependency, release or dev, and its source names none of
      `SessionStore`, `SkillStore`, `Config`, the provider type or the tool type.
- [x] Tests: the TUI's `TestBackend` snapshots still cover `App`; the CLI's handler is covered
      against a fake `UiHandler` that records effects and rendered items; the tmux smoke suite drives
      the real binary in both modes.
- [x] `cargo test` green, `cargo fmt --all`, `cargo clippy --all-targets --all-features --
      -D warnings` clean.
- [x] Docs: the TUI section and the CLI section of `development.md` describe the new split (library
      vs total entry point); the decision is ADR-0013.

## Notes

- Domain terms (per `CONTEXT.md`): `CLI`, `TUI`, `Frontend`, `Effect`, `RenderItem`, `Renderer`,
  `Skill`, `Command`, `Prompt`, `Session`.
- `usage_summary` stays in the app layer; the TUI's own footer formatting stays in the TUI.
- The `Effect` vocabulary is a UI intent, not a semantic: `LoadSession` means "the user asked for
  this session", and the CLI decides what that costs.

## Comments

## Answer

已实现（ADR-0013，含两处实现期收敛，见下）：

- **库入口**：`slimcode_tui::run(terminal, app, handler)`（`crates/tui/src/run.rs`，`lib.rs`
  重导出）。`UiHandler` 定义在 `crates/tui/src/handler.rs`：`on_effect` / `submit` /
  `cancel`。两处与 ADR 草案的偏差已写进 ADR-0013 的 implementation note：
  1. `submit`/`cancel` 取 `&self` 且 `UiHandler: Sync` —— turn 跑在 worker 线程时帧循环还要
     在 Esc 调 `cancel`，一个 `&mut` 借不到两处；CLI 用 `Mutex` 满足 `Sync`。
  2. `on_effect` 返回 crate 自有的 `ControlFlow { Continue, Submit(Prompt), Quit }` ——
     std 的 `ControlFlow` 载不了“请库跑一轮”（`/!!`、skill 触发都会转成 turn）。
- **职责切分**：raw mode / alternate screen / 终端标题 / panic hook / 退出码在
  `cli/src/tui.rs`；帧循环（`event::poll(80ms)`、tick、draw、通道排空）与 scoped worker
  留库内。`Effect` 收敛为 `SubmitPrompt(Prompt)` / `Command{name,arg}` / `Quit` /
  `QuitAfterTurn` / `CancelRunning`：**reducer 不解析命令语义**，`/` 开头原样变成 `Command`。
- **CLI 侧**：`cli/src/tui.rs::TuiSession` 拥有 provider（`Option<Box<dyn Provider + Send>>`，
  turn 期间 take 出来、结束归还）、工具、session store、input history、skills、context
  files、environment，并实现全部 `/` 命令语义（help/new/load/sessions/usage/history/skills/
  install-skill/exit/!!/!N/skill/did-you-mean）、每轮上下文组装、逐消息落盘、失败/取消收尾。
  `/help` 文案、did-you-mean、skill 名解析都以 `RenderItem` 到达；`CliCompletions`
  （命令表 + skills 快照，`/install-skill` 后就地刷新）作为 `CompletionProvider` 在
  `App::new` 注入 —— TUI 因此无命令注册表、无 skills store。
- **显示词汇**：`RenderItem` 去掉 ticket 04 的 `Skills(Vec<SkillInfo>)` 变体与
  `SkillInfo` / `SkillScope`（补全来自注入 provider、`/skills` 文本由 CLI 产出，TUI 已无
  任何 skill 状态）；ADR-0014 D1 已加修订注记。`TuiAdapter` 改为持有库给的 emit 回调
  （不再是 mpsc sender），并新增 `to_footer_usage`（`TokenUsage → FooterUsage` 换算仍只在
  CLI）。
- **依赖**：`crates/tui/Cargo.toml` 无任何 `slimcode-*`（含 dev-dependency）；源码不出现
  `SessionStore` / `SkillStore` / `Config` / provider 类型 / tool 类型（`cargo tree` 与 grep
  均可证）。为让 CLI 持有 `Box<dyn Provider>`，`core::agent::Provider` 增加
  `total_usage()`（默认零）并对 `Box<T: Provider + ?Sized>` 提供转发实现；
  `app::context::skill_loaded_in` 变为 pub（CLI 组装 skill 触发消息用）。

测试：TUI 的 `TestBackend` 帧缓冲测试仍覆盖 `App`（fake `CompletionProvider` 注入，123 条）；
CLI handler 测试 15 条（记录 emit 的闭包 + 脚本化 provider：help/skills/unknown/skill 触发/
重跑与坏索引/new/load/usage/install-skill 刷新候选/整轮落盘/重跑不记录/失败收尾/并发拒绝/
cancel 重置）；table-driven 适配器测试覆盖每个 `DisplayItem` 变体。tmux 冒烟 3 条：pi 风格 UI、
Esc 取消、**one-shot 文本模式**（`slimcode "<prompt>"` + 同一 mock，断言 reasoning 行、工具行、
回答与 tokens 汇总）。

验证：`cargo test --workspace` 全绿（cli 62 + tmux 3 + ai 45 + app 194 + commands 26 +
core 67 + tui 123），`cargo fmt --all`，clippy 0 error / 0 warning。手动 tmux 走查
`/help`（命令表格式不变）、`/skills`、未知命令提示、`/` 补全弹框、`/usage`、`/new`、
`/sessions`、Ctrl+C 退出均正常；调试用 tmux session 已销毁。

文档：`development.md` 的 TUI 段重写为「终端库 + 运行 seam」，CLI 段新增 `tui` 模块与入口
描述；ADR-0013 加 implementation note，ADR-0014 D1 加 ticket 05 修订注记；`CONTEXT.md` 新增
`Effect` / `UiHandler` / `CompletionProvider` 并更新 `RenderItem`。
