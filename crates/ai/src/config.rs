//! Provider configuration for the Bailian (阿里云百炼) OpenAI-compatible endpoint.
//!
//! Config comes from the environment (ticket 01 / grilling Q9):
//! - `DASHSCOPE_API_KEY` — API key (required).
//! - `SLIMCODE_AI_BASE_URL` — endpoint override; defaults to the China-station
//!   legacy compatible-mode URL.
//! - `SLIMCODE_AI_MODEL` — model override; defaults to `qwen-plus`.

use std::env;

/// Default China-station legacy compatible-mode base URL (no WorkspaceId needed).
pub const DEFAULT_BASE_URL: &str = "https://dashscope.aliyuncs.com/compatible-mode/v1";
/// Recommended default model (ticket 01).
pub const DEFAULT_MODEL: &str = "qwen-plus";

/// Resolved provider configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BailianConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
}

impl BailianConfig {
    /// Load from environment, applying defaults.
    pub fn from_env() -> Result<Self, String> {
        let api_key = env::var("DASHSCOPE_API_KEY").map_err(|_| {
            "DASHSCOPE_API_KEY is not set. Set it (e.g. export DASHSCOPE_API_KEY=sk-...)."
                .to_string()
        })?;
        let base_url =
            env::var("SLIMCODE_AI_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());
        let model = env::var("SLIMCODE_AI_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
        Ok(Self {
            api_key,
            base_url,
            model,
        })
    }

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
    fn defaults_apply_without_env() {
        // from_env fails without the key
        unsafe {
            env::set_var("DASHSCOPE_API_KEY", "sk-test");
        }
        let cfg = BailianConfig::from_env().unwrap();
        assert_eq!(cfg.base_url, DEFAULT_BASE_URL);
        assert_eq!(cfg.model, DEFAULT_MODEL);
        assert_eq!(cfg.api_key, "sk-test");
        unsafe {
            env::remove_var("DASHSCOPE_API_KEY");
        }
    }

    #[test]
    fn chat_completions_url_appends_path() {
        let cfg = BailianConfig::new("k", "https://x.example.com/compatible-mode/v1/", "m");
        assert_eq!(
            cfg.chat_completions_url(),
            "https://x.example.com/compatible-mode/v1/chat/completions"
        );
    }
}
