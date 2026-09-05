//! The `slimcode config` subcommand: interactively fill in `model` /
//! `base_url` / `api_key` in `config.toml`, merge the answers over the
//! existing file, write it back, and (on Unix) tighten permissions to 0600
//! when an API key ends up in the file.
//!
//! The merge core is a pure function in `slimcode-common::config`
//! ([`slimcode_common::config::merge_config_toml`]); this module is the thin
//! I/O shell around it (stdin/stdout prompting + file write + chmod).

use std::io::{BufRead, Write};
use std::path::Path;

use slimcode_common::config::{self, ConfigAnswers};

/// Entry for `slimcode config` (invoked when `args.first() == Some("config")`).
/// Rejects extra arguments and non-TTY runs (both stdout and stdin must be
/// terminals); resolves the config path and delegates to [`run_config`]. `tty`
/// and `stdin_tty` are injected so the gates are unit-testable.
pub fn entry(
    args: &[String],
    tty: bool,
    stdin_tty: bool,
    reader: &mut dyn BufRead,
    writer: &mut dyn Write,
) -> Result<(), String> {
    if args.len() != 1 {
        return Err(format!(
            "slimcode config takes no arguments (got {} extra)",
            args.len() - 1
        ));
    }
    if !tty || !stdin_tty {
        return Err(
            "slimcode config is interactive and needs a terminal (run on a TTY)".to_string(),
        );
    }
    let home =
        config::slimcode_home().ok_or_else(|| "cannot determine home directory".to_string())?;
    let path = home.join("config.toml");
    run_config(&path, reader, writer)
}

/// Interactively prompt for the missing `[ai]` fields and write the merged
/// `config.toml` back to `path`. `reader`/`writer` are injected so the shell
/// is unit-testable with a scripted stdin. Errors on config resolution
/// failure or I/O.
pub fn run_config(
    path: &Path,
    reader: &mut dyn BufRead,
    writer: &mut dyn Write,
) -> Result<(), String> {
    let existing = if path.exists() {
        Some(std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?)
    } else {
        None
    };

    // Current values are the interactive defaults; empty answer = keep them.
    let current = config::read_ai_fields(existing.as_deref());
    let base_url = prompt(writer, reader, "base_url", current.base_url.as_deref())?;
    let model = prompt(writer, reader, "model", current.model.as_deref())?;
    let api_key = prompt(writer, reader, "api_key", current.api_key.as_deref())?;

    let answers = ConfigAnswers {
        base_url,
        model,
        api_key,
    };
    let merged = config::merge_config_toml(existing.as_deref(), &answers)?;

    // Nothing configured (all fields absent in both file and answers) → no-op.
    let merged_fields = config::read_ai_fields(Some(&merged));
    if merged_fields == ConfigAnswers::default() {
        writeln!(writer, "nothing to write: no [ai] values configured")
            .map_err(|e| e.to_string())?;
        return Ok(());
    }

    std::fs::write(path, merged).map_err(|e| format!("{}: {e}", path.display()))?;

    // Tighten permissions when the resulting file holds a plaintext key.
    if merged_fields.api_key.is_some() {
        tighten_permissions(path)?;
    }

    writeln!(writer, "wrote {}", path.display()).map_err(|e| e.to_string())?;
    Ok(())
}

/// Ask for one field: show the current value (if any) as the default; an empty
/// line keeps it. Returns the answer (`None` = keep / leave absent).
fn prompt(
    writer: &mut dyn Write,
    reader: &mut dyn BufRead,
    name: &str,
    current: Option<&str>,
) -> Result<Option<String>, String> {
    match current {
        Some(v) => write!(writer, "[ai] {name} [{v}]: "),
        None => write!(writer, "[ai] {name} (new): "),
    }
    .map_err(|e| e.to_string())?;
    writer.flush().map_err(|e| e.to_string())?;
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| e.to_string())?;
    let answer = line.trim().to_string();
    Ok(if answer.is_empty() {
        None
    } else {
        Some(answer)
    })
}

/// On Unix, tighten `config.toml` to 0600 so a plaintext API key is not
/// readable by other users. No-op elsewhere.
#[cfg(unix)]
fn tighten_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("chmod {}: {e}", path.display()))
}

