# 02: Startup empty-session cleanup (current project only)

**What to build:** On startup, silently clean empty session files inside the
current project's directory only. Empty means: session JSON whose `messages`
array is empty, a zero-byte file, or a JSON file that fails to parse. Non-empty
sessions are untouched; other projects and legacy root-level files are out of
scope for this pass. No notice, no stderr output.

**Blocked by:** 01 (project-scoped session store with load/list isolation)

**Status:** resolved

- [x] Startup triggers a one-shot cleanup of the current project's directory.
- [x] `messages: []` files are deleted; sessions with messages survive.
- [x] Zero-byte and unparseable JSON files are deleted as empty.
- [x] Only the current project's directory is scanned (no other projects, no root).
- [x] Cleanup is fully silent (no TUI notice, no CLI output), and a failure to
      clean never blocks startup.
- [x] CLI one-shot and TUI both get the startup cleanup.
- [x] Tests cover empty / zero-byte / corrupt / non-empty outcomes.
