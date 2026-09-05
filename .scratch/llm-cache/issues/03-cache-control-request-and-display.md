# 03: cache_control request emission + provider wiring + usage display

**What to build:** The full vertical path. With cache on, the system message is sent with a `cache_control` marker (its content becomes a one-block array) so the stable "system prompt + tool definitions" prefix is cached by the endpoint; the end-of-run one-shot summary and the TUI `/usage` line show the cached-token count. With cache off, request bytes are byte-for-byte identical to today, so the feature is a pure opt-in.

**Blocked by:** 01 (needs the resolved `cache` flag), 02 (needs the extended `TokenUsage`)

**Status:** resolved

- [x] `WireMessage.content` serializes as either a plain string or a list of content blocks; a block is `{"type":"text","text":...,"cache_control":{"type":"ephemeral"}}`.
- [x] `message_to_wire` takes a cache flag and emits `cache_control` only for the system message when cache is on and its text is non-empty; all existing semantics (assistant tool-call empty content, tool messages always carry content, empty text omitted) are preserved.
- [x] The provider passes its resolved `cache` flag into `message_to_wire` on every chat request.
- [x] The one-shot `render_usage` summary and the TUI usage line show the cached-token count (`{cached} cached`); both render sites are updated together so wording cannot drift.
- [x] Tests: `message_to_wire` emits the block array for system+cache and a plain string otherwise; existing wire tests updated for the new flag; cli render and TUI frame-buffer tests cover the cached count (and `0` when absent).

- [x] The full test suite passes; `cargo fmt --all` and clippy are clean.

## Comments

- 2026-08-30 (user request): 汇总行新增缓存命中百分比 —— `({cached} cached, {pct}%)`，
  `{pct}` = `cached / prompt`（保留一位小数、四舍五入，`0 / 0%` 当端点未回传或 prompt 为 0）。
  实现集中在 `common::render::usage_summary` / `cache_hit_percent`，cli 与 TUI 渲染点不变。
