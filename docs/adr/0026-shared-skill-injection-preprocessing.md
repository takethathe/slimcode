# Skill injection is a shared app-layer prompt-preprocessing seam, consumed by every frontend

ADR-0013 D3 placed *command and skill semantics on the CLI side*: the CLI parses a leading
`/skill:name`, renders the `<skill>` block with `skill_prompt` (deduping against history via
`skill_loaded_in`) and hands the finished text to the SDK's `prompt`, which treats it as an opaque
user message. That worked while the CLI was the only interactive frontend, but it duplicated the
orchestration across two call sites (`main.rs::normalize_skill_trigger_prompt` for one-shot, with no
history; `tui.rs` for the interactive path, deduping against `session.messages`), and it left the GUI
frontend with no way to inject a skill at all — its controller forwards text straight to
`AgentSession::prompt`, so a typed `/skill:name` reaches the model as literal text. The **primitive
helpers already live in `slimcode_app::skills`** and are public; only the top-level orchestration was
CLI-owned. We lifted that orchestration into one public app-layer function so both frontends consume
the identical logic, and refactored the CLI's two paths to call it.

This supersedes the *"skill semantics stay on the CLI side"* half of **ADR-0013 D3**. Command
*dispatch* stays a frontend concern (the effects differ per frontend); only skill-trigger →
model-prompt preprocessing moved down.

## Decisions

### D1 — `prepare_user_prompt(input, skills, history) -> PreparedPrompt`

A pure function in `slimcode_app::skills` turning raw user input into what a turn actually sends:

```rust
pub enum PreparedPrompt {
    Prompt(String),                    // plain text, incl. an unrecognised leading /xxx (normalized)
    Skill { name, model_text, display }, // hit: model_text → SDK prompt; display → frontend block
}
```

It parses a leading skill trigger (`leading_skill_ref`), resolves it (`find_skill`), and renders
`skill_prompt(skill, arg, already_loaded)`, where `already_loaded` is computed from `history` through
`skill_loaded_in`. An empty `history` reproduces the one-shot's prior `already_loaded: false`; the
interactive/GUI paths pass the session's messages. A miss returns `Prompt(normalize_skill_trigger(input))`.
Command recognition (`find` / `is_builtin_command`) is deliberately **not** in here — commands become
frontend effects, not a prompt.

### D2 — `Command` carries a `gui_applicable` flag; `complete()` filters by caller

The GUI Completion popup must not offer commands with no GUI backing (`/history`, `/!!`, `/!N` need
input history the GUI does not persist; `/install-skill` writes the store). Rather than a host-side
allowlist (a second source of truth), `slimcode_commands::Command` gains a `gui_applicable: bool`
field, and the candidate assembly filters on it for the GUI while the TUI still sees everything.
Single-declarative-source, unit-testable. A `frontends` bitset was considered and rejected as
speculative while only two frontends exist.

### D3 — GUI submit path splits by layer

- **Frontend TS** intercepts the small effect-mapped command set — `/new`→newSession,
  `/compact`→compactNow, `/exit`→close, `/help`→local command list — and does **not** call
  `prompt` for them.
- **Everything else** (skill triggers, plain text, the Q-Accepted pass-through of un-mapped
  `/usage` / `/skills`) goes over a new host `prompt` path that runs `prepare_user_prompt` and, for a
  `Skill`, sends `model_text` to the SDK.
- Candidates come from a new host `complete(input) -> Vec<CompletionItem>` IPC reusing
  `slimcode_app::skills::complete` (fuzzy, best-first), fetched per keystroke.
- While the popup is open, `Enter` **commits to buffer** exactly like `Tab` (a later `Enter`, popup
  closed, submits); selection auto-tracks the best match and goes sticky after `↑/↓`, mirroring
  ADR-0005. A committed, recognised leading `/` token renders as a **Command chip** in the input; the
  recognition test reuses `complete(token)` for an exact-`value` match (no extra IPC).

## Considered Options

- **Reimplement skill injection (and the fuzzy matcher) in frontend TS** — rejected: instant drift
  from the Rust registry the CLI/TUI share, and TS cannot reach `skill_prompt`/`skill_loaded_in`.
- **Move `/` command + skill semantics into the SDK** — rejected: ADR-0022 keeps the SDK a thin,
  semantics-free programmatic seam; command effects are frontend-specific.
- **Add a `resolve(input)` IPC for chip recognition** — rejected: `complete(token)` already answers
  exact-match recognition with no new surface.

## Consequences

- `crates/cli` `main.rs` and `tui.rs` call `prepare_user_prompt`; their local duplication is removed
  (behaviour preserved: one-shot passes an empty history).
- `slimcode_app::skills` gains `prepare_user_prompt` + `PreparedPrompt`; `slimcode-commands::Command`
  gains `gui_applicable` (data-model change — update `architecture.rs` expectations and command tests).
- `gui/src-tauri` gains a `complete` IPC, wires a Skills snapshot into the controller, and routes
  `prompt` through `prepare_user_prompt`; the skill invocation first renders as an ordinary user
  message in the GUI transcript (a dedicated `[skill]` block is deferred).
