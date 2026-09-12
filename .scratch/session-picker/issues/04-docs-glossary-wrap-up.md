# 04: 文档、术语与 ADR 索引收尾

**What to build:** 让文档与新的两个 session 命令一致：`CONTEXT.md` 里不再出现 `/load` 与
`/sessions`，并补上两个词条——**Current session**（前端内存里的那个会话，可能还没有日志文件，
因此可能不在 picker 的列表里）与 **Session picker**（列会话、标 `*`/`›`、回车载入的那个全屏
视图）；README 与用户手册的命令表只剩 `/session` 与 `/new`，并新增 picker 小节（键位、`*` 与
`›` 的含义、`(i/n)`、空态、以及"恢复的会话从 0 重新计 usage"这条反直觉规则）；开发文档与说明
文档同步新的命令面与视图；ADR 目录行已经指向 ADR-0018。

**Blocked by:** 01, 02, 03

**Status:** resolved

- [x] `CONTEXT.md` 不含 `/load`、`/sessions`，并定义 Current session 与 Session picker。
- [x] 会话相关命令表恰好是 `/session` 与 `/new`（README + 用户手册）。
- [x] 用户手册记录了 picker 的键位、`*`/`›` 的含义、溢出指示、空态。
- [x] 用户手册显式写明"恢复的会话 usage 从 0 起算、不持久化"。
- [x] `docs/development.md` 与 `docs/explanation.md` 描述的新命令面与 TUI 视图与代码一致。
- [x] 命令描述与 `usage` 字符串与实现一致（`/help` 输出不留 `/load` 痕迹）。
