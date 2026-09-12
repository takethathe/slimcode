# slimcode 说明文档

> 本文件记录项目背景、核心概念、设计动机与原理。随代码变更同步维护。

## 项目背景

slimcode 是一个 Rust 编码 agent CLI：一条 prompt 在目标目录内自主执行
read / write / edit / bash / grep / find / ls 工具循环直到完成，流式输出、
token 用量统计、会话可保存/恢复（详见 `docs/development.md`）。有两个前端：
带 prompt 时跑非交互 one-shot；不带 prompt 时进入全屏 TUI（ADR-0003），两者共用
同一套渲染模型与 turn runner（ADR-0004）。

## 核心概念

### Context（构建后的消息列表）

一次 agent 运行需要一组按序排列的 `Message`（system → user → assistant →
tool → …）。这组消息在交给运行时之前，由前端无关的 `ContextBuilder`（
`slimcode-app::context`）组装：基础系统提示（默认 `DEFAULT_SYSTEM_PROMPT`
或 `with_system` 覆盖）+ 可自动调用 skill 的广告段落 + 可选 message history
+ 当轮 user prompt 或 skill 触发。`build()` 返回的 `Vec<Message>` 可直接交给
`AgentRunner::run`，无需二次转换。前端启动时还会注入系统环境信息
（`with_environment`）：OS 名、global home（slimcode home）、project home（git
仓库根，无 git 时回退到 OS 用户主目录），渲染为 `## Environment` 章节放在基础
提示之后、上下文文件之前，让模型无需探测文件系统就知道平台与全局/项目根目录。

关键区分（见 `CONTEXT.md` 词条）：**Context** 是一次构建的产物；**message
history**（`session.messages`）是会话中已发生的消息，可被当作 Context 的
`with_history` 输入继续构建；**input history**（前端输入历史）与上下文组装
无关。

组装规则（所有前端入口共享，行为可预测、不漂移）：

- 新会话（空 history）首轮前置一条 system 消息；恢复的会话 history 已含
  system，不重复插入。
- 只有 `disable-model-invocation: false` 的 skill 进入 system 的
  `## Skills` markdown 索引（每 skill 一行 `- name: description [Read from
  <file>]`，`file` 是其 `SKILL.md` 路径），并说明
  模型可按名字/描述匹配即用该 skill，或按用户显式 `/{name}` 引用触发；
  `true` 的 skill 仅通过显式 `/skill:name` 触发。
- `/skill:name` 触发时，skill 正文以 pi 风格的 `<skill name location>` XML 块
  注入当轮 user 消息，并附 `References are relative to <skill 目录>.` 一行，模型
  无需再 read `SKILL.md`，直接按正文里的相对路径引用辅助文件。
- 去重：若同一 skill 已在更早 message 加载过（扫描 `<skill name="..."` 标记），
  重触发时正文替换为 “already loaded” 提示、保留外壳与 base-dir 行，模型去更早
  的 message 找指令，避免重复加载；扫描无状态，会话 `/load` 恢复后依然有效。
- 当轮 user 消息必须存在（`with_user_prompt`；skill 触发的消息文本由 CLI 用
  `skills::skill_prompt` 渲染后再传入），否则 `build()` 报错，而不是静默产出没有
  user 消息的 turn。

### AGENTS.md 上下文文件注入（pi 对齐）

slimcode 会把 `AGENTS.md` 上下文文件注入到 system 消息里（对齐 pi 的 project
context 加载，见 `slimcode-app::context_files`）。发现规则：

- **全局**：`<home>/AGENTS.md`（`$SLIMCODE_HOME` 或 `~/.slimcode`，跨项目生效），
  scope 标为 `global`。
- **项目**：只判定两个位置——cwd 自身，以及 **git 仓库根**（最近的含 `.git` 条目
  的祖先目录；`.git` 可以是目录，也可以是 `gitdir:` 文件标记，兼容
  worktree/submodule），scope 标为 `project`；git 根在前、cwd 在后的顺序注入；
  同一路径只注入一次（cwd 嵌套在 home 下时，全局文件不会被重复当作项目文件）。

渲染时整个章节用 markdown 标题 `## Project context`（与 `## Skills` / `## Tools`
同层），每个文件的内容被一个 `<project_instructions path="…" scope="…">` XML 块
包裹（`scope="global|project"` 标注意图；XML 块把文件内容隔离成原子单元，
防止 AGENTS.md 内部的 `#` 标题/列表与外层 markdown 结构互相干扰），段首声明
**项目要求可覆盖全局要求**（仅声明，不做程序级合并；两个文件都是自由文本）。
上下文文件章节位于基础 system 之后、`## Skills` 索引之前；没有任何可注入的
`AGENTS.md` 时整个章节省略，system 与现状完全一致。

### DisplayItem / Renderer / run_turn（前端无关的渲染与运行 seam）

渲染与 turn 执行收敛为 `slimcode-app` 的前端无关 seam（ADR-0004）：

- **DisplayItem**：前端无关的显示单元（turn 标记、流式文本片段、思考行、工具
  开始/结果、停止标记、token 用量），由共享纯映射 `map_event(AgentEvent) ->
  Option<DisplayItem>` 从 agent 事件产出；
- **Renderer**：每前端只实现一个 `Renderer` trait 消费 DisplayItem——CLI 是
  文本行（`TextRenderer`），TUI 是 widget 状态（App 的 `Renderer` impl）；
- **run_turn**：共享 turn runner 逐事件流式回调 `&mut dyn Renderer`，返回更新后的
  消息历史，CLI 与 TUI 共用同一 turn 循环。

