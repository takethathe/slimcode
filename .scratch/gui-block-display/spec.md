# GUI 转录块化显示：Skill active block 与 Transcript chip

Status: ready-for-agent

来源：本次 grill 会话 + `GLOSSARY.md` 新增词条 **Skill active block** / **Transcript chip**。
纯前端改动（`gui/ui`），后端 Tauri 宿主 / GUI wire 事件零改动。测试 seam 已与用户确认：
**transcript reducer**（纯函数），并为此在 `gui/ui` 引入 Vitest；渲染层手动验收。

## Problem Statement

GUI 前端当前把每条消息渲染成带 "You"/"AI" 圆形头像的对话气泡。当用户用一个 `/skill:name`
（或 `/{name}`）触发某个 skill 时，前端显示的**不是用户敲的那行**，而是 backend 展开后回传的整段
`<skill name="..." location="...">…</skill>` XML 块——对用户是一堆噪音。同样，当模型自己 `read`
某个 skill 的 `SKILL.md` 来激活它时，GUI 只显示一张普通的 `read` 工具卡，看不出"这里激活了一个 skill"。
此外，任何以 `/xxx` 开头的命令/触发词落到转录里后，和普通 prompt 文本视觉上无从区分。

用户想要：转录区按**块**来呈现，去掉 You/AI 头像；用户输入用**粗体 + 更醒目的颜色块**；skill 激活
做成一个**显眼提示**（如 `skill xxxx activated` 那样的形式）；命令与 skill 的触发词在块里用**小徽标**
重点显示。

## Solution

重新定义 GUI 转录的显示模型：所有信息以块呈现，不再有角色头像。skill 激活与命令/skill 触发词各有
其强调形态，全部由**前端检测消息/工具结果文本里的 `<skill>` 块**得出（backend 已把该块放进用户消息
和 skill-serving read 结果里），因此直播流与 resume 回放走同一套规则，无需新增 wire 事件。

三类可观测显示：
- **user 块**：粗体 + 浅靛蓝底（比现在醒目），取代 "You" 头像。若文本以 `/{token}` 开头，该触发词
  （含斜杠）渲染成 inline **Transcript chip**：来自 skill 触发的用 teal 徽标，普通命令触发词用 amber 徽标。
- **assistant 块**：无边框正文流式（含 reasoning 折叠与流式光标），取代 "AI" 头像。
- **Skill active block（模型激活路径）**：当一次 `read` 的结果文本是 `<skill>` 块（即模型主动激活 skill），
  把那张 read 工具卡**原位替换**成一块 slate 底 + accent 色条的提示：`Skill "<name>" activated`。

两条激活路径**非对称呈现**（关键决定）：用户触发是一条 prompt，落到 user 块里（带 teal chip），不再叠加
独立提示块；模型 `read` 激活没有用户消息，只能靠独立提示块表达。

## User Stories

1. 作为 GUI 用户，我想转录区不再显示 "You"/"AI" 头像，以便界面更简洁、信息密度更高。
2. 作为 GUI 用户，我想每条消息以独立的块呈现，以便一眼分辨谁说了什么、哪段是工具/系统信息。
3. 作为 GUI 用户，我想我输入的 prompt 用粗体显示，以便我的发言在长转录里清晰可辨。
4. 作为 GUI 用户，我想我的输入块用比助手正文更醒目的底色，以便快速定位"我说过的话"。
5. 作为 GUI 用户，我想助手的回复以普通的无边框块呈现，以便阅读长篇回答时不被头像/气泡打断。
6. 作为 GUI 用户，当我用 `/skill:name` 触发一个 skill 时，我想看到那条 skill 被激活的明确信号，以便知道模型这次带上了哪些指令。
7. 作为 GUI 用户，触发 skill 后我不想在转录里看到整段 `<skill …>` XML 块，以便界面不被模型才需要的噪音污染。
8. 作为 GUI 用户，我想 skill 触发显示成带 skill 名的强调徽标（如 `/grill`），以便既看清我触发了哪个 skill、又能读到跟随的任务参数。
9. 作为 GUI 用户，当我的 skill 触发没有额外任务参数时，我也想看到那个只含触发词的徽标，以便这次触发不会在转录里凭空消失。
10. 作为 GUI 用户，当模型自己 `read` 一个 `SKILL.md` 来激活 skill 时，我想看到一句 `Skill "x" activated` 提示，以便知道发生了 skill 激活而不是普通文件读取。
11. 作为 GUI 用户，我想那次激活 skill 的 `read` 不再显示成普通 read 工具卡，以便同一件事不被呈现两遍。
12. 作为 GUI 用户，我想激活提示的视觉形态（slate 底 + 色条）区别于我的输入块，以便分辨"系统注入"和"我说的话"。
13. 作为 GUI 用户，我想任何以 `/xxx` 开头的命令落到转录里时，其触发词显示成徽标，以便分辨一条命令和一句普通 prompt。
14. 作为 GUI 用户，我想 skill 触发词徽标和命令触发词徽标用不同颜色，以便区分这两类 `/` token。
15. 作为 GUI 用户，当我恢复一个历史会话时，我想 skill 激活与命令徽标按与直播时相同的规则呈现，以便回放和实时看到的转录一致。
16. 作为 GUI 用户，被前端拦截的本地命令（`/new`、`/usage`、`/compact`、`/help`、`/exit`）不进入对话历史，所以我不想它们被误显示成带徽标的用户消息。
17. 作为审阅者，我想这套显示完全由前端从消息文本推导（解析 `<skill>` 块），以便后端 wire 事件契约与 controller 保持不变。
18. 作为维护者，我想 skill/chip 的判定逻辑落在纯 reducer 里并可用单测锁定，以便回归不依赖手动点界面。
19. 作为 GUI 用户，我想去掉头像后工具卡、用量行、压缩提示、错误块保持原有呈现，以便本次改动只影响用户关心的显示面。

