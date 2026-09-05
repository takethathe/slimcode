# 01: cache config toggle

**What to build:** A `cache` switch that resolves through the four config layers and lands on the resolved provider config. A user who passes `--cache`, sets `SLIMCODE_AI_CACHE`, or writes `[ai] cache = true` gets a resolved config with cache on; otherwise cache stays off (default), so nothing pays for cache creation unless they opt in. A malformed env boolean fails at startup with a clear error rather than silently ignoring it.

**Blocked by:** None (can start immediately)

**Status:** resolved

- [x] `BailianConfig` carries a `cache: bool` (default `true` per user decision, overriding the spec's default-off) plus a `with_cache(bool)` builder; the existing 3-arg `new` keeps the default so no call site breaks.
- [x] Resolution order is CLI `--cache`/`--no-cache` > env `SLIMCODE_AI_CACHE` > `config.toml [ai] cache` > default `true`, resolved independently of `base_url` / `model` (per-field fallback).
- [x] The env var accepts `true`/`false`/`1`/`0`/`yes`/`no`/`on`/`off` (case-insensitive); any other value errors, naming the variable.
- [x] The one-shot CLI parses `--cache` / `--no-cache` flags and lists them in help output.
- [x] Unit tests cover: override > env > file > default; invalid env value; default-on when unset.

- [x] The full test suite passes; `cargo fmt --all` and clippy are clean.
