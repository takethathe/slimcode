# 02: slimcode-sdk crate：create_agent_session + 持久化单回合 prompt

**What to build:** 可嵌入的 SDK crate。一次调用 `create_agent_session(options)` 创建持久化会话，`prompt(text)` 跑完整 agent 回合（复用共享回合循环），事件经 `subscribe` 实时送达，会话落为现有 JSONL 日志，返回终止原因。这是整个 SDK 的 tracer bullet：从公共 API 一路通到事件流与落盘，可端到端验收。

**Blocked by:** None (can start immediately)

**Status:** resolved

- [x] 新 crate 加入 workspace，依赖方向符合 ADR-0011/0021（sdk → app → ai/core/commands，无终端依赖）；架构依赖断言测试补一行
- [x] `AgentSessionOptions` builder（cwd / persist / provider / tools / base_url / model / api_key / cache）+ `create_agent_session(options)`：服务组装对齐 CLI 启动路径（四层配置解析、slimcode 家目录、项目 key、会话存储），缺省回落默认配置与绑定 cwd 的七工具；`provider`/`tools` 可注入（测试 seam）
- [x] `prompt(text)` 同步跑一轮并返回 `StopReason`，复用共享回合循环（不新写第二套循环）；回合编排对齐 CLI：上下文组装、prompt 消息入历史并落盘、标题推断与标题记录、失败/取消回合以收尾消息关闭日志（不悬在工具批次上）
- [x] `subscribe(listener) -> Unsubscribe`：事件同步送达——`SdkEvent::Display`（流式文本/思考/工具起止/用量/Stop）、`SdkEvent::Message`（每条进历史的消息，assistant 带 usage、工具结果无）、`SdkEvent::TurnFinished`（一次 prompt 的最后一个事件）；`unsubscribe` 停止送达
- [x] 持久化模式：会话落为现有 JSONL 会话日志（项目分区、与 `/session` picker 兼容），日志可被 `SessionStore::load` 回放且与内存 messages 一致
- [x] 脚本化 provider 经公共 API 驱动：断言事件顺序、StopReason、落盘字节；provider 错误传播为 `Err`
- [x] 文档同步：`docs/development.md` crate 表格与职责描述补 SDK 一行
