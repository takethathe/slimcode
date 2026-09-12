# Session 落盘改为追加式 JSONL 日志

Status: ready-for-agent

决策记录见 [ADR-0009](../../docs/adr/0009-appended-jsonl-session-log.md)。

## Problem Statement

Session 现在整份存成 pretty JSON，每次保存都重新序列化全部历史并覆写 `<id>.json`
（先截断再写）。用户因此承受两件不该承受的事：

1. **一次崩溃吃掉整个会话**：写到一半进程挂掉（或被 kill、或磁盘写满），丢的不是"正在保存的
   那一轮"，而是整个会话——用户几小时的工作可能一起消失。
2. **每轮保存的代价与历史等长**：回合末把整份历史重新序列化并写出，长会话越用越慢，而这些
   字节里绝大多数上一轮已经写过。

此外，模型/供应商对 `tool_call_id` 配对与会话尾部状态有硬约束：assistant 消息里的每个
`tool_calls` 必须有匹配的 tool result，历史不能停在"模型请求了工具但结果缺失"的状态上。现在
的覆写模型下，取消恰好发生在工具批次中间时，部分历史被整体写进文件，恢复出来就是会被
provider 拒绝的会话。

## Solution

按 pi（参考实现）的做法，把 session 落盘改成 **追加式 JSONL 日志**：

- 每个 Session 一个 `<id>.jsonl`：首行 header（身份 + 格式版本 + project home），其后**一行
  一条记录**（`message` / `title`），只追加、永不重写。
- **每条 message 进入 history 的瞬间就落盘**（system / prompt → assistant → 每条 tool
  result），崩在回合中途只丢当前这一批，之前的一切都在盘上。
- **lazy create**：文件只在第一条 assistant 消息出现时创建；此前 append 是 no-op，所以首条
  assistant 之前就失败/取消的回合不留任何文件，也不会有空壳会话混进 `/sessions`。
- **写时不校验、不 fsync**：调用方给的就是"刚进 history 的新消息"，存储层不做前缀校验、不做
  全量重写。
- **读侧宽容**：坏记录行静默跳过并计数（`/load` 给 notice），末尾被截断的半行丢弃并补一个
  换行"封口"；**悬空工具批次在内存里修补**（缺的 id 补 `Error: interrupted`、孤儿 result
  丢弃），日志文件不动。
- **失败/取消由一条 assistant 消息收尾**，内存会话在失败时也前进到日志已达的位置，内存与
  磁盘不脱节。
- 旧 `<id>.json` 不做兼容：`/load`、`/sessions` 看不见它们，只由配额淘汰顺带清除。
- `/save` 命令删除：纯追加日志下它已经无事可做。

## User Stories

1. As a TUI 用户, I want 每条消息一进入对话就写进会话文件, so that 进程崩溃或被 kill 时我最多
   丢当前这一批工具调用，而不是整个会话。
2. As a TUI 用户, I want 工具结果一执行完就落盘, so that 长批次跑到一半崩溃时，已经跑完的工具
   结果仍然保得住。
3. As a TUI 用户, I want 首条 assistant 消息之前就失败/取消的回合不留任何文件, so that 失败的
   尝试不会在 `/sessions` 里堆出打不开的空壳会话。
4. As a TUI 用户, I want 崩在工具批次中间时已完成的工具结果留在文件里, so that 恢复后我能看到
   崩溃前工具到底做了什么。
5. As a TUI 用户, I want `/load` 出来的历史与崩溃前一致, so that 我能接着上次的位置继续。
6. As a TUI 用户, I want 会话文件含一行坏数据时整个会话仍然能加载, so that 一次局部损坏不会让
   我彻底失去这个会话。
7. As a TUI 用户, I want 跳过坏行时看到一条 notice, so that 我知道历史里少了东西，而不是被静默
   糊弄过去。
8. As a TUI 用户, I want 被 Ctrl+C 打断在工具批次中间的历史仍然合法, so that 我下一次发 prompt
   不会被 provider 以 tool_call_id 不匹配为由拒绝。
9. As a TUI 用户, I want 取消/失败的回合在会话里留下一条 assistant 消息收尾, so that 会话不会
   停在"模型请求了工具但没有结果"的不合法状态。
10. As a TUI 用户, I want 失败之后立刻 `/load` 得到的历史与当前会话内存里的历史一致, so that
    恢复出来的东西不会比屏幕上看到的更多或更少。
11. As a TUI 用户, I want 失败原因仍然出现在 notice 里, so that 我知道这一轮为什么结束。
12. As a TUI 用户, I want 会话文件一行一条记录, so that 我能用 `tail -f` 实时看、用 `jq` 解析、
    用 `git diff` 看历史变化（而不是每次一个整份覆写的大 JSON）。
13. As a 用户, I want 会话文件里记录它属于哪个 project, so that 文件被复制或搬迁后仍能看出来历。
14. As a 用户, I want 升级后旧 `.json` 会话不出现在 `/load`/`/sessions` 里, so that 我不会加载
    已废弃格式的文件。
