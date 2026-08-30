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
//! Config (Q9): CLI (`--base-url` / `--model`) overrides env
//! (`SLIMCODE_AI_BASE_URL` / `SLIMCODE_AI_MODEL`), which overrides the file
//! (`~/.slimcode/config.toml`), which overrides defaults; the API key comes
//! only from `DASHSCOPE_API_KEY`.

mod render;

use std::env;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use slimcode_agent::agent::Message;
use slimcode_ai::BailianConfig;
use slimcode_common::config::{self, Overrides};
use slimcode_common::context::ContextBuilder;
use slimcode_common::history::HistoryStore;
use slimcode_common::render::{DisplayItem, Renderer};
use slimcode_common::session::{SessionStore, infer_title};
use slimcode_common::skills::{Skill, SkillStore};

fn usage() -> String {
    format!(
        "slimcode — Rust coding agent (v1)\n\n\
         USAGE:\n  \
         slimcode \"<prompt>\"             run one prompt, then exit\n  \
         slimcode --cwd <dir> \"<prompt>\"  run one prompt in <dir>\n  \
         slimcode                       start the interactive TUI (on a TTY)\n  \
         slimcode --help                show this help\n\n\
         OPTIONS:\n  \
         --cwd <dir>         working directory for the agent\n  \
         --model <model>     override model id (default: {})\n  \
         --base-url <url>    override endpoint (default: {})\n\n\
         ENV:\n  \
         DASHSCOPE_API_KEY       API key (required)\n  \
         SLIMCODE_AI_BASE_URL    override endpoint\n  \
         SLIMCODE_AI_MODEL       override model\n  \
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
    out: &mut dyn Write,
) -> Result<i32, String> {
    let (mut provider, tools) = slimcode_common::setup::setup(cwd, config)?;
    let mut session = store.new_session();
    session.title = infer_title(&[Message::text(slimcode_agent::session::Role::User, prompt)]);
    let messages = ContextBuilder::new()
        .with_skills(skills)
        .with_user_prompt(prompt)
        .build()?;
    let cfg = slimcode_agent::agent::RunConfig::default();
    let mut renderer = render::TextRenderer::new(out);
    let updated =
        slimcode_common::runner::run_turn(&mut provider, &tools, messages, &cfg, &mut renderer)?;
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
) -> Result<i32, String> {
    slimcode_tui::terminal::run(cwd, config, store, history, skills)
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

/// Argument parsing + mode dispatch (I/O separated from main for testability).
/// `tty` is whether stdout is a terminal (launch rule uses
/// `std::io::IsTerminal`), injected so the dispatch is unit-testable.
fn run(args: &[String], out: &mut dyn Write, tty: bool) -> Result<i32, String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        writeln!(out, "{}", usage()).map_err(|e| e.to_string())?;
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

    let app_config = config::load_with_overrides(Overrides {
        base_url: parsed.base_url,
        model: parsed.model,
    })?;
    let home =
        config::slimcode_home().ok_or_else(|| "cannot determine home directory".to_string())?;
    let store = SessionStore::new(home.join("sessions"));
    let history = HistoryStore::new(home.join("history.json"));
    let skills_store = SkillStore::new(&home, &cwd);
    // Skill discovery failures must not block a run; `/skills` surfaces them.
    let skills = skills_store.list().unwrap_or_default();

    match parsed.prompt {
        Some(p) => run_once(&p, &cwd, app_config, &store, &skills, out),
        None => run_tui(&cwd, app_config, &store, &history, &skills_store),
    }
}

/// Parsed command-line arguments (before config resolution).
#[derive(Debug, PartialEq, Eq)]
struct CliArgs {
    cwd: Option<PathBuf>,
    prompt: Option<String>,
    base_url: Option<String>,
    model: Option<String>,
}

/// Pure flag/positional parsing, separated from I/O for unit testing. The first
/// non-flag argument is the prompt; anything after it is ignored with a warning.
fn parse_args(args: &[String]) -> Result<CliArgs, String> {
    let mut cwd = None;
    let mut prompt = None;
    let mut base_url = None;
    let mut model = None;
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
}
