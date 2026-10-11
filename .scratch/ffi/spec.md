# slimcode FFI 模块（cdylib + 纯 C ABI）

Status: resolved

来源：`.scratch/sdk/spec.md` §后续工作 1、ADR-0021 D1（「FFI 模块是独立 follow-up cdylib」）。SDK 已落地（ADR-0021 / ADR-0022），本 spec 承接 FFI 这一半。

## Problem Statement

slimcode 的能力目前有两个消费面：CLI 二进制（薄壳，ADR-0022）与 Rust 侧的 `slimcode-sdk`。**非 Rust 宿主没有入口**——Node / Python 程序的唯一办法是起 `slimcode` 子进程、解析它的终端输出，拿不到结构化事件、取消控制与会话状态。

ADR-0021 已经定下路线：FFI 模块是一个**独立的 cdylib crate**（纯 C ABI、不透明句柄），依赖 `slimcode-sdk`，让 Node/Python 薄绑定能够驱动会话。SDK 侧同步、单飞、事件在回合线程同步送达的架构已经稳定，缺的只是这层 ABI 与一份能证明它真的能被另一种语言驱动的验收证据。

## Solution

新增 workspace crate **`slimcode-ffi`**（cdylib + rlib，依赖仅 `slimcode-sdk`）与一份手写 C 头文件，把 `AgentSession` 暴露为**不透明句柄 + 纯 C ABI**：

- 会话：`create`（选项走 JSON）/ `prompt`（同步跑完一轮，返回终止码）/ `abort`（任意线程）/ `dispose` / `compact_now`；
- 事件：**per-handle 事件队列**——会话的每个 `SdkEvent` 编码为一条扁平 JSON 入队，宿主用阻塞式 `next_event(timeout)` 取（实时流 = 宿主在工作线程跑 `prompt`，主线程 pump 队列）；
- 只读状态：`session_id` / `title` / `usage_json` / `messages_json`（`AgentMessage` 沿用会话日志的 JSON 形状）；
- 错误：失败返回 `NULL`/`-1` + thread-local `last_error`；返回的字符串归库所有，`free_string` 释放；
- 契约载体：手写头文件 `include/slimcode.h`，含符号、类型、所有权、线程语义与 ABI 版本。

验收证据是**一个零依赖的 Node demo**（Node ≥ 26 内建 `node:ffi`，不需要 koffi/npm）：内嵌本地伪 Bailian SSE 端点，跑「工具回合 + 实时事件流 + 跨线程 abort + 状态断言 + 持久化/回放」，自断言后打印 `PASS`/`FAIL`。

### ABI 契约（decision-rich 形状）

```c
uint32_t slimcode_abi_version(void);                            /* 1 */

/* options_json: UTF-8 JSON。失败返回 NULL + last_error。 */
void*    slimcode_session_create(const char* options_json);

/* 0 = Completed, 1 = Cancelled, -1 = 错误（见 last_error）。同步、单飞。 */
int32_t  slimcode_session_prompt(void* session, const char* text);

/* 阻塞至多 timeout_ms（< 0 = 无限）取一条事件；返回 JSON 串（free_string 释放），
   超时 / 无事件 / 已阻塞时被 dispose 唤醒返回 NULL，此时 last_error 保持空。
   note: 「调用时」句柄就 NULL / 未知 / 已 dispose 是错误（NULL + last_error 非空），
   与等待中被 dispose 唤醒（非错误）区分开。 */
char*    slimcode_session_next_event(void* session, int32_t timeout_ms);

/* {"before":N,"after":M}；失败返回 NULL + last_error。 */
char*    slimcode_session_compact_now(void* session);

void     slimcode_session_abort(void* session);                 /* 任意线程、不等会话锁 */
void     slimcode_session_dispose(void* session);               /* 唤醒等待者、后置句柄失效 */

char*    slimcode_session_id(void* session);
char*    slimcode_session_title(void* session);                 /* 无标题 = 空串 */
char*    slimcode_session_usage_json(void* session);
char*    slimcode_session_messages_json(void* session);         /* AgentMessage 的 JSON 数组 */

/* thread-local 最近一次错误（NULL = 无）；库所有、不可 free。规则只写一次：每个可失败入口进入时
   清空旧消息、失败时设置新消息，从不碰它的是 abi_version / abort / dispose / last_error /
   free_string，所以指针在同线程下一次会更新它的调用前有效。 */
const char* slimcode_last_error(void);

void     slimcode_free_string(char* s);
```