## Implementation Decisions

**范围**：仅 `gui/ui`（React/TS）。`gui/src-tauri`（宿主、`GuiWireEvent`、`controller`）与 SDK/app 零改动。
依据事实：SDK 的 `before_tool` skill 拦截 hook 已把 skill-activation 块写入 skill-serving `read` 的工具结果；
用户触发的 skill 块作为该轮的 user 消息文本回传前端。两者都以 `<skill name="..." location="...">` 起始，可被前端识别。

**检测机制（前端单一职责）**：新增一个纯 TS `parseSkillBlock(text)`，镜像 `slimcode_app::skills::parse_skill_block`
的契约——仅当 `text` **以** `<skill name="…"` 起始时解析出 `(name, innerContent)`，否则 `None`。它同时用于用户触发
消息、skill-serving read 结果、以及 resume 快照消息三处。

**转录块模型（`transcript.ts`）新增/变更 Block 变体**：
- `user` 块新增可选语义标记：其文本若以 `<skill>` 起始 → 视为用户触发的 skill，重建显示为 `/{name}` 徽标 +
  `</skill>` 之后的任务参数（若有）；徽标类别 = `skill`。否则其文本若以 `/{token}` 起始 → 徽标类别 = `command`；
  普通文本 → 无徽标。判定顺序：先 skill 块检测，再命令检测。
- 新增 `skill` 块变体（slate 提示，`Skill "<name>" activated`），仅由**模型 read 激活路径**与**回放中的等价情形**产生。

**reducer 三处分支**：
- `message`(role=user)：`textOf` 结果喂给 `parseSkillBlock` → 命中则产出带 `skill` 徽标的 user 块（name + 尾部任务参数），
  不显示 XML；未命中但文本以 `/` 起始 → 产出带 `command` 徽标的 user 块。
- `tool_result`：若该结果文本以 `<skill>` 起始，则把其配对 `read` 工具卡**原位替换**成 `skill` 块（不留 read 卡）；否则维持现有卡片配对。
- `blocksFromHistory`：role=user 消息走同一 `parseSkillBlock`（与直播一致）；role=tool 的 skill-serving 结果替换其配对 read 卡为 `skill` 块。

**呈现（`Transcript.tsx`，手动验收）**：
- 删除 user/assistant 的 "You"/"AI" 圆形头像。
- user 块：粗体、浅靛蓝底 + 深靛蓝字（现有靛蓝体系加深、加粗）。assistant 块：无边框正文流式（保留 reasoning 折叠与光标）。
- skill-serving 提示块：slate 底 + 左侧 accent 色条，文案 `Skill "<name>" activated`（英文，措辞用 activated）。
- Transcript chip：inline 圆角徽标包住含斜杠的触发词，`font-mono`；skill 徽标 teal 系、command 徽标 amber 系；均在 user 块底色之上仍需可读。

**非对称呈现决定（写进 ADR-0027）**：用户触发只显示带徽标的 user 块（无独立提示块）；模型 `read` 激活显示 slate 提示块。
两路都从 `<skill>` 文本检测得出，无独立激活事件。

