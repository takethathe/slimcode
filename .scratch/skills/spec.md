# Skills support

Status: ready-for-agent

## Problem Statement

slimcode 目前只有内置 `/` 命令，无法安装、组织或按需加载可复用的 agent 指令
（skill）。用户希望像 commands 一样用 `/` 触发 skill、获得预测提示，并能按
user / project 两个 scope 安装；同时希望 `disable-model-invocation: true` 能把
skill 的描述挡在系统提示词之外，防止 agent 自动加载。

## Solution

- **Skill 格式**：`SKILL.md`，YAML 风格 frontmatter（`name` / `description` /
  `disable-model-invocation`），后接 markdown 正文；`name` 为触发名
  （`[A-Za-z0-9_-]+`，不得与内置命令重名）。
- **作用域**：user（`<home>/skills/`）与 project（`<cwd>/.slimcode/skills/`），
  同名时 project 优先。
- **安装**：`/install-skill <path> --user|--project`，目录（含 `SKILL.md`）或
  单个 markdown 文件；落为 `<scope>/skills/<name>/SKILL.md`，同名覆盖更新。
- **列出**：`/skills`。
- **触发**：`/name [任务]` 把 skill 正文作为一轮 agent 指令执行，不写入输入历史。
- **预测**：未知 `/` 前缀的 `did you mean` 与裸 `/` 提示合并内置命令与 skill。
- **disable-model-invocation**：`true` 时 skill 描述不进系统提示词，仅显式
  `/name` 触发；`false`（默认）时描述进入系统提示词供 agent 自动选用。

## User Stories

1. 作为 REPL 用户，我能把 skill 安装到 user 或 project scope，以便复用 agent 指令。
2. 作为 REPL 用户，我能用 `/skills` 看到已安装 skill 及其 scope。
3. 作为 REPL 用户，我能用 `/name` 像命令一样触发 skill。
4. 作为 REPL 用户，输入 `/` 前缀时预测提示同时包含命令与 skill。
5. 作为 skill 作者，我能用 `disable-model-invocation: true` 让描述不进系统提示词，
   仅在显式触发时生效。

## Out of Scope

- 把 skill 暴露为模型可调用的 tool（`Skill` tool）；本特性只控制系统提示词广告与
  `/` 手动触发。
- 解析 SKILL.md 正文里引用的辅助文件（当前只读取 SKILL.md 文本）。

## Implementation Decisions

- `slimcode-common::skills`：`Skill` / `SkillScope` / `SkillStore`（list / inspect /
  install / dir_for / skill_dir）+ 纯函数 `find_skill` / `suggest_skills` /
  `skill_prompt`；frontmatter 解析为无依赖的 YAML 子集（单层标量映射）。
- `slimcode-commands`：登记 `/skills` 与 `/install-skill` 两个内置命令；skill 是
  独立的动态 `/` 触发集，前端合并预测。
- `slimcode-cli`：`build_system_prompt(skills)` 动态生成系统提示（仅广告
  `disable_model_invocation=false` 的 skill）；`ReplCtx` 持有 `SkillStore`；`/name`
  命中分支、`combined_suggestions`、`parse_install_args`、安装重名校验。

## Testing Decisions

- 外部行为优先：`SkillStore` 的解析/发现/安装用临时目录端到端验证；
  REPL 用脚本化输入验证 `/skills`、`/install-skill`、预测提示与 skill 触发分派。
- 触发分派测试用 `http://127.0.0.1:9` 的 provider，连接被拒即证明确实走了
  prompt turn 而非 unknown command。