选项 JSON（未知字段直接报错，避免 `api_key` 拼错被静默忽略）：

```json
{ "cwd": "/path", "persist": true, "session_id": "slimcode-…",
  "base_url": "http://…", "model": "…", "api_key": "…",
  "cache": false, "home": "/path/to/slimcode-home" }
```

事件 JSON：扁平 `kind`，与 `SdkEvent` 一一对应，映射在 FFI 侧穷尽（`DisplayItem` 新增变体 → 编译错误，逼着 wire 契约一起更新）：

```json
{"kind":"turn","turn":1}
{"kind":"reasoning","text":"…"}
{"kind":"text","text":"…"}
{"kind":"tool_start","tool_call_id":"…","name":"…","arguments":"{…}"}
{"kind":"tool_result","tool_call_id":"…","name":"…","ok":true,"result":"…"}
{"kind":"stop","reason":"completed"|"cancelled"}
{"kind":"usage","usage":{"prompt_tokens":0,"completion_tokens":0,"total_tokens":0,
                         "prompt_tokens_details":{"cached_tokens":0,"cache_creation_input_tokens":0}}}
{"kind":"message","message":{"kind":"llm","…":"…"},"usage":null}
{"kind":"turn_finished","stop_reason":"completed"|"cancelled"}
{"kind":"compacted","before":0,"after":0}
```

## User Stories

