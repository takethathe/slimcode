# 01 (S1): rename crates and invert the provider seam

**What to build:** The mechanical half of ADR-0011: rename `crates/agent` → `crates/core`
(`slimcode-core`) and `crates/common` → `crates/app` (`slimcode-app`), and move the LLM seam out of
the agent runtime into `slimcode-ai` so `ai` stops depending on `core`. No behaviour changes.

**Blocked by:** None (can start immediately)

**Status:** ready-for-agent

- [ ] Rename both crate directories, their `[package] name`, the workspace `members` and
      `workspace.dependencies` entries, and every `use slimcode_agent::…` / `use slimcode_common::…`
      path (`slimcode_core::…` / `slimcode_app::…`). `crates/cli`'s `[[bin]] name = "slimcode"` stays.
- [ ] Move into `slimcode-ai`: the `Provider` trait, `Message`/`Role`/`Part`/`ToolCall` (the wire
      message model, currently `slimcode_agent::session`), `Delta`, `FinishReason`, `CancelToken`,
      and a new `ToolSpec { name, description, parameters }`.
- [ ] `Provider::chat` takes `tools: &[ToolSpec]`. `slimcode-core` keeps `Tool { spec: ToolSpec,
      run: Box<dyn Fn(Value) -> Result<String, String> + Send + Sync> }` and builds one
      `Vec<ToolSpec>` per run (S1 mounts it on the loop entry, not per request).
- [ ] `slimcode-core` keeps `AgentEvent`, `StopReason`, `RunConfig`, the loop and `assemble`, and
      gains the sole dependency edge `core → ai`. `slimcode-ai` has **no** `slimcode-*` runtime
      dependency (its `slimcode-app` dev-dependency for the live smoke tests' config constants may
      stay: Cargo allows a dev-dependency cycle; drop it only if it blocks the build).
- [ ] `ai/src/wire.rs` and `ai/src/provider.rs` import the message/delta/tool types from `ai` itself
      instead of `slimcode_agent::…`; `TokenUsage` stays where it is.
- [ ] Every existing unit test moves with its code and keeps passing; the two `#[ignore]` live
      smoke tests still compile.

- [ ] `cargo test` green, `cargo fmt --all`, `cargo clippy --all-targets --all-features -- -D warnings`
      clean.

## Notes

- Domain terms (per `CONTEXT.md`): `Provider`, `Message`, `Tool batch`. `AgentMessage` arrives in S2 —
  do not introduce it here.
- Docs: update `docs/development.md`'s 现状 table (`crates/agent`/`crates/common` rows and every
  `### crates/agent …` / `### crates/common …` heading) to the new names, and flip ADR-0011's status
  if you add one. Doc/decisions reference: `docs/adr/0011-crate-layering-cli-as-entry.md` D1/D4.
- Sweep every remaining *current-state* doc for the old crate paths (`README.md`,
  `docs/explanation.md`, `docs/user-manual.md`, `docs/configuration.md`); `CONTEXT.md` was already
  rid of them in the architecture session, so it should need nothing here.

## Comments
