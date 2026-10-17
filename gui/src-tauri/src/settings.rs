//! GUI settings persistence: `gui.toml` under the slimcode home (ADR-0024
//! D5, spec §Implementation Decisions).
//!
//! The GUI never writes the shared `config.toml` — that file is the third
//! layer of the four-layer resolution the CLI/TUI/GUI all read, so a GUI
//! write would silently change the other frontends' behavior. Instead the
//! GUI keeps its own `gui.toml` (`model` / `base_url` / `cache` and an
//! optional `api_key`), read at startup as the highest-precedence SDK
//! overrides. An `api_key` on disk is plaintext with 0600 permissions
//! (macOS Keychain is an explicit v2 enhancement); the file is always
//! tightened to 0600 on save so a later save without a key cannot loosen it.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use slimcode_app::config::Overrides;

/// The settings file's name under the slimcode home.
pub const SETTINGS_FILE: &str = "gui.toml";

/// The GUI's own settings, as TOML in `gui.toml`. Every field is optional:
/// absent fields fall through to the SDK's four-layer resolution
/// (overrides > env > `config.toml` > defaults), exactly like the CLI/FFI.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuiSettings {
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub cache: Option<bool>,
    pub api_key: Option<String>,
}

impl GuiSettings {
    /// The `gui.toml` path under the given slimcode home.
    pub fn path(home: &Path) -> PathBuf {
        home.join(SETTINGS_FILE)
    }

    /// Read `gui.toml` under `home`. A missing file is the default (all
    /// fields fall through); a malformed file is a loud error — silently
    /// dropping a typo'd `api_key` would be worse.
    pub fn load(home: &Path) -> Result<Self, String> {
        let path = Self::path(home);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Write the settings to `gui.toml` under `home`, creating the home
    /// directory if needed. After writing, the file permissions are tightened
    /// to 0600 (unix) so a stored `api_key` is not world-readable; a failed
    /// chmod is a hard error (a lax key file must not pass silently).
    pub fn save(&self, home: &Path) -> Result<(), String> {
        let path = Self::path(home);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| format!("serialize gui.toml: {e}"))?;
        let mut file = fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        file.write_all(text.as_bytes())
            .map_err(|e| format!("{}: {e}", path.display()))?;
        file.flush()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        tighten_permissions(&path)
    }

    /// The SDK overrides assembled from these settings (`None` fields fall
    /// through), for `AgentSessionOptions` / `config::load_app_config`.
    pub fn overrides(&self) -> Overrides {
        Overrides {
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            cache: self.cache,
            api_key: self.api_key.clone(),
        }
    }
}

/// Tighten the file to owner-only on unix; a no-op elsewhere (the platform
/// has no meaningful POSIX mode bits).
#[cfg(unix)]
fn tighten_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("chmod 600 {}: {e}", path.display()))
}

#[cfg(not(unix))]
fn tighten_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_home(tag: &str) -> PathBuf {
        let dir = slimcode_app::testutil::unique_temp_dir(&format!("slimcode-gui-settings-{tag}"));
        fs::create_dir_all(&dir).expect("create temp home");
        dir
    }

    #[test]
    fn missing_file_loads_defaults() {
        let home = temp_home("missing");
        let settings = GuiSettings::load(&home).expect("missing file is not an error");
        assert_eq!(settings, GuiSettings::default());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn save_then_load_round_trips() {
        let home = temp_home("roundtrip");
        let settings = GuiSettings {
            model: Some("qwen-max".into()),
            base_url: Some("https://example.invalid/v1".into()),
            cache: Some(false),
            api_key: Some("sk-secret".into()),
        };
        settings.save(&home).expect("save");
        let loaded = GuiSettings::load(&home).expect("load");
        assert_eq!(loaded, settings);
        let _ = fs::remove_dir_all(&home);
    }

    #[cfg(unix)]
    #[test]
    fn save_tightens_permissions_to_0600() {
        use std::os::unix::fs::PermissionsExt;
        let home = temp_home("perm");
        let settings = GuiSettings {
            api_key: Some("sk-secret".into()),
            ..GuiSettings::default()
        };
        settings.save(&home).expect("save");
        let mode = fs::metadata(GuiSettings::path(&home))
            .expect("file exists")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "gui.toml must be owner-only");
        let _ = fs::remove_dir_all(&home);
    }

    #[cfg(unix)]
    #[test]
    fn save_without_api_key_still_tightens() {
        use std::os::unix::fs::PermissionsExt;
        let home = temp_home("perm2");
        // Pre-create a lax file, then save without a key: the permission must
        // still be tightened (a later save must not loosen the key file).
        let path = GuiSettings::path(&home);
        fs::create_dir_all(&home).expect("home");
        fs::write(&path, "model = \"m\"\n").expect("seed file");
        let settings = GuiSettings {
            model: Some("m".into()),
            ..GuiSettings::default()
        };
        settings.save(&home).expect("save");
        let mode = fs::metadata(&path)
            .expect("file exists")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn malformed_file_is_a_loud_error() {
        let home = temp_home("malformed");
        fs::create_dir_all(&home).expect("home");
        fs::write(GuiSettings::path(&home), "not = [valid toml").expect("seed");
        let err = GuiSettings::load(&home).expect_err("malformed gui.toml must error");
        assert!(err.contains("gui.toml"), "{err}");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn overrides_carry_only_the_present_fields() {
        let settings = GuiSettings {
            model: Some("m".into()),
            ..GuiSettings::default()
        };
        let overrides = settings.overrides();
        assert_eq!(overrides.model.as_deref(), Some("m"));
        assert_eq!(overrides.base_url, None);
        assert_eq!(overrides.cache, None);
        assert_eq!(overrides.api_key, None);
    }

    #[test]
    fn save_never_touches_config_toml() {
        let home = temp_home("no-config-touch");
        fs::create_dir_all(&home).expect("home");
        let config_path = home.join("config.toml");
        fs::write(&config_path, "[ai]\nmodel = \"cli-model\"\n").expect("seed config.toml");
        let settings = GuiSettings {
            model: Some("gui-model".into()),
            ..GuiSettings::default()
        };
        settings.save(&home).expect("save");
        let untouched = fs::read_to_string(&config_path).expect("config.toml still there");
        assert_eq!(untouched, "[ai]\nmodel = \"cli-model\"\n");
        let _ = fs::remove_dir_all(&home);
    }
}
