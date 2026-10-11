# 05: 持久化：两模式与 session_id 续会

**What to build:** 宿主不写任何持久化选项就得到与 CLI 相同的行为（追加式 JSONL 会话日志，`/session` picker 兼容），也能用 `persist:false` 得到零文件会话，或用 `session_id` 续一个已落盘的会话。

**Blocked by:** 02

**Status:** resolved

- [x] 测试：默认（不写 `persist`）落出 `<home>/sessions/<project-key>/<id>.jsonl`，内容可被 SDK 的 `SessionStore::load` 回放，且与 `messages_json` 一致
- [x] 测试：`persist:false` + 临时 home → 整个回合后目录零文件
- [x] 测试：`session_id` 续会回放 messages 与标题，并继续追加同一日志
- [x] 测试：显式 `home` 生效（会话落在被 pin 的 home 下）

**Gate:** `cargo test` 全绿；fmt/clippy 干净。

## Answer

**What was built**

- `crates/ffi/tests/persistence.rs` (new, 4 tests):
  - `an_omitted_persist_option_writes_the_default_session_log` — the options JSON carries no
    `persist` key (asserted on the JSON text), and the turn persists to
    `<home>/sessions/<project-key>/<id>.jsonl`. The file is validated the way
    `SessionStore::load` reads it: a well-formed mandatory header (`type` / `v` / `id` /
    `created_at` / `project_home` = the canonical project home), typed records in order
    (`session`, `title`, `message`, `message`), a trailing newline, and `message` records
    whose payloads equal element-for-element both what the ABI reported on the event queue and
    the `messages_json` array of the same handle (the routes spec U13/D3 pin to one shape).
    The `title` record agrees with `slimcode_session_title`.
  - `persist_false_leaves_no_file_after_a_whole_turn` — a whole mock-endpoint turn
    (`turn_finished` received, one request served) leaves **zero files** anywhere under the
    scratch home, and not even a `sessions` directory.
  - `resuming_by_session_id_replays_the_session_and_keeps_appending` — a second handle created
    with `session_id` (and `persist` omitted, so the FFI default is what makes the store
    exist) keeps the id and the replayed title; the resumed turn's request body carries the
    first turn's prompt and reply (replay proven at the provider seam); and the same log
    grows — one header, one title record, the appended records equal to the resumed turn's
    `message` events.
  - `an_explicit_home_option_wins_over_the_environment_home` — with `SLIMCODE_HOME` pointed at
    a second scratch home (ticket 01's `home_guard` pattern), the session lands under the
    option's home while the environment home stays empty (no `sessions` directory).
- `crates/ffi/tests/common/mod.rs` (append-only, ticket-05 section): `persisted_logs(home)`
  (walks `<home>/sessions/**/*.jsonl`, sorted) and `log_records(path)` (one parsed JSON record
  per line, panicking on a malformed line rather than skipping it).
- `crates/ffi/tests/turn_stream.rs`: ticket 02's private walker removed in favour of
  `common::persisted_logs` (`turn_stream` keeps its behaviour; only the call site changed).

**Commits**

- `test(ffi): cover persistence modes and session_id resume`
- `refactor(ffi): share the session log walker across FFI tests`
- `chore(scratch): resolve the FFI persistence ticket`
- `test(ffi): compare the persisted log against messages_json`

Integration was a rebase onto the ticket-03 tip plus a cherry-pick to `ffi` (no merge). Only
`crates/ffi/tests/common/mod.rs` conflicted — both tickets appended a helper block at the end
of the file; both were kept (ticket 03's `usage_json` / `messages_json`, then ticket 05's
`persisted_logs` / `log_records`). The merged `ffi` tree is byte-identical to the ticket
branch's tree.

**Verification**

```
env -u CARGO_TARGET_DIR -u CARGO_BUILD_BUILD_DIR cargo fmt --all --check              # clean
env -u CARGO_TARGET_DIR -u CARGO_BUILD_BUILD_DIR cargo clippy --all-targets \
    --all-features --message-format=json -- -D warnings                               # exit 0, 0 error/warning lines
env -u CARGO_TARGET_DIR -u CARGO_BUILD_BUILD_DIR cargo test --workspace               # exit 0, 29 suites ok, 0 failures
env -u CARGO_TARGET_DIR -u CARGO_BUILD_BUILD_DIR bash crates/ffi/demo/run.sh          # PASS (10 checks)
```

The merged FFI suites: 33 tests ok (14 `abi_contract` + 4 `persistence` + 6
`tool_turn_accessors` + 9 `turn_stream`). The environment is sanitized so artifacts land in the
repository's own `target/` instead of the machine-wide `CARGO_BUILD_BUILD_DIR`.

Test sensitivity was checked by mutation before committing: flipping the FFI's `persist`
default to `false` and dropping `session_id` in `into_agent_options` made exactly the
default-persist and resume tests fail (the other two stayed green); both mutations were
reverted. The added `messages_json` comparison was checked the same way at merge time:
truncating the accessor's array made exactly the default-persist test fail.

**Deviations**

- **The log is validated structurally rather than through `SessionStore::load`.** The
  architecture test's `ffi → only sdk` row reads `[dev-dependencies]` too, and the SDK does
  not re-export `SessionStore`, so the crate cannot link the loader; `log_records` mirrors
  what `load` requires instead (a mandatory `session` header with `v` / `id` / `created_at` /
  `project_home`, then typed `title` / `message` records, panicking on a malformed line rather
  than skipping it silently).
- **The "consistent with `messages_json`" half of bullet 1 was originally asserted against
  the `message` events only**, because the branch was cut before ticket 03 landed
  `slimcode_session_messages_json`. After the rebase onto the ticket-03 tip, the same test
  also compares the stored payloads against the `messages_json` array of the same handle; the
  event-queue comparison stays as additional evidence.
- **The resumed session's message replay is proven at the provider seam**, not by reading the
  history back through the ABI: the resumed turn's request body carries the first turn's
  prompt and reply (only a real load can produce that). The id and title are asserted through
  the ABI accessors. `messages_json` exists on the merged base and could also enumerate the
  resumed handle's history; the request body was kept as the replay proof because it shows
  the replayed messages reached the model context.
- **One edit to another ticket's file**: `turn_stream.rs` lost its private walker in favour of
  the shared helper, as the brief asked ("promote/append rather than duplicating").

**For tickets 06–07**

- Reusable helpers: `common::persisted_logs(home) -> Vec<PathBuf>` (already sorted) and
  `common::log_records(path) -> Vec<Value>`.
- Log discovery convention: walk `<home>/sessions/<project-key>/<id>.jsonl` rather than
  recomputing the key — `project_key` is `<project-home-basename>-<12 hex>` from
  `slimcode-app`, which this crate cannot link (dev-dependencies included).
- Log records and `messages_json` are one shape (`message` records equal the accessor's
  array), so either route parses with one parser.
- Resume details that are easy to get wrong: the log is never recreated (exactly one `session`
  header and one `title` record after a resume), the title is replayed from the log rather
  than re-inferred, appends continue after the existing records, `persist` may be omitted on
  the resume options (the FFI default `true` supplies the store), and a resume target must
  have an assistant record — `SessionStore::cleanup_empty` at creation deletes a log without
  one.