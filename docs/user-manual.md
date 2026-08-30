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
  ```

- **config.toml（可选）**：位于 `~/.slimcode/config.toml`（可用 `SLIMCODE_HOME`
  覆盖目录）。只放非敏感覆盖项：

  ```toml
  [ai]
  base_url = "https://dashscope.aliyuncs.com/compatible-mode/v1"
  model = "qwen-plus"
  ```

- **环境变量覆盖**：

  | 变量 | 作用 | 默认 |
  | --- | --- | --- |
  | `DASHSCOPE_API_KEY` | 百炼 API key（必填） | — |
  | `SLIMCODE_AI_BASE_URL` | 端点 base URL | `https://dashscope.aliyuncs.com/compatible-mode/v1` |
  | `SLIMCODE_AI_MODEL` | 模型 id | `qwen-plus` |
  | `SLIMCODE_HOME` | slimcode 家目录（含 `config.toml` 与 `sessions/`） | `~/.slimcode` |

## 使用方式

### 单次非交互模式

在目标目录内运行一条 prompt，完成后退出：

```bash
slimcode "为 README 补一段简介"
slimcode --cwd /path/to/repo "运行 cargo test 并修复失败用例"
slimcode --model qwen-max "为 README 补一段简介"
```

运行结束后打印 token 用量与本次会话的保存路径。

### 交互式 REPL

不带参数启动即进入行式 REPL，会话在每一轮后自动保存到
`~/.slimcode/sessions/<id>.json`：

```bash
slimcode
```

| 命令 | 作用 |
| --- | --- |
| `<prompt>` | 作为用户消息运行一轮 agent 循环；以 `\` 结尾的行续行，非 `\` 行（或空行）提交整个多行 prompt 为一条用户消息 |
| `/help` | 列出命令 |
| `/new` | 新建会话 |
| `/load <id>` | 从磁盘恢复一个已保存会话（`/resume` 同义） |
| `/sessions` | 列出已保存会话 id |
| `/usage` | 显示累计 token 用量 |
| `/save` | 显式保存当前会话 |
| `/history` | 列出输入历史（最近 20 条、最新在前、带编号） |
| `/!!` | 重跑最近一条 prompt（作为新一轮，不重复写入历史） |
| `/!N` | 重跑编号 N 的 prompt（verbatim，多行原样） |
| `/exit` / `/quit` | 退出 |

### 输入历史与多行 prompt

输入历史（`input history`，区别于会话的消息历史 `message history`）记录你提交过的
普通 prompt，存于 `~/.slimcode/history.json`（JSON 数组，上限 500 条，超出丢最旧），
跨运行保留；`/` 命令不记入。`/!N` 编号以 `1` = 最新，重跑沿用当前会话、保留消息历史。

多行 prompt：以 `\` 结尾的行会继续下一行，直到遇到不以 `\` 结尾的行（或空行）才提交
为**一条**用户消息。续行态中 `/` 开头的行也作为 prompt 内容。实现无 readline 依赖、
无 raw mode（Shift+Enter 与 Enter 在传统终端不可区分，故不支持，见
`docs/adr/0001-repl-input-history-dependency-free.md`）。

### 会话文件

每个会话以 JSON 存于 `~/.slimcode/sessions/`，消息模型为
`Message{role, parts, tool_calls, tool_call_id}` + `Session{id, created_at,
messages, title}`。`/load` 恢复会话后，历史消息（含系统提示）原样继续。
会话不记录工作目录——恢复后工具作用于当前启动目录。

### 工具集

agent 在启动目录内可用七种工具：`read`、`write`、`edit`、`bash`、`grep`、
`find`、`ls`。工具调用默认串行执行；工具失败会以 `Error: …` 反馈给模型供其
自行纠正。
