# Session 命令收敛：全屏 picker（`/session`）与重置（`/new`）

Status: resolved

## Problem Statement

slimcode 目前有三个 session 命令，分属两套心智模型：`/load <id>`（按 id 恢复）、
`/sessions`（往 transcript 里打印一串裸 id）、`/new`。要恢复一个会话，用户得先让
`/sessions` 打出一屏 id，再把 id 抄进 `/load`——而打印出来的只有
`slimcode-1756-1234-0` 这种无法辨认的字符串，标题、时间、消息数一概看不到。参考实现
pi 用同一族命令把"列出并选择"做成了一个**交互式 session selector**，slimcode 采用它，
并把命令面收敛为两个：`/session`（全屏 picker：列出本项目所有会话、标出当前会话、
`Enter` 载入所选）与 `/new`（开新会话：清空消息、清空用量、清空屏幕）。

同时暴露出两个既有缺陷：`/new` 不重置用量统计（沿用进程级 provider 计数器），
`/load` 恢复后 footer 仍然显示**上一个会话**的 token（同一个计数器的另一个后果）；
而 `/load` 的"title / skipped / repaired"三条 notice 被紧随其后的 `SessionChanged`
清屏动作吃掉，用户永远看不到。

## Solution

命令表只剩 `/session` 与 `/new`：`/load`（含 `/resume` 别名）与 `/sessions` 删除。
`/session` 由 CLI 扫描当前项目的 session store、把行数据交给 TUI，TUI 用一个**全屏
视图**渲染：顶部一行标题与条数、中间列表（`*` 标当前会话、`›` 标光标行、右侧为
"消息条数 + 修改时间"）、底部一行按键提示；`↑`/`↓`/`PgUp`/`PgDn` 移动、`Enter` 载入、
`Esc` 关闭、滚轮滚动列表。`Enter` 产生的 `Effect::LoadSession` 复用既有 `load_session`
路径；`SessionChanged` 承担"清 transcript、重插启动 Header、footer 用量归零、更新
session id"四件事，于是 `/new` 与 `/load` 在 TUI 侧同构。

用量统计改为挂在 Session 的内存字段上（不落盘）：每轮把 provider 累计值的差值累加到
`session.usage`，`/usage` 与 footer 都读它。新会话与恢复的会话都从 0 起算（恢复后
看不到历史 token，这是明确接受的取舍）。

## User Stories

1. As a TUI 用户, I want 用 `/session` 一屏看到本项目所有会话, so that 我不用先 `/sessions`
   抄 id 再 `/load`。
2. As a TUI 用户, I want 每一行显示标题、消息条数与修改时间, so that 我能认出哪个会话是
   我要的（而不是靠 `slimcode-1756-1234-0`）。
3. As a TUI 用户, I want 当前会话被 `*` 标出、光标行被 `›` 标出, so that 我一眼分得清
   "我在哪"和"我选到哪"。
4. As a TUI 用户, I want 用 `↑`/`↓`/`PgUp`/`PgDn` 移动选择、`Enter` 载入, so that 我用
   键盘就能完成恢复（不需要打字搜索）。
5. As a TUI 用户, I want `Esc` 关闭 picker 并原样回到刚才的聊天视图, so that 误开 picker
   没有任何代价。
6. As a TUI 用户, I want 在 picker 里滚轮滚动的是列表而不是 transcript, so that 列表
   长时我能用鼠标翻。
7. As a TUI 用户, I want picker 里选中"当前会话"那一行时什么都不发生, so that 我不会因为
   回车而丢掉内存里比磁盘更新的内容。
8. As a TUI 用户, I want 从来没用过（还没有日志文件）的新会话不出现在列表里, so that
   列表只反映磁盘上真实存在的东西。
9. As a TUI 用户, I want 本项目一个会话都没有时 picker 显示空态而不是报错, so that 我知道
   命令本身没问题。
10. As a TUI 用户, I want 载入或新建会话后 transcript 被清空、启动 Header 重新出现, so that
    屏幕上不会混着上一段对话。
11. As a TUI 用户, I want `/new` 把 token 用量归零, so that 新会话的统计从 0 起算。
12. As a TUI 用户, I want 恢复的会话 token 也从 0 起算, so that 统计只反映"这个会话这次
    被用掉多少"（明确接受的取舍，写在用户手册里）。
13. As a TUI 用户, I want 载入一个需要修补的会话时看到"跳过 N 条记录 / 修补 N 个工具调用"
    的提示, so that 我知道磁盘上的内容不完整（修掉现在被清屏吃掉的 bug）。
14. As a 用户, I want `/load`、`/resume`、`/sessions` 从命令表里消失, so that 命令面只有
    一个恢复入口、不会有人再用 id 抄写。
