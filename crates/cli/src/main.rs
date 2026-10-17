//! slimcode — a working Rust coding-agent CLI (v1).
//!
//! Two modes (ADR-0003):
//! - `slimcode "<prompt>"` — non-interactive: run one prompt to completion in
//!   the current directory (or `--cwd`), stream events, print token usage. No
//!   session is persisted (the one-shot has no interactive session workflow),
//!   per ADR-0009 D5.
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
#[cfg(test)]
mod testutil;
mod tui;

use std::env;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use slimcode_ai::ProviderConfig;
use slimcode_app::config::{self, Overrides};
use slimcode_app::context_files::resolve_project_home;
use slimcode_app::history::HistoryStore;
use slimcode_app::render::Renderer;
use slimcode_app::session::{SessionStore, project_key};
use slimcode_app::skills::{PreparedPrompt, Skill, SkillStore, prepare_user_prompt};
use slimcode_sdk::{AgentSession, AgentSessionOptions, SdkEvent, create_agent_session};

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

/// Non-interactive one-shot: run one prompt, print usage. Sessions are never
/// persisted here (ADR-0009 D5): the one-shot has no interactive session
/// workflow, so it leaves no session files behind — matching the TUI's rule
/// that a turn with no assistant message leaves no log. `home` is the home
/// the startup path resolved, pinned into the session so the one-shot reads
/// the same config / skills / session-store root as startup.
fn run_once(
    prompt: &str,
    cwd: &Path,
    home: &Path,
    config: ProviderConfig,
    skills: &[Skill],
    out: Box<dyn Write + Send + Sync>,
) -> Result<i32, String> {
    run_once_with(prompt, cwd, home, config, skills, out, create_agent_session)
}

/// The one-shot's wiring, shared by [`run_once`] and its tests: session
/// creation is injectable, so a test can drive the whole one-shot through the
/// SDK with a scripted provider (`AgentSessionOptions::with_provider`) instead
/// of a network round trip.
fn run_once_with(
    prompt: &str,
    cwd: &Path,
    home: &Path,
    config: ProviderConfig,
    skills: &[Skill],
    out: Box<dyn Write + Send + Sync>,
    create: impl FnOnce(AgentSessionOptions) -> Result<AgentSession, String>,
) -> Result<i32, String> {
    // The shared Prompt preprocessing seam (ADR-0026): a leading skill trigger
    // (`/skill:{name}` or `/{name}` for an installed skill) injects the skill
    // active block as the user prompt — the same content the TUI's `/`-command
    // path embeds, so the model sees the skill instructions either way. Any
    // other leading `/`-form is normalized to the `/{name}` text form. The
    // one-shot has no prior history, so it passes an empty slice (its prior
    // no-dedup behaviour). Runs before the prompt reaches the SDK.
    let prompt = match prepare_user_prompt(prompt, skills, &[]) {
        PreparedPrompt::Skill { model_text, .. } => model_text,
        PreparedPrompt::Prompt(text) => text,
    };
    // One-shot: a `persist: false` AgentSession (ADR-0009 D5), the resolved
    // home pinned (so the session reads the same root startup resolved), the
    // frozen config re-applied as the highest-precedence overrides (spec
    // Decision 4), and the CLI's skills snapshot. The SDK discovers the
    // AGENTS.md context files and the environment itself.
    let session = create(one_shot_options(cwd, home, &config, skills))?;
    // Stream the SDK's display events into the shared renderer. The renderer
    // owns the boxed writer (the listener requires `Send + Sync`), and a
    // `Mutex` lets the `Fn` listener mutate the renderer's streaming state.
    let renderer = std::sync::Mutex::new(render::TextRenderer::with_skills(
        out,
        skills.to_vec(),
        cwd.to_path_buf(),
    ));
    // The handle is held for the one-shot's lifetime: dropping it would not
    // unsubscribe, but keeping it documents that the session owns the listener.
    let _subscription = session.subscribe(move |event: SdkEvent| {
        if let SdkEvent::Display(item) = event {
            let _ = renderer.lock().unwrap().render(&item);
        }
    });
    session.prompt(&prompt)?;
    // Per-request usage already streamed into the renderer with each assistant
    // message (ticket 03): no final total line, so a single-request turn does
    // not print its usage twice.
    Ok(0)
}

