//! The `options_json` → [`AgentSessionOptions`] translation (spec D5).
//!
//! The JSON object keeps the ABI stable when options are added, and
//! `deny_unknown_fields` makes a typo (`api_ky`) a hard error instead of a
//! silent fall-through to the four-layer config. Everything the FFI cannot
//! express across the C ABI — provider / tools / skills injection — stays out
//! on purpose (spec D5): a host's only replaceable external dependency is the
//! HTTP endpoint, via `base_url`.

use std::path::PathBuf;

use serde::Deserialize;
use slimcode_sdk::AgentSessionOptions;

/// The options object `slimcode_session_create` accepts. Every field is
/// optional; a missing field falls through to the SDK's own default.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FfiOptions {
    /// Working directory the tools and project resolution bind to.
    pub cwd: Option<String>,
    /// Persist the session as a JSONL log; absent means `true` (ADR-0021 D3).
    pub persist: Option<bool>,
    /// Resume an existing persisted session by id (requires `persist: true`).
    pub session_id: Option<String>,
    /// Provider base URL override.
    pub base_url: Option<String>,
    /// Provider model override.
    pub model: Option<String>,
    /// Provider API key override.
    pub api_key: Option<String>,
    /// Explicit context-cache override.
    pub cache: Option<bool>,
    /// Pinned slimcode home directory (session store root, skills, context
    /// files); absent means the process-level `SLIMCODE_HOME` / user home.
    pub home: Option<String>,
}

impl FfiOptions {
    /// Parse the UTF-8 JSON object, mapping serde's error (bad syntax, wrong
    /// type, unknown field) to a human-readable message.
    pub fn parse(json: &str) -> Result<Self, String> {
        serde_json::from_str(json).map_err(|error| format!("invalid options JSON: {error}"))
    }

    /// Translate to SDK options: `persist` absent means `true`, exactly like
    /// the SDK / CLI default.
    pub fn into_agent_options(self) -> AgentSessionOptions {
        let mut options = AgentSessionOptions::new(self.cwd.map(PathBuf::from))
            .with_persist(self.persist.unwrap_or(true));
        options.session_id = self.session_id;
        options.base_url = self.base_url;
        options.model = self.model;
        options.api_key = self.api_key;
        options.cache = self.cache;
        options.home = self.home.map(PathBuf::from);
        options
    }
}
