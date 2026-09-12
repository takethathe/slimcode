# 04 (S4): de-business the TUI behind the `UiHandler` seam

**What to build:** ADR-0013. `slimcode-tui` becomes a library the CLI enters: the CLI owns the
process and the application lifecycle, the TUI owns UI mechanics. This is the step that deletes
`crates/tui`'s slimcode dependencies.

**Blocked by:** 03

**Status:** ready-for-agent

- [ ] `tui::run(terminal, app, handler)` with
      `trait UiHandler { on_effect(&mut self, Effect, emit: &mut dyn FnMut(RenderItem)) -> ControlFlow;
      submit(&mut self, Prompt, emit) -> Result<TurnReport, String>; cancel(&mut self) }`.
      The frame loop (crossterm poll ~80 ms + tick + draw + channel drain) and the scoped worker
      thread stay in the library; raw mode, alternate screen, terminal title, panic hook, signals and
      the exit code move to the CLI.
- [ ] Move from `crates/tui` to `crates/cli`: provider/tools construction, `SessionStore`,
      `HistoryStore`, `SkillStore`, context files, environment, `submit_prompt`, `trigger_skill`,
      `replay_history`, `close_turn`, `drive_turn`, the session-log append path, `install_skill`, and
      the semantics of every `Effect` (`/help /new /load /sessions /usage /history /skills
      /install-skill /!! /!N /exit`). The reducer only turns text into an `Effect`.
- [ ] `App::new` takes the initial UI state (session id, branch, skills, history) and an injected
      completion provider; `/help` text, the unknown-command "did you mean" notice and skill-name
      resolution are produced by the CLI and arrive as `RenderItem::Notice`/`Error`.
- [ ] `crates/tui/Cargo.toml` drops `slimcode-agent`, `slimcode-ai`, `slimcode-common` and
      `slimcode-commands`; `crates/tui/src` no longer names `SessionStore`, `SkillStore`, `Config`,
      `BailianProvider` or `Tool`.
- [ ] Tests: `App` keeps its `TestBackend` snapshots; the CLI's handler is covered against a fake
      `UiHandler` that records rendered items (this replaces the terminal-loop integration tests);
      `crates/cli/tests/tui_smoke.rs` still drives the real binary end to end.

- [ ] `cargo test` green, `cargo fmt --all`, `cargo clippy --all-targets --all-features -- -D warnings`
      clean.

## Notes

- `usage_summary` stays in `app`; the TUI's footer formatting stays in `tui::footer`.
- Domain terms (per `CONTEXT.md`, already updated): `CLI`, `TUI`, `Frontend`, `Renderer`, `Effect`
  (UI intent), `RenderItem`.
- Docs: `docs/development.md`'s TUI section (worker-thread runner, `terminal::run(cwd, config, …)`)
  and the CLI section (total entry point) are rewritten here. Decision:
  `docs/adr/0013-tui-runtime-seam.md`.

## Comments
