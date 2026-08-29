# slimcode

slimcode is a Rust coding-agent CLI: a prompt runs a read/write/edit/bash/grep/find/ls tool loop against a target directory, with streaming output, token-usage tracking, and saveable/restorable sessions.

## Language

**Prompt**:
A single user input submitted to the agent for one turn. A prompt may span multiple lines (a multi-line prompt).
_Avoid_: input, question

**Command**:
A `/xxx` control instruction in the REPL (e.g. `/save`, `/load`), distinct from a Prompt.
_Avoid_: slash-command

**Session**:
A conversation with a stable id, timestamp, messages, and optional title; persisted as JSON and restorable via `/load`.
_Avoid_: conversation (used interchangeably)

**Message history**:
The conversation messages of a Session (`session.messages`), restorable via `/load`.
_Avoid_: history (bare — collides with input history)

**Input history**:
The previously submitted Prompts in the REPL, persisted across runs; a UI concern, not part of a Session.
_Avoid_: history (bare — collides with message history), shell history

**Multi-line prompt**:
A Prompt entered across multiple lines via continuation (a line ending in `\`, or best-effort Shift+Enter). Stored and submitted as a single Prompt.
_Avoid_: block, paste