1. 作为 Node 宿主开发者，我想用一次 `slimcode_session_create(options_json)` 拿到会话句柄，以便在进程内驱动 slimcode，而不是起子进程解析终端输出。
2. 作为 Node 宿主开发者，我想用 `node:ffi` 内建模块直接 `dlopen` 这份 cdylib，以便零安装（不需要编译原生插件、不需要 npm 依赖）就能接上。
3. 作为宿主开发者，我想让选项走 JSON（cwd / persist / base_url / model / api_key / cache / home / session_id），以便在 JS 里直接构造对象、加字段不破 ABI。
4. 作为宿主开发者，我想在选项里显式给 `base_url` / `model` / `api_key`，以便指向自建端点而不依赖 `config.toml` 与 API key 环境变量。
5. 作为宿主开发者，我想让未知的选项字段报错，以便 `api_key` 这类字段拼错时立刻失败而不是静默走默认值。
6. 作为宿主开发者，我想 `prompt(handle, text)` 同步跑完一整轮并返回终止码（completed / cancelled / error），以便用最简单的同步模型拿到结果。
7. 作为宿主开发者，我想把 `prompt` 放在自己的工作线程上跑，以便主线程不被阻塞（UI、事件循环）。
8. 作为宿主开发者，我想用阻塞式 `next_event(handle, timeout_ms)` 从队列取事件，以便 C 侧永不回调进宿主语言（无 GIL / 线程回调的隐忧），任意语言都能消费。
9. 作为宿主开发者，我想让超时返回 NULL 而不污染 `last_error`，以便区分「暂时没事件」与「出错了」。
10. 作为宿主开发者，我想让 `timeout_ms < 0` 表示无限等待，以便写一个纯粹的 pump 循环。
11. 作为宿主开发者，我想收到流式事件（`turn` / `reasoning` / `text` / `tool_start` / `tool_result` / `stop` / `usage`），以便实时渲染与展示进度。
12. 作为宿主开发者，我想收到 `message` 事件（每条进入历史的 `AgentMessage`，assistant 带 usage、用户 prompt 与工具结果为 null），以便按消息转发、统计 token 开销。
13. 作为宿主开发者，我想让 `message.message` 用会话日志的 JSON 形状（`{"kind":"llm",…}` / `{"kind":"compact_summary",…}`），以便与 JSONL 会话日志共用一套解析代码。
14. 作为宿主开发者，我想在回合结束时收到 `turn_finished`（含 `completed` / `cancelled`），以便知道包括回合后处理在内整轮已结束。
15. 作为宿主开发者，我想在压缩发生时收到 `compacted`（before / after），以便提示上下文被压缩。
16. 作为宿主开发者，我想从另一个线程调用 `abort(handle)` 取消在途回合，以便实现超时与用户取消；取消不等会话锁、不阻塞。
17. 作为宿主开发者，我想在取消后拿到 `prompt` 返回的 cancelled，且已流出的 partial 文本只作为 display 事件交付、不进历史，以便会话状态一致。
18. 作为宿主开发者，我想调用 `dispose(handle)` 释放会话，并让被阻塞的 `next_event` 立刻醒来（返回 NULL）、后续 `prompt` 报错，以便干净收尾、无需额外握手。
19. 作为宿主开发者，我想让 `dispose` 后句柄失效这一事实写进头文件，以便我不去二次释放。
20. 作为宿主开发者，我想在另一个线程调用 `dispose`，以便主线程只管 UI。
21. 作为宿主开发者，我想调用 `compact_now(handle)` 在回合外强制压缩并拿到 before/after，以便实现「/compact」这类命令。
22. 作为宿主开发者，我想在队列里看到 `compact_now` 产生的 `compacted` 事件，以便同一套事件渲染路径覆盖两种压缩来源。
23. 作为宿主开发者，我想读取 `session_id` / `title` / `usage_json` / `messages_json`，以便在回合之间做展示、跳转或断言。
24. 作为宿主开发者，我想 `title` 在无标题时返回空串而不是 NULL，以便不必区分「未命名」与「出错」。
25. 作为宿主开发者，我想让失败统一走 `-1`/`NULL` + thread-local `last_error()`，以便一处拿到人类可读的原因。
26. 作为宿主开发者，我想让 `last_error()` 返回的指针不归我释放、且下次同线程调用前有效，以便我只做一次字符串拷贝、不冒内存错误的风险。
27. 作为宿主开发者，我想让 `abi_version()` 可查，以便绑定层能断言自己载入的是期望的 ABI。
28. 作为宿主开发者，我想在任何边界都不被 Rust panic 打断（跨 ABI unwind 是 UB），以便一个内部 bug 表现为错误码而不是进程崩溃。
29. 作为宿主开发者，我想让默认持久化（`persist: true`）与 SDK 一致，以便宿主不写选项也能得到与 CLI 相同的行为。
30. 作为宿主开发者，我想用 `persist: false` + 临时 home 得到零文件会话，以便在无磁盘写入要求的宿主里嵌入。
31. 作为宿主开发者，我想用 `session_id` 续一个已落盘的会话（messages / 标题回放），以便做出「继续上次的会话」。
32. 作为宿主开发者，我想让默认七工具绑定会话自己的取消令牌，以便 `abort`/`dispose` 也能杀掉正在跑的 `bash` 子进程。
33. 作为宿主开发者，我想让多个句柄互不干扰（各自的队列、事件、日志），以便一个进程里跑多个会话。
34. 作为宿主开发者，我想用 Node 26 内建 `node:ffi`（`dlopen` + `functions` + `toString` + `free_string` 组合）驱动它，并且知道它在 `worker_threads` 里同样可用（已实测），以便把回合放进 worker、主线程实时 pump。
35. 作为 Node 宿主开发者，我想在 demo 里看到「工具回合 / 实时流 / 跨线程 abort / 状态断言 / 持久化回放」五段全部自断言并打印 PASS，以便我照着它写自己的集成。
36. 作为 Node 宿主开发者，我想在 demo 的 README 里看到 Node 版本与 FFI 支持的前提（`--no-experimental-ffi` 会禁用该模块）以及「更老的 Node 用 koffi，同一套 C ABI」的说明，以便失败时知道往哪走。
37. 作为未来 Python 宿主，我想这一层 C ABI 不依赖任何 Node/Python 特有机制（无回调进宿主语言、无宿主运行时假设），以便 ctypes/cffi 也能照同一份头文件接上（绑定本身留后续）。
38. 作为 slimcode 维护者，我想让 `slimcode-ffi` 只依赖 `slimcode-sdk`（不碰 app/ai/core/cli），以便 FFI 永远只是薄薄一层、SDK 仍是唯一的编排实现。
39. 作为 slimcode 维护者，我想让依赖矩阵测试断言 `ffi → 仅 sdk`，以便这条边界在评审中不会悄悄腐化。
40. 作为测试作者，我想用 stdlib-only 的 mock SSE 端点伪造 provider，以便 `cargo test` 离线、无需 API key 就能覆盖完整的 ABI 回合路径。
41. 作为测试作者，我想让事件 JSON 与 `DisplayItem` 的映射是穷尽 match，以便 DisplayItem 加变体时测试立刻发现 wire 契约没更新。
42. 作为仓库维护者，我想在 ADR 里读到「为什么是队列而不是回调」「为什么是 JSON 而不是 C struct」「为什么 cdylib 同时要 rlib」的取舍与证据，以便后来者不重走一遍。

