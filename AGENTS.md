# AGENTS.md — slimcode

slimcode：AI coding agent 项目。

## 工程纪律

### 1. 提交前：fmt + clippy

```bash
cargo fmt --all   # 直接应用格式化，不做 check、不输出 JSON
cargo clippy --all-targets --all-features --message-format=json -- -D warnings
```

- clippy 必须 0 error / 0 warning（JSON 流中不得出现 `"level":"error"` 或 `"level":"warning"`）。
- 可选（不强制）：`cargo fmt --all --check` 验证格式化状态。

### 2. 只在本地提交

只做本地 `git commit`；不 push、不新增 remote。

### 3. docs/ 文档同步

文档统一维护在 `docs/`（索引见 `docs/index.md`）。任何影响架构、接口或使用方式的改动，必须同步更新对应文档后才能提交。

### 4. 开发流程

1. **需求分析**：明确问题、边界、验收标准。
2. **计划**：拆解步骤与影响范围。
3. **TDD**：先写失败测试（red）→ 实现使其通过（green）→ 重构（refactor）。
4. **测试**：`cargo test` 全部通过。
5. **文档**：按第 3 条同步更新。
6. **review**：自查 diff，确认全部纪律满足。
7. **提交**：按第 5 条写 commit message 后本地提交。

### 5. Git 提交规范

`<type>(module): <subject>`，仅英文，按实际修改填写。

- `<type>`：`feat` / `fix` / `docs` / `refactor` / `test` / `chore` / `build` / `perf` / `ci` / `style`
- `(module)`：受影响的 Rust 模块 / crate
- `<subject>`：简短祈使句，小写开头

示例：

```
feat(agent): add prompt templating engine
fix(cli): handle empty input gracefully
test(core): cover timeout path
```

### 6. 语言规则

- 代码注释只用英文。
- `docs/` 文档可中可英，术语前后一致。

### 7. TODO.md 仅作待办输入

- 人工维护：只放尚未完成的事项，完成即删；历史交给 `git log`。
- agent 工作追踪走 `.scratch/` issue tracker（见下文 Agent skills）。

---

## Agent skills

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
