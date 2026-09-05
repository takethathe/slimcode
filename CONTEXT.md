# slimcode

slimcode is a Rust coding-agent CLI: a prompt runs a read/write/edit/bash/grep/find/ls tool loop against a target directory, with streaming output, token-usage tracking, and saveable/restorable sessions.

## Language

**Prompt**:
A single user input submitted to the agent for one turn. A prompt may span multiple lines (a multi-line prompt).
_Avoid_: input, question

**Command**:
A `/xxx` control instruction in an interactive frontend (currently the TUI), e.g. `/save`, `/load`; distinct from a Prompt.
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
The previously submitted Prompts in an interactive frontend, persisted across runs; a frontend concern, not part of a Session.
_Avoid_: history (bare — collides with message history), shell history

**Context**:
The assembled list of Messages for one turn, produced by `ContextBuilder::build()`
and handed straight to the agent runtime. Built from a base system prompt (default or
overridden), an advertised skills list, an optional message history, and a user prompt
or skill trigger. Distinct from message history (`session.messages`, which may be the
`with_history` input) and from input history.
_Avoid_: context window (collides with the LLM notion), assembled messages

**Multi-line prompt**:
A Prompt entered across multiple lines in the TUI input box (Shift+Enter inserts a newline; Enter submits). Stored and submitted as a single Prompt.
_Avoid_: block, paste

**Completion popup**:
The fuzzy candidate list shown below the TUI input box while a partial `/` command is typed (commands + installed skills, ranked best-first). `↑`/`↓` move the selection, `Tab` commits it to the buffer, `Enter` executes it, `Esc` cancels. Distinct from the post-submit "did you mean" notice.
_Avoid_: autocomplete box, suggestion popup, picker

**Commit to buffer**:
The action of accepting a Completion popup selection with `Tab`: the selected `/` spelling is written into the input buffer with a trailing space and the cursor placed after it, ready for an argument; the command is **not** submitted. `Enter` commits without the trailing space and submits immediately.
_Avoid_: 上屏, fill, autofill, insert completion

**Frontend**:
A user-facing entry point — the one-shot CLI or the interactive TUI — that reads input, drives the shared turn runner, and renders output through its own Renderer.
_Avoid_: UI, client

**TUI**:
The interactive full-screen terminal frontend (ratatui + crossterm) entered when `slimcode` starts without a prompt; it replaces the line-based REPL.
_Avoid_: REPL, UI

**Renderer**:
The per-frontend component that turns DisplayItems into frontend output — text lines for the CLI, widget state for the TUI.
_Avoid_: printer, formatter

**DisplayItem**:
The frontend-agnostic unit a Renderer consumes (turn marker, streamed text fragment, reasoning line, tool start/result, stop marker, or token usage), produced from an AgentEvent by a shared mapping function.
_Avoid_: RenderText, view model
