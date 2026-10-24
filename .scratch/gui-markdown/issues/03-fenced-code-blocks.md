# 03: 围栏代码块：灰底块 + 语言标签 + 横向滚动

**What to build:** ``` 围栏渲染成独立的灰底圆角代码块，代码进等宽元素并在块内横向滚动（超长行不
换行、不撑破转录区）；围栏带 info string（如 ```rust）时右上角显示语言标签，没带就不显示空标签。
正文仍是 UI 字体，只有行内代码与代码块用等宽——markdown 的可读性收益全靠这层反差。

**Blocked by:** 01（markdown 渲染骨架）。

**Status:** ready-for-agent

- [x] 围栏代码块渲染为独立灰底块，内容位于 `<pre><code>` 内，代码行不在块内折行
- [x] 超长代码行在代码块内横向滚动，转录区不出现横向溢出
- [x] info string 存在时右上角显示语言标签；缺失时不渲染任何标签
- [x] 行内代码与代码块用等宽字体，正文仍用 UI 字体
- [x] 测试覆盖"带语言标签 / 不带语言标签"两例

## Comments
- 已实现（f418259）：`pre` 映射到 `CodeBlock`（slate-100 灰底圆角面板 + `overflow-x-auto`），语言从 code 子元素
  的 `language-*` class 读出，右上角显示等宽小号标签（无 info string 则不渲染），`<pre>` 留 `data-language`
  供断言。待人工验收：超长行横向滚动与滚动条手感；转录区不横向溢出这一条只有 devshot 人工确认。