15. As a 维护者, I want 行数据由 CLI 组装、选择状态留在 TUI, so that ADR-0013/0014 的
    seam 不被打破（TUI 依旧不依赖 app/ai/core）。
16. As a 维护者, I want 扫描在打开 picker 时同步完成且只做前缀判定, so that 打开 picker 的
    代价随日志行数而不是随 500 MiB 配额增长。
17. As a 维护者, I want 时间格式化零新依赖、零 unsafe, so that 不复用 `civil_from_days` 之外
    不动任何依赖图。
18. As a 维护者, I want 每个新行为都有测试钉住, so that 以后"让 picker 也能搜索"之类的改动
    不会静默改变现有语义。

## Implementation Decisions

### 命令表（slimcode-commands）

- 保留 `Command::new("/new", "/new", "start a new session")`。
- 新增 `Command::new("/session", "/session", "browse this project's sessions")`。
- 删除 `/load`（`Command::aliased("/load", &["/resume"], …)`）与 `/sessions`。
- `/session` 带参数时忽略参数（与 `/new`、`/usage`、`/skills` 的既有行为一致），不报错。
- 同步更新 registry 单测（`find_resolves_name_and_alias` 里 `/load`/`/resume` 用例改为
  `/session`；`suggest_prefix_matches_alias` 换成别的带别名命令或删除）。

### 存储层（slimcode-app 的 session 模块）

- 新增 `pub struct SessionSummary { pub id: String, pub title: Option<String>,
  pub modified: SystemTime, pub messages: usize }`。
- 新增 `SessionStore::entries(&self) -> Result<Vec<SessionSummary>, String>`：列出当前
  project 子目录下的 `.jsonl`，逐个做一次流式扫描：
  - `fs::metadata` 取 mtime（失败则跳过该文件）；
  - 第一行按日志头解析，**解析失败或缺头则整份跳过**（对应 pi 的"解析失败返回 null"）；
  - 逐行判定 record 的 `type`：为 `message` 则计数 +1；为 `title` 则记录首个标题（按
    需要解析该行）；其他类型（含未知类型）忽略；
  - 排序：mtime 降序，同 mtime 时按 id 降序（保证确定性）。
- `list()` 删除（唯一调用方是即将删除的 `/sessions`），现有隔离性测试改用 `entries()`
  并断言返回的 id 序列。
- 新增 `pub fn format_minute(secs: i64) -> String`，输出 `YYYY-MM-DD HH:MM`（复用同模块
  的 `civil_from_days`；**UTC 时钟、不做时区换算、不带 `Z`**）；`unix_to_rfc3339` 继续只
  用于落盘。需要把 `SystemTime` → 秒的转换一并提供（`SystemTime::duration_since(UNIX_EPOCH)`，
  失败按 0 处理，与 `now_rfc3339` 的既有写法一致）。
- 计数口径写死在文档注释里：**日志里 `type=message` 的 record 条数**（不等于载入后
  `session.messages.len()`——只有需要修补的日志才不同）。
- 前缀判定：把"读 record 的 `type`"抽成一个私有小函数（容忍 `:` 前后空格），配等价性
  测试：fixture 里包含用户内容中字面量 `{"type":"message"` 的转义形式、未知类型 record、
  坏行、合法 message/title record，断言计数与实际 message record 数一致。

### 用量归属（slimcode-core / slimcode-app / slimcode-cli）

- `Session` 新增 `#[serde(skip)] pub usage: TokenUsage`（默认值即 0；不落盘、不参与日志）。
- `slimcode-cli` 的 turn 执行路径：turn 前快照 `provider.total_usage()`，turn 结束后取差值
  累加到 `state.session.usage`，再用 `RenderItem::Usage(to_footer_usage(&session.usage))`
  更新 footer（一次 turn 可能含多轮 tool-loop 请求，差值才是本轮真实用量）。
- `/usage` 改为 `emit(RenderItem::Notice(usage_summary(&inner.session.usage)))`。
- 一次性 CLI（slimcode-cli 的 one-shot 路径）无会话，继续读 `provider.total_usage`，不动。

### TUI 视图（slimcode-tui）

- 显示词表：新增 `SessionRow { id, title, meta }` 与 `RenderItem::SessionPicker { rows }`
  （行 `title` 已由 CLI 回退为 id，`meta` 已由 CLI 组装成右侧字符串）。
