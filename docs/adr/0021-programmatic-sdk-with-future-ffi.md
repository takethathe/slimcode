# A programmatic SDK crate with a future FFI module and CLI thin-shell

slimcode's coding-agent capabilities — one agent turn, streamed events, the seven tools, session
persistence, skills, context files, auto-compaction — are only reachable through the `slimcode`
binary (one-shot text or the TUI). pi solves this by shipping an SDK: `pi-coding-agent` exposes
`createAgentSession()` / `AgentSession` (prompt / subscribe / abort / dispose), the CLI is a thin
shell over it, and a separate experimental protocol/client/server trio serves remote processes.
slimcode's `app` layer (ADR-0011 D2) already owns the frontend-agnostic services and the shared
`run_turn` seam; what it lacks is a *programmatic session object* a host program can drive.

We adopt pi's in-process SDK shape, adapted to slimcode's synchronous Rust architecture: a new
`slimcode-sdk` crate wraps the app layer behind a session-centric API (`create_agent_session` →
`AgentSession`), reuses `run_turn` (no second turn loop), and leaves the CLI untouched for now. The
FFI module and the CLI thin-shell refactor are recorded here as explicit follow-up work, not
shipped in this slice.

## Decisions

### D1 — `slimcode-sdk` is a new pure-Rust crate wrapping `app`; the FFI module is a separate follow-up cdylib

`slimcode-sdk` (rlib) depends on `slimcode-app` (→ `ai` / `core` / `commands`) and on no terminal
crate. Its whole external surface is `create_agent_session(AgentSessionOptions)` +
`AgentSession { prompt / subscribe / abort / dispose / messages / session_id / title / usage }`.
It mirrors the CLI's startup composition with the same app-layer helpers: four-layer config
resolution, `slimcode_home` / `resolve_project_home`, `SessionStore`, `load_context_files`,
`Environment`, `SkillStore`.

The FFI module is a *separate* crate (`slimcode-ffi`, cdylib, plain C ABI: opaque handle +
create/prompt/subscribe/abort/dispose + last-error) that depends on `slimcode-sdk`. A cdylib crate
cannot be depended on by other crates, so a single mixed rlib+cdylib crate would make the SDK
unusable from Rust; the split keeps the SDK pure and testable and the ABI surface mechanical.
The FFI module is deliberately **not implemented in this slice** — only the SDK crate is, so a
Node/Python host has a stable Rust API to bind against later.

### D2 — The SDK reuses `run_turn_with_hooks`; it does not own a second turn loop

`AgentSession::prompt` drives the existing `app::runner::run_turn_with_hooks` (the seam both the
one-shot CLI and the TUI share, ADR-0004), supplying an SDK-owned `Renderer` that forwards each
`DisplayItem` to subscribers as `SdkEvent::Display`, and an `on_message` sink that mirrors each
history entry into the session, emits `SdkEvent::Message`, and persists it (best-effort, like the
TUI). Turn-end handling mirrors `TuiSession::run_turn`: prompt-message append + title inference,
`Completed` → auto-compaction (`compaction::should_compact` + `compact_messages` + checkpoint
append, `SdkEvent::Compacted`), `Cancelled` / `Err` → closing message via `append_closing`
(`Aborted` / `Error`), so logs never end on a dangling tool batch (ADR-0009 D3). The skill-read
interception (a `read` of a skill's own `SKILL.md` served as the skill block) rides the same
`RunHooks::before_tool` seam the CLI uses.

### D3 — Sessions are persistent by default, with an in-memory mode

`AgentSessionOptions.persist` defaults to `true` and routes through the existing `SessionStore`
(project-partitioned append-only JSONL, quota eviction, `/session`-picker compatible). `false`
builds no store at all: messages / title / usage live in memory and no file is written. The id
scheme (`slimcode-<unix>-<pid>-<n>`) moves from `SessionStore::new_id` into a shared free
function so both modes generate identical ids. The turn orchestration is one code path — only the
persistence point branches.

### D4 — `prompt` is synchronous and single-flight; `abort` is cross-thread

`prompt(&self, text) -> Result<StopReason, String>` runs the whole turn on the calling thread
(slimcode's `Provider` seam is synchronous; no async runtime is introduced). Internal state mirrors
the TUI's take/put pattern (ADR-0013 D2): the provider/session/tools live behind a mutex, are
taken out for the turn, and `abort(&self)` touches only the shared `CancelToken`, so it never
waits on the lock. A second `prompt` while one runs errors ("a turn is already running"). Events
are delivered synchronously to listeners in subscription order on the turn's thread; a consumer
needing another thread's delivery bridges it itself. `dispose` marks the session dead and clears
listeners.

### D5 — The SDK does not interpret `/` commands or own input history

`prompt` treats its text as plain user input (the CLI's one-shot already normalizes a leading
`/skill:name` trigger; the SDK leaves that to the consumer). Command semantics and input history
stay frontend concerns (ADR-0013 D3). Multi-session management (pi's `AgentSessionRuntime`
new/switch/fork/import) is out of scope for this slice: a consumer creates a new `AgentSession`
for a new conversation.

## Considered Options

- **Remote protocol trio first (pi-protocol/client/server in Rust)** — rejected: pi labels those
  packages experimental with no compatibility guarantees, and slimcode has no identified
  cross-process consumer yet; the in-process SDK is the official pi SDK shape and the natural
  first step. The FFI module covers the "drive from another language" need later.
- **One mixed rlib+cdylib crate** — rejected: a cdylib crate cannot be a dependency, so Rust
  consumers could not `use` the SDK; the split keeps both audiences (D1).
- **Rebuild the CLI as the SDK's consumer now (pi's thin-shell)** — rejected for this slice: it is
  a large refactor with no behavioral change; it is recorded as follow-up work so the SDK lands
  and stabilizes first.
- **A new async/polling event API** — rejected: slimcode is synchronous end-to-end; synchronous
  listener delivery is zero-cost and testable, and the future FFI layer handles thread marshalling
  on the host side.

## Consequences

- `crates/cli/tests/architecture.rs` (ADR-0011's dependency matrix test) gains a row:
  `slimcode-sdk` → `slimcode-app` (→ `ai`/`core`/`commands`).
- `docs/development.md`'s crate table and `docs/index.md` gain the SDK entry; the spec lives at
  `.scratch/sdk/spec.md` (`Status: ready-for-agent`).
- Follow-up work (not this slice): `slimcode-ffi` (cdylib, C ABI, opaque handle, event
  wait+drain/callback) for Node/Python thin bindings; the CLI thin-shell refactor (one-shot + TUI
  orchestration converging on `slimcode-sdk`).
- No behavioral change to existing crates: the only public-surface change is additive —
  `crates/app/src/lib.rs` re-exports the core/ai types the SDK's public API names, and
  `crates/sdk` re-exports the app types its API names — while the CLI, TUI and session format
  are untouched by this slice.
