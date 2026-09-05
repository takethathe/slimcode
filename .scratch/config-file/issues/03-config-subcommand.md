# 03: `slimcode config` 子命令

**What to build:** 新增 `slimcode config` 子命令：交互式补填 model / base_url /
api_key（已有值显示当前值作默认，回车保留），合并写回 `config.toml`，写入
api_key 时（Unix）chmod 600 并打印文件路径。核心合并逻辑为纯函数，可单测。

**Blocked by:** 01（复用 `FileConfig` 文件形状与家目录解析）；与 02 同改
`cli/main.rs`，建议顺序执行。

**Status:** resolved

- [ ] `run()` 入口特判 `args.first() == Some("config")`（且无其它标志）→ 进入
      `slimcode config` 子命令，不走 prompt 分发。
- [ ] 纯函数合并核心：`merge(existing_toml: Option<&str>, answers) -> String`——
      读取现有 `[ai]`，对 model / base_url / api_key 逐项：填写了则覆盖、
      回答为空则保留现有值；文件不存在从空开始。不触碰 cache 字段。
- [ ] 交互壳（薄 I/O）：stdin/stdout 行输入逐项询问缺失项；非 TTY 时报错提示。
- [ ] 写回 + 权限：写入后若含 api_key，Unix 上 chmod 600，打印文件路径。
- [ ] 单测：脚本 stdin 喂入回答 → 写回文件、保留已有项、仅覆盖填写项；
      含 api_key 时 chmod 600（Unix）；非 TTY 报错；合并核心表驱动。
- [ ] `cargo test` 全绿；`cargo fmt --all`；clippy 0 error / 0 warning。

## Answer

Implemented in `crates/cli/src/config_cmd.rs` + `common::config` pure core:

- `run()` special-cases `args.first() == Some("config")` (after `--help`, before prompt
  parsing) → `config_cmd::entry(args, tty, stdin, out)`.
- Pure merge core `merge_config_toml(existing, answers)` + `read_ai_fields(existing)`
  in `common::config` (single owner of the file shape): filled fields overwrite,
  empty answers keep existing values, `cache` never touched; table-driven tests.
- Thin I/O shell `run_config(path, reader, writer)`: prompts model / base_url / api_key
  with current values as defaults (empty line = keep); writes merged TOML; on Unix
  chmod 600 when the resulting file holds an api_key; prints the file path; no-op when
  nothing is configured (no empty `[ai]` file created).
- Non-TTY (stdout or stdin not a terminal) and extra-argument runs error out.
- Tests: new-file write, keep-existing on empty answers, override-only-filled,
  chmod 600 on write / on preserved key, no chmod without key, absent-file no-op,
  non-TTY error, extra-args error. Verified live via a pty driver (answers align,
  perms 0600, existing fields preserved).

Review fixes:
- `merge_config_toml` rewritten on `toml::Value` (whole-document merge): untouched tables and
  unknown keys survive the write-back (only comments are lost in the TOML round-trip), so
  "保留已有项" holds beyond the known `[ai]` fields; verified live (cache + unknown key +
  `[other]` table preserved).
- `config_cmd::entry` now takes injected `tty` + `stdin_tty` so the non-TTY gates are
  unit-testable (`entry_rejects_non_tty_stdout` / `entry_rejects_piped_stdin`); `run()` passes
  `stdin().is_terminal()`.
