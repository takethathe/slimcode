# 01: `/session` 交互闭环——可选择的会话列表

**What to build:** 敲 `/session` 打开一个占满整屏的 picker，列出当前项目的会话（刚 `/new`
出来、还没落盘的会话不在其中）：当前会话标 `*`、光标行标 `›`，用 `↑`/`↓`/`PgUp`/`PgDn`
或滚轮移动，`Enter` 载入所选会话、`Esc` 原样返回聊天视图。`/load`、`/resume`、`/sessions`
从命令表中消失，于是恢复会话只剩"打开 picker、选一行、回车"这一条路。载入与 `/new` 都会
把 transcript 清空并重新出现启动 Header；载入时"跳过 N 条记录 / 修补 N 个工具调用"的提示
终于能留在屏幕上（今天它们被紧随其后的清屏动作吃掉）。选中当前会话那一行按 `Enter` 什么
都不发生。

行布局、`(i/n)` 溢出指示、空态与键位细节见 spec 的「渲染规格（picker）」——本工单只要求
行里放 id（标题与统计留给 02）。

**Blocked by:** None (can start immediately)

**Status:** resolved

- [x] `/session` 打开 picker；`/load`、`/resume`、`/sessions` 不再解析（`find()` 返回 `None`）。
- [x] picker 列出当前项目会话；列表为空时显示空态而不是报错。
- [x] `↑`/`↓` 每次一行、`PgUp`/`PgDn` 每次一页，两端都 clamp；选中行始终可见，溢出时显示 `(i/n)`。
- [x] 滚轮移动选中行，且不改变 transcript 的滚动位置、following 状态与滚动条淡出。
- [x] `Enter` 载入所选会话并关闭 picker；`Esc` 关闭 picker 且不产生任何 Effect。
- [x] picker 打开时的按键不会进入输入框、不触发输入历史 recall；Ctrl+C / Ctrl+D 仍然退出程序。
- [x] 选中当前会话按 `Enter`：不清屏、不重载、不重置任何状态。
- [x] 载入与 `/new` 都清空 transcript、重新出现启动 Header、更新 session id 与终端标题。
- [x] 载入一个需要修补的会话后，"skipped / repaired" 提示在清屏之后仍然可见。
- [x] turn 运行中的按键语义不变（picker 无法在运行中途打开或操作）。
- [x] 新视图与新的 item/effect 仍不依赖 app/ai/core 类型（ADR-0013 / ADR-0014 的 seam 不被打破）。
