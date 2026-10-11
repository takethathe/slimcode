# A plain C ABI over the SDK, with a queue-delivered JSON event stream

ADR-0021 D1 recorded the FFI module as follow-up work: after the SDK landed (ADR-0021) and the CLI
became a thin shell over it (ADR-0022), non-Rust hosts still had no entry point. A Node or Python
program could only spawn the `slimcode` binary and parse its terminal output, which yields no
structured events, no cancellation control and no session state. ADR-0021 D1 already fixed the
shape — a *separate* `slimcode-ffi` crate (cdylib, plain C ABI, opaque handle, create / prompt /
abort / dispose / last-error) depending on `slimcode-sdk`. This slice (`.scratch/ffi/spec.md`)
implements it.

The substrate ADR-0021 promised is stable: `AgentSession` is synchronous and single-flight, and
events are delivered synchronously on the turn's calling thread (ADR-0021 D4). The FFI layer is
therefore mechanical: translate one options object into `AgentSessionOptions`, subscribe once per
handle, and expose the session's operations as 13 C symbols whose whole contract is the handwritten
`crates/ffi/include/slimcode.h`. The acceptance evidence is a zero-dependency Node demo
(`crates/ffi/demo/`) that `dlopen`s the cdylib with the built-in `node:ffi` and drives a real tool
turn against a local fake endpoint.

D1–D4 record the four trades this layer fixes: crate types, event delivery, payload encoding, and
the persistence default plus the v1 boundary.

## Decisions

### D1 — The crate is both `cdylib` and `rlib`; `ffi` depends on `sdk` and nothing else

`crates/ffi` (`slimcode-ffi`) builds `crate-type = ["cdylib", "rlib"]` and depends only on
`slimcode-sdk` (plus `serde` / `serde_json` for the payloads) — no `app`, `ai`, `core`,
`commands`, `cli` or `tui`, not even from a `[dev-dependencies]` table.
`crates/cli/tests/architecture.rs` asserts that row (`ffi_depends_on_sdk_only`), extending the
dependency matrix ADR-0011 D4 pinned.

The `cdylib` is the shipped artifact a host `dlopen`s (`libslimcode_ffi.dylib` / `.so` / `.dll`);
the `rlib` is what makes the ABI testable. Evidence: with `crate-type = ["cdylib"]` alone, the
integration tests cannot resolve `use slimcode_ffi::…` at all — the compiler fails with E0433
(unresolved import), because a cdylib is not linkable as a Rust dependency. With both types, the
tests call the exported `extern "C"` symbols directly, which is the whole test seam: no test-only
entry point leaks into `sdk` or `app`.

This does not contradict ADR-0021 D1's rejection of "one mixed rlib+cdylib crate": that decision
kept the *SDK* pure by splitting it from the FFI module, so a Rust consumer can still `use`
`slimcode-sdk`. Here the mixed crate is the leaf, and the rlib only exists for the tests.

### D2 — Events go through a per-handle queue drained by a blocking `next_event`, not callbacks

At `slimcode_session_create` the FFI installs one SDK listener per handle. The listener encodes
each `SdkEvent` and pushes it into that handle's queue — a `Mutex<VecDeque>` plus a `Condvar` —
synchronously on the turn's thread. The host takes events with
`slimcode_session_next_event(handle, timeout_ms)`: `< 0` waits forever, `0` polls, `> 0` waits at
most that long. A real-time host runs `slimcode_session_prompt` (which blocks its caller) on its
own worker thread and pumps the queue on its UI thread; `crates/ffi/demo/node/demo.mjs` is exactly
that shape.

The reason is the callback constraint on the Node side. `node:ffi`'s `registerCallback` documents
that the callback must be invoked on the **same system thread that created it**, while slimcode
delivers events on the turn's thread — in the FFI's target shape, a worker thread the host created
for `prompt`. A callback-based `subscribe` would therefore require thread affinity the host cannot
guarantee; koffi's and ctypes' behaviour on that path was not established, and a C ABI for any host
language must not assume one runtime's callback semantics. A queue is neutral: it works in any
language and thread model, needs no host-side trampoline, and keeps the C side from ever calling
back into the host language.

The costs are explicit. The host owns the pump loop; the queue is unbounded in v1 (events are
never dropped or merged, so a host must drain it while a turn runs); and a callback path is left
for later. A `subscribe` symbol is purely additive — the header's version policy bumps ABI version
1 only for an incompatible signature, return-code or ownership change — so the queue does not close
that door.

