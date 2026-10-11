# CLI thin-shell over the SDK: dependency reversal, SDK face extensions, accepted regressions

ADR-0021 shipped `slimcode-sdk` and recorded the CLI thin-shell refactor as follow-up work. It
landed here. Before this slice the turn orchestration existed in **three** copies: the SDK's
`AgentSession::prompt` (ADR-0021 D2), the one-shot `run_once`, and the TUI's
`TuiSession::run_turn` / `run_compaction` / `close_turn` — the last two driving
`app::runner::run_turn_with_hooks` directly. Every orchestration behaviour (closing-message
wording, the compaction threshold, the skill read interception, persistence points) had to be
changed in three places, and behaviour drift was the norm rather than the exception.

We invert the dependency: `cli` now consumes `slimcode-sdk` for both modes, first for the one-shot
turn (`persist: false`, ADR-0009 D5) and then for the TUI's current session object. The CLI keeps
only what is genuinely frontend work — argv, mode selection, the terminal, `/` command semantics,
input history, the session picker list, the display adapters (`TextRenderer` / `TuiAdapter`), and
session-object management (creating, switching, disposing an `AgentSession`). The SDK face grew the
four capabilities the CLI needed (`with_home`, `with_session_id`, `compact_now`, cancellable
default tools). External behaviour is unchanged except three rare-path regressions (D5), which are
accepted and documented here.

## Decisions

### D1 — The dependency direction is reversed: `cli` → {ai, core, app, commands, tui, sdk}

The CLI no longer reaches the runtime through `app::runner::run_turn_with_hooks`; it drives an
`AgentSession`. `crates/cli/tests/architecture.rs` asserts the new matrix row (`cli` depends on
every crate, `sdk` included), so the boundary is locked by a test instead of convention. The
one-shot builds a `persist: false` session per run and subscribes a `Display`-only listener into
the `TextRenderer`; the skill-trigger normalization (a leading `/skill:name` or `/{name}`) stays in
the CLI, because it is frontend semantics.

`app` remains a CLI dependency: the picker list (`SessionStore`), `/` command and skill completion
(`SkillStore`), config resolution and the display mapping are frontend concerns the SDK does not
own (ADR-0013 D3). A thin shell over the SDK is not a CLI without `app`.

### D2 — In the TUI, an `AgentSession` *is* the current session object

`TuiSession` holds `Inner { session: Arc<AgentSession>, adapter: Arc<Mutex<TuiAdapter>> }`.
`submit` (worker thread) clones the `Arc` under a short lock, releases the lock, and calls
`prompt`; the lock is never held across a turn. `cancel` (frame thread) only calls `abort()`, so it
never waits on the session lock (ADR-0021 D4). `/new` and `/session` loading replace the session
only while the worker is idle (the library ignores ordinary keys during a turn) and dispose the old
one, which also clears its listeners. Skills are passed in as the CLI's snapshot; the SDK assembles
the store, provider, tools, context files and environment.

SDK events arrive synchronously on the prompt thread, but the TUI library's `emit` callback is only
valid during `submit`. The bridge is a **thread-local emit slot**: `submit` installs the slot, the
SDK listener reads it and calls `emit`, and the slot is cleared before returning. The frame thread
never touches the slot during a turn, so the bridge adds no contention and no async runtime.

### D3 — The SDK face grows exactly four capabilities

- `AgentSessionOptions::with_home(PathBuf)` pins slimcode's home (session store root, skills and
  context-file discovery, `Environment.global_home`). Absent, the default stays
  `config::slimcode_home()`; the CLI passes its resolved home so no second env lookup happens.
- `AgentSessionOptions::with_session_id(String)` resumes a persisted session (lenient replay of
  messages and title; usage starts from zero because it is not persisted) and errors when combined
  with `persist: false` — a load without a store would be a silent lie.
- `AgentSession::compact_now()` forces a compaction outside a turn (single-flight with `prompt`),
  emits `SdkEvent::Compacted { before, after }`, returns `Result<(usize, usize), String>` with the
  provider error detail on failure and leaves the history untouched, and folds the compaction's
  token cost into `session.usage` (ADR-0018 D4, ADR-0020).
