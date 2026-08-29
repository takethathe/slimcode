# slimcode

一个用 Rust 编写的 AI coding agent CLI。输入一条 prompt，agent 会在目标目录内运行
`read` / `write` / `edit` / `bash` / `grep` / `find` / `ls` 七种工具循环完成任务，
支持流式输出、token 用量统计与可保存/恢复的会话。

## 特性

- 单次非交互模式：`slimcode "为 README 补一段简介"`
- 交互式 REPL：`/help`、`/new`、`/load`、`/save`、`/usage`、`/history` 等命令
- 流式渲染 agent 输出，会话每轮自动保存
- 基于 DashScope（百炼）OpenAI 兼容接口，默认使用 `qwen-plus` 模型
- 多行 prompt（以 `\` 结尾续行）与跨运行输入历史

## 架构

cargo workspace，三个 crate：

| crate | 包名 | 职责 |
| --- | --- | --- |
| `crates/ai` | `slimcode-ai` | 统一 LLM provider 层（Provider trait + OpenAI-compatible/Bailian） |
| `crates/agent` | `slimcode-agent` | agent 运行时、七工具引擎、会话消息模型 |
| `crates/cli` | `slimcode` | 二进制入口（非交互模式 + REPL） |

## 安装

cargo workspace，从源码构建：

```bash
cargo build --release
```

产物为 `target/release/slimcode`，需要 Rust（edition 2024）与 cargo。

## 配置

配置分三层，优先级从高到低：环境变量 > `config.toml` > 默认值。

- **API key（必填）**：只从环境变量 `DASHSCOPE_API_KEY` 读取，绝不落盘：

  ```bash
  export DASHSCOPE_API_KEY=sk-...
  ```

- **config.toml（可选）**：位于 `~/.slimcode/config.toml`（可用 `SLIMCODE_HOME`
  覆盖目录），只放非敏感覆盖项：

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

## 使用

### 单次非交互模式

在目标目录内运行一条 prompt，完成后退出：

```bash
slimcode "为 README 补一段简介"
slimcode --cwd /path/to/repo "运行 cargo test 并修复失败用例"
```

运行结束后打印 token 用量与本次会话的保存路径。

### 交互式 REPL

不带参数启动即进入 REPL，会话在每一轮后自动保存到
`~/.slimcode/sessions/<id>.json`：

```bash
slimcode
```

| 命令 | 作用 |
| --- | --- |
| `<prompt>` | 作为用户消息运行一轮 agent 循环；以 `\` 结尾的行续行 |
| `/help` | 列出命令 |
| `/new` | 新建会话 |
| `/load <id>` | 从磁盘恢复一个已保存会话（`/resume` 同义） |
| `/sessions` | 列出已保存会话 id |
| `/usage` | 显示累计 token 用量 |
| `/save` | 显式保存当前会话 |
| `/history` | 列出输入历史（最近 20 条、最新在前、带编号） |
| `/!!` | 重跑最近一条 prompt |
| `/!N` | 重跑编号 N 的 prompt |
| `/exit` / `/quit` | 退出 |

## 文档

- [用户手册](docs/user-manual.md)：安装、配置与使用方式
- [配置文档](docs/configuration.md)：三层配置来源、config.toml 与环境变量使用说明
- [开发文档](docs/development.md)：架构、设计决策、构建/测试方法
- [说明文档](docs/explanation.md)：背景、概念与设计动机
- [ADR 目录](docs/adr/)：架构决策记录
