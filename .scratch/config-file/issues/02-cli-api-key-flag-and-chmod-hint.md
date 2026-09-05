# 02: CLI `--api-key` 标志 + chmod 600 提示

**What to build:** CLI 层接入 ticket 01 的解析结果：(1) `parse_args` 支持
`--api-key <key>` 并填入 `Overrides.api_key`；(2) 加载配置后若 key 来自文件且
（Unix）`config.toml` 权限过宽，打印一条 `chmod 600` stderr 提示。

**Blocked by:** 01（依赖 `Overrides.api_key` 与 `ApiKeySource` 标记）

**Status:** resolved

- [ ] `parse_args` 支持 `--api-key`（紧跟值、缺值报错，与 `--model` 同款），填入
      `Overrides.api_key`；`CliArgs` 增 `api_key: Option<String>`。
- [ ] `usage()` 帮助文本增补 `--api-key <key>`；ENV 段说明 api key 优先级
      `--api-key > DASHSCOPE_API_KEY > config.toml [ai] api_key`。
- [ ] `run()` 在 `load_with_overrides` 之后：若 `ApiKeySource::File` 且（Unix）
      `config.toml` 权限 `mode & 0o077 != 0`，打印 `chmod 600 <path>` 提示到 stderr
      （one-shot 与 TUI 共用此打印点；非 Unix 跳过）。
- [ ] 单测：`parse_args` 读 `--api-key`、缺值报错；`--help` 列出 `--api-key`；
      Unix 下 0644 + key 来自文件 → 输出含 `chmod 600`；0600 → 不输出。
- [ ] `cargo test` 全绿；`cargo fmt --all`；clippy 0 error / 0 warning。

## Answer

Implemented in `crates/cli/src/main.rs`:

- `parse_args` handles `--api-key <key>` (missing value → error, same shape as `--model`),
  fills `CliArgs.api_key` → `Overrides.api_key`.
- `usage()` lists `--api-key <key>` and states the env precedence
  `--api-key > DASHSCOPE_API_KEY > config.toml [ai] api_key`.
- Pure `chmod_warning(path, ApiKeySource)` (Unix-only logic; non-Unix returns `None`):
  fires when `ApiKeySource::File` and `mode & 0o077 != 0`. `run()` prints it to stderr
  right after `load_with_overrides`, before any turn / the TUI alternate screen
  (one-shot and TUI share this print point).
- Tests: `--api-key` parse + missing-value error; `--help` lists it; chmod warning
  fires for 0644 + File source, silent for 0600, silent for Env/Cli source, silent
  when config missing. Verified live against the real binary (stderr hint on 0644,
  none on 0600 / env / `--api-key`).
