# slimcode 配置文档

> 本文件说明 slimcode 的配置方式：三层配置来源、每个配置项的取值规则与常见用法。
> 随代码变更同步维护。实现见 `crates/cli/src/config.rs` 与 `crates/ai/src/config.rs`。

## 配置总览

slimcode 的配置分三层，**逐项**按以下优先级解析（高 → 低）：

1. **环境变量**
2. **`config.toml` 文件**
3. **内置默认值**

三者共同决定最终配置：API key 只从环境变量读取；base URL 与 model 遵循
「环境变量 > 文件 > 默认值」的覆盖顺序。

## 配置项

| 配置项 | 来源 | 默认值 | 是否必填 |
| --- | --- | --- | --- |
| API key | 环境变量 `DASHSCOPE_API_KEY` | — | **必填** |
| base URL | 环境变量 `SLIMCODE_AI_BASE_URL` 或 `config.toml` 的 `[ai] base_url` | `https://dashscope.aliyuncs.com/compatible-mode/v1` | 否 |
| model | 环境变量 `SLIMCODE_AI_MODEL` 或 `config.toml` 的 `[ai] model` | `qwen-plus` | 否 |
| slimcode 家目录 | 环境变量 `SLIMCODE_HOME` | `~/.slimcode` | 否 |

### API key

API key 只从环境变量 `DASHSCOPE_API_KEY` 读取，**绝不落盘**（不写入
`config.toml`，也不从文件读取）。未设置时启动报错并给出提示：

```bash
export DASHSCOPE_API_KEY=sk-...
```

### 家目录

slimcode 家目录存放 `config.toml` 与会话文件（`sessions/`），默认是
`~/.slimcode`。可用 `SLIMCODE_HOME` 覆盖：

```bash
export SLIMCODE_HOME=/path/to/custom/slimcode
```

未设置 `SLIMCODE_HOME` 且 `$HOME` 为空时，slimcode 不加载 `config.toml`（文件
加载被跳过），base URL 与 model 直接使用默认值。

## config.toml 文件

### 位置

- 默认：`~/.slimcode/config.toml`
- 覆盖：`$SLIMCODE_HOME/config.toml`

### 格式

TOML 格式，只放**非敏感**覆盖项。`[ai]` 下的 `base_url` 与 `model` 均可选，
可只写其中一项：

```toml
[ai]
base_url = "https://dashscope.aliyuncs.com/compatible-mode/v1"
model = "qwen-plus"
```

### 解析规则

- 文件**可选**：不存在时静默跳过，直接使用环境变量与默认值。
- **逐项覆盖**：`base_url` 与 `model` 独立解析——文件只写了 `model` 时，
  `base_url` 回落到环境变量或默认值（不互相牵连）。
- **空文件**（全空白）视为不存在，跳过。
- **格式错误**（非法 TOML）：启动时报错，提示 `config.toml: …`。
- 文件里**不应**写 API key（该字段不会被读取）。

## 环境变量

| 变量 | 作用 | 优先级 |
| --- | --- | --- |
| `DASHSCOPE_API_KEY` | 百炼 API key（必填） | 唯一来源 |
| `SLIMCODE_AI_BASE_URL` | 覆盖端点 base URL | 高于 `config.toml` |
| `SLIMCODE_AI_MODEL` | 覆盖模型 id | 高于 `config.toml` |
| `SLIMCODE_HOME` | 覆盖 slimcode 家目录 | 高于默认 `~/.slimcode` |

`SLIMCODE_AI_BASE_URL` / `SLIMCODE_AI_MODEL` 若同时出现在环境变量与
`config.toml` 中，**环境变量优先**。

## 使用示例

### 只用环境变量（最简）

```bash
export DASHSCOPE_API_KEY=sk-...
slimcode "为 README 补一段简介"
```

### 用 config.toml 固定模型

```bash
export DASHSCOPE_API_KEY=sk-...
cat > ~/.slimcode/config.toml <<'EOF'
[ai]
model = "qwen-max"
EOF
slimcode "运行 cargo test 并修复失败用例"
```

### 临时覆盖文件配置

```bash
SLIMCODE_AI_MODEL=qwen-turbo slimcode "列出当前目录"
```

### 校验配置

配置错误会在启动时直接报错（例如缺失 API key、`config.toml` 非法）。
运行一条 prompt 观察输出即可确认模型/端点是否生效。