/// The one-shot's `AgentSessionOptions`: no persistence (ADR-0009 D5), the
/// resolved home pinned (so the SDK uses the startup path's root instead of
/// re-reading the environment), the frozen config re-applied as the
/// highest-precedence overrides (spec Decision 4 — overrides beat env and
/// `config.toml`, so the resolved values are reproduced exactly), and the
/// CLI's skills snapshot. `max_tokens` has no options field and the CLI's
/// config always resolves it to `None` anyway (no loss).
fn one_shot_options(
    cwd: &Path,
    home: &Path,
    config: &ProviderConfig,
    skills: &[Skill],
) -> AgentSessionOptions {
    AgentSessionOptions::new(Some(cwd.to_path_buf()))
        .with_persist(false)
        .with_home(home.to_path_buf())
        .with_base_url(config.base_url.clone())
        .with_model(config.model.clone())
        .with_api_key(config.api_key.clone())
        .with_cache(config.cache)
        .with_skills(skills.to_vec())
}

/// Interactive TUI (ADR-0003). The session (provider, tools, store, skills,
/// context files) is built inside the TUI entry from the pinned home, before
/// the alternate screen opens, so config/API-key errors surface on the normal
/// terminal.
fn run_tui(
    cwd: &Path,
    home: &Path,
    config: ProviderConfig,
    store: &SessionStore,
    history: &HistoryStore,
    skills: &SkillStore,
) -> Result<i32, String> {
    tui::run(cwd, home, config, store, history, skills)
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let tty = std::io::stdout().is_terminal();
    match run(&args, Box::new(std::io::stdout()), tty) {
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
/// `std::io::IsTerminal`), injected so the dispatch is unit-testable. `out` is
/// owned so the one-shot path can hand a `Send + Sync` boxed writer to the SDK
/// listener; help / config output writes through the same box.
fn run(args: &[String], mut out: Box<dyn Write + Send + Sync>, tty: bool) -> Result<i32, String> {
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
            &mut *out,
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

    let app = config::load_app_config(Overrides {
        base_url: parsed.base_url,
        model: parsed.model,
        cache: parsed.cache,
        api_key: parsed.api_key,
    })?;
    let app_config = app.provider;
    let api_key_source = app.api_key_source;
    let home =
        config::slimcode_home().ok_or_else(|| "cannot determine home directory".to_string())?;
    // Print the `chmod 600` hint right after loading — before any turn or, for
    // the TUI, before the alternate screen opens — so it lands on the normal
    // terminal (one-shot and TUI share this print point).
    if let Some(w) = chmod_warning(&home.join("config.toml"), api_key_source) {
        eprintln!("slimcode: {w}");
    }
    // The project home (git root, falling back to the OS user home) keys the
    // session store and is the boundary the SDK's session store is scoped to.
    let user_home = std::env::var_os("HOME").map(PathBuf::from);
    let project_home = resolve_project_home(&cwd, user_home.as_deref());
    // The session store is scoped to the current project: sessions persist
    // under `<home>/sessions/<project-key>/`, so the session picker only
    // ever sees this project's sessions.
    let store = SessionStore::new(
        home.join("sessions"),
        project_key(&project_home),
        project_home.clone(),
    )
    .with_max_bytes(app.sessions_max_bytes);
    // Startup empty-session cleanup: silently remove empty/corrupt session
    // files from the current project's directory (best-effort; a cleanup
    // failure never blocks startup).
    let _ = store.cleanup_empty();
    let history = HistoryStore::new(home.join("history.json"));
    let skills_store = SkillStore::new(&home, &cwd);
    // Skill discovery failures must not block a run; `/skills` surfaces them.
    let skills = skills_store.list().unwrap_or_default();

    match parsed.prompt {
        Some(p) => run_once(&p, &cwd, &home, app_config, &skills, out),
        None => run_tui(&cwd, &home, app_config, &store, &history, &skills_store),
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
    use crate::testutil::SharedBuf;

    /// Serializes tests that mutate process-global env vars (`run_isolated`
    /// and `no_args_no_tty_errors_clearly` both touch `ENV_API_KEY`/`ENV_HOME`;
    /// without the lock they race in parallel runs and clobber each other's
    /// restoration).
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Run `args` with `tty` injected and env isolated: the API key is cleared
    /// and HOME points at a temp dir so config resolution is deterministic.
    fn run_isolated(args: &[String], tty: bool) -> Result<i32, String> {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let old_key = env::var_os(config::ENV_API_KEY);
        unsafe { env::remove_var(config::ENV_API_KEY) };
        let old_home = env::var_os(config::ENV_HOME);
        unsafe { env::set_var(config::ENV_HOME, std::env::temp_dir()) };
        let result = run(args, Box::new(Vec::new()), tty);
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

    /// Run `args` capturing the written bytes, for output-asserting tests.
    fn run_captured(args: &[String], tty: bool) -> Result<(i32, String), String> {
        let buf = SharedBuf::default();
        let code = run(args, Box::new(buf.clone()), tty)?;
        Ok((code, buf.contents()))
    }

    #[test]
    fn help_prints_usage() {
        let (code, s) = run_captured(&["--help".to_string()], false).unwrap();
        assert_eq!(code, 0);
        assert!(s.contains("USAGE"));
        assert!(s.contains("interactive TUI"));
    }

    #[test]
    fn no_args_no_tty_errors_clearly() {
        // Launch rule: no prompt on a non-TTY fails with a clear error before
        // any config resolution (the API key is deliberately unset).
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let old_key = env::var_os(config::ENV_API_KEY);
        unsafe { env::remove_var(config::ENV_API_KEY) };
        let err = run(&[], Box::new(Vec::new()), false).unwrap_err();
        match old_key {
            Some(v) => unsafe { env::set_var(config::ENV_API_KEY, v) },
            None => unsafe { env::remove_var(config::ENV_API_KEY) },
        }
        assert!(err.contains("no TTY"), "err: {err}");
    }

    #[test]
    fn no_args_on_tty_dispatches_to_tui_needs_config() {
        // No prompt on a TTY → the TUI path: its config resolution fails on
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
        let (code, s) = run_captured(&["--help".to_string()], false).unwrap();
        assert_eq!(code, 0);
        assert!(s.contains("--api-key"), "help must list --api-key");
        assert!(s.contains("config"), "help must list slimcode config");
        assert!(
            s.contains("--api-key > env > config.toml"),
            "help must state api key precedence"
        );
    }

    #[test]
    fn help_lists_cache_flags() {
        let (code, s) = run_captured(&["--help".to_string()], false).unwrap();
        assert_eq!(code, 0);
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
        let dir = slimcode_app::testutil::unique_temp_dir("chmod-loose");
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
        let dir = slimcode_app::testutil::unique_temp_dir("chmod-tight");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "[ai]\napi_key = \"sk-x\"\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(chmod_warning(&path, config::ApiKeySource::File), None);
    }

    #[test]
    fn chmod_warning_silent_when_key_not_from_file() {
        let dir = slimcode_app::testutil::unique_temp_dir("chmod-env");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "[ai]\napi_key = \"sk-x\"\n").unwrap();
        assert_eq!(chmod_warning(&path, config::ApiKeySource::Env), None);
        assert_eq!(chmod_warning(&path, config::ApiKeySource::Cli), None);
    }

    #[test]
    fn chmod_warning_silent_when_config_missing() {
        let dir = slimcode_app::testutil::unique_temp_dir("chmod-missing");
        let path = dir.join("config.toml");
        assert_eq!(chmod_warning(&path, config::ApiKeySource::File), None);
    }

    // --- config-file ticket 03: `slimcode config` dispatch -----------------

    #[test]
    fn config_subcommand_non_tty_errors() {
        // `slimcode config` is interactive: on a non-TTY it must fail before
        // touching any config (mirrors the launch rule).
        let err = run(&["config".to_string()], Box::new(Vec::new()), false).unwrap_err();
        assert!(err.contains("terminal"), "err: {err}");
    }

    #[test]
    fn config_subcommand_rejects_extra_arguments() {
        // Dispatched by args.first() == "config", then the subcommand itself
        // rejects extra positional args.
        let err = run(
            &["config".to_string(), "extra".to_string()],
            Box::new(Vec::new()),
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

    // --- one-shot converges on the SDK (ticket 04) -------------------------

    /// A scripted provider: each `chat` replays the next delta batch, so the
    /// one-shot turn can be driven end-to-end without a network. Every message
    /// batch it sees is recorded, so a test can assert what the SDK actually
    /// sent after the CLI normalized the prompt. Injected via
    /// `AgentSessionOptions::with_provider` (the SDK's test seam).
    struct FakeProvider {
        script: Vec<Vec<slimcode_ai::Delta>>,
        calls: usize,
        error: Option<String>,
        seen: std::sync::Arc<std::sync::Mutex<Vec<Vec<String>>>>,
    }

    impl FakeProvider {
        fn new(script: Vec<Vec<slimcode_ai::Delta>>) -> Self {
            Self {
                script,
                calls: 0,
                error: None,
                seen: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            }
        }

        /// A provider whose request always fails, for the error path.
        fn failing(message: &str) -> Self {
            Self {
                error: Some(message.to_string()),
                ..Self::new(Vec::new())
            }
        }

        /// The message batches this provider saw (shares its record).
        fn seen(&self) -> std::sync::Arc<std::sync::Mutex<Vec<Vec<String>>>> {
            std::sync::Arc::clone(&self.seen)
        }
    }

    impl slimcode_app::Provider for FakeProvider {
        fn chat(
            &mut self,
            messages: &[slimcode_core::session::Message],
            _tools: &[slimcode_core::agent::ToolSpec],
            _config: &ProviderConfig,
            _cancel: &slimcode_core::agent::CancelToken,
            on_delta: &mut dyn FnMut(slimcode_ai::Delta) -> Result<(), String>,
        ) -> Result<Option<slimcode_ai::TokenUsage>, String> {
            self.seen.lock().unwrap().push(
                messages
                    .iter()
                    .map(slimcode_core::session::Message::text_content)
                    .collect(),
            );
            if let Some(e) = &self.error {
                return Err(e.clone());
            }
            let batch = self.script.get(self.calls).cloned().unwrap_or_default();
            self.calls += 1;
            for delta in batch {
                on_delta(delta)?;
            }
            Ok(None)
        }
    }

    /// Run `f` with `SLIMCODE_HOME` pointed at a scratch home and a `cwd`
    /// carrying a `.git` marker (so the project home resolves deterministically
    /// to the cwd itself, mirroring the SDK test scaffolding). Serialized by
    /// `ENV_LOCK`, since `SLIMCODE_HOME` is process-global.
    fn with_scratch_home<T>(
        tag: &str,
        f: impl FnOnce(&std::path::Path, &std::path::Path) -> T,
    ) -> T {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = slimcode_app::testutil::unique_temp_dir(tag);
        let home = root.join("home");
        let cwd = root.join("proj");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(cwd.join(".git")).unwrap();
        let old_home = env::var_os(config::ENV_HOME);
        unsafe { env::set_var(config::ENV_HOME, &home) };
        let result = f(&home, &cwd);
        match old_home {
            Some(v) => unsafe { env::set_var(config::ENV_HOME, v) },
            None => unsafe { env::remove_var(config::ENV_HOME) },
        }
        let _ = std::fs::remove_dir_all(&root);
        result
    }

    /// Recursively count session logs under `dir` (a missing store dir counts
    /// as zero).
    fn count_jsonl(dir: &std::path::Path) -> usize {
        if !dir.exists() {
            return 0;
        }
        let mut n = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let ty = entry.file_type().unwrap();
            n += if ty.is_dir() {
                count_jsonl(&entry.path())
            } else if entry.path().extension().is_some_and(|e| e == "jsonl") {
                1
            } else {
                0
            };
        }
        n
    }

    /// A frozen config for the one-shot tests (all four overrides set, so
    /// session creation never touches env / config.toml).
    fn test_config() -> ProviderConfig {
        ProviderConfig::new("sk-test", "https://example.invalid/v1", "test-model")
    }

    /// One installed skill for the trigger tests.
    fn test_skill() -> Skill {
        Skill {
            name: "grill".to_string(),
            description: "stress-test a plan".to_string(),
            disable_model_invocation: false,
            body: "# Grill\n\nPush back on the plan.".to_string(),
            scope: slimcode_app::skills::SkillScope::Project,
            dir: std::path::PathBuf::from("/proj/.slimcode/skills/grill"),
            file: std::path::PathBuf::from("/proj/.slimcode/skills/grill/SKILL.md"),
        }
    }

    /// The one-shot options: no persistence (ADR-0009 D5), the resolved home
    /// pinned, and the resolved config re-applied as the highest-precedence
    /// overrides, so the SDK reproduces the frozen values (spec Decision 4).
    #[test]
    fn one_shot_options_freeze_the_resolved_config_without_persistence() {
        let opts = one_shot_options(
            Path::new("/proj"),
            Path::new("/home/user"),
            &test_config(),
            &[test_skill()],
        );
        assert!(!opts.persist, "one-shot must never persist (ADR-0009 D5)");
        assert_eq!(opts.cwd.as_deref(), Some(Path::new("/proj")));
        assert_eq!(opts.home.as_deref(), Some(Path::new("/home/user")));
        assert_eq!(opts.base_url.as_deref(), Some("https://example.invalid/v1"));
        assert_eq!(opts.model.as_deref(), Some("test-model"));
        assert_eq!(opts.api_key.as_deref(), Some("sk-test"));
        assert_eq!(opts.cache, Some(true));
        assert_eq!(opts.skills.as_deref(), Some([test_skill()].as_slice()));
    }

    /// The one-shot drives the whole turn through an `AgentSession`: the SDK's
    /// `Display` events reach the boxed writer (streamed text, tool block, tool
    /// result) and a successful turn exits 0.
    #[test]
    fn one_shot_streams_the_sdk_display_events() {
        with_scratch_home("cli-oneshot-stream", |home, cwd| {
            let provider = FakeProvider::new(vec![
                vec![
                    slimcode_ai::Delta::Text("hello ".to_string()),
                    slimcode_ai::Delta::Text("world".to_string()),
                    slimcode_ai::Delta::ToolCallStart {
                        index: 0,
                        id: "c1".to_string(),
                        name: "probe".to_string(),
                    },
                    slimcode_ai::Delta::ToolCallArgs {
                        index: 0,
                        fragment: "{}".to_string(),
                    },
                    slimcode_ai::Delta::Done(slimcode_ai::FinishReason::ToolCalls),
                ],
                vec![
                    slimcode_ai::Delta::Text("done".to_string()),
                    slimcode_ai::Delta::Done(slimcode_ai::FinishReason::Stop),
                ],
            ]);
            let buf = SharedBuf::default();
            let code = run_once_with(
                "hello",
                cwd,
                home,
                test_config(),
                &[],
                Box::new(buf.clone()),
                move |opts| {
                    create_agent_session(opts.with_provider(Box::new(provider)).with_tools(vec![
                        slimcode_app::Tool::new(
                            "probe",
                            "returns a probe result",
                            serde_json::json!({}),
                            |_| Ok("probed".to_string()),
                        ),
                    ]))
                },
            )
            .unwrap();
            assert_eq!(code, 0);
            let out = buf.contents();
            assert!(out.contains("hello world"), "streamed text: {out}");
            assert!(out.contains("▶ probe"), "tool block: {out}");
            assert!(out.contains("probed"), "tool result: {out}");
        });
    }

    /// ADR-0009 D5: the one-shot never leaves a session log behind, even though
    /// the same SDK path persists by default.
    #[test]
    fn one_shot_never_persists_a_session() {
        with_scratch_home("cli-oneshot-nopersist", |home, cwd| {
            let provider = FakeProvider::new(vec![vec![
                slimcode_ai::Delta::Text("hi".to_string()),
                slimcode_ai::Delta::Done(slimcode_ai::FinishReason::Stop),
            ]]);
            let buf = SharedBuf::default();
            let code = run_once_with(
                "hi",
                cwd,
                home,
                test_config(),
                &[],
                Box::new(buf.clone()),
                move |opts| {
                    assert!(
                        !opts.persist,
                        "the one-shot must create a non-persisting session"
                    );
                    assert_eq!(
                        opts.home.as_deref(),
                        Some(home),
                        "the one-shot must pin the resolved home"
                    );
                    create_agent_session(opts.with_provider(Box::new(provider)))
                },
            )
            .unwrap();
            assert_eq!(code, 0);
            assert_eq!(count_jsonl(home), 0, "home: {}", home.display());
        });
    }

    /// The CLI's leading-skill-trigger semantics (spec Decision 2): a
    /// `/skill:name` trigger becomes the skill activation block before the
    /// prompt reaches the SDK, so the provider sees the skill instructions.
    #[test]
    fn one_shot_skill_trigger_reaches_the_provider_as_a_skill_block() {
        with_scratch_home("cli-oneshot-skill", |home, cwd| {
            let provider = FakeProvider::new(vec![vec![
                slimcode_ai::Delta::Text("ok".to_string()),
                slimcode_ai::Delta::Done(slimcode_ai::FinishReason::Stop),
            ]]);
            let seen = provider.seen();
            let skills = [test_skill()];
            run_once_with(
                "/skill:grill stress my plan",
                cwd,
                home,
                test_config(),
                &skills,
                Box::new(SharedBuf::default()),
                move |opts| create_agent_session(opts.with_provider(Box::new(provider))),
            )
            .unwrap();
            let batches = seen.lock().unwrap().clone();
            let prompt = batches
                .iter()
                .flatten()
                .find(|text| text.contains("stress my plan"))
                .expect("the normalized prompt reached the provider");
            assert!(prompt.contains("<skill name=\"grill\""), "prompt: {prompt}");
            assert!(
                prompt.contains("Push back on the plan."),
                "prompt: {prompt}"
            );
        });
    }

    #[test]
    fn one_shot_skill_trigger_injects_the_skill_block() {
        // The one-shot routes its prompt through the shared seam (ADR-0026): a
        // leading `/skill:name` trigger becomes the skill active block.
        let PreparedPrompt::Skill { model_text, .. } =
            prepare_user_prompt("/skill:grill stress my plan", &[test_skill()], &[])
        else {
            panic!("expected the trigger to expand into a skill")
        };
        assert!(
            model_text.contains("<skill name=\"grill\""),
            "model_text: {model_text}"
        );
        assert!(
            model_text.contains("stress my plan"),
            "model_text: {model_text}"
        );
        // A bare `/{name}` trigger works the same way.
        let PreparedPrompt::Skill { model_text, .. } =
            prepare_user_prompt("/grill stress my plan", &[test_skill()], &[])
        else {
            panic!("expected the bare trigger to expand into a skill")
        };
        assert!(
            model_text.contains("<skill name=\"grill\""),
            "model_text: {model_text}"
        );
        // An unknown skill falls back to the `/{name}` text form (a plain
        // prompt), and a plain prompt passes through unchanged.
        assert_eq!(
            prepare_user_prompt("/skill:nope hi", &[test_skill()], &[]),
            PreparedPrompt::Prompt("/nope hi".to_string())
        );
        assert_eq!(
            prepare_user_prompt("hello", &[test_skill()], &[]),
            PreparedPrompt::Prompt("hello".to_string())
        );
    }

    /// A session-creation failure stays an `Err`, so `main` exits 1.
    #[test]
    fn one_shot_session_creation_failure_is_an_error() {
        with_scratch_home("cli-oneshot-create-err", |home, cwd| {
            let err = run_once_with(
                "hi",
                cwd,
                home,
                test_config(),
                &[],
                Box::new(Vec::new()),
                |_| Err("missing API key".to_string()),
            )
            .unwrap_err();
            assert!(err.contains("missing API key"), "err: {err}");
        });
    }

    /// A turn failure propagates as an `Err`, so `main` exits 1.
    #[test]
    fn one_shot_turn_failure_is_an_error() {
        with_scratch_home("cli-oneshot-turn-err", |home, cwd| {
            let provider = FakeProvider::failing("boom");
            let err = run_once_with(
                "hi",
                cwd,
                home,
                test_config(),
                &[],
                Box::new(Vec::new()),
                move |opts| create_agent_session(opts.with_provider(Box::new(provider))),
            )
            .unwrap_err();
            assert!(err.contains("boom"), "err: {err}");
        });
    }
}
