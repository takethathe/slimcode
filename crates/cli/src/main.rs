//! slimcode — a working Rust coding-agent CLI (v1).
//!
//! Two modes (grilling Q6+B):
//! - `slimcode "<prompt>"` — non-interactive: run one prompt to completion in
//!   the current directory (or `--cwd`), stream events, print token usage, and
//!   save the session.
//! - `slimcode` — line-based REPL that keeps a session and persists it after
//!   every turn.
//!
//! Config (Q9): `~/.slimcode/config.toml` overrides defaults; env
//! (`SLIMCODE_AI_BASE_URL` / `SLIMCODE_AI_MODEL`) overrides the file; the API
//! key comes only from `DASHSCOPE_API_KEY`.

mod config;
mod history;
mod render;
mod repl;
mod session;
mod tools;

#[cfg(test)]
mod testutil;

use std::env;
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};

use slimcode_agent::agent::{Message, RunConfig, Tool};
use slimcode_ai::{BailianConfig, BailianProvider};

use crate::config::AppConfig;
use crate::history::HistoryStore;
use crate::session::SessionStore;

/// System prompt grounding the agent in its tools and working directory.
const SYSTEM_PROMPT: &str = concat!(
    "You are slimcode, a coding agent that works in a repository directory. ",
    "You have these tools: read, write, edit, bash, grep, find, ls. ",
    "Plan with the tools available: inspect files before editing, run commands ",
    "to verify, and complete the user's task. Keep answers concise. When a tool ",
    "fails, read the error and retry with a corrected approach."
);

fn usage() -> String {
    format!(
        "slimcode — Rust coding agent (v1)\n\n\
         USAGE:\n  \
         slimcode \"<prompt>\"             run one prompt, then exit\n  \
         slimcode --cwd <dir> \"<prompt>\"  run one prompt in <dir>\n  \
         slimcode                       start the interactive REPL\n  \
         slimcode --help                show this help\n\n\
         ENV:\n  \
         DASHSCOPE_API_KEY       API key (required)\n  \
         SLIMCODE_AI_BASE_URL    override endpoint (default: {}),\n  \
         SLIMCODE_AI_MODEL       override model (default: {})\n  \
         SLIMCODE_HOME           override ~/.slimcode\n",
        slimcode_ai::DEFAULT_BASE_URL,
        slimcode_ai::DEFAULT_MODEL
    )
}

/// Run one agent turn over `messages`, streaming rendered events to `out`,
/// returning the updated message history.
fn run_turn(
    provider: &mut BailianProvider,
    tools: &[Tool],
    messages: Vec<Message>,
    out: &mut dyn Write,
) -> Result<Vec<Message>, String> {
    let cfg = RunConfig::default();
    let result = slimcode_agent::agent::run_agent_from_messages(provider, tools, messages, &cfg)?;
    render_events(&result.events, out)?;
    Ok(result.messages)
}

