# GUI assistant 正文的 markdown 渲染（标题保留 `#` 标记）

Status: ready-for-agent

来源：本次 grill 会话（6 + 4 问全部确认）+ `GLOSSARY.md` 新增词条 **Heading marker**。范围是
`gui/ui` 渲染层，外加宿主侧一次性的 opener 插件接入（capability + 插件注册）。测试 seam 已与用户
确认：**`<Markdown>` 组件的 DOM 断言**（jsdom + Testing Library），既有 reducer 单测不动。

## Problem Statement

GUI 的 assistant 正文现在是一个 `<pre>`，把模型输出的 markdown 原样吐出来：`**粗体**` 显示成带
星号的字面量，markdown 表格是一堆竖线对齐的噪音，``` 围栏只是一行三个反引号，标题 `###` 和普通
段落长得一样。模型写的回答本身是结构化文档（小节、表格、代码、清单），GUI 却只能当纯文本读。

同时存在一个「层级怎么表达」的坑：TUI 侧（`crates/tui/src/markdown.rs`）用 `pulldown-cmark` 做了
真解析，但**把标题的 `#` 去掉了**，层级只落在颜色和一个 `MdHeading` 样式上。对用户来说，字号/颜色
差异不足以确认"这是二级还是三级标题"。GUI 如果要跟进，必须换一种表达方式，而不是照抄 TUI。

## Solution

GUI 的 assistant 正文改为真 markdown 渲染：CommonMark + GFM，覆盖粗体/斜体、标题、列表、引用、
行内代码、代码块、表格、链接。核心约定：

- **标题保留字面标记**：`## 标题` 渲染后，文本前面仍能看到 `##`（弱化为 dim 色、等宽字体），
  层级的第一信号是 `#` 的个数，字号只做两档装饰（`#`/`##` 略大，`###` 及以下与正文同号）。
- **散文换 sans 字体**，行内代码/代码块保留 mono：markdown 排版的可读性收益主要来自这个反差。
- **代码块有结构**：浅灰底、圆角、横向滚动、围栏带语言时右上角显示语言标签（不做语法高亮、
  不做复制按钮）。
- **表格可读**：GFM 表格带边框，超宽时在块内横向滚动，绝不撑破转录区。
- **链接能点**：点击用系统默认浏览器打开 `http(s)` 链接。
- **流式期间**整段重解析（半截 markdown 由解析器兜底），不再拼 `▌` 光标——流式状态由转录底部
  已有的 `Working…` 脉冲行表达。

user 块（输入回显，粗体 mono + 靛蓝底）、reasoning（dim 折叠纯文本）、工具卡、用量行、压缩提示、
错误块**一律不变**。

## User Stories

1. 作为 GUI 用户，我想 assistant 的 `**粗体**` 显示成真正的粗体，以便不用透过星号去读重点。
2. 作为 GUI 用户，我想 `*斜体*` 显示成斜体，以便强调和普通文本可区分。
3. 作为 GUI 用户，我想 markdown 标题显示成更大的字重/字号，以便长回答里能一眼看到小节结构。
4. 作为 GUI 用户，我想标题前面保留 `##` 这样的字面标记，以便不靠字号就能确认这是二级还是三级标题。
5. 作为 GUI 用户，我想标题标记弱化成灰色等宽前缀，以便它像"编号"一样存在但不抢正文的注意力。
6. 作为 GUI 用户，我想 `###` 及更深的标题与正文同字号、只加粗，以便层级完全由 `#` 个数决定。
7. 作为 GUI 用户，我想 markdown 表格渲染成带边框的表格，以便并排对比的数据能被真正读出来。
8. 作为 GUI 用户，我想宽表格在块内横向滚动，以便它不会把整个转录区撑出横向滚动条。
9. 作为 GUI 用户，我想 ``` 围栏代码块显示成独立的灰底块，以便代码和散文清楚分开。
10. 作为 GUI 用户，我想围栏带语言（如 ```rust）时代码块右上角显示语言标签，以便知道这段是什么语言。
11. 作为 GUI 用户，我想围栏不带语言时不显示空的标签，以便不出现无意义的装饰。
12. 作为 GUI 用户，我想超长代码行在代码块内横向滚动而不换行，以便代码缩进不被破坏。
13. 作为 GUI 用户，我想行内 `` `code` `` 显示成等宽 + 浅底的小片段，以便和正文区分。
14. 作为 GUI 用户，我想 markdown 列表渲染成带项目符号的清单，以便步骤和枚举可读。
15. 作为 GUI 用户，我想 `> 引用` 渲染成带左侧色条的引用块，以便引文和正文分开。
16. 作为 GUI 用户，我想 `[文本](https://…)` 显示成链接样式，以便识别这是可打开的外部地址。
17. 作为 GUI 用户，我想点击链接用系统默认浏览器打开，以便不在 agent 应用里迷路。
18. 作为 GUI 用户，我想非 http(s) 的链接目标（相对路径、锚点）不触发跳转，以便点错也不会把应用导航走。
19. 作为 GUI 用户，我想 assistant 正文用正常 UI 字体（sans）、代码用等宽字体，以便长文比全等宽更好读。
20. 作为 GUI 用户，我想流式输出期间就能看到已到达部分的排版效果，以便边生成边读。
21. 作为 GUI 用户，我想流式末尾不再出现 `▌` 光标，以便光标不会污染最后一行 markdown 的解析。
22. 作为 GUI 用户，我想流式结束后正文与结束前的一瞬间不跳变，以便阅读位置不被重置。
23. 作为 GUI 用户，我想我自己输入的 prompt 块保持现在的粗体高亮样式，以便"我说过的话"依旧是最好认的。
24. 作为 GUI 用户，我想 reasoning（思考）块保持原样，以便这次改动只影响答案正文。
25. 作为 GUI 用户，我想工具卡、用量行、压缩提示、错误块保持原样，以便工具输出不被 markdown 解析搞乱。
26. 作为 GUI 用户，我想恢复历史会话时正文的渲染与直播完全一致，以便回放不需要另一套心智。
27. 作为 GUI 用户，我想模型偶尔写出的行内 HTML 被丢弃而不是当代码显示，以便正文里不出现原始标签。
28. 作为维护者，我想 markdown 只在渲染层解析、Block 模型不新增字段，以便 wire 事件与 reducer 契约零改动。
29. 作为维护者，我想标题标记与语言标签这些规则有 DOM 级自动测试，以便"`#` 被吞掉"这类回归不用靠看截图发现。
30. 作为维护者，我想 GUI 引入的这一次性依赖（解析器、测试环境、opener 插件）被记录在 ADR 里，以便后人知道为什么这里和 TUI 不一样。

