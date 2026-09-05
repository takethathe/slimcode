# LLM 上下文缓存 + 缓存命中 usage 统计

Status: ready-for-agent

## Problem Statement

slimcode 每一轮 agent turn 都把完整消息历史（system + user + assistant + tool …）
发给百炼兼容端点。system 提示（含工具定义与可自动调用 skill 广告）在多轮之间、
甚至多个会话之间几乎不变，却被每一轮当作全新输入重新计算，白白消耗推理时间与
输入 token 成本。

同时，slimcode 已经用 `stream_options.include_usage=true` 拿到最终 chunk 的
`usage`（`prompt_tokens` / `completion_tokens` / `total_tokens`），但没有解析
`usage.prompt_tokens_details.cached_tokens`，因此用户看不到「缓存命中」带来的
节省，无法判断缓存是否生效。

百炼为 OpenAI 兼容 Chat Completions 端点提供**显式缓存**：在 messages 的
`content` 上放 `cache_control: {"type": "ephemeral"}` 标记，系统以该标记为终点
向前回溯最长匹配前缀命中缓存；命中的 token 数回传在
`usage.prompt_tokens_details.cached_tokens`，创建缓存的 token 数回传在
`usage.prompt_tokens_details.cache_creation_input_tokens`。slimcode 需要一个
可开关的缓存功能，把这些字段解析并展示出来。

## Solution

在 provider 层增加一个**可选**的显式缓存开关，默认**开启**（用户决定覆盖 spec 初稿的
默认关闭：默认即享受缓存命中收益；不愿为创建缓存承担额外计费的用户可显式关闭）：

- 开启时，请求的 **system 消息** `content` 从纯字符串改为「内容块数组」，单个
  text 块携带 `cache_control: {"type": "ephemeral"}`，把「system 提示 + 工具
  定义」这段稳定前缀交给百炼缓存；其余消息保持纯字符串不变。
- 关闭时，请求字节与未开启缓存的客户端完全一致（命令行 `--no-cache` 或
  env/config 关闭）。
- `usage` 解析扩展出 `prompt_tokens_details.cached_tokens`（缓存命中 token 数）
  与 `cache_creation_input_tokens`（创建缓存 token 数），随现有
  `last_usage` / `total_usage` 一起累计。
- token 用量汇总行（one-shot 结尾与 TUI 的 `/usage`）新增缓存命中数展示；端点
  未回传该字段时显示 0，不影响现有输出。

## User Stories

1. 作为 one-shot CLI 用户，我能用 `--cache` 临时开启上下文缓存，以便多轮提示复用同一 system 前缀。
2. 作为用户，我能用环境变量 `SLIMCODE_AI_CACHE=true` 开启缓存，以便不依赖命令行参数。
3. 作为用户，我能在 `config.toml` 的 `[ai] cache = true` 开启缓存，以便会话默认生效。
4. 作为用户，缓存默认开启（用户决定），以便默认享受命中收益；不想承担创建缓存
   额外（125%）输入计费的用户可显式关闭（`--no-cache` / `SLIMCODE_AI_CACHE=false` /
   `[ai] cache = false`）。
5. 作为用户，缓存开关遵循「命令行 > 环境变量 > config.toml > 默认值」的四层优先级，以便与 `--model` / `--base-url` 行为一致。
6. 作为 one-shot 用户，运行结束的 token 用量汇总里能看到缓存命中的 token 数与命中
   百分比，以便确认缓存是否帮到了我。
7. 作为 TUI 用户，`/usage` 命令显示的累计 token 用量里同样包含缓存命中数与命中
   百分比，以便交互式确认缓存统计。
8. 作为用户，缓存命中/创建 token 数随 `total_usage` 跨轮累计，以便运行结束的汇总反映整个 run 的缓存收益。
9. 作为用户，当端点不回传 `prompt_tokens_details`（如未命中、模型不支持或未开缓存）时，汇总仍正常显示且缓存命中为 0，以便任何配置都不报错。
10. 作为用户，关闭缓存时请求字节与升级前完全一致，以便我可以放心地把缓存当作纯增量功能。
11. 作为开发者，缓存标记只落在 system 消息上，以便稳定前缀（system + 工具定义）被确定性缓存，而无需逐消息手工维护。
12. 作为开发者，`cache_control` 只加在 `content`（而非 `tools`）上，以便符合百炼「工具定义随 system 参与缓存计算」的约定。

