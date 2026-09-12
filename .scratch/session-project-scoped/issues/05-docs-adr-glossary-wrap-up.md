# 05: Docs, ADR, and glossary wrap-up

**What to build:** Finalise the feature's documentation: update the user manual
(session file section: directory layout + eviction behaviour), the
configuration doc (`[sessions] max_mb`), and the development doc (common module
table + session description); add CONTEXT.md glossary entries for the new
concepts (Session store / Storage quota / Eviction, refined via domain
modeling); add an ADR recording the project-scoped layout plus quota eviction
(layout change is hard to reverse, surprising without context, and the
basename+hash naming / mtime ordering were real trade-offs). Final gate:
`cargo test` green, `cargo fmt --all`, clippy 0 error / 0 warning.

**Blocked by:** 01, 02, 03, 04

**Status:** resolved

- [x] User manual describes the project-scoped layout and eviction behaviour.
- [x] Configuration doc documents `[sessions] max_mb`.
- [x] Development doc reflects the session module changes.
- [x] CONTEXT.md gains glossary entries for the new terms.
- [x] ADR added under the ADR convention.
- [x] `cargo test` green; `cargo fmt --all`; clippy 0 error / 0 warning.
