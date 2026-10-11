# 03: 工具回合与只读访问器

**What to build:** 模型发起工具调用时宿主能看到完整链路（工具事件 + 端点收到的第二次请求携带工具结果），并能在回合之间用 `messages_json` / `usage_json` / `session_id` / `title` 读回会话状态。

**Blocked by:** 02

**Status:** resolved

- [x] 导出 `slimcode_session_messages_json`（`AgentMessage` 的 JSON 数组，沿用会话日志形状）与 `slimcode_session_usage_json`
- [x] 测试：工具回合事件（`tool_start`（`ls`）→ `tool_result`（ok）→ 后续文本回合）与「端点收到的第二次请求体包含工具结果」
- [x] 测试：选项显式 `base_url` / `model` / `api_key` 抵达端点（URL / 请求体 / `Authorization` 头）
- [x] 测试：`messages_json` 是 JSON 数组且含用户 prompt 与 assistant 消息；`usage_json` 反映本轮用量；有标题会话的 `title` 非空

## Answer

### 实现

`crates/ffi/src/abi/state.rs`（提交 `8b4b36d`）：

- `slimcode_session_usage_json` — 沿用 `abi::session` 的既有契约：`error::clear()` → `catch_panic` →
  `handle::lookup`（NULL / 已 dispose → `error::set("invalid or disposed session handle")` + `NULL`），
  然后 `serde_json::to_string(&handle.session().usage())`。载荷是派生的 `TokenUsage` 形状，与 `usage`
  事件载荷同一个对象（端点省略 cache details 时是 `"prompt_tokens_details":null`）。**`usage()` 是会话
  累计值**，不是最后一轮的值：一个工具回合的两次请求都会累加。
- `slimcode_session_messages_json` — 同一契约；序列化 `AgentSession::messages()`，得到 `AgentMessage`
  的 JSON 数组（会话日志形状：`{"kind":"llm","role":…,"parts":[{"type":"text","text":…}],
  "tool_calls":[…],"tool_call_id":…}`，压缩检查点为 `{"kind":"compact_summary",…}`）。

两者都只新增实现、未改头文件（`include/slimcode.h` 在 ticket 01 已写全，`abi_contract.rs` 的符号扫描通过）。

### 测试

`crates/ffi/tests/tool_turn_accessors.rs`（新文件，6 个用例，提交 `8618406`）：

- `a_tool_turn_emits_the_pair_and_the_next_request_carries_the_result` — `ls` 工具回合钉住完整事件序列
  （`message`/`turn`/`message`/`tool_start`/`tool_result`/`message`/`turn`/`text`/`message`/`stop`/`turn_finished`），
  `tool_result` 的 `result` 是 scratch cwd 的真实输出（`.git`）；解析**第二次**请求体断言 assistant 的
  `tool_calls`（`call_ls`/`ls`/`{"path":"."}`）与 `role:"tool"` 消息（`tool_call_id`、内容 `.git`）都在，
  并对照断言第一次请求体里没有 tool 消息。
- `explicit_endpoint_options_reach_the_request` — 用非默认值（`ticket-03-model` / `sk-ticket-03`）：
  `base_url` 体现为请求直达 mock 的 `Host` 且 target 是 `/v1/chat/completions`；`model` 在请求体；
  `api_key` 在 `Authorization: Bearer …`。
- `messages_json_returns_the_history_in_the_session_log_shape` — 无回合时是 `[]`；一次文本回合后是长度 2 的数组
  （user prompt + assistant），逐字段断言日志形状（`kind`/`role`/`parts[0].type`/`parts[0].text`），并与流出的
  `message` 事件载荷数组逐项相等。
- `usage_json_accumulates_the_turns_reported_usage` — 新会话为零值（`prompt_tokens_details:null`）；工具回合两次请求
  分别报 `11/5/16+(3,1)` 与 `20/4/24+(2,0)`，断言累计 `31/9/40`、cache details `5/1`。
- `the_title_is_inferred_from_the_first_prompt` — 回合前是空串（不是 NULL），回合后等于首个 prompt。
- `the_state_accessors_reject_null_and_disposed_handles` — NULL 与已 dispose 的句柄 → `NULL` + 非空 `last_error`。

`crates/ffi/tests/common/mod.rs` 只在末尾追加了 `usage_json(handle)` / `messages_json(handle)`
（返回解析后的 `Option<Value>`，私有 `parse_state_json`），既有内容一行未动。

### 验证

```bash
cd /tmp/slimcode-ffi-wt/t03 && export CARGO_TARGET_DIR=/tmp/slimcode-ffi-target
cargo fmt --all --check            # clean
cargo clippy --all-targets --all-features --message-format=json -- -D warnings
                                   # exit 0，JSON 流 0 个 level:error / 0 个 level:warning
cargo test --workspace             # exit 0，28 个 suite 全 ok、0 failed（tool_turn_accessors: 6 passed）
bash crates/ffi/demo/run.sh        # PASS: slimcode-ffi demo (1 script(s))
```

### 偏差

1. 多加了第 6 个用例 `the_state_accessors_reject_null_and_disposed_handles`：ticket 只列了 4 条测试，但实现被要求
   遵守 NULL / disposed 句柄的错误契约，这个用例把它钉住（还顺手钉了「无回合时 `messages_json` 为 `[]`」）。
2. 环境坑（**给 04/06/07 与合并者**）：本机环境有 `CARGO_BUILD_BUILD_DIR=/Users/takethat/.cargo/build`，
   它把 cargo 的**产物目录**（deps/build/incremental）从各 worktree 的 `CARGO_TARGET_DIR` 重定向到同一个共享目录，
   于是 `libslimcode_ffi.rlib` / `libslimcode_ffi-*.rmeta` 与 fingerprint 在所有 worktree 之间共享、跨 worktree 互相
   覆盖。表现是 `cargo clippy` 偶发/持续报 `cannot find function slimcode_session_usage_json in module state_abi`
   （别的 worktree 用旧 `state.rs` 的产物覆盖了本 worktree 的 check 产物），与代码无关。绕过办法：跑门禁前
   `touch crates/ffi/src/abi/state.rs` 让本 worktree 的源 mtime 最新，迫使 cargo 从本 worktree 重建该单元；
   门禁随即全绿。没有改 `CARGO_TARGET_DIR`，也没有动其它 worktree 的文件/产物。

**Gate:** `cargo test` 全绿；fmt/clippy 干净。