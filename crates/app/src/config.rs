//! Frontend-agnostic application configuration.
//!
//! `~/.slimcode/config.toml` holds overrides (`[ai] base_url` / `[ai] model` /
//! `[ai] cache` / `[ai] api_key`, plus `[sessions] max_mb` for the session
//! storage quota). Precedence is frontend overrides
//! (`--base-url` / `--model` / `--api-key`) > env (`SLIMCODE_AI_BASE_URL` /
//! `SLIMCODE_AI_MODEL` / `DASHSCOPE_API_KEY`) > config.toml > defaults (owned
//! here). The API key therefore has three sources (`--api-key` >
//! `DASHSCOPE_API_KEY` > `[ai] api_key`) — a conscious reversal of the earlier
//! "key never touches disk" decision: env stays a higher-precedence secret
//! source, the file is a convenience fallback. File values are literal only
//! (no `$ENV` / `!command` interpolation). The session quota is the exception
//! to the four-layer scheme: file > default, with no env or CLI override.
//!
//! This module is the single owner of that resolution and of the env var names
//! and defaults behind it: it produces an [`AppConfig`] (the provider config
//! plus the session quota), so the provider layer stays pure data and no other
//! crate re-parses the same env variables and defaults.

use serde::Deserialize;
use slimcode_ai::ProviderConfig;

// The endpoint defaults are owned by the provider layer (`slimcode-ai`); the
// app layer owns the resolution order and the env var names, and re-exports
// the defaults so callers keep using one spelling.
pub use slimcode_ai::config::{DEFAULT_BASE_URL, DEFAULT_MODEL};
/// Default session quota in MiB (spec: 500).
pub const DEFAULT_MAX_MB: u64 = 500;

/// Environment variable holding the API key (required).
pub const ENV_API_KEY: &str = "DASHSCOPE_API_KEY";
/// Environment variable overriding the endpoint base URL.
pub const ENV_BASE_URL: &str = "SLIMCODE_AI_BASE_URL";
/// Environment variable overriding the model.
pub const ENV_MODEL: &str = "SLIMCODE_AI_MODEL";
/// Environment variable overriding explicit context caching.
pub const ENV_CACHE: &str = "SLIMCODE_AI_CACHE";
/// Environment variable overriding the slimcode home directory.
pub const ENV_HOME: &str = "SLIMCODE_HOME";

/// Where the resolved API key came from — drives the CLI's `chmod 600` hint
/// when the key was read from `config.toml` (which stores it in plaintext).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApiKeySource {
    /// `--api-key` on the command line (one-shot override, never persisted).
    Cli,
    /// `DASHSCOPE_API_KEY` environment variable.
    Env,
    /// `config.toml` `[ai] api_key` (plaintext on disk).
    File,
}

/// Frontend-supplied overrides for the config fields. Highest precedence;
/// `None` means "not given on the command line", so resolution falls through
/// to env / file / default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Overrides {
    pub base_url: Option<String>,
    pub model: Option<String>,
    /// `--cache` / `--no-cache` on the command line; `None` falls through to
    /// env / file / default.
    pub cache: Option<bool>,
    /// `--api-key` on the command line; `None` falls through to env / file.
    pub api_key: Option<String>,
}

/// The editable `[ai]` fields `slimcode config` reads and writes. As "current
/// values" a `None` means the field is absent; as "answers" a `None` (or an
/// all-whitespace string) means "leave the existing value unchanged".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConfigAnswers {
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub api_key: Option<String>,
}

/// The `config.toml` file shape (serde-tolerant; unknown fields are ignored).
#[derive(Deserialize, Default)]
struct FileConfig {
    #[serde(default)]
    ai: FileAi,
    #[serde(default)]
    sessions: FileSessions,
}

#[derive(Deserialize, Default)]
struct FileAi {
    base_url: Option<String>,
    model: Option<String>,
    cache: Option<bool>,
    api_key: Option<String>,
}

#[derive(Deserialize, Default)]
struct FileSessions {
    max_mb: Option<u64>,
}

