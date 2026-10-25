# A portable Engine: a seven-phase hook pipeline over the sync loop, with an async facade

slimcode's SDK (ADR-0021) is a *frontend embedding surface*: it deliberately welds the product
decisions — context assembly, JSONL session persistence, turn-end compaction, the seven tools, the
fixed `RunConfig::default()` — into `AgentSession::prompt`, and exposes only three run hooks
(`before_tool` / `after_tool` / `turn_end`) that never reach the SDK surface. An investigation into
replacing sibling project **slimbot**'s agent loop with this SDK concluded it cannot be done without
losing slimbot's must-haves (dynamic context, a seven-phase hook pipeline, self-built trace spans,
iteration/timeout guardrails, consolidation memory). The gap is not a missing parameter: the SDK is
a whole-product embed, while slimbot needs a *pluggable loop engine* — the two point in opposite
directions under ADR-0021. Rather than fork or force a fit, we promote slimcode's loop into a
general **Engine** that serves both a coding agent and a bot agent, with everything except the loop
skeleton living on the public seams. Product behaviors attach to the seam that fits each (a phase
hook, an operand, the turn-end component, the message sink), and a host overrides them at those
seams rather than through privileged internals. This slice only *prepares the Engine inside
slimcode*; slimbot adopts it later by taking a dependency on the SDK.

## Decisions

### D1 — The loop skeleton is the one irreducible core; everything else is a seam

The agent loop itself stays the single, non-replaceable kernel (one loop, frontends cannot drift —
the ADR-0004 principle extended). Around it sit three kinds of extension surface:

- **Operand** — what the skeleton calls to make progress: `Provider` and `Tool` execution, plus its
  inputs (message history, cancel token, run config). Operands are injected by value and are
  deliberately *not* hooks; forcing them through the hook bus would reduce the kernel to an empty
  `for` loop.
- **Hook** — cross-cutting interception at loop boundaries that may rewrite the `AgentMessage` about
  to enter history, veto/skip a call, or control flow.
- **Event** — read-only observation (streamed deltas, per-message entries, turn finish, compaction,
  span start/end).

The current three hooks are generalized into the seven-phase pipeline of D2. The behaviors today
hardcoded in `prompt` spread across the seam that fits each: cross-cutting veto/rewrite as phase
hooks (skill interception on `pre_tool`, rollback on `after_turn`), context assembly and persistence
as skeleton-and-operand responsibilities a host may still rewrite through a hook, compaction as a
turn-end component, and event firing on the bus. None of them sits in a privileged internal path; a
host's hook and slimcode's default are the same kind of thing at the same seam.

### D2 — Lift slimbot's seven-phase pipeline into the shared loop kernel as the public seam

`core::AgentRunner` grows from three hooks to slimbot's already-proven phase pipeline —
`pre_turn / pre_chat / after_chat / pre_tool / after_tool / post_iteration / after_turn` — threading
an immutable collaborator bundle and a mutable run state with a cross-phase typed extension map, and
returning flow control (`HookAction{Continue,Abort}`; `ToolHookAction{Execute,Skip,Abort}`;
`after_turn` always runs on Completed / Cancelled / a guardrail stop / Abort, but not on the errored
path, where the run returns `Err` instead). The skill-read interception moves onto `pre_tool`'s
`Skip`. We adopt slimbot's shape rather than invent a neutral one because it is validated, it is
exactly the "defaults-not-privileged" exemplar, and code-type simply registers fewer default hooks.
The kernel is still one loop (in `core`), not a second implementation.

### D3 — Async is a facade over the sync kernel, not an async kernel

The kernel stays synchronous and runs on a dedicated OS thread; the Engine exposes an async
`prompt` future and a **runtime-agnostic `Stream`** for events, backed by an internal bounded
message-bus. slimcode does **not** adopt tokio. Operands stay synchronous traits, so slimcode's own
code-type preset (sync `BailianProvider`, sync tools) never crosses a bridge; a future async host
adapts its async provider/tool to the sync operand with a host-side `block_on` shim running on that
dedicated (non-tokio) loop thread. Events go over the bus because that is where a bus strictly wins
— backpressure isolation so a slow subscriber cannot stall the kernel, burst absorption, and a
natural async `Stream` — while an operand-side bus would only re-plumb the inherent wait. The turn's
terminal events (`TurnFinished`, then an optional `Compacted`) are still delivered after the taken
provider/tools/session state is put back, preserving ADR-0021 D4's read-the-finished-turn contract;
the bus must not let a subscriber see the pre-turn snapshot.

