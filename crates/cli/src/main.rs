//! slimcode — a working Rust coding-agent CLI (v1).
//!
//! Two modes (ADR-0003):
//! - `slimcode "<prompt>"` — non-interactive: run one prompt to completion in
//!   the current directory (or `--cwd`), stream events, print token usage, and
//!   save the session.
//! - `slimcode` on a terminal — the full-screen TUI: a scrollable transcript,
//!   a multi-line input box, live streamed output, `/` commands and skills,
//!   and ↑/↓ input-history recall. On a non-TTY (pipe/CI) it fails with a
//!   clear error.
//!
//! Config: CLI (`--base-url` / `--model` / `--api-key`) overrides env
//! (`SLIMCODE_AI_BASE_URL` / `SLIMCODE_AI_MODEL` / `DASHSCOPE_API_KEY`), which
//! overrides the file (`~/.slimcode/config.toml`, including `[ai] api_key`),
//! which overrides defaults. `slimcode config` interactively edits the file.

mod config_cmd;
mod render;

use std::env;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use slimcode_agent::agent::Message;
use slimcode_ai::BailianConfig;
use slimcode_common::config::{self, Overrides};
use slimcode_common::context::{ContextBuilder, Environment};
use slimcode_common::context_files::{ContextFile, load_context_files, resolve_project_home};
use slimcode_common::history::HistoryStore;
use slimcode_common::render::{DisplayItem, Renderer};
use slimcode_common::session::{SessionStore, infer_title};
use slimcode_common::skills::{Skill, SkillStore, normalize_skill_trigger};

fn usage() -> String {
    format!(
        "slimcode — Rust coding agent (v1)\n\n\
         USAGE:\n  \
         slimcode \"<prompt>\"             run one prompt, then exit\n  \
         slimcode --cwd <dir> \"<prompt>\"  run one prompt in <dir>\n  \
         slimcode                       start the interactive TUI (on a TTY)\n  \
         slimcode config                interactively edit ~/.slimcode/config.toml\n  \
         slimcode --help                show this help\n\n\
         OPTIONS:\n  \
         --cwd <dir>         working directory for the agent\n  \
         --model <model>     override model id (default: {})\n  \
         --base-url <url>    override endpoint (default: {})\n  \
         --api-key <key>     override API key for this run\n  \
         --cache             enable explicit context caching (default: on)\n  \
         --no-cache          disable explicit context caching\n\n\
         ENV:\n  \
         DASHSCOPE_API_KEY       API key (precedence: --api-key > env > config.toml)\n  \
         SLIMCODE_AI_BASE_URL    override endpoint\n  \
         SLIMCODE_AI_MODEL       override model\n  \
         SLIMCODE_AI_CACHE       override context caching (true/false/1/0/yes/no/on/off)\n  \
         SLIMCODE_HOME           override ~/.slimcode\n",
        config::DEFAULT_MODEL,
        config::DEFAULT_BASE_URL
    )
}

/// Non-interactive one-shot: run one prompt, save the session, print usage.
fn run_once(
    prompt: &str,
    cwd: &Path,
    config: BailianConfig,
    store: &SessionStore,
    skills: &[Skill],
    context_files: &[ContextFile],
    environment: Environment,
    out: &mut dyn Write,
) -> Result<i32, String> {
    let (mut provider, tools) = slimcode_common::setup::setup(cwd, config)?;
    let mut session = store.new_session();
    // The CLI one-shot path has no command parser, so a leading `/skill:{name}`
    // trigger is normalized to the `/{name}` form the model understands before
    // it becomes a user message (the TUI already embeds the skill content).
    let prompt = normalize_skill_trigger(prompt);
    session.title = infer_title(&[Message::text(
        slimcode_agent::session::Role::User,
        prompt.as_str(),
    )]);
    let messages = ContextBuilder::new()
        .with_environment(environment)
        .with_context_files(context_files)
        .with_skills(skills)
        .with_user_prompt(prompt)
        .build()?;
    let cfg = slimcode_agent::agent::RunConfig::default();
    let cancel = slimcode_agent::agent::CancelToken::new();
    let mut renderer = render::TextRenderer::new(out);
    let updated = slimcode_common::runner::run_turn(
        &mut provider,
        &tools,
        messages,
        &cfg,
        &cancel,
        &mut renderer,
    )?;
    session.messages = updated;
    let path = store.save(&session)?;
    let usage = provider.total_usage;
    renderer.render(&DisplayItem::Usage(usage))?;
    writeln!(out, "session saved: {}", path.display()).map_err(|e| e.to_string())?;
    Ok(0)
}

