# 02: 事件流与第一轮真实回合

**What to build:** 宿主能跑完整一轮 agent 回合并从队列里实时取到结构化事件：`prompt` 同步返回终止码，回合中每个事件以扁平 JSON 进入 per-handle 无界队列，`next_event(timeout)` 阻塞取一条；provider 由本地伪 SSE 端点充当，`cargo test` 离线覆盖整条路径。

**Blocked by:** 01

**Status:** resolved

- [x] `slimcode_session_prompt(handle, text)` → 0 `Completed` / 1 `Cancelled` / -1 错误 + last_error；空 / 全空白 prompt 报错且不落日志
- [x] `slimcode_session_next_event(handle, timeout_ms)`：`< 0` 无限等待；取到 → JSON 串（`free_string` 释放）；超时 → NULL 且 `last_error` 保持空
- [x] FFI 自有的扁平 wire 契约 + 穷尽映射：`turn` / `reasoning` / `text` / `tool_start` / `tool_result` / `stop` / `usage` / `message` / `turn_finished` / `compacted`（`DisplayItem` 新增变体 → 编译失败）
- [x] 事件入队由会话的一次性监听器完成（沿用 SDK 的订阅顺序与同步送达语义）
- [x] stdlib-only mock SSE 端点（`TcpListener` + 线程，按请求顺序服务脚本化 `data: {chunk}`，捕获请求头/体）
- [x] 测试：事件顺序（`turn` → `text` → `stop` → `turn_finished`）；`message` 的 usage 语义（assistant 有，用户 prompt 与工具结果为 null）；`usage` 事件出现时机
- [x] 测试：provider 失败（端点 500）→ `prompt` 返回 -1、last_error 非空、会话仍可继续下一轮
- [x] 测试：单飞（回合进行中再 `prompt` → -1）；`next_event` 空闲短超时 → NULL 且 last_error 空
- [x] 文档：CONTEXT.md 新增 `FFI 事件队列` 词条

**Gate:** `cargo test` 全绿；fmt/clippy 干净。

## Answer

**What was built**

- `crates/ffi/src/wire.rs` (new): the FFI-owned flat wire enum. `WireEvent` derives
  `Serialize` with `tag = "kind"` / `rename_all = "snake_case"`; `From<&SdkEvent>` and
  `From<&DisplayItem>` are exhaustive matches with no `_` arm, so a new `DisplayItem`
  variant breaks the build. `encode(&SdkEvent) -> Result<String, serde_json::Error>` keeps
  the listener path panic-free.
- `crates/ffi/src/handle.rs`: `SessionHandle` gained `queue: Arc<EventQueue>`.
  `EventQueue` is the per-handle unbounded `Mutex<QueueState>` + `Condvar` (`VecDeque<String>`
  + `closed`); `pop(Option<Duration>)` has a monotonic deadline (spurious-wakeup safe),
  `close()` sets `closed`, clears the queue and `notify_all`. `handle::insert` subscribes the
  one-time listener (SDK subscription order, synchronous delivery, drops the `Unsubscribe`)
  and pushes each encoded event; `SessionHandle::close` now closes the queue, so `dispose`
  wakes blocked `next_event` callers immediately (ticket 04 only verifies it).
- `crates/ffi/src/abi/events.rs`: `slimcode_session_prompt` (0 / 1 / -1 + `last_error`,
  NULL/unknown/disposed handle rejected, NULL / non-UTF-8 text rejected) and
  `slimcode_session_next_event` (`< 0` waits forever, `0` polls, `> 0` waits; timeout /
  empty poll / wake-on-dispose return NULL with an EMPTY `last_error`; a bad handle at call
  time returns NULL with `last_error` set).
- `crates/ffi/include/slimcode.h`: the ambiguous `next_event` NULL sentence was rewritten to
  the three unambiguous cases; the Events JSON section now pins the `usage` shape
  (`prompt_tokens_details: null` when the endpoint omitted it, never a zero object), the
  `message.usage` semantics and the exact order of a text turn (user `message` first, ...,
  `stop`, then `turn_finished`); handle lifetime documents that `dispose` drops unconsumed
  events.
- `crates/ffi/tests/common/mod.rs` (append-only): `MockServer` / `Reply` (
  `Sse(Vec<String>)` / `Status{status, body}`) / `CapturedRequest` (`header` / `target` /
  `json`), the `sse_text` / `sse_reasoning` / `sse_tool_call` / `sse_tool_finish` /
  `sse_usage` payload builders and `SSE_DONE`, plus `prompt`, `next_event`, `drain_ready`,
  `drain_until_turn_finished`, `kind`, `kinds`. The server uses a non-blocking accept loop
  and a stop flag, so `Drop` can never hang.
- `crates/ffi/tests/turn_stream.rs` (new, 9 tests): the pinned text-turn order, the tool-turn
  `message` usage semantics (`assistant` carries the request's usage; the tool-result
  message and the user prompt are `null`), usage timing, the 500 failure + recovery,
  single-flight, idle-timeout and the two NULL kinds, an infinite (`-1`) pump thread, the
  dispose wake-up, and empty/blank/NULL prompts leaving no log.
- `CONTEXT.md`: the `FFI 事件队列` glossary entry.

**Commits**

- `feat(ffi): stream the first turn through a per-handle event queue` (wire + queue +
  symbols + header + tests)
- `docs(ffi): add the FFI event queue glossary entry` (CONTEXT.md)
- `chore(scratch): resolve the FFI event-stream ticket`

**Verification**

```
cargo fmt --all
cargo clippy --all-targets --all-features --message-format=json -- -D warnings   # 0 error / 0 warning
cargo test --workspace                                                            # 27 suites ok, 0 failures
bash crates/ffi/demo/run.sh                                                       # PASS (10 checks)
```

**Deviations**

- The ticket's checklist mentions the tool-result `message` usage semantics; covering it
  needs one tool turn, so `turn_stream.rs` scripts a single serialized `ls` turn. Ticket 03
  still owns the tool-turn/accessor test file and its second-request-body assertions.
- The mock server pauses 100 ms between SSE payloads (not required by the checklist). It is
  what makes the single-flight and blocked-pump tests deterministic: the captured request
  proves the turn is in flight while the provider waits on the next chunk.
- The dispose wake-up has a test here even though ticket 04 verifies the lifecycle, because
  this ticket implements the condvar wake-up.