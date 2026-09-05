# slimcode 用户手册

> 本文件面向最终用户，说明安装、配置与使用方式。随代码变更同步维护。

## 安装

cargo workspace，从源码构建：

```bash
cargo build --release
```

产物为 `target/release/slimcode`。需要 Rust（edition 2024）与 cargo。

## 配置

> 详细说明见 [configuration.md](./configuration.md)：四层配置来源、逐项覆盖规则与示例。

slimcode 的配置分四层，优先级从高到低：命令行参数 > 环境变量 > `config.toml` > 默认值。

- **API key（必填）**：只从环境变量 `DASHSCOPE_API_KEY` 读取，绝不落盘：

  ```bash
  export DASHSCOPE_API_KEY=sk-...
  ```

- **命令行参数（可选，临时生效）**：

  ```bash
  slimcode --model qwen-max "为 README 补一段简介"
  slimcode --model qwen-max --base-url https://my.example.com/v1 "列出当前目录"
  slimcode --no-cache "运行 cargo test 并修复失败用例"
  ```

- **config.toml（可选）**：位于 `~/.slimcode/config.toml`（可用 `SLIMCODE_HOME`
  覆盖目录）。只放非敏感覆盖项：

  ```toml
  [ai]
  base_url = "https://dashscope.aliyuncs.com/compatible-mode/v1"
  model = "qwen-plus"
  cache = false   # 可选：显式关闭上下文缓存（默认开启）
  ```

- **环境变量覆盖**：

  | 变量 | 作用 | 默认 |
  | --- | --- | --- |
  | `DASHSCOPE_API_KEY` | 百炼 API key（必填） | — |
  | `SLIMCODE_AI_BASE_URL` | 端点 base URL | `https://dashscope.aliyuncs.com/compatible-mode/v1` |
  | `SLIMCODE_AI_MODEL` | 模型 id | `qwen-plus` |
  | `SLIMCODE_AI_CACHE` | 显式上下文缓存开关（`true`/`false`/`1`/`0`/`yes`/`no`/`on`/`off`，非法值启动报错） | `true` |
  | `SLIMCODE_HOME` | slimcode 家目录（含 `config.toml` 与 `sessions/`） | `~/.slimcode` |

### 上下文缓存（默认开启）

显式上下文缓存默认开启：system 消息（含工具定义）作为稳定前缀交给端点缓存，
多轮/多会话复用同一前缀时命中缓存，节省输入 token。

- `--cache` / `--no-cache`（命令行，临时）、`SLIMCODE_AI_CACHE=true|false`（环境
  变量）、`[ai] cache = true|false`（`config.toml`）按「命令行 > 环境变量 > 文件
  > 默认」的优先级解析；不配置即为默认开启。
- 运行结束与 TUI `/usage` 的 token 用量汇总行包含缓存命中数：
  `tokens: {prompt} prompt ({cached} cached, {pct}%) + {completion} completion = {total} total`。
  `{cached}` 为累计缓存命中 token 数、`{pct}` 为命中百分比（`cached / prompt`，保留一位小数）；
  端点未回传命中数（未命中、模型不支持或缓存已关闭）时显示 `0` / `0%`。
- 注意：创建缓存本身按 125% 输入价计费一次；多轮命中后由命中价回收成本。
  系统前缀过短（< 1024 token）时可能不会实际命中，属端点运行行为。

## 使用方式

### 单次非交互模式

在目标目录内运行一条 prompt，完成后退出：

```bash
slimcode "为 README 补一段简介"
slimcode --cwd /path/to/repo "运行 cargo test 并修复失败用例"
slimcode --model qwen-max "为 README 补一段简介"
```

运行结束后打印 token 用量（含缓存命中数，如
`tokens: 3019 prompt (2048 cached, 67.8%) + 104 completion = 3123 total`）与本次会话的
保存路径。

### 交互式 TUI

不带参数启动（stdout 是终端）即进入全屏 TUI，会话在每一轮后自动保存到
`~/.slimcode/sessions/<id>.json`：

```bash
slimcode
```

**布局**：上部为 transcript（滚动输出区，显示每轮开始标记、流式文本、思考行与工具调用；流式文本与思考行按 delta 逐片段拼接成一行，换行只来自内容本身的 `\n`，不会每个 delta 另起一行；超长行按面板宽度自动折行、不截断；随输出自动滚动到底部），底部为输入框与状态行（当前会话 id、最近的 notice / error）。

