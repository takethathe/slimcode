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

四者共同决定最终配置：API key 从命令行 `--api-key`、环境变量 `DASHSCOPE_API_KEY`
或 `config.toml` 的 `[ai] api_key` 三者之一读取（优先级见下）；base URL、model 与
上下文缓存（cache）遵循「命令行参数 > 环境变量 > 文件 > 默认值」的覆盖顺序。

## 配置项

| 配置项 | 来源 | 默认值 | 是否必填 |
| --- | --- | --- | --- |
| API key | 命令行 `--api-key` 或环境变量 `DASHSCOPE_API_KEY` 或 `config.toml` 的 `[ai] api_key` | — | **必填** |
| base URL | 命令行 `--base-url` 或环境变量 `SLIMCODE_AI_BASE_URL` 或 `config.toml` 的 `[ai] base_url` | `https://dashscope.aliyuncs.com/compatible-mode/v1` | 否 |
| model | 命令行 `--model` 或环境变量 `SLIMCODE_AI_MODEL` 或 `config.toml` 的 `[ai] model` | `qwen-plus` | 否 |
| 上下文缓存 | 命令行 `--cache` / `--no-cache` 或环境变量 `SLIMCODE_AI_CACHE` 或 `config.toml` 的 `[ai] cache` | **`true`（默认开启）** | 否 |
| 会话存储配额 | `config.toml` 的 `[sessions] max_mb` | `500`（MiB） | 否 |
| slimcode 家目录 | 环境变量 `SLIMCODE_HOME` | `~/.slimcode` | 否 |

### 上下文缓存（cache）

显式上下文缓存（Bailian `cache_control: {"type": "ephemeral"}` 标记）默认**开启**：
多轮提示中稳定的「system 提示 + 工具定义」前缀会交给端点缓存，命中后每轮输入
token 按缓存价计费（命中时只算少量）。关闭缓存后请求字节与不开启缓存的客户端
完全一致。缓存命中的 token 数与命中百分比（`cached / prompt`）会显示在运行结束的
token 用量汇总行中（`({cached} cached, {pct}%)`）。

### API key

API key 是**必填**的秘密（credential），来源优先级：

```text
--api-key > DASHSCOPE_API_KEY > config.toml [ai] api_key
```

即：命令行 `--api-key` 临时覆盖单次运行（不落盘）；环境变量 `DASHSCOPE_API_KEY`
次之；`config.toml` 的 `[ai] api_key` 是便捷回退（文件里只认字面量，不支持
`$ENV` / `!command` 插值）。与 base_url / model 同构的**逐项**解析，互不牵连。

三者皆缺时启动报错，措辞同时指向环境变量与 config.toml 两种来源。

```bash
export DASHSCOPE_API_KEY=sk-...
# 或写进 config.toml（见下）
```

**权限提示**：API key 写入 `config.toml` 时是明文落盘（对早前「key 绝不落盘」
决策的有意识反转——env 仍是更高优先级的秘密来源，文件是便捷回退）。若 key 来自
文件且（Unix）`config.toml` 存在 group/other 读权限（`mode & 0o077 != 0`），
slimcode 启动时会在 stderr 打印一条 `chmod 600` 提示；权限已收紧（0600）时不打扰。
`slimcode config` 写入 api_key 后会自动 chmod 600。

### 家目录

slimcode 家目录存放 `config.toml`、会话文件（`sessions/`）与 user 级 skill
（`skills/`），默认是 `~/.slimcode`。可用 `SLIMCODE_HOME` 覆盖：

```bash
export SLIMCODE_HOME=/path/to/custom/slimcode
```

未设置 `SLIMCODE_HOME` 且 `$HOME` 为空时，slimcode 不加载 `config.toml`（文件
加载被跳过），base URL 与 model 直接使用默认值。

### 会话存储配额（`[sessions] max_mb`）

`config.toml` 的 `[sessions]` 表只认一个键 `max_mb`：会话文件的总字节上限，
单位 MiB，默认 `500`。**没有**环境变量或命令行覆盖——只有文件与默认两层：

```toml
[sessions]
max_mb = 500
```

每次追加会话记录后，slimcode 统计 `sessions/` 下所有会话文件（各项目的 `.jsonl`
日志 + 遗留的旧 `.json` 整文件，含各项目子目录）的总字节数；
超过 `max_mb` 时按文件 mtime **从最旧**删除，直到总占用降到阈值的一半（默认 250 MiB），
并跳过当前正在使用的会话；删空的项目目录一并移除。清理是尽力而为的：失败不会影响会话。
启动时还会静默清理当前项目内重放不到任何 assistant 消息的 `.jsonl` 日志（0 字节、
日志头损坏、或只有头的崩溃残留）；遗留的 `.json` 只计入配额，不参与启动清理。
详见 [user-manual.md](./user-manual.md) 的「会话文件」节与
[ADR-0008](./adr/0008-project-scoped-sessions-with-quota-eviction.md)（配额机制）和
[ADR-0009](./adr/0009-appended-jsonl-session-log.md)（JSONL 日志格式）。