## Implementation Decisions

### 缓存机制选择

- 采用**显式缓存**（`cache_control: {"type": "ephemeral"}`），而不是
  Responses API 的 Session 缓存（`x-dashscope-session-cache` header）：slimcode
  走的是 OpenAI 兼容 **Chat Completions**（`/chat/completions`），显式缓存在该
  接口上直接可用、命中确定性最高。
- **隐式缓存**是百炼自动行为（无需配置、命中率不确定）：本次不新增任何请求侧
  改动，但 `cached_tokens` 的解析与展示对隐式命中同样生效——即使 `cache=false`，
  用户也能在汇总里看到隐式缓存命中数。

### 配置

- `BailianConfig`（纯 provider 数据）新增布尔字段 `cache`，`new` 默认 `true`，
  提供 `with_cache(bool)` 建造式 setter（避免改 `new` 三参签名波及所有调用点）。
- `slimcode-common::config` 是四层解析的单一 owner，新增 `cache` 项：
  - 命令行 `--cache` / `--no-cache`（one-shot 前端 override；`--cache` 开启、
    `--no-cache` 显式关闭）；
  - 环境变量 `SLIMCODE_AI_CACHE`（`true`/`false`/`1`/`0`/`yes`/`no`/`on`/`off`，
    大小写不敏感；非法值启动报错，不静默忽略）；
  - `config.toml` 的 `[ai] cache`（TOML bool）；
  - 默认 `true`。
  - 逐项解析：`cache` 与 `base_url` / `model` 独立回落，互不牵连。

### 请求 wire（cache_control）

`WireMessage.content` 从 `Option<String>` 改为「字符串 或 内容块数组」的并集类型
（serde `untagged`），使同一字段既能序列化为字符串、也能序列化为数组。内容块
形状：

```json
{"type": "text", "text": "<system prompt>", "cache_control": {"type": "ephemeral"}}
```

- `message_to_wire` 增加 `cache: bool` 入参：仅当 `cache == true` 且消息角色是
  `system`（且文本非空）时，把 content 序列化为单元素块数组并携带
  `cache_control`；否则维持现状（纯字符串 / 空字符串 / 省略）。assistant 工具调用
  空 content、tool 消息必带 content、空文本省略等现有语义全部不变。
- provider 的 `chat` 把 `self.config.cache` 传给 `message_to_wire`。system 消息在
  每轮历史里都位于开头，因此多轮 turn 天然逐轮命中同一 system 前缀。

### 响应 usage（缓存命中统计）

`TokenUsage` 忠实扩展 wire 形状，新增嵌套字段（保持「不 `deny_unknown_fields`、
缺省容忍」的既有 serde 清单风格）：

```json
"usage": {
  "prompt_tokens": 3019,
  "completion_tokens": 104,
  "total_tokens": 3123,
  "prompt_tokens_details": {
    "cached_tokens": 2048,
    "cache_creation_input_tokens": 1605
  }
}
```

- `PromptTokensDetails { cached_tokens: u64, cache_creation_input_tokens: u64 }`，
  两个字段 `#[serde(default)]`（缺省视为 0）。
- `TokenUsage.prompt_tokens_details: Option<PromptTokensDetails>`，
  `#[serde(default)]`（整体缺失视为 None，如关闭缓存或端点不支持）。
- 提供访问器 `cached_tokens()` / `cache_creation_tokens()`，`None` 时返回 0，
  供累计与展示统一取值。
- `accumulate_usage` 在累加三个既有字段之外，把 `cached_tokens` 与
  `cache_creation_input_tokens` 也并入 `total_usage`。

### 展示

token 用量汇总行（one-shot 结尾与 TUI `/usage` 共用同一措辞，两处渲染点都更新）：

```
tokens: {prompt} prompt ({cached} cached, {pct}%) + {completion} completion = {total} total
```

`{cached}` 为累计缓存命中 token 数、`{pct}` 为命中百分比（`cached / prompt`，保留一位
小数、四舍五入，未命中/不支持时 0%）；创建缓存 token 数保留在 `TokenUsage` 模型上供未来
细粒度展示，不挤进汇总行（聚焦命中统计）。one-shot CLI 与 TUI 的渲染点都消费
`common::render::usage_summary`，避免两端措辞漂移。