/// The slimcode home directory: `$SLIMCODE_HOME` if set, else `~/.slimcode`.
/// `None` only when `$HOME` is unset and no override was given.
pub fn slimcode_home() -> Option<std::path::PathBuf> {
    if let Some(h) = std::env::var_os(ENV_HOME) {
        return Some(std::path::PathBuf::from(h));
    }
    std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".slimcode"))
}

/// Resolve one config value: overrides > env > file > default.
fn pick_value(
    override_: Option<&str>,
    env: Option<String>,
    file: Option<&str>,
    default: &str,
) -> String {
    override_
        .map(str::to_string)
        .or(env)
        .or_else(|| file.map(str::to_string))
        .unwrap_or_else(|| default.to_string())
}

/// Parse a boolean env value (`true`/`false`/`1`/`0`/`yes`/`no`/`on`/`off`,
/// case-insensitive). Any other value is a startup error naming the variable
/// — never silently ignored.
fn parse_env_bool(var: &str, value: &str) -> Result<bool, String> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(format!(
            "{var} must be one of true/false/1/0/yes/no/on/off, got {value:?}"
        )),
    }
}

/// Pure resolution core: given optional file contents, an override set and an
/// env lookup, return the resolved provider config plus where the API key came
/// from. Separated from I/O for unit testing.
fn resolve(
    file_toml: Option<&str>,
    env: &dyn Fn(&str) -> Option<String>,
    overrides: &Overrides,
) -> Result<(ProviderConfig, ApiKeySource), String> {
    let file = match file_toml {
        Some(s) if !s.trim().is_empty() => {
            let parsed: FileConfig = toml::from_str(s).map_err(|e| format!("config.toml: {e}"))?;
            Some(parsed)
        }
        _ => None,
    };

    // API key: `--api-key` > `DASHSCOPE_API_KEY` > `[ai] api_key`. Empty /
    // whitespace-only values count as absent, so an accidentally-empty key
    // never slips through as a silently broken run. All three absent is a
    // startup error naming both the env var and the file field.
    let cli_key = overrides
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty());
    let env_key = env(ENV_API_KEY)
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty());
    let file_key = file
        .as_ref()
        .and_then(|f| f.ai.api_key.as_deref())
        .map(str::trim)
        .filter(|k| !k.is_empty());

    let (api_key, api_key_source) = if let Some(k) = cli_key {
        (k.to_string(), ApiKeySource::Cli)
    } else if let Some(k) = env_key {
        (k, ApiKeySource::Env)
    } else if let Some(k) = file_key {
        (k.to_string(), ApiKeySource::File)
    } else {
        return Err(format!(
            "no API key configured. Set {ENV_API_KEY} (e.g. export {ENV_API_KEY}=sk-...) \
             or add `[ai] api_key = \"sk-...\"` to config.toml"
        ));
    };

    let (file_base_url, file_model) = match &file {
        Some(f) => (f.ai.base_url.as_deref(), f.ai.model.as_deref()),
        None => (None, None),
    };

    let base_url = pick_value(
        overrides.base_url.as_deref(),
        env(ENV_BASE_URL),
        file_base_url,
        DEFAULT_BASE_URL,
    );
    let model = pick_value(
        overrides.model.as_deref(),
        env(ENV_MODEL),
        file_model,
        DEFAULT_MODEL,
    );

    // Cache resolves independently of base_url / model (per-field fallback):
    // CLI `--cache`/`--no-cache` > env > file > default on. An invalid env
    // value fails fast instead of being silently ignored.
    let cache = match overrides.cache {
        Some(v) => v,
        None => match env(ENV_CACHE) {
            Some(raw) => parse_env_bool(ENV_CACHE, &raw)?,
            None => file.as_ref().and_then(|f| f.ai.cache).unwrap_or(true),
        },
    };

    Ok((
        ProviderConfig::new(api_key, base_url, model).with_cache(cache),
        api_key_source,
    ))
}

/// Convert a MiB value to bytes (1024² per MiB).
pub fn max_mb_to_bytes(max_mb: u64) -> u64 {
    max_mb * 1024 * 1024
}

