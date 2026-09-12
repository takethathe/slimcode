# 06: the layering is enforced by a test and the docs describe only it

**What to build:** The dependency directions decided in ADR-0011 become something the build checks
rather than something reviewers remember, and `development.md` stops carrying two architectures.

**Blocked by:** 02, 05

**Status:** ready-for-agent

- [ ] A test in the CLI crate reads each crate's manifest and asserts the matrix **both ways** (a
      missing edge and an extra edge both fail): `ai` has no slimcode dependency; `core` depends on
      `ai` only; `app` on `ai`/`core`/`commands`; `commands` on nothing; `tui` on nothing; the CLI on
      all five.
- [ ] The same test asserts the TUI source mentions none of `SessionStore`, `SkillStore` or `Config`.
- [ ] The test is proven to bite: temporarily adding a forbidden dependency makes it fail, and the
      temporary change is reverted.
- [ ] `development.md` loses the "实施状态" banner and the 现状 table; each module section describes
      the new layout only. Any leftover "two entry points" or old-layer narrative in `README.md`,
      `explanation.md`, `user-manual.md`, `configuration.md` and `index.md` is corrected.
- [ ] `cargo test` green, `cargo fmt --all`, `cargo clippy --all-targets --all-features --
      -D warnings` clean.

## Notes

- The three-name check is deliberately coarse: application-level leakage in the TUI beyond those
  names stays a review concern.
- This ticket is the contract step: nothing is added to the product, and afterwards the architecture
  documents and the manifests agree with each other.
- Source of truth for the matrix: ADR-0011 D4; the spec's Implementation Decisions section.

## Comments
