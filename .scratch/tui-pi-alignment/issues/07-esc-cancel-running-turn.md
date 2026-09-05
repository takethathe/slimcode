# 07: Esc cancels the running turn anywhere in the agent runner

**What to build:** The user's report: Esc still cannot cancel the current task — an
in-flight LLM request, tool calls, etc. Today Esc is a no-op while a turn runs
(only Ctrl+C/Ctrl+D set quit-after-turn; the spec explicitly listed Esc-interrupt as
out of scope because the provider seam was synchronous and non-cancellable). Make Esc
cancel the running turn **at any position in the agent runner**: while the LLM request
is in flight (interrupt the body read), before/after each tool call and between
iterations, and during a long-running tool (bash child process killed). After cancel
the TUI returns to idle immediately; whatever already streamed / was already applied
to history stays; no error line appears.

**Blocked by:** 06 — per-tool titles (07 and 06 both touch the tool construction in
`crates/agent/src/tools/files.rs`; land 06 first to settle that churn)

**Status:** resolved

- [x] **`CancelToken` (new, `slimcode-agent`)** — a small shared handle over
      `Arc<AtomicBool>` with `new` / `cancel` / `reset` / `is_cancelled` and `Clone`.
      Unit tests: default false, cancel flips, reset clears, clones observe the same
      flag.
- [x] **`StopReason::Cancelled`** added to `slimcode-agent`; `run_loop` (and every
      entry point that feeds it: `run_agent`, `run_agent_from_messages`,
      `run_agent_from_messages_sink`) gains a `cancel: &CancelToken` parameter and
      boundary checks:
      - before a provider `chat` call (flag set → no request is made, stop Cancelled);
      - right after `chat` returns an `Err` — if the flag is set, treat it as
        Cancelled (silent stop), otherwise propagate the provider error;
      - before assembling/pushing the assistant message (a cancel that landed during
        the request discards the un-emitted deltas so no half-text message enters
        history);
      - before each tool dispatch and after each tool result, serial and parallel;
      - between iterations. Partial history (tool results already appended) is
        returned as-is with `StopReason::Cancelled`.
      Tests: cancel before first chat → zero provider calls, Cancelled; cancel set
      after turn 1's tools → Cancelled stop with the tool result in history; provider
      Err while flag set → Cancelled not an error; cancel between parallel tools.
- [x] **`Provider::chat` signature widening** — add `cancel: &CancelToken` (the
      provider needs it to interrupt its own request); update every `impl Provider`
      across agent/common/tui tests (mechanical). The `Tool::run` closure signature is
      untouched.
- [x] **Interruptible provider body read (`slimcode-ai`)** — replace the
      whole-body `resp.text()` in `BailianProvider::chat` with a chunked read through
      `std::io::Read` that checks the token between chunks and stops as soon as it is
      set (aborts an in-flight request: connection is dropped, `Err("request
      cancelled")` returned, which `run_loop` maps to Cancelled when the flag is set).
      Extract the read loop as a testable helper over any `Read`
      (e.g. `read_body_interruptibly(reader, cancel) -> ReadOutcome::{Complete(bytes), Cancelled}`)
      unit-tested against an in-memory reader with a scripted flag flip; keep the
      existing status-error handling and the client's whole-request timeout. Note in
      the docs: blocking reqwest has no per-read timeout, so a *silent* server is only
      bounded by the client timeout (300s) — cancellation reacts within one socket
      chunk for a flowing stream (verified by the tmux smoke with a slow-drip mock).
- [x] **Cancellable tool execution** — long-running tools must not swallow Esc:
      `bash` (the only unbounded tool) runs its `sh -c` via `Command::spawn`, polls
      `child.try_wait()` every ~50ms and checks the token, killing the child when
      cancelled and returning a marker result. Tool factories gain token-aware
      variants used by the TUI (`build_tools_with_cancel(cwd, token)` /
      `bash_tool_with_cancel(cwd, token)`), while the CLI one-shot keeps the plain
      `build_tools` (no token). Test: spawn `sh -c 'sleep 30'`, cancel, assert the
      tool returns quickly and the child is gone (no 30s wait). Other local tools are
      fast fs ops — the between-tool checks bound them.
- [x] **`slimcode-common` plumbing** — `runner::run_turn(provider, tools, messages,
      cfg, cancel, renderer)` threads the token into the agent sink; `setup.rs` gains
      the cancellable tool construction for the TUI. Runner tests: a scripted
      `FakeProvider` (two-turn script) with the token flipped mid-run asserts the
      stream stops and `Stop(Cancelled)` is emitted; cancel passed through untouched.
- [x] **TUI wiring** — `Effect::CancelRunning`; `App::handle_key_running`: bare `Esc`
      → `Some(Effect::CancelRunning)` (Ctrl+C/Ctrl+D stay `QuitAfterTurn`, everything
      else still ignored); pure transition tests updated (`running_keys_…`). The
      worker/UI loop (`drive_turn`): the `Tui` owns one `CancelToken`, `reset()` at the
      start of every `drive_turn`; on `Effect::CancelRunning` it calls `cancel()` and
      keeps polling/draining/joining as today (provider + tools always restored); the
      turn then ends with `StopReason::Cancelled`; spinner stops, input re-enables, no
      error text. `map_event`: `Stop(Cancelled)` renders nothing (like Completed — the
      partial transcript is the feedback; no red line).
- [x] **tmux smoke (Esc cancel e2e)** — extend `crates/cli/tests/tui_smoke.rs`: submit
      a prompt against a mock that drips chunks slowly; press `Esc`; poll-assert the
      spinner disappears and the input box is usable again while the mock is still
      mid-response; assert the turn's partial text is present and no error/red text
      appeared; then a fresh prompt runs normally (token reset verified).
- [x] Docs sync (AGENTS.md): reverse the spec/ADR "Esc does not cancel / provider
      non-cancellable" notes; spec Revision-2 decision + US wording (Esc cancels
      anywhere in the runner; Ctrl+C/D quit-after-turn unchanged); ADR-0006 D6 +
      Consequences updated (or a new ADR-0007 recorded) describing the
      `CancelToken`/chunked-read/cancellable-bash design and the `Provider::chat`
      widening; `docs/development.md` runner/provider/terminal sections;
      `docs/user-manual.md` key table (Esc cancels while running) and run-state bullet;
      `.scratch/tui-pi-alignment/test-map.md` rows.

## Notes

- pi ground truth: Esc aborts the in-flight request (AbortController-style) and stops
  the run; Ctrl+C stays quit-after-turn. Local tool runs in slimcode that cannot be
  preempted (non-bash) are bounded by between-step checks — document as the
  equivalent of "cancels at the next runner boundary".
- The `Provider::chat` signature change ripples through every fake provider in
  `slimcode-agent`, `slimcode-common`, and `crates/tui` tests — mechanical but must be
  in the same commit (crate compiles green at each commit boundary; consider a
  `CancelToken::none()`/uncancellable helper to keep one-shot call sites ergonomic).
- History consistency rule: a cancel never leaves a half-assistant message or a
  half-applied tool batch; check the flag before pushing the assistant message and
  before dispatching each tool; results already pushed stay.
