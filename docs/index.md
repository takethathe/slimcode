# slimcode 文档索引

| 文档 | 说明 |
| --- | --- |
| [development.md](./development.md) | 开发文档：架构、设计决策、构建/测试方法 |
| [explanation.md](./explanation.md) | 说明文档：背景、概念、设计动机与原理 |
| [user-manual.md](./user-manual.md) | 用户手册：安装、配置与使用方式 |
| [configuration.md](./configuration.md) | 配置文档：四层配置来源、config.toml、环境变量与命令行参数使用说明 |
| [ADR 目录](./adr/) | 架构决策记录（无依赖 REPL 输入历史 ADR-0001、TUI 前端 ADR-0003、前端无关渲染 seam ADR-0004、`/` 补全弹框 ADR-0005、TUI 显示对齐 pi ADR-0006、TUI 第二轮对齐 pi（无边框编辑器/嵌入状态/弹框上移）ADR-0007、会话按项目分区 + 配额清理 ADR-0008、会话落盘改为追加式 JSONL 日志 ADR-0009、并行工具调用 ADR-0010、crate 分层与 cli 总入口 ADR-0011、两层消息模型 ADR-0012、TUI 运行 seam ADR-0013、TUI 零依赖与 cli 侧适配器 ADR-0014、运行时钩子可改写进 history 的消息 ADR-0015、provider 自有关注点与配置 seam ADR-0016、鼠标滚轮滚动 transcript ADR-0017、session 命令收敛为 picker + 重置 ADR-0018、provider delta 即时流入前端 ADR-0019、压缩为会话内 CompactSummary 摘要 + usage 事件流与 footer context ADR-0020、程序化 SDK crate（slimcode-sdk）+ 后续 FFI 模块与 CLI 薄壳化 ADR-0021、CLI 薄壳化（依赖反转 cli → sdk、SDK 面扩展 with_home / with_session_id / compact_now / 可取消默认工具、行为权衡）ADR-0022、FFI 模块（纯 C ABI + cdylib/rlib、per-handle 事件队列而非回调、扁平 JSON wire 契约、persist 默认 true）[ADR-0023](./adr/0023-ffi-c-abi-queue-and-json.md)、Tauri Web GUI（第三个前端：Rust 后端内嵌 SDK + React/Vite/Tailwind 浅色现代 UI、wire 事件桥、gui.toml、workspace 外置）[ADR-0024](./adr/0024-tauri-web-gui-over-the-sdk.md)、压缩持久化保留边界使 Kept tail 跨 resume 存活（`kept_count` + load 重建 + 先裁剪后修复，修订 ADR-0020 D2）[ADR-0025](./adr/0025-compaction-persists-a-keep-boundary.md)、skill 注入上提为 app 层公共 prompt preprocessing（`prepare_user_prompt`，GUI/TUI/一次性共用；`Command.gui_applicable`；GUI `/` 补全弹框；部分取代 ADR-0013 D3）[ADR-0026](./adr/0026-shared-skill-injection-preprocessing.md) 等） |
| [GUI 规格](./.scratch/gui/spec.md) | GUI 前端实现规格：架构（Tauri 宿主 / 会话注册表 / wire 桥 / gui.toml）、Tauri command 面、GUI wire 事件表、验收标准 |
