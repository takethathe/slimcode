# 07: Node 验收 demo

**What to build:** 把 01 的 smoke 长成完整验收证据：单文件、零依赖 `node:ffi`，在 worker 线程跑回合、主线程实时 pump 事件（证明队列跨线程可用、C 侧不回调进 JS），并对工具回合、abort、状态访问器、持久化回放逐项自断言，打印 PASS/FAIL。

**Blocked by:** 02, 03, 04, 05

**Status:** resolved

- [x] worker 线程跑 `prompt`、主线程 `next_event` 实时打印事件（同一句柄跨线程使用）
- [x] 内嵌 `node:http` 伪 Bailian SSE 端点：工具回合、慢滴虫回合（供 abort）
- [x] 断言：工具回合事件序列与终止码；`abort` → cancelled 且 partial 不在 `messages_json`；`session_id` / `title` / `usage_json`；`persist:false` 零文件与 `persist:true` 落盘 + 按 id 回放；坏选项的错误路径
- [x] `demo/README.md`：Node ≥ 26（内建 FFI；`--no-experimental-ffi` 会禁用）、运行命令、预期输出、koffi 作为老 Node 的替代路径说明
- [x] `demo/run.sh` 作为一条命令的验收 gate（build + demo，失败非零退出）

**Gate:** `bash crates/ffi/demo/run.sh` 打印 PASS、退出码 0（离线、无 API key）。

## Answer

**What was built**

- `crates/ffi/demo/node/demo.mjs` (new, 39 self-asserting checks, `b2b0497`): one
  self-contained, zero-dependency file (`node:ffi` only) with an `isMainThread` branch.
  The ABI declarations use the header's exact signatures (`arguments` / `return`, pointer
  results as `bigint`, `0n` = NULL). Structure: a `loadAbi` wrapper applying the ownership
  rule (`takeString` → `free_string`; `last_error` never freed) and the "two NULL kinds"
  rule; a scripted `node:http` SSE endpoint on `127.0.0.1:0`; `playTurn`, which spawns the
  worker (handle + text + library path via `workerData`) and pumps `next_event` here while
  the worker's `prompt` is still in flight; then the segments.
  - **Tool turn** — scenario `tool`: `ls` call, then a second request carrying the tool
    result. Pins the 13-event sequence (`message, turn, message, tool_start, tool_result,
    message, turn, reasoning, text, message, usage, stop, turn_finished`), the `call_ls`
    pair with the real `.git` listing, `prompt` → `0`, and the endpoint's second request
    body carrying `role: "tool"` with that listing.
  - **Abort** — scenario `slow` (200 ms drip between chunks): the main thread calls `abort`
    the moment the `partial-marker-7b21` text event arrives, while the worker is still
    inside `prompt` (asserted). `prompt` → `1`, worker and main `last_error` stay empty
    (cancellation is not an error), `stop` / `turn_finished` report `cancelled`, the partial
    text is on the display transcript but absent from `messages_json` (the user prompt is
    present), and exactly one request reached the endpoint.
  - **Accessors** — `session_id` (`slimcode-…`), `title` = `list the files`,
    `usage_json` = the accumulated `TokenUsage` (20/4/24 with
    `prompt_tokens_details.cached_tokens = 2`), and `messages_json` equal, in order, to the
    `message` event payloads (roles `user, assistant, tool, assistant`).
  - **Persistence** — the two `persist: false` sessions (tool turn + abort) leave the temp
    home with **zero files** (not even a `sessions/` dir); the `persist: true` session lands at
    `<home>/sessions/<project-key>/<id>.jsonl` with the header / title / message records and
    the exact ABI-reported payloads; a second handle created with the same `session_id`
    replays the id, title and history, its resumed request carries the replayed conversation
    back to the endpoint, and its turn appends two records to the same log (one `session`
    record, one `title` record, no rewrite).
  - **Bad options** — malformed JSON and an unknown field (`api_ky`) both return `NULL`, the
    latter's `last_error` naming the field.
  The demo binds its own endpoint, passes a dummy `api_key`, pins `home` / `cwd` under an
  `mkdtemp` root (never the real `~/.slimcode`) and removes the root in a `finally`. Every
  wait is bounded: `next_event` uses 100 ms slices plus a 30 s turn deadline, the
  worker/endpoint side has finite timeouts, and the worker is always terminated.
