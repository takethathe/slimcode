# 05: 链接可点：`http(s)` 走系统浏览器

**What to build:** assistant 正文里的 `[文本](https://…)` 渲染成链接样式，点击用系统默认浏览器
打开，而不是在 agent 窗口里迷路；非 `http(s)` 的目标（相对路径、锚点）保持链接样式但不触发跳转。
这是 GUI 第一次需要 opener 能力，宿主侧要一并接入并只授予最小权限。

**Blocked by:** 01（markdown 渲染骨架）。

**Status:** ready-for-agent

- [x] `http(s)` 链接渲染为链接样式，点击交给系统默认浏览器打开
- [x] 非 `http(s)` 目标不触发跳转、也不调用外部打开逻辑
- [x] 宿主注册 opener 插件，capability 只授予打开链接所需的最小权限
- [x] 前端把"打开外部链接"收敛在一个薄模块里，测试 mock 它：点击调用一次且不发生页面导航；
      非 `http(s)` 不调用
- [x] 测试环境不依赖 Tauri 运行时（jsdom 下可跑）

## Comments
- 已实现（f418259）：`external.ts` 收敛 `isExternalUrl` / `openExternal`（内部 try/catch），测试 `vi.mock` 它；
  宿主注册 `tauri_plugin_opener`，capability 授 `opener:allow-open-url` 且 scope 限 `https://*` / `http://*`
  （不用 `opener:default`，它含 reveal-item-in-dir）。断言：`http(s)` 点击调一次且 `fireEvent.click` 返回
  false（已 preventDefault），非 `http(s)` 不调用。待人工验收：点链接真的唤起系统浏览器。