### Skill 目录（非 config.toml）

skill 不写入 `config.toml`，按 scope 从目录发现：

- **user scope**：`<家目录>/skills/`（随 `SLIMCODE_HOME` 走），跨项目共享；
- **project scope**：启动目录下的 `.slimcode/skills/`，仅当前项目，同名 skill 优先于 user。

发现为递归深搜：任意深度含 `SKILL.md` 的目录都是 skill（分类目录会被穿透），
slimcode 不内置 skill，两个目录默认不存在（视为空），首次 `/install-skill` 才创建；
skill 集完全由用户安装内容决定。详见 [user-manual.md](./user-manual.md) 的 Skills 节。

## 命令行参数

启动时可用 `--model`、`--base-url` 与 `--api-key` 临时覆盖模型、端点与 API key，
**优先于**环境变量、`config.toml` 与默认值；`--cache` / `--no-cache` 可临时开关
显式上下文缓存（默认开启，见上）：

```bash
slimcode --model qwen-max "为 README 补一段简介"
slimcode --model qwen-max --base-url https://my.example.com/v1 "列出当前目录"
slimcode --api-key sk-temp "运行一次使用临时 key"
slimcode --no-cache "运行 cargo test 并修复失败用例"
```

- 配置项均可选，可只写其中一个，其余回落到环境变量 / 文件 / 默认值。
- 参数需紧跟其值（如 `--model qwen-max`、`--api-key sk-...`）；缺少值时启动报错。
  `--cache` 与 `--no-cache` 是布尔开关，不需要值；后者用于显式关闭（默认开启的缓存）。
- 参数只作用于当次启动，不写入任何文件（`--api-key` 的一次性覆盖也不落盘）。

## config.toml 文件

### 位置

- 默认：`~/.slimcode/config.toml`
- 覆盖：`$SLIMCODE_HOME/config.toml`

### 格式

TOML 格式，`[ai]` 下的 `base_url`、`model`、`cache` 与 `api_key` 均可选，可只写
其中任意几项（`api_key` 是明文秘密，见上节权限提示）：

```toml
[ai]
base_url = "https://dashscope.aliyuncs.com/compatible-mode/v1"
model = "qwen-plus"
cache = false   # 可选：显式关闭上下文缓存（默认开启）
api_key = "sk-..."  # 可选：明文 key，写入后建议 chmod 600
```

### slimcode config 子命令

不想手写 TOML 时，可用 `slimcode config` 交互式补填：

```bash
slimcode config
```

- 读取现有 `config.toml`（若存在），对缺失/已有的 `model` / `base_url` / `api_key`
  逐项询问；已有值显示为默认值，回车保留，填入则覆盖。
- 只覆盖你填写的项，保留已有项（含 `cache`，不交互）——重复运行安全。
- 写回时按解析后的 TOML 文档重写：未触碰的表与键保留，文件里的注释会丢失。
- 写入 `api_key` 后（Unix）自动 `chmod 600` 并打印文件路径。
- 交互式命令需要终端（stdin/stdout 均为 TTY）；非 TTY 时报错。
- 不处理 `cache` 字段（保持手动编辑）。
- 子命令写入的是文件默认值；解析优先级不变（CLI / env 仍压过文件）。

### 解析规则

- 文件**可选**：不存在时静默跳过，直接使用环境变量与默认值。
- **逐项覆盖**：`base_url`、`model`、`cache` 与 `api_key` 独立解析——文件只写了
  `model` 时，其余各项各自回落到环境变量或默认值（不互相牵连）。
- **空文件**（全空白）视为不存在，跳过。
- **格式错误**（非法 TOML）：启动时报错，提示 `config.toml: …`。
- API key 优先级 `--api-key > DASHSCOPE_API_KEY > [ai] api_key`（与 base_url /
  model 同构的逐项解析；无默认值，三者皆缺时启动报错）。

## 环境变量

| 变量 | 作用 | 优先级 |
| --- | --- | --- |
| `DASHSCOPE_API_KEY` | 百炼 API key（必填） | 高于 `config.toml`、低于 `--api-key` |
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

### 用 config.toml 固定模型与 API key（一次配好，不再 export）

```bash
slimcode config
# 按提示填入 model / base_url / api_key；写入 api_key 后自动 chmod 600

# 等价的手写方式：
cat > ~/.slimcode/config.toml <<'EOF'
[ai]
model = "qwen-max"
api_key = "sk-..."
EOF
chmod 600 ~/.slimcode/config.toml
slimcode "运行 cargo test 并修复失败用例"
```

### 临时覆盖文件配置

```bash
SLIMCODE_AI_MODEL=qwen-turbo slimcode "列出当前目录"
slimcode --api-key sk-temp "单次运行使用临时 key"
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
`config.toml` 非法）。缺失 API key 的报错同时指向 `DASHSCOPE_API_KEY` 与
`config.toml [ai] api_key` 两种配置方式。运行一条 prompt 观察输出即可确认模型/端点
是否生效；运行结束的 token 用量汇总行里的 `({cached} cached)` 可确认缓存是否命中。
