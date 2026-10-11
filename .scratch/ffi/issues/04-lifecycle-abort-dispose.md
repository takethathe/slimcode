# 04: 生命周期：跨线程 abort / dispose / 并发会话

**What to build:** 宿主真的能控制会话：另一线程取消在途回合（partial 文本只进 display 事件、不进历史），`dispose` 唤醒阻塞的 pump 线程并拒绝后续调用，多个句柄互不干扰。

**Blocked by:** 02

**Status:** resolved

- [x] 导出 `slimcode_session_abort`：任意线程、任意时刻可调，只碰 SDK 取消令牌、不等会话锁
- [x] `dispose` 在 01 的基础上补齐：唤醒被 `next_event` 阻塞的线程（返回 NULL）、拒绝后续 `prompt`
- [x] 测试：另一 OS 线程 `abort` → `prompt` 返回 1、队列里有 `turn_finished{cancelled}`、partial 文本不在 `messages_json` 里
- [x] 测试：默认七工具的取消令牌生效（慢回合中 abort 后不再出现 `tool_result`）
- [x] 测试：pump 线程被 `dispose` 唤醒（NULL），后续 `prompt` 返回 -1
- [x] 测试：两个句柄并发跑回合 → 事件与日志互不干扰

**Gate:** `cargo test` 全绿；fmt/clippy 干净。

## Answer

**实现**

- `crates/ffi/src/abi/lifecycle.rs`：`slimcode_session_abort` 落地。只调 `AgentSession::abort()`——SDK 只设置共享取消令牌，不等会话锁（`crates/sdk/src/lib.rs` 的 `abort` 文档即此语义），所以从任意线程、任意时刻（包括 provider / tool 调用中）调用都立即返回。NULL / unknown / 已 dispose 的句柄是 no-op；与 `dispose` 的 NULL 路径一致，它**既不设置也不清除** `last_error`（`abort` 不是 fallible 入口，不做 `error::clear()`）。panic 仍由 `catch_panic` 兜底。头文件是既有契约，未改。
- `dispose` 的唤醒面：ticket 02 的 `SessionHandle::close()`（置 closed、清队列、`notify_all`）已经实现，本 ticket 不重写，只由测试把行为钉住。
- `crates/ffi/tests/lifecycle.rs`（新增，5 个测试）：
  1. `abort_without_a_turn_is_a_silent_no_op` — NULL / unknown / 空闲句柄都是 no-op；既不设置也不清除此前遗留的 `last_error`；空闲 abort 不污染下一回合（SDK 每回合 reset token），下一 prompt 正常 completed。
  2. `abort_from_another_thread_cancels_a_text_turn_and_keeps_partial_text_out_of_history` — `prompt` 在另一个 OS 线程跑，主线程观察到 `text` 事件（mock 分块间有 100ms 间隙）后 abort：`prompt` 返回 1，队列以 `stop{cancelled}` + `turn_finished{cancelled}` 收尾，partial 标记出现在 `text` 事件里但不出现在整个 `messages_json` JSON 里，`last_error` 保持空。
  3. `abort_cancels_a_running_bash_child_promptly` — mock 让模型调 `bash: touch <marker> && sleep 30`；等 marker 文件出现（证明子进程真的在跑）后 abort：`prompt` 返回 1，elapsed < 5s（远低于 30s），`turn_finished{cancelled}`。
  4. `dispose_wakes_a_pump_blocked_forever_and_invalidates_the_handle` — pump 线程阻塞在 `next_event(-1)`，主线程 dispose：2s 有界等待内返回 NULL 且 `last_error` 空；此后 `next_event` / `prompt` 都报错（NULL / -1 + `last_error`）；二次 dispose 是 no-op 且不动 `last_error`。
  5. `two_handles_run_concurrently_without_crossing_events_or_logs` — 两个句柄各接一个 mock、并发各跑一个回合：每个队列只含自己的 marker 与 prompt；同一个 `<home>/sessions/<project>/` 下恰好两个日志文件，各自只含自己的 marker。
- `crates/ffi/tests/common/mod.rs`：只追加 ticket 04 段（`abort`、`prompt_on_thread`（channel 回传停止码，便于 `recv_timeout` 有界等待）、`await_code`），既有内容未动。

**cancelled `bash` 的解释（与 ticket 字面措辞的差异，附证据）**

ticket 写「abort 后不再出现 `tool_result`」，brief 预判 runner 会为被取消的调用再构造并发出一个 `Error: … cancelled …` 的 `tool_result`。实测本仓库版本**没有** `tool_result`（也没有 `tool_start`）：`RunConfig::default().parallel_tools = true`（`crates/core/src/agent.rs:167`），`execute_tools` 的并行分支在批量收齐后先判 `self.cancel.is_cancelled()` 并 `return Ok(true)`（该文件 `execute_tools` 的并行收尾处，`build_result` 循环之前），`build_result`（`:403`）根本没跑；串行分支同样在 dispatch 之后立刻判取消。用临时 `eprintln!` 观察该回合队列，实际序列为 `message(user) → turn → message(assistant, tool_calls) → stop{cancelled} → turn_finished{cancelled}`，无 tool 事件对。因此测试按两侧都成立的不变量断言：**要么没有 `tool_result`；要么它必须是 `ok=false` 且载荷含 cancelled**，绝不可能是成功的 `sleep 30` 结果。字面目标（不出现成功结果）达成，但机制是「整批丢弃」而非「发出 cancelled 结果」。

另：fresh 会话被取消的**首**回合不会落盘、也没有 closing assistant message——store 在首条 assistant 消息前不建文件（`crates/app/src/session.rs` 的 `append`），于是 `close_turn` 的 `appended > 0 || log_exists` 为假；`messages_json` 此时只含用户 prompt。brief 提到的 closing 消息（“The turn was cancelled.”）只在已有日志或本回合已产生消息时出现。两种形状下「partial 标记不出现在整个 history JSON」都成立，测试断言的正是这一条。

**验证**（worktree `/tmp/slimcode-ffi-wt/t04`，`CARGO_TARGET_DIR=/tmp/slimcode-ffi-target`）

```bash
cargo fmt --all
cargo clippy --all-targets --all-features --message-format=json -- -D warnings   # exit 0，0 条 level error/warning
cargo test --workspace                                                            # exit 0，全绿
bash crates/ffi/demo/run.sh                                                       # PASS（10 checks）
cargo test -p slimcode-ffi --test lifecycle                                       # 5 passed，连跑 10 次无 flake
```

**提交**

- `feat(ffi): add the session abort symbol`
- `test(ffi): cover the session lifecycle`
- `chore(scratch): resolve the FFI lifecycle ticket`

**偏差 / 备注**

- 上述 cancelled-`bash` 解释：断言的是「不出现成功结果」的不变量，而非事件缺席。
- 本机 `CARGO_BUILD_BUILD_DIR`（共享 `~/.cargo/build`）会被并行 ticket 的构建覆写，期间出现过两次 `cannot find function slimcode_session_abort`（源码已实现）；按 brief 用 `touch crates/ffi/src/**/*.rs` 重跑即恢复，未切换 target dir。
- 未改头文件、未加依赖、未动其它 ticket 的文件。