# 01: Project-scoped session store with load/list isolation

**What to build:** Session persistence becomes project-scoped. The session store
roots at the sessions directory plus a project key derived from the project home
path (git root, falling back to the OS user home; basename + first 12 hex chars
of the path's SHA-256, a deterministic function of the path). `save`, `load`,
and `list` operate only inside `<sessions>/<project-key>/`, so `/load` and
`/sessions` only ever see the current project's sessions and legacy root-level
files are never read again. The session id format is unchanged.

**Blocked by:** None (can start immediately)

**Status:** resolved

- [x] `save` writes `<sessions>/<project-key>/<id>.json` and never a root-level file.
- [x] `load` reads only the current project's directory; legacy root-level files and
      other projects' files are not found.
- [x] `list` returns only the current project's session ids.
- [x] Project key is deterministic: same path → same key, different paths → different
      keys; basename collision is disambiguated by the hash suffix.
- [x] Non-git cwd falls back to the user home project key, matching `resolve_project_home`.
- [x] CLI one-shot and TUI both route through the same project-scoped store.
- [x] Session struct and id format (`slimcode-<unix>-<pid>-<n>`) unchanged.
- [x] Tests cover the directory layout, load/list isolation, and key determinism.

## Notes

- Project key hash is FNV-1a 64 (top 48 bits, 12 hex chars) instead of the spec's
  SHA-256: crates.io was unreachable (offline), so `sha2` could not be added;
  user approved FNV-1a. Baseline-commit-only clippy `too_many_arguments` on
  `run_once` was allowed with a comment; the flaky cli env-race was fixed with a
  static test `ENV_LOCK`.
