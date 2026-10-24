# 01: Card view 面板与静态渲染

**What to build:** GUI 前端右侧出现一个 Card view：固定宽度、可折叠、独立滚动的面板。它用 json-render 渲染一张 Card，组件全部来自我们手写的 catalog，视觉沿用 GUI 现有的浅色 slate/indigo 设计语言。此时 Card 的内容由组件入参直接给出（不来自 Tauri 宿主），目标是先把"渲染这条路"打通并定下视觉。

**Blocked by:** None (can start immediately)

**Status:** ready-for-agent

- [ ] json-render 的渲染器与核心包按其稳定版本引入（含其 zod peer 依赖），锁版本，前端构建与既有测试仍然全绿
- [ ] catalog 提供九个组件（容器、堆叠、文本、列表、键值、徽标、进度、分隔线、Markdown）并附各自的 props 描述，另有一个安静的未知类型占位实现
- [ ] Markdown 组件复用 GUI 既有的正文渲染规则（标题保留层级标记、链接经系统浏览器外开），不另起一套
- [ ] 面板位于 Transcript 右侧，与转录各自独立滚动；可折叠，折叠状态记在本地，不写 GUI 设置文件
- [ ] 空态有明确文案；未知组件类型或组件内部抛错时该块降级为占位，不影响同一张卡的其他块
- [ ] 组件与面板的 DOM 测试覆盖：正常渲染、未知类型占位、空态、折叠；前端 test 与 build 门禁全绿
