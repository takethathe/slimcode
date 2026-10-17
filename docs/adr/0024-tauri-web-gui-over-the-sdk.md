# A Tauri web GUI over the SDK: the GUI 前端 as slimcode's third frontend

slimcode has two entry surfaces (ADR-0022): the `slimcode` binary (one-shot + TUI) and the
programmatic `slimcode-sdk`. The SDK's synchronous, session-centric shape (ADR-0021 D4) and the FFI's
wire-enum precedent (ADR-0023 D3) already settled the hard parts. This slice adds a **third frontend**:
a desktop GUI. The user wants a web-technology UI with a light, modern look, built on Tauri and
driven by the SDK. Design was sharpened by interview; this ADR records the decisions and
`.scratch/gui/spec.md` is the implementable spec.

The shape: a Tauri v2 app whose Rust backend embeds `slimcode-sdk` directly — no FFI, no subprocess,
no second turn loop. It owns one current `AgentSession` (single active session), runs each `prompt`
on a worker thread (the SDK is synchronous), and bridges `SdkEvent`s to the webview as **GUI wire
事件** (a GUI-owned flat JSON enum, mirroring the FFI's wire enum). The frontend is React + Vite +
Tailwind, hand-rolled components, light slate + indigo aesthetic. The project lives outside the
cargo workspace at `gui/` so the workspace gates (fmt / clippy / `cargo test`, the
`architecture.rs` dependency matrix) keep covering only the agent core.

## Decisions

### D1 — Tauri v2, Rust backend embeds `slimcode-sdk` directly; never the FFI or a subprocess

The GUI's backend is the **Tauri 宿主**: a Tauri v2 Rust app that depends on `slimcode-sdk` (path
dependency into the workspace) plus the app layer's `SessionStore` for the session picker list
(ADR-0022 D1's precedent: the picker is a frontend concern and reads the store directly). The FFI
(C ABI, ADR-0023) exists for non-Rust hosts — Node/Python — and a Tauri backend *is* Rust, so
routing through it would be pure overhead. Spawning the `slimcode` binary and parsing terminal
output is rejected: it yields no structured events, no cancellation, no session state — the exact
gap ADR-0021/0023 closed.

The 宿主 owns the session registry (one current `AgentSession`), the worker thread that runs
`prompt`, the wire bridge, and GUI settings. It depends on `slimcode-sdk` and `slimcode-app` (for
`SessionStore`, `config`, `render` types) and on no terminal crate — same layering rule the SDK
itself follows (ADR-0021 D1, ADR-0011 D4).

### D2 — Frontend is React + Vite + Tailwind, hand-rolled components, light modern

Web-tech UI rendered in the Tauri webview: React (typed, ubiquitous, best Tauri/Vite support) +
Vite (build tooling, HMR) + Tailwind CSS. Components are hand-rolled, not a component library —
the coding-agent transcript UI is highly custom (streaming text, tool cards, reasoning folding,
status colors), so a library like shadcn/ui would be more friction than speed. The design language
is light and modern: white/light-gray background on Tailwind's `slate` scale, a single indigo
accent, hairline borders, restrained 6–8px radii and soft shadows, system UI font (SF Pro / Inter)
with a monospace (JetBrains Mono) for code and transcript text, and semantic status colors
(success green / failure red / running accent). TUI's `Theme` (ADR-0006, dark-only) is not reused;
the GUI owns its own light palette.

### D3 — `gui/` is a self-contained project outside the cargo workspace

A new top-level `gui/` directory holds `gui/src-tauri` (the Tauri 宿主, its own `Cargo.toml` +
`Cargo.lock`) and `gui/ui` (the React/Vite frontend, its own `package.json`). It is **not** a
workspace member. Tauri drags in hundreds of dependencies; making it a member would put all of them
under the workspace gates (`cargo fmt` / `clippy -D warnings` / `cargo test`) and the
`crates/cli/tests/architecture.rs` dependency matrix would need a row that has nothing to do with
the agent core. The GUI instead has its own local gates (`cargo fmt` / `clippy` / `test` inside
`gui/src-tauri`) and path-deps into the workspace crates. `docs/development.md`'s crate table gains
a GUI section; the workspace's own docs and the ADR index list it as a separate product surface.

### D4 — Single active session; `prompt` on a worker thread; events bridged as GUI wire 事件 over a Tauri `Channel`

v1 has one current `AgentSession` at a time, matching the TUI's "current session" mental model
(ADR-0013 D2). `prompt` is synchronous and single-flight (ADR-0021 D4), so the 宿主 runs it on a
worker thread spawned per turn; the UI thread stays responsive and `cancel` only calls
`abort()` (cross-thread, touches only the shared `CancelToken`, kills a running `bash` child,
ADR-0022 D3) — never waiting on the session lock.

Events arrive synchronously on the turn thread as `SdkEvent`. Because `SdkEvent` / `DisplayItem`
cannot derive `Serialize` (the exact problem ADR-0023 D3 recorded), the 宿主 owns a **GUI wire
事件** enum — a flat `{"kind": …}` JSON shape — produced by an exhaustive match over `SdkEvent`
(no `_` arm), delivered per `prompt` over a Tauri v2 `Channel<GuiWireEvent>`. The frontend never
sees an `SdkEvent`. The wire enum is the GUI's own (in `gui/src-tauri`), not the FFI's
`WireEvent`, because the two hosts have different payload needs; the mapping code is a sibling, not
a shared import.

### D5 — Settings live in `gui.toml` (SDK overrides), never `config.toml`; api_key plaintext 0600 in v1

The GUI persists its settings in `gui.toml` under slimcode_home: `model` / `base_url` / `cache`,
and optionally `api_key`. At startup the 宿主 reads it and passes the values to every
`AgentSession` as the highest-precedence SDK overrides (ADR-0021's four-layer resolution:
overrides > env > `config.toml` > defaults). It deliberately never writes `config.toml` — that file
is the shared third layer the CLI/TUI/GUI all read, so GUI writes would silently change CLI/TUI
behavior. `api_key` follows the repo precedent (`API key / credential`, GLOSSARY): plaintext on
disk written with 0600 permissions; macOS Keychain is an explicit v2 enhancement, not v1.

### D6 — Shared session store, launch to a fresh session, picker in the left sidebar

The GUI, CLI and TUI share one session repository — the same slimcode_home JSONL logs through the
same `SessionStore` (ADR-0008/0009) — so sessions created in the CLI/TUI are resumable in the GUI
and vice versa. On launch the app opens a fresh empty session (transcript empty, focus in the
input); historical sessions are listed in the left sidebar (title + time) and resumed via
`with_session_id` (ADR-0022 D3). Deleting a session removes its JSONL log (with confirmation).
The current session's title, model name, and context-window usage (the `ContextUsage` concept,
ADR-0020) render in the top bar.

