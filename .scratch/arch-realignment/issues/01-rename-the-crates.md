# 01: rename the crates so the names say what they are

**What to build:** The workspace compiles and its whole test suite passes on the final crate names:
`slimcode-agent` → `slimcode-core`, `slimcode-common` → `slimcode-app`. The binary stays `slimcode`
and nothing observable changes — this is the prefactor that lets every later ticket work against the
names the target architecture uses.

**Blocked by:** None (can start immediately)

**Status:** resolved

- [x] Both crate directories, `[package] name`, the workspace `members` and
      `workspace.dependencies` entries, every dependency key in the other crates, and every
      `use` path across the workspace say `slimcode-core` / `slimcode-app`.
- [x] No behavior change: the one-shot output and the TUI test suite are untouched apart from import
      paths; the binary is still `slimcode` and `--help` still prints the same usage.
- [x] `cargo test` green, `cargo fmt --all`, `cargo clippy --all-targets --all-features --
      -D warnings` clean.
- [x] Docs that name the crates are renamed with them: `development.md` (the 现状 table rows and
      every module heading), `README.md`, `explanation.md`, `user-manual.md`, `configuration.md`.
      The target-architecture note at the top of the architecture section keeps pointing at the
      still-open tickets.

## Notes

- Deliberately one atomic ticket rather than expand–contract: the blast radius is six manifests and
  a few hundred import lines, one commit compiles once, and CI stays green.
- No type moves here — `Provider`/`Message` stay where they are until ticket 02, so this ticket is
  purely mechanical and cheap to review.

## Comments

## Answer

已实现：`crates/agent` → `crates/core`（包名 `slimcode-core`）、`crates/common` → `crates/app`
（包名 `slimcode-app`），同步 workspace `members`/`workspace.dependencies`、各 crate 的依赖键与全仓
`use` 路径。纯机械改动，无类型搬动（`Provider`/`Message` 仍留在原处，ticket 02 处理）。

验证：`cargo test --workspace` 全绿（44 + 2 + 38 + 191 + 26 + 132 + tmux 冒烟等），`cargo fmt --all`
已应用，`cargo clippy --all-targets --all-features -- -D warnings` 0 error / 0 warning。二进制仍是
`slimcode`，`--help` 用法未变。文档已同步：`development.md`（现状表与各模块标题）、`README.md`、
`explanation.md`、`user-manual.md`、`configuration.md`；顶部“实施状态”注记仍指向未完成 ticket。
