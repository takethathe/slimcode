# 04: Verification and docs sync

**What to build:** Final integration pass for the pi display alignment: full-workspace `cargo fmt --all`, `cargo clippy --all-targets --all-features -- -D warnings` (0 errors/warnings), `cargo test` green, manual TUI smoke check (layout, spinner, footer, tool blocks, resize, exit) and the docs sync mandated by AGENTS.md.

**Blocked by:** 03 — Footer, status, spinner

**Status:** resolved

- [x] **tmux rendering verification**: `crates/cli/tests/tui_smoke.rs` — real-terminal smoke of the built
  `slimcode` binary in a detached tmux pane against a local mock SSE server speaking `wire::parse_stream`
  format (scripted two-turn chat: reasoning → text → real `ls` tool call → follow-up answer; per-event
  delays keep the run ~2.5s): capture-pane asserts the header (name + hint), the boxed prompt, thinking,
  tool block with pretty args + real tool output, ≥2 distinct spinner chars across captures while running,
  final answer, two-line footer (` • ` line 1; `↑` stats + right-aligned model line 2), `/` popup opens on
  `/` and Esc closes it, resize to 80×14 overflows and PageUp/PageDown scroll between header and bottom,
  resize back keeps the dock fixed with the stats line last, `#{pane_title}` == `slimcode - <session> -
  <cwd basename>`, Ctrl+C exits and restores the pane. Auto-skips when `tmux -V` fails; polling is
  wait_for-with-timeout. Stable over repeated runs (~3.8s each).
- [x] `cargo fmt --all` + clippy 0 warnings + `cargo test` green across the workspace (incl. the tmux
  test when tmux is present).
- [x] **Feature → test coverage checklist**: `.scratch/tui-pi-alignment/test-map.md` — every spec user
  story (US 1–24) and implementation decision (D1–D7) maps to ≥ 1 concrete test by name (app frame-buffer
  tests, markdown/theme/text/footer/git leaf tests, the terminal channel e2e, and the tmux smoke). No
  checklist line without a test; `cargo llvm-cov` not installed in this environment (noted in the file).
- [x] `docs/development.md` updated for the TUI architecture: module list (theme/markdown/text/footer/git),
  block-aware transcript, header/boxes/tool blocks, footer/status indicator, worker-thread spinner runner
  (replaces the stale sync-runner paragraph), select-list popup tokens, test seams incl. the tmux harness;
  crate-table row updated.
- [x] `docs/user-manual.md` updated: TUI display description rewritten for the pi-aligned layout (header,
  boxed prompts, markdown/thinking, state-colored tool blocks + Ctrl+O collapse, footer two lines incl.
  branch, spinner row, editor-border semantics, scrollbar, terminal title, popup styling, removed
  markers/usage line); key table gains Ctrl+O and the run-time Ctrl+C/Ctrl+D semantics.
- [x] `docs/explanation.md` checked and corrected: per-turn usage line wording (footer stats + `/usage` dim
  notice replace the old run-end summary sentence) and the TUI-features sentence now mentions dock footer
  + status indicator.
- [x] Manual smoke: the tmux harness is the scripted stand-in (mock SSE + scripted keys) — header, boxed
  prompt, markdown + thinking, tool block, footer stats, animated spinner during a slow response, popup
  restyle, scrollbar while scrolling, resize keeps the dock fixed, Ctrl+C quits and restores the pane all
  pass end-to-end; a real-terminal debug session confirmed colors/layout match the ADR.
- [x] Tickets 01–03 marked resolved; spec deviations already corrected in-session (footer branch dim, not
  gray — US 11 / D5; `Working...` ASCII — US 12; title `<cwd basename>` — US 18).

## Notes

- AGENTS.md: fmt + clippy before every commit, docs sync before commit, local commits only.
- This ticket is the acceptance gate; resolve it only when the whole alignment is verified.