- 运行 seam：新增 `Effect::LoadSession { id: String }`。
- App 状态与键位：
  - `App` 新增 `pub picker: Option<Picker>`，`pub struct Picker { pub rows: Vec<SessionRow>,
    pub selected: usize, pub offset: usize }`；
  - `apply(RenderItem::SessionPicker { rows })` → `picker = Some(Picker{ rows, selected: 0,
    offset: 0 })`；
  - `apply(RenderItem::SessionChanged { id })` → 清 transcript → 重新 push `Entry::Header`
    → `status.usage = FooterUsage::default()` → `status.session_id = id` → `picker = None`
    → `reset_view()`（仍然提前 return，跳过 park 视图的高度补偿）；
  - `handle_key`：`picker.is_some()` 时走 picker 键位并**不把按键送给文本区**：
    `↑`/`↓` 移动 1 行（边界 clamp）、`PgUp`/`PgDn` 移动 `PAGE_LINES`、`Enter` → 关闭
    picker + `Effect::LoadSession { id: rows[selected].id }`、`Esc` → 关闭 picker 且无
    Effect、Ctrl+C / Ctrl+D → `Effect::Quit`（保留全局退出语义）、其余键忽略；
  - `handle_mouse`：`picker.is_some()` 时滚轮上下移动选择 `WHEEL_LINES` 行（clamp），
    **不动** transcript 的 `scroll`/`follow`/`scrollbar_ticks`；
  - `draw`：`picker.is_some()` 时整帧只画 picker 并提前返回；
  - 运行中（`handle_key_running`）语义不变：picker 不可能在 turn 中途打开或操作。

### 渲染规格（picker）

```
Sessions (this project)                                    3 saved
  * Fix the parser crash                12 msgs  2026-02-14 15:32
  › Ship the session picker              4 msgs  2026-02-14 15:40
    slimcode-1756-1234-0                 1 msg   2026-02-11 09:02
  (2/3)
  Esc cancel · ↑/↓ move · PgUp/PgDn page · Enter load
```

- 布局：`[1 行 header][Min(0) 列表][2 行底部]`；底部第 1 行是提示、第 2 行留白（与聊天
  视图 footer 的 2 行高度一致）。
- 行 = `光标列(2) + 当前标记列(2) + 标题 + 空隙 + meta`：光标为 `› ` 或两个空格；当前
  会话（`row.id == status.session_id`）标记为 `* `，否则两个空格（保证标题列对齐）；
  `meta` 右对齐（dim）；标题超出可用宽度时以 `…` 截断；宽度不足以放 meta 时丢弃 meta
  优先保标题。
- 选中行：`selectedBg` 背景 + 加粗；当前会话的标题用 accent 色。
- 列表可见行数 = `列表高度`，行数溢出时最后一行改作 `(i/n)` 指示（`i` = 当前选中序号，
  1-based），可见行数相应减一；窗口 `offset` 保证选中行始终可见。
- 空态：列表区显示 `  no saved sessions in this project`（dim），提示行显示 `Esc close`。

### CLI 接线（slimcode-cli 的 TUI handler）

- `command()`：`"/session"` → 新增 `open_session_picker(emit)`：
  `store.entries()` 成功则把每个 summary 组装成 `SessionRow`（`title.clone().unwrap_or(id)`；
  `meta = format!("{count} {msgs}  {time}")`，`msgs` 单数用 `1 msg`、复数用 `N msgs`，
  `time = format_minute(mtime)`），`emit(RenderItem::SessionPicker { rows })`；失败则
  `emit(RenderItem::Error(..))` 且不开 picker。`"/load"`/`"/sessions"` 分支删除。
- `on_effect`：新增 `Effect::LoadSession { id } => self.load_session(&id, emit)`。
- `load_session(id)`：
  - **幂等守卫**：`id == self.session_id()` 时直接 `ControlFlow::Continue`（不重载、不清屏、
    不重置用量）；
  - 顺序修正：先 `emit(RenderItem::SessionChanged { id })`，**之后**才发 title / skipped /
    repaired / `loaded session: <id>` 等 notice，再发 `Branch` 与 `set_title`（今天这些
    notice 发在 `SessionChanged` 之前，会被清屏吃掉）。
- `new_session()` 逻辑不变（依赖 `SessionChanged` 的新语义完成用量归零与 Header 重插）。

## Testing Decisions

- **好测试的标准**：断言外部可观察行为——picker 打开/关闭、选中行、`Enter` 产生哪个
  Effect、渲染出的字符、`SessionChanged` 之后 footer 用量与 transcript 内容、`entries()`
  返回的标题/条数/顺序、日志扫描对坏文件的取舍；不断言内部窗口算法细节本身（只断言
  可见结果）。每个 tick 的行为都落在上面三个既有 seam 上，不新建 seam。
- **三个 seam，零新增（沿用现有基建）**：
  - `slimcode-app` 的 session 模块单测（`temp_dir` + 手写 `Session`/`append`）；
  - `slimcode-tui` 的 App 单测（`TestBackend` 渲染断言 + `handle_key`/`handle_mouse`
    返回的 `Effect`）；
  - `slimcode-cli` 的 handler 单测（既有 `Fixture`/`command()` 辅助）。
