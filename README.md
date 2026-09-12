# slimcode

一个用 Rust 编写的 AI coding agent CLI。输入一条 prompt，agent 会在目标目录内运行
`read` / `write` / `edit` / `bash` / `grep` / `find` / `ls` 七种工具循环完成任务，
支持流式输出、token 用量统计与可恢复的会话（追加式 JSONL 会话日志，ADR-0009）。

## 特性

- 单次非交互模式：`slimcode "为 README 补一段简介"`
- 交互式全屏 TUI：`/help`、`/new`、`/session`、`/usage`、`/history` 等命令，
  未知 `/` 命令给出预测提示（前缀匹配建议）
- 流式渲染 agent 输出，会话按消息逐条追加保存
- 基于 DashScope（百炼）OpenAI 兼容接口，默认使用 `qwen-plus` 模型
- 多行 prompt（TUI 内 `Shift+Enter` 换行）与跨运行输入历史
- 前端无关的命令注册表与预测逻辑（`slimcode-commands`）与公共应用模块（`slimcode-app`），可供其它前端复用

## 架构

cargo workspace，六个 crate，唯一二进制 `slimcode`；依赖单向、由测试断言
（`crates/cli/tests/architecture.rs`，见 ADR-0011）：

| crate | 包名 | 职责 |
| --- | --- | --- |
| `crates/ai` | `slimcode-ai` | LLM 层：wire `Message` / `Provider` / `ToolSpec` / `Delta` / `TokenUsage`（无 slimcode 依赖） |
| `crates/core` | `slimcode-core` | agent 运行时：`AgentEvent` / runner 循环 / `AgentMessage` / `Tool{spec,run}`（→ ai） |
| `crates/commands` | `slimcode-commands` | 纯命令注册表 + 模糊预测（无依赖） |
| `crates/app` | `slimcode-app` | 前端无关应用层：`DisplayItem`/`map_event`/`Renderer`/`run_turn`、上下文组装、会话与输入历史持久化、skills、七工具（→ ai, core, commands） |
| `crates/tui` | `slimcode-tui` | 终端图形库：`App` reducer + 帧循环 + `RenderItem`/`UiHandler` seam（无 slimcode 依赖） |
| `crates/cli` | `slimcode` | 唯一二进制 = 总入口：argv / 模式选择（one-shot 文本 vs 交互 TUI）/ 配置 / 服务构建 / 命令语义 / 会话落盘 / 两个显示适配器 |

## 安装

cargo workspace，从源码构建：

```bash
cargo build --release
```

产物为 `target/release/slimcode`，需要 Rust（edition 2024）与 cargo。

## 配置

配置分四层，优先级从高到低：命令行参数 > 环境变量 > `config.toml` > 默认值。

- **API key（必填）**：来源优先级 `--api-key` > `DASHSCOPE_API_KEY` > `[ai] api_key`。
  可用 `slimcode config` 交互式写进 `~/.slimcode/config.toml`（明文 key，写入后自动
  chmod 600；权限过宽时启动会提示 `chmod 600`）：

  ```bash
  export DASHSCOPE_API_KEY=sk-...   # 或
  slimcode config                   # 交互式把 model / base_url / api_key 写进 config.toml
  ```

- **命令行参数（可选，临时生效）**：`--model <model>` 覆盖模型 id，
  `--base-url <url>` 覆盖端点 base URL，`--api-key <key>` 临时覆盖 API key：

  ```bash
  slimcode --model qwen-max "为 README 补一段简介"
  ```

- **config.toml（可选）**：位于 `~/.slimcode/config.toml`（可用 `SLIMCODE_HOME`
  覆盖目录），`[ai]` 下 `base_url` / `model` / `cache` / `api_key` 均可选（
  `api_key` 是明文秘密，写入后建议 `chmod 600`）：

  ```toml
  [ai]
  base_url = "https://dashscope.aliyuncs.com/compatible-mode/v1"
  model = "qwen-plus"
  api_key = "sk-..."   # 可选：明文 key；权限过宽时启动会提示 chmod 600
  ```

- **环境变量覆盖**：

  | 变量 | 作用 | 默认 |
  | --- | --- | --- |
  | `DASHSCOPE_API_KEY` | 百炼 API key（必填；`--api-key` > env > `[ai] api_key`） | — |
  | `SLIMCODE_AI_BASE_URL` | 端点 base URL | `https://dashscope.aliyuncs.com/compatible-mode/v1` |
  | `SLIMCODE_AI_MODEL` | 模型 id | `qwen-plus` |
  | `SLIMCODE_HOME` | slimcode 家目录（含 `config.toml` 与 `sessions/`） | `~/.slimcode` |

## 使用

### 单次非交互模式

在目标目录内运行一条 prompt，完成后退出：

```bash
slimcode "为 README 补一段简介"
slimcode --cwd /path/to/repo "运行 cargo test 并修复失败用例"
slimcode --model qwen-max "为 README 补一段简介"
```

运行结束后打印 token 用量（一次性 CLI 不落盘会话）。

### 交互式 TUI

不带参数启动即进入全屏 TUI，会话以追加式 JSONL 日志逐条写入
`~/.slimcode/sessions/<project-key>/<id>.jsonl`（首个 assistant 消息出现时才
创建文件，之后每条进入历史的消息各占一行）：

```bash
slimcode
```

| 命令 | 作用 |
| --- | --- |
| `<prompt>` | 作为用户消息运行一轮 agent 循环（`Shift+Enter` 换行） |
| `/help` | 列出命令 |
| `/new` | 新建会话（清空消息、用量与屏幕） |
| `/session` | 打开全屏 session picker：列出**当前项目**已保存的会话，`↑`/`↓`/`PgUp`/`PgDn`（或滚轮）移动，`Enter` 载入，`Esc` 关闭 |
| `/usage` | 显示累计 token 用量 |
| `/history` | 列出输入历史（最近 20 条、最新在前、带编号） |
| `/!!` | 重跑最近一条 prompt |
| `/!N` | 重跑编号 N 的 prompt |
| `/exit` / `/quit` | 退出 |

输入未知的 `/` 命令时会给出**预测提示**：按已输入前缀匹配命令名或其别名
（如 `/his` → `did you mean: /history`），前缀为 `/` 时列出全部命令，完全无法
匹配时提示运行 `/help`。该提示来自前端无关的 `slimcode-commands` 注册表，
任何前端（当前 TUI、未来 Web）都能复用同一套命令定义与补全逻辑。

## 文档

- [用户手册](docs/user-manual.md)：安装、配置与使用方式
- [配置文档](docs/configuration.md)：四层配置来源、config.toml、环境变量与命令行参数使用说明
- [开发文档](docs/development.md)：架构、设计决策、构建/测试方法
- [说明文档](docs/explanation.md)：背景、概念与设计动机
- [ADR 目录](docs/adr/)：架构决策记录

## 许可证

[MIT](LICENSE)