/// Interactive TUI (ADR-0003). Provider + tool setup happens inside the TUI
/// entry, before the alternate screen opens, so config/API-key errors surface
/// on the normal terminal.
fn run_tui(
    cwd: &Path,
    config: BailianConfig,
    store: &SessionStore,
    history: &HistoryStore,
    skills: &SkillStore,
    context_files: &[ContextFile],
    environment: Environment,
) -> Result<i32, String> {
    slimcode_tui::terminal::run(
        cwd,
        config,
        store,
        history,
        skills,
        context_files,
        environment,
    )
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let tty = std::io::stdout().is_terminal();
    match run(&args, &mut std::io::stdout(), tty) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("slimcode: {e}");
            std::process::exit(1);
        }
    }
}

/// Build the `chmod 600` stderr hint when the API key came from `config.toml`
/// and (Unix) the file is readable by group/other (`mode & 0o077 != 0`).
/// Non-Unix platforms skip the check and never warn.
#[cfg(unix)]
fn chmod_warning(path: &Path, source: config::ApiKeySource) -> Option<String> {
    if source != config::ApiKeySource::File {
        return None;
    }
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).ok()?;
    let mode = meta.mode();
    if mode & 0o077 != 0 {
        Some(format!(
            "API key read from {} (mode {:#o}) — run `chmod 600 {}` to keep it private",
            path.display(),
            mode & 0o777,
            path.display()
        ))
    } else {
        None
    }
}

#[cfg(not(unix))]
fn chmod_warning(_path: &Path, _source: config::ApiKeySource) -> Option<String> {
    None
}

/// Argument parsing + mode dispatch (I/O separated from main for testability).
/// `tty` is whether stdout is a terminal (launch rule uses
/// `std::io::IsTerminal`), injected so the dispatch is unit-testable.
fn run(args: &[String], out: &mut dyn Write, tty: bool) -> Result<i32, String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        writeln!(out, "{}", usage()).map_err(|e| e.to_string())?;
        return Ok(0);
    }

    // `slimcode config` subcommand: interactive config-file editing, entered
    // before prompt parsing (`config` is a subcommand, not a prompt).
    if args.first().map(String::as_str) == Some("config") {
        config_cmd::entry(
            args,
            tty,
            std::io::stdin().is_terminal(),
            &mut std::io::stdin().lock(),
            out,
        )?;
        return Ok(0);
    }

    let parsed = parse_args(args)?;

    // Launch rule: no prompt on a non-TTY fails fast with a clear error
    // before any config resolution or UI is attempted.
    if parsed.prompt.is_none() && !tty {
        return Err(
            "no prompt and no TTY: pass a prompt (slimcode \"<prompt>\") or run on a terminal"
                .to_string(),
        );
    }

    let cwd = match parsed.cwd {
        Some(dir) => dir,
        None => env::current_dir().map_err(|e| format!("cwd: {e}"))?,
    };

    let (app_config, api_key_source) = config::load_with_overrides(Overrides {
        base_url: parsed.base_url,
        model: parsed.model,
        cache: parsed.cache,
        api_key: parsed.api_key,
    })?;
    let home =
        config::slimcode_home().ok_or_else(|| "cannot determine home directory".to_string())?;
    // Print the `chmod 600` hint right after loading — before any turn or, for
    // the TUI, before the alternate screen opens — so it lands on the normal
    // terminal (one-shot and TUI share this print point).
    if let Some(w) = chmod_warning(&home.join("config.toml"), api_key_source) {
        eprintln!("slimcode: {w}");
    }
    let store = SessionStore::new(home.join("sessions"));
    let history = HistoryStore::new(home.join("history.json"));
    let skills_store = SkillStore::new(&home, &cwd);
    // Skill discovery failures must not block a run; `/skills` surfaces them.
    let skills = skills_store.list().unwrap_or_default();
    // AGENTS.md discovery is infallible: missing files simply yield none.
    let context_files = load_context_files(&home, &cwd);
    // System environment info: OS name, global home (slimcode home), and the
    // project home (git root, falling back to the OS user home). Frozen at
    // startup; a restored session keeps the environment from its first turn.
    let user_home = std::env::var_os("HOME").map(PathBuf::from);
    let environment = Environment {
        os: std::env::consts::OS.to_string(),
        global_home: home.clone(),
        project_home: resolve_project_home(&cwd, user_home.as_deref()),
    };

    match parsed.prompt {
        Some(p) => run_once(
            &p,
            &cwd,
            app_config,
            &store,
            &skills,
            &context_files,
            environment,
            out,
        ),
        None => run_tui(
            &cwd,
            app_config,
            &store,
            &history,
            &skills_store,
            &context_files,
            environment,
        ),
    }
}