/// Resolve `[sessions] max_mb` from config.toml text: the file value or the
/// default (500). Tolerant of unparseable input (falls back to the default);
/// there is deliberately no env / CLI override for this knob. A non-integer
/// value fails the TOML parse, which the strict AI resolution path surfaces as
/// a startup error while this tolerant reader just defaults.
pub fn resolve_max_mb(file_toml: Option<&str>) -> u64 {
    file_toml
        .filter(|s| !s.trim().is_empty())
        .and_then(|s| toml::from_str::<FileConfig>(s).ok())
        .and_then(|f| f.sessions.max_mb)
        .unwrap_or(DEFAULT_MAX_MB)
}

/// The resolved, non-secret application settings handed to frontends: the AI
/// provider config plus the session storage quota in bytes.
pub struct AppConfig {
    pub provider: ProviderConfig,
    pub api_key_source: ApiKeySource,
    pub sessions_max_bytes: u64,
}

/// Load config from the real environment and config file, applying frontend
/// overrides on top, plus the session storage quota.
pub fn load_app_config(overrides: Overrides) -> Result<AppConfig, String> {
    let path = slimcode_home().map(|h| h.join("config.toml"));
    let file_toml = match &path {
        Some(p) if p.exists() => {
            Some(std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?)
        }
        _ => None,
    };
    let (provider, api_key_source) =
        resolve(file_toml.as_deref(), &|k| std::env::var(k).ok(), &overrides)?;
    let sessions_max_bytes = max_mb_to_bytes(resolve_max_mb(file_toml.as_deref()));
    Ok(AppConfig {
        provider,
        api_key_source,
        sessions_max_bytes,
    })
}

/// Read the current editable `[ai]` values from existing `config.toml` text
/// (for the interactive `slimcode config` defaults). Tolerant: unparseable
/// input yields all `None` — the subsequent [`merge_config_toml`] surfaces the
/// parse error, so the user is not prompted against garbage.
pub fn read_ai_fields(existing: Option<&str>) -> ConfigAnswers {
    let file = existing
        .filter(|s| !s.trim().is_empty())
        .and_then(|s| toml::from_str::<FileConfig>(s).ok());
    match file {
        Some(f) => ConfigAnswers {
            base_url: f.ai.base_url,
            model: f.ai.model,
            api_key: f.ai.api_key,
        },
        None => ConfigAnswers::default(),
    }
}

