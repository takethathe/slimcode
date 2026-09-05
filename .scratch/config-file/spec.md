# config 文件支持 — API key 进 config.toml + `slimcode config` 命令

Status: ready-for-agent

## Problem Statement

slimcode 的配置解析（`crates/common/src/config.rs`）已支持四层优先级
（命令行 > 环境变量 > config.toml > 默认值），`~/.slimcode/config.toml` 可配置
`[ai] base_url` / `[ai] model` / `[ai] cache`。但 **API key 只能从
`DASHSCOPE_API_KEY` 环境变量读取**——这是 Q9 grilling 刻意锁定的决策，文档明示
「API key 绝不落盘」。

用户希望 model / base url / api key 三个都能在 config 文件里一次配好，避免每次
都要 export；同时希望有一个写配置的入口，而不是手写 TOML。

按 grilling 推荐（用户确认）：
1. **(a)** 把 `api_key` 加进现有 `config.toml` 的 `[ai]`（与 model / base_url 同构）；
2. **(b)** 只做用户级全局配置，不做项目级；
3. **(c)** 新增极简 `slimcode config` 命令（交互式补填 + 写回 + chmod 600）。

## Solution

- **`[ai] api_key` 字段**：`FileConfig.ai` 新增 `api_key: Option<String>`，解析优先级
  `--api-key > DASHSCOPE_API_KEY > [ai] api_key`（与 base_url / model 同构的逐项
  解析；无默认值，三者皆缺时启动报错）。这是对「key 绝不落盘」决策的**有意识反转**：
  环境变量仍是更高优先级的秘密来源，文件是便捷回退。文件里只认字面量，不做
  pi 风格的 `$ENV` / `!command` 插值。
- **chmod 提示**：API key 解析结果来自文件时（Unix），检查 `config.toml` 权限位，
  存在 group/other 读权限（`mode & 0o077 != 0`）时打印一条 stderr 提示
  `chmod 600 ~/.slimcode/config.toml`；权限已收紧时不打扰。
- **`--api-key` 命令行标志**：`Overrides` 新增 `api_key: Option<String>`，CLI 解析
  `--api-key <key>`，帮助文本与 env 段同步更新。一次性覆盖，不落盘。
- **`slimcode config` 子命令**：`slimcode config`（第一个参数为 `config` 时特判为
  子命令，先于 prompt 解析）——读取现有 config.toml（若存在），对缺失的
  model / base_url / api_key 逐项交互式询问（已有值显示当前值作默认，回车保留），
  合并写回 config.toml；若写入了 api_key，Unix 上 chmod 600 并打印文件路径。
  不处理 cache（保持手动编辑）。
- **文档同步**：`docs/configuration.md` / `docs/user-manual.md` / `docs/development.md`
  新增 `[ai] api_key`、`--api-key`、`slimcode config`，改写「绝不落盘」表述；
  `CONTEXT.md` 增补词条（config 与 credential / API key 的区分）。

## User Stories

1. 作为 one-shot CLI 用户，我能在 `config.toml` 的 `[ai] api_key` 里写 API key，
   以便只配一次文件就能运行，不必每次 export。
2. 作为用户，api key 优先级为 `--api-key > DASHSCOPE_API_KEY > [ai] api_key`，
   以便临时覆盖或环境变量仍可压过文件，行为与 base_url / model 完全同构。
3. 作为用户，三者皆缺时启动报错并同时提示环境变量与 config.toml 两种配置方式，
   以便我知道怎么修。
4. 作为用户，key 来自文件且文件权限过宽时得到 `chmod 600` 提示，以便密钥不被
   同机其他用户读到。
5. 作为用户，我能用 `--api-key` 临时覆盖单次运行，以便多账户 / 多端点切换。
6. 作为用户，我能用 `slimcode config` 交互式补填 model / base_url / api_key，
   以便不手写 TOML。
7. 作为用户，`slimcode config` 只覆盖我填写的项、保留已有项，以便重复运行安全。
8. 作为用户，`slimcode config` 写入 api_key 后把文件 chmod 600，以便密钥默认受保护。
9. 作为开发者，config 解析仍是 `common::config` 单一 owner，以便 CLI / TUI 两端
   行为一致（TUI 接收已解析的 `BailianConfig`，不直接调用解析层）。

## Implementation Decisions

