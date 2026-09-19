//! Provider configuration, owned by `slimcode-ai`.
//!
//! `ProviderConfig` is pure provider data: api key, base URL, model and the
//! explicit-cache flag. The name is deliberately provider-agnostic: the app /
//! CLI layers must not know which concrete provider they talk to (ADR-0016),
//! and the same config shape rides the `Provider::chat` seam. The endpoint
//! defaults (`DEFAULT_BASE_URL` / `DEFAULT_MODEL`) are owned here, next to the
//! provider that talks to that endpoint. The four-layer precedence resolution
//! (frontend overrides > env > `config.toml` > defaults) — along with the env
//! var names behind it — lives in `slimcode-app::config`, which re-exports
//! these defaults. This crate does not read env or files.

/// Default China-station legacy compatible-mode base URL (no WorkspaceId needed).
pub const DEFAULT_BASE_URL: &str = "https://dashscope.aliyuncs.com/compatible-mode/v1";
/// Recommended default model.
pub const DEFAULT_MODEL: &str = "qwen-plus";

/// Resolved provider configuration (ADR-0016: the provider-owned settings a
/// runner hands across the `chat` seam — the provider instance itself stays
/// stateless).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    /// Explicit context caching (Bailian `cache_control` marker on the system
    /// and last conversation messages). Default on; disable with
    /// `with_cache(false)` to keep request bytes byte-identical to a
    /// non-cache client.
    pub cache: bool,
    /// Optional response cap for this request (the wire `max_tokens`). `None`
    /// omits the field entirely, so ordinary turns keep their byte shape;
    /// one-off requests (compaction summaries) set a conservative cap with
    /// [`ProviderConfig::with_max_tokens`].
    pub max_tokens: Option<u32>,
}

impl ProviderConfig {
    /// Construct explicitly (used by callers with their own config / tests).
    /// Cache is on by default and no response cap is set — call
    /// [`ProviderConfig::with_cache`] / [`ProviderConfig::with_max_tokens`] to
    /// override.
    pub fn new(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: base_url.into(),
            model: model.into(),
            cache: true,
            max_tokens: None,
        }
    }

    /// Builder-style setter for the explicit cache flag (keeps the 3-arg
    /// `new` signature stable so existing call sites do not break).
    pub fn with_cache(mut self, cache: bool) -> Self {
        self.cache = cache;
        self
    }

    /// Builder-style setter for the response cap (the wire `max_tokens`).
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    /// `{base_url}/chat/completions`.
    pub fn chat_completions_url(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_defaults_are_the_bailian_station() {
        assert_eq!(
            DEFAULT_BASE_URL,
            "https://dashscope.aliyuncs.com/compatible-mode/v1"
        );
        assert_eq!(DEFAULT_MODEL, "qwen-plus");
    }

    #[test]
    fn chat_completions_url_appends_path() {
        let cfg = ProviderConfig::new("k", "https://x.example.com/compatible-mode/v1/", "m");
        assert_eq!(
            cfg.chat_completions_url(),
            "https://x.example.com/compatible-mode/v1/chat/completions"
        );
    }

    #[test]
    fn new_defaults_cache_on() {
        let cfg = ProviderConfig::new("k", "https://x.example.com/v1", "m");
        assert!(cfg.cache, "cache must default to on");
    }

    #[test]
    fn with_cache_sets_the_flag() {
        let cfg = ProviderConfig::new("k", "https://x.example.com/v1", "m");
        assert!(cfg.clone().with_cache(true).cache);
        assert!(!cfg.clone().with_cache(false).cache);
    }

    #[test]
    fn max_tokens_defaults_to_unset_and_is_settable() {
        let cfg = ProviderConfig::new("k", "https://x.example.com/v1", "m");
        assert_eq!(cfg.max_tokens, None);
        assert_eq!(cfg.with_max_tokens(2048).max_tokens, Some(2048));
    }
}
