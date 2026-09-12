# 05 (S5): dependency-matrix test and docs rewrite

**What to build:** make ADR-0011's layering a thing the build enforces, and finish the docs so
`development.md` describes only the new layout.

**Blocked by:** 02, 04

**Status:** ready-for-agent

- [ ] Add `crates/cli/tests/architecture.rs`: read each crate's `Cargo.toml` (`toml` as a
      dev-dependency) and assert the matrix — `ai`: no `slimcode-*` dependency; `core`: exactly
      `ai`; `app`: `ai`/`core`/`commands`; `commands`: none; `tui`: **none**; `cli`: all five.
      Assert both directions (no missing edge, no extra edge) so the test fails when a dependency is
      added as well as when one disappears.
- [ ] Assert in the same test that `crates/tui/src` mentions no `SessionStore`, `SkillStore` or
      `Config` (a coarse `read_dir` + substring check is enough).
- [ ] Delete `docs/development.md`'s 现状 table and the "实施状态" banner, rewrite each module
      section (`crates/ai` / `crates/core` / `crates/app` / `crates/commands` / `crates/tui` /
      `crates/cli`) to the new layout, and drop the reference to `.scratch/arch-realignment/` once
      S1–S4 are done.
- [ ] Sweep the remaining docs for the old vocabulary: `user-manual.md` / `explanation.md` /
      `README.md` must not call the TUI an entry point or a frontend-with-services, and
      `CONTEXT.md`/`docs/index.md` must agree with the final state.

- [ ] `cargo test` green (including the new test, which must fail if the matrix is violated),
      `cargo fmt --all`, `cargo clippy --all-targets --all-features -- -D warnings` clean.

## Notes

- Verify the test actually bites: temporarily add e.g. `slimcode-core` to `crates/tui/Cargo.toml`,
  confirm the test fails, then revert.
- Acceptable residual: application-level leakage inside `crates/tui/src` beyond the three checked
  names is a review concern, not a test concern.

## Comments
