# 01: ABI 骨架：句柄 / 选项 / 结构化错误 / 所有权

**What to build:** 宿主能载入 `slimcode-ffi` 的动态库（Rust 集成测试直调 ABI + 一段 Node smoke 脚本），用 JSON 选项建会话、读 `session_id` 与标题、拿到 thread-local 的结构化错误、释放库返回的字符串、`dispose` 收尾、并用 `abi_version` 校验载入的是期望的 ABI。回合路径留给 02。

**Blocked by:** None (can start immediately)

**Status:** resolved

- [x] `crates/ffi` 加入 workspace：包名 `slimcode-ffi`，`crate-type = ["cdylib", "rlib"]`，依赖仅 `slimcode-sdk` + `serde`/`serde_json`；`cargo build -p slimcode-ffi` 产出平台动态库
- [x] 架构依赖断言新增 `ffi → 仅 slimcode-sdk`；`cli` 行不动
- [x] 导出符号（`extern "C"`、符号体 `catch_unwind`、NULL 句柄容错）：`slimcode_abi_version`（=1）、`slimcode_session_create`、`slimcode_session_id`、`slimcode_session_title`（无标题 = 空串）、`slimcode_session_dispose`、`slimcode_last_error`（thread-local）、`slimcode_free_string`
- [x] 选项 JSON → `AgentSessionOptions`：`cwd`/`persist`（默认 true）/`session_id`/`base_url`/`model`/`api_key`/`cache`/`home`；未知字段直接报错
- [x] 错误/所有权：失败 = `NULL` + `last_error`；返回串归库所有、`free_string` 释放；`last_error` 不可 free、同线程下一次调用前有效；每个可失败入口先清旧错误
- [x] 3 个 additive serde derive 且各有断言线上形状的测试：`TokenUsage` / `PromptTokensDetails`（`ai`）补 `Serialize`；`StopReason`（`core`）补 `Serialize` + snake_case
- [x] 手写头文件：签名、终止码、`timeout_ms`、句柄生命期、字符串所有权、thread-local 错误、线程语义、ABI 版本
- [x] 集成测试（直调 ABI）：版本号；`{}` 与显式配置建会话；坏 JSON / 未知字段 → NULL + last_error；新会话标题为空串；`free_string(NULL)` 容错；NULL 句柄的访问器报错而 `dispose` 静默；`last_error` 不跨线程串
- [x] Node smoke（`node:ffi`：`dlopen` + `create` + `session_id` + 错误路径 + `dispose`，打印 PASS/FAIL）+ `demo/run.sh`（build + run）
- [x] 文档：crate 表加 `crates/ffi` 行；CONTEXT.md 的 `FFI 模块` 词条去掉「未实现」并写成实际形态

**Gate:** `cargo test` 全绿；`cargo fmt --all` 与 clippy（0 warning）干净；`bash crates/ffi/demo/run.sh` 打印 PASS。

## Answer

### What was built

`crates/ffi` (`slimcode-ffi`, `cdylib` + `rlib`, deps: `slimcode-sdk` + `serde` + `serde_json` only) with
the ticket-01 ABI skeleton:

- `src/abi/session.rs` — `slimcode_abi_version` (= 1), `slimcode_session_create`,
  `slimcode_session_id`, `slimcode_session_title` (empty string, never NULL), `slimcode_session_dispose`,
  `slimcode_last_error`, `slimcode_free_string`. Every body runs under `catch_panic`; every fallible one
  clears the previous `last_error` first. `dispose` is the one silent entry point (NULL/unknown no-op,
  touches neither side of `last_error`).
- `src/abi/{events,state,lifecycle,compact}.rs` — empty placeholders naming tickets 02/03/04/06, so each
  parallel branch fills exactly one file and never contends on `abi/mod.rs`.
- `src/handle.rs` — process-global `Mutex<HashMap<usize, Arc<SessionHandle>>>` keyed by the `Arc` data
  pointer; lookup clones the `Arc` before use, `take` removes the entry, `close` marks the handle dead.
- `src/error.rs` — thread-local `last_error` (`clear` / `set` / `get`, interior NULs escaped).
- `src/options.rs` — `deny_unknown_fields` options struct → `AgentSessionOptions` (`persist` absent = true).
- `include/slimcode.h` — written once, complete: all 13 spec symbols with signatures, return codes,
  `timeout_ms` semantics, options/event JSON shapes, ownership, threading, handle lifetime and the ABI
  version policy.
- `tests/abi_contract.rs` + `tests/common/mod.rs` — 14 integration tests calling the `extern "C"` symbols
  by path, including the source-scan ↔ header consistency check.
- `demo/node/smoke.mjs` + `demo/run.sh` — zero-dependency `node:ffi` smoke (10 checks) and the build+run
  gate.
