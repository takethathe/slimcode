# Engine 迁移终验报告（ticket 09）

把 slimcode 同步循环内核升级为**可移植的通用 agent 引擎**（ADR-0031）的收口证据。整条迁移以
「逻辑等效」为铁律：code preset 的外部可观测面逐字节不变，扩展 seam 全部就位、可选件默认关闭。

## 工单落地（8 个 commit，坐落 `origin/main` = 8beae30 之上）

| ticket | commit | 内容 |
| --- | --- | --- |
| 01 | `82a127d` | 迁移前等效性 golden 基线（sdk + app 各一条 `equivalence_baseline` 快照） |
| 02 | `c1d3e5c` | core 运行 seam 扩为七阶段 phase-hook 管线（expand，与旧 3-hook 并存） |
| 03 | `9230a5e` | code 型行为落到各自接缝（skill/回滚=phase hook，context/持久化=operand+sink，hook 可改写覆盖） |
| 04 | `fb19553` | turn-end compaction hook 化并收缩、退役旧 3-hook seam |
| 05 | `85af8af` | 公开 Engine/Preset 构造面 + 内建 operand-call trace span（ai trace seam） |
| 06 | `4a5893d` | 会话事件改走有界 event bus 投递（dispatcher 线程 + flush 屏障 + panic 隔离） |
| 07 | `9fba35d` | runtime-agnostic async 门面（`prompt_async`/`PromptFuture` + `event_stream`/`Stream`） |
| 08 | `7e7f2f8` | 可选迭代/超时护栏 + 取消回滚游标（bot 默认 `after_turn` hook） |

## 等效性与接缝就位

- **等效性基线逐点通过**：`crates/sdk/tests/equivalence_baseline.rs::equivalence_baseline_matches_golden_snapshot`
  与 `crates/app/tests/equivalence_baseline.rs::app_baseline_matches_golden_snapshot` 均 byte-for-byte 命中迁移前
  快照——迁移后 code preset 的外部输出（事件顺序 / display 流 / 会话落盘 / stop reason）与迁移前一致。
- **每个相位以「列表是否为空」为门**：空 `PhaseHooks` 是 provably no-op，`Tracer: None` 时 span 与
  `chat_traced` 逐字节退回 `chat`；护栏默认 `None` 时循环不读时钟、不判上限。可选件不挂即无感。
- **跨会话队列 / 串行调度仍留宿主**：本迁移只保证单会话可取消 / 可回滚，不引调度器。

## 消费面未破坏（FFI / GUI）

- **FFI 纯 C ABI**：`include/slimcode.h` 符号表（13 个）与返回码契约不变；`slimcode_session_prompt` 对新增
  `StopReason` 变体走 catch-all 映射到既有干净码，code preset 永不触发护栏变体。`crates/ffi/tests/`
  （`abi_contract` 14 / `lifecycle` / `persistence` / `turn_stream` / `compact_now` / `tool_turn_accessors`）全绿。
- **GUI wire 事件词汇**：`StopReason` serde `rename_all = snake_case` 仅新增 `max_iterations` / `timeout`
  两个从不发送的串；既有 `completed` / `cancelled` 与全部事件签名不变（sdk golden 佐证）。GUI
  （`gui/`，workspace 外）继续经同步 `AgentSession` 面消费，不过 async 桥。

## 架构与依赖矩阵（`crates/cli/tests/architecture.rs`）

- 内部 crate 边未加宽、**无新增 crate**：`ai→{}`、`core→{ai}`、`app→{ai,core,commands}`、`commands→{}`、
  `tui→{}`（零依赖）、`cli→全部`、`sdk→{app}`、`ffi→{sdk}`。async 门面所需的 `futures-*` 是 sdk 的**外部**依赖，
  不受内部矩阵约束。
- **core/app/sdk 不引 tokio**：`engine_layers_declare_no_tokio` 只在三 crate 的 Cargo.toml dependency 表内匹配
  （剔除注释行），证明门面 runtime-agnostic。

## 门禁全绿

- `cargo fmt --all` 无差异。
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`：JSON 流 0 error / 0 warning。
- `cargo test --workspace`：全绿（含新增 core 护栏测试、`crates/app/tests/guardrails_rollback.rs`、
  `crates/sdk/tests/async_facade.rs`、`crates/app/tests/engine_trace.rs`、`crates/sdk/tests/event_bus.rs`）。

## 文档同步

- `docs/development.md`：新增「可移植 Engine 与七阶段管线（ADR-0031）」小节（管线 / Engine 面 / 护栏与回滚
  游标 / trace seam / event bus / async 门面）；更新运行时循环小节（ADR-0015→ADR-0031、可选护栏）。
- `docs/index.md`：ADR-0031 入目录；新增 `.scratch/engine/spec.md` 规格索引行。
- `GLOSSARY.md`：补 Guardrail、Rollback cursor 两条术语（承接既有 Engine/Loop skeleton/Operand/Phase hook/
  Preset/Event bus/Async facade）。

## code-review 后的收口校正

对全部 9 个 commit 做两轴 review（Standards / Spec）后修正。均为**文档与死代码**问题，不动逻辑、不触
等效基线：

- **相对 spec 的两处有意收口（以 ADR-0031 为准）**：
  - context 装配与 JSONL 持久化**不是** phase hook（spec §L25/L96 曾这么写）——它们是循环骨架 + operand /
    turn seam（message sink）的职责；按 ADR-0031 D1「operands 有意不是 hook」，把持久化做成 hook 会动摇逐
    字节事件序（等效 gate）。宿主仍可注册 hook 改写 `RunState::messages` 覆盖默认维护。已同步
    GLOSSARY（Phase hook/Preset/Operand）、`docs/development.md`、ADR-0031 D1/D5 与工单 03。
  - 回滚（`initial_message_count` 游标）只覆盖**取消 / 中止（含护栏停止）**；provider 错误路径不触发
    `after_turn`（ADR-0031 D2 有意），出错回合把 `Err` 交回宿主、恢复由宿主负责。已同步 ADR-0031 D2/D5 与工单 08。
- **硬伤修复**：GLOSSARY 删除已退役的 `Run hooks` 条目、改 “optional Run hooks”→phase hooks、“before-tool
  hook”→pre-tool hook、SdkEvent “calling thread”→经有界 event bus 投递。
- **死代码清理**（ticket 08 后续）：删 `PhaseHooks::is_none()` 与 `Extensions::is_empty()`（零调用方，等效 gate
  的真实机制是各相位对空 `Vec` 天然短路）。
- **命名对称**（ticket 08 后续）：`AfterToolPhase`→`AfterTool`、`AfterToolPhaseHook`→`AfterToolHook`（旧消歧类型
  已退役，向 `AfterChat`/`AfterTurn` 对齐；不触 wire/ABI）。
