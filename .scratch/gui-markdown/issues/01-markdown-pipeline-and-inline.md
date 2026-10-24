# 01: assistant 正文走 markdown 渲染骨架（段落 / 粗体 / 斜体 / 行内代码 / 列表 / 引用）

**What to build:** GUI 用户看到的 assistant 回答不再是把 markdown 原样吐出的等宽文本：`**粗体**`、
`*斜体*`、行内 `` `code` ``、有序/无序列表、`> 引用` 都渲染成对应排版，源标记本身不再出现在可见
文本里。同时流式末尾那个 `▌` 光标消失——流式状态由转录底部已有的 `Working…` 行表达。这一票同时
把前端渲染测试的地基立起来：`<Markdown>` 组件可用 DOM 断言直接测。

**Blocked by:** None (can start immediately).

**Status:** ready-for-agent

- [x] assistant 正文由 `<Markdown>` 渲染，段落 / 粗体 / 斜体 / 行内代码 / 有序与无序列表 / 引用可见，
      源标记不出现在可见文本
- [x] 裸 HTML 标签被丢弃（`skipHtml`），不原样输出、也不作为元素进入 DOM
- [x] user 块（粗体 mono + 靛蓝底）、reasoning、工具卡、用量行、压缩提示、错误块渲染保持不变
- [x] 流式期间已到达部分即时排版，且不再拼 `▌` 光标
- [x] `npm test` 能跑 jsdom 下的 DOM 断言（文件级 `@vitest-environment jsdom`， 纯 reducer 测试仍走 node），
      `npx tsc --noEmit` 与 `npm run build` 通过
- [x] 新增的 runtime / dev 依赖（解析器 + GFM + 测试环境）只加在 `gui/ui`；宿主 侧唯一的一次性改动（opener 插件注册 + capability）归 ticket 05

## Comments
- 已实现（f418259）：`Transcript.tsx` assistant 分支改走 `<Markdown>`，删掉 `▌`；`skipHtml` 丢弃裸 HTML。
  与原文的出入只有一条：宿主侧在 ticket 05 一并汇入的改动之外，`img` 也改成 alt 文本占位（CSP 为 null 时不载远程图）。
  待人工验收：转录区实际观感（间距、流式重解析的跟手程度）。