### D7 — Rust-side unit tests with FakeProvider injection; frontend manual acceptance

The 宿主's session registry, wire mapping, and command-layer state machine are covered by Rust
unit/integration tests using the SDK's injection seam (`with_provider(FakeProvider)` +
`with_tools` + `with_home(scratch)`), the same pattern `crates/sdk/tests/` uses — so the tests are
offline, key-free, and exercise the real public API. The React frontend is verified by manual
acceptance in v1 (transcript UI automation has low value for the effort); no Vitest in this slice.
The Rust tests run under the `gui/` project's own gates, not the workspace's.

### D8 — Naming: `slimcode-gui`, display name "slimcode GUI"

The Rust crate and the Tauri identifier are `slimcode-gui`; the window/bundle display name is
"slimcode GUI" — following the `slimcode-sdk` / `slimcode-ffi` naming family. The GUI is not a
rebrand; it is the desktop face of slimcode.

## Considered Options

- **Frontend through the FFI (Node/Python UI) or spawning the binary** — rejected: the Tauri
  backend is Rust, so the C ABI is dead weight, and subprocess parsing loses structured events /
  cancellation / session state (D1).
- **Vue / Svelte / vanilla TS instead of React** — rejected: React + Vite is the most common,
  best-supported Tauri pairing and matches the team's ecosystem; the choice is deliberately
  conventional, not novel.
- **A component library (shadcn/ui, Mantine)** — rejected: the transcript UI is custom enough
  (D2) that a library's conventions would fight it; hand-rolled Tailwind components are simpler.
- **`crates/gui` as a workspace member** — rejected: Tauri's dependency tree under the workspace
  gates, and a pointless `architecture.rs` row, with no benefit to the agent core (D3).
- **Concurrent multi-session tabs in v1** — rejected: the TUI's single-current-session model is
  the proven shape; the session registry is designed so tabs can be layered on in v2.
- **Global `tauri::Emitter` events instead of per-prompt `Channel`** — rejected: a `Channel` is
  scoped to one `prompt` stream and naturally rebuilds the transcript; a global bus would need
  per-session routing state on both sides for no capability (D4).
- **GUI writes `config.toml`** — rejected: shared-state conflict with the CLI/TUI (D5).
- **macOS Keychain in v1** — rejected: adds a dependency and an async path for little v1 value;
  plaintext + 0600 matches the existing credential precedent, Keychain is tracked for v2 (D5).
- **Frontend Vitest in v1** — rejected: transcript UI automation costs more than it returns at
  this stage; Rust tests cover the logic that matters (D7).

## Consequences

- A third frontend (GUI 前端) joins the CLI and TUI; GLOSSARY's `Frontend` definition generalizes
  to "one of slimcode's user-facing entry surfaces" and new terms land: `GUI 前端`, `Tauri 宿主`,
  `gui.toml`, `GUI wire 事件`.
- `gui/` is a new top-level project outside the workspace with its own gates; the workspace's
  `cargo test` / clippy / `architecture.rs` matrix are untouched. `docs/development.md` gains the
  GUI section; `docs/index.md` lists the ADR and spec.
- The GUI shares the session store with the CLI/TUI (D6) but never writes `config.toml` (D5) —
  settings are one-directional into the SDK, so the GUI cannot disturb other frontends.
- The `architecture.rs` matrix is unchanged: `slimcode-gui` is not a workspace member, so there is
  no new row to enforce (D3).
- No change to the SDK, app, CLI, TUI or session log format: the GUI is the SDK's third consumer
  face (after the CLI thin shell and the FFI), not a new orchestrator. slimcode stays synchronous.
- Follow-up work (not this slice): macOS Keychain for api_key; concurrent session tabs; a Windows /
  Linux build pass; frontend component tests; `subscribe`-style callback delivery if the frontend
  ever needs push outside a turn.