15. As a 用户, I want 旧 `.json` 文件仍然计入磁盘配额, so that 碎片最终会被自动清掉而不需要我手动
    处理。
16. As a 用户, I want `/save` 从命令列表消失, so that 我不会去按一个按了什么都不会发生的命令。
17. As a 用户, I want 会话文件的总磁盘占用仍然受配额约束, so that 换格式不会让磁盘失去控制。
18. As a 用户, I want 删除空会话日志的清理仍然存在, so that 崩溃残骸不会长期躺在 `sessions/` 里。
19. As a 维护者, I want 会话文件带格式版本号与记录 `type` 字段, so that 以后新增记录类型或换代时
    旧文件仍能被读，新文件在旧版本里也不会炸。
20. As a 维护者, I want 存储层接口只有"追加一条记录"与"读回会话", so that 存储不需要持有 per-session
    状态，也不需要替调用方校验不变量。
21. As a 维护者, I want 磁盘 IO 仍然只发生在会话层, so that agent 内核保持与前端/存储无关。
22. As a 维护者, I want 同一条消息不会被重复写入日志, so that 重试或失败路径不会产出重复记录。
23. As a 维护者, I want 损坏的会话头（缺失或非法）明确报错, so that 不会静默产出一个身份不明的会话。
24. As a 维护者, I want 悬空工具批次的修补是确定性的, so that 同一个文件每次 load 得到同样的历史。
25. As a 维护者, I want 读侧修补不修改日志文件, so that 会话文件永远只被追加、字节可审计。

## Implementation Decisions

- **格式**（`slimcode-agent` 的 `session` 消息模型 + `slimcode-common` 的 `session`
  模块）：
  - header 行：`{"type":"session","v":1,"id":"…","created_at":"…","project_home":"…"}`。
    `project_home` 取 project home（git 根，非 git 回退用户家目录，canonicalize 后的绝对
    路径；与 project key 同源）。header **不含** `cwd`、不含 title。
  - 记录行：`{"type":"message","message":{…}}`、`{"type":"title","title":"…"}`；`type`
    未知的记录一律跳过。记录不带 `id`/`parentId`/时间戳——slimcode 的历史是线性的，pi 的树
    字段服务于它自己的 `/tree` 分支，这里不需要。
  - `Message` 新增可选 `stop_reason`（`stop`/`tool_calls`/`error`/`aborted`）与 `error`
    字段，序列化时缺省即省略（普通消息的字节形状与现在完全一致）。它们不进入 provider 线格式
    （线格式转换按需处理），也不能让 provider 看到空的 assistant content（失败/取消消息的
    `parts` 带一小段非空文本）。
- **写入**（`SessionStore`）：
  - `append(&self, session: &Session, message: &Message) -> Result<Option<PathBuf>, String>`：
    文件不存在且 `message` 是 assistant → 独占创建（已存在则明显报错），写入 header、title
    记录（当 `session.title` 已知）、以及 `session.messages` 里已有的全部 message；文件已存在
    → 追加这一条记录；文件不存在且不是 assistant → no-op。
  - `append_title(&self, session: &Session, title: &str)`：标题在日志已存在之后才确定时用
    （今天只有创建路径会写 title 记录，接口保留给后续的标题变更）。
  - 不做前缀校验、不 fsync、不重写文件。
- **读取**（`SessionStore`）：
  - `load(&self, id) -> Result<LoadOutcome, String>`，`LoadOutcome { session,
    skipped_records, repaired_tool_calls }`。
  - header 缺失或非法 → 报错（没有身份可重建）；记录行逐条重放，解析失败的跳过并计数。
  - 文件末尾不以换行结尾时：最后一段不是合法 JSON 就丢弃它，然后追加一个换行把文件"封口"，
    保证后续 append 仍然一行一条；恰好是完整 JSON 则当成正常记录收下。
  - 悬空工具批次修补（只在内存里）：对每条带 `tool_calls` 的 assistant，缺 result 的 id 按顺序
    补 `Error: interrupted` 工具结果；没有对应 `tool_calls` 的 tool result 丢弃。
  - title 取最后一条 title 记录，没有则 `None`。
- **store 表面**：路径解析与列表只看 `.jsonl`；配额统计把 `.jsonl` 与遗留 `.json` 的字节一起
  求和，按文件 mtime 从最旧删到配额一半、跳过当前会话（行为不变，只换后缀）；启动清理只删当前
  project 内"0 字节"或"重放后不含任何 assistant 记录"的日志（不碰 `.json`）。
- **前端接线**（agent → runner → TUI）：
  - agent loop 在每条 message 进 history 时发一个新事件，携带 message 本体（assistant 组装
    完成、每条 tool result push 之后）；runner 把它交给注入的 sink；TUI 的 worker 线程直接调
    `append`（存储层是共享引用 + 原子计数，可跨线程使用）。
  - 提交一轮时，前端先把本轮新产生的 system（仅首轮）/ user 消息 push 进会话历史并各 append
    一次（此时是 no-op），再用这份历史构建 context——内存、日志、context 始终是同一条消息序列。
  - 回合结束（**成功或失败**）主线程整体接收 worker 返回的会话；失败/取消时合成一条 assistant
    消息（`stop_reason` + `error`）追加进去，保证日志收尾在 assistant 边界上。
  - 删除 `/save` 命令与对应的 effect/分支；`/load`、`/sessions`、`/new` 保留；`/load` 时若
    有跳过记录或修补的工具调用，给一条 notice。
