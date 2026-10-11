# 08: ADR-0023 与剩余文档同步

**What to build:** 把这一层的取舍与用法写进仓库文档，让后来者不必重走设计过程。

**Blocked by:** 01, 02, 03, 04, 05, 06, 07

**Status:** resolved

- [x] ADR-0023：cdylib + rlib（rlib 是集成测试可链接的前提，实测仅 cdylib 时 E0433）；事件走队列而非回调（证据：`node:ffi` 的 `registerCallback` 要求回调在创建它的同一系统线程被调用，而事件来自回合线程；koffi/ctypes 跨线程行为未证实；回调可后续增量补）；JSON 载荷 + wire enum 由 FFI 拥有（serde internally-tagged 装不下 `DisplayItem::Text(String)`）；`persist` 默认 true 沿用 ADR-0021 D3；v1 边界与备选方案
- [x] `docs/development.md` 新增 FFI 章节：符号表、事件 JSON 形状、错误/所有权约定、demo 命令
- [x] `docs/index.md` 同步 ADR-0023 摘要与指针
- [x] `.scratch/sdk/spec.md` 的「后续工作 1」加一行指向 `.scratch/ffi/spec.md`

**Gate:** 文档与实现逐项一致；链接可达。

## Answer

**What was written**

- `docs/adr/0023-ffi-c-abi-queue-and-json.md` (new, `aa68f19`): the four trades with
evidence — D1 cdylib + rlib (a cdylib-only crate cannot be `use`d by the integration
tests, E0433; the rlib is the testable half, the cdylib the shipped artifact); D2
per-handle queue + blocking `next_event` instead of callbacks (`node:ffi`'s
`registerCallback` same-system-thread rule vs slimcode's turn-thread delivery;
koffi/ctypes cross-thread behaviour unproven; `subscribe` stays additive); D3 JSON
payloads and the FFI-owned flat wire enum (serde internally-tagged cannot carry
`DisplayItem::Text(String)`-style newtype-in-primitive variants; `{"kind":…}` is the
shape a host switches on; the exhaustive match makes a new `DisplayItem` variant a
compile error; the three additive `Serialize` derives in `ai` (`TokenUsage`,
`PromptTokensDetails`) and `core` (`StopReason` + snake_case) and the reused
`AgentMessage` session-log shape are recorded here); D4 `persist` defaults to `true`
(ADR-0021 D3) with the v1 boundaries (no provider/tools/skills injection, no async
`prompt`, one session per handle, unbounded queue, no multiplexing) and the ownership
/ error / threading contract. `## Considered Options` names callbacks, direct
`SdkEvent` serialization, `struct_size` C structs, async, injection, and a bounded
queue; `## Consequences` lists the architecture-test row, the three derives, the docs
sync, the acceptance evidence and the follow-up list. Cross-references: ADR-0011 D4,
ADR-0019 D3, ADR-0021 D1/D3/D4/D5, ADR-0022.
- `docs/development.md` (new `### crates/ffi 纯 C ABI` section + table row, `aa68f19`):
module map and the `cdylib`/`rlib` layering with the `ffi → 仅 sdk` assertion
(`ffi_depends_on_sdk_only`); the 13-symbol table with return and ownership/threading
semantics; the options JSON (8 fields, `deny_unknown_fields`, `persist` default,
`session_id` + `persist: false`, no injection); the 10 event `kind`s with the
`message` / `usage` shapes and the pinned turn order; errors / string ownership /
thread-local `last_error` / the two `NULL` kinds / panic containment / single-flight /
handle lifetime / unbounded queue; and how to run the acceptance evidence
(`bash crates/ffi/demo/run.sh`, Node ≥ 26, `--no-experimental-ffi`, the
`ExperimentalWarning`, koffi only as a documented path, offline + temp homes,
`compact_now` covered by the Rust tests). The `crates/ffi` crate-table row now says
the whole ABI landed (it still described tickets 02–06 as pending).
- `docs/index.md` (`aa68f19`): ADR-0023 added to the ADR summary line as a direct link
to the new file.
- `crates/ffi/tests/abi_contract.rs` (`c377fb9`): new
`the_abi_is_exactly_the_spec_symbol_set` asserts set equality of `ABI_SYMBOLS`, the
symbols the header *declares* (declaration lines only, so nothing from the prose
leaks in), and the `slimcode_*` symbols `common::implemented_symbols` finds in
`src/**/*.rs` — all three pairwise. The forward assertions and the ticket-01-style
comments were kept/updated so nothing claims the reverse check is still pending.
- `crates/ffi/include/slimcode.h` (`aa68f19`): the usage example now carries a comment
saying `prompt_tokens_details` is `null`, not a zeroed object, when the endpoint
reported no details; the prose below was reworded to match.
- `CONTEXT.md` (`aa68f19`): `FFI 模块` no longer describes tickets 02–06 as pending
(it lists the 13 landed symbols and the Node demo as the evidence); the SDK entry's
"a future FFI 模块 remains follow-up work (ADR-0021)" now points at the landed module
(ADR-0023). `FFI 事件队列` matched the code as written and was left untouched.
- `.scratch/sdk/spec.md` + `.scratch/ffi/spec.md` (tracker commit): follow-up item 1
points at `.scratch/ffi/spec.md`; the FFI spec is `Status: resolved`.
- This ticket file (tracker commit): boxes ticked, status resolved, this Answer.

