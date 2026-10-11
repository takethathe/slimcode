# 06: 文档同步 + 死代码清理

**What to build:** 薄壳化落定后的文档与清理：ADR-0022 记录决策（依赖反转、SDK 面扩展、行为权衡）、CONTEXT.md 的 `CLI` / `SDK` 词条同步薄壳现实、development.md 与 index.md 同步；spec 置 resolved；`app::setup` 确无消费者则删除。

**Blocked by:** 04 (one-shot 收敛)、05 (TUI 收敛)

**Status:** resolved

- [x] ADR-0022「CLI thin-shell over the SDK」：依赖方向反转（cli → sdk）、SDK 扩展（with_home / with_session_id / compact_now / 可取消默认工具）、行为权衡（repair 计数提示移除、skills 冻结、配置冻结）
- [x] CONTEXT.md：`CLI` 词条改为消费 `slimcode-sdk`（argv / 模式 / 终端 / 命令语义 / 输入历史 / picker 列表 / 显示适配 / 会话对象管理）；`SDK` 词条末尾的「thin-shell follow-up」改为已实现
- [x] development.md：crate 表 cli 行加 sdk、架构依赖说明同步；index.md 的 ADR 摘要加 ADR-0022
- [x] spec 置 `Status: resolved`
- [x] `app::setup` 确无其它消费者则删除（含导出与文档引用）

## Answer

分支 `feat/t6-docs`（worktree `/private/tmp/cli-thin-shell-wt/t6-docs`），rebase 到 `cli-thin-shell` tip（`e40183e`）之后：

- `6d544cc docs(docs): add ADR-0022 and sync the CLI thin-shell docs` — 新建 `docs/adr/0022-cli-thin-shell-over-the-sdk.md`（D1 依赖反转、D2 TUI 的 `AgentSession` 当前会话对象 + thread-local emit 桥、D3 SDK 面四项扩展、D4 前端语义不入 SDK、D5 两条已知回退 + 配置冻结；含 Considered Options 与 Consequences）；`CONTEXT.md` 的 `CLI` / `SDK` / `AgentSession` 词条同步，新增 `compact_now` / `with_session_id` 词条，`Compaction` 词条补按需路径；`docs/development.md` 的 crate 表（cli 行、sdk 行、app 行去掉 setup）、依赖方向说明、app/cli/tui/render 各节同步薄壳现实；`docs/index.md` ADR 摘要加 ADR-0022。
- `951bfde refactor(app): delete the unused app::setup seam` — `rg` 确认无消费者后删除 `crates/app/src/setup.rs`（`setup` / `setup_with_cancel`）、`lib.rs` 的模块声明与文档条目，并修正 `main.rs` 一条过时测试注释。

验证：`cargo fmt --all --check` 通过；`cargo test --workspace` 23 个 suite 全 ok（重跑多次；`crates/core` 的 `parallel_history_follows_model_order_while_events_follow_completion_order` 偶发一次 timing flake，与本次改动无关——该测试用 sleep/线程 starving 断言事件顺序，且 diff 完全未触碰 `core`）；`cargo clippy --all-targets --all-features --message-format=json -- -D warnings` exit 0，JSON 流中 0 个 error/warning。spec 已置 `Status: resolved`。

偏差：ADR-0011 / ADR-0016 中提及 `app::setup` 的句子保留原样（ADR 为历史记录，仓库既有惯例是旧 ADR 保留旧命名，如 `slimcode-common` / `slimcode-agent`），由 ADR-0022 的 Consequences 指明该模块已删除、旧 ADR 该处按历史阅读。
