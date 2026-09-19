# Two session commands: a full-screen session picker and a session reset

slimcode had three session commands split across two mental models: `/load <id>` (resume
by id), `/sessions` (print a bare id list into the transcript), and `/new`. Resuming meant
reading ids off a printed list and retyping one, and every session question required a
command of its own. The reference implementation (pi) has the same three jobs but answers
the list-then-pick one with a **session selector**; slimcode now collapses the surface to
`/session` (one full-screen picker that lists this project's sessions, marks the current
one and loads the chosen one on Enter) and `/new` (start over: fresh messages, fresh usage,
cleared screen). This ADR records the command surface, the picker seam, what "current
session" means for selection, and the decision that token usage belongs to a live Session.

## Decisions

### D1 — The session command surface is `/session` and `/new`, and nothing else

`/load`, its `/resume` alias, and `/sessions` are removed. `/session` opens the picker and
takes no argument; an argument is ignored, the way `/new`, `/usage` and `/skills` already
ignore theirs. There is no id-based load path left in the TUI, and no `-r`/`--resume`
flag is added to the CLI, so the picker is the only way in (short of rerunning with a
different working directory).

This is a deliberate reversal of pi's naming: pi's `/session` prints session *info*
(file, id, counts, tokens, cost) and its picker is `/resume`. slimcode uses `/session` for
the picker because picking *is* the job the user has for this command, and because the
only part of pi's info dump worth keeping — tokens — already has a home in `/usage`.

### D2 — The picker is a TUI view fed by CLI-owned rows

`/session` produces the existing `Effect::Command`; the CLI answers it by scanning the
project-scoped session store and pushing one new item, `RenderItem::SessionPicker { rows }`,
whose `SessionRow { id, title, meta }` fields are CLI-composed (`title` already falls back
to the id, `meta` is the preformatted "count + time" right column). The library owns the
picker state (`selected`, window offset), the keymap, the wheel and the full-screen
rendering; selecting a row emits the new `Effect::LoadSession { id }`, which the CLI
answers with the same code path `/load` used. `Esc` closes the picker inside the library
and produces no effect — closing a view is not a command.

The picker replaces the whole frame (transcript, completion popup, editor and footer alike,
with its own one-line header, list and one-line hint), because the library's layout is a
fixed four-region grid rather than pi's grow/shrink dock. It is therefore the library's
first non-chat view, and the wheel becomes view-dependent: over the picker it moves the
selection, and it still never touches the transcript or input-history recall (this amends
ADR-0017 D4, which made the wheel a whole-screen reading gesture).

### D3 — Selecting the current session is a no-op

The picker lists what is on disk, so the current session appears in it once its log exists
(the log is created with the first assistant message, ADR-0009 D2) and is marked with `*`.
Enter on that row closes the picker and changes nothing: the CLI recognises its own session
id and skips the load. Reloading would have been pi's behaviour, but it re-reads the log,
drops the in-memory history that is ahead of it, clears the screen and resets usage — a
destructive outcome for a row the cursor often starts on.

A brand-new `/new` session has no log yet, so it is absent from the list rather than shown
as a synthetic row. The picker header therefore never lies about what is on disk.

### D4 — Token usage belongs to the Session, in memory only

`Session` gains a `usage` field (`#[serde(skip)]`, never written to the log) that the CLI
accumulates per turn from the difference in the provider's running total, and `/usage`, the
footer stats line and any future session report read *it* instead of the provider.
`Provider::total_usage` stays the runtime's own running counter (the one-shot path still
reports it), but it stops being the display's source of truth — which also fixes `/load`,
where the footer used to keep showing the previous session's numbers. A resumed or new
session starts at zero; usage is not persisted, so a restarted session forgets it. This is
deliberate: pi persists per-entry usage, the owner chose not to.

> **Amended by ADR-0020 D6**: the session still owns the total and the display still reads it,
> but the accumulation is now event-driven — each assistant message carries its request's usage
> (`Provider::chat` return value → `AgentEvent::Message`), so the footer updates per assistant
> reply instead of once per turn, and the turn-end provider diff is gone. `Provider::total_usage`
> is now read only for the compaction call's own cost.

### D5 — A row is the title, the record count and the file's mtime

Rows are ordered by modification time, newest first, and read `title ?? id` on the left with
`N msgs` and the modification time on the right. Title and count come from one streaming
pass over each log: the header line must parse (a log whose header does not is skipped, as
pi does), the first title record wins, and messages are counted by recognising the
`message` record tag rather than fully re-parsing every line — the scan runs synchronously
on the UI thread when the picker opens, and full parsing would scale with the store's 500
MiB quota instead of with its line count. The time is the file's own mtime, formatted with
the existing dependency-free civil-date arithmetic and therefore on the **UTC clock with no
zone conversion and no `Z`**: sessions are only ever compared against each other, and the
owner explicitly rejected a timezone dependency.

## Considered Options

- **Keep pi's names** (`/resume` for the picker, `/session` for info) — rejected: two
  commands for one job, and the info dump duplicates `/usage`.
- **Keep `/load <id>` as a non-interactive escape hatch** — rejected: with the picker in
  place it is dead weight, and the CLI has no resume flag to keep consistent.
- **pi's feature set** (type-to-search with `re:`/`"phrase"`, `Tab` scope toggling, `Ctrl+S`
  sort modes, `Ctrl+N` named-only, `Ctrl+P` path, `Ctrl+R` rename, `Ctrl+D` delete) —
  rejected: slimcode is project-scoped by ADR-0008, has no user-set session names, no forks,
  and no delete story; the picker keeps keys only.
- **pi's asynchronous loader with a `Loading n/N` header** — rejected: it would add an async
  progress path to the ADR-0013 seam to render one number.
- **`Provider::reset_usage()`** — rejected: it would zero the counter on `/new` while leaving
  `/load` showing the previous session's totals.
- **Persisting usage as a log record** (pi's model, viable thanks to ADR-0009's unknown-record
  rule) — rejected by the owner: restore starts from zero.
- **`chrono`, `jiff`, or a hand-rolled `libc::localtime_r` for local time** — rejected: the
  mtime plus the civil-date helper already in `slimcode-app` needs no dependency and no
  `unsafe`; the displayed wall clock is UTC.
- **A new `RenderItem::SessionReset`** — rejected: `/new` and `/load` are identical as far as
  the library is concerned, so `SessionChanged` just grows the reset duties.
- **Printing the session list and the repair notices into the transcript** — rejected: a
  selectable list needs state, which a stream of notices cannot carry.

## Consequences

- `SessionChanged` now clears the transcript, re-pushes the startup header, resets the footer
  usage and closes the picker, so `/new` leaves a clean, freshly-headed view. `/load` then
  **replays the loaded session's history into the transcript** (prompt boxes, assistant text,
  paired tool start/result blocks) so the screen shows the conversation again instead of ending
  at the header; usage still restarts at zero (D4). The CLI must emit `SessionChanged` *before*
  the load notices; it previously emitted the "title / skipped
  N / repaired N" notices first, where the clear wiped them — a latent bug this fixes.
- Usage numbers are per live session: resuming shows zeros until the session earns tokens
  again. Documented as a user-visible rule, not a defect.
- The library grows a view mode and one item/effect pair; the CLI keeps owning what a session
  is and which sessions exist. A future picker (input history, model choice) can reuse the
  same view shape rather than invent another.
- `SessionStore::list()` goes away in favour of a summary read, so any future caller that
  wants ids gets them from the summaries.
- Deleting `/load`/`/sessions` makes the documentation's command tables the place where a
  returning user discovers the picker; the docs and the glossary ship with the code.
