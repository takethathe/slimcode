# 03 (S3): `tui::RenderItem` and the CLI-side adapter

**What to build:** ADR-0014. Give the TUI its own display vocabulary (`RenderItem`) and move the
`DisplayItem → RenderItem` conversion into the CLI, so `slimcode-tui` no longer consumes
`app::DisplayItem` and can eventually drop its slimcode dependencies. `DisplayItem`, `map_event`,
`Renderer` and `run_turn` in `app` stay exactly as they are (ADR-0004).

**Blocked by:** 01

**Status:** ready-for-agent

- [ ] `slimcode_tui::RenderItem`: `Text` / `Reasoning` / `ToolStart{tool_call_id,name,arguments}` /
      `ToolResult{tool_call_id,name,ok,result}` / `Notice` / `Error` / `UserPrompt` /
      `Usage(FooterUsage)` / `Skills(Vec<SkillInfo>)` / `Branch(Option<String>)` /
      `SessionChanged{id}`. No `ai`/`app` type appears in it.
- [ ] `App::apply(RenderItem)` replaces `impl Renderer for App` and `push_display_item`. The
      transcript merge/pairing rules stay private to `App` and keep their `TestBackend` coverage
      (tests switch from `app.render(&DisplayItem::…)` to `app.apply(RenderItem::…)`).
- [ ] `tui::FooterUsage` gets a plain constructor; the `impl From<&TokenUsage> for FooterUsage` moves
      to the CLI (the TUI must not name `ai::TokenUsage`).
- [ ] CLI gains `TuiAdapter` implementing `app::Renderer` (and, for CLI-originated state —
      notices/errors/session change/skills/branch/usage/user prompt — a direct `RenderItem`
      emitter). `DisplayItem::Turn` and `DisplayItem::Stop` are dropped by the adapter.
- [ ] The worker-thread channel that carries display items now carries `RenderItem`
      (`ChannelRenderer` becomes a `RenderItem` sender). The frame loop is untouched in this step
      (S4 moves it behind `UiHandler`).
- [ ] Tests: a table-driven adapter test (`AgentEvent` → `DisplayItem` → `RenderItem`, including the
      two dropped variants), plus `App::apply` tests for every variant.

- [ ] `cargo test` green, `cargo fmt --all`, `cargo clippy --all-targets --all-features -- -D warnings`
      clean.

## Notes

- Do not touch `app::render` in this step — it is the surviving shared contract.
- Domain terms (per `CONTEXT.md`, already updated): `RenderItem` (new), `DisplayItem`, `Renderer`,
  `Entry`.
- Docs: `docs/development.md` TUI section gets the `RenderItem`/adapter paragraph. Decision:
  `docs/adr/0014-tui-zero-dep-and-cli-side-adapter.md` (+ the ADR-0004 amendment note).

## Comments
