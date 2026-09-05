# 01 — Skills support

Type: task
Status: resolved

## Question

为 slimcode 增加 skills 支持：可安装 skill，按 user / project 两个 scope 安装；
skill 与 commands 一样用 `/` 触发并参与预测提示；支持
`disable-model-invocation: true`，使该 skill 的描述不进入系统提示词（防止 agent
自动加载）。

## Spec

见 [../spec.md](../spec.md)。

## Answer

（2026-08-30，TDD 落地）实现完成，全绿：cli 53 / agent 48 / ai 16（含 2 ignored
live）/ commands 15 / common 42 tests，clippy 0 警告，fmt clean。

- `crates/common/src/skills.rs`：`Skill` / `SkillScope` / `SkillStore`（`list` /
  `inspect` / `install` / `dir_for` / `skill_dir`）+ 纯函数 `find_skill` /
  `suggest_skills` / `skill_prompt`；无新依赖的 YAML 子集 frontmatter 解析
  （`name` / `description` / `disable-model-invocation`）；user/project 发现与
  同名 project 优先；目录/单文件安装；built-in 重名校验留在 CLI（install 前
  `inspect`）。
- `crates/commands/src/lib.rs`：新增 `/skills` 与 `/install-skill` 内置命令；
  模块文档注明 skill 是独立的动态 `/` 触发集，由前端合并预测。
- `crates/cli/src/main.rs`：`build_system_prompt(skills)` 动态生成系统提示，
  仅广告 `disable_model_invocation=false` 的 skill；`run_once` / `run_repl` 接入
  `SkillStore` / 预列 skill 列表。
- `crates/cli/src/repl.rs`：`ReplCtx` 增 `skills`；`/skills`、`/install-skill`
  分派；`/name` 精确命中 → `submit_skill`（skill 正文作为用户消息跑一轮、不入
  input history）；`combined_suggestions` 合并命令 + skill；`parse_install_args`、
  `is_builtin_command` 重名校验。
- 文档同步：`CONTEXT.md`（Skill 术语）、`docs/development.md`、
  `docs/user-manual.md`、`docs/configuration.md`、`docs/adr/0002-*`。

## Comments

- 触发分派集成测试用 `http://127.0.0.1:9` 的 provider（连接被拒立即失败）证明
  `/name` 走了 prompt turn 而非 unknown command，不发起真实网络请求。