### D3 — JSON payloads, and the flat wire enum is owned by the FFI

Everything that crosses the ABI is a UTF-8 JSON string except `prompt`'s `int32_t` stop code.
Options arrive as one JSON object (`deny_unknown_fields`, so a mistyped `api_key` fails loudly
instead of silently falling through to `config.toml`); `session_id` / `title` / `usage_json` /
`messages_json` / `next_event` / `compact_now` return JSON strings the host releases with
`slimcode_free_string`. A `struct_size`-carrying C struct was rejected for options: a new field
would need a new struct version and field-order discipline, while a JSON object gains fields
without an ABI break and is native to construct in Node or Python.

Events are **not** the SDK's `Serialize` output. `crates/ffi/src/wire.rs` owns a flat
`{"kind":…}` enum and maps `SdkEvent` / `DisplayItem` onto it with an exhaustive match (no `_`
arm). Two pieces of evidence:

- serde's internally-tagged representation cannot carry `DisplayItem::Text(String)`-style
  newtype-in-primitive variants — serializing `#[serde(tag = "kind")]` over a newtype variant that
  wraps a primitive fails at runtime — so deriving `Serialize` on `DisplayItem` / `SdkEvent`
  directly is not an option.
- the flat shape is the one a host consumes (`switch (event.kind)`), and the exhaustive match makes
  a new `DisplayItem` variant a **compile error** in the FFI crate, forcing the wire contract and
  the header to be updated together instead of silently dropping an event.

Reusing the SDK's own serde shapes where the spec pinned them keeps the decoder count down:
`message` carries the persisted session-log shape (`AgentMessage`, tag `kind`: `llm` /
`compact_summary`), so a host shares one parser across the queue, `messages_json` and the JSONL
session files. That needed three additive derives: `TokenUsage` and `PromptTokensDetails` (`ai`,
which had `Deserialize` only) and `StopReason` (`core`, plus `rename_all = "snake_case"` so stop
kinds read `completed` / `cancelled`). `DisplayItem` deliberately gains no derive — the FFI-owned
enum is the contract, not the SDK type.

### D4 — `persist` defaults to `true`; v1 boundaries stay narrow

The options object is `cwd` / `persist` / `session_id` / `base_url` / `model` / `api_key` /
`cache` / `home`, each optional and falling through the SDK's four-layer resolution otherwise.
`persist` absent means `true`, exactly as in ADR-0021 D3 and the CLI: a host that writes no
options gets CLI-equivalent behaviour, `persist: false` with a pinned `home` gives a zero-file
session, and `session_id` together with `persist: false` is an error (a resume without a store
would be a silent lie).

The rest of the contract is the SDK's, restated in C terms: failures are `NULL` / `-1` plus a
thread-local `slimcode_last_error()` (library-owned, never freed by the host: every fallible entry
point clears the previous message on entry and sets this one on failure, `abi_version` / `abort` /
`dispose` / `last_error` / `free_string` never touch it, so the pointer stays valid until the next
call that updates it on the same thread); every returned string is library-owned and released
exactly once with `slimcode_free_string`; `Cancelled` is a return code (`1`) and not an error;
every fallible symbol body runs under `catch_unwind`, so a Rust panic surfaces as an error code
instead of unwinding into a foreign frame (`slimcode_abi_version` only loads a constant, so it
needs no guard); `abort` and `dispose` are callable from any thread, `abort` touches only the shared
`CancelToken` and never waits on the session lock (the cancel boundary ADR-0019 D3 pins — the
provider drops the torn SSE tail), and `dispose` closes the queue, waking blocked pumps with `NULL`
and invalidating the pointer.

v1 deliberately does **not** include:

- **provider / tools / skills injection.** Rust trait objects and `Tool` / `Skill` values cannot
  cross a C ABI; the one external dependency a host can replace is the HTTP endpoint (`base_url`).
  Tests and the demo therefore fake the *endpoint* rather than the provider, so the artifact under
  test is the shipped artifact.
- **async `prompt` or an FFI-owned turn thread.** slimcode is synchronous end-to-end (ADR-0021 D4);
  a host that wants a live UI already owns the thread that must pump the queue, so its worker is the
  natural turn thread. The ABI stays free of a second threading model.
- **multi-session management.** One handle is one session (ADR-0021 D5); a host creates several
  handles for several sessions, each with its own queue and log.
