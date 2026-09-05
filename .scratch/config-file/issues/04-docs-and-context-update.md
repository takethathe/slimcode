# 04: 文档 + CONTEXT.md 同步

**What to build:** 随 01–03 的行为变更同步全部受影响文档，改写与实现矛盾的
「绝不落盘」表述，并在 `CONTEXT.md` 增补术语（config 与 credential / API key 的
区分）。

**Blocked by:** 01, 02, 03（文档必须反映最终行为）

**Status:** resolved

- [ ] `docs/configuration.md`：新增 `[ai] api_key` 配置项与优先级
      `--api-key > DASHSCOPE_API_KEY > [ai] api_key`；改写「API key 绝不落盘」
      段落为「API key 可从 env 或 config.toml 读取，env 优先；文件含明文 key 时
      建议 chmod 600」；新增 `--api-key` 与 `slimcode config` 说明。
- [ ] `docs/user-manual.md`：env 表与示例同步（`[ai] api_key`、`--api-key`、
      `slimcode config` 用法）；改写「绝不落盘」表述。
- [ ] `docs/development.md`：config 解析变更（`FileConfig` / `Overrides` 新字段、
      `ApiKeySource`、`load_*` 返回形状）与 CLI 子命令。
- [ ] `CONTEXT.md`：增补词条——**Config**（四层解析产出的非敏感设置）、
      **config.toml**（用户配置文件）、**API key / credential**（秘密，来源优先级
      CLI > env > 文件，落盘需权限提示）；术语与 `docs/configuration.md` 一致。
- [ ] `docs/index.md` 如需（配置文档条目不变则跳过）。
- [ ] 校验：`cargo test` 全绿（文档不改代码，但提交前整套验收需通过）。

## Answer

- `docs/configuration.md`: `[ai] api_key` item + precedence `--api-key > DASHSCOPE_API_KEY
  > [ai] api_key`; rewritten API key section (env is higher-precedence secret source, file
  is a convenience fallback; chmod 600 hint; `slimcode config` auto-tightens); `--api-key`
  + `slimcode config` sections; env table priority updated; examples updated.
- `docs/user-manual.md`: API key bullet, `--api-key` / `slimcode config` usage, config.toml
  example with `api_key`, env table updated; 「绝不落盘」表述改写.
- `docs/development.md`: `common::config` bullet updated (`Overrides.api_key`,
  `FileAi.api_key`, `ApiKeySource`, tuple return of `resolve`/`load_*`,
  `read_ai_fields`/`merge_config_toml`); `crates/cli` section adds `--api-key`, the
  `slimcode config` subcommand, and the shared chmod print point.
- `CONTEXT.md`: new glossary entries **Config** / **config.toml** / **API key / credential**
  (config vs credential distinction, precedence, permission hint).
- `README.md` also updated (its 「绝不落盘」 statement contradicted the new behavior).
- `docs/index.md`: config-doc entry unchanged → skipped per ticket.