## Implementation Decisions

### 1. 新 crate `slimcode-ffi`：cdylib + rlib，只依赖 SDK

workspace 新增成员，包名 `slimcode-ffi`，依赖 `slimcode-sdk`（→ `app` → `ai`/`core`/`commands`）与 `serde`/`serde_json`；**不依赖** `app`/`ai`/`core`/`commands`/`cli`/`tui`。`crate-type = ["cdylib", "rlib"]`——`cdylib` 是发布形态，`rlib` 是集成测试能链接的前提（实测：仅 `cdylib` 时 `tests/` 里连 `use` 都解析不了，E0433）。产物 `libslimcode_ffi.{dylib,so,dll}`。架构断言测试新增一行 `ffi → 仅 sdk`，`cli` 行不动。

### 2. 事件投递：per-handle 队列 + 阻塞式 `next_event`；v1 不做回调

`create` 时 FFI 就为句柄装一个 SDK 监听器，把每个 `SdkEvent` 编码后推入该句柄的队列（`Mutex<VecDeque>` + `Condvar`）；`next_event(handle, timeout_ms)` 阻塞取一条，`timeout_ms < 0` 无限等待。

理由（写入 ADR）：C 侧**永不回调进宿主语言**，因此不依赖宿主 FFI 库跨线程回调的语义——`node:ffi` 的文档明确要求 `registerCallback` 创建的回调**必须在创建它的同一系统线程上被调用**，而 slimcode 的事件来自回合线程；koffi/ctypes 在这条路径上的行为未能证实。队列方案在任意语言、任意线程模型下都成立，且实现与测试面最小。回调是**纯增量**（新增 `subscribe` 符号即可不破 ABI），留作后续。

队列**无界**：宿主必须边跑边取（demo 与文档都演示这一点）；未取走的事件留在队列里，不丢弃、不合并（当前 wire 类型不可能编码失败；万一失败，监听器把事件记进回合线程的 `last_error` 而不是静默丢弃）。

### 3. 载荷编码：JSON 字符串；3 个 additive serde derive

选项、事件、只读访问器全部走 UTF-8 JSON 串（C 侧只见 `char*` + `free_string`），`prompt` 返回 `int32_t` 终止码。

- 选项用 JSON 而非带 `struct_size` 的 C struct：加字段不破 ABI、Node/Python 天然构造对象、C 侧符号面更小。
- 事件用**FFI 自有的扁平 wire enum**（`{"kind":…}`）+ 穷尽映射，而不是直接序列化 `SdkEvent`/`DisplayItem`：serde 的 internally-tagged 表示装不下 `DisplayItem::Text(String)` 这类 newtype-in-primitive 变体（运行时报错），且扁平形状正是宿主 `switch (e.kind)` 想消费的；穷尽 match 保证 `DisplayItem` 加变体时编译失败。
- 需要的 additive serde：`TokenUsage` 与 `PromptTokensDetails`（`ai`）补 `Serialize`（现只有 `Deserialize`，不对称）、`StopReason`（`core`）补 `Serialize` + `rename_all = "snake_case"`。`AgentMessage` 已有 `Serialize`，`message` 事件的载荷直接沿用会话日志形状。`app`/`core`/`ai` 无其它改动，`DisplayItem` 不加 derive（由 FFI 映射覆盖）。

### 4. 错误、所有权与 panic

- 每个可能失败的 `extern "C"` 函数体 `catch_unwind`：panic 一律转成错误码，绝不跨 ABI unwind（唯一例外是只加载常量的 `slimcode_abi_version`，它没有可 panic 的操作）。
- 失败 = `NULL` / `-1` + thread-local `slimcode_last_error()`（库所有、不可 free；每个可失败入口进入时清空旧消息、失败时设置新消息，从不碰它的是 `abi_version` / `abort` / `dispose` / `last_error` / `free_string`，所以指针在同线程下一次会更新它的调用前有效）。
- 返回的字符串一律由库分配，宿主用 `slimcode_free_string` 释放；`last_error` 的指针**不**可 free（头文件写明）。
- `Cancelled` 不是错误：`prompt` 返回 1，`last_error` 保持空。
- `NULL` 句柄：`abort`/`dispose` 忽略（no-op），其余返回 `NULL`/`-1` + last_error。