/// Render a list of agent events to `out`. Structural lines (tool starts,
/// results, stop markers) always start on their own row, even when the
/// preceding assistant text didn't end with a newline.
fn render_events(
    events: &[slimcode_agent::agent::AgentEvent],
    out: &mut dyn Write,
) -> Result<(), String> {
    let mut text_line_open = false;
    for e in events {
        if let Some(rt) = render::render_event(e) {
            if rt.streamed {
                write!(out, "{}", rt.text).map_err(|e| e.to_string())?;
                text_line_open = !rt.text.ends_with('\n');
            } else {
                if text_line_open {
                    // The last assistant text didn't end with a newline; start
                    // this structural line on its own row.
                    writeln!(out).map_err(|e| e.to_string())?;
                    text_line_open = false;
                }
                writeln!(out, "{}", rt.text).map_err(|e| e.to_string())?;
            }
            out.flush().map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Shared setup: resolve cwd + config, build provider + tools.
fn setup(cwd: &Path, config: AppConfig) -> Result<(BailianProvider, Vec<Tool>), String> {
    let provider = BailianProvider::new(BailianConfig::new(
        config.api_key,
        config.base_url,
        config.model,
    ))?;
    let tools = tools::build_tools(cwd);
    Ok((provider, tools))
}

/// Non-interactive one-shot: run one prompt, save the session, print usage.
fn run_once(
    prompt: &str,
    cwd: &Path,
    config: AppConfig,
    store: &SessionStore,
    out: &mut dyn Write,
) -> Result<i32, String> {
    let (mut provider, tools) = setup(cwd, config)?;
    let mut session = repl::new_session(store);
    session.title =
        session::infer_title(&[Message::text(slimcode_agent::session::Role::User, prompt)]);
    let messages = vec![
        Message::text(slimcode_agent::session::Role::System, SYSTEM_PROMPT),
        Message::text(slimcode_agent::session::Role::User, prompt),
    ];
    let updated = run_turn(&mut provider, &tools, messages, out)?;
    session.messages = updated;
    let path = store.save(&session)?;
    let usage = provider.total_usage;
    writeln!(out, "\n{}", render::render_usage(&usage)).map_err(|e| e.to_string())?;
    writeln!(out, "session saved: {}", path.display()).map_err(|e| e.to_string())?;
    Ok(0)
}

/// Interactive REPL.
fn run_repl(
    cwd: &Path,
    config: AppConfig,
    store: &SessionStore,
    history: &HistoryStore,
    out: &mut dyn Write,
) -> Result<i32, String> {
    let (mut provider, tools) = setup(cwd, config)?;
    let session = repl::new_session(store);
    let mut ctx = repl::ReplCtx {
        provider: &mut provider,
        tools: &tools,
        store,
        history,
    };
    repl::run(
        &mut ctx,
        cwd,
        session,
        out,
        &mut BufReader::new(std::io::stdin()),
    )?;
    Ok(0)
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    match run(&args, &mut std::io::stdout()) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("slimcode: {e}");
            std::process::exit(1);
        }
    }
}

/// Argument parsing + mode dispatch (I/O separated from main for testability).
fn run(args: &[String], out: &mut dyn Write) -> Result<i32, String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        writeln!(out, "{}", usage()).map_err(|e| e.to_string())?;
        return Ok(0);
    }

    let mut cwd = env::current_dir().map_err(|e| format!("cwd: {e}"))?;
    let mut prompt: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--cwd" => {
                i += 1;
                let dir = args
                    .get(i)
                    .ok_or_else(|| "--cwd needs a directory".to_string())?;
                cwd = PathBuf::from(dir);
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

    let config = AppConfig::load()?;
    let home =
        config::slimcode_home().ok_or_else(|| "cannot determine home directory".to_string())?;
    let store = SessionStore::new(home.join("sessions"));
    let history = HistoryStore::new(home.join("history.json"));

    match prompt {
        Some(p) => run_once(&p, &cwd, config, &store, out),
        None => run_repl(&cwd, config, &store, &history, out),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_prints_usage() {
        let mut buf = Vec::new();
        let code = run(&["--help".to_string()], &mut buf).unwrap();
        assert_eq!(code, 0);
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("USAGE"));
    }

    #[test]
    fn no_args_dispatch_to_repl_needs_config() {
        // Without DASHSCOPE_API_KEY set, setup fails before the REPL loop.
        let old = env::var_os(config::ENV_API_KEY);
        unsafe { env::remove_var(config::ENV_API_KEY) };
        let old_home = env::var_os(config::ENV_HOME);
        unsafe { env::set_var(config::ENV_HOME, std::env::temp_dir()) };
        let mut buf = Vec::new();
        let err = run(&[], &mut buf).unwrap_err();
        assert!(err.contains(config::ENV_API_KEY), "err: {err}");
        // restore
        match old {
            Some(v) => unsafe { env::set_var(config::ENV_API_KEY, v) },
            None => unsafe { env::remove_var(config::ENV_API_KEY) },
        }
        match old_home {
            Some(v) => unsafe { env::set_var(config::ENV_HOME, v) },
            None => unsafe { env::remove_var(config::ENV_HOME) },
        }
    }

    #[test]
    fn structural_line_after_text_starts_new_row() {
        use slimcode_agent::agent::{AgentEvent, Delta, StopReason};
        let events = vec![
            AgentEvent::Stream(Delta::Text("answer".to_string())),
            AgentEvent::Stop(StopReason::Completed),
        ];
        let mut buf = Vec::new();
        render_events(&events, &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("answer\n✓ done"), "got: {s:?}");
    }

    #[test]
    fn text_trailing_newline_not_duplicated() {
        use slimcode_agent::agent::{AgentEvent, Delta, StopReason};
        let events = vec![
            AgentEvent::Stream(Delta::Text("done\n".to_string())),
            AgentEvent::Stop(StopReason::Completed),
        ];
        let mut buf = Vec::new();
        render_events(&events, &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s, "done\n✓ done\n");
    }
}