- **红先清单**：
  - registry：`/session` 存在、`/load`·`/resume`·`/sessions` 不再解析
    （`find()` 返回 `None`），`suggest("/")` 数量随之变化；
  - `entries()`：标题优先、缺标题回退（由 CLI 层回退，store 返回 `None`）、计数等于
    message record 数（含未知类型/坏行/内容里带 `{"type":"message"` 字面量的 fixture）、
    mtime 降序 + id 降序 tie-break、坏 header 的文件被跳过、空目录返回空、
    `list()` 的既有隔离性测试迁移到 `entries()`；
  - `format_minute`：`0` → `1970-01-01 00:00`，以及一个已知时间戳（UTC 时钟）；
  - `Session.usage`：新会话默认 0；`#[serde(skip)]` 不写进日志（对已有日志格式的回归断言）；
  - TUI：`apply(SessionPicker)` 后 `draw` 出现 header/行/`*`/`›`/`(i/n)`/空态；`↑`/`↓`/
    `PgUp`/`PgDn` 的 clamp；`Enter` → `Effect::LoadSession{id}` 且 picker 关闭；`Esc` →
    无 Effect 且 picker 关闭；picker 打开时按键不进入输入框、滚轮不动 `scroll`/`follow`、
    不触发历史 recall；`SessionChanged` → transcript 只剩 Header、`status.usage` 归零、
    `status.session_id` 更新、picker 关闭；
  - CLI：`/session` → 发出 `SessionPicker` 且行内容正确（标题回退、`1 msg`/`N msgs`、
    时间格式）、scan 失败 → `Error` 且不发 picker；`/new` → `SessionChanged` + notice；
    `Effect::LoadSession` 载入磁盘会话、notice 顺序在 `SessionChanged` 之后可见、
    选当前会话 id 时无副作用。
- **先例**：slimcode-tui 的 completion popup 渲染/键位测试、ADR-0017 的滚轮
  测试（滚轮不动 transcript）直接作为 picker 滚轮测试的模板；session 模块的
  `temp_dir` 与 mtime 回填辅助（`write_aged`）复用。

## Out of Scope

- picker 里的搜索/过滤（`re:`/`"phrase"` 语法、fuzzy 排序）、`Tab` scope 切换（slimcode
  按 ADR-0008 固定为当前项目）、`Ctrl+S` 排序模式、`Ctrl+N` 只看命名会话、`Ctrl+P` 路径
  显示、`Ctrl+R` 重命名、`Ctrl+D` 删除。
- 异步扫描与 `Loading n/N` 进度、任何 seam 的异步通道。
- 用量的持久化（不新增 `usage` record 类型，不改日志格式）、`cost` 统计。
- `/name`（用户自定义会话名）、`/fork`/`/tree`（树形会话）、CLI 的 `-r`/`--resume` 启动
  flag、一次性 CLI 的 session 行为。
- 鼠标点击选择行、拖拽、picker 内的 transcript 查看。
- 目标平台的非 Unix 时间处理（picker 只做 UTC 时钟渲染，无平台分支）。

## Further Notes

- **与 pi 的差异清单（有意偏离，写进 ADR-0018）**：命令名相反（pi 的 `/session` 是 info
  dump，picker 叫 `/resume`）；删掉 pi picker 的搜索/scope/排序/重命名/删除；`/new` 在
  turn 运行中不可达（pi 允许并 teardown 当前 turn，slimcode 保留"运行中忽略按键"）；
  选中当前会话不重载（pi 会重载）；用量不落盘（pi 按 entry 持久化）。
- **时间显示是 UTC 时钟**，不带 `Z`、不做时区换算——这是明确的设计取舍（复用零依赖的
  `civil_from_days`，不引入 chrono/jiff，也不写 `libc::localtime_r` FFI），不要当成 bug 去"修"。
- **文档同步（与代码同一提交）**：`CONTEXT.md`（`Command` 举例改 `/session`、`Session`
  词条补"内存中的用量"、`Session store` 词条里的 `/load`/`/sessions` 改为 picker，新增
  **Current session** 与 **Session picker** 两个词条）；`README.md` 与
  `docs/user-manual.md` 的命令表（删 `/load`·`/resume`·`/sessions`，加 `/session`）与
  用户手册新增 picker 小节（按键、`*`/`›` 含义、`(i/n)`、空态、恢复后用量从 0 起算）；
  `docs/development.md`（命令语义清单、TUI 视图与 seam 描述）；`docs/explanation.md`
  中提到 `/load` 的位置；`docs/index.md` 的 ADR 目录行加入 ADR-0018。
- **验收**：`cargo test` 全绿；`cargo fmt --all`；`cargo clippy --all-targets
  --all-features --message-format=json -- -D warnings` 0 error / 0 warning；仅本地
  `git commit`（不 push）；提交信息形如 `feat(cli): collapse session commands into a picker`。