**术语**：全程使用 `GLOSSARY.md` 的 **Skill active block** 与 **Transcript chip**（后者是提交侧对偶于输入侧 **Command chip**）。
与 **Command** / **Skill** / **Prompt** / **Transcript** 定义一致；不改 **Session log**、wire 契约。

**架构登记**：新增 `docs/adr/0027-gui-skill-active-block-and-transcript-chip.md`（前端从消息文本检测 `<skill>`、而非后端加
wire 事件；两激活路径非对称呈现；为此在 GUI 引入 reducer 单测），并在 `docs/index.md` 的 ADR 索引登记。

## Testing Decisions

- **只测外部行为**：断言"给定输入 → 得到何种 Block 列表"，不断言内部实现（不测具体 Tailwind 类名、不测组件 DOM）。
- **seam = transcript reducer（纯函数）**，全部逻辑在此终止为可观测 Block：
  - `parseSkillBlock`：命中/不命中、仅"以 `<skill>` 起始"才解析、XML 内 name 与 innerContent 的切分、尾部任务参数留在块外。
  - 用户触发 skill：含 `<skill name="grill">…</skill>\n\n<arg>` 的 user 消息 → 得 skill-chip user 块（name=grill、arg 正确、无 XML）；
    无参数时仍产出仅含徽标的 user 块。
  - 命令触发：以 `/deploy x` 起始的 user 消息 → command-chip user 块；普通文本 → 无 chip。
  - 模型 read 激活：`tool_start(read)` + 以 `<skill>` 起始的 `tool_result` → read 卡原位替换为 `skill` 块（列表里不再有该 tool 块）。
  - 判定优先级：同一轮既有 skill 又有其它 `/` 文本时不误判；"句中"出现 `<skill>` 不触发（须起始）。
  - resume 回放：`blocksFromHistory` 对 skill 用户消息与 skill-serving read 结果，产出与直播等价的用户触发/`skill` 块，验证一致性。
  - 本地拦截命令（`/usage` 等）不进入 user 消息流，故 reducer 不为其产生 chip（回归护栏）。
- **引入 Vitest**（`gui/ui` dev 依赖 + `test` 脚本 + 最小配置）：这是本切片对 ADR-0024 D7"v1 不引 Vitest"的**有意反转**，
  取舍理由是 skill/chip 判定逻辑值得锁定；渲染层仍不进单测。
- **prior art**：Rust 侧 `crates/cli/src/render.rs` 的 skill-read/history-replay 到 skill 块映射测试，以及 `crates/app/src/skills.rs`
  的 `parse_skill_block` 单测——本切片是其在 GUI reducer 的对偶；前端块模型测试形态参照 reducer 现有纯函数风格。

## Out of Scope

- 后端 `GuiWireEvent` / controller / SDK / app 任何改动（检测全在前端）。
- 渲染层（`Transcript.tsx` 头像删除、teal/amber/slate 具体配色与徽标 DOM）的自动化测试——维持手动验收。
- 新增 wire 事件来表达 skill 激活（明确否决，走前端文本检测）。
- 被前端拦截的本地命令（`/new` `/compact` `/help` `/exit` `/usage`）的转录呈现改动——它们不产生 user 消息。
- TUI 的 skill 块 / Ctrl+O 展开——TUI 已是 `[skill] <name>` 形态，本切片不动。
- 暗色主题 / 主题系统——GUI 现有浅色单一配色。
- 会话日志格式变更——skill 仍以 `<skill>` 块文本存于既有 user/tool 记录中。

## Further Notes

- 关键事实核对来源：SDK `create_agent_session` 的 `before_tool` hook 将 skill-serving read 结果替换为 skill 激活块；
  `prepare_user_prompt` 把用户 `/skill:name` 触发展开为 `<skill>` 块作为该轮 user 消息；两者均以 `<skill name="…"` 起始，故前端可统一检测。
- 措辞修正：用户原话 "actived" → "activated"。
- "block 提示" 的具体形态（`Skill "x" activated`）、徽标取词（含斜杠 `/grill`）、两路非对称、resume 一致性、配色（teal/amber/slate）
  均在 grill 会话中逐条确认。
- 落地顺序建议：先补 Vitest 基建 → `parseSkillBlock` red/green → reducer 三处分支 red/green → `Transcript.tsx` 呈现（手动验收）。
