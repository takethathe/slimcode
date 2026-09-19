# 02: Per-message agent event and runner sink (expand)

**What to build:** The agent loop reports every message that enters history, so a session layer can
persist it at the moment it exists. An assistant message is announced once it is assembled, and
every tool result is announced as it is pushed. The shared runner forwards that event to a sink the
caller provides. Nothing consumes the event yet: the transcript renders exactly as before and no
disk I/O is added, so this is an expand step that can land independently of the storage work. The
agent core keeps knowing nothing about sessions or files — it only emits an event carrying the
message.

**Blocked by:** None (can start immediately)

**Status:** resolved

- [x] The loop emits one event carrying the assembled assistant message, after that message enters
      history.
- [x] The loop emits one event per tool result, after that result enters history (serial and
      parallel execution alike).
- [x] The shared runner forwards the event to a sink the caller injects, without mapping it to a
      display item and without touching disk.
- [x] Existing events (`Turn`, `Stream`, `ToolStart`, `ToolResult`, `Stop`) and the rendered
      transcript are unchanged; existing agent and runner tests still pass.
- [x] A scripted-fake-provider test pins the emitted event sequence for a tool-calling turn.
- [x] The agent crate still has no filesystem dependency.

## Notes

- Design: ADR-0009 D2 and D5; spec `Implementation Decisions` → 前端接线. The shape follows pi's
  `message_end`: the agent emits, the session layer persists.
- This event is the seam ticket 03 appends from; keep it free of storage concerns so the agent stays
  frontend- and storage-agnostic (ADR-0004).