### 5. 选项与默认值

选项 JSON 字段：`cwd`、`persist`（默认 `true`，与 SDK/CLI 语义一致，ADR-0021 D3）、`session_id`、`base_url`、`model`、`api_key`、`cache`、`home`。解析用 `deny_unknown_fields`（未知字段 = 错误）。缺省回落 SDK 的四层配置解析与默认资源组装（AGENTS.md 上下文文件、skills、环境信息）。

FFI **不暴露** provider / tools / skills 注入：Rust trait 对象与 `Tool`/`Skill` 值无法跨 C ABI 表达。宿主唯一可替换的外部依赖是 HTTP 端点（`base_url`），测试与 demo 因此伪造**端点**而不是 provider——保证「被测试的产物 == 发布的产物」。

### 6. 生命周期语义

- `prompt` 同步、单飞（SDK 语义）：并发第二个 `prompt` → `-1` + 「a turn is already running」。
- `abort` 任意线程、任意时刻可调，只碰 SDK 的取消令牌，不等会话锁。
- `dispose` = dispose SDK 会话 + 唤醒所有阻塞在 `next_event` 的线程（返回 `NULL`）+ 清空队列 + 释放句柄；此后该指针失效（文档化，宿主不得再用，亦不得二次 `dispose`）。
- 只读访问器与 `compact_now` 与 `prompt` 共用 SDK 的单飞语义（`compact_now` 与回合互斥）。
- 默认七工具经 SDK 绑定会话自己的取消令牌，所以 `abort`/`dispose` 也能终止在途 `bash` 子进程。

### 7. 头文件是契约

手写 `include/slimcode.h`：符号与签名、`int32_t` 终止码含义、`timeout_ms` 语义、句柄不透明与生命期、字符串所有权、thread-local 错误、线程语义（哪些可从任意线程调用）、`slimcode_abi_version()` 与版本策略。绑定作者只读这一份文件即可；实现符号与头文件的一致性由集成测试覆盖（每条符号都被调用一次）。

## Testing Decisions

**唯一的测试 seam：C ABI 函数本身。** 集成测试位于 `slimcode-ffi` 的 `tests/`，直接 `unsafe` 调用 `extern "C"` 符号（crate 带 `rlib` 才能链接）；不新增任何 Rust 层的测试专用入口，`sdk`/`app` 不暴露测试 API。

**provider 的 seam 在端点，不在 trait 对象**：测试用 stdlib-only 的本地 mock SSE 端点（`TcpListener` 起线程，按请求顺序服务脚本化 `data: {chunk}` 事件，`[DONE]` 收尾；请求头与请求体都被捕获以便断言）。先例：`crates/cli/tests/tui_smoke.rs` 的 `MockServer`/`read_request`/`sse(tool)`，以及 `crates/ai/src/provider.rs` 的内联 mock。

**什么算好测试**：只断言外部行为——返回码、`next_event` 给出的 JSON 串（按 `kind` 断字段）、`last_error` 的非空/空、以及文件系统副作用（哪个目录有/没有 `.jsonl`）。不断言内部结构、不读 Rust 侧私有状态、不依赖事件以外的时序假设。

**测试清单**（行为级）：

