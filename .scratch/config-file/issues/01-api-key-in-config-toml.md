# 01: API key 进 config.toml（解析层）

**What to build:** `common::config` 解析层支持 API key 的第三种来源——`config.toml`
的 `[ai] api_key`，优先级 `--api-key > DASHSCOPE_API_KEY > [ai] api_key`，并上报
「key 来自文件」的标记供 CLI 打 chmod 提示。

**Blocked by:** None（可立即开工）

**Status:** resolved

- [ ] `FileConfig.ai` 增 `api_key: Option<String>`（serde optional，缺省容忍）。
- [ ] `Overrides` 增 `api_key: Option<String>`（`--api-key` 命令行覆盖，为 ticket 02
      预留；本 ticket 只改解析层）。
- [ ] `resolve` 核心：`api_key = overrides.api_key.or(env(ENV_API_KEY)).or(file.api_key)`；
      三者皆 `None` → 报错，措辞同时指向 `DASHSCOPE_API_KEY` 与
      `config.toml [ai] api_key` 两种来源（现有报错只提 env，需改写）。
- [ ] 解析结果携带 `ApiKeySource`（Cli | Env | File）或等价「来自文件」标记：
      内部 `resolve` 核心改返回 `(BailianConfig, ApiKeySource)`（或新增旁路函数），
      `load_from` / `load_with_overrides` 相应携带；唯一外部调用点
      `cli/main.rs:149` 跟随更新，TUI 不受影响。
- [ ] 单测（沿用 `env_of` / `resolve` 表驱动模式）：仅 file 有 key → 采用且
      `ApiKeySource::File`；env 压过 file → `Env`；override 压过 env/file → `Cli`；
      三者皆缺 → 报错且措辞含两种来源；空文件视为缺省（回归）；api_key 与
      base_url / model / cache 逐项独立回落。
- [ ] `cargo test` 全绿；`cargo fmt --all`；clippy 0 error / 0 warning。

## Answer

Implemented in `crates/common/src/config.rs`:

- `FileAi.api_key: Option<String>` (serde optional) + `Overrides.api_key` (`--api-key`).
- `ApiKeySource { Cli | Env | File }`; `resolve` now returns `(BailianConfig, ApiKeySource)`,
  `load_from` / `load_with_overrides` carry it. Only external call site
  (`cli/main.rs`) destructures; TUI unaffected (receives resolved `BailianConfig`).
- `api_key = overrides.api_key.or(env(ENV_API_KEY)).or(file.api_key)`; all three absent →
  error naming both `DASHSCOPE_API_KEY` and `config.toml [ai] api_key`.
- 6 new tests (file/env/override source, all-missing error, per-field independence);
  existing table-driven tests updated to the tuple return.
- Also added `ConfigAnswers` + `read_ai_fields` + `merge_config_toml` (pure merge core
  for ticket 03, living in the single config owner).

Review fix: empty / whitespace-only api key values (from `--api-key ""`, empty env, or
`[ai] api_key = ""`) are treated as absent → error like all-missing (spec: 三者皆缺时启动
报错), so an accidentally empty key never produces a silently broken run. Covered by
`api_key_empty_values_treated_as_missing` / `api_key_whitespace_env_treated_as_missing`.