token 用量保持前端关注点：运行结束后前端读取其具体 provider 的 total usage，
喂一条 `DisplayItem::Usage` 给自己的 renderer。

### 显式上下文缓存与缓存命中统计（llm-cache + cache-last-message-mark）

每一轮 agent turn 都把完整消息历史（system + user + assistant + tool …）发给
百炼兼容端点，其中 system 提示（含工具定义）在多轮之间几乎不变、历史部分则随轮次
增长。显式上下文缓存把稳定前缀交给端点缓存：开启时（默认开启），system 消息以及
自尾部扫描到的**最后一条「非空文本的 user/assistant/tool」消息**都把 `content` 序列化为
单元素块数组并携带 `cache_control: {"type": "ephemeral"}` 标记；端点以最靠后的
标记为终点向前回溯最长匹配前缀命中缓存，命中的 token 数经
`usage.prompt_tokens_details.cached_tokens` 回传，创建缓存的 token 数经
`usage.prompt_tokens_details.cache_creation_input_tokens` 回传。因此「system 前缀」
与「完整对话前缀」都进入缓存，下一轮追加新消息后此前全部历史命中、只为新消息计费。

为什么用显式缓存而非其它方案：slimcode 走 OpenAI 兼容 **Chat Completions**
（`/chat/completions`），显式缓存在该接口上直接可用、命中确定性最高；Responses
API 的 Session 缓存（`x-dashscope-session-cache` header）不在范围内。隐式缓存是
百炼自动行为，无需请求侧改动——即使关闭显式缓存，`cached_tokens` 的解析与展示对
隐式命中同样生效。

标记落在 system 与最后一条可缓存消息上：system + 工具定义是最稳定的前缀，逐轮
确定性命中；历史部分则要等下一轮变成前缀后才命中，故把 mark 放在当前历史末尾。
尾部若是空文本（assistant 工具调用 / 空 tool 结果）则跳过并向前找第一条可缓存消息
（百炼不接受空 content 上的 mark）。`cache_control` 只加在 `content`（而非
`tools`），符合百炼「工具定义随 system 参与缓存计算」的约定。

百炼只在**数组形态**的 content 上接受 `cache_control`，且按 content 块匹配前缀：
若只有被标记的消息用数组形态、其余用字符串，某条消息从「末尾带 mark 的数组」变成
「历史不带 mark 的字符串」时字节就变了，前缀匹配会断、历史缓存永远不命中。因此
开启缓存时**所有非空文本消息一律用数组形态**，唯一差异是是否带 mark（mark 属比较
豁免的元数据）。关闭缓存时所有消息回到旧的字符串形态，请求字节与未开启缓存的客户端
完全一致，缓存是纯增量功能。

缓存命中的 token 数随 `total_usage` 跨轮累计。CLI 在运行结束时打印汇总行
`tokens: {prompt} prompt ({cached} cached, {pct}%) + {completion} completion =
{total} total`；TUI 不打印每轮用量行（ADR-0006 移除），改为 footer 的紧凑统计
（`↑in ↓out Rcache WcacheWrite CH{pct}%`，随每轮累计值更新）与 `/usage` 的 dim
notice 行（措辞与 CLI 共用 `usage_summary`，含同样式汇总）。`{pct}` 为缓存命中百分比
（`cached / prompt`，保留一位小数）；端点未回传
`prompt_tokens_details`（未命中、模型不支持或缓存已关闭）时显示 0 / 0%，任何配置都不报错。创建缓存 token 数保留在 `TokenUsage` 模型上
供未来细粒度展示。注意创建缓存按 125% 输入价计费一次、命中按低价计费，且被缓存
前缀需 ≥ 1024 token 才会实际命中（默认 system 提示较短时可能不触发，属运行时
行为，不影响代码正确性）。

## 设计动机

上下文组装曾散落在 CLI 的两个前端（非交互 `run_once` 与交互 TUI 的 `submit_prompt`）
各自实现一遍「system / message history / user」规则，skill 触发再绕一层，规则
重复、容易漂移。把组装收敛为 `slimcode-app::context` 的单一 `ContextBuilder`
后，one-shot 前端与 TUI（在 CLI 里）消费同一套规则，未来 Web 前端也可直接复用。

同样地，渲染与 turn 执行曾由 CLI 独占：事件→输出的映射和 turn 循环只在
`crates/cli` 里，TUI 若另写一套必然漂移。把 `map_event` / `DisplayItem` /
`Renderer` / `run_turn` 上收到 `slimcode-app`（ADR-0004）后，CLI 与 TUI
共享同一份事件到显示的映射与同一个 turn 循环，每个前端只实现自己的 `Renderer`，
行为可预测、不漂移。

**为什么 TUI 替代 REPL**（ADR-0003）：行式 REPL 无法区分 Shift+Enter 与 Enter，
多行输入只能靠 `\` 续行（ADR-0001 的取舍）；raw mode 一旦开启（TUI 的必然），
Shift+Enter 与 Enter 可区分，Enter 提交、Shift+Enter 换行，交互自然得多。同时
TUI 提供滚动 transcript、`PgUp`/`PgDn` 翻页、鼠标滚轮滚动（ADR-0017）、输入历史
recall（`↑`/`↓`）、pi 风格
的 dock footer 与状态指示器（运行中 spinner 行），可维护性也更好——ratatui 的
widget 模型 + `TestBackend` 让 UI 逻辑可做帧缓冲
测试（spec：好的测试断言帧缓冲，而非内部状态）。代价是交互式前端引入
crossterm/ratatui 依赖（仅交互式前端，ADR-0001 的无依赖立场对交互前端让位）；
不带 prompt 且非 TTY 时打印明确错误并以非零码退出。
