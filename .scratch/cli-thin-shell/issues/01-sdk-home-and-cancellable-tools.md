# 01: SDK：with_home pin 住 home + 默认工具绑定会话 cancel

**What to build:** SDK 会话可显式 pin 住 slimcode home（会话存储、skills 与上下文文件发现、环境 global_home 都落在指定目录），生产不传时行为不变；默认七工具改为绑定会话自己的 cancel token，使 `abort()` / `dispose()` 能终止运行中的 bash 子进程（修掉 SDK 缺口，TUI 的 Esc-kill 免费获得）。CLI 未动。

**Blocked by:** None (can start immediately)

**Status:** ready-for-agent

- [ ] `AgentSessionOptions.with_home(PathBuf)`：pin 住 home；不传时维持 `config::slimcode_home()`，生产路径行为不变
- [ ] 默认工具改为 `build_tools_with_cancel(cwd, &session_token)`（工具持有会话自己的 token：prompt 开头 reset、abort/dispose 置位），`abort()` 可终止 bash 子进程；`with_tools` 注入不受影响
- [ ] SDK 测试：with_home 在 env 干扰下仍把会话落盘与发现落在 pin 的 home（另一干扰 home 零文件）；既有 SDK 测试全绿，无行为变化

## Answer

已实现：`AgentSessionOptions::with_home` 显式 pin 住 home（会话存储、skills / 上下文发现、global_home 都落在指定目录），不传时维持 `config::slimcode_home()`；默认七工具改为绑定会话自己的 cancel token，`abort()` / `dispose()` 可终止运行中的 bash 子进程（`with_tools` 注入路径不受影响）。测试覆盖 env 干扰下的 home pin 与可取消默认工具。

**Status:** resolved
