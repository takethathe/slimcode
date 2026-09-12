# 最后一条消息 cache mark + ProviderConfig seam

Status: resolved

## Problem Statement

我跑多轮会话时，只有 system 前缀命中了上下文缓存。llm-cache 落地时把 `cache_control`
mark 只打在 system 消息上：跨轮稳定的"system 前缀"能命中，但对话历史
（user/assistant/tool 消息）随轮次无限增长、从不参与缓存——多轮 turn 的后期请求
只命中 system 前缀，历史部分每轮重新计费，缓存收益被截断。

同时，`cache` 开关在 llm-cache 落地时放进了 `BailianConfig`（provider 端点配置）。
这违背了"provider 负责 LLM 强相关设置"的边界：`RunConfig`（runner 运行时配置）
本不应承载 provider 专属设置；`BailianConfig` 这个 provider 命名也不应泄漏到
app/cli 层——app 和 cli 不该知道"底层是哪个 provider"。

## Solution

多轮会话的历史部分也能命中缓存：除了 system 消息，**最后一条消息**也打上
`cache_control` mark（参考 pi 项目 `applyAnthropicCacheControl` 的
`addCacheControlToLastConversationMessage` 模式，适配 Bailian）。Bailian 以 mark
为终点向前回溯最长匹配前缀，因此"system 前缀"与"完整对话前缀"都进入缓存，下一轮
追加新消息后，此前全部历史命中、只需为新消息计费。

同时理顺归属：`cache` 开关经泛化的 **ProviderConfig**（不再用 Bailian 命名）在
**chat 边界**传递给 provider；`RunConfig` 保持纯运行时配置、不承载 cache；provider
内部按其端点要求打 mark。工具顺序今天已确定、无需 sort；wire 层以**字节确定性契约**
锁定缓存前缀稳定。

## User Stories

1. 作为多轮会话用户，我希望后续 turn 的完整历史前缀（不只是 system）命中缓存，这样
   长会话的输入成本随轮次摊薄而不是线性叠加。
2. 作为多轮会话用户，我希望 last-message mark 出现在最后一条非空 user/assistant/tool
   消息上，这样即使末尾是空文本（工具调用 / 空结果）也能回退到上一条可缓存消息。
3. 作为 one-shot 用户，我希望 `--no-cache` / `SLIMCODE_AI_CACHE=false` /
   `[ai] cache = false` 关闭缓存后请求字节与升级前完全一致，这样缓存始终是纯增量功能。
4. 作为用户，我希望关闭缓存时两条 mark 都不出现，这样无论开关如何模型看到的消息形状
   一致（请求字节只差 mark 本身）。
   > 实现注记：开启缓存时所有非空文本消息一律用数组形态（百炼只在数组形态上接受
   > `cache_control`，且按 content 块匹配前缀），因此开启缓存时字节差异不止 mark；
   > 关闭缓存时字节仍与升级前逐字节一致。见 ADR-0016 D6。
5. 作为开发者，我希望 `cache` 开关经 `ProviderConfig` 在 chat 边界传递，这样
   `RunConfig` 只承载 runner 自身的运行时行为（如 `parallel_tools`），不被 provider
   设置污染。
6. 作为开发者，我希望 provider 配置对外暴露为泛化的 `ProviderConfig`，这样 app/cli
   层不出现 `BailianConfig` 命名，不依赖具体 provider。
7. 作为开发者，我希望 `Provider::chat` 接收 `config` 入参、provider 实例本身无状态，
   这样同一 provider 实例可配合不同配置复用（测试、未来配置切换）。
8. 作为开发者，我希望 mark 的 placement 策略（打在哪些消息、按什么规则）归 provider
   内部决定，这样换端点时只改 provider、不碰运行时。
9. 作为开发者，我希望工具定义不加 mark，这样符合既有决策（`cache_control` 只加在
   content，工具定义随 system 前缀参与缓存计算）。
10. 作为开发者，我希望工具列表今天不做 sort，这样不引入模型可见的顺序变更，也不破坏
    关闭缓存时的字节一致承诺。
11. 作为开发者，我希望 wire seam 有字节确定性测试（相同 messages + tools + cache →
    相同字节），这样缓存前缀稳定性被测试锁定，未来不会有人塞进时间戳/随机字段。
12. 作为开发者，我希望 response 侧的缓存命中统计（`cached_tokens`）不受本次改动影响，
    这样 last-message mark 生效与否仍能被用户观测。
13. 作为维护者，我希望本次边界规则（provider-owned concern / runtime 配置 / provider
    config 三分）写入 ADR，这样未来加 provider 时归属不再重新争论。

## Implementation Decisions

### 边界规则（本次确立，写入 ADR-0016）

- **provider-owned concern**：实现随 LLM 提供商而异的设置归 provider。判别测试：
  "换一个 provider，这一项的实现会不同吗？"是 → provider；任何 provider 都以同样
  方式受益 → app 或共享。cache mark 语法与 placement 策略属此类。
