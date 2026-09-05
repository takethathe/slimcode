# 02: wire non-interactive path

**What to build:** The one-shot CLI mode (`slimcode "<prompt>"` with its flags) assembles its message list through the new context builder instead of hand-building system + user messages. Output and behavior are unchanged: a fresh turn still seeds the system prompt and advertises the same skills. The CLI-level system-prompt construction that the builder replaces is deleted.

**Blocked by:** 01 — context builder core in common

**Status:** resolved

- [ ] The one-shot mode builds its message list via the context builder with the resolved skills and the user prompt.
- [ ] The old base system prompt constant and the CLI-side system-prompt builder are removed; their tests are moved to the context-builder module.
- [ ] Non-interactive CLI tests pass with no change to expected output or session contents.

- [ ] The development docs describe the non-interactive path as consuming the shared builder.

## Answer

`run_once` in `crates/cli/src/main.rs` now builds its message list through
`ContextBuilder::new().with_skills(skills).with_user_prompt(prompt).build()`.
The old `BASE_SYSTEM_PROMPT` constant and CLI-side `build_system_prompt` are
deleted; their four unit tests moved to `slimcode-common::context` (system
prompt structure, skills advertising, markdown layout, no-skills case).
`docs/development.md` describes the non-interactive path as consuming the
shared builder.