- The default seven tools are built with `build_tools_with_cancel(cwd, &session_token)`: the tools
  hold the session's own `CancelToken`, so `abort()` / `dispose()` also kill a running `bash`
  child. Injected tools (`with_tools`) are unaffected.

### D4 — Frontend semantics do not move into the SDK

`/` command parsing and execution, input-history recording (`Prompt.record`), the session picker
list, the display adapters, and the one-shot's skill-trigger normalization stay in the CLI
(ADR-0013 D3). The SDK treats `prompt` text as plain user input. The display contract
(`DisplayItem` / `map_event` / `usage_summary` / `run_turn_with_hooks`) stays in `app`, so both CLI
modes and any future frontend keep one mapping.

### D5 — Three accepted regressions and one frozen value

- The `/session` load's repair-count notices (`skipped N unreadable record(s)` / `repaired N
  interrupted tool call(s)`) are gone: the SDK's replay is lenient and silent, and the notices only
  appeared for corrupted or torn logs. Title, history replay, branch and zeroed usage are
  unchanged.
- The old CLI printed a `session log: {e}` notice whenever a session-log write failed — a message
  append, the title append, the closing message, or the compaction checkpoint. The SDK's appends
  are best-effort and swallow the error silently, so a failed write now shows up only as a missing
  or truncated log. History, title, branch and zeroed usage are unaffected.
- Skills are frozen when the session is created. After `/install-skill`, the newly installed skill
  is advertised and read-intercepted only in the next session (`/new` or a restart); the CLI's
  `/skills` list and `/` completion update immediately, since they read the CLI's own `SkillStore`.
- Config is frozen at its startup-resolved values: the CLI passes its resolved base URL / model /
  API key / cache as the SDK's highest-precedence overrides, so `/new` reproduces the footer's
  model instead of silently re-reading `config.toml` (ADR-0016). This matches the previous
  behaviour — a property, not a regression. `sessions_max_mb` is not part of the overrides; a
  mid-run edit leaves the SDK store and the CLI picker list briefly disagreeing on quota, which is
  accepted as best-effort.

## Considered Options

- **Keep the CLI driving `run_turn_with_hooks` and delete the SDK's duplicate** — rejected: the SDK
  is the programmatic surface ADR-0021 committed to, and the TUI needs the session object anyway
  (subscribe / abort / dispose / compact-now).
- **Push more into the SDK (command semantics, input history, the picker list), pi-style** —
  rejected: `/` commands and input history are frontend concerns (ADR-0013 D3); a picker list is a
  TUI view over the CLI's own store.
- **Keep `app::setup` as a compatibility seam for hosts** — rejected: with both modes consuming the
  SDK it had no consumers left; the SDK's `create_agent_session` is the seam now.
- **An async SDK or a per-turn callback API** — rejected: slimcode is synchronous end-to-end; the
  thread-local emit slot bridges the TUI without a runtime or extra threads (D2).

## Consequences

- `crates/cli/tests/architecture.rs` flips its CLI row to include `slimcode-sdk` (the test is the
  enforcement point for D1).
- `crates/app/src/setup.rs` (`setup` / `setup_with_cancel`) is deleted, along with its module
  declaration and docs; the stale mentions in ADR-0011 D2 and ADR-0016 now point here.
- CLI handler tests drive the real structure: they build sessions through
  `AgentSessionOptions::with_home(scratch)` + `with_provider(FakeProvider)` + `with_tools`, so the
  tests exercise the same public API a host uses.
- SDK orchestration tests (`crates/sdk/tests/`) own the turn-behaviour coverage — one-shot
  persistence rules, compaction, resume, cancellation — and the CLI keeps the frontend coverage
  (command semantics, rendering, picker, log files).
- The four behaviour notes in D5 are the only user-visible deltas; the one-shot still never
  persists, `Esc` still kills a running `bash`, `/compact` and auto-compaction still render
  identically, and loading a session still replays history and title.