- **runtime 配置**（`RunConfig`）：只放 runner 自身的运行时行为（`parallel_tools`），
  不放 provider 设置。
- **provider config**（`ProviderConfig`）：provider 专属设置的载体，chat 时跨 seam
  传递；命名泛化，不泄漏具体 provider 名。

### ProviderConfig seam

- `BailianConfig` 更名为 `ProviderConfig`（纯 provider 数据：api_key / base_url /
  model / cache）。字段、建造器（`new` 三参 + `with_cache`）、默认值（cache 默认
  true）、`chat_completions_url()` 全部保持不变。
- `Provider::chat` 增加 `config: &ProviderConfig` 入参：chat 时经 provider config
  传递配置（含 cache 开关）。provider 实例变为无状态——构造只构建 HTTP client。
- 穿线：runner 持有 `&ProviderConfig`（与 cancel 并列），`run` 内传给 chat；
  `run_turn` 同样增加 `config` 入参；CLI one-shot 与 TUI 各自传递。
- `RunConfig` 字段不变（仍只有 `parallel_tools`），不承载 cache。

### mark 放置（pi 模式，Bailian 适配）

- system 消息：保持现有 mark（content 序列化为单块数组 + `cache_control`）。
- 最后一条消息：自尾部扫描第一个「非空文本的 user/assistant/tool」消息，把其
  content 序列化为单块数组 + `cache_control`（与 system 同形）。空文本
  （assistant 工具调用 / 空 tool 结果）跳过并向前找。
- 工具定义：不加 mark（既有决策：`cache_control` 只加在 content）。
- `cache=false` 时两处 mark 都不打，请求字节与升级前一致。

### 字节确定性契约

- wire 层保证：相同 messages + tools + cache → 请求序列化字节逐字节相同。
- 今天不引入工具 sort / canonical 顺序：工具顺序已确定（固定工具集、skills 按名
  排序、context files root-first）；canonical 顺序的归属留待动态工具出现时再议。

## Testing Decisions

- **唯一（最高）测试 seam 是 `slimcode-ai` 的 wire 边界**——请求侧
  `message_to_wire` / `messages_to_wire` 与请求序列化，全部为纯函数、无网络。这与
  llm-cache 的最高 seam 一致，不新增 seam。好的测试断言**外部行为**（序列化出的
  JSON 字节、完整请求形状），不断言内部结构体字段。
- 请求侧单测：
  - `messages_to_wire`：system 与最后一条非空消息均打 mark（单块数组 +
    `cache_control`）；`cache=false` 时不打任何 mark；
  - 尾部空消息（assistant 工具调用 / 空 tool 结果）跳过并向前找第一条可 mark 消息；
  - 工具角色空文本不打 mark；
  - 既有语义回归：assistant 工具调用空 content、tool 消息必带 content、空文本省略。
  - **字节确定性**：相同输入两次构造请求，序列化字节逐字节相同；并锁定完整请求
    JSON 字面量（两处 mark 的形状）。
- 穿线回归：`run_turn` / runner 测试（`FakeProvider` 接收 `&ProviderConfig`）全部
  保持绿，证明 config 经 seam 到达 provider。
- 配置单测：`resolve` 返回 `ProviderConfig`，既有 cache 四层优先级（override > env >
  file > default true）测试不变。
- Prior art：llm-cache 的 wire 夹具测试（`parse_stream_captures_final_usage`、
  `message_to_wire_system_with_cache_emits_block_array`）、config `resolve` 表驱动
  测试、runner 的 `FakeProvider` 脚本测试。
- 不新增 live 冒烟测试（需真实 key + 网络，沿用既有 `#[ignore]` 模式）。

## Out of Scope

- 工具定义的 cache mark（pi 打了三处；Bailian 不需要——工具随 system 前缀缓存）。
- 对 user/assistant/tool 消息的多段精细 mark 放置策略演进（留给 provider 内部，
  未来 feature 单独落地）。
- 工具 canonical 顺序 / sort：今天无非确定性可修，动态工具（如 MCP）出现时再议归属。
- 任何费用/折扣计算或账单口径；本改动只影响请求侧 mark，不改变 usage 解析。

## Further Notes

- 术语同步：CONTEXT.md 新增 `ProviderConfig` / provider-owned concern / cache mark，
  更新 `Config` 词条的 `BailianConfig` 引用。
- ADR-0016：provider 拥有 LLM 专属 wire 关注点（边界规则、无 sort、字节确定性契约、
  mark placement 归 provider、RunConfig 纯运行时）。
- 文档同步：docs/development.md（ai wire 模型与 config 变更）、docs/explanation.md
  （缓存机制：system + 最后一条消息）、docs/configuration.md（`[ai] cache` 引用更新）。
- 验收：`cargo test` 全绿；`cargo fmt --all`；`cargo clippy --all-targets
  --all-features --message-format=json -- -D warnings` 0 error / 0 warning。