1. `abi_version` 返回 1；每条导出符号至少被调用一次（符号与头文件一致）。
2. `create`：`{}` 成功；坏 JSON / 未知字段 / 非法 `home` 组合 → `NULL` + `last_error` 非空。
3. 选项抵达端点：显式 `base_url` / `model` / `api_key` 出现在请求 URL / 请求体 / `Authorization` 头。
4. 纯文本回合：事件顺序 `turn` →（`reasoning`）→ `text` → `stop` → `turn_finished`，`message` 事件（用户 prompt 无 usage、assistant 带 usage），`prompt` 返回 0。
5. 工具回合：`tool_start`（`ls`）+ `tool_result`（ok）+ 第二次 HTTP 请求的请求体含工具结果；回合仍以 `turn_finished` 收尾。
6. 失败回合：端点 500 → `prompt` 返回 -1、`last_error` 非空，会话仍可继续（下一次 prompt 正常）。
7. 单飞：一个回合进行中再 `prompt` → -1。
8. 取消：在另一个 OS 线程 `abort` → `prompt` 返回 1，队列里有 `{"kind":"turn_finished","stop_reason":"cancelled"}`，且 partial 文本只出现在 display 事件里。
9. `next_event` 超时：空闲会话上短超时 → `NULL` 且 `last_error` 为空。
10. `dispose`：唤醒被阻塞的 pump 线程（`NULL`）、后续 `prompt` -1、在途回合被取消。
11. `compact_now`：返回 `{"before":N,"after":M}` 且队列里出现 `compacted`；压缩失败（端点报错）→ `NULL` + last_error、历史不变。
12. 访问器：`messages_json` 是 JSON 数组且含 prompt/assistant 消息；`usage_json` 反映回合用量；`session_id` 非空；无标题会话 `title` 为空串。
13. 持久化：`persist: true` 落 `<home>/sessions/<project-key>/<id>.jsonl`；`persist: false` 时临时 home 目录零文件；`session_id` 续会回放 messages 与标题。
14. 并发会话：两个句柄各自跑一个回合（不同线程）→ 事件互不串、日志各自独立。
15. `free_string`：对每个返回串调用一次不崩；`last_error` 的指针不被释放（不调用 `free_string`，仅读取）。

## Out of Scope

- **Python（ctypes/cffi）绑定与 demo**：本 slice 只做 Node demo；C ABI 本身不偏向任何宿主语言，Python 绑定留后续。
- **koffi / node-ffi 适配**：demo 只用 Node ≥ 26 内建 `node:ffi`；koffi 仅在 demo README 里作为「老 Node 的替代路径」说明（同一套 C ABI），不写适配代码。
- **`subscribe` 回调式事件投递**：v1 只有队列；回调是后续的增量符号。
- **异步 `prompt` / FFI 自持回合线程**：v1 同步阻塞；需要实时流的宿主自己起线程（demo 演示）。
- **provider / tools / skills 注入**：无法跨 C ABI，永久不做（除非将来引入序列化的工具描述）。
- **多会话管理**（SDK 的 `new`/`switch`/`fork`/`import` 等价物）：一个句柄一个会话。
- **有界队列 / 事件丢弃策略**：v1 无界，宿主自行及时取走。
- **CLI 侧任何改动**：CLI 已经薄壳化（ADR-0022），本 slice 不碰它。
- **打包与分发**（npm 包、预编译产物、CI 交叉编译）：只保证本地 `cargo build -p slimcode-ffi` 出 dylib。

## Further Notes

- **术语（写入 CONTEXT.md）**：更新既有 `FFI 模块` 词条（去掉「未实现」，写成：cdylib + 纯 C ABI、不透明句柄、JSON 选项/事件、句柄生命期）；新增 `FFI 事件队列` 词条（per-handle、无界、`next_event` 阻塞取、`timeout_ms < 0` 无限等待、超时不置错）。
- **ADR**：新增 ADR-0023，记录本 slice 的分层与四项取舍：cdylib + rlib 的原因；队列而非回调（附 `node:ffi` 回调同线程约束这条证据）；JSON 载荷与「为什么 wire enum 由 FFI 拥有而不是给 `DisplayItem` 加 derive」；`persist` 默认 `true` 沿用 ADR-0021 D3。顺带在 ADR-0021 的 follow-up 段落下留指针（若不改 ADR-0021，则在本 ADR 里引用它的 D1）。
- **docs 同步**：`docs/development.md` 的 crate 表加 `crates/ffi` 行，并新增一节写符号表、事件 JSON 形状、demo 运行命令；`docs/index.md` 的 ADR 摘要加 ADR-0023。README 保持 CLI 用户视角，不写 FFI。
- **验收 demo（Node）**：`crates/ffi/demo/node/demo.mjs` 单文件自包含（`isMainThread` 分支 + 内嵌 `node:http` 伪 SSE 端点），`crates/ffi/demo/README.md` 说明前提与预期输出，`crates/ffi/demo/run.sh` 一条命令（build + run）作为验收 gate。
- **对齐 pi 的映射**：pi 的跨进程/跨语言路径是实验性的 protocol/client/server（远程协议），slimcode 选择进程内 C ABI + 宿主薄绑定；SDK 层面 `AgentSession.prompt/subscribe/abort/dispose` 的映射见 `.scratch/sdk/spec.md`。slimcode 同步架构不变：FFI 不做 async。
- **性能/线程说明**：事件在回合线程同步入队（一次 mutex + condvar 通知），pump 线程阻塞等待时零轮询；若宿主需要多路复用（多个会话一个 pump），自行把 `next_event` 放进自己的事件循环或每会话一个线程——FFI 不提供多路复用（后续可考虑 `next_event_any`）。
- **后续工作（另行文档化）**：`subscribe` 回调符号；Python（ctypes/cffi）绑定与 demo；koffi 适配与 npm 包；有界队列/丢弃计数事件；多路复用 `next_event_any`。

