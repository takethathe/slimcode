//! Provider configuration for the Bailian (阿里云百炼) OpenAI-compatible endpoint.
//!
//! `BailianConfig` is pure provider data: api key, base URL and model. The
//! four-layer precedence resolution (frontend overrides > env > `config.toml` >
//! defaults) — along with the env var names and default values behind it —
//! lives in `slimcode-common::config`, which is the single owner of those
//! constants. This crate does not re-read them.

/// Resolved provider configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BailianConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
}

impl BailianConfig {
    /// Construct explicitly (used by callers with their own config / tests).
    pub fn new(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: base_url.into(),
            model: model.into(),
        }
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
    fn chat_completions_url_appends_path() {
        let cfg = BailianConfig::new("k", "https://x.example.com/compatible-mode/v1/", "m");
        assert_eq!(
            cfg.chat_completions_url(),
            "https://x.example.com/compatible-mode/v1/chat/completions"
        );
    }
}