- **a bounded queue or a drop/coalesce policy.** Silently dropping events is worse than unbounded
  growth, and coalescing is a rendering policy the host owns; the queue is unbounded and the host
  drains it while a turn runs.
- **multiplexing across handles.** One pump thread per session (or `next_event(handle, 0)`
  polling); `next_event_any` is future work.

`include/slimcode.h` is the written-once contract for all of this — the 13 symbols and their
return / ownership / threading semantics, the options and event JSON, the handle lifetime and the
ABI version policy — so a binding author reads one file.
`crates/ffi/tests/abi_contract.rs::the_abi_is_exactly_the_spec_symbol_set` pins it: the header, the
ABI list recorded in the test's `ABI_SYMBOLS` (transcribed from the spec's symbol table) and the
`slimcode_*` symbols defined in `src/**/*.rs` must be the same set (ticket 01 could assert only the
forward direction while tickets 02–06 were still landing).

## Considered Options

- **A callback-based `subscribe` symbol** — rejected for v1: `node:ffi`'s same-system-thread
  callback rule collides with turn-thread delivery, and koffi / ctypes cross-thread behaviour is
  unproven (D2). The queue is the language-neutral path; `subscribe` remains an additive follow-up.
- **Serializing `SdkEvent` / `DisplayItem` directly instead of an FFI-owned wire enum** — rejected:
  serde's internally-tagged representation cannot carry newtype-in-primitive variants such as
  `DisplayItem::Text(String)`, and the SDK's shape is not what a host switches on. The FFI-owned
  enum also makes a new display variant break the build (D3).
- **Options as a `struct_size`-carrying C struct** — rejected: a new option would be a new struct
  version, while a JSON object is ABI-stable across additions and is native in the host languages
  (D3).
- **Async `prompt`, or an FFI-managed turn thread and async event delivery** — rejected: the SDK is
  synchronous (ADR-0021 D4), and the host already has the worker thread the pump loop needs; an
  async ABI would add a second threading model for no capability (D4).
- **Exposing providers / tools / skills injection through serialized descriptions** — rejected:
  tools have execution closures and skills have parsed file trees, so a real injection seam would
  need a serializeable tool protocol nobody has asked for. Faking the endpoint keeps the seam
  honest (D4).
- **A bounded queue with a drop counter or coalescing** — rejected for v1: dropping data silently
  is a worse failure mode than unbounded growth, and the host can merge events itself (D4).

## Consequences

- The workspace gains a leaf crate: `cdylib` for hosts, `rlib` for the integration tests, and the
  `ffi → 仅 sdk` row is enforced by `crates/cli/tests/architecture.rs` (ADR-0011 D4's matrix, with
  the FFI-specific comment).
- Three additive `Serialize` derives land in `ai` (`TokenUsage`, `PromptTokensDetails`) and `core`
  (`StopReason`, snake_case); no behaviour in those crates changes, and `DisplayItem` gains no
  derive.
- `docs/development.md` gains the FFI section (symbol table, options / events JSON, ownership and
  threading semantics, crate layering, how to run the demo); `docs/index.md` lists this ADR;
  `CONTEXT.md`'s `FFI 模块` and `FFI 事件队列` entries describe the landed module.
- Acceptance evidence is one command, `bash crates/ffi/demo/run.sh`: the ABI smoke (ticket 01's
  10 checks) and the acceptance demo (39 checks) both print `PASS`. The Rust integration tests own
  the turn-path coverage — including `compact_now` and the failure contracts — against a
  stdlib-only mock SSE endpoint, so `cargo test` stays offline and key-free; the Node demo
  (Node ≥ 26 with the built-in `node:ffi`, no npm and no koffi in code) owns the cross-language,
  cross-thread evidence: a worker-thread `prompt` pumped by the main thread, a cross-thread
  `abort`, the read-only accessors, and the `persist: false` / JSONL / `session_id` resume paths.
- No change to the CLI, the TUI or the session log format: the FFI is the SDK's second consumer
  face (after the CLI thin shell, ADR-0022), not a new orchestrator. slimcode stays synchronous —
  no async runtime, no second turn loop.
- Follow-up work (not this slice): a `subscribe` callback symbol; Python `ctypes` / `cffi`
  bindings and a demo; koffi adapters and npm packaging; a bounded queue / drop-count event;
  `next_event_any` multiplexing.