- **数据形状**：
  - `FileConfig.ai` 增 `api_key: Option<String>`（serde optional，缺省容忍）。
  - `Overrides` 增 `api_key: Option<String>`（`--api-key` 命令行覆盖）。
  - `resolve` 核心：`api_key = overrides.api_key.or(env(ENV_API_KEY)).or(file.api_key)`；
    三者皆 `None` → 报错，措辞同时指向 `DASHSCOPE_API_KEY` 环境变量与
    `config.toml [ai] api_key` 两种来源。
  - 解析结果需携带「api key 是否来自文件」的标记（`ApiKeySource: Cli | Env | File`），
    供 CLI 打 chmod 提示。具体形状实现时定：内部 `resolve` 核心改为返回
    `(BailianConfig, ApiKeySource)` 或新增旁路函数；`load_from` / `load_with_overrides`
    相应携带标记。约束：唯一外部调用点是 `cli/main.rs:149`，TUI 不受影响
    （`terminal.rs` 接收已解析 config）。
- **chmod 提示打印点**：`cli/main.rs` 在 `load_with_overrides` 之后一次性打印
  （one-shot 与 TUI 两模式共用，TUI 进 alternate screen 前已打过 stderr）。
  非 Unix 平台跳过检查、不打提示。
- **`--api-key`**：`parse_args` 支持，紧跟值，缺值报错（与 `--model` 同款）；
  `usage()` 帮助文本 + ENV 段增补。
- **`slimcode config` 子命令**：位置在 CLI crate；`run()` 入口特判
  `args.first() == Some("config")`（无其它标志）→ 进入子命令，不走 prompt 分发。
  交互：stdin/stdout 行输入；非 TTY 时报错提示（与 launch rule 一致的思路）。
  合并核心为纯函数（`merge(existing_toml, answers) -> String`），可单测；
  写回 + 权限收紧（Unix `chmod 600`）为薄 I/O 壳。回答为空 = 保留现有值；
  文件不存在 = 从空开始。
- **API key 来源与 `slimcode config` 的关系**：子命令写入的是「文件默认值」；
  解析优先级不变（CLI / env 仍压过文件）。

## Testing Decisions

- **解析层**（沿用 `common::config` 现有 `env_of` / `resolve` 表驱动模式）：
  - 仅 file 有 key → 采用且 `ApiKeySource::File`；
  - env 有 key 压过 file → 采用且 `ApiKeySource::Env`；
  - override 压过 env 与 file → 采用且 `ApiKeySource::Cli`；
  - 三者皆缺 → 报错，措辞含两种来源；
  - 空文件视为缺省（既有语义回归）；
  - `[ai] api_key` 与 base_url / model / cache 逐项独立回落（互不牵连）。
- **CLI**（沿用 `cli/main.rs` 现有 `parse_args` / `run_isolated` 模式）：
  - `parse_args` 读 `--api-key`，缺值报错；`--help` 列出；
  - Unix：config.toml 权限 0644 且 key 来自文件 → 输出含 `chmod 600`；
    权限 0600 → 不输出；
  - `slimcode config`：脚本 stdin 喂入回答 → 写回文件、保留已有项、仅覆盖填写项、
    含 api_key 时 chmod 600（Unix）；非 TTY 报错。
- **不新增 live 冒烟测试**（沿用既有 `#[ignore]` 模式）。
- **验收**：`cargo test` 全绿；`cargo fmt --all`；`cargo clippy --all-targets
  --all-features --message-format=json -- -D warnings` 0 error / 0 warning；
  文档同步。

## Out of Scope

- 项目级配置（`.slimcode/config.toml`）——留作后续 feature（Q2 决策：本轮只做用户级）。
- pi 风格的 `$ENV` / `!command` 插值写法——文件里只认字面量。
- `slimcode config` 对 `cache` 字段的交互——保持手动编辑。
- 密钥轮换 / 加密存储 / keychain / 凭据管理集成。
- 非 Unix 平台的文件权限管理（chmod 提示与子命令的权限收紧仅在 Unix 生效）。

## Further Notes

- **决策反转**：原始「key 永不落盘」是 grilling Q9 锁定 + 文档表述，未形成 ADR。
  本轮反转以本 spec + 文档改写记录，不新增 ADR（可逆；反转后文档与行为一致，
  无 surprise 残留）。
- **术语**（供 `CONTEXT.md` 增补，见 ticket 04）：config（非敏感设置：model /
  base_url / cache）与 credential（API key 这类秘密）分开定义；`[ai] api_key`
  字段属于 credential，落盘时提示权限。
- **对齐 pi**：pi 允许 config.toml 里的 `apiKey`（字面量 / `$ENV` / `!command`）
  并有 `--api-key` 标志；本轮取其字面量与标志部分，不做插值。
- 文档当前「绝不落盘」表述（`docs/configuration.md`、`docs/user-manual.md`）必须
  改写，否则与实现矛盾。
