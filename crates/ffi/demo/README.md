# slimcode-ffi 验收 demo

Node 侧的验收证据：一个单文件、零依赖的 `demo.mjs`，用内置 `node:ffi` 直接
`dlopen` `libslimcode_ffi`，在本地伪 Bailian SSE 端点上跑完五段自断言，最后打印
`PASS`（全部通过）或 `FAIL`（非零退出）。它是 `crates/ffi/include/slimcode.h`
的可执行对照物：每一段断言的正是头文件写明的契约。

## 前提

- **Node ≥ 26**：`node:ffi` 是内建模块，无需 npm 包、无需 koffi、无需编译原生插件。
  - `--no-experimental-ffi`（或 `NODE_OPTIONS=--no-experimental-ffi`）会**禁用**该模块，
    demo 会直接报错——如果 `node:ffi` 导入失败，先检查这个开关。
  - `node:ffi` 在 stderr 打印一行 `ExperimentalWarning: FFI is an experimental feature…`
    是正常现象，**不是失败**；demo 的 PASS/退出码只看断言结果。
- Linux / macOS（`run.sh` 按平台解析 `libslimcode_ffi.{so,dylib}`）。
- `cargo` 可用；`run.sh` 会先 `cargo build -p slimcode-ffi`。

demo 完全离线、不需要 API key：端点绑在 `127.0.0.1:0`，选项里传 dummy `api_key`；
会话的 `home` / `cwd` 都在 `mkdtemp` 临时根下，**不读也不写真实的 `~/.slimcode`**，
结束时删除临时根。

## 一条命令运行（验收 gate）

```bash
bash crates/ffi/demo/run.sh
```

`run.sh` 可以从任意 cwd 运行（repo root 由脚本自身路径解析），并沿用
`CARGO_TARGET_DIR`（设置了就找它下面的产物）。它先跑 ticket 01 的 ABI smoke
（`node/smoke.mjs`，10 项检查），再跑完整 demo（`node/demo.mjs`，39 项检查），
每步失败都非零退出。

也可以单独跑 demo（`run.sh` 就是这么调用的）：

```bash
SLIMCODE_FFI_LIB=/path/to/libslimcode_ffi.dylib node crates/ffi/demo/node/demo.mjs
# 或
node crates/ffi/demo/node/demo.mjs /path/to/libslimcode_ffi.so
```

## 预期输出

事件行（`event …`）是主线程**实时**从事件队列取到就打印的，发生在 worker 线程还在
`prompt` 里时；`ok` / `FAIL` 是每项自断言的结果。节选：

```text
==> cargo build -p slimcode-ffi
==> library: /path/to/target/debug/libslimcode_ffi.dylib
==> node node/smoke.mjs
  ok   abi_version is 1
  ok   malformed options JSON returns NULL
  …
PASS: slimcode-ffi smoke — 10 checks
==> node node/demo.mjs
  ok   abi_version is 1

== tool turn: worker prompt + main-thread pump
  ok   create() returns a handle
  ok   a successful create clears last_error
  event  message user
  event  turn
  event  message assistant
  event  tool_start ls call_ls {"path":"."}
  event  tool_result ls ok=true
  event  message tool
  event  turn
  event  reasoning "Checking the listing."
  event  text "There it is."
  event  message assistant usage=24
  event  usage total=24
  event  stop completed
  event  turn_finished completed
  ok   the worker's prompt returned Completed (0)
  ok   events arrived while the worker was still in the turn (real-time pump)
  …
== abort: cancel an in-flight turn from the main thread
  event  text "partial-marker-7b21"
  event  stop cancelled
  event  turn_finished cancelled
  ok   a cancelled turn returns Cancelled (1)
  …
== persistence: persist: false vs a JSONL log and a session_id resume
  ok   the persist: false sessions wrote zero files
  ok   the log landed at <home>/sessions/<project-key>/<id>.jsonl
  …
PASS: slimcode-ffi Node demo — 39 checks
PASS: slimcode-ffi demo (2 script(s))
```

任何一项失败都会打印 `FAIL <检查名> — <细节>`，最后
`FAIL: slimcode-ffi Node demo — n/m checks failed` 并以非零码退出。

## `demo.mjs` 演示了什么

1. **工具回合 + 实时事件流**：worker 线程跑 `slimcode_session_prompt`（它是阻塞的），
   主线程用有限的 `next_event` 切片 pump 同一个句柄，事件到达即打印；断言整轮事件序列
   （`tool_start` / `tool_result` 配对、`turn_finished`）、`ls` 的真实输出，以及伪端点
   的第二次请求确实带回了工具结果。
2. **跨线程 abort**：主线程在 worker 的回合还在流式输出时调用 `slimcode_session_abort`；
   断言 `prompt` 返回 `1`（Cancelled）、`stop` / `turn_finished` 报告 `cancelled`、
   已流出的 partial 文本只在 display 事件里、**不**进 `messages_json` 历史。
3. **只读状态**：`session_id` / `title` / `usage_json` / `messages_json` 的值与形状，
   并与事件流里的 `message` 事件逐条对照。
4. **持久化**：`persist: false` 的会话在其临时 home 下写零个文件（连 `sessions/` 目录
   都没有）；`persist: true` 落到 `<home>/sessions/<project-key>/<id>.jsonl`，第二个
   句柄仅凭同一 `session_id` 就能回放标题与消息，且续跑继续追加到同一份日志（header
   与 title 记录都只有一条）。
5. **坏选项错误路径**：畸形 JSON 与未知字段都返回 `NULL` +
   `slimcode_last_error()`（后者点名未知字段，防止 `api_key` 拼错被静默忽略）。

demo 里的伪 provider 是按请求顺序服务的脚本化 SSE（`tool` / `slow` / `text` 三个
scenario，由 `base_url` 的路径段选择），因此整轮行为确定、可离线重放。所有等待都有
上界：`next_event` 用有限的 `timeout_ms` + 总 deadline，worker 一定会被 `terminate`。

## 更老的 Node：koffi 走同一套 C ABI

`node:ffi` 是 Node ≥ 26 的内建模块；更老的 Node 可以用 [koffi](https://koffi.dev/)
等第三方 FFI 绑定同一份 `libslimcode_ffi` 与同一份 `include/slimcode.h`（符号、JSON
载荷、字符串所有权与线程语义都写在头文件里，绑定只需照抄——`demo.mjs` 的声明对象
就是一份可对照的清单）。本仓库**不提供** koffi 适配代码，本节只是失败时的替代路径
说明（spec 的 out-of-scope 列表如此规定）。