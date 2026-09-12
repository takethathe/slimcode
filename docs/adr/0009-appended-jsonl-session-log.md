# Appended JSONL session logs

Sessions were persisted as a whole-file pretty-printed JSON document: every save re-serialized the
entire history and rewrote `<id>.json` with `fs::write`, i.e. truncate-then-write. Two consequences
were accepted until now and are not acceptable any more: a crash (or a full disk) in the middle of a
rewrite loses the whole session, not just the turn being saved; and every turn pays a write
proportional to the entire history. pi (the reference coding agent) persists sessions as
**append-only JSONL**: one line per record, appended the moment a message enters history, a log
rather than a snapshot. This ADR adopts that discipline together with the read-side rules a growing
log needs — lenient parsing, tail sealing, in-memory repair of dangling tool batches — and records
why the legacy `<id>.json` layout is abandoned rather than migrated.

## Decisions

### D1 — One appended log per Session: header line plus typed records

A session log lives at `<home>/sessions/<project-key>/<id>.jsonl` (same project-scoped directory as
before). Line 1 is the **log header**
`{"type":"session","v":1,"id":"…","created_at":"…","project_home":"/abs/path"}`; every following
line is one typed record, `{"type":"message","message":{…}}` or `{"type":"title","title":"…"}`.
Records with an unknown `type` are ignored, so new record kinds can be added without breaking older
readers. Records carry no `id`/`parentId`/timestamp: slimcode's history is linear, and pi's tree
fields exist to serve `/tree`-style branching, which slimcode does not have. The header records the
**project home** (the glossary term; the same value the project key is derived from) rather than
pi's `cwd`, and is informational — the project-key directory, never the header, decides where a
session is found. The title is a record, not a header field: an append-only log has no mutable
field, so a title that becomes known later stays representable.

### D2 — Append per message, creating the log lazily at the first assistant message

The log is appended once per message at the moment the message enters history: the system message
and the prompt at submit time (the system message only on the first turn), the assistant message
when it is assembled, and each tool result as it completes. A crash therefore keeps every prior
turn and the completed part of the current one.

The log is created only when the first **assistant** message arrives (pi's rule). At that point the
store writes the header, a title record when the title is known (the title is inferred from the
first prompt, before the first assistant message, so it always is), and one record for every message
already in `session.messages` — the in-memory history is the backlog. Creation uses an exclusive
create so a second writer fails loudly instead of interleaving. Before creation, appends are
no-ops: a turn that dies before its first assistant message leaves no file at all.

Writes are not validated (the caller hands the store exactly the message that just entered
history), not fsynced (a process crash already preserves everything appended; power loss is not the
threat being defended), and never rewrites the file.

### D3 — Lenient read: skip bad records, seal a torn tail

`load` parses the header first — a missing or malformed header is an error, because there is no
identity to rebuild — then replays records in order, silently skipping malformed ones and counting
them so the frontend can report "skipped N records". A torn trailing line (the file does not end
with a newline) is dropped when it does not parse, and the file is **sealed** by appending a single
newline so later appends stay line-aligned; a torn line that happens to be complete JSON is kept.

Skipping beats refusing: one bad line must not make a session unloadable, and the count is surfaced
rather than hidden. This is pi's behaviour on the layer that writes the user-visible session files,
and it is why the storage layer never has to validate on write.

### D4 — Dangling tool batches are repaired in memory on load

For every assistant message whose `tool_calls` lack results — a crash or a cancel in the middle of
a tool batch — the missing ids get an `Error: interrupted` tool result inserted in order after the
batch's existing results, and tool results with no matching `tool_calls` are dropped. The log file
is never rewritten: correctness is a property of `load`'s output, not of the file, and each load
repairs deterministically. This keeps the replayed history valid for providers that require every
`tool_call_id` to be matched, without write-side batch buffering (which would discard completed
tool results) and without embedding fabricated data in the file.

### D5 — A failed or cancelled turn is closed by an assistant message

`Message` gains optional `stop_reason` and `error` fields (omitted from the wire when absent, so the
byte shape of ordinary messages is unchanged). A turn that errors or is cancelled appends an
assistant message carrying that reason, so a log left mid-batch is closed on an assistant boundary
rather than exposing a trailing tool result or an unmatched tool call. The in-memory session
advances with the log on failure too: the frontend adopts the turn's partial messages plus that
closing message, because the log has already recorded them and memory must not lag behind disk.

### D6 — Store surfaces follow the extension

`session_path` and `list` use `.jsonl`. The startup sweep is narrowed to the current project's
**empty session logs** — zero-byte files, and logs that replay to no assistant record, which can
only be residue of a crash during log creation. Quota eviction still measures the whole store and
deletes oldest-first by mtime down to half the threshold, skipping the active session; legacy
`<id>.json` files remain invisible to `/load` and `/sessions` but still count toward the quota, so
they disappear through eviction. `/save` is deleted along with its `Effect`: a pure append log has
already written everything a save could write.

## Considered Options

- **Snapshot per line** (each line a full session state, load reads the last line) — rejected: every
  turn would still serialize and write the whole history, so the write-volume motivation is unmet,
  and the file would grow as the sum of all past states.
- **Write-time prefix validation** (`save` re-reads the log, checks `messages[..n]`, rewrites on
  mismatch) — rejected: the writer knows exactly which messages are new; validating the caller's own
  invariant buys nothing and turns the store into a state machine.
- **A `persisted_len` watermark on `Session`** — rejected: persistence progress is a store concern,
  and every construction site would have to initialize it correctly or corrupt the log.
- **Batch-atomic tool results** (buffer the assistant message with its batch) — rejected in favour
  of D4: it discards completed tool results on every crash/cancel, and per-message durability is the
  point of the change.
- **Repairing a torn tail by atomic rewrite** (pi's newer harness storage) — rejected: appending one
  newline is a zero-risk seal that never touches existing bytes, and a rewrite on the read path
  makes `load` a fallible write operation.
- **Legacy `.json` read compatibility with migrate-on-save** — rejected by the user: the old layout
  is abandoned outright and its files are removed by quota eviction. No startup migration, no dual
  read paths.
- **pi's tree structure, `seq`/transaction storage layer, compaction/fork entries, per-record
  timestamps, `fsync` per append** — rejected as complexity for problems slimcode does not have
  (branching, fork snapshots, SDK-level state, power-loss durability).

## Consequences

- `SessionStore::save(&Session)` is replaced by `append(&Session, &Message)` (append or, on the
  first assistant message, create the log with the header, title and backlog) and `append_title`;
  `load` returns the rebuilt session plus how many records were skipped and how many tool calls were
  repaired.
- `slimcode-agent`'s loop emits a new per-message event (`AgentEvent::Message`) so the session layer
  can persist messages as they enter history; the runner forwards it to a sink and the TUI's worker
  thread appends to the store. The agent core still performs no disk I/O (ADR-0004's division of
  responsibilities holds).
- `/save` and `Effect::SaveSession` are removed: with an append-only log the command has nothing
  left to do.
- A session's on-disk records and its in-memory messages can differ in exactly two ways: repaired
  dangling tool batches exist only in memory, and pre-creation messages exist only in memory until
  the log is created.
- Docs updated: `CONTEXT.md` (Session, Session log, Log header, Session store, Dangling tool batch,
  Eviction), `user-manual.md` (session files, commands), `configuration.md` (quota),
  `development.md` (common module notes), `README.md`, `index.md` (this ADR).