#[cfg(not(unix))]
fn tighten_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use slimcode_common::testutil::unique_temp_dir;

    /// Run `run_config` against a fresh temp config path with scripted answers.
    /// Creates the (absent) temp directory first.
    fn run_config_with(path: &Path, input: &str) -> (Result<(), String>, String) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut reader = std::io::Cursor::new(input.as_bytes().to_vec());
        let mut buf = Vec::new();
        let result = run_config(path, &mut reader, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        (result, output)
    }

    #[test]
    fn entry_rejects_extra_arguments() {
        let mut reader = std::io::Cursor::new(Vec::new());
        let mut buf = Vec::new();
        let err = entry(
            &["config".to_string(), "extra".to_string()],
            true,
            true,
            &mut reader,
            &mut buf,
        )
        .unwrap_err();
        assert!(err.contains("no arguments"), "err: {err}");
    }

    #[test]
    fn entry_rejects_non_tty_stdout() {
        let mut reader = std::io::Cursor::new(Vec::new());
        let mut buf = Vec::new();
        let err = entry(&["config".to_string()], false, true, &mut reader, &mut buf).unwrap_err();
        assert!(err.contains("terminal"), "err: {err}");
    }

    #[test]
    fn entry_rejects_piped_stdin() {
        let mut reader = std::io::Cursor::new(Vec::new());
        let mut buf = Vec::new();
        let err = entry(&["config".to_string()], true, false, &mut reader, &mut buf).unwrap_err();
        assert!(err.contains("terminal"), "err: {err}");
    }

    #[test]
    fn run_config_writes_new_file_from_answers() {
        let dir = unique_temp_dir("config-cmd-new");
        let path = dir.join("config.toml");
        // answers: base_url (new), model (new), api_key (new)
        let (res, _out) = run_config_with(&path, "https://cli.example.com/v1\nqwen-max\nsk-new\n");
        res.unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(
            written.contains("base_url = \"https://cli.example.com/v1\""),
            "got: {written}"
        );
        assert!(written.contains("model = \"qwen-max\""), "got: {written}");
        assert!(written.contains("api_key = \"sk-new\""), "got: {written}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let mode = std::fs::metadata(&path).unwrap().mode();
            assert_eq!(mode & 0o077, 0, "api key written → must chmod 600");
        }
    }

    #[test]
    fn run_config_keeps_existing_values_on_empty_answers() {
        let dir = unique_temp_dir("config-cmd-keep");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "[ai]\nbase_url = \"https://existing.example.com/v1\"\nmodel = \"m1\"\ncache = false\napi_key = \"sk-old\"\n",
        )
        .unwrap();
        // All answers empty (user pressed enter) → everything preserved.
        let (res, _out) = run_config_with(&path, "\n\n\n");
        res.unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(
            written.contains("base_url = \"https://existing.example.com/v1\""),
            "got: {written}"
        );
        assert!(written.contains("model = \"m1\""), "got: {written}");
        assert!(written.contains("api_key = \"sk-old\""), "got: {written}");
        assert!(
            written.contains("cache = false"),
            "cache preserved: {written}"
        );
    }

    #[test]
    fn run_config_overrides_only_filled_fields() {
        let dir = unique_temp_dir("config-cmd-override");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "[ai]\nbase_url = \"https://existing.example.com/v1\"\nmodel = \"m1\"\ncache = false\n",
        )
        .unwrap();
        // model and api_key filled; base_url empty (keep existing).
        let (res, _out) = run_config_with(&path, "\nm2\nsk-new\n");
        res.unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(
            written.contains("base_url = \"https://existing.example.com/v1\""),
            "got: {written}"
        );
        assert!(written.contains("model = \"m2\""), "got: {written}");
        assert!(written.contains("api_key = \"sk-new\""), "got: {written}");
        assert!(
            written.contains("cache = false"),
            "cache preserved: {written}"
        );
    }

    #[test]
    fn run_config_no_op_when_nothing_configured() {
        let dir = unique_temp_dir("config-cmd-noop");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "[ai]\nmodel = \"m1\"\n").unwrap();
        // all answers empty; existing model preserved → still a no-op on new
        // fields, file rewritten with existing content.
        let (res, out) = run_config_with(&path, "\n\n\n");
        res.unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("model = \"m1\""), "got: {written}");
        assert!(out.contains("wrote"), "expected write notice, got: {out}");
    }

    #[test]
    fn run_config_from_absent_file_with_all_empty_answers_is_noop() {
        let dir = unique_temp_dir("config-cmd-absent-noop");
        let path = dir.join("config.toml");
        let (res, out) = run_config_with(&path, "\n\n\n");
        res.unwrap();
        assert!(out.contains("nothing to write"), "got: {out}");
        assert!(!path.exists(), "must not create an empty config.toml");
    }

    #[test]
    fn run_config_chmod_600_when_key_preserved_from_existing() {
        let dir = unique_temp_dir("config-cmd-chmod-existing");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "[ai]\napi_key = \"sk-old\"\nmodel = \"m1\"\n").unwrap();
        // User keeps the existing key (empty answer) → file still has a key.
        let (res, _out) = run_config_with(&path, "\n\n\n");
        res.unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let mode = std::fs::metadata(&path).unwrap().mode();
            assert_eq!(mode & 0o077, 0, "preserved key → must chmod 600");
        }
    }

    #[test]
    fn run_config_leaves_permissions_when_no_key() {
        let dir = unique_temp_dir("config-cmd-nokey");
        let path = dir.join("config.toml");
        let (res, _out) = run_config_with(&path, "https://cli.example.com/v1\nqwen-max\n\n");
        res.unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let mode = std::fs::metadata(&path).unwrap().mode();
            assert_ne!(mode & 0o077, 0, "no key → no forced chmod 600");
        }
    }
}
