# AGENTS.md — slimcode

slimcode：Rust 实现的 coding agent。

## 改动流程

每步的完成判据（gate）过了才进下一步：

1. **需求分析** — 写清问题、边界、验收标准。
2. **计划** — 读 `CONTEXT.md` 与涉及的 `docs/adr/`，列出步骤与影响范围。
3. **TDD** — red → green → refactor。
4. **测试** — `cargo test` 全绿。
5. **文档** — 影响架构 / 接口 / 用法的改动，同步更新 `docs/` 里对应的文档（索引 `docs/index.md`）。
6. **review** — 逐条对照本文件，确认每条纪律都已在 diff 中满足。
7. **提交** — 按《提交规则》写 message，然后本地提交。

### 提交规则

先过 gate：

```bash
cargo fmt --all
cargo clippy --all-targets --all-features --message-format=json -- -D warnings
```

clippy 的 JSON 流中 0 个 `"level":"error"` / `"level":"warning"`。

- **只落本地**：`git commit` 就是终点，remote 不动。
- **合入**：分支与 patch 只有 `git rebase` 和 `git cherry-pick` 两条路（`--onto`、`git rebase -i` 整理历史同理）；同步远端用 `git pull --rebase`；`git merge` 一律不用，包括 `--no-ff` 与 `pull` 的默认 merge 行为。
- **message**：`<type>(module): <subject>`，仅英文，按实际修改填写。`<type>` 取 `feat` / `fix` / `docs` / `refactor` / `test` / `chore` / `build` / `perf` / `ci` / `style`；`(module)` 是受影响的 Rust 模块 / crate；`<subject>` 是简短祈使句，小写开头。

## 常驻约定

- 代码注释只用英文；`docs/` 文档中英皆可，术语前后一致。
- `TODO.md` 只放未完成的人工待办，做完即删，历史交给 `git log`；agent 的待办与规格走 `.scratch/` issue tracker（见下）。

---

## Agent skills

> `/{name}` 是 skill 的触发写法，例如 `/code-review`。

### Issue tracker

Issues and specs live as markdown files under `.scratch/`. See `docs/agents/issue-tracker.md`.

### Triage labels

Default triage roles. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: root `CONTEXT.md` + `docs/adr/`. See `docs/agents/domain.md`.

<!-- CODEGRAPH_START -->
## CodeGraph

In repositories indexed by CodeGraph (a `.codegraph/` directory exists at the repo root), reach for it BEFORE grep/find or reading files when you need to understand or locate code:

- **MCP tool** (when available): `codegraph_explore` answers most code questions in one call — the relevant symbols' verbatim source plus the call paths between them, including dynamic-dispatch hops grep can't follow. Name a file or symbol in the query to read its current line-numbered source. If it's listed but deferred, load it by name via tool search.
- **Shell** (always works): `codegraph explore "<symbol names or question>"` prints the same output.

If there is no `.codegraph/` directory, skip CodeGraph entirely — indexing is the user's decision.
<!-- CODEGRAPH_END -->
