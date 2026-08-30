//! slimcode — a working Rust coding-agent CLI (v1).
//!
//! Two modes (grilling Q6+B):
//! - `slimcode "<prompt>"` — non-interactive: run one prompt to completion in
//!   the current directory (or `--cwd`), stream events, print token usage, and
//!   save the session.
//! - `slimcode` — line-based REPL that keeps a session and persists it after
//!   every turn.
//!
//! Config (Q9): CLI (`--base-url` / `--model`) overrides env
//! (`SLIMCODE_AI_BASE_URL` / `SLIMCODE_AI_MODEL`), which overrides the file
//! (`~/.slimcode/config.toml`), which overrides defaults; the API key comes
//! only from `DASHSCOPE_API_KEY`.

mod render;
mod repl;

use std::env;
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};

use slimcode_agent::agent::{Message, RunConfig, Tool};
use slimcode_ai::{BailianConfig, BailianProvider};
use slimcode_common::config::{self, Overrides};
use slimcode_common::history::HistoryStore;
use slimcode_common::session::{SessionStore, infer_title};
use slimcode_common::skills::{Skill, SkillStore};
use slimcode_common::tools;

/// Base system prompt grounding the agent in its tools and working directory.
/// Skill descriptions are appended on top by [`build_system_prompt`].
const BASE_SYSTEM_PROMPT: &str = concat!(
    "You are slimcode, a coding agent that works in a repository directory.\n\n",
    "## Tools\n\n",
    "You have these tools: `read`, `write`, `edit`, `bash`, `grep`, `find`, `ls`.\n\n",
    "## Working style\n\n",
    "- Plan with the tools available: inspect files before editing, run commands ",
    "to verify, and complete the user's task.\n",
    "- Keep answers concise.\n",
    "- When a tool fails, read the error and retry with a corrected approach."
);

/// Build the system prompt for a fresh session: the base grounding plus a list
/// of auto-invokable skills (those without `disable-model-invocation: true`).
/// Skills marked `disable-model-invocation` stay out of the system prompt and
/// are only reachable through an explicit `/name` trigger.
pub(crate) fn build_system_prompt(skills: &[Skill]) -> String {
    let mut prompt = BASE_SYSTEM_PROMPT.to_string();
    let auto: Vec<&Skill> = skills
        .iter()
        .filter(|s| !s.disable_model_invocation)
        .collect();
    if !auto.is_empty() {
        prompt.push_str("\n\n## Available skills\n\n");
        prompt.push_str("Enter the `/name` as a command to apply it:\n\n");
        for s in auto {
            prompt.push_str(&format!(
                "- `/{name}` — {desc}\n",
                name = s.name,
                desc = s.description
            ));
        }
    }
    prompt
}

fn usage() -> String {
    format!(
        "slimcode — Rust coding agent (v1)\n\n\
         USAGE:\n  \
         slimcode \"<prompt>\"             run one prompt, then exit\n  \
         slimcode --cwd <dir> \"<prompt>\"  run one prompt in <dir>\n  \
         slimcode                       start the interactive REPL\n  \
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
fn setup(cwd: &Path, config: BailianConfig) -> Result<(BailianProvider, Vec<Tool>), String> {
    let provider = BailianProvider::new(config)?;
    let tools = tools::build_tools(cwd);
    Ok((provider, tools))
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
    let (mut provider, tools) = setup(cwd, config)?;
    let mut session = repl::new_session(store);
    session.title = infer_title(&[Message::text(slimcode_agent::session::Role::User, prompt)]);
    let messages = vec![
        Message::text(
            slimcode_agent::session::Role::System,
            build_system_prompt(skills),
        ),
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
    config: BailianConfig,
    store: &SessionStore,
    history: &HistoryStore,
    skills: &SkillStore,
    out: &mut dyn Write,
) -> Result<i32, String> {
    let (mut provider, tools) = setup(cwd, config)?;
    let session = repl::new_session(store);
    let mut ctx = repl::ReplCtx {
        provider: &mut provider,
        tools: &tools,
        store,
        history,
        skills,
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

    let parsed = parse_args(args)?;
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
        None => run_repl(&cwd, app_config, &store, &history, &skills_store, out),
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

    #[test]
    fn help_prints_usage() {
        let mut buf = Vec::new();
        let code = run(&["--help".to_string()], &mut buf).unwrap();
        assert_eq!(code, 0);
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("USAGE"));
    }

    fn skill(name: &str, desc: &str, disable: bool) -> Skill {
        Skill {
            name: name.to_string(),
            description: desc.to_string(),
            disable_model_invocation: disable,
            body: String::new(),
            scope: slimcode_common::skills::SkillScope::User,
        }
    }

    #[test]
    fn system_prompt_advertises_only_auto_invokable_skills() {
        let skills = vec![
            skill("auto", "runs automatically", false),
            skill("manual", "only on demand", true),
        ];
        let prompt = build_system_prompt(&skills);
        assert!(prompt.contains("## Available skills"), "got: {prompt}");
        assert!(
            prompt.contains("- `/auto` — runs automatically"),
            "got: {prompt}"
        );
        assert!(!prompt.contains("manual"), "got: {prompt}");
        assert!(!prompt.contains("only on demand"));
    }

    #[test]
    fn system_prompt_skills_section_is_markdown() {
        let skills = vec![
            skill("auto", "runs automatically", false),
            skill("hist", "history-ish", false),
        ];
        let prompt = build_system_prompt(&skills);
        // Heading + blank line + instruction + blank line + bullet list.
        assert!(prompt.contains("## Available skills\n\n"), "got: {prompt}");
        assert!(
            prompt.contains("Enter the `/name` as a command to apply it:\n\n"),
            "got: {prompt}"
        );
        assert!(
            prompt.contains("- `/hist` — history-ish\n"),
            "got: {prompt}"
        );
        // The base grounding (markdown) is still present before the skills
        // heading.
        assert!(prompt.contains("You are slimcode"), "got: {prompt}");
    }

    #[test]
    fn base_system_prompt_is_markdown_structured() {
        let prompt = build_system_prompt(&[]);
        // Role line, then markdown sections for tools and working style.
        assert!(
            prompt.contains(
                "You are slimcode, a coding agent that works in a repository directory.\n\n"
            ),
            "got: {prompt}"
        );
        assert!(prompt.contains("## Tools\n\n"), "got: {prompt}");
        // Tool names are wrapped in code spans.
        assert!(prompt.contains("`read`"), "got: {prompt}");
        assert!(prompt.contains("`bash`"), "got: {prompt}");
        assert!(prompt.contains("## Working style\n\n"), "got: {prompt}");
        // Working style is a bullet list.
        assert!(
            prompt.contains("- Keep answers concise.\n"),
            "got: {prompt}"
        );
    }

    #[test]
    fn system_prompt_without_skills_has_no_skills_section() {
        let prompt = build_system_prompt(&[]);
        assert!(!prompt.contains("Available skills"));
        assert!(prompt.contains("read"));
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