## Implementation Decisions

**范围**：`gui/ui`（渲染层 + 依赖 + 测试环境），加宿主侧一次性接入 `@tauri-apps/plugin-opener`。
SDK / app / wire 事件 / `transcript.ts` 的 Block 模型**零改动**——assistant 块仍持有原始文本，
markdown 在渲染时解析。这条决定了直播与 resume 回放天然同构（同一份文本走同一渲染函数）。

**解析器**：`react-markdown` + `remark-gfm`。理由：需要的是 CommonMark/GFM 的事实语义（表格、
嵌套列表、内联强调的成对判定），手写解析器会在这些边界上长期掉坑；`react-markdown` 默认不把裸
HTML 当 HTML 渲染（`skipHtml`），不带 `dangerouslySetInnerHTML`，XSS 面天然更小；组件映射正是
「标题保留 `#`」要落的位置，无需二次处理字符串。

**新增组件**：`Markdown`（`gui/ui/src/components/`）。`Transcript.tsx` 的 assistant 分支把
`<pre>` 换成它，并删除行尾 `▌`（流式状态由转录底部的 `Working…` 行表达）。`user` / `reasoning`
/ `tool` / `usage` / `compacted` / `error` 分支不动。

**标题（Heading marker）**：每个标题渲染为真实的 `h1`–`h6` 语义元素，内容前插入 `#`×N 的字面前缀
（复用同一 `#` 的取值），前缀样式为 dim 灰 + 等宽、与标题字重无关；标题本身 `font-semibold`。
字号只做两档：`h1`/`h2` 略大于正文，`h3`–`h6` 与正文同号（层级由 `#` 个数承担，字号只是装饰）。
词条 **Heading marker** 进 `GLOSSARY.md`。

**字体**：正文用 `--font-sans`；行内代码与代码块用 `--font-mono`。user 块继续 mono + bold +
靛蓝底（既有规范不变）。

**代码块**：浅灰底（`slate-100`）+ 圆角 + `overflow-x-auto`；围栏的 info string 作为右上角语言
标签，无 info string 则不渲染标签。不做语法高亮、不做复制按钮。

**表格**：GFM 表格渲染为带边框表格，外层容器 `overflow-x-auto`，保证宽表格不撑破转录区。

**链接**：`http(s)` 链接点击走 `@tauri-apps/plugin-opener` 在系统浏览器打开；其它 target 渲染成
链接样式但不触发跳转。前端把「打开外部链接」收敛到一个薄模块，测试通过 mock 该模块断言"点击即
调用、且不导航"，避免测试触碰 Tauri 运行时。

**行内 HTML**：`skipHtml`——标签丢弃，内容按文本处理，不原样输出标签。

**图片**：不渲染。`img` 映射为斜体 alt 文本占位（无 alt 时显示 `[image]`）——图片渲染本就不在本次范围，
而 webview 的 CSP 是 `null`，直接载图等于允许模型输出的 markdown 发起任意远程请求（追踪像素）。

**流式**：每个文本 chunk 到达都整段重解析（与 TUI 的 per-frame re-parse 同构），不做增量解析
状态；解析器对未闭合强调/围栏自行兜底。

