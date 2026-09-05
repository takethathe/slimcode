# slimcode 配置文档

> 本文件说明 slimcode 的配置方式：四层配置来源、每个配置项的取值规则与常见用法。
> 随代码变更同步维护。实现见 `crates/common/src/config.rs`（单一解析 owner）与
> `crates/ai/src/config.rs`（纯 provider 数据）。

## 配置总览

slimcode 的配置分四层，**逐项**按以下优先级解析（高 → 低）：

1. **命令行参数**
2. **环境变量**
3. **`config.toml` 文件**
4. **内置默认值**

四者共同决定最终配置：API key 只从环境变量读取；base URL、model 与上下文缓存
（cache）遵循「命令行参数 > 环境变量 > 文件 > 默认值」的覆盖顺序。

## 配置项

| 配置项 | 来源 | 默认值 | 是否必填 |
| --- | --- | --- | --- |
| API key | 环境变量 `DASHSCOPE_API_KEY` | — | **必填** |
| base URL | 命令行 `--base-url` 或环境变量 `SLIMCODE_AI_BASE_URL` 或 `config.toml` 的 `[ai] base_url` | `https://dashscope.aliyuncs.com/compatible-mode/v1` | 否 |
| model | 命令行 `--model` 或环境变量 `SLIMCODE_AI_MODEL` 或 `config.toml` 的 `[ai] model` | `qwen-plus` | 否 |
| 上下文缓存 | 命令行 `--cache` / `--no-cache` 或环境变量 `SLIMCODE_AI_CACHE` 或 `config.toml` 的 `[ai] cache` | **`true`（默认开启）** | 否 |
| slimcode 家目录 | 环境变量 `SLIMCODE_HOME` | `~/.slimcode` | 否 |

### 上下文缓存（cache）

显式上下文缓存（Bailian `cache_control: {"type": "ephemeral"}` 标记）默认**开启**：
多轮提示中稳定的「system 提示 + 工具定义」前缀会交给端点缓存，命中后每轮输入
token 按缓存价计费（命中时只算少量）。关闭缓存后请求字节与不开启缓存的客户端
完全一致。缓存命中的 token 数与命中百分比（`cached / prompt`）会显示在运行结束的
token 用量汇总行中（`({cached} cached, {pct}%)`）。

### API key

API key 只从环境变量 `DASHSCOPE_API_KEY` 读取，**绝不落盘**（不写入
`config.toml`，也不从文件读取）。未设置时启动报错并给出提示：

```bash
export DASHSCOPE_API_KEY=sk-...
```

### 家目录

slimcode 家目录存放 `config.toml`、会话文件（`sessions/`）与 user 级 skill
（`skills/`），默认是 `~/.slimcode`。可用 `SLIMCODE_HOME` 覆盖：

```bash
export SLIMCODE_HOME=/path/to/custom/slimcode
```

未设置 `SLIMCODE_HOME` 且 `$HOME` 为空时，slimcode 不加载 `config.toml`（文件
加载被跳过），base URL 与 model 直接使用默认值。

### Skill 目录（非 config.toml）

skill 不写入 `config.toml`，按 scope 从目录发现：

- **user scope**：`<家目录>/skills/`（随 `SLIMCODE_HOME` 走），跨项目共享；
- **project scope**：启动目录下的 `.slimcode/skills/`，仅当前项目，同名 skill 优先于 user。

发现为递归深搜：任意深度含 `SKILL.md` 的目录都是 skill（分类目录会被穿透），
slimcode 不内置 skill，两个目录默认不存在（视为空），首次 `/install-skill` 才创建；
skill 集完全由用户安装内容决定。详见 [user-manual.md](./user-manual.md) 的 Skills 节。

## 命令行参数

启动时可用 `--model` 与 `--base-url` 临时覆盖模型与端点，**优先于**环境变量、
`config.toml` 与默认值；`--cache` / `--no-cache` 可临时开关显式上下文缓存
（默认开启，见上）：

```bash
slimcode --model qwen-max "为 README 补一段简介"
slimcode --model qwen-max --base-url https://my.example.com/v1 "列出当前目录"
slimcode --no-cache "运行 cargo test 并修复失败用例"
```

- 三个配置项均可选，可只写其中一个，其余回落到环境变量 / 文件 / 默认值。
- 参数需紧跟其值（如 `--model qwen-max`）；缺少值时启动报错。`--cache` 与
  `--no-cache` 是布尔开关，不需要值；后者用于显式关闭（默认开启的缓存）。
- 参数只作用于当次启动，不写入任何文件。

## config.toml 文件

### 位置

- 默认：`~/.slimcode/config.toml`
- 覆盖：`$SLIMCODE_HOME/config.toml`

### 格式

TOML 格式，只放**非敏感**覆盖项。`[ai]` 下的 `base_url`、`model` 与 `cache`
均可选，可只写其中任意几项：

```toml
[ai]
base_url = "https://dashscope.aliyuncs.com/compatible-mode/v1"
model = "qwen-plus"
cache = false   # 可选：显式关闭上下文缓存（默认开启）
```

### 解析规则

- 文件**可选**：不存在时静默跳过，直接使用环境变量与默认值。
- **逐项覆盖**：`base_url`、`model` 与 `cache` 独立解析——文件只写了 `model` 时，
  `base_url` 与 `cache` 各自回落到环境变量或默认值（不互相牵连）。
- **空文件**（全空白）视为不存在，跳过。
- **格式错误**（非法 TOML）：启动时报错，提示 `config.toml: …`。
- 文件里**不应**写 API key（该字段不会被读取）。

## 环境变量

| 变量 | 作用 | 优先级 |
| --- | --- | --- |
| `DASHSCOPE_API_KEY` | 百炼 API key（必填） | 唯一来源 |
| `SLIMCODE_AI_BASE_URL` | 覆盖端点 base URL | 高于 `config.toml` |
| `SLIMCODE_AI_MODEL` | 覆盖模型 id | 高于 `config.toml` |
| `SLIMCODE_AI_CACHE` | 覆盖显式上下文缓存开关 | 高于 `config.toml` |
| `SLIMCODE_HOME` | 覆盖 slimcode 家目录 | 高于默认 `~/.slimcode` |

`SLIMCODE_AI_CACHE` 接受 `true` / `false` / `1` / `0` / `yes` / `no` / `on` / `off`
（大小写不敏感）；其它值启动报错并指明变量名（不静默忽略）。

`SLIMCODE_AI_BASE_URL` / `SLIMCODE_AI_MODEL` / `SLIMCODE_AI_CACHE` 若同时出现在
命令行、环境变量与 `config.toml` 中，**命令行参数优先，环境变量次之**。

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

### 命令行参数临时覆盖

```bash
slimcode --model qwen-max --base-url https://my.example.com/v1 "列出当前目录"
```

### 关闭上下文缓存（默认开启，按需关闭）

```bash
# 命令行临时关闭（默认开启）
slimcode --no-cache "运行 cargo test 并修复失败用例"

# 环境变量关闭
export SLIMCODE_AI_CACHE=false

# config.toml 关闭（会话默认生效）
cat > ~/.slimcode/config.toml <<'EOF'
[ai]
cache = false
EOF
```

### 校验配置

配置错误会在启动时直接报错（例如缺失 API key、`SLIMCODE_AI_CACHE` 非法、
`config.toml` 非法）。运行一条 prompt 观察输出即可确认模型/端点是否生效；
运行结束的 token 用量汇总行里的 `({cached} cached)` 可确认缓存是否命中。
