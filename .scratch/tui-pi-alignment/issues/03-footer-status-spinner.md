# 03: Footer, status indicator, worker-thread spinner, terminal title

**What to build:** The pi-style two-line Footer (dim cwd/branch/session; dim token stats `↑in ↓out Rcache Wcache CH%` with right-aligned model, pi compact number formatting), the Status indicator row (pi braille frames at 80ms, accent spinner + muted "Working...", hidden when idle), the worker-thread turn runner so the spinner animates during a run (mpsc channel of `DisplayItem`s, UI loop polls crossterm events + channel with an 80ms timeout, `App::tick()` advances the frame), the `Tool::run` + `Send` widening, quit-after-turn for Ctrl+C/Ctrl+D while running, best-effort git branch in the Footer, and the OSC 0 terminal title (`slimcode - <session> - <cwd basename>`, set on entry and on `/new` `/load`).

**Blocked by:** 02 — Block-aware transcript + layout

**Status:** resolved

- [x] Footer: line 1 dim `~/path (branch) • session-id` (whole line dim — pi wraps the line in one `dim`, no gray branch; best-effort `git branch --show-current`); line 2 dim stats with right-aligned model; pi `formatTokens` ported; truncation to width.
- [x] `/usage` renders a dim detail line (no magenta block); the footer accumulates from provider totals; per-turn usage line removed.
- [x] `Tool::run` becomes `Box<dyn Fn(Value) -> Result<String,String> + Send>`; all call sites compile unchanged.
- [x] One turn's `run_turn` moves to a worker thread streaming `DisplayItem`s over a channel; the loop polls with an 80ms timeout, advances the spinner via `App::tick()` and redraws; Ctrl+C/Ctrl+D set quit-after-turn; other keys ignored while running.
- [x] Status indicator row: hidden when idle, spinner + "Working..." while running; idle rows removed from layout.
- [x] Terminal title set on entry and updated on `/new`/`/load`.
- [x] `App::tick()` and all running-state rendering covered by `TestBackend`/unit tests; shell changes stay thin but every decision is extracted: the channel worker is tested end-to-end with a scripted provider + channel-recording renderer asserting the ordered `DisplayItem` stream and final result (FakeProvider prior art); the quit-after-turn decision is a pure `App` transition (non-Ctrl+C keys ignored while running, flag fires after the turn); `terminal_title(session, cwd)` and the footer/branch formatting are pure unit-tested functions; the git-branch wrapper is integration-tested in a temp `git init` repo (skipped when git is absent). clippy clean.

## Notes

- Spec: User Stories 10–14, 18, 20; Implementation Decisions "Footer", "Status indicator + spinner animation", "Terminal title"; ADR-0006 D5/D6/D7.
- If `Send` widening proves problematic, fall back to per-item frame advance (spinner freezes during long tool runs) and record the deviation; the worker design is the approved default.
- Context%/cost omitted (no data source) — do not fabricate a window size.