**按键**：

| 按键 | 作用 |
| --- | --- |
| `Enter` | 提交输入框内容，作为用户消息运行一轮 agent 循环 |
| `Shift+Enter` | 在输入框中插入换行，支持多行 prompt |
| `↑` / `↓` | 输入框为空时进入输入历史 recall（从最新一条开始）；recall 中 `↑` 更早、`↓` 更新，`↓` 到最新再按退出到空输入；输入非空时移动光标 |
| `PgUp` / `PgDn` | transcript 上/下翻页；`/` 补全弹框打开时改为翻页候选列表 |
| `Tab` | 输入 `/` 前缀时把补全弹框中选中的候选**上屏**到输入框（尾部自动加空格、光标落在其后，不提交）；无候选时强制打开弹框 |
| `Esc` | 关闭 `/` 补全弹框（保留已输入文本） |
| `Ctrl+C` / `Ctrl+D` | 退出 TUI（恢复终端） |

recall 状态下按 `Enter` 会把选中的历史 prompt 作为**新一轮**运行（不再写入历史）。

#### `/` 命令补全弹框

输入框以 `/` 开头（且未输入参数空格）时，TUI 自动在输入框下方打开**补全弹框**，
模糊匹配候选命令与已安装 skill 并按相关度排序（连续命中、词边界、精确匹配加分；
间隔与靠后位置减分）：

- `↑` / `↓` 在候选间循环移动；`PgUp` / `PgDn` 翻页；
- `Tab` 把选中项**上屏**到输入框（尾部自动补一个空格、光标落在空格后，便于继续输入参数），
  不提交；
- `Enter` 把选中项展开为完整命令名后**直接提交执行**（而不是提交你正在输入的部分文本）；
- `Esc` 取消弹框、保留已输入文本；继续输入字符实时过滤，输入空格（进入参数段）时弹框关闭；
- 候选值始终是裸拼写（如 `/save`、`/resume`、`/skill-name`），不含 `usage` 中的参数占位符；
- 输入 `/` 后无候选时弹框不显示。

弹框只在前端 TUI 出现；命令定义、模糊匹配与候选合并都在前端无关的
`slimcode-commands` / `slimcode-common` 中（见 ADR-0005），匹配逻辑可独立单测。

**命令**（与行式 REPL 相同的 `/` 命令集，经共享命令注册表 `slimcode-commands`）：

| 命令 | 作用 |
| --- | --- |
| `/help` | 列出命令 |
| `/new` | 新建会话（清空 transcript） |
| `/load <id>` | 从磁盘恢复一个已保存会话（`/resume` 同义；清空 transcript 后载入其消息历史） |
| `/sessions` | 列出已保存会话 id |
| `/usage` | 显示累计 token 用量 |
| `/save` | 显式保存当前会话 |
| `/history` | 列出输入历史（最近 20 条、最新在前、带编号） |
| `/skills` | 列出已安装的 skill（含 user/project scope 与 manual-only 标记） |
| `/install-skill <path> --user\|--project` | 从路径安装一个 skill（目录含 `SKILL.md`，或单个 markdown 文件） |
| `/!!` | 重跑最近一条 prompt（作为新一轮，不重复写入历史） |
| `/!N` | 重跑编号 N 的 prompt（verbatim，多行原样） |
| `/exit` / `/quit` | 退出 |

**非 TTY**：不带 prompt 且 stdout 不是终端时，slimcode 打印明确错误并以非零退出码结束，不会尝试打开 TUI。

#### `/` 命令预测提示

输入 `/` 时，TUI 用**实时补全弹框**做预测（见上文「`/` 命令补全弹框」）。此外，
如果弹框未打开而仍提交了未知的 `/` 命令，TUI 会给出**预测提示**（来自前端无关的命令注册表
`slimcode-commands`）：

- 按已输入前缀匹配命令名或其别名，例如 `/his` → `did you mean: /history`；
- 前缀仅为 `/` 时列出全部命令；
- 完全无法匹配时提示运行 `/help`。

