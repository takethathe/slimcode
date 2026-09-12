# 04: Configurable quota via `[sessions] max_mb`

**What to build:** The quota threshold becomes configurable: `config.toml`
gains a `[sessions] max_mb` key (u64, default 500, unit MiB), resolved through
the existing config layering (file > default; no env / CLI override). The
resolved value is converted to bytes and handed to the session store,
replacing the hardcoded default from ticket 03.

**Blocked by:** 03 (quota eviction on save)

**Status:** resolved

- [x] `[sessions] max_mb` parses from `config.toml`; absent key defaults to 500.
- [x] The resolved value flows from config into the eviction threshold
      (MiB → bytes conversion).
- [x] No env variable or CLI flag override is introduced.
- [x] Eviction behaviour from ticket 03 still holds with a custom `max_mb`.
- [x] Config tests cover parsing, defaulting, and invalid values.

## Notes

- `load_with_overrides` / `load_from` were replaced by a single `load_app_config`
  entry returning `AppConfig { provider, api_key_source, sessions_max_bytes }`,
  so no dead duplicate config entry point remains. A sync test pins
  `config::DEFAULT_MAX_MB` to `session::DEFAULT_MAX_BYTES`.
