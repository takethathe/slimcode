//! Frontend-agnostic application configuration.
//!
//! Locked by grilling Q9: `~/.slimcode/config.toml` holds non-secret overrides
//! (`[ai] base_url` / `[ai] model`); the API key is **only ever** read from
//! `DASHSCOPE_API_KEY` (never written to or read from a file). Precedence is
//! frontend overrides (`--base-url` / `--model`) > env (`SLIMCODE_AI_BASE_URL` /
//! `SLIMCODE_AI_MODEL`) > config.toml > defaults (owned here).
//!
//! This module is the single owner of that resolution and of the env var names
//! and defaults behind it: it produces a [`slimcode_ai::BailianConfig`], so the
//! provider layer stays pure data and no other crate re-parses the same env
//! variables and defaults.

use std::path::Path;

use serde::Deserialize;
use slimcode_ai::BailianConfig;

/// Default China-station legacy compatible-mode base URL (no WorkspaceId needed).
pub const DEFAULT_BASE_URL: &str = "https://dashscope.aliyuncs.com/compatible-mode/v1";
/// Recommended default model (ticket 01).
pub const DEFAULT_MODEL: &str = "qwen-plus";

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

/// Frontend-supplied overrides for the non-secret config fields. Highest
/// precedence; `None` means "not given on the command line", so resolution
/// falls through to env / file / default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Overrides {
    pub base_url: Option<String>,
    pub model: Option<String>,
    /// `--cache` / `--no-cache` on the command line; `None` falls through to
    /// env / file / default.
    pub cache: Option<bool>,
}

/// The `config.toml` file shape (non-secret overrides only).
#[derive(Deserialize, Default)]
struct FileConfig {
    #[serde(default)]
    ai: FileAi,
}

#[derive(Deserialize, Default)]
struct FileAi {
    base_url: Option<String>,
    model: Option<String>,
    cache: Option<bool>,
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
/// env lookup, return the resolved provider config. Separated from I/O for
/// unit testing.
fn resolve(
    file_toml: Option<&str>,
    env: &dyn Fn(&str) -> Option<String>,
    overrides: &Overrides,
) -> Result<BailianConfig, String> {
    let api_key = env(ENV_API_KEY).ok_or_else(|| {
        format!("{ENV_API_KEY} is not set. Set it (e.g. export {ENV_API_KEY}=sk-...).")
    })?;

    let file = match file_toml {
        Some(s) if !s.trim().is_empty() => {
            let parsed: FileConfig = toml::from_str(s).map_err(|e| format!("config.toml: {e}"))?;
            Some(parsed)
        }
        _ => None,
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

    Ok(BailianConfig::new(api_key, base_url, model).with_cache(cache))
}

/// Load config from the real environment and config file, applying frontend
/// overrides on top.
pub fn load_with_overrides(overrides: Overrides) -> Result<BailianConfig, String> {
    let path = slimcode_home().map(|h| h.join("config.toml"));
    load_from(path.as_deref(), &overrides)
}

/// Load config with an explicit config file path and frontend overrides. `None`
/// (or a missing file) skips file loading entirely.
pub fn load_from(path: Option<&Path>, overrides: &Overrides) -> Result<BailianConfig, String> {
    let file_toml = match path {
        Some(p) if p.exists() => {
            Some(std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?)
        }
        _ => None,
    };
    resolve(file_toml.as_deref(), &|k| std::env::var(k).ok(), overrides)
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

    #[test]
    fn defaults_apply_when_no_file_or_env_overrides() {
        let env = env_of(&[(ENV_API_KEY, "sk-test")]);
        let cfg = resolve(None, &env, &Overrides::default()).unwrap();
        assert_eq!(cfg.api_key, "sk-test");
        assert_eq!(cfg.base_url, DEFAULT_BASE_URL);
        assert_eq!(cfg.model, DEFAULT_MODEL);
    }

    #[test]
    fn file_overrides_defaults() {
        let env = env_of(&[(ENV_API_KEY, "sk-test")]);
        let file = "[ai]\nbase_url = \"https://file.example.com/v1\"\nmodel = \"file-model\"\n";
        let cfg = resolve(Some(file), &env, &Overrides::default()).unwrap();
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
        let cfg = resolve(Some(file), &env, &Overrides::default()).unwrap();
        assert_eq!(cfg.base_url, "https://env.example.com/v1");
        assert_eq!(cfg.model, "env-model");
    }

    #[test]
    fn partial_file_falls_back_per_field() {
        let env = env_of(&[(ENV_API_KEY, "sk-test")]);
        let file = "[ai]\nmodel = \"file-model\"\n"; // no base_url
        let cfg = resolve(Some(file), &env, &Overrides::default()).unwrap();
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
    fn missing_api_key_errors() {
        let env = env_of(&[]);
        let err = resolve(None, &env, &Overrides::default()).unwrap_err();
        assert!(err.contains(ENV_API_KEY), "err: {err}");
    }

    #[test]
    fn empty_file_is_treated_as_absent() {
        let env = env_of(&[(ENV_API_KEY, "sk-test")]);
        let cfg = resolve(Some("\n  \n"), &env, &Overrides::default()).unwrap();
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
        };
        let cfg = resolve(Some(file), &env, &overrides).unwrap();
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
        };
        let cfg = resolve(None, &env, &overrides).unwrap();
        assert_eq!(cfg.model, "cli-model");
        assert_eq!(cfg.base_url, DEFAULT_BASE_URL);
    }

    // --- cache flag (llm-cache tickets 01/03) -----------------------------

    #[test]
    fn cache_defaults_on() {
        let env = env_of(&[(ENV_API_KEY, "sk-test")]);
        let cfg = resolve(None, &env, &Overrides::default()).unwrap();
        assert!(cfg.cache, "cache must default to on");
    }

    #[test]
    fn cache_file_true_enables() {
        let env = env_of(&[(ENV_API_KEY, "sk-test")]);
        let file = "[ai]\ncache = true\n";
        let cfg = resolve(Some(file), &env, &Overrides::default()).unwrap();
        assert!(cfg.cache);
    }

    #[test]
    fn cache_env_overrides_file() {
        let env = env_of(&[(ENV_API_KEY, "sk-test"), (ENV_CACHE, "true")]);
        let file = "[ai]\ncache = false\n";
        let cfg = resolve(Some(file), &env, &Overrides::default()).unwrap();
        assert!(cfg.cache, "env true must beat file false");
    }

    #[test]
    fn cache_env_false_overrides_file_true() {
        let env = env_of(&[(ENV_API_KEY, "sk-test"), (ENV_CACHE, "false")]);
        let file = "[ai]\ncache = true\n";
        let cfg = resolve(Some(file), &env, &Overrides::default()).unwrap();
        assert!(!cfg.cache, "env false must beat file true");
    }

    #[test]
    fn cache_override_beats_env() {
        let env = env_of(&[(ENV_API_KEY, "sk-test"), (ENV_CACHE, "false")]);
        let overrides = Overrides {
            base_url: None,
            model: None,
            cache: Some(true),
        };
        let cfg = resolve(None, &env, &overrides).unwrap();
        assert!(cfg.cache, "CLI --cache must beat env false");
    }

    #[test]
    fn cache_resolves_independently_of_other_fields() {
        let env = env_of(&[(ENV_API_KEY, "sk-test")]);
        let file = "[ai]\ncache = true\n"; // no base_url / model
        let cfg = resolve(Some(file), &env, &Overrides::default()).unwrap();
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
        };
        let cfg = resolve(None, &env, &overrides).unwrap();
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
            let cfg = resolve(None, &env, &Overrides::default()).unwrap();
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
}