- **一次性 CLI**：不落盘会话（与现状一致）；启动时仍调用一次收窄后的空日志清理。

## Testing Decisions

- **好测试的标准**：只断言外部可观察行为——文件里有哪些记录、`load` 出来的消息序列、跳过了几条
  坏行、修补出哪几条工具结果、清理/淘汰后哪些文件还在。不测私有结构、不测排序实现、不测 TUI
  渲染像素。
- **seams**（共两条，都复用既有测试位置，不新增测试基建）：
  1. **存储 seam（主 seam）**：`slimcode-common` 的 `session` 模块对 `SessionStore` 公开
     API 的单元测试。格式、写入时机、lazy create、读侧宽容、修补、清理、配额全部在这一层
     断言——行为与前端无关，收敛在一个 seam 上。既有先例：同文件现有的 round-trip / list /
     invalid-id 测试，配合 `unique_temp_dir` 做并行安全的临时目录。
  2. **事件 seam**：`slimcode-agent` 的 loop 用脚本化 fake provider 驱动公开入口，断言发出的
     事件序列里包含每条进 history 的 message。既有先例：agent crate 现有的 FakeProvider +
     事件断言测试。
- **TUI 接线不新增测试基建**：`terminal.rs` 的 worker 线程与落盘接线保持极薄，靠手工验收
  （`tail -f` 看记录逐条出现；`kill -9` 后确认已完成的工具结果在文件里；`/load` 看 notice）；
  `app.rs` 现有的纯逻辑测试（命令补全弹框、effect 解析）只做一处调整：不再拿 `/save` 当样例
  命令。沿用上一个特性（`session-project-scoped`）的立场。
- **覆盖清单**：
  - 首条 assistant 之前 append 是 no-op（磁盘上无文件）；首条 assistant 到达时创建文件，内容 =
    header + title 记录（若有）+ 全部已有 message；此后每条 append 只加一行；
  - 未知 `type` 记录被跳过；`title` 记录取最后一条；
  - 中间坏行跳过并计入 `skipped_records`，其余历史完整；
  - 末尾半行：丢弃、文件被补换行封口，随后的 append 仍然一行一条；
  - header 缺失/损坏 → 报错；
  - 悬空工具批次：缺失 id 补 `Error: interrupted`（顺序在已有结果之后）、孤儿 tool result
    被丢弃、日志文件字节不变；
  - `stop_reason`/`error` 往返，且普通消息的序列化字节不变；
  - 列表只含 `.jsonl`；启动清理只删当前 project 的 0 字节/无 assistant 日志；
  - 配额淘汰：`.jsonl` + 遗留 `.json` 一起统计、按 mtime 最旧先删到一半、跳过当前会话；
  - agent loop 对 assistant 与每条 tool result 各发一次 message 事件。

## Out of Scope

- 旧 `.json` 的读取兼容 / 迁移（用户明确不要；只由配额淘汰清除）。
- 分支/树（`parentId`）、`/tree`、`/fork`、compaction、label、per-record 时间戳。
- pi 新 harness 的 `seq`/事务/快照层（`nextSeq`、tmp+rename 重写）。
- fsync / 掉电持久化；写侧前缀校验；批量原子的工具结果写入；日志压实（compaction）。
- 会话内容压缩 / 归档格式；`/save` 的替代命令。
- TUI/CLI 新增测试基建；任何 UI 布局改动。

## Further Notes

- **文档同步**：`docs/adr/0009-appended-jsonl-session-log.md`（已写好）、`docs/index.md`
  （ADR 行，已更新）、`CONTEXT.md`（Session / Session log / Log header / Dangling tool batch /
  Session store / Eviction，已更新）、`README.md`（命令表去掉 `/save`、会话路径示例）、
  `docs/user-manual.md`（会话文件、命令表、磁盘清理）、`docs/configuration.md`（配额小节的
  文件后缀）、`docs/development.md`（common/session 模块描述 + agent 新事件）。
- **验收**：`cargo test` 全绿；`cargo fmt --all`；
  `cargo clippy --all-targets --all-features --message-format=json -- -D warnings`
  0 error / 0 warning。
- **手工验收**：`tail -f` 观察记录逐条出现；手工追加一行坏数据验证跳过与 notice；`kill -9`
  正在跑工具批次的 TUI，确认已完成的工具结果仍在文件里；`/load` 该会话确认历史合法。
- **实现票**：`issues/01-session-log-format.md`（格式与存储）、`02-persist-on-message.md`
  （agent 事件与前端接线）、`03-docs-sync.md`（文档），按 01 → 02 → 03 顺序做。