## Code review follow-up

Two independent reviews (standards axis + spec axis) ran over `git diff main...ffi`. This section
records the triage of that pass (branch `ffi-review-fixes`, on top of `ffi` tip `4059212`).

| Item | Disposition | Reason |
| --- | --- | --- |
| F1 code comments in English | fixed | `AGENTS.md` requires English-only code comments; the four comments this branch added (`ffi/src/wire.rs`, `ai/src/wire.rs`, `core/src/agent.rs`, `ffi/tests/abi_contract.rs`) are rewritten. |
| F2 dispose cancels an in-flight turn | fixed | `tests/lifecycle.rs::dispose_cancels_an_in_flight_turn` pins Testing item 10's third clause against a gated SSE reply; deliberately breaking `dispose` makes it fail (`got 0` instead of `1`). |
| F3 `last_error` lifetime wording | fixed | The header states the rule once (fallible entry points clear on entry / set on failure; `abi_version` / `abort` / `dispose` / `last_error` / `free_string` never touch it; the pointer is valid until the next call that updates it); `development.md`, `CONTEXT.md`, ADR-0023 and the source docs say the same. |
| F4 `next_event` NULL semantics in docs | fixed | Behaviour is unchanged (and pinned by tests); `development.md`'s event section and this spec's listing now say the "already disposed at call time" case is an error while the wake-up case is not. |
| F5 interior NUL escape | fixed (docs only) | The escape stays; the header and `development.md` document it. No test added: reaching it needs a NUL inside a value the library returns (a hand-edited session log's title, or a provider error body, which `String::from_utf8_lossy` preserves), and a private-helper unit test would break the slice's rule that the only test seam is the C ABI. |
| F6 `abi_version` "never fails" | fixed | The `catch_panic(0, …)` guard is dropped — a constant load cannot unwind, and a `0` fallback would be indistinguishable from a real ABI mismatch; the no-unwind rule still holds (no fallible operation remains). |
| F7 silently dropped unencodable event | fixed | The listener records the drop via `error::set` on the turn's thread instead of skipping silently; `CONTEXT.md` and `development.md` state the exception. |
| F8 test-claim wording | fixed | The docs now name `the_abi_is_exactly_the_spec_symbol_set` and the `ABI_SYMBOLS` transcription instead of claiming the test reads this spec. |
| F9 demo ADR citations | fixed | The cancel wording is `crates/ai/src/provider.rs:202` / ADR-0006 D6a (not ADR-0019); the closing message is ADR-0009 D5 / `crates/sdk/src/lib.rs:826` (not D3). |
| F10 subsumed ABI test | fixed | `every_implemented_symbol_is_declared_in_the_header` is deleted; `the_abi_is_exactly_the_spec_symbol_set` (three-way set equality) covers it. |
| F11 duplicated `mock_session` | fixed | The four byte-identical bodies (only the doc comments differed) moved to `tests/common/mod.rs`. |

Accepted trade-offs (report-only, unchanged): `smoke.mjs` is a subset of `demo.mjs`; one ABI file per
ticket; `tests/common/mod.rs` grows append-only; the header/impl check is textual (signatures are
pinned by the ABI integration tests); `[workspace.dependencies] slimcode-ffi` has no in-repo
consumer; `ABI_SYMBOLS` is a deliberate transcription of the spec's symbol table.
