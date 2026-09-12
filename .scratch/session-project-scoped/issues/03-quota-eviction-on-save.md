# 03: Quota eviction on save (oldest-first, to half the threshold)

**What to build:** After each `save`, check the total byte size of the whole
sessions directory (all project subdirectories plus legacy root-level files).
When it exceeds the threshold — initially a hardcoded default of 500 MiB,
parameterised later in ticket 04 — delete session files oldest-first by file
mtime until the total is at or below half the threshold. The current active
session is skipped. Project directories emptied by eviction are removed
(non-recursive, ignoring failures). Eviction failures never fail the `save`.
Legacy root-level files are thus cleaned up automatically.

**Blocked by:** 01 (project-scoped session store with load/list isolation)

**Status:** resolved

- [x] `save` triggers a total-size check of the whole sessions directory.
- [x] Over the threshold (default 500 MiB), files are deleted by mtime, oldest
      first, until total ≤ half the threshold (250 MiB default).
- [x] The current active session file is never evicted.
- [x] Legacy root-level files participate in the size accounting and get evicted.
- [x] Directories emptied by eviction are removed (empty only, failures ignored).
- [x] Eviction failure leaves the `save` result untouched (best-effort).
- [x] Below the threshold, no files are deleted.
- [x] Tests cover oldest-first ordering, half-threshold target, skip-active,
      root-file cleanup, empty-dir removal, and best-effort failure.
