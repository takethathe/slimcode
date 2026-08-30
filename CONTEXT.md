# slimcode

slimcode is a Rust coding-agent CLI: a prompt runs a read/write/edit/bash/grep/find/ls tool loop against a target directory, with streaming output, token-usage tracking, and saveable/restorable sessions.

## Language

**Prompt**:
A single user input submitted to the agent for one turn. A prompt may span multiple lines (a multi-line prompt).
_Avoid_: input, question

**Command**:
A `/xxx` control instruction in the REPL (e.g. `/save`, `/load`), distinct from a Prompt.
_Avoid_: slash-command

**Skill**:
An installable set of agent instructions (a `SKILL.md` with frontmatter `name` /
`description` / `disable-model-invocation`), discovered from a user scope
(`<home>/skills/`) or a project scope (`<cwd>/.slimcode/skills/`) and triggered
like a Command via `/name`. `disable-model-invocation: true` keeps the skill out
of the system prompt so the model only uses it on an explicit `/name` trigger.
A `/name` trigger injects the skill body plus its directory (the base for
relative paths in the body) as that turn's user message.
_Avoid_: plugin, extension

**Session**:
A conversation with a stable id, timestamp, messages, and optional title; persisted as JSON and restorable via `/load`.
_Avoid_: conversation (used interchangeably)

**Message history**:
The conversation messages of a Session (`session.messages`), restorable via `/load`.
_Avoid_: history (bare — collides with input history)

**Input history**:
The previously submitted Prompts in the REPL, persisted across runs; a UI concern, not part of a Session.
_Avoid_: history (bare — collides with message history), shell history

**Context**:
The assembled list of Messages for one turn, produced by `ContextBuilder::build()`
and handed straight to the agent runtime. Built from a base system prompt (default or
overridden), an advertised skills list, an optional message history, and a user prompt
or skill trigger. Distinct from message history (`session.messages`, which may be the
`with_history` input) and from input history.
_Avoid_: context window (collides with the LLM notion), assembled messages

**Multi-line prompt**:
A Prompt entered across multiple lines via continuation (a line ending in `\`, or best-effort Shift+Enter). Stored and submitted as a single Prompt.
_Avoid_: block, paste
