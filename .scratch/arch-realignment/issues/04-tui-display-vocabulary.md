# 04: the TUI speaks its own display vocabulary

**What to build:** The TUI no longer consumes the application layer's display contract. The CLI
converts each `DisplayItem` into a `RenderItem` the TUI owns, the transcript is rebuilt from those,
and the rendering is pixel-for-pixel what it is today.

**Blocked by:** 01 (independent of 03)

**Status:** resolved

- [x] `slimcode_tui::RenderItem` carries no `ai`/`core`/`app` type:
      `Text`, `Reasoning`, `ToolStart{tool_call_id,name,arguments}` and
      `ToolResult{tool_call_id,name,ok,result}` for the streamed agent output, plus
      `Notice`, `Error`, `UserPrompt`, `Usage(FooterUsage)`, `Skills`, `Branch`, `SessionChanged`
      for state the CLI owns.
- [x] `App::apply(RenderItem)` replaces `impl Renderer for App`; the transcript merge/pairing rules
      (streaming text and reasoning merged into the last entry, tool start/result paired on
      `tool_call_id`, status colours) stay private to the TUI.
- [x] The CLI owns the `DisplayItem` → `RenderItem` adapter: it runs on the turn's worker thread and
      is what the worker's channel carries; `DisplayItem::Turn` and `DisplayItem::Stop` are dropped.
      CLI-originated state is emitted as `RenderItem`s too, so the TUI has exactly one input channel.
- [x] The token-usage conversion is done by the CLI: `FooterUsage` keeps a plain constructor and the
      TUI source no longer names the provider's usage type.
- [x] `app`'s display contract is untouched: `DisplayItem`, `map_event`, `Renderer`, `usage_summary`
      and the shared turn runner stay exactly as they are (ADR-0004).
- [x] Tests: a table-driven adapter test covering every `DisplayItem` variant (including the two that
      are dropped); the TUI's `TestBackend` frame snapshots pass with `RenderItem` inputs; the TUI's
      visible output is unchanged.
- [x] `cargo test` green, `cargo fmt --all`, `cargo clippy --all-targets --all-features --
      -D warnings` clean.
- [x] Docs: the TUI section of `development.md` gains the `RenderItem`/adapter paragraph; the
      decision is ADR-0014 plus the amendment note on ADR-0004.

## Notes

- Domain terms (per `CONTEXT.md`): `RenderItem`, `DisplayItem`, `Renderer`, `Entry`, `Transcript`.
- This ticket only adds the vocabulary and the adapter; the frame loop and who owns the services
  move in ticket 05.

## Comments

## Answer

> **修订（ticket 05）**：本票实现的 `Skills(Vec<SkillInfo>)` 变体与 `SkillInfo` / `SkillScope`
> 在 ticket 05 中删除 —— 补全候选由注入的 `CompletionProvider` 提供、`/skills` 文本由 CLI 以
> `Notice` 产出，TUI 不再持有 skill 状态。ADR-0014 D1 与 spec 显示链均已加注。

已实现（ADR-0014，ADR-0004 已加修订注记）：

- 新增 `crates/tui/src/render.rs`：`RenderItem`（`Text` / `Reasoning` / `ToolStart` /
  `ToolResult` / `Notice` / `Error` / `UserPrompt` / `Usage(FooterUsage)` /
  `Skills(Vec<SkillInfo>)` / `Branch(Option<String>)` / `SessionChanged{id}`）与 TUI 自有的
  `SkillInfo` / `SkillScope`。该文件不含任何 `ai`/`core`/`app` 类型。
- `App::apply(RenderItem)` 取代 `impl Renderer for App`（后者已删除），合并/配对规则仍私有；
  `push_notice`/`push_error`/`push_user_prompt` 变为私有，`set_branch`/`set_usage`/
  `set_skills`/`clear_for_new_session`/`apply_loaded_session` 删除（由 `apply` 的
  `Branch`/`Usage`/`Skills`/`SessionChanged` 变体承担）。`App::skills: Vec<SkillInfo>`。
- 适配器归 CLI：`cli/src/render.rs::TuiAdapter`（`impl Renderer`）+ 可直接单测的
  `to_render_item`；`Turn`/`Stop` 返回 `None` 被丢弃。TUI 侧新增
  `terminal::AdapterFactory`（`Fn(mpsc::Sender<RenderItem>) -> Box<dyn Renderer + Send>`），
  `main.rs` 注入 `TuiAdapter`；worker 的通道类型改为 `RenderItem`，`drain_channel` 只做
  `apply`。turn 结束后的 footer 用量也经同一适配器（`DisplayItem::Usage`），CLI 侧状态的
  文案（notice/error/skills/branch/session）在 TUI 内直接以 `RenderItem` 走 `apply`。
- token 用量换算在 CLI：`FooterUsage::new(...)` 取代 `impl From<&TokenUsage>`，
  `crates/tui` 源码不再出现 provider 的用量类型（`total_usage()` 辅助已删）。
- `app` 显示契约未动：`DisplayItem` / `map_event` / `Renderer` / `usage_summary` /
  `run_turn` 原样。`app::skills` 的 `find_skill` / `suggest_skills` / `combined_suggestions`
  / `complete` 改为对 `SkillView`（name/description 只读视图）泛型，`Skill` 与 TUI 的
  `SkillInfo` 各实现一次，预测规则仍只有一份。

测试：cli 侧表驱动适配器测试覆盖全部 `DisplayItem` 变体（含 `Turn`/`Stop` 两个丢弃项）、
通道发送与断管道；TUI 的 `TestBackend` 快照全部改为 `RenderItem` 输入并通过；`/usage`
notice 措辞与 CLI 共用 `usage_summary`；tmux 冒烟两腿（one-shot / TUI）保持通过，即可见输出不变。

验证：`cargo test --workspace` 全绿（cli 47 + tmux 2 + ai 45 + app 194 + commands 26 +
core 67 + tui 132），`cargo fmt --all`，clippy 0 error / 0 warning。文档：development.md 的
TUI 与 CLI 段新增 `RenderItem`/适配器叙述。
