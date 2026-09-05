# 02: cache-hit usage parsing and accumulation

**What to build:** The endpoint's cache-hit usage — `usage.prompt_tokens_details.cached_tokens` and `usage.prompt_tokens_details.cache_creation_input_tokens` — is parsed from the final streaming chunk and accumulated into the provider's total usage. Callers can read cached/creation counts, getting `0` when the endpoint omits the details (no cache, unsupported model, or cache disabled), so the feature never errors on a missing field.

**Blocked by:** None (can start immediately)

**Status:** resolved

- [x] `TokenUsage` gains an optional nested `prompt_tokens_details` with `cached_tokens` and `cache_creation_input_tokens` (each defaults to `0` when absent; the whole block is optional).
- [x] `parse_stream` surfaces those fields from the final chunk's `usage` (already fetched via `stream_options.include_usage`).
- [x] `accumulate_usage` sums cached/creation tokens across samples, alongside the existing three fields.
- [x] Accessors `cached_tokens()` / `cache_creation_tokens()` return `0` when details are absent.
- [x] Existing `TokenUsage` struct literals are updated (`..Default::default()` or explicit `None`) so nothing breaks.
- [x] Tests: fixture parse with details present; usage without details yields `0` via accessors; `usage: null` still yields `None`; accumulation sums across multiple samples.

- [x] The full test suite passes; `cargo fmt --all` and clippy are clean.
