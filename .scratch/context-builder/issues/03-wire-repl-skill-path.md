# 03: wire REPL + skill-trigger path

**What to build:** The REPL builds every turn through the new context builder: a fresh session seeds the system prompt once, a continued session is not re-seeded, and a `/name` skill trigger makes the skill body the user message without recording it in input history. The REPL-side message-list helper that the builder replaces is deleted.

**Blocked by:** 01 — context builder core in common

**Status:** resolved

- [ ] New-session prompts build through the context builder (system seeded once, skills advertised).
- [ ] Continued-session prompts build through the context builder without re-seeding the system message.
- [ ] Skill triggers build through the context builder's `with_skill` path; the trigger is dispatched as a prompt turn and is not recorded in input history.
- [ ] The old REPL message-list helper is removed; its tests are moved to the context-builder module.
- [ ] REPL scripted tests pass, including the skill-trigger-dispatches-a-turn case.

- [ ] The development docs and the explanation doc describe the REPL path and the core concept of built context.

## Answer

`submit_prompt` and `submit_skill` in `crates/cli/src/repl.rs` now build every
turn through `ContextBuilder` (shared `finish_turn` tail: title, optional input
history record, run, persist). A fresh session seeds the system message once
with skills advertised; a continued session is not re-seeded; a `/name` skill
trigger goes through `with_skill` and is dispatched as a prompt turn without
being recorded in input history. The old `messages_for_prompt` is deleted and
its two unit tests moved to the context module. REPL scripted tests pass,
including `skill_trigger_dispatches_a_turn_not_unknown_command`.
`docs/development.md` and `docs/explanation.md` describe the REPL path and the
core concept of built context.
