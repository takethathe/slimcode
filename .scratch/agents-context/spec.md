# AGENTS.md 上下文文件注入（global + project，scope 标注 + 优先级声明）

Status: ready-for-agent

## Problem Statement

slimcode 目前只有两种「给 agent 的指令」：基础 system 提示（固定文案）与
按需触发的 Skill（`/skill:name`）。缺少 pi 那样的**自动注入的常驻项目指令**：
在项目的 `AGENTS.md`（以及全局的 agent 级 `AGENTS.md`）里写下工程纪律、语言规则、
提交流程等，agent 每次启动自动读到，而不是靠用户每次手动粘贴或触发 skill。

pi 的实现（`packages/coding-agent/src/core/resource-loader.ts` 的
`loadProjectContextFiles` + `system-prompt.ts` 的 `buildSystemPrompt`）：
全局文件（agentDir/AGENTS.md）+ 从 cwd 逐级向上的祖先 AGENTS.md，渲染成
`<project_context>` 包着的 `<project_instructions path="...">` 块。但 pi **不标注
scope、也不声明优先级**。

本特性在 slimcode 复刻该机制，并补上两处差异化要求，同时对项目发现范围做**简化**：
不逐级遍历所有祖先，project 只判定 **cwd 自身** 与 **git 仓库根**（最近的含 `.git`
条目的祖先目录）：
1. 注入时**标明意图**（`scope="global"` 全局 vs `scope="project"` 项目）；
2. **注明项目要求可覆盖全局要求**（冲突时以项目为准，仅声明、不做程序级合并）。

## Solution

`slimcode-common` 新增前端无关模块 `context_files`，`ContextBuilder` 增加
`with_context_files(&[ContextFile])`，CLI 与 TUI 各自接入：

- 发现：`load_context_files(home, cwd)` —— 先读全局 `<home>/AGENTS.md`
  （`$SLIMCODE_HOME` 或 `~/.slimcode`，scope `Global`）；project 只判定两个位置：
  cwd 自身，与 **git 仓库根**（最近的含 `.git` 条目的祖先目录；`.git` 可以是目录或
  `gitdir:` 文件标记，兼容 worktree/submodule），按 git 根在前、cwd 在后的顺序
  （scope `Project`）；按规范化路径去重（cwd 嵌套在 home 下时，全局文件不再重复
  作为 project）。只认 `AGENTS.md`（不含 `AGENTS.override.md` / `CLAUDE.md`，留作
  以后扩展）。发现无失败路径：文件缺失即跳过。
- 渲染：`format_context_files(&[ContextFile])` 产出 `## Project context` markdown 章节
  （与 `## Skills` / `## Tools` 同标题层级）：每个文件一个
  `<project_instructions path="…" scope="global|project">` XML 块包裹内容（path 经
  XML 转义；XML 块把内容隔离成原子单元，防止 AGENTS.md 内部的 `#` 标题/列表与外层
  markdown 冲突），段首声明 `project requirements override global requirements when
  they conflict.`；空输入返回 `""`（无任何可注入文件时整个章节省略，system 与
  未启用时逐字节一致）。注入位置：基础 system 之后、`## Skills` 索引之前（对齐 pi
  的顺序）。
- `ContextBuilder`：新增 `context_files: Vec<ContextFile>` 字段与
  `with_context_files(&[ContextFile])`；`build_system_prompt` 在 base 与 skills 之间
  插入 context files 段。恢复会话（history 非空）不重注入——与现有 skills 广告语义
  一致，无需特殊处理。
- 前端接入（各一行）：CLI `run()` 里 `load_context_files(&home, &cwd)` 后传给
  `run_once` / `run_tui`；TUI `Tui` 持 `context_files: Vec<ContextFile>`，
  `submit_prompt` / `trigger_skill` 的 `ContextBuilder` 链加 `.with_context_files`。

## User Stories

1. 作为用户，我在项目根写 `AGENTS.md`（工程纪律等），agent 每次启动自动读到。
2. 作为用户，我在 `~/.slimcode/AGENTS.md` 写跨项目习惯，所有项目都能读到。
3. 作为用户，项目在 `~` 下时，全局文件不会重复注入为项目文件。
4. 作为用户，注入的每个文件都标注其作用域（global/project），我能分辨来源。
5. 作为用户，当项目与全局要求冲突时，系统声明项目要求优先。
6. 作为用户，我在 cwd 的子目录（git 仓库内）工作时，git 根与 cwd 的 `AGENTS.md`
   都会注入；不在任何 git 仓库时只注入 cwd 的。
7. 作为开发者，无 `AGENTS.md` 时系统提示与现状完全一致（空段省略）。
8. 作为开发者，CLI 与 TUI 共享同一套发现/渲染逻辑（`context_files` 模块）。

## Implementation Decisions

- 新模块 `slimcode-common::context_files`（单文件 `context_files.rs`），导出
  `ContextScope`（`Global` / `Project`）、`ContextFile { path, content, scope }`、
  `load_context_files(home, cwd)`、`format_context_files(&[ContextFile])`。
- `ContextScope::Global/Project` 命名（用户原话「全局/项目」+ pi 术语），与
  `SkillScope::User/Project` 并存：前者讲适用/优先级层级，后者讲 skill 安装位置。
- 只认 `AGENTS.md` 一个文件名（用户原话「agents.md」）；override/CLAUDE 兼容、
  `--no-context-files` 开关均留作未来扩展，本次不做。
- 优先级只声明、不合并：两文件都是自由文本，无结构化 key；pi 同款纯注入。
- 去重用 `fs::canonicalize` 失败回退原始路径（`canonical_or_self`）。
- 不新增 ADR：可逆、对齐 pi 不惊讶、无真实权衡替代方案，不满足 ADR 三条件。

## Testing Decisions

- `context_files` 单测（临时目录端到端）：global+cwd 顺序与 scope、cwd 在 git 仓库
  子目录时 git 根 + cwd 均注入（顺序 git 根在前）、`.git` 为 `gitdir:` 文件标记时
  仍识别 git 根、cwd 即 git 根时去重只注入一次、非 git 根的普通祖先 AGENTS.md
  不注入、cwd 嵌套 home 去重、无文件返回空、渲染格式逐字符断言（markdown 章节头
  `## Project context` + scope 属性 + 优先级声明句 + 末尾无多余空行）、空输入渲染空
  （章节整体不出现）、path XML 转义。
- `context` 单测：`with_context_files` 注入在 base 与 `## Skills` 之间；未注入时
  system 不含 `## Project context` 与 `AGENTS.md`。
- 全量 `cargo test`；fmt + clippy 0 error / 0 warning。

## Out of Scope

- `AGENTS.override.md` / `CLAUDE.md` 兼容。
- `--no-context-files` 之类的开关。
- 程序级优先级合并 / 冲突检测。
- provider / wire 层改动。

## Further Notes

- 术语（见 `CONTEXT.md`，本特性已补词条）：`Context file`（发现的 `AGENTS.md`，
  scope global/project，始终注入 system）区别于 `Skill`（按需 `/skill:name` 触发）。
- 文档同步：`docs/development.md`（common 模块表 + context_files）、
  `docs/explanation.md`（新小节「AGENTS.md 上下文文件注入」）、
  `docs/user-manual.md`（新小节「AGENTS.md 上下文文件」）、`CONTEXT.md`（词条）。
- 验收：`cargo test` 全绿；`cargo fmt --all`；`cargo clippy --all-targets
  --all-features --message-format=json -- -D warnings` 0 error / 0 warning。
