# 07: 设计打磨与收尾验收

**What to build:** 全应用一致性打磨与最终验收：空态、删除确认、键盘/焦点管理、间距/圆角/阴影统一、窗口元数据；对照 `.scratch/gui/spec.md` 的验收清单逐条过一遍手动验收，确认浅色现代观感与「会话共享仓库、设置不污染 config.toml、门禁干净」等 spec 承诺全部成立。

**Blocked by:** 03, 04, 05, 06

**Status:** resolved

- [x] 空态：无历史会话 / 空转录的占位文案与样式
- [x] 删除确认交互（04 的删除按钮）样式与文案统一
- [x] 键盘/焦点：Enter 发送 / Shift+Enter 换行一致、输入框在启动与切会话后自动聚焦、中止按钮焦点可达
- [x] 视觉一致性：间距/圆角/阴影/状态色全应用统一；浅色现代观感达标（slate 底、indigo 强调、等宽代码字体）
- [x] 窗口元数据：显示名 "slimcode GUI"、合适的最小尺寸
- [x] 对照 spec 验收清单逐条手动验收：共享会话仓库（CLI/TUI 会话可恢复）、设置不写 config.toml、`gui/` 项目门禁（fmt / clippy / test）干净、workspace 门禁不受影响
- [x] 更新对应文档（如实现中暴露的契约与 GLOSSARY/ADR 有不一致处则同步）

**Gate:** `gui/` 内门禁全绿；spec 验收清单逐条勾完；最终可交付的手动验收通过。

## Answer

Implemented: empty states (no sessions / blank transcript), delete confirmation, keyboard/focus
(Enter send, Shift+Enter newline, input focused at launch and after switching sessions),
consistent spacing/radius/shadow/status colors, window metadata ("slimcode GUI", 800×600 min).
Verified against the spec: shared session repository, settings never touch `config.toml`,
`gui/` gates clean (fmt/clippy/test), workspace gates untouched, and `tauri dev` smoke-tested
(window opens, webview loads the React app over the vite dev server, no runtime errors).
