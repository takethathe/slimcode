# 02: Run hooks — the loop can be intervened in

**What to build:** A caller can intervene in a run at three boundaries: before a Tool batch is
dispatched (rewrite a call's arguments, or skip the call and supply its result), as each tool result
is about to enter history (rewrite what the model, the log and the screen will see), and when the run
stops (rewrite the whole history). Hooks are optional closures carrying the `AgentMessage` that is
about to enter history, they run before the events and the log append for the same fact, and a hook
that errors aborts the run. With no hook set, nothing about a run changes.

**Blocked by:** 01.

**Status:** resolved

- [x] `RunHooks` exists with three optional closure fields (`before_tool` / `after_tool` /
      `turn_end`), defaulting to unset, and `AgentRunner` carries it as a public field.
- [x] `ToolDecision { Run, Skip(Result<String, String>) }` exists; a hook returning `Skip` stops that
      call from executing, and its supplied result (or `Error: {err}` shape) enters history and is
      announced by the usual `ToolStart`/`ToolResult` events, keeping every `tool_call` paired with a
      result.
- [x] `AgentMessage` gains `llm_mut`, the single mutation entry point hooks use.
- [x] A batch's `before_tool` calls all run on the loop thread in model order before anything in that
      batch is dispatched — in parallel mode too; `after_tool` runs in completion order, at the same
      moment as the tool events, and also visits a skipped call's yielded result.
- [x] `turn_end` runs once per run, on `Completed` and on `Cancelled`, before the stop event, with the
      turn count, the stop reason and the run's final history (what it leaves there is what is
      returned).
- [x] A hook that returns `Err` aborts the run and propagates the message like a provider or sink
      error: no stop event, no `turn_end`.
- [x] A `before_tool` that removes a call from the batch produces an error, not a panic.
- [x] Mutation is observable on both sides of the boundary: the rewritten text is what the returned
      history carries and what the event stream reported.
- [x] Seam tests are written red first and cover each boundary, the batch ordering in parallel mode,
      skip semantics (success and error), mutation visibility, hook aborts, and `turn_end` on both
      stop reasons; the loop tests from ticket 01 still pass untouched.
- [x] Documentation: ADR-0015 (hooks may rewrite the messages that enter history, with the rejected
      alternatives), `CONTEXT.md`'s `Run hooks` and `ToolDecision`, the `.scratch/arch-realignment`
      spec section that claimed the seam was missing, and `TODO.md`'s open item.
- [x] `cargo test --workspace` green, `cargo fmt --all`, `cargo clippy --all-targets
      --all-features -- -D warnings` clean, tmux smoke tests (including one-shot text mode) green.

## Notes

- The seam ships with no production consumer; its tests are the demonstration.
- No frontend gains a hook API: the display contract still sees only events.
- The end-of-run hook is deliberately the mounting point for a future compaction feature, which is
  why it receives the history rather than just the stop reason.

## Answer

- `RunHooks<'a>` 落地（`crates/core/src/agent.rs`）：三个可选闭包字段 `before_tool` /
  `after_tool` / `turn_end`，`#[derive(Default)]` 全空即「无 hook」；`AgentRunner` 新增公共字段
  `hooks`（`new` 置默认）。为满足 clippy `type_complexity`，三个字段类型提炼为公共 type 别名
  `BeforeToolHook` / `AfterToolHook` / `TurnEndHook`（底层类型与 frozen 接口一致）。
- `ToolDecision { Run, Skip(Result<String, String>) }` 与 `AgentMessage::llm_mut`（session.rs）按
  ADR-0015 落地。`execute_tools` 由自由函数转为 `AgentRunner` 方法：phase 1 在 loop 线程按
  model 序对整批跑 `before_tool`（从 assistant 消息重读 effective calls，参数改写即 dispatch 所见；
  索引消失报 `Err` 不 panic）；parallel 路径只对 `Run` 的调用 spawn，`Skip` 结果按 model 序预填
  completion_order，完成后按完成序跑 `after_tool` + 发 `ToolStart`/`ToolResult`，再按 model 序 push
  history；serial 路径逐个「before → dispatch/skip → after → 事件 → push」。`run()` 在 `Stop` 事件前
  调 `turn_end`（仅 `Completed`/`Cancelled`）。hook `Err` 原样上抛，不发 Stop 事件、不调 `turn_end`。
- 边界测试 12 条（red-first 后 green）：整批先行、参数改写、skip 成功/失败、after_tool 改写同时
  反映在 history 与事件、after_tool 的 ok 标志、hook 错误中止、删调用报错、turn_end 于两种停止原因、
  错误运行不触发 turn_end、parallel 下 skip 先于已 dispatch 调用报出；ticket 01 的 loop 测试原样通过。
- 测试 harness 变化：`run_with_hooks` 保持返回三元组；事件收集改为传入调用方持有的
  `&mut Vec`（`run_with_hooks_collect`），规避 runner drop glue 使 sink 借用存活导致的 E0505。
- 文档：ADR-0015（D1–D4 + 备选 + 后果）、`CONTEXT.md` 的 `AgentRunner` / `Run hooks` /
  `ToolDecision` 已在设计轮写入并由 01 提交带上；`TODO.md` 勾掉「AgentRunner 运行时 seam」。

验证：`cargo test --workspace` 全绿（cli 64 + architecture 7 + tmux 3 + ai 45 + app 190 +
commands 26 + core 79 + tui 123），`cargo fmt --all`，clippy 0 error / 0 warning。