## Testing Decisions

- 唯一（最高）测试 seam 是 `slimcode-ai` 的 **wire 边界**——请求侧
  `message_to_wire` 与响应侧 `parse_stream` / `WireChunk`，全部为纯函数，无需
  网络。好的测试断言**外部行为**（序列化出的 JSON 字节、解析出的字段值），不
  断言内部结构体字段。
- 请求侧单测（`message_to_wire`）：
  - `cache=true` + system 消息 → content 序列化为单元素块数组，块内
    `type=text`、`text` 原文、`cache_control.type=ephemeral`；
  - `cache=false`（或非 system 消息）→ content 仍为纯字符串，字节不变；
  - 既有语义回归：assistant 工具调用空 content、tool 消息必带 content、
    空文本省略，全部保持。
- 响应侧单测（`parse_stream` / 反序列化）：
  - 最终 chunk 的 `usage.prompt_tokens_details.cached_tokens` /
    `cache_creation_input_tokens` 被解析进 `TokenUsage`；
  - `usage` 整体缺失 `prompt_tokens_details` → 访问器返回 0，不 panic；
  - `usage: null` / 无 usage → 维持 None（复用既有 `parse_stream_null_usage_yields_none`）。
- 累计单测（`accumulate_usage`）：多轮样本的 cached / cache-creation 正确相加。
- 配置单测（`common::config::resolve`）：`cache` 的 override > env > file >
  default true；env 非法布尔值报错；缺省时 true。沿用现有 `env_of` /
  `resolve` 测试模式。
- 展示单测：one-shot `render_usage` 与 TUI 帧缓冲断言（prior art：
  `usage_renders_with_leading_blank_line`、`usage_renders_token_counts`）覆盖
  cached 字段与命中百分比出现在汇总行、缺省时显示 0 / 0%。
- Prior art：wire 夹具测试（`parse_stream_captures_final_usage`）、config
  `resolve` 表驱动测试、cli `render_usage`、tui `usage_renders_token_counts`。
- 不新增 live 冒烟测试（需真实 key + 网络，沿用现有 `#[ignore]` 模式）。

## Out of Scope

- Responses API 及其 Session 缓存（`x-dashscope-session-cache` / `previous_response_id`）。
- 对 user / assistant / tool 消息的精细 `cache_control` 放置（多缓存标记、并行
  工具结果合并回传等优化），本次只缓存 system 前缀。
- 隐式缓存的请求侧控制（本就无法关闭，无需改动）。
- 创建缓存 token 数在汇总行里的展示（模型保留字段，展示聚焦命中）。
- 任何费用/折扣计算或账单口径；本次只解析与展示百炼回传的原始 token 数。

## Further Notes

- 缓存默认开启（用户决定覆盖初稿的默认关闭）：默认请求即携带 `cache_control` 标记；
  显式关闭（`--no-cache` 等）后请求字节与未开启缓存的客户端完全一致。
- 缓存生效前提：被缓存的「system 提示 + 工具定义」前缀需 ≥ 1024 token 才会实际
  命中/创建缓存（百炼约束）；默认 system 提示较短时可能不触发，属运行时行为，
  不影响代码正确性——仍正常回传 `cached_tokens`（可能为 0）。
- 工具定义参与缓存计算（作为 system 的一部分），因此开启缓存时工具列表顺序/
  字段顺序/结构需保持稳定以获得命中；本次不做额外约束。
- 术语沿用 `CONTEXT.md`：缓存是 provider/wire 层行为，不改 `Context` /
  `message history` / `DisplayItem` 词条含义；`Usage` 展示单元承载扩展后的
  `TokenUsage`。
- 文档同步：`docs/configuration.md`（新增 `[ai] cache` / `SLIMCODE_AI_CACHE` /
  `--cache` 与优先级）、`docs/user-manual.md`（`--cache` 与 usage 汇总示例）、
  `docs/development.md`（ai wire 模型与 common config 变更）、
  `docs/explanation.md`（缓存机制与命中统计说明）。
- 验收：`cargo test` 全绿；`cargo fmt --all`；`cargo clippy --all-targets
  --all-features --message-format=json -- -D warnings` 0 error / 0 warning。
