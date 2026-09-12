# Crate layering with the CLI as the sole entry point

The workspace is re-cut into six crates with a single direction of dependency and one binary:
`slimcode-ai` (LLM wire model + provider), `slimcode-core` (agent runtime: events, runner, hooks),
`slimcode-app` (frontend-agnostic application services + the display contract), `slimcode-commands`
(pure command registry), `slimcode-tui` (terminal graphics library), and `slimcode` (the CLI, the
only entry point). The two existing names that no longer describe their contents are renamed:
`crates/agent` → `crates/core`, `crates/common` → `crates/app`.

## Decisions

### D1 — `ai` owns the LLM seam; the current `ai → agent` dependency is inverted

`Provider`, `Message` (the LLM wire message), `Delta`, `FinishReason`, `ToolSpec` (the
name/description/parameters schema a provider needs to advertise a tool), `CancelToken` and
`TokenUsage` all live in `slimcode-ai`, which depends on no other slimcode crate. `slimcode-core`
owns the executable tool (`Tool { spec, run }`), `AgentEvent`, `StopReason`, `RunConfig`,
`AgentRunner`, `AgentMessage` and the `to_llm`/`convert` step, and depends only on `ai`. The
`Provider` trait therefore no longer forces `ai` to import the agent runtime (today
`crates/ai/src/provider.rs` references `slimcode_agent::agent::Provider` and
`slimcode_agent::session::Message`, and `crates/ai/src/wire.rs` references
`slimcode_agent::session`).

This mirrors pi: `pi-ai` owns the message model and the tool schema, `pi-agent-core` wraps them
with execution behaviour.

### D2 — `app` stays the frontend-agnostic application layer

`ContextBuilder`/`Context`, the project-scoped session store, input history, skills,
`context_files`, the seven coding tools, `setup`, and the display contract (`DisplayItem`,
`map_event`, `Renderer`, `usage_summary`, `run_turn`) remain in the
frontend-agnostic layer. Dissolving this into the CLI (pi's shape, where `pi-coding-agent` owns all
of it) was rejected: ADR-0004's investment in shared, non-drifting frontend behaviour is what makes
a second frontend (or a future web/RPC one) cheap, and a "graphics library" TUI needs a
non-terminal place for its services to live.

### D3 — the CLI is the entry point; the TUI is a library

`slimcode` (crates/cli) is the only binary. It owns argv parsing, mode selection (one-shot text vs
interactive TUI), config resolution, service construction, command semantics, session persistence,
and the display adapters for both modes. `slimcode-tui` owns no process or application lifecycle: no
argv, no config, no provider, no session I/O, no command semantics — and no dependency on any other
slimcode crate (ADR-0014).

### D4 — dependency matrix

| crate | depends on |
| --- | --- |
| `slimcode-ai` | — |
| `slimcode-core` | `ai` |
| `slimcode-app` | `ai`, `core`, `commands` |
| `slimcode-commands` | — |
| `slimcode-tui` | — |
| `slimcode` (bin) | all of the above |

ticket 06 pins this with a test that reads each crate's `Cargo.toml`
(`.scratch/arch-realignment/issues/06`).

## Considered Options

- **Dissolve `app` into the CLI** (pi's `pi-coding-agent` shape) — rejected: it reverses ADR-0004,
  welds session/skills/context/tools to the terminal binary, and leaves a future non-terminal
  frontend with nothing to reuse.
- **Keep `slimcode-common` untouched and only add a `core` crate** — rejected: `common` would keep
  both application services and the display/runner seam while the TUI kept importing services, so
  "the TUI is a library" would be a claim without a boundary.
- **Rename nothing, only move files** — rejected: `agent` would still host coding tools and the
  session model, `common` would still be the everything crate, and every later step would have to
  re-read a misleading layout.

## Consequences

- Migration is split into six green tickets (`.scratch/arch-realignment/issues/01..06`); ticket 01
  is the mechanical rename and ticket 02 the `Provider`/`Message`/`ToolSpec` move, so later tickets
  work against final names.
- `crates/ai` gains the `CancelToken` type (the provider seam's cancellation handle) and
  `ToolSpec`; `crates/core`'s loop builds a `Vec<ToolSpec>` per run from its `Tool`s.
- The CLI grows (services, command semantics and both display adapters move in) and
  `crates/tui/src/terminal.rs` (~844 lines today) shrinks to UI mechanics (ADR-0013).
- `docs/development.md`'s architecture section carries the target table until ticket 06 lands, at
  which point the old "现状" block is deleted.