`/help` 的命令列表、启动时的命令提示条也由同一注册表生成，保证单一事实来源；
未来其它前端（TUI / Web 等）可复用同一套命令定义与补全逻辑。**Skill** 也以 `/` 触发，
未知 `/` 命令的预测提示与补全弹框都会把内置命令与已安装 skill 合并展示（见下节 Skills）。

### Skills（技能）

Skill 是一份可安装的 agent 指令：一个 `SKILL.md` 文件，开头为 YAML 风格
frontmatter，后接 markdown 正文。frontmatter 支持：

```markdown
---
name: my-skill
description: What this skill does and when to use it
disable-model-invocation: true   # 可选；省略 = false
---

# 正文（触发 /my-skill 时作为指令交给 agent）
```

- `name`：触发名（`/name`），只能是字母、数字、`_`、`-`，不能与内置命令重名；
- `description`：一句话说明（用于 `/skills` 列表与系统提示词）；
- `disable-model-invocation`：可选。设为 `true` 时该 skill 的**描述不会写入系统提示词**
  （agent 不会自动得知/调用它），只能通过显式 `/name` 触发；省略或 `false` 时描述会
  进入系统提示词，agent 可按需选用。

#### 作用域（user / project）

- **user**：`~/.slimcode/skills/`（可用 `SLIMCODE_HOME` 覆盖家目录），跨项目共享；
- **project**：`<cwd>/.slimcode/skills/`，仅当前项目；同名 skill 时 project 优先。

每个 skill 在对应目录下以 `<name>/SKILL.md` 存放；也可直接放 `<name>.md` 单文件。

**默认状态**：slimcode 不内置任何 skill——两个 scope 目录默认不存在（视为空），
首次 `/install-skill` 时才创建；所有 skill 均需自行安装，无随包分发的默认集。
想确认当前已装 skill 用 `/skills`。

#### 安装与使用

在 TUI 输入框中直接输入命令（无提示符）：

```text
/install-skill ~/skills/tdd --user
/install-skill ./my-skill.md --project
/skills
/tdd 为这个模块补测试
```

- `/install-skill <path> --user|--project`：把目录（含 `SKILL.md`）或单个 markdown
  文件复制到对应 scope，`--user` 与 `--project` 二选一；同名 skill 会被覆盖更新；
  与内置命令重名的 skill 会被拒绝安装；
- `/skills`：列出已安装 skill（触发名、描述、manual-only 标记、scope）；
- `/name [任务]`：触发一个 skill，把其正文（+ 可选任务，另附 skill 所在目录，
  供正文里的相对路径解析）作为一轮 agent 指令执行；与其它 `/` 命令一样，skill
  触发**不**写入输入历史；
- 预测提示：输入未知的 `/` 前缀时，候选同时包含内置命令与 skill（如 `/td` →
  `did you mean: /tdd`）；裸 `/` 列出全部。


### 输入历史与多行 prompt

输入历史（`input history`，区别于会话的消息历史 `message history`）记录你提交过的
普通 prompt，存于 `~/.slimcode/history.json`（JSON 数组，上限 500 条，超出丢最旧），
跨运行保留；`/` 命令不记入。`/!N` 编号以 `1` = 最新，重跑沿用当前会话、保留消息历史。

多行 prompt：在 TUI 输入框中按 `Shift+Enter` 插入换行，按 `Enter` 提交整个多行
prompt 为**一条**用户消息；多行 prompt 中的 `/` 开头行是 prompt 内容而非命令。
（ADR-0001 的 `\` 续行方案随行式 REPL 一并移除——raw mode 下 Shift+Enter 与 Enter
可区分，故 Shift+Enter 成为多行换行键，Enter 直接提交。）

### 会话文件

每个会话以 JSON 存于 `~/.slimcode/sessions/`，消息模型为
`Message{role, parts, tool_calls, tool_call_id}` + `Session{id, created_at,
messages, title}`。`/load` 恢复会话后，历史消息（含系统提示）原样继续。
会话不记录工作目录——恢复后工具作用于当前启动目录。

### 工具集

agent 在启动目录内可用七种工具：`read`、`write`、`edit`、`bash`、`grep`、
`find`、`ls`。工具调用默认串行执行；工具失败会以 `Error: …` 反馈给模型供其
自行纠正。
