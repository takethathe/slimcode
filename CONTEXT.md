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
like a Command via `/skill:name`. `disable-model-invocation: true` keeps the skill
out of the system prompt so the model only uses it on an explicit `/skill:name`
trigger. A `/skill:name` trigger injects the skill body plus its file location
(`<location>`) and directory (the base for relative paths in the body) as that
turn's user message, wrapped in a `<skill>` XML block (pi-style). Skills without
`disable-model-invocation` are advertised in the system prompt as a `## Skills`
markdown index, one bullet per skill (`- name: description [Read from
<file>]`), and the
model applies a skill when its name/description matches the task or when the user
references it explicitly as `/{name}`. If the
skill was already loaded in an earlier message of the same conversation, its body
is replaced by an already-loaded notice (the base-dir line is kept) so the model
finds the instructions in the earlier message instead of reloading.
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
overridden), injected context files (global + project `AGENTS.md`), an advertised
skills list, an optional message history, and a user prompt or skill trigger. Distinct
from message history (`session.messages`, which may be the `with_history` input) and
from input history.
_Avoid_: context window (collides with the LLM notion), assembled messages

**Context file**:
A discovered `AGENTS.md` injected into the system message's `## Project context`
markdown section (pi-style, but with a markdown header instead of a
`<project_context>` XML wrapper). Two scopes: **global** (`<home>/AGENTS.md`, i.e.
`$SLIMCODE_HOME` or `~/.slimcode`, applies to every project) and **project**
(`AGENTS.md` in the working directory itself and in the git repository root —
the nearest ancestor holding a `.git` entry, whether a directory or a
`gitdir:` file marker; ordered root-first so cwd lands last). Each injected
file's content is wrapped in a `<project_instructions path scope>` XML block
(its `scope="global|project"` attribute labels the intent), and the section
declares that project requirements override global ones when they conflict.
When no `AGENTS.md` can be injected the whole section is omitted, so the system
message is byte-identical to a run without this feature. Discovery is
infallible (missing files yield none) and deduplicated by path in favour of the
global scope. Distinct from a Skill (instructions on demand via
`/skill:name`) — a context file is always loaded into the system message.
_Avoid_: project instructions file (ambiguity with scope), rules file

**Multi-line prompt**:
A Prompt entered across multiple lines in the TUI input box (Shift+Enter or Ctrl+J inserts a newline; Enter submits). Stored and submitted as a single Prompt.
_Avoid_: block, paste

**Completion popup**:
The fuzzy candidate list shown above the TUI input box while a partial `/` command is typed (commands + installed skills, ranked best-first); it renders as bare SelectList rows (no box) directly above the input so the input box stays put when it opens/closes. `↑`/`↓` move the selection, `Tab` commits it to the buffer, `Enter` executes it, `Esc` cancels. Distinct from the post-submit "did you mean" notice.
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

**Theme**:
A semantic color-token system (accent, border, borderAccent, muted, dim, userMessageBg, toolPendingBg, toolSuccessBg, toolErrorBg, markdown tokens, ...) whose names and hex values mirror pi's `dark.json`; the TUI resolves tokens to terminal colors at render time. Dark only.
_Avoid_: palette, colors, style sheet

**Transcript**:
The scrollable top region of the TUI holding the startup header, user/assistant messages, and tool blocks; it scrolls, the dock below it never does.
_Avoid_: chat area, message list

**Dock**:
The fixed bottom region of the TUI (completion popup, editor, footer) that never scrolls; the transcript scrolls above it. The popup (0 rows when closed) sits directly above the editor so opening it never shifts the input box or footer.
_Avoid_: footer bar, bottom bar

**Footer**:
The bottom two lines: a dim cwd/branch/session line plus a dim token-stats line with the model name right-aligned. Distinct from the runner status embedded in the editor's top border.
_Avoid_: status bar, statusline, status line

**Status indicator**:
The runner status embedded in the editor's top border while a turn runs: a braille spinner + "Working..." drawn left-aligned on the top `─` line, in the running border color (`borderAccent` cyan); idle is a plain `─` border line. Replaces the old separate spinner row above the editor (ADR-0007 D2).
_Avoid_: loading bar, RUNNING flag, spinner line

**Config**:
The resolved, non-secret application settings (model, base URL, cache flag) produced by the four-layer resolution in `slimcode-common::config` (frontend overrides > env > `config.toml` > defaults), handed to the frontends as a `BailianConfig`. Distinct from credentials.
_Avoid_: settings file, options

**config.toml**:
The user-level config file at `~/.slimcode/config.toml` (or `$SLIMCODE_HOME/config.toml`): a TOML `[ai]` table with optional `base_url` / `model` / `cache` / `api_key`. Edited by hand or interactively via `slimcode config`. Distinct from credentials management.
_Avoid_: config directory, rc file

**API key / credential**:
A secret (e.g. `DASHSCOPE_API_KEY` / `[ai] api_key`) with source precedence `--api-key` > env > `config.toml`, distinct from non-secret Config. When stored in `config.toml` it is plaintext on disk, so slimcode suggests `chmod 600` on loose permissions and `slimcode config` tightens to 0600 after writing a key.
_Avoid_: config value, setting