- Additive `Serialize` for `TokenUsage`/`PromptTokensDetails` (`ai`, with a `to_value` round-trip test and
  a null-details test) and `Serialize + snake_case` for `StopReason` (`core`, with a wire-shape test), each
  commented `// Serialized for the FFI wire contract (ADR-0023)`.
- Docs: the `crates/ffi` row + crate count + dependency sentence in `docs/development.md`, and the rewritten
  `FFI 模块` glossary entry in `CONTEXT.md`.

### Commits (branch `ffi-t01-abi-skeleton`)

- `feat(ffi): add the slimcode-ffi workspace crate`
- `feat(ffi): expose the ABI skeleton and its C header`
- `test(ffi): pin the ticket-01 ABI contract`
- `feat(ai,core): serialize the usage and stop-reason wire shapes`
- `feat(ffi): add the Node ABI smoke test and demo runner`
- `docs(ffi): document the slimcode-ffi crate and module`
- `chore(scratch): resolve the FFI ABI skeleton ticket`

### Verification (worktree `/tmp/slimcode-ffi-wt/t01`, `CARGO_TARGET_DIR=/tmp/slimcode-ffi-target`)

- `cargo fmt --all` — no diff afterwards.
- `cargo clippy --all-targets --all-features --message-format=json -- -D warnings` — 0 lines with
  `"level":"error"`, 0 with `"level":"warning"`.
- `cargo test --workspace` — exit 0, every suite green (ffi's new `abi_contract` suite: 14 passed).
- `bash crates/ffi/demo/run.sh` — exit 0, ends with `PASS: slimcode-ffi smoke — 10 checks` and
  `PASS: slimcode-ffi demo (1 script(s))`; also verified from `/` (cwd-independent) and with
  `SLIMCODE_FFI_LIB` for the script's standalone mode.

### Deviations (and why)

1. **The `{}`-options test additionally pins `DASHSCOPE_API_KEY=sk-test`.** `{}` carries no options, and the
   SDK's four-layer config resolution *requires* an API key, so the test guard pins the scratch
   `SLIMCODE_HOME` (as the pinned decisions allow) **and** a dummy key; otherwise the test is red against a
   clean environment. `tests/common/mod.rs::home_guard` documents this.
2. **The event queue is not in `handle.rs` yet.** The pinned layout still holds, but ticket 01 has no
   caller for a queue: ticket 02 adds the mutex+condvar queue and the listener, ticket 04 adds the
   `dispose` wake-up. `handle.rs`'s module doc says exactly that (no `#[allow(dead_code)]` scaffolding).
3. **`abi_version` / `last_error` / `free_string` live in `abi/session.rs`** (as the task text directed)
   even though they are crate-wide; `abi/mod.rs` documents the split.
4. **`slimcode-ffi` was also added to `[workspace.dependencies]`** for consistency with the other seven
   members (no other crate consumes it).
5. **Extra doc lines beyond "crate 表加一行":** the crate count (`七个` → `八个`) and the dependency-assertion
   sentence got the `ffi → 仅 sdk` edge — leaving them stale would have made the page lie.
6. **Node signature field names:** the smoke script uses Node 26.x's actual `{ arguments: [...], return: … }`
   spellings. The `{ parameters, result }` spelling mentioned in the task notes is silently ignored by the
   installed Node 26.10 (`getpid()` returned `undefined` in a probe), so a script written that way would
   compile-run but assert against garbage.

### For tickets 02–06

- Helpers left in `tests/common/mod.rs` (extend additively): `scratch(tag) -> (home, cwd)`,
  `home_guard(&home) -> HomeGuard`, `test_env(tag) -> (HomeGuard, home, cwd)`, `Options` (`.set/.home/.cwd/
  .persist/.session_id/.base_url/.model/.api_key/.cache/.to_json()`), `options(home, cwd)`, `abi_version()`,
  `create(json)`, `create_ok(json)`, `session_id(handle)`, `title(handle)`, `dispose(handle)`,
  `last_error() -> Option<String>`, `take_string(*mut c_char) -> Option<String>` (copies then frees),
  `free_string(ptr)`, `implemented_symbols(src_dir) -> Vec<(String, PathBuf)>`.
- Handle convention: the C value is `Arc::as_ptr(&SessionHandle) as *mut c_void`; look up with
  `handle::lookup(ptr)` (clones the `Arc`, `None` for NULL/unknown/closed), remove with `handle::take(ptr)`
  (used by `dispose`), and mark with `SessionHandle::{close, is_closed}`. Never dereference the pointer in
  C-side code; the registry only hashes it. `dispose` does not touch `last_error`; a NULL/unknown handle is
  a no-op.
- Reusable crate internals: `crate::{str_from_c, into_c_string, catch_panic}`, `crate::error::{clear, set,
  get}`, `crate::options::FfiOptions`.