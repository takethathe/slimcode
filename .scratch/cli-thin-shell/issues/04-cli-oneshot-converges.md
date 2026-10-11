# 04: CLI：one-shot 收敛为消费 SDK

**What to build:** one-shot（`run_once`）的回合编排收敛为消费 `AgentSession`（`persist: false`），行为不变（不落盘、技能触发归一化保留、流式输出与用量打印一致）；`TextRenderer` 改为持有 `Box<dyn Write>` 供 `Send + Sync` 监听器捕获；`cli` 新增 `slimcode-sdk` 依赖并同步架构依赖断言。TUI 未动。

**Blocked by:** None (can start immediately)

**Status:** ready-for-agent

- [ ] `run_once` 改为创建 `AgentSession`（`persist: false` + 冻结配置 overrides + skills 快照）→ `subscribe` 转发 `Display` 事件到 `TextRenderer` → `prompt`；前导 `/skill:name` / `/{name}` 技能触发归一化留在 CLI
- [ ] 行为不变：one-shot 从不落盘（ADR-0009 D5）、流式文本/思考/工具块/用量渲染与现状一致、错误仍 exit 1
- [ ] `TextRenderer` 持有 `Box<dyn Write>`（渲染状态跨事件持续）；调用点与测试适配
- [ ] `cli` 依赖加 `slimcode-sdk`；architecture.rs 的 cli 依赖断言含 sdk
- [ ] one-shot 相关测试（main.rs / render.rs）绿；真实二进制冒烟不受影响

## Answer

已实现：one-shot（`-p`）收敛到不持久化的 SDK 会话（`with_persist(false)`），单一 `run_once` 装配路径 + `run_once_with` 测试缝驱动真实接线；smoke 断言不留 session 日志。review 后补：one-shot 也把解析出的 home 传给 `with_home`（`2c877a5`），回合末自动压缩的陈旧 usage 修复见 `669339c`。

**Status:** resolved