**Commits** (branch `ffi-t08-adr-docs`)

- `c377fb9` `test(ffi): assert the ABI symbol set in all three directions`
- `aa68f19` `docs(ffi): document the C ABI, the event wire contract and the Node demo`
- `chore(scratch): resolve the FFI ADR and docs ticket` (this file + the two spec files; the
  commit carrying this Answer)

(The hashes are the post-rebase ones; the branch was rebased onto `ffi` tip `847de1c` at the
end, which skipped ticket 07's already-applied commits and replayed these three.)

**Verification**

```bash
cd /tmp/slimcode-ffi-wt/t08 && export CARGO_TARGET_DIR=/tmp/slimcode-ffi-target
cargo fmt --all && cargo fmt --all --check     # clean
cargo clippy --all-targets --all-features --message-format=json -- -D warnings
                                               # exit 0, 0 "level":"error" / 0 "level":"warning"
cargo test --workspace                         # exit 0, every suite ok (abi_contract 15/15)
bash crates/ffi/demo/run.sh                    # exit 0
                                               # PASS: slimcode-ffi smoke — 10 checks
                                               # PASS: slimcode-ffi Node demo — 39 checks
                                               # PASS: slimcode-ffi demo (2 script(s))
```

Docs-vs-code checks (the ticket gate): the 13-symbol table diffs equal to the header's
declarations; the 10 event `kind`s match `src/wire.rs`'s `WireEvent`; the options
fields match `src/options.rs`'s `FfiOptions`; the documented turn order matches
`tests/turn_stream.rs`; the demo counts (10/39), the Node gate, `--no-experimental-ffi`,
the `ExperimentalWarning` and the artifact names match `run.sh` and the observed
output; every markdown link in the touched files resolves relative to its own
directory (e.g. `docs/index.md` → `docs/adr/0023-ffi-c-abi-queue-and-json.md`). The new
test was also exercised negatively (deleting the `slimcode_session_title` declaration
made it fail with the missing symbol in the diff, then the header was restored).

**Deviations**

- The brief's steps 5–8 are beyond this ticket's four checkboxes and were done as
specified: the reverse ABI assertion, the spec status, the two CONTEXT.md drift fixes
and the header's `prompt_tokens_details` nit. The only Rust change is the test
assertion; the header change is comment-only.
- `docs/index.md` keeps its single `./adr/` directory link for every ADR except the new
one, which is also linked directly (the ticket's gate asks that the ADR link point at
the new file).
- No commit touched `ffi`; the branch was rebased onto `ffi` at the end as instructed.