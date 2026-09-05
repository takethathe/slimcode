# slimcode v1.1 — REPL Input History & Multi-line Prompts

Status: ready-for-agent

## Problem Statement

The line-based REPL (`crates/cli`) forgets what you typed. Once you submit a prompt it is gone: you cannot recall or re-run a previous prompt without retyping it, and any retyping is lost the moment you press Enter. There is also no way to enter a prompt that spans multiple lines — a long instruction must be crammed onto one line or pasted as a single unwieldy blob, which is error-prone and hard to edit. This makes multi-turn work in the REPL repetitive and brittle.

## Solution

From the user's perspective: the REPL now remembers every prompt you have submitted (across runs) and lets you recall and re-run it. You can enter a prompt that spans several lines by ending each line but the last with `\`; the whole block is submitted as one prompt. Two UI concepts — *input history* and *multi-line prompts* — make the REPL behave more like a shell without pulling in a readline dependency or raw terminal mode.

## User Stories

1. As a REPL user, I want the REPL to remember the prompts I submit, so that I don't have to retype them.
2. As a REPL user, I want the history to survive a restart, so that I can recall prompts from earlier sessions.
3. As a REPL user, I want to list recent prompts with `/history`, so that I can see what I've been working on.
4. As a REPL user, I want `/history` to show the most recent prompt first, so that the freshest item is easiest to find.
5. As a REPL user, I want `/history` to number each entry, so that I can refer to an entry by number.
6. As a REPL user, I want `/history` to cap at 20 displayed entries, so that the list stays scannable.
7. As a REPL user, I want `/!!` to re-run my most recent prompt, so that I can repeat the last action instantly.
8. As a REPL user, I want `/!N` to re-run the N-th listed prompt, so that I can repeat any recent prompt, not just the last.
9. As a REPL user, I want `/!N` to re-run the prompt verbatim, so that multi-line prompts repeat exactly as entered.
10. As a REPL user, I want a re-run to submit as a fresh turn in the current session, so that my message history and session continuity are preserved.
11. As a REPL user, I want history to record only ordinary prompts, not `/` commands, so that the list stays meaningful and isn't cluttered by control commands.
12. As a REPL user, I want each submitted prompt recorded even if it duplicates a previous one, so that behavior is predictable and nothing is silently dropped.
13. As a REPL user, I want history persisted to a JSON file under the slimcode home directory, so that it lives alongside my sessions and respects `SLIMCODE_HOME`.
14. As a REPL user, I want history capped at 500 entries with the oldest dropped, so that the file stays bounded.
15. As a REPL user, I want history saved immediately after each prompt, so that a crash doesn't lose what I've typed.
16. As a REPL user, I want to enter a multi-line prompt by ending lines with `\`, so that long instructions are readable and writable.
17. As a REPL user, I want a multi-line prompt submitted as a single user message, so that the agent sees one coherent instruction rather than fragments.
18. As a REPL user, I want a multi-line prompt recorded as a single history entry, so that `/!N` re-runs the whole block, not pieces.
19. As a REPL user, I want lines starting with `/` inside a continuation to be treated as prompt content, so that prompts containing literal `/foo` work.
20. As a REPL user, I want an empty input line (outside continuation) to be ignored, so that accidental Enter doesn't submit anything.
21. As a REPL user, I want the `/help` text to document the new `/history`, `/!!`, `/!N` commands and the `\` continuation, so that the features are discoverable.

## Implementation Decisions

- **New `history` module** in the CLI crate, mirroring the existing `session` module pattern: a `HistoryStore` backed by a single JSON file of strings (a JSON array), constructed from the slimcode home directory (`<home>/history.json`, respecting `SLIMCODE_HOME`). API: load-all, append-one, trim-to-limit, save. No new dependencies.
- **`repl` module changes**:
  - Two new pure functions alongside the existing `classify`: `is_continuation(line) -> bool` (line is non-empty after trim and ends with `\`) and `strip_continuation(line)` (removes the trailing continuation marker).
  - The `run` loop gains a `pending` buffer: while a continuation marker is present, the incoming line is stripped of the marker, appended, and reading continues; the first line without the marker commits the whole block. All lines inside a continuation — including ones starting with `/` — are treated as prompt content. Outside continuation, behaviour is unchanged (empty / command / prompt dispatch via `classify`).
  - Three new commands: `/history` (render the last 20 entries, newest first, numbered 1 = newest), `/!!` (re-run entry 1), `/!N` (re-run entry N). A re-run submits the stored prompt verbatim as a fresh turn through the same path as a typed prompt, but is **not** appended to history again.
  - History append happens once per submitted prompt (whether typed directly or as a committed multi-line block), after the turn is dispatched.
  - `messages_for_prompt` is unchanged: a committed block is a single user message (the prompt string may contain newlines).
- **`run` signature**: takes the new `HistoryStore` alongside the existing `SessionStore`. Existing callers and tests updated accordingly.
- **Architecture constraints honoured** (see ADR-0001): no readline crate, no raw terminal mode, no Shift+Enter detection (indistinguishable from Enter on traditional terminals). Continuation is the trailing-`\` marker only.

## Testing Decisions

- A good test asserts **external behaviour**: scripted bytes fed through the REPL `run` loop, then assertions on captured output, the resulting session messages, and the on-disk history file. Pure helpers are tested directly; the loop's behaviour is tested through `run` (the single high seam), not by poking at internal state.
- **Module under test**: the CLI crate (`repl` + `history`). The agent and ai crates are untouched.
- **Prior art**: existing `repl` tests (`classify_distinguishes_kinds`, `messages_for_prompt`/`continued_prompt_does_not_reseed_system`, `load_resumes_a_saved_session`, `load_without_id_errors`) — pure functions unit-tested plus scripted `run` integration tests with a temp dir and captured output. New tests follow the same shape: a `temp_dir()` helper, scripted byte input, and `SessionStore`/`HistoryStore` on a unique temp dir.
- Cases to cover: continuation commits one user message; `/!N` re-runs verbatim without re-appending; `/history` newest-first numbering; only prompts recorded (no commands); no dedup; 500-entry cap drops oldest; empty input outside continuation ignored; `/`-prefixed lines inside continuation stay in the block; history survives a re-open of the file.

## Out of Scope

- Shift+Enter continuation (infeasible in traditional terminals; recorded as not-done in ADR-0001).
- Arrow-key / line-editing / Ctrl-R search (would require a readline crate or raw terminal mode).
- Input history per-session or per-`Session` scoping — history is global across sessions.
- Recording `/` commands in history.
- Any change to the agent or ai crates, the session JSON schema, or the session store.

## Further Notes

- Terminology is recorded in `CONTEXT.md`: `input history` (the REPL's recalled prompts) is deliberately distinct from `message history` (`session.messages`). Use the glossary terms in code, tests, and docs.
- The no-dependency / no-Shift+Enter decision is ADR-0001 (`docs/adr/0001-repl-input-history-dependency-free.md`).
- Ticket `06-repl-input-history-and-multiline` in `.scratch/slimcode-v1/issues/` tracks this work; `map.md` has been updated.
