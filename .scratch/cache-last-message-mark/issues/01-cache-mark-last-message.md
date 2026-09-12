# 01: 最后一条消息 cache mark

**What to build:** 多轮会话的完整历史前缀也能命中上下文缓存。除 system 消息外，
请求里最后一条「非空文本的 user/assistant/tool」对话消息也被打上缓存 mark
（与 system 同形：content 序列化为单块数组 + `cache_control`）；system 保持现有
mark；关闭缓存时两处 mark 都不出现，请求字节与升级前逐字节一致。wire 层以字节
确定性测试锁定前缀稳定。这是本 effort 的垂直 tracer bullet：在 wire seam 上
完整落地、可独立验证（纯函数测试，无网络）。

**Blocked by:** None (can start immediately)

**Status:** resolved

- [x] system 消息的 cache mark 保持现状（content 为单块数组 + `cache_control`）。
- [x] 自尾部扫描，最后一条「非空文本的 user/assistant/tool」消息的 content 被打上
      同形 mark。
- [x] 尾部空消息（assistant 工具调用、空 tool 结果）跳过并向前找第一条可 mark
      消息；空文本本身不打 mark。
- [x] 工具定义不加 mark（既有决策：`cache_control` 只加在 content）。
- [x] cache 关闭时两处 mark 都不出现，请求字节与升级前逐字节一致。
- [x] 字节确定性契约：相同 messages + tools + cache 两次构造请求，序列化字节
      逐字节相同；并锁定完整请求 JSON 形状（两处 mark）。
- [x] 既有 wire 语义回归（assistant 工具调用空 content、tool 消息必带 content、
      空文本省略）全部保持。
- [x] 全量测试通过；`cargo fmt --all` 与 clippy（`-D warnings`）干净。