**边缘情形归属**：宽表格与超长代码块横向滚动、超长 URL / 无空格串折行、无 info string 不显示
标签、任务列表 `- [ ]` 文案化、脚注与数学公式不启用——由实现方按「渲染正确、绝不横向溢出、退化
优雅」统一处理，用户只在验收时看效果。其中两条在 mdast 层就完成降级（`remark-gfm` 默认会解析）：
任务项的 `checked` 改为 `☐`/`☑` 文字前缀（不留 `<input>` 表单控件），脚注引用改回字面 `[^id]` 文本、
定义丢弃（否则会凭空长出一个模型没写过的 “Footnotes” 标题，破坏 `#` 计数即层级的不变量），
两者都有 DOM 断言（ADR-0028 D5）。

**依赖与门禁侧的一次性改动**：
- `gui/ui` devDependencies 增 `jsdom` + `@testing-library/react` + `@testing-library/dom`；渲染
  测试用文件级 `@vitest-environment jsdom` docblock 开启，纯逻辑测试仍跑 node 环境（不改全局默认）。
- runtime 依赖增 `react-markdown` + `remark-gfm` + `@tauri-apps/plugin-opener`。
- 宿主侧：`gui/src-tauri/Cargo.toml` 增 `tauri-plugin-opener`，`lib.rs` 注册插件，
  `capabilities/default.json` 增 opener 权限条目。

**文档与记录**：新增 `docs/adr/0028-gui-markdown-rendering.md`（记录：解析器选型、标题保留 `#`
并**有意偏离 TUI/pi**、opener 插件引入、渲染层首次进单测），并在 `docs/index.md` 的 ADR 索引登记；
`docs/user-manual.md` 的「桌面 GUI」章节补一句正文渲染说明；`docs/development.md` 的 gui 一节更新
前端依赖与前端门禁描述（渲染层不再是"仅手动验收"）。

## Testing Decisions

- **什么算好测试**：只断言外部可观测行为——给定 markdown 源文本，渲染后的 DOM 里有什么文本、
  什么结构、点击触发了什么；不断言 Tailwind 类名、不逐个断言组件树、不测样式像素。
- **seam（唯一）**：`<Markdown>` 组件本身。它是这次改动的最高点——把 markdown 源文本喂进去，
  得到用户能看见的结果；Transcript 只是它的调用方，reducer 完全不参与。
- **要覆盖的断言**：
  - 标题：`## 小节` 渲染后可见文本同时包含 `##` 与 `小节`；`#` 前缀个数与层级一致（`#`/`###` 各一例）；
    语义元素层级正确。
  - 粗体/斜体：`**粗**` / `*斜*` 的文本可见（标记符号本身不出现）。
  - 表格：GFM 表格渲染出表格结构，单元格文本可查。
  - 代码块：围栏内容出现在代码元素内；带 info string 时有语言标签，不带时没有标签。
  - 行内代码：文本可见、不出现反引号。
  - 列表 / 引用：条目文本与引用文本可见。
  - 链接：`http(s)` 链接渲染为可识别链接；点击通过 mock 的外部打开模块被调用一次，且未发生页面导航；
    非 http(s) 不触发调用。
  - 行内 HTML：`<span>` 不进入 DOM 作为元素（文本按文本处理）。
- **prior art**：`gui/ui/src/transcript.test.ts` / `chip.test.ts` 的纯函数风格（Vitest 已在
  ADR-0027 引入）；DOM 断言是本仓库前端的第一处，形态对齐既有 Vitest 用法（`npm test` 一条命令）。

## Out of Scope

- 语法高亮、代码块复制按钮（本次只做结构与排版）。
- user 块、reasoning 块、工具卡、用量行、压缩提示、错误块的任何渲染改动。
- 修改 TUI 的标题渲染（TUI 当前丢 `#`，本次不动；ADR-0028 记录这处有意的不一致）。
- 增量/流式 markdown 解析器（每次整段重解析）。
- 暗色主题、markdown 的图片渲染、脚注、数学公式、任务列表的勾选框交互。
- wire 事件、`transcript.ts` Block 模型、SDK / app / 宿主 controller 的逻辑改动。
- 首页/输入框等其他区域的 markdown 化。

## Further Notes

- 关键事实：assistant 文本现在由 `Transcript.tsx` 的 `<pre className="whitespace-pre-wrap
  font-mono …">` 原样输出，光标 `▌` 拼在文本末尾——这正是 markdown 化后必须去掉的东西。
- TUI 的 `markdown.rs` 已证明"真解析器 + 自定义映射"这条路可行，但它选择丢弃 `#`；GUI 的有意
  分歧要写进 ADR，避免后人误以为是漏改。
- `gui/ui` 目前只有 `react` / `react-dom` / `@tauri-apps/api` 三个 runtime 依赖，这次是全仓库前端
  依赖面变化最大的一次，ADR 里要写清取舍。
- 验收手段：DOM 单测进 `npm test`；视觉效果用 devshot 截图通道人工确认（`docs/development.md`
  已记录 `/tmp/slimcode-gui-dev` 的取图约定）。
- 落地顺序建议：依赖与测试环境 → ADR/GLOSSARY → `<Markdown>` red → 实现 green → Transcript 接线
  → 宿主 opener 接入 → 文档同步。