/// Parsed command-line arguments (before config resolution).
#[derive(Debug, PartialEq, Eq)]
struct CliArgs {
    cwd: Option<PathBuf>,
    prompt: Option<String>,
    base_url: Option<String>,
    model: Option<String>,
    /// `--cache` / `--no-cache` on the command line; `None` = not given.
    cache: Option<bool>,
    /// `--api-key` on the command line; `None` = not given.
    api_key: Option<String>,
}

/// Pure flag/positional parsing, separated from I/O for unit testing. The first
/// non-flag argument is the prompt; anything after it is ignored with a warning.
fn parse_args(args: &[String]) -> Result<CliArgs, String> {
    let mut cwd = None;
    let mut prompt = None;
    let mut base_url = None;
    let mut model = None;
    let mut cache = None;
    let mut api_key = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--cwd" => {
                i += 1;
                let dir = args
                    .get(i)
                    .ok_or_else(|| "--cwd needs a directory".to_string())?;
                cwd = Some(PathBuf::from(dir));
            }
            "--model" => {
                i += 1;
                let value = args
                    .get(i)
                    .ok_or_else(|| "--model needs a value".to_string())?;
                model = Some(value.to_string());
            }
            "--base-url" => {
                i += 1;
                let value = args
                    .get(i)
                    .ok_or_else(|| "--base-url needs a value".to_string())?;
                base_url = Some(value.to_string());
            }
            "--api-key" => {
                i += 1;
                let value = args
                    .get(i)
                    .ok_or_else(|| "--api-key needs a value".to_string())?;
                api_key = Some(value.to_string());
            }
            "--cache" => {
                cache = Some(true);
            }
            "--no-cache" => {
                cache = Some(false);
            }
            other => {
                prompt = Some(other.to_string());
                if i + 1 < args.len() {
                    // ignore extra positional args for now
                    eprintln!("slimcode: ignoring extra argument: {}", args[i + 1]);
                }
                break;
            }
        }
        i += 1;
    }
    Ok(CliArgs {
        cwd,
        prompt,
        base_url,
        model,
        cache,
        api_key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run `args` with `tty` injected and env isolated: the API key is cleared
    /// and HOME points at a temp dir so config resolution is deterministic.
    fn run_isolated(args: &[String], tty: bool) -> Result<i32, String> {
        let old_key = env::var_os(config::ENV_API_KEY);
        unsafe { env::remove_var(config::ENV_API_KEY) };
        let old_home = env::var_os(config::ENV_HOME);
        unsafe { env::set_var(config::ENV_HOME, std::env::temp_dir()) };
        let mut buf = Vec::new();
        let result = run(args, &mut buf, tty);
        // restore
        match old_key {
            Some(v) => unsafe { env::set_var(config::ENV_API_KEY, v) },
            None => unsafe { env::remove_var(config::ENV_API_KEY) },
        }
        match old_home {
            Some(v) => unsafe { env::set_var(config::ENV_HOME, v) },
            None => unsafe { env::remove_var(config::ENV_HOME) },
        }
        result
    }

    #[test]
    fn help_prints_usage() {
        let mut buf = Vec::new();
        let code = run(&["--help".to_string()], &mut buf, false).unwrap();
        assert_eq!(code, 0);
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("USAGE"));
        assert!(s.contains("interactive TUI"));
    }

    #[test]
    fn no_args_no_tty_errors_clearly() {
        // Launch rule: no prompt on a non-TTY fails with a clear error before
        // any config resolution (the API key is deliberately unset).
        let old_key = env::var_os(config::ENV_API_KEY);
        unsafe { env::remove_var(config::ENV_API_KEY) };
        let err = run(&[], &mut Vec::new(), false).unwrap_err();
        match old_key {
            Some(v) => unsafe { env::set_var(config::ENV_API_KEY, v) },
            None => unsafe { env::remove_var(config::ENV_API_KEY) },
        }
        assert!(err.contains("no TTY"), "err: {err}");
    }

    #[test]
    fn no_args_on_tty_dispatches_to_tui_needs_config() {
        // No prompt on a TTY → the TUI path: its provider/tool setup fails on
        // the missing API key before the alternate screen opens.
        let err = run_isolated(&[], true).unwrap_err();
        assert!(err.contains(config::ENV_API_KEY), "err: {err}");
    }

    #[test]
    fn prompt_dispatches_to_one_shot_needs_config() {
        // A prompt → the one-shot path, which resolves config and fails on the
        // missing API key (it never hits the non-TTY error).
        let err = run_isolated(&["hello".to_string()], false).unwrap_err();
        assert!(err.contains(config::ENV_API_KEY), "err: {err}");
    }

    #[test]
    fn parse_args_reads_model_and_base_url_flags() {
        let args = vec![
            "--model".to_string(),
            "cli-model".to_string(),
            "--base-url".to_string(),
            "https://cli.example.com/v1".to_string(),
            "hello".to_string(),
        ];
        let parsed = parse_args(&args).unwrap();
        assert_eq!(parsed.model.as_deref(), Some("cli-model"));
        assert_eq!(
            parsed.base_url.as_deref(),
            Some("https://cli.example.com/v1")
        );
        assert_eq!(parsed.prompt.as_deref(), Some("hello"));
        assert_eq!(parsed.cwd, None);
        assert_eq!(parsed.cache, None, "no --cache/--no-cache passed");
    }

    #[test]
    fn parse_args_reads_cache_flags() {
        let args = vec!["--cache".to_string(), "hello".to_string()];
        let parsed = parse_args(&args).unwrap();
        assert_eq!(parsed.cache, Some(true), "--cache must enable");
        assert_eq!(parsed.prompt.as_deref(), Some("hello"));
        // The flag also works before other flags and without a prompt.
        let parsed = parse_args(&[
            "--cwd".to_string(),
            "/tmp".to_string(),
            "--cache".to_string(),
        ])
        .unwrap();
        assert_eq!(parsed.cache, Some(true));
        assert_eq!(parsed.cwd, Some(PathBuf::from("/tmp")));
        assert_eq!(parsed.prompt, None);
        // `--no-cache` turns it off explicitly (the default is on).
        let parsed = parse_args(&["--no-cache".to_string(), "hi".to_string()]).unwrap();
        assert_eq!(parsed.cache, Some(false));
        // The last cache flag wins.
        let parsed = parse_args(&[
            "--cache".to_string(),
            "--no-cache".to_string(),
            "x".to_string(),
        ])
        .unwrap();
        assert_eq!(parsed.cache, Some(false));
    }

    #[test]
    fn parse_args_reads_api_key_flag() {
        let args = vec![
            "--api-key".to_string(),
            "sk-cli".to_string(),
            "hello".to_string(),
        ];
        let parsed = parse_args(&args).unwrap();
        assert_eq!(parsed.api_key.as_deref(), Some("sk-cli"));
        assert_eq!(parsed.prompt.as_deref(), Some("hello"));
        // Works without a prompt and mixed with other flags.
        let parsed = parse_args(&[
            "--model".to_string(),
            "m".to_string(),
            "--api-key".to_string(),
            "sk-2".to_string(),
        ])
        .unwrap();
        assert_eq!(parsed.api_key.as_deref(), Some("sk-2"));
        assert_eq!(parsed.prompt, None);
    }

    #[test]
    fn parse_args_missing_api_key_value_errors() {
        let args = vec!["--api-key".to_string()];
        let err = parse_args(&args).unwrap_err();
        assert!(err.contains("--api-key"), "err: {err}");
    }

    #[test]
    fn help_lists_api_key_flag_and_env_precedence() {
        let mut buf = Vec::new();
        let code = run(&["--help".to_string()], &mut buf, false).unwrap();
        assert_eq!(code, 0);
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("--api-key"), "help must list --api-key");
        assert!(s.contains("config"), "help must list slimcode config");
        assert!(
            s.contains("--api-key > env > config.toml"),
            "help must state api key precedence"
        );
    }

    #[test]
    fn help_lists_cache_flags() {
        let mut buf = Vec::new();
        let code = run(&["--help".to_string()], &mut buf, false).unwrap();
        assert_eq!(code, 0);
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("--cache"), "help must list --cache");
        assert!(s.contains("--no-cache"), "help must list --no-cache");
        assert!(
            s.contains("SLIMCODE_AI_CACHE"),
            "help must list the env var"
        );
    }

    #[test]
    fn parse_args_cwd_flag_reads_directory() {
        let args = vec!["--cwd".to_string(), "/tmp/repo".to_string()];
        let parsed = parse_args(&args).unwrap();
        assert_eq!(parsed.cwd, Some(PathBuf::from("/tmp/repo")));
        assert_eq!(parsed.prompt, None);
    }

    #[test]
    fn parse_args_missing_model_value_errors() {
        let args = vec!["--model".to_string()];
        let err = parse_args(&args).unwrap_err();
        assert!(err.contains("--model"), "err: {err}");
    }

    #[test]
    fn parse_args_missing_base_url_value_errors() {
        let args = vec!["--base-url".to_string()];
        let err = parse_args(&args).unwrap_err();
        assert!(err.contains("--base-url"), "err: {err}");
    }

    // --- config-file ticket 02: --api-key + chmod hint --------------------

    #[cfg(unix)]
    #[test]
    fn chmod_warning_fires_for_loose_file_with_file_key() {
        let dir = slimcode_common::testutil::unique_temp_dir("chmod-loose");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "[ai]\napi_key = \"sk-x\"\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let w = chmod_warning(&path, config::ApiKeySource::File).unwrap();
        assert!(w.contains("chmod 600"), "warning: {w}");
        assert!(w.contains("config.toml"), "warning: {w}");
    }

    #[cfg(unix)]
    #[test]
    fn chmod_warning_silent_for_tight_file() {
        let dir = slimcode_common::testutil::unique_temp_dir("chmod-tight");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "[ai]\napi_key = \"sk-x\"\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(chmod_warning(&path, config::ApiKeySource::File), None);
    }

    #[test]
    fn chmod_warning_silent_when_key_not_from_file() {
        let dir = slimcode_common::testutil::unique_temp_dir("chmod-env");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "[ai]\napi_key = \"sk-x\"\n").unwrap();
        assert_eq!(chmod_warning(&path, config::ApiKeySource::Env), None);
        assert_eq!(chmod_warning(&path, config::ApiKeySource::Cli), None);
    }

    #[test]
    fn chmod_warning_silent_when_config_missing() {
        let dir = slimcode_common::testutil::unique_temp_dir("chmod-missing");
        let path = dir.join("config.toml");
        assert_eq!(chmod_warning(&path, config::ApiKeySource::File), None);
    }

    // --- config-file ticket 03: `slimcode config` dispatch -----------------

    #[test]
    fn config_subcommand_non_tty_errors() {
        // `slimcode config` is interactive: on a non-TTY it must fail before
        // touching any config (mirrors the launch rule).
        let err = run(&["config".to_string()], &mut Vec::new(), false).unwrap_err();
        assert!(err.contains("terminal"), "err: {err}");
    }

    #[test]
    fn config_subcommand_rejects_extra_arguments() {
        // Dispatched by args.first() == "config", then the subcommand itself
        // rejects extra positional args.
        let err = run(
            &["config".to_string(), "extra".to_string()],
            &mut Vec::new(),
            true,
        )
        .unwrap_err();
        assert!(err.contains("no arguments"), "err: {err}");
    }

    #[test]
    fn config_as_prompt_when_not_first_argument() {
        // `config` only dispatches as a subcommand when it is the first arg;
        // as a later positional it is an ordinary prompt.
        let parsed = parse_args(&["hello".to_string(), "config".to_string()]).unwrap();
        assert_eq!(parsed.prompt.as_deref(), Some("hello"));
    }
}
