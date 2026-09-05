# 06 — REPL 输入历史与多行 prompt

Type: task
Status: resolved

## Question

给行式 REPL 增加两个子特性：**输入历史**（`input history`，区别于 `message history`）与**多行 prompt**（`multi-line prompt`）。设计已通过 grill-with-docs 与用户逐题拍板，spec 如下。

## Spec（grilling 定案）

### 输入历史

- **概念**：`input history` = 已提交的普通 Prompt 列表（**不含** `/` 命令），落盘跨运行；与 `message history`（`session.messages`）严格区分（见 CONTEXT.md）。
- **持久化**：`<SLIMCODE_HOME||~/.slimcode>/history.json`（config.rs 的 home + `history.json`），JSON 字符串数组，最多 500 条、超出丢最旧；append 后即时落盘（与会话自动保存一致，崩溃不丢）。
- **交互**（无 readline 依赖、无 raw mode）：
  - `/history`：最近 20 条，**最新在前**，带编号（1 = 最新）；
  - `/!N`：重跑编号 N 的那条 prompt（verbatim，多行块原样）；
  - `/!!`：= `/!1`（重跑最近一条）；
  - 重跑 = 作为新一轮 Prompt 提交，沿用当前会话、保留消息历史。
- **去重**：不做（每条提交的 prompt 都记录，可预测）。
- 命令不进历史；历史仅存 Prompt。

### 多行 prompt

- **触发**：行尾 `\`（trim 后以 `\` 结尾）续行；剥掉 `\`、以 `\n` 连接，直至遇到不以 `\` 结尾的行 → 整块提交为**一条** user 消息（Q8b）。
- **Shift+Enter**：不可行（传统终端与 Enter 同字节，仅 kitty 协议可区分，需 raw-mode 协商）→ 不做（见 ADR-0001）。
- **续行态**中所有行（含 `/` 开头行）都作为块内容；仅在**非续行态**按 `classify` 分派 Empty/Command/Prompt（Q8a）。
- `classify` 保持**纯逐行**；`run()` 维护 `pending` 缓冲；新增纯函数 `is_continuation(line)` / `strip_continuation(line)`（可测，Q8a）。

### 验收

- `cargo test` 全绿 + clippy 零警告 + `cargo fmt` clean；
- docs 同步（development.md / user-manual.md 增补 REPL 输入历史与多行 prompt）；只本地提交。

## Answer

（2026-08-29，TDD 落地）实现完成，全绿：cli 53 / agent 48 / ai 17（含 2 ignored live）tests，clippy 0 警告。

- `crates/cli` 新增 `history` 模块：`HistoryStore`（`<home>/history.json`，JSON 数组，上限 500 丢最旧，append 即落盘），`trim_to_limit` 纯函数。
- `repl`：`/history`（最近 20 条、最新在前、编号 1=最新）/`/!!`/`/!N`（verbatim 重跑为新一轮、不再写入历史；错误仅打印并继续循环）；多行 prompt 用行尾 `\` 续行、非 `\` 行或空行提交为一条 user 消息，续行态中 `/` 行也作内容；`classify` 保持纯逐行。
- 纯函数 `is_continuation`/`strip_continuation`/`accumulate`/`parse_replay`/`resolve_replay_index`/`render_history`；共享依赖收在 `ReplCtx`（clippy too_many_arguments 重构）。
- 无 readline 依赖、无 raw mode（ADR-0001）；Shift+Enter 不可行（传统终端同字节）。

## Review follow-up（2026-08-29，/code-review 双轴）

Standards + Spec 双 sub-agent 审查后全量修复：
- **Spec P1**：`/!` 畸形输入（`/!abc`、`/!`）改为打印并继续，不再 `?` 终止 REPL；`/history` load 损坏 `history.json` 也只打印继续；`replay_and_report` 抽取消除 `/!!`/`/!N` 重复分支。
- **Spec US12/15**：`submit_prompt` 把 `history.append` 移到 `run_turn` **前**并非致命 —— 每条提交的 prompt 即时入史，turn 失败也入史。
- **Spec P2**：续行态改为 `is_continuation(line) || !pending.is_empty()` 优先判定 —— 首行保留缩进、`/` 开头行在续行态作内容、空行提交。
- **Standards P1**：术语 `block→prompt` 重命名（注释 + 测试名 `accumulate_*_multiline_prompt` 等）；裸 `history` 注释改 `input history`。
- **Standards P2**：`docs/README.md` 索引补 ADR 目录。
- 补缺口测试：`accumulate_preserves_leading_whitespace`、`accumulate_treats_slash_line_as_content`、`replay_malformed_input_errors_and_continues`、`corrupt_history_errors_and_continues`、`empty_input_line_is_ignored`、`append_keeps_duplicates`（no-dedup）。

最终状态：cli 59 / agent 48 / ai 17（含 2 ignored live）tests 全绿，clippy 0 警告，fmt clean，release 冒烟通过。
