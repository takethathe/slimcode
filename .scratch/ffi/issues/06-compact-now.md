# 06: compact_now：回合外强制压缩

**What to build:** 宿主能强制压缩历史并拿到 before/after，压缩事件进入同一事件队列，失败不丢历史。

**Blocked by:** 02, 03

**Status:** resolved

- [x] 导出 `slimcode_session_compact_now` → `{"before":N,"after":M}`，失败 NULL + last_error
- [x] 测试：`compacted` 事件进队列；`messages_json` 显示历史被替换为 `compact_summary`
- [x] 测试：压缩失败（端点报错）→ NULL + last_error 非空、历史原样、下一次 `prompt` 仍可用
- [x] 测试：与 `prompt` 单飞（回合进行中 `compact_now` 被拒）

**Gate:** `cargo test` 全绿；fmt/clippy 干净。

## Answer

**What was built**

- `crates/ffi/src/abi/compact.rs` (`ea9f8dd`): `slimcode_session_compact_now`, the usual thin
  wrapper (spec D4) — `error::clear()`, `catch_panic`, `handle::lookup` (NULL / unknown /
  closed → `"invalid or disposed session handle"`), then `AgentSession::compact_now()`.
  `Ok((before, after))` becomes the library-owned `{"before":N,"after":M}` string (the field
  order the header documents); `Err(message)` becomes `NULL` + `last_error`. Nothing else was
  needed for the `compacted` event: the ticket-02 listener already pushes every `SdkEvent`,
  and the SDK emits `Compacted` synchronously before returning, so the event is in the queue
  by the time the call returns.
- `crates/ffi/tests/compact_now.rs` (new, 5 tests, `561545c`):
  - `compact_now_returns_the_token_change_and_replaces_the_history` — a 70 000-character
    prompt (17 500 estimated tokens > the 16 000-token retained budget) makes the history
    compaction-eligible; the mock serves the summarization request. Pins the exact numbers
    (`before` = 17 500 + 2 = 17 502, `after` = 7 + 2 = 9, both derived from the documented
    `ceil(chars / 4)` heuristic), the `compacted` event carrying the same `before`/`after`,
    and `messages_json` = `[compact_summary{summary, tokens_before = before}, assistant tail]`
    (the summarized prompt is gone, the reply is kept). The second request's body carries
    `<conversation>` — the span really was serialized to the summarizer.
  - `nothing_to_compact_and_invalid_handles_follow_the_failure_contract` — a short history →
    `NULL` + a `"nothing to compact"` error, history and queue untouched, and **no** request
    reaches the endpoint; then NULL and a disposed pointer → `NULL` + `last_error`.
  - `a_failed_summarization_keeps_the_history_and_the_session_usable` — reply 2 is an HTTP
    500: `NULL` + non-empty `last_error`, `messages_json` byte-identical to the snapshot
    taken before the call, nothing queued, and the next `prompt` (reply 3) runs a normal turn
    that appends prompt + reply.
  - `compact_now_is_rejected_while_a_turn_is_in_flight` — the ticket's direction: while a slow
    turn holds the single-flight slot, the call is `NULL` + `"a turn is already running"`,
    never reaches the endpoint, and the turn still finishes with `turn_finished`.
  - `prompt_is_rejected_while_a_compaction_is_in_flight` — the reverse direction: while a slow
    summarization is in flight, `prompt` returns `-1` with the same error, and the compaction
    still completes and queues its event with the numbers it returned.
- `crates/ffi/tests/common/mod.rs` (append-only, ticket-06 section, `561545c`): one helper,
  `compact_now(handle) -> Option<Value>` (parses the returned object, mirroring `usage_json` /
  `messages_json`). No existing line changed.

The header was not touched: `include/slimcode.h` already documents the symbol (ticket 01), and
`tests/abi_contract.rs` keeps implemented ↔ declared in sync.

**Verification**

```bash
cd /tmp/slimcode-ffi-wt/t06 && export CARGO_TARGET_DIR=/tmp/slimcode-ffi-target
cargo fmt --all -- --check                       # clean
cargo clippy --all-targets --all-features --message-format=json -- -D warnings
                                                 # exit 0, 0 "level":"error" / 0 "level":"warning" lines
cargo test --workspace                           # exit 0, every suite ok, 0 failed (compact_now: 5 passed)
bash crates/ffi/demo/run.sh                      # PASS: slimcode-ffi smoke (10 checks) / demo (1 script)
```

The timing-sensitive tests (both single-flight directions) were run five times in a row, all
green. The demo's cdylib was force-rebuilt from this worktree before the final run.

**Deviations**

- Two tests beyond the checklist: the "nothing to compact" case (plus the invalid-handle
  contract) and the reverse single-flight direction (`prompt` during a compaction). Both are
  named in the ticket's scope statement / the brief ("`NULL` + `last_error` ... including
  'nothing to compact', a disposed session, a NULL/unknown handle") and cost no extra seam.
- The success test pins exact `before`/`after` instead of only "sane numbers": they are
  ABI-visible outputs whose arithmetic the compaction spec pins (`ceil(chars / 4)` over
  message text), so a worked example is a stronger regression net than a range.
- Environment note (same as tickets 03/05): this machine's shared `CARGO_BUILD_BUILD_DIR`
  makes the `slimcode-ffi` rlib/rmeta in the shared target dir stale across worktrees, which
  shows up as a bogus `cannot find function slimcode_session_compact_now in module
  compact_abi`. `touch crates/ffi/src/lib.rs` + re-run rebuilds it from this worktree; no
  other worktree or target dir was touched.

**For ticket 07 (Node demo)**

- Call it exactly like the header says: `compact_now(handle)` returns a `char*` the host must
  release with `free_string`, and on `NULL` read `last_error()` for the reason. The
  `{"kind":"compacted","before":N,"after":M}` event arrives through the ordinary
  `next_event` pump (no separate subscription), so the demo's event loop can render it with
  the same `switch (event.kind)` it already has.
- A `compact_now` demo segment must (a) grow the history above 16 000 estimated tokens first
  (`ceil(chars / 4)`; ~70 000 characters of prompt is the cheapest way), and (b) give the
  embedded fake SSE endpoint one extra non-empty text reply for the summarization request —
  otherwise the call fails with "nothing to compact" or a provider error. Do not assert
  `usage_json` around compaction (the summarization call folds into session usage), and
  expect `messages_json` to start with a `compact_summary` record afterwards.
- Helper appended to `tests/common/mod.rs`: `common::compact_now(handle) -> Option<Value>`
  (parsed `{"before","after"}`; `None` is the `NULL`/failure return — pair it with
  `common::last_error()`).