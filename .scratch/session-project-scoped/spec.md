# Session 落盘按 Project 分区 + 磁盘配额清理

Status: ready-for-agent

## Problem Statement

slimcode 目前把所有 Session 扁平存于 `~/.slimcode/sessions/<id>.json`，不同项目的会话
全部混在一起。`/load` 与 `/sessions` 看到的也是全局大杂烩：用户在项目 A 里很容易误加载
项目 B 的会话，会话文件会无限累积、磁盘占用不受控。此外，改版前的旧落盘文件（不带
project 层级）没有独立清理路径，会永久残留。

## Solution

Session 落盘改为按 Project 分区：每个 project 一个子目录，目录名是 project home 路径的
确定性函数（basename + 路径 SHA-256 前 12 位），`save` / `load` / `/sessions` 只操作
当前 project 的子目录，不再读取旧根目录文件（不做兼容迁移）。同时引入磁盘配额清理：
启动时静默清理当前 project 的空 Session 文件；每次 `save` 后检查整个 sessions 目录总
占用，超过阈值（默认 500 MiB，可经 `config.toml` 的 `[sessions] max_mb` 配置）就按文件
mtime 从最旧删除，直到总占用降到阈值一半（250 MiB），跳过当前活跃会话；旧根目录文件
由此被一并自动清掉。

## User Stories

1. As a TUI 用户, I want 我的会话按项目分目录落盘, so that 不同项目的会话互不混杂。
2. As a TUI 用户, I want `/load` 只加载当前项目的会话, so that 我不会误加载其他项目的会话。
3. As a TUI 用户, I want `/sessions` 只列出当前项目的会话 id, so that 列表与 `/load` 的
   可见范围一致、不会出现无法加载的条目。
4. As a 用户, I want 在不同路径的同名 git 仓库里工作时会话互不串扰, so that 每个仓库
   的会话独立（目录名含路径 hash，避免 basename 碰撞）。
5. As a 用户, I want 在非 git 目录工作时会话仍能保存与恢复, so that 行为与
   `## Environment` 的 project home 语义一致（fallback 到 user home，所有非 git 目录
   共享一个 project 目录）。
6. As a TUI 用户, I want 启动时自动清理当前项目内的空会话文件, so that 新建但没对话的
   会话不积攒垃圾。
7. As a TUI 用户, I want 启动清理完全静默, so that 启动体验不被 notice 打扰。
8. As a 用户, I want 会话总占用超过阈值（默认 500 MiB）后自动按最旧清理到阈值一半,
   so that 磁盘占用长期可控。
9. As a 用户, I want 超阈值清理跳过当前活跃会话, so that 正在进行的会话不会被删除。
10. As a 用户, I want 旧的根目录落盘文件在超阈值清理时被一并清除, so that 改版前的
    遗留文件自动消失、无需手动处理。
11. As a 用户, I want 0 字节或 JSON 损坏的会话文件也被视为空文件清除, so that 不可恢复
    的垃圾不残留。
12. As a 用户, I want 通过 `config.toml` 的 `[sessions] max_mb` 调整配额阈值, so that
    磁盘预算可按需配置（默认 500）。
13. As a 用户, I want 会话 id 格式保持不变, so that `/load <id>` 的既有用法与终端标题
    显示不受影响。
14. As a one-shot CLI 用户, I want 一次性运行的会话也按当前项目落盘, so that 与 TUI
    的存储语义一致。
15. As a 维护者, I want 清理失败不影响 `save` 成功, so that 存储异常不会破坏会话保存
    这一主路径。
16. As a 维护者, I want 被清空的 project 子目录被移除, so that sessions 目录下不堆积
    空壳目录。
17. As a 用户, I want `/load` 一个不属于当前项目的 id 时得到清晰报错, so that 我知道
    该会话不在当前项目里。
18. As a 用户, I want 同一项目路径每次启动都落到同一个 project 目录, so that 会话跨
    进程可稳定恢复（目录名是路径的确定性函数）。

## Implementation Decisions

- **模块**：`slimcode-common` 的 `session` 模块（`SessionStore`）是唯一承载行为的模块；
  CLI 与 TUI 只做接线（计算 project key、传阈值、启动时调一次空清理）。
- **目录结构**：`sessions/<project-key>/<id>.json`；`SessionStore` 持有 base 目录 +
  当前 project key；`session_path` 在 project 子目录下解析。Session 结构与 id 格式
  （`slimcode-<unix>-<pid>-<n>`）不变。
- **project key**：由 `resolve_project_home(cwd)`（git root，非 git fallback 到 OS
  user home，与现有 `## Environment` 语义一致）的结果计算：`basename + '-' +
  SHA-256(路径) 前 12 位 hex`。basename 原样保留（含空格/点开头，PathBuf 天然支持），
  无需 sanitize。
- **save**：`create_dir_all` project 子目录 → 写 JSON → 触发超阈值检查。
- **启动空清理**：扫描当前 project 子目录内所有 `.json`，`messages` 为空数组、0 字节、
  或 JSON 解析失败的视为空文件删除；完全静默（无 notice、无 stderr 输出）。只清当前
  project，不扫其他 project 与根目录。
- **超阈值清理**（save 后自动）：递归统计整个 sessions 目录（所有 project 子目录 +
  根目录旧文件）的总字节；超过 `max_mb` 后按文件 mtime 升序（最旧优先）删除 `.json`，
  直到总占用 ≤ `max_mb / 2`；跳过当前活跃 session 的文件；删除后对空出的 project
  目录做非递归 `remove_dir`（非空则忽略失败）。清理失败降级为忽略，不影响 save 结果。
- **阈值**：单位 MiB，默认 500（= 524,288,000 字节）；`config.toml` 新增 `[sessions]
  max_mb`（u64），无 env / CLI 覆盖，融入现有四层解析（file > default）。
- **load / list**：只读取当前 project 子目录；根目录旧格式文件完全不在查找范围内。
- **兼容性**：不做任何旧格式迁移；旧文件仅通过超阈值清理自然消亡。

## Testing Decisions

- **好测试的标准**：只断言外部可观察行为——文件落在哪个目录、`load` 能否读到、清理后
  哪些文件还在/不在、总占用降到多少；不测内部实现（如循环结构、排序细节的函数级暴露）。
- **唯一 seam**：`SessionStore` 单元测试（`session.rs` 的 `#[cfg(test)]`，使用
  `unique_temp_dir` 保证并行安全）。行为逻辑与前端无关，全部收敛在存储层；CLI/TUI 只
  传参，不在 CLI/TUI 层新增测试（现有 `run_isolated` 端到端测试不动，TUI 无测试基建）。
- **覆盖清单**：
  - project 子目录落盘（save 后文件在 `<base>/<key>/`，根目录无文件）；
  - load 隔离：根目录旧文件与他 project 文件不被当前 store 读到；
  - list 只返回当前 project 的 id；
  - 空文件清理：messages 空 / 0 字节 / 损坏 JSON 均被删，非空保留；
  - 超阈值清理：构造小阈值场景，断言按 mtime 最旧先删、删到 ≤ 一半、当前 session 被
    跳过、旧根目录文件被清、空 project 目录被移除；
  - project key 确定性：同路径两次计算同 key，不同路径不同 key。
- **先例**：`session.rs` 现有 round-trip / list / invalid-id 单测（`unique_temp_dir`
  模式）；`agents-context` 特性同样把逻辑收敛在 common 层单测。

## Out of Scope

- 旧格式会话迁移 / 兼容读取（用户明确不需要，直接废弃）。
- 按 project 独立的配额（阈值是全局总占用）。
- env / CLI flag 覆盖 `max_mb`。
- 清理通知 UI（启动清理完全静默；save 后清理也不提示）。
- TUI / CLI 层新增测试基建与 UI 改动。
- 会话内容压缩 / 增量存储 / 归档格式变化。

## Further Notes

- **文档同步**：`docs/user-manual.md`（会话文件章节：目录结构、配额清理行为）、
  `docs/configuration.md`（`[sessions] max_mb`）、`docs/development.md`（common
  模块表 + session 描述）、`CONTEXT.md`（新增词条：Session store / Storage quota /
  Eviction，或按 domain-modeling 精化后的命名）。
- **ADR**：建议新增 ADR 记录「session 按 project 分区 + 配额清理」——满足 ADR 三条件
  （目录结构改动难以回退；未来读者会惊讶为何按 project 分区、为何默认 500 MiB、为何
  清到一半；basename+hash 命名与 mtime 排序是真实权衡）。
- **验收**：`cargo test` 全绿；`cargo fmt --all`；`cargo clippy --all-targets
  --all-features --message-format=json -- -D warnings` 0 error / 0 warning。
