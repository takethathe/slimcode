//! Provider configuration for the Bailian (阿里云百炼) OpenAI-compatible endpoint.
//!
//! `BailianConfig` is pure provider data: api key, base URL and model. The
//! endpoint defaults (`DEFAULT_BASE_URL` / `DEFAULT_MODEL`) are owned here,
//! next to the provider that talks to that endpoint. The four-layer precedence
//! resolution (frontend overrides > env > `config.toml` > defaults) — along
//! with the env var names behind it — lives in `slimcode-app::config`, which
//! re-exports these defaults. This crate does not read env or files.

/// Default China-station legacy compatible-mode base URL (no WorkspaceId needed).
pub const DEFAULT_BASE_URL: &str = "https://dashscope.aliyuncs.com/compatible-mode/v1";
/// Recommended default model.
pub const DEFAULT_MODEL: &str = "qwen-plus";

/// Resolved provider configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BailianConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    /// Explicit context caching (Bailian `cache_control` marker on the system
    /// message). Default on; disable with `with_cache(false)` to keep request
    /// bytes byte-identical to a non-cache client.
    pub cache: bool,
}

impl BailianConfig {
    /// Construct explicitly (used by callers with their own config / tests).
    /// Cache is on by default — call [`BailianConfig::with_cache`] to disable.
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
        }
    }

    /// Builder-style setter for the explicit cache flag (keeps the 3-arg
    /// `new` signature stable so existing call sites do not break).
    pub fn with_cache(mut self, cache: bool) -> Self {
        self.cache = cache;
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
        let cfg = BailianConfig::new("k", "https://x.example.com/compatible-mode/v1/", "m");
        assert_eq!(
            cfg.chat_completions_url(),
            "https://x.example.com/compatible-mode/v1/chat/completions"
        );
    }

    #[test]
    fn new_defaults_cache_on() {
        let cfg = BailianConfig::new("k", "https://x.example.com/v1", "m");
        assert!(cfg.cache, "cache must default to on");
    }

    #[test]
    fn with_cache_sets_the_flag() {
        let cfg = BailianConfig::new("k", "https://x.example.com/v1", "m");
        assert!(cfg.clone().with_cache(true).cache);
        assert!(!cfg.clone().with_cache(false).cache);
    }
}
