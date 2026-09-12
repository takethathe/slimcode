# 06: the layering is enforced by a test and the docs describe only it

**What to build:** The dependency directions decided in ADR-0011 become something the build checks
rather than something reviewers remember, and `development.md` stops carrying two architectures.

**Blocked by:** 02, 05

**Status:** resolved

- [x] A test in the CLI crate reads each crate's manifest and asserts the matrix **both ways** (a
      missing edge and an extra edge both fail): `ai` has no slimcode dependency; `core` depends on
      `ai` only; `app` on `ai`/`core`/`commands`; `commands` on nothing; `tui` on nothing; the CLI on
      all five.
- [x] The same test asserts the TUI source mentions none of `SessionStore`, `SkillStore` or `Config`.
- [x] The test is proven to bite: temporarily adding a forbidden dependency makes it fail, and the
      temporary change is reverted.
- [x] `development.md` loses the "实施状态" banner and the 现状 table; each module section describes
      the new layout only. Any leftover "two entry points" or old-layer narrative in `README.md`,
      `explanation.md`, `user-manual.md`, `configuration.md` and `index.md` is corrected.
- [x] `cargo test` green, `cargo fmt --all`, `cargo clippy --all-targets --all-features --
      -D warnings` clean.

## Notes

- The three-name check is deliberately coarse: application-level leakage in the TUI beyond those
  names stays a review concern.
- This ticket is the contract step: nothing is added to the product, and afterwards the architecture
  documents and the manifests agree with each other.
- Source of truth for the matrix: ADR-0011 D4; the spec's Implementation Decisions section.

## Comments

## Answer

- 新增 `crates/cli/tests/architecture.rs`（7 条测试）：手写一个小扫描器读每个 crate 的
  `Cargo.toml` **所有**依赖表（`dependencies` / `dev-dependencies` / `build-dependencies`，
  含 workspace 继承的 `x.workspace = true` 键），逐 crate 断言**恰好等于** ADR-0011 D4
  的矩阵——多一条边与少一条边都会失败：
  `ai` 无；`core` → `ai`；`app` → `ai`/`core`/`commands`；`commands` 无；`tui` 无；
  `cli` → 全部五个。另有一条递归扫描 `crates/tui/src/**/*.rs` 的测试，断言源码不出现
  `SessionStore` / `SkillStore` / `Config`。
- **证明会咬**：临时给 `crates/tui/Cargo.toml` 加 `slimcode-commands.workspace = true`，
  `cargo test -p slimcode --test architecture` 报
  `tui_depends_on_no_other_crate ... left: {"slimcode-commands"} right: {}`（6 passed /
  1 failed）；随后还原，`git diff` 为空、7 条全绿。
- 文档：`development.md` 删除「实施状态」banner 与「现状（迁移前）」表，「目标架构」段改为
  「crate 分层」并修正重命名描述，模块段标题/正文改为唯一布局（`crates/tui` 段是「终端库」、
  `crates/cli` 段是「唯一二进制 = 总入口」）。`README.md` 的 crate 表补上 `crates/tui`、
  改为六个 crate、删掉「交互式 REPL / `\` 续行」旧叙述；`docs/explanation.md` 的
  「CLI 两个入口」改为「CLI 的两个前端」；`docs/user-manual.md` 去掉「与行式 REPL 相同」
  并说明弹框候选池由 CLI 注入（ADR-0014 D3）。`docs/index.md` / `configuration.md` 无需改动
  （已与 ADR-0011–0014 一致）。
- 顺带收敛：`ContextBuilder::with_skill` 与 `UserInput::Skill` 在 ticket 05 后已无调用者
  （skill 触发文本由 CLI 用 `skills::skill_prompt` + `context::skill_loaded_in` 组装），
  一并删除，app 侧保留可复用的措辞函数与去重判定（测试改为直接覆盖 `skill_loaded_in`）。

验证：`cargo test --workspace` 全绿（cli 62 + architecture 7 + tmux 3 + ai 45 + app 190 +
commands 26 + core 67 + tui 123），`cargo fmt --all`，clippy 0 error / 0 warning。
