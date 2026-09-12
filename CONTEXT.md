# slimcode

slimcode is a Rust coding-agent CLI: a prompt runs a read/write/edit/bash/grep/find/ls tool loop against a target directory, with streaming output, token-usage tracking, and saveable/restorable sessions.

## Language

**Prompt**:
A single user input submitted to the agent for one turn. A prompt may span multiple lines (a multi-line prompt).
_Avoid_: input, question

**Command**:
A `/xxx` control instruction in an interactive frontend (currently the TUI), e.g. `/load`, `/sessions`; distinct from a Prompt.
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
A conversation with a stable id, timestamp, message history, and optional title; persisted as an
append-only session log and restorable via `/load`.
_Avoid_: conversation (used interchangeably)

**Session log**:
One Session's `<id>.jsonl` file: a log header line followed by one record per line
(`{"type":"message","message":{…}}`, `{"type":"title","title":…}`), appended as messages enter
history and never rewritten — the only writes are the header-plus-backlog write that creates the
log, one record per message after that, and the sealing newline a lenient load may add to a torn
tail. A record whose `type` is unknown is ignored, so new record kinds stay compatible with older
readers. The title is a record, never a header field.
_Avoid_: session file (ambiguous with the legacy `<id>.json`), dump, snapshot

**Log header**:
The first line of a session log —
`{"type":"session","v":1,"id":…,"created_at":…,"project_home":…}` — carrying the Session's
immutable identity, the log format version, and the project home it was created under
(informational: the project-key directory, not the header, decides where a session is found).
_Avoid_: metadata, front matter

**Dangling tool batch**:
A read state in which an assistant message's `tool_calls` have no matching tool results (a crash or
cancel mid-batch), or a tool result has no matching `tool_calls`. Repaired in memory on load —
missing results filled in with an `Error: interrupted` tool result, orphan results dropped — so the
replayed message history always pairs every `tool_call_id`; the log itself is never rewritten.
_Avoid_: broken tool call, incomplete turn

**Session store**:
The project-scoped place sessions are persisted: one directory per project
(`<home>/sessions/<project-key>/`) holding one session log per Session; `/load` and `/sessions` see
only the current project's directory, and legacy `<id>.json` files in it are neither listed nor
loaded (they still count toward the storage quota).
_Avoid_: sessions dir, archive

**Project key**:
The deterministic name of a project's directory inside the session store: the project
home's basename plus a hash of its full path, so a given project always maps to the same
directory and same-named projects at different paths stay separate.
_Avoid_: project slug, project id

**Storage quota**:
The byte ceiling on the whole session store (`[sessions] max_mb`, default 500 MiB),
distinct from any single session's size.
_Avoid_: size limit, cache

**Eviction**:
Removing session logs once the session store exceeds the storage quota: oldest-first by mtime, down
to half the quota, never the active session; legacy `<id>.json` files are invisible to `/load` but
still counted, so they disappear through eviction rather than migration. An **empty session log**
(zero-byte, or replaying to no assistant record — residue of a crash during log creation) is
removed by a separate startup sweep that touches only the current project.
_Avoid_: cleanup, pruning, GC

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

**Global home**:
The slimcode home directory (`$SLIMCODE_HOME` or `~/.slimcode`) as named in the system prompt's `## Environment` info — the global scope where context files and skills live. Same directory as `home`; the "global" modifier mirrors the `global|project` scope labels of context files.
_Avoid_: user home, OS home

**Project home**:
The directory the agent is grounded in, surfaced in the system prompt's `## Environment` info: the nearest ancestor of the working directory holding a `.git` entry (a directory or a `gitdir:` file marker, per `context_files::find_git_root`), falling back to the user home when no ancestor is a git repo. Distinct from the working directory (cwd, which may be a subdirectory of the project home) and from the slimcode home (home / global home).
_Avoid_: project root (ambiguous with git root), working directory

**User home**:
The OS user home directory (`$HOME`); the fallback value of project home when no ancestor of the working directory holds a `.git` entry. Distinct from `home` / global home (the slimcode home, which defaults to `~/.slimcode` under the user home).
_Avoid_: home (bare — collides with slimcode home), global home

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
