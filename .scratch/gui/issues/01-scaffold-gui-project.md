# 01: GUI 项目脚手架：Tauri 宿主 + React/Vite/Tailwind 骨架

**What to build:** 顶层 `gui/` 项目立起（独立于 cargo workspace），在 macOS 上启动一个 Tauri 窗口，webview 渲染 React 首页，浅色现代的 Tailwind 主题 tokens 就位（slate 灰阶 + indigo 强调色 + 等宽代码字体），一条 ping 命令验证前端 ↔ Tauri 宿主 IPC 通。应用能打开、能看到占位页、前端能调通后端——后续所有切片的地基。

**Blocked by:** None (can start immediately)

**Status:** resolved

- [x] `gui/` 目录建立：Tauri 宿主工程（自己的 Cargo.toml / Cargo.lock，path 依赖 `slimcode-sdk` 与 `slimcode-app`）与 React + Vite + Tailwind 前端（自己的 package.json）；不加入 cargo workspace，workspace 的 `cargo test` / clippy / 依赖矩阵不受影响
- [x] macOS 上 `tauri dev` 能启动：原生窗口打开，webview 渲染 React 首页
- [x] Tailwind 浅色主题 tokens 定义：slate 灰阶底、indigo 强调色、语义状态色（成功绿 / 失败红 / 运行中强调色）、6–8px 圆角、柔和阴影、系统 UI 字体 + 等宽字体
- [x] 一条 ping 形状的命令：前端 `invoke` 后端返回确认，证明 IPC 通路工作
- [x] 标识符与显示名：`slimcode-gui` / "slimcode GUI"（窗口元数据）
- [x] `gui/` 项目内自己的门禁可跑：`cargo fmt` / clippy（0 warning）/ `cargo test` 全绿

**Gate:** `gui/` 内 `cargo test` 全绿、fmt/clippy 干净；`tauri dev` 启动窗口显示首页。

## Answer

Implemented (commit `feat(gui)`). `gui/` sits outside the cargo workspace with its own
Cargo.toml + package.json; the Tauri host path-depends on `slimcode-sdk` + `slimcode-app`.
`tauri dev` opens a 1200×800 "slimcode GUI" window (id `com.slimcode.gui`) loading the
React/Vite/Tailwind home page; the IPC proof is the `get_session` snapshot loaded on mount.
`cargo test` (34) green, fmt/clippy 0 warnings in `gui/src-tauri`; `tsc --noEmit` + `vite build`
green in `gui/ui`.
