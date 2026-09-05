# 01: context builder core in common

**What to build:** A frontend-agnostic context builder in the common crate that assembles one turn's message list from a default (or overridden) system prompt, an advertised skills list, an optional message history, and a user prompt or skill trigger. On build it returns the assembled message list, ready to hand straight to the agent runtime. This slice is not yet wired into the CLI and is verified by unit tests.

**Blocked by:** None (can start immediately)

**Status:** resolved

- [ ] The builder is fluent: default system, `with_system` override, `with_skills`, `with_history`, `with_user_prompt`, and `with_skill` (reusing the existing skill-prompt text, including an optional task argument).
- [ ] `build()` returns the assembled `Vec<Message>`; when no user content is set it errors instead of silently producing a turn without a user message.
- [ ] The default system prompt text lives in the common module and is used unless overridden.
- [ ] Skills are advertised in a markdown `## Available skills` section, and only skills not marked `disable-model-invocation` are included.
- [ ] Empty history seeds exactly one leading system message; non-empty history is not re-seeded.
- [ ] Unit tests cover: default system, custom system, skill advertising and filtering, empty vs non-empty history, user prompt, skill trigger (with and without task argument), and the missing-user error.
- [ ] The module is registered so other crates can consume it; the glossary gains a `Context` term and the development docs note the new module.

- [ ] The full common-crate test suite passes.

## Answer

Implemented `slimcode-common::context` (`crates/common/src/context.rs`): a
frontend-agnostic `ContextBuilder` that assembles one turn's message list from a
default/overridden system prompt, an advertised skills list, an optional message
history, and a user prompt or skill trigger. `DEFAULT_SYSTEM_PROMPT` (the CLI's
former `BASE_SYSTEM_PROMPT`) lives in the module and is the builder's default
system. Registered as `pub mod context` in `lib.rs`. `build()` returns
`Vec<Message>` ready for `run_agent_from_messages`; it errors when no user
content is set. Empty history seeds exactly one leading system message with the
`## Available skills` section (auto-invokable skills only); non-empty history is
not re-seeded. 12 unit tests assert directly on `build()` output.
