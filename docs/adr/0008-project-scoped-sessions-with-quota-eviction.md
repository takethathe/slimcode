# Project-scoped session storage with quota eviction

Sessions were persisted flat at `~/.slimcode/sessions/<id>.json`, so every project's
conversations piled into one directory: `/load` and `/sessions` showed a global grab-bag,
cross-project mix-ups were one keystroke away, and nothing ever removed old files. This ADR
records the two coupled decisions: sessions are **partitioned per project**, and the
whole-store footprint is **capped by a quota with oldest-first eviction**. Because the flat
layout is abandoned outright (no read compatibility), the change is hard to reverse — an
existing `sessions/<id>.json` file is never read again and only disappears through
eviction.

## Decisions

### D1 — Per-project session directories keyed by project home

Session files live at `<home>/sessions/<project-key>/<id>.json`. The project key is derived
from the **project home** (the git root, falling back to the OS user home — the same value
the system prompt's `## Environment` reports):
`<basename>-<12 hex chars of the FNV-1a hash of the full path>`. The basename keeps the
directory readable; the path hash disambiguates same-named repositories at different paths.
`save`, `load`, and `list` all operate inside that one directory, so `/load` and `/sessions`
only ever see the current project's sessions and legacy root-level files are invisible to
them.

FNV-1a rather than the SHA-256 originally planned: the build environment had no crates.io
access, so `sha2` could not be added. FNV-1a is a stable, dependency-free algorithm (the
hash only disambiguates collisions; it is not a security boundary).

### D2 — Quota with oldest-first eviction to half the threshold

After every `save`, every session JSON file under the sessions directory (all project
subdirectories **plus** legacy root-level files) is measured. Past the quota —
`[sessions] max_mb` in `config.toml`,
default 500 MiB, no env/CLI override — session files are deleted **oldest-first by mtime**
until the total is at or below **half** the quota (250 MiB). The just-saved active session
is never evicted, and directories emptied by eviction are removed. Eviction is best-effort:
a failure never fails the `save`. Halving rather than evicting to exactly the threshold
avoids re-evicting on every subsequent save.

### D3 — Startup cleanup of empty sessions (current project only)

At startup, the current project's directory is swept once and **empty sessions** are
removed: sessions whose `messages` array is empty, zero-byte files, and unparseable JSON.
Silent and best-effort. Other projects and legacy root files are deliberately out of scope
here — they are handled by quota eviction (D2), which is why the old flat files disappear
without a migration step.

## Considered Options

- **Read-compatible migration of flat files** — rejected: the user explicitly waived
  compatibility, and a migration path would keep the legacy layout alive indefinitely.
- **SHA-256 project-key hash** — rejected for lack of offline dependency availability (D1);
  the hash is a collision guard, not a security primitive.
- **Sorting by `created_at` or the id's embedded unix timestamp** — rejected: mtime needs no
  JSON/parse pass, and "oldest" then means "least recently saved", which is the more useful
  retention rule.
- **Evicting to exactly the quota** — rejected: the next save would immediately evict again;
  half the quota gives headroom (D2).
- **Per-project quotas** — rejected for now: the quota bounds total disk usage, which is the
  stated problem; a per-project cap would let many small projects still fill the disk.

## Consequences

- `SessionStore` carries the base directory and the project key; `config::load_app_config`
  returns an `AppConfig` that carries `sessions_max_bytes` alongside the provider config.
- `/load` of an id that lives in another project (or the legacy root) now errors, because
  that file is not in the current project's directory.
- The default quota is declared once per unit — `config::DEFAULT_MAX_MB` (500 MiB) and
  `session::DEFAULT_MAX_BYTES` — with a test pinning them together.
- Docs updated: `CONTEXT.md` (Session store, Project key, Storage quota, Eviction),
  `configuration.md` (`[sessions] max_mb`), `user-manual.md` (session files),
  `development.md` (session/config module notes), `index.md` (this ADR).
