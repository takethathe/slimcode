# slimcode 用户手册

> 本文件面向最终用户，说明安装、配置与使用方式。随代码变更同步维护。

## 安装

cargo workspace，从源码构建：

```bash
cargo build --release
```

产物为 `target/release/slimcode`。需要 Rust（edition 2024）与 cargo。

## 配置

slimcode 的配置分三层，优先级从高到低：环境变量 > `config.toml` > 默认值。

- **API key（必填）**：只从环境变量 `DASHSCOPE_API_KEY` 读取，绝不落盘：

  ```bash
  export DASHSCOPE_API_KEY=sk-...
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
| `<prompt>` | 作为用户消息运行一轮 agent 循环 |
| `/help` | 列出命令 |
| `/new` | 新建会话 |
| `/load <id>` | 从磁盘恢复一个已保存会话（`/resume` 同义） |
| `/sessions` | 列出已保存会话 id |
| `/usage` | 显示累计 token 用量 |
| `/save` | 显式保存当前会话 |
| `/exit` / `/quit` | 退出 |

### 会话文件

每个会话以 JSON 存于 `~/.slimcode/sessions/`，消息模型为
`Message{role, parts, tool_calls, tool_call_id}` + `Session{id, created_at,
messages, title}`。`/load` 恢复会话后，历史消息（含系统提示）原样继续。
会话不记录工作目录——恢复后工具作用于当前启动目录。

### 工具集

agent 在启动目录内可用七种工具：`read`、`write`、`edit`、`bash`、`grep`、
`find`、`ls`。工具调用默认串行执行；工具失败会以 `Error: …` 反馈给模型供其
自行纠正。
