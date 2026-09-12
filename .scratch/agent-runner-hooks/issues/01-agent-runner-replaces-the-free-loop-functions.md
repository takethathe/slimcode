# 01: the loop becomes an AgentRunner, byte-identically

**What to build:** The agent loop stops being three free functions and becomes one borrowed per-run
value, `AgentRunner`, holding that run's tools, config, cancel token and event subscription. Every
existing caller (the application layer's shared turn runner, and every loop test) goes through it,
and a run behaves exactly as it does today: same messages, same events, same stop reasons, same
one-shot output. No hooks yet — this ticket is the shaped loop, the next one is the seam.

**Blocked by:** None (can start immediately).

**Status:** resolved

- [x] `AgentRunner` exists with the spec's shape minus hooks (`tools` / `cfg` / `cancel` /
      `on_event`), constructed by `new`, with `run(provider, system, messages)` returning the
      updated history and the stop reason; the event sink is public because it is part of the
      struct's surface.
- [x] `run_agent`, `run_agent_from_messages`, `run_agent_from_messages_sink` and `RunResult` are
      gone, with no dead remainder anywhere in the workspace.
- [x] The shared turn runner in the application layer constructs a runner instead of calling a free
      loop function; its recording-renderer tests pass unchanged in meaning.
- [x] Every existing loop test asserts the same behaviour it asserts today, moved onto a local
      harness (a run returns the history and drains the event stream) with no assertion weakened.
- [x] Cancellation, parallel-tool ordering (events in completion order, history in model order) and
      per-message events behave exactly as before.
- [x] `CONTEXT.md` gains `AgentRunner`; `docs/development.md`'s core section describes the runner and
      loses the "known spec deviation" paragraph; ADR-0011's implementation note records the shape
      instead of the deviation; the `.scratch/arch-realignment` spec's matrix row no longer says the
      runner was not built.
- [x] `cargo test --workspace` green, `cargo fmt --all`, `cargo clippy --all-targets
      --all-features -- -D warnings` clean, and the CLI's tmux smoke tests (including one-shot text
      mode) stay green.

## Notes

- This is a wide-but-mechanical refactor: the loop body moves, the collaborators become struct
  fields, and the call sites are few (one in the application layer, the rest in tests). Nothing in
  it is meant to be observable.
- `RunResult` goes with the entry points it packaged; tests derive the turn count from `Turn`
  events instead.
- Sequencing note for the next ticket: `RunHooks` arrives with ticket 02, so the struct grows a
  field there rather than carrying an unused one here.

## Answer

- `AgentRunner<'a>` 落地（`crates/core/src/agent.rs`）：借用式 per-run 值，字段
  `tools: &'a [Tool]` / `cfg: RunConfig` / `cancel: &'a CancelToken` / `on_event: EventSink<'a>`；
  `new` 构造，`run<P: Provider>(provider, system, messages) -> Result<(Vec<AgentMessage>,
  StopReason), String>` 承载原 loop 主体（含取消边界、并行/串行工具批次、per-message 事件）。
  `EventSink` 从私有 `type` 升为 `pub type`。
- 三个自由函数（`run_agent` / `run_agent_from_messages` / `run_agent_from_messages_sink`）与
  `RunResult` 全部删除；`app::runner::run_turn` 改为构造 runner + `run()`（签名不变），其
  「与 agent 语义一致」测试改用同一 runner 直接驱动。
- core 测试迁到本地 harness（`run_with`：收集型 sink + 返回 `(messages, stop, events)`；
  `run` 从 `Turn` 事件数出迭代数），全部断言原样，行为不变：并行完成序事件 + model 序历史、
  取消各边界、半段文本不入 history、`Error: ` 前缀恢复等 25 条测试原样通过。
- 文档：ADR-0011 的 implementation note 从「未实现」改为「已实现：借用式 per-run 结构体，
  自由函数删除，hook seam 见 ADR-0015」；`development.md` 的 crate 表 core 行与运行时循环段改为
  `AgentRunner` 形状，删除「已知 spec 偏差」段；`explanation.md` 与 `context.rs` 模块注释里
  对已删函数的引用改指 `AgentRunner::run`；arch-realignment spec 矩阵行恢复 `AgentRunner`。

验证：`cargo test --workspace` 全绿（cli 64 + architecture 7 + tmux 3 + ai 45 + app 190 +
commands 26 + core 67 + tui 123），`cargo fmt --all`，clippy 0 error / 0 warning。
