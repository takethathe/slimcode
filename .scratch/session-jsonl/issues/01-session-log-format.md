# 01: Session log format and store-side log API (expand)

**What to build:** The session store gains the ability to persist a Session as an appended JSONL
session log and read it back — **beside** the existing whole-file JSON path, which keeps serving
every caller unchanged. This is an expand step: nothing user-visible changes, no caller is switched
over yet, and the whole tree keeps compiling and passing. It is what makes the switch (ticket 03) a
mechanical call-site change. Writing: the log is created exclusively at the first assistant message
(header, a title record when the title is known, and one record for every message already in the
session history — the in-memory history is the backlog); afterwards each call appends exactly one
record; a call before the first assistant message writes nothing. Reading: the header must parse
(there is no identity to rebuild otherwise), record lines replay in order with malformed ones
skipped and counted, a torn trailing line is dropped and the file sealed with one appended newline,
and a dangling tool batch is repaired in memory only (missing tool-call ids get an
`Error: interrupted` tool result in order, orphan tool results are dropped) without touching the
file's bytes. The message model gains the optional `stop_reason` / `error` fields the log schema
carries.

**Blocked by:** None (can start immediately)

**Status:** resolved

- [x] A session log is `<id>.jsonl`: a log header line (`type`, `v`, `id`, `created_at`,
      `project_home`) followed by one record per line (`{"type":"message",…}`,
      `{"type":"title",…}`); a record with an unknown `type` is skipped.
- [x] Appending before the first assistant message writes no file.
- [x] The first assistant message creates the file exclusively (a pre-existing file is a loud
      error) with the header, a title record when the title is known, and one record for every
      message already in the session history.
- [x] After creation, each append adds exactly one record line, so a session with N messages has
      1 + N record lines (plus title records).
- [x] A title record can be appended for a title that becomes known after the log exists; loading
      resolves the title from the last title record, or none.
- [x] Loading replays records in order, skips malformed lines and reports how many were skipped,
      and reports how many tool calls were repaired.
- [x] A torn trailing line is dropped when it does not parse (kept when it is complete JSON) and the
      file is sealed with one appended newline so a later append stays line-aligned.
- [x] A missing or malformed header makes loading fail with an error naming the log.
- [x] Dangling tool batch repair is in-memory only: missing tool-call ids get an
      `Error: interrupted` tool result in order, orphan tool results are dropped, and the log's
      bytes are identical before and after a load.
- [x] The message model's optional `stop_reason` (`stop` / `tool_calls` / `error` / `aborted`) and
      `error` round-trip, and an ordinary message serializes byte-identically to before.
- [x] The existing whole-file save / load / list / startup cleanup / quota eviction paths are
      untouched and their tests still pass; no production caller uses the new API yet.
- [x] Unit tests in the `session` module cover the whole list above (unique-temp-dir pattern).

## Notes

- Design: ADR-0009 D1–D4; spec `Implementation Decisions` → 格式 / 写入 / 读取.
- Repair is deterministic and read-side only: the log keeps its dangling records forever and every
  load re-derives the same repaired history. Never write repaired records back to the file.
- The two new message fields are wire-invisible: the provider wire mapping must keep ignoring them,
  and a failure/cancel message needs a short non-empty text so no provider sees an empty assistant
  content.