### D4 — Trace is a required, non-degrading seam built into the kernel

A `Tracer` is injected as a collaborator and the kernel emits RAII spans for `iteration`,
`provider.chat`, and `tool.{name}` (no-op when the tracer has no backend). A `SpanRecorder` is
threaded into the `Provider` operand so a serialized request body still lands on the
`provider.chat` span even when the transport fails. Trace cannot be reconstructed purely from
read-only events without losing request-level latency/token/recorder detail, so the kernel produces
the spans and the tracer backends stay a host-swappable leaf (mirroring slimbot's self-built span
model).

### D5 — Cross-session scheduling stays in the host; presets differ only in their defaults

The Engine/Session layer guarantees single-session semantics — cancellable, rollbackable (an
`initial_message_count` cursor a host's `after_turn` hook truncates back to on a cancelled or aborted
turn; an errored run instead returns `Err` and its recovery is the host's, since `after_turn` does not
fire there), event-producing, single-flight. Multi-session
concurrency, the per-session serial queue, channel dispatch, and AgentSession lifecycle stay in the
host (slimbot keeps its SessionRunner/MessageBus). A "code agent" and a "bot agent" are the same
Engine with different default-hook bundles: `AgentSession` becomes the code-type preset
(behavior-identical to today's slimcode), and a bot preset adds memory injection, runtime-context,
governance, rollback, consolidation and channel broadcast.

## Considered Options

- **Redesign the SDK in place to be a portable engine** — rejected: it pollutes `AgentSession`'s
  frontend-embed role (ADR-0021) and forces every slimcode frontend to absorb engine-level
  configuration; the two-audience split (Engine below, AgentSession preset above) keeps both.
- **Keep slimcode's 3-hook / 4-event kernel and bolt on the gaps** — rejected: it cannot host
  request-level trace spans, rollback, or the phase granularity governance needs, so slimbot's
  must-haves stay unmet.
- **A true async-native kernel** — rejected for this slice: rewriting `core`/`app` async plus all
  four frontends and the FFI C-ABI in one step is a cross-repo architectural break that contradicts
  ADR-0021 D4 and Q12's low-risk preference; the sync-kernel + async-facade shape reuses the whole
  existing sync stack additively.
- **A third neutral crate owning a shared engine both repos depend on** — rejected for now: extra
  dependency-direction churn; slimcode is the engine's home and slimbot depends on it later.

## Consequences

- `core::AgentRunner`'s public hook seam expands from three callbacks to the seven-phase pipeline;
  `app::runner::run_turn_with_hooks` and `AgentSession` are re-expressed on top of it, so the CLI,
  TUI, GUI and FFI keep their current behavior through the code-type preset.
- A `Tracer` / `SpanRecorder` / span model lands in slimcode (new; slimbot's `trace` is the prior
  art), and `RunConfig` gains optional iteration/timeout guardrails (default stays uncapped for
  code-type).
- slimcode gains an async facade and an internal event bus but no tokio dependency; a
  runtime-agnostic `Stream` is the async host's event surface.
- `crates/cli/tests/architecture.rs` is adjusted for the widened `core`/`app` seam (no new crate;
  S1 = a).
- **Equivalence gate**: the code-preset refactor is behavior-identical to today's slimcode on the
  public observable surface — event order, `StopReason`, the persisted JSONL log and its replay,
  usage/title, turn-end compaction, cancellation/error closing, and skill-read interception text.
  The seven-phase reordering, hook splitting, and expressing skill/rollback as hooks are internal
  only and must not change external results. This is pinned by end-to-end integration
  tests (in `crates/sdk` and `crates/app` `tests/`) that run the migrated engine against a
  pre-migration baseline captured on the same scripted inputs and assert byte-for-byte / event-for-
  event equality; any deliberate divergence must be recorded and proven externally unobservable,
  otherwise it is a regression. Adding a bot-only hook must leave the code-preset baseline green.
- Spec: `.scratch/engine/spec.md` (`Status: ready-for-agent`). Out of this slice: slimbot adopting
  the Engine, the cross-repo dependency, and any migration of slimbot's own loop.
