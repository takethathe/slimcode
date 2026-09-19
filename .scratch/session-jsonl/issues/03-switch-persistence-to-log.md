# 03: Switch session persistence to the appended log

**What to build:** Sessions stop being whole-file JSON rewrites and become appended JSONL logs, from
the first turn onwards. Submitting a prompt pushes the turn's system message (first turn only) and
the prompt into the session history and appends them, which is what seeds the log; during the turn
every message is appended the moment it enters history, so a crash or a kill keeps every prior turn
and the completed part of the current one, and a session log grows one line at a time. A turn that
fails or is cancelled appends a closing assistant message carrying the stop reason, and the live
session adopts the partial turn either way, so memory never lags behind the log. Loading replays the
log (skipping bad records, sealing a torn tail, repairing dangling tool batches in memory) and tells
the user when it skipped or repaired something. Only `.jsonl` logs are listed and loaded; legacy
`.json` sessions disappear from `/load` and `/sessions` while still counting toward the disk quota,
and `/save` is removed since an append-only log has nothing left for it to do.

**Blocked by:** 01 (session log format and store-side log API), 02 (per-message agent event and
runner sink)

**Status:** resolved

- [x] A new session's first turn creates `<id>.jsonl` when the first assistant message arrives,
      containing the log header, the title record and one record per message already in history.
- [x] Every message that enters history during a turn is appended as it enters (system message and
      prompt at submit, assistant when assembled, each tool result as it completes), so `tail -f`
      shows records appearing before the turn ends.
- [x] A turn that fails or is cancelled appends a closing assistant message (`stop_reason` =
      `error` / `aborted`, `error` set, non-empty text) and the live session adopts the partial turn
      plus that message; the log never ends on a dangling tool batch or a trailing tool result.
- [x] A turn that fails before its first assistant message leaves no session file on disk.
- [x] The system message and prompt are pushed into the session history before the context is built,
      so memory, log and context are one message sequence (no message built or persisted twice).
- [x] `/load` restores the replayed history and reports skipped records and repaired tool calls as a
      notice; a log with a bad record line still loads, one with a torn last line loads and gets
      sealed.
- [x] Listing, path resolution and the startup sweep operate on `.jsonl` only; the startup sweep
      deletes zero-byte logs and logs that replay to no assistant record.
- [x] The quota still bounds the store: `.jsonl` and legacy `.json` bytes are counted together,
      oldest-first by mtime, down to half the quota, never the active session.
- [x] The old whole-file save and load paths are deleted along with every caller; the whole-file
      `save(&Session)` API no longer exists.
- [x] `/save` and its effect are removed (the command table entry, the dispatch arm and the
      completion-popup tests that used it as a sample command); `/load`, `/sessions`, `/new` still
      work.
- [x] Legacy `.json` sessions are neither listed nor loaded, and are still removed by quota
      eviction.
- [x] `cargo test` passes; manual acceptance: watch records appear line by line, `kill -9` a turn
      mid-tool-batch and confirm the completed tool results are on disk, and `/load` that session.

## Notes

- Design: ADR-0009 D2, D5, D6; spec `Implementation Decisions` → 前端接线 / store 表面.
- Keep the TUI wiring thin (it is not covered by tests): the worker thread appends from the event
  sink, the main thread adopts the session the worker returns. The store is shareable as-is.
- The `session` module's own tests from ticket 01 already pin the format and the read-side rules;
  this ticket adds the wiring, the closing message and the removed command.
