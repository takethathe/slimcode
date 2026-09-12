# 05: the TUI becomes a library the CLI enters

**What to build:** `slimcode` owns the process and the application; the TUI is a terminal library
that renders and takes input. Starting the binary and running a one-shot prompt behave exactly as
before, but every service, command semantic and session write now happens in the CLI, and the TUI
crate depends on no other slimcode crate.

**Blocked by:** 03, 04

**Status:** ready-for-agent

- [ ] Entry point is `tui::run(terminal, app, handler)`: the caller builds the terminal, and the
      handler the CLI implements answers the reducer:

      ```rust
      trait UiHandler {
          fn on_effect(&mut self, effect: Effect, emit: &mut dyn FnMut(RenderItem)) -> ControlFlow;
          fn submit(&mut self, prompt: Prompt, emit: &mut dyn FnMut(RenderItem)) -> Result<TurnReport, String>;
          fn cancel(&mut self);
      }
      ```

- [ ] Raw mode, the alternate screen, the terminal title, the panic hook, signal handling and the
      exit code live in the CLI; the frame loop (input polling, tick, draw, channel drain) and the
      scoped worker thread stay in the library.
- [ ] Provider/tool construction, the session store, input history, skills, context files,
      environment, the turn loop's session writes, cancelled/errored turn closing, skill installation
      and the semantics of every `/` command (`/help /new /load /sessions /usage /history /skills
      /install-skill /!! /!N /exit`) live in the CLI. The reducer turns text into an `Effect` and
      nothing else.
- [ ] `/help` text, the unknown-command "did you mean" notice and skill-name resolution are produced
      by the CLI and arrive as `RenderItem`s; command/skill completion candidates come from a
      provider injected at construction, so the TUI imports no command registry and no skills store.
- [ ] `crates/tui` lists no `slimcode-*` dependency, release or dev, and its source names none of
      `SessionStore`, `SkillStore`, `Config`, the provider type or the tool type.
- [ ] Tests: the TUI's `TestBackend` snapshots still cover `App`; the CLI's handler is covered
      against a fake `UiHandler` that records effects and rendered items; the tmux smoke suite drives
      the real binary in both modes.
- [ ] `cargo test` green, `cargo fmt --all`, `cargo clippy --all-targets --all-features --
      -D warnings` clean.
- [ ] Docs: the TUI section and the CLI section of `development.md` describe the new split (library
      vs total entry point); the decision is ADR-0013.

## Notes

- Domain terms (per `CONTEXT.md`): `CLI`, `TUI`, `Frontend`, `Effect`, `RenderItem`, `Renderer`,
  `Skill`, `Command`, `Prompt`, `Session`.
- `usage_summary` stays in the app layer; the TUI's own footer formatting stays in the TUI.
- The `Effect` vocabulary is a UI intent, not a semantic: `LoadSession` means "the user asked for
  this session", and the CLI decides what that costs.

## Comments