/// Merge existing `config.toml` text with the fields the user filled in —
/// the pure core of `slimcode config`. A non-empty answer replaces the field;
/// an empty (or absent) answer keeps the existing value (or leaves the field
/// absent). `cache` is never touched. Operates on the full TOML document, so
/// untouched tables/keys are preserved (only comments are lost, inherent to a
/// TOML round-trip). Malformed input is a startup-style error (`config.toml: …`).
pub fn merge_config_toml(
    existing: Option<&str>,
    answers: &ConfigAnswers,
) -> Result<String, String> {
    let mut root: toml::Value = match existing {
        Some(s) if !s.trim().is_empty() => {
            toml::from_str(s).map_err(|e| format!("config.toml: {e}"))?
        }
        _ => toml::Value::Table(Default::default()),
    };

    // Non-empty answers only: an empty answer leaves the field untouched.
    let edits: Vec<(&str, String)> = [
        ("base_url", &answers.base_url),
        ("model", &answers.model),
        ("api_key", &answers.api_key),
    ]
    .into_iter()
    .filter_map(|(key, answer)| {
        answer
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(|t| (key, t.to_string()))
    })
    .collect();

    // Only touch `[ai]` when there is something to write, so an all-empty
    // merge leaves the document (or absence thereof) untouched.
    if !edits.is_empty() {
        let root_table = root
            .as_table_mut()
            .ok_or_else(|| "config.toml: top level must be a table".to_string())?;
        let ai = root_table
            .entry("ai")
            .or_insert_with(|| toml::Value::Table(Default::default()));
        let ai = ai
            .as_table_mut()
            .ok_or_else(|| "config.toml: `[ai]` must be a table".to_string())?;
        for (key, value) in edits {
            ai.insert(key.to_string(), toml::Value::String(value));
        }
    }

    toml::to_string(&root).map_err(|e| format!("config.toml: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an env lookup from a slice of (key, value) pairs.
    fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k: &str| {
            pairs
                .iter()
                .find(|(pk, _)| *pk == k)
                .map(|(_, v)| v.to_string())
        }
    }

    /// Resolve and keep just the config (the source is asserted separately).
    fn resolve_cfg(
        file: Option<&str>,
        env: &dyn Fn(&str) -> Option<String>,
        overrides: &Overrides,
    ) -> ProviderConfig {
        resolve(file, env, overrides).unwrap().0
    }

    #[test]
    fn defaults_apply_when_no_file_or_env_overrides() {
        let env = env_of(&[(ENV_API_KEY, "sk-test")]);
        let cfg = resolve_cfg(None, &env, &Overrides::default());
        assert_eq!(cfg.api_key, "sk-test");
        assert_eq!(cfg.base_url, DEFAULT_BASE_URL);
        assert_eq!(cfg.model, DEFAULT_MODEL);
    }

    #[test]
    fn file_overrides_defaults() {
        let env = env_of(&[(ENV_API_KEY, "sk-test")]);
        let file = "[ai]\nbase_url = \"https://file.example.com/v1\"\nmodel = \"file-model\"\n";
        let cfg = resolve_cfg(Some(file), &env, &Overrides::default());
        assert_eq!(cfg.base_url, "https://file.example.com/v1");
        assert_eq!(cfg.model, "file-model");
    }

    #[test]
    fn env_overrides_file() {
        let env = env_of(&[
            (ENV_API_KEY, "sk-test"),
            (ENV_BASE_URL, "https://env.example.com/v1"),
            (ENV_MODEL, "env-model"),
        ]);
        let file = "[ai]\nbase_url = \"https://file.example.com/v1\"\nmodel = \"file-model\"\n";
        let cfg = resolve_cfg(Some(file), &env, &Overrides::default());
        assert_eq!(cfg.base_url, "https://env.example.com/v1");
        assert_eq!(cfg.model, "env-model");
    }

    #[test]
    fn partial_file_falls_back_per_field() {
        let env = env_of(&[(ENV_API_KEY, "sk-test")]);
        let file = "[ai]\nmodel = \"file-model\"\n"; // no base_url
        let cfg = resolve_cfg(Some(file), &env, &Overrides::default());
        assert_eq!(cfg.base_url, DEFAULT_BASE_URL); // default
        assert_eq!(cfg.model, "file-model"); // file
    }

    #[test]
    fn malformed_toml_errors() {
        let env = env_of(&[(ENV_API_KEY, "sk-test")]);
        let err = resolve(Some("[ai\n base_url = bad"), &env, &Overrides::default()).unwrap_err();
        assert!(err.contains("config.toml"), "err: {err}");
    }

    #[test]
    fn missing_api_key_errors_naming_both_sources() {
        let env = env_of(&[]);
        let err = resolve(None, &env, &Overrides::default()).unwrap_err();
        assert!(err.contains(ENV_API_KEY), "err: {err}");
        assert!(err.contains("config.toml"), "err: {err}");
        assert!(err.contains("api_key"), "err: {err}");
    }

    #[test]
    fn empty_file_is_treated_as_absent() {
        let env = env_of(&[(ENV_API_KEY, "sk-test")]);
        let cfg = resolve_cfg(Some("\n  \n"), &env, &Overrides::default());
        assert_eq!(cfg.base_url, DEFAULT_BASE_URL);
        assert_eq!(cfg.model, DEFAULT_MODEL);
    }

    #[test]
    fn overrides_env_and_file() {
        let env = env_of(&[
            (ENV_API_KEY, "sk-test"),
            (ENV_BASE_URL, "https://env.example.com/v1"),
            (ENV_MODEL, "env-model"),
        ]);
        let file = "[ai]\nbase_url = \"https://file.example.com/v1\"\nmodel = \"file-model\"\n";
        let overrides = Overrides {
            base_url: Some("https://cli.example.com/v1".to_string()),
            model: Some("cli-model".to_string()),
            cache: None,
            api_key: None,
        };
        let cfg = resolve_cfg(Some(file), &env, &overrides);
        assert_eq!(cfg.base_url, "https://cli.example.com/v1");
        assert_eq!(cfg.model, "cli-model");
    }

    #[test]
    fn overrides_each_field_independently() {
        let env = env_of(&[(ENV_API_KEY, "sk-test")]);
        let overrides = Overrides {
            base_url: None,
            model: Some("cli-model".to_string()),
            cache: None,
            api_key: None,
        };
        let cfg = resolve_cfg(None, &env, &overrides);
        assert_eq!(cfg.model, "cli-model");
        assert_eq!(cfg.base_url, DEFAULT_BASE_URL);
    }

    // --- api key from config.toml (config-file tickets 01-03) -------------

    #[test]
    fn api_key_from_file_is_used_with_file_source() {
        let env = env_of(&[]); // no env key
        let file = "[ai]\napi_key = \"sk-file\"\n";
        let (cfg, source) = resolve(Some(file), &env, &Overrides::default()).unwrap();
        assert_eq!(cfg.api_key, "sk-file");
        assert_eq!(source, ApiKeySource::File);
    }

    #[test]
    fn api_key_env_beats_file() {
        let env = env_of(&[(ENV_API_KEY, "sk-env")]);
        let file = "[ai]\napi_key = \"sk-file\"\n";
        let (cfg, source) = resolve(Some(file), &env, &Overrides::default()).unwrap();
        assert_eq!(cfg.api_key, "sk-env");
        assert_eq!(source, ApiKeySource::Env);
    }

    #[test]
    fn api_key_override_beats_env_and_file() {
        let env = env_of(&[(ENV_API_KEY, "sk-env")]);
        let file = "[ai]\napi_key = \"sk-file\"\n";
        let overrides = Overrides {
            base_url: None,
            model: None,
            cache: None,
            api_key: Some("sk-cli".to_string()),
        };
        let (cfg, source) = resolve(Some(file), &env, &overrides).unwrap();
        assert_eq!(cfg.api_key, "sk-cli");
        assert_eq!(source, ApiKeySource::Cli);
    }

    #[test]
    fn api_key_resolves_independently_of_other_fields() {
        // api_key from file, everything else falls back per-field.
        let env = env_of(&[]);
        let file = "[ai]\napi_key = \"sk-file\"\n"; // no base_url / model / cache
        let (cfg, source) = resolve(Some(file), &env, &Overrides::default()).unwrap();
        assert_eq!(cfg.api_key, "sk-file");
        assert_eq!(source, ApiKeySource::File);
        assert_eq!(cfg.base_url, DEFAULT_BASE_URL);
        assert_eq!(cfg.model, DEFAULT_MODEL);
        assert!(cfg.cache);
    }

    #[test]
    fn file_api_key_beats_missing_env_when_others_present() {
        // base_url / model from env, api_key only from file: per-field fallback
        // must not couple them.
        let env = env_of(&[
            (ENV_BASE_URL, "https://env.example.com/v1"),
            (ENV_MODEL, "env-model"),
        ]);
        let file = "[ai]\napi_key = \"sk-file\"\n";
        let (cfg, source) = resolve(Some(file), &env, &Overrides::default()).unwrap();
        assert_eq!(cfg.api_key, "sk-file");
        assert_eq!(source, ApiKeySource::File);
        assert_eq!(cfg.base_url, "https://env.example.com/v1");
        assert_eq!(cfg.model, "env-model");
    }

    #[test]
    fn api_key_empty_values_treated_as_missing() {
        // An empty `--api-key` or `[ai] api_key` must not produce a silently
        // broken empty key: it counts as absent and errors like all-missing.
        let env = env_of(&[]);
        let overrides = Overrides {
            base_url: None,
            model: None,
            cache: None,
            api_key: Some(String::new()),
        };
        let err = resolve(None, &env, &overrides).unwrap_err();
        assert!(err.contains(ENV_API_KEY), "err: {err}");
        assert!(err.contains("config.toml"), "err: {err}");

        let err = resolve(Some("[ai]\napi_key = \"\"\n"), &env, &Overrides::default()).unwrap_err();
        assert!(err.contains(ENV_API_KEY), "err: {err}");
    }

    #[test]
    fn api_key_whitespace_env_treated_as_missing() {
        let env = env_of(&[(ENV_API_KEY, "   ")]);
        let err = resolve(None, &env, &Overrides::default()).unwrap_err();
        assert!(err.contains(ENV_API_KEY), "err: {err}");
    }

    // --- cache flag (llm-cache tickets 01/03) -----------------------------

    #[test]
    fn cache_defaults_on() {
        let env = env_of(&[(ENV_API_KEY, "sk-test")]);
        let cfg = resolve_cfg(None, &env, &Overrides::default());
        assert!(cfg.cache, "cache must default to on");
    }

    #[test]
    fn cache_file_true_enables() {
        let env = env_of(&[(ENV_API_KEY, "sk-test")]);
        let file = "[ai]\ncache = true\n";
        let cfg = resolve_cfg(Some(file), &env, &Overrides::default());
        assert!(cfg.cache);
    }

    #[test]
    fn cache_env_overrides_file() {
        let env = env_of(&[(ENV_API_KEY, "sk-test"), (ENV_CACHE, "true")]);
        let file = "[ai]\ncache = false\n";
        let cfg = resolve_cfg(Some(file), &env, &Overrides::default());
        assert!(cfg.cache, "env true must beat file false");
    }

    #[test]
    fn cache_env_false_overrides_file_true() {
        let env = env_of(&[(ENV_API_KEY, "sk-test"), (ENV_CACHE, "false")]);
        let file = "[ai]\ncache = true\n";
        let cfg = resolve_cfg(Some(file), &env, &Overrides::default());
        assert!(!cfg.cache, "env false must beat file true");
    }

    #[test]
    fn cache_override_beats_env() {
        let env = env_of(&[(ENV_API_KEY, "sk-test"), (ENV_CACHE, "false")]);
        let overrides = Overrides {
            base_url: None,
            model: None,
            cache: Some(true),
            api_key: None,
        };
        let cfg = resolve_cfg(None, &env, &overrides);
        assert!(cfg.cache, "CLI --cache must beat env false");
    }

    #[test]
    fn cache_resolves_independently_of_other_fields() {
        let env = env_of(&[(ENV_API_KEY, "sk-test")]);
        let file = "[ai]\ncache = true\n"; // no base_url / model
        let cfg = resolve_cfg(Some(file), &env, &Overrides::default());
        assert!(cfg.cache);
        assert_eq!(cfg.base_url, DEFAULT_BASE_URL); // fell back per-field
        assert_eq!(cfg.model, DEFAULT_MODEL);
    }

    #[test]
    fn cache_override_false_beats_env_true() {
        // `--no-cache` on the command line must be able to turn caching off
        // even when the env var enables it.
        let env = env_of(&[(ENV_API_KEY, "sk-test"), (ENV_CACHE, "true")]);
        let overrides = Overrides {
            base_url: None,
            model: None,
            cache: Some(false),
            api_key: None,
        };
        let cfg = resolve_cfg(None, &env, &overrides);
        assert!(!cfg.cache, "CLI --no-cache must beat env true");
    }

    #[test]
    fn cache_env_accepts_all_boolean_spellings() {
        for (raw, expect) in [
            ("true", true),
            ("TRUE", true),
            ("1", true),
            ("yes", true),
            ("on", true),
            ("false", false),
            ("FALSE", false),
            ("0", false),
            ("no", false),
            ("off", false),
        ] {
            let pairs = [(ENV_API_KEY, "sk-test"), (ENV_CACHE, raw)];
            let env = env_of(&pairs);
            let cfg = resolve_cfg(None, &env, &Overrides::default());
            assert_eq!(cfg.cache, expect, "SLIMCODE_AI_CACHE={raw:?}");
        }
    }

    #[test]
    fn cache_invalid_env_errors_naming_variable() {
        let env = env_of(&[(ENV_API_KEY, "sk-test"), (ENV_CACHE, "banana")]);
        let err = resolve(None, &env, &Overrides::default()).unwrap_err();
        assert!(
            err.contains(ENV_CACHE),
            "error must name the variable: {err}"
        );
    }

    // --- read_ai_fields / merge_config_toml (config-file ticket 03) -------

    #[test]
    fn read_ai_fields_returns_present_values_only() {
        let fields = read_ai_fields(Some("[ai]\nmodel = \"m\"\napi_key = \"k\"\n"));
        assert_eq!(fields.model.as_deref(), Some("m"));
        assert_eq!(fields.api_key.as_deref(), Some("k"));
        assert_eq!(fields.base_url, None);
    }

    #[test]
    fn read_ai_fields_treats_empty_as_absent() {
        let fields = read_ai_fields(Some("\n  \n"));
        assert_eq!(fields, ConfigAnswers::default());
        let fields = read_ai_fields(None);
        assert_eq!(fields, ConfigAnswers::default());
    }

    #[test]
    fn read_ai_fields_tolerates_malformed_input() {
        // Tolerant: defaults; merge later surfaces the parse error.
        let fields = read_ai_fields(Some("[ai\n base_url = bad"));
        assert_eq!(fields, ConfigAnswers::default());
    }

    #[test]
    fn merge_from_empty_sets_only_answered_fields() {
        let answers = ConfigAnswers {
            base_url: None,
            model: Some("qwen-max".to_string()),
            api_key: Some("sk-new".to_string()),
        };
        let out = merge_config_toml(None, &answers).unwrap();
        assert!(out.contains("model = \"qwen-max\""), "out: {out}");
        assert!(out.contains("api_key = \"sk-new\""), "out: {out}");
        assert!(!out.contains("base_url"), "out: {out}");
        assert!(!out.contains("cache"), "cache must not be touched: {out}");
    }

    #[test]
    fn merge_keeps_existing_values_for_empty_answers() {
        let existing = "[ai]\nbase_url = \"https://existing.example.com/v1\"\nmodel = \"m1\"\ncache = false\napi_key = \"sk-old\"\n";
        let answers = ConfigAnswers::default(); // user pressed enter everywhere
        let out = merge_config_toml(Some(existing), &answers).unwrap();
        assert!(
            out.contains("base_url = \"https://existing.example.com/v1\""),
            "out: {out}"
        );
        assert!(out.contains("model = \"m1\""), "out: {out}");
        assert!(out.contains("api_key = \"sk-old\""), "out: {out}");
        assert!(
            out.contains("cache = false"),
            "cache must be preserved: {out}"
        );
    }

    #[test]
    fn merge_overrides_only_filled_fields_and_preserves_others() {
        let existing =
            "[ai]\nbase_url = \"https://existing.example.com/v1\"\nmodel = \"m1\"\ncache = false\n";
        let answers = ConfigAnswers {
            base_url: None,
            model: Some("m2".to_string()),
            api_key: Some("sk-new".to_string()),
        };
        let out = merge_config_toml(Some(existing), &answers).unwrap();
        assert!(
            out.contains("base_url = \"https://existing.example.com/v1\""),
            "out: {out}"
        );
        assert!(out.contains("model = \"m2\""), "out: {out}");
        assert!(out.contains("api_key = \"sk-new\""), "out: {out}");
        assert!(
            out.contains("cache = false"),
            "cache must be preserved: {out}"
        );
    }

    #[test]
    fn merge_does_not_touch_cache_when_absent_from_file() {
        // No cache in the file and cache not answered → must stay absent.
        let existing = "[ai]\nmodel = \"m1\"\n";
        let answers = ConfigAnswers {
            base_url: Some("https://new.example.com/v1".to_string()),
            model: None,
            api_key: None,
        };
        let out = merge_config_toml(Some(existing), &answers).unwrap();
        assert!(!out.contains("cache"), "cache must not appear: {out}");
    }

    #[test]
    fn merge_whitespace_answer_keeps_existing() {
        let existing = "[ai]\nmodel = \"m1\"\n";
        let answers = ConfigAnswers {
            base_url: None,
            model: Some("   ".to_string()), // all-whitespace answer = keep
            api_key: None,
        };
        let out = merge_config_toml(Some(existing), &answers).unwrap();
        assert!(out.contains("model = \"m1\""), "out: {out}");
    }

    #[test]
    fn merge_trims_answers() {
        let answers = ConfigAnswers {
            base_url: None,
            model: Some("  qwen-turbo  ".to_string()),
            api_key: Some("  sk-x  ".to_string()),
        };
        let out = merge_config_toml(None, &answers).unwrap();
        assert!(out.contains("model = \"qwen-turbo\""), "out: {out}");
        assert!(out.contains("api_key = \"sk-x\""), "out: {out}");
    }

    #[test]
    fn merge_malformed_existing_errors() {
        let err =
            merge_config_toml(Some("[ai\n base_url = bad"), &ConfigAnswers::default()).unwrap_err();
        assert!(err.contains("config.toml"), "err: {err}");
    }

    #[test]
    fn merge_preserves_unknown_tables_and_keys() {
        // Merge operates on the full document: untouched tables / unknown
        // keys survive the round-trip (only comments are lost).
        let existing = "[ai]\nmodel = \"m1\"\nfuture_opt = 42\n\n[other]\nkeep = \"yes\"\n";
        let answers = ConfigAnswers {
            base_url: None,
            model: Some("m2".to_string()),
            api_key: None,
        };
        let out = merge_config_toml(Some(existing), &answers).unwrap();
        assert!(out.contains("model = \"m2\""), "out: {out}");
        assert!(
            out.contains("future_opt = 42"),
            "unknown [ai] key lost: {out}"
        );
        assert!(out.contains("[other]"), "non-[ai] table lost: {out}");
        assert!(
            out.contains("keep = \"yes\""),
            "other-table key lost: {out}"
        );
    }

    #[test]
    fn merge_empty_document_stays_empty() {
        // Nothing configured → the merged document has no `[ai]` table at all
        // (the CLI treats this as a no-op, never creating an empty file).
        let out = merge_config_toml(None, &ConfigAnswers::default()).unwrap();
        assert!(out.trim().is_empty(), "expected empty output, got: {out:?}");
    }

    // --- sessions max_mb (session-project-scoped ticket 04) ---------------

    #[test]
    fn sessions_max_mb_defaults_to_500() {
        assert_eq!(resolve_max_mb(None), 500);
        assert_eq!(resolve_max_mb(Some("\n  \n")), 500);
        assert_eq!(resolve_max_mb(Some("[ai]\nmodel = \"m\"\n")), 500);
    }

    #[test]
    fn sessions_max_mb_reads_file_value() {
        assert_eq!(resolve_max_mb(Some("[sessions]\nmax_mb = 42\n")), 42);
    }

    #[test]
    fn sessions_max_mb_ignores_non_numeric_value() {
        // A non-integer `max_mb` fails the whole TOML parse; the tolerant
        // reader falls back to the default rather than panicking.
        assert_eq!(resolve_max_mb(Some("[sessions]\nmax_mb = \"abc\"\n")), 500);
    }

    #[test]
    fn sessions_table_survives_ai_parse() {
        // A config with only `[sessions]` still resolves the AI fields.
        let env = env_of(&[(ENV_API_KEY, "sk-test")]);
        let cfg = resolve_cfg(
            Some("[sessions]\nmax_mb = 7\n"),
            &env,
            &Overrides::default(),
        );
        assert_eq!(cfg.model, DEFAULT_MODEL);
    }

    #[test]
    fn max_mb_converts_to_bytes() {
        assert_eq!(max_mb_to_bytes(500), 500 * 1024 * 1024);
        assert_eq!(max_mb_to_bytes(0), 0);
    }

    #[test]
    fn default_quota_matches_session_store_default() {
        // The config default (MiB) and the store's byte fallback must agree.
        assert_eq!(
            max_mb_to_bytes(DEFAULT_MAX_MB),
            crate::session::DEFAULT_MAX_BYTES
        );
    }
}