- `crates/ffi/demo/README.md` (new, `cff2daf`): Node ≥ 26 and the built-in `node:ffi`
  (`--no-experimental-ffi` disables it; the `ExperimentalWarning` is not a failure), the
  one-command run, expected output, a per-segment description and the koffi note for older
  Node (documentation only — no adapter code).
- `crates/ffi/demo/run.sh` (`b2b0497`): `scripts=(node/smoke.mjs node/demo.mjs)` — the smoke
  stays as the cheap ABI check, the demo is appended; the runner's cwd-independence,
  platform artifact resolution and non-zero exit on failure are unchanged. Still one command
  and the acceptance gate.

No Rust file and no Rust test was touched (the crate and `tests/` are byte-identical to
`ffi` tip `d4b1fd4`), as the brief requires.

**Verification**

```bash
cd /tmp/slimcode-ffi-wt/t07 && export CARGO_TARGET_DIR=/tmp/slimcode-ffi-target
cargo fmt --all                                  # clean (no Rust change)
cargo clippy --all-targets --all-features --message-format=json -- -D warnings
                                                 # exit 0, 0 "level":"error" / 0 "level":"warning" lines
cargo test --workspace                           # exit 0, every suite ok, 0 failed
bash crates/ffi/demo/run.sh                      # PASS: slimcode-ffi smoke — 10 checks
                                                 # PASS: slimcode-ffi Node demo — 39 checks
                                                 # PASS: slimcode-ffi demo (2 script(s)), exit 0
```

Extra checks required by the brief:

- **cwd independence**: `cd / && bash /tmp/slimcode-ffi-wt/t07/crates/ffi/demo/run.sh` →
  both PASS lines and exit 0, with the same shared `CARGO_TARGET_DIR`.
- **deliberately broken assertion**: flipping `title === 'list the files'` to a wrong
  literal made the run print
  `FAIL title is inferred from the first prompt — list the files`,
  `FAIL: slimcode-ffi Node demo — 1/39 checks failed`, and
  `FAIL: node/demo.mjs` with exit 1; restoring the assertion returned the run to green.
- **hermeticity**: no `slimcode-ffi-demo-*` directory survives a run, and the demo never
  reads the developer's `~/.slimcode` (explicit `home` + `cwd` on every session).

**Deviations**

- Segment order is tool turn → accessors → abort → persistence → bad options, not the order
  the brief lists them in. The list was read as a set of required segments; the persistence
  segment's "`persist: false` wrote zero files" assertion is strongest after every in-memory
  session has run (the tool-turn session, which the accessors segment reuses, and the abort
  session), which is why the abort segment comes first.
- The abort segment asserts "the script's remaining chunks never arrived" rather than an
  exact one-chunk cut. The provider checks the cancel token **between** socket reads
  (ADR-0019: "reacts to an Esc cancel within one socket chunk"), so the chunk already in
  flight may legitimately be delivered after `abort`; an exact cut is not part of the
  contract. The check is therefore `≤ 2` text events and the scripted final chunk
  (` and done`) absent — the stream really was cut.
- The resume is proven twice over, beyond the checklist's "messages and title replayed": the
  resumed turn's request body is asserted to carry the replayed conversation (mirroring
  `tests/persistence.rs::resuming_by_session_id_replays_the_session_and_keeps_appending`),
  and the log is asserted to grow by exactly the resumed turn's messages with a single
  header / title record.
- `compact_now` is deliberately **not** in the demo: it is ticket 06's deliverable and has
  Rust-level coverage (`tests/compact_now.rs`); ticket 07's checklist does not name it.
  Ticket 08's `docs/development.md` section should therefore describe the demo as the five
  segments above, not as a compaction demo.
- Object comparisons in the demo use a canonical (key-sorted) JSON form so the assertions
  test values, not the serializer's key order; array/kind comparisons stay order-sensitive
  because the order is the contract.