//! Line-based REPL (grilling Q6+B): a prompt per line, `/` commands for
//! control. Input history (`/history`, `/!!`, `/!N`) and multi-line prompts
//! (trailing `\` continuation) per ticket 06. No readline dependency; a
//! command can be pasted as one line (see ADR-0001: no raw mode, no
//! Shift+Enter).

use std::io::{BufRead, Write};

use slimcode_agent::agent::Tool;
use slimcode_agent::session::{Message, Role};
use slimcode_ai::BailianProvider;

use crate::history::{HISTORY_DISPLAY, HistoryStore};
use crate::render;
use crate::session::{SessionStore, infer_title};
use slimcode_commands::{COMMANDS, suggest};

/// What a line of REPL input means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    Empty,
    /// A command like `/help` (unknown commands carry their raw text).
    Command(String),
    /// A normal prompt to run.
    Prompt(String),
}

/// Classify a raw input line (pure, testable).
pub fn classify(line: &str) -> Input {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Input::Empty;
    }
    if trimmed.starts_with('/') {
        return Input::Command(trimmed.to_string());
    }
    Input::Prompt(trimmed.to_string())
}

/// Build the message history for a new prompt: seed the system prompt on a
/// fresh session, then append the user message. Pure and testable.
pub fn messages_for_prompt(mut messages: Vec<Message>, prompt: &str) -> Vec<Message> {
    if messages.is_empty() {
        messages.push(Message::text(Role::System, crate::SYSTEM_PROMPT));
    }
    messages.push(Message::text(Role::User, prompt));
    messages
}

/// A line continues a multi-line prompt when its last non-whitespace char is `\`.
pub fn is_continuation(line: &str) -> bool {
    line.trim_end().ends_with('\\')
}

/// Remove the trailing continuation marker (the `\`), trimming trailing
/// whitespace from the remaining content. Caller must ensure `is_continuation`
/// is true for `line`.
pub fn strip_continuation(line: &str) -> String {
    let t = line.trim_end();
    t[..t.len() - 1].trim_end().to_string()
}

/// Accumulate a line into a multi-line prompt. Returns `Some(prompt)` when
/// `line` commits the prompt (it does not end in a continuation marker), and
/// `None` when the prompt is still being continued. Also returns `Some(line)`
/// immediately for a single-line prompt.
pub fn accumulate(pending: &mut String, line: &str) -> Option<String> {
    let cont = is_continuation(line);
    if cont {
        let content = strip_continuation(line);
        if !content.is_empty() {
            if !pending.is_empty() {
                pending.push('\n');
            }
            pending.push_str(&content);
        }
        None
    } else {
        let content = line.trim_end();
        if !content.is_empty() {
            if !pending.is_empty() {
                pending.push('\n');
            }
            pending.push_str(content);
        }
        Some(std::mem::take(pending))
    }
}

/// Parse a replay arg: `!!` (most recent) or `!N` (the N-th most recent).
pub fn parse_replay(arg: &str) -> Result<usize, String> {
    let s = arg.trim();
    if s == "!!" {
        return Ok(1);
    }
    let n = s
        .strip_prefix('!')
        .ok_or_else(|| format!("unknown replay: {arg:?}"))?;
    n.parse::<usize>()
        .map_err(|_| format!("bad replay index: {arg:?}"))
}

/// Map a replay number (1 = newest) to the entry, if it exists.
pub fn resolve_replay_index(entries: &[String], n: usize) -> Option<&str> {
    if n == 0 {
        return None;
    }
    let idx = entries.len().checked_sub(n)?;
    entries.get(idx).map(|s| s.as_str())
}

/// Render history entries for `/history`, newest first, numbered 1 = newest,
/// limited to `limit` entries. Multi-line / over-long entries collapse to a
/// single truncated line.
pub fn render_history(entries: &[String], limit: usize) -> Vec<String> {
    let total = entries.len();
    let start = total.saturating_sub(limit);
    entries
        .iter()
        .enumerate()
        .skip(start)
        .map(|(i, e)| format!("{}: {}", total - i, summarize_entry(e)))
        .collect()
}

/// Collapse an entry to a single display line (first line, truncated).
fn summarize_entry(entry: &str) -> String {
    const MAX: usize = 48;
    let first = entry.lines().next().unwrap_or("").trim_end();
    let truncated: String = first.chars().take(MAX).collect();
    let is_truncated = truncated.chars().count() < first.chars().count()
        || first.is_empty()
        || entry.lines().count() > 1;
    if is_truncated {
        format!("{truncated}…")
    } else {
        truncated
    }
}

/// Shared REPL dependencies, bundled to keep signatures slim. Lives for the
/// whole REPL session; `session`/`out`/`input` stay per-loop state.
pub struct ReplCtx<'a> {
    pub provider: &'a mut BailianProvider,
    pub tools: &'a [Tool],
    pub store: &'a SessionStore,
    pub history: &'a HistoryStore,
}

/// The interactive loop. `cwd` is the working directory for the tools.
/// Returns the final session so callers can persist or inspect it after exit.
pub fn run(
    ctx: &mut ReplCtx<'_>,
    cwd: &std::path::Path,
    mut session: slimcode_agent::session::Session,
    out: &mut dyn Write,
    input: &mut dyn BufRead,
) -> Result<slimcode_agent::session::Session, String> {
    writeln!(out, "slimcode REPL — cwd: {}", cwd.display()).map_err(|e| e.to_string())?;
    let banner = COMMANDS
        .iter()
        .map(|c| c.usage)
        .collect::<Vec<_>>()
        .join("  ");
    writeln!(out, "  {banner}").map_err(|e| e.to_string())?;

    let mut pending = String::new();
    loop {
        write!(out, "slimcode> ").map_err(|e| e.to_string())?;
        out.flush().map_err(|e| e.to_string())?;
        let mut line = String::new();
        if input.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            writeln!(out).map_err(|e| e.to_string())?;
            break; // EOF
        }

        // A line ending in `\` continues a multi-line prompt (first or middle
        // line). Every continuation line is prompt content — even one starting
        // with `/` — and its leading whitespace is preserved.
        if is_continuation(&line) || !pending.is_empty() {
            if let Some(prompt) = accumulate(&mut pending, &line) {
                submit_prompt(ctx, &mut session, &prompt, true, out)?;
            }
            continue;
        }

        match classify(&line) {
            Input::Empty => continue,
            Input::Command(cmd) => {
                let (name, arg) = match cmd.split_once(char::is_whitespace) {
                    Some((n, a)) => (n, Some(a.trim())),
                    None => (cmd.as_str(), None),
                };
                match name {
                    "/help" => {
                        writeln!(out, "commands:").map_err(|e| e.to_string())?;
                        let width = COMMANDS
                            .iter()
                            .map(|c| c.usage.chars().count())
                            .max()
                            .unwrap_or(0);
                        for c in COMMANDS {
                            let alias = if c.aliases.is_empty() {
                                String::new()
                            } else {
                                format!(" (alias: {})", c.aliases.join(", "))
                            };
                            writeln!(out, "  {:<width$}  {}{}", c.usage, c.description, alias)
                                .map_err(|e| e.to_string())?;
                        }
                        writeln!(
                            out,
                            "multi-line: end a line with \\ to continue; a blank line submits"
                        )
                        .map_err(|e| e.to_string())?;
                    }
                    "/exit" | "/quit" => break,
                    "/new" => {
                        session = new_session(ctx.store);
                        writeln!(out, "new session: {}", session.id).map_err(|e| e.to_string())?;
                    }
                    "/sessions" => {
                        for id in ctx.store.list().map_err(|e| e.to_string())? {
                            writeln!(out, "  {id}").map_err(|e| e.to_string())?;
                        }
                    }
                    "/usage" => {
                        let u = ctx.provider.total_usage;
                        writeln!(out, "{}", render::render_usage(&u)).map_err(|e| e.to_string())?;
                    }
                    "/save" => {
                        let path = ctx.store.save(&session).map_err(|e| e.to_string())?;
                        writeln!(out, "saved: {}", path.display()).map_err(|e| e.to_string())?;
                    }
                    "/load" | "/resume" => {
                        let id = arg
                            .filter(|a| !a.is_empty())
                            .ok_or_else(|| format!("{name} needs a session id"))?;
                        session = ctx.store.load(id).map_err(|e| e.to_string())?;
                        writeln!(out, "loaded session: {}", session.id)
                            .map_err(|e| e.to_string())?;
                        if let Some(t) = &session.title {
                            writeln!(out, "  title: {t}").map_err(|e| e.to_string())?;
                        }
                    }
                    "/history" => match ctx.history.load() {
                        Ok(entries) => {
                            for entry in render_history(&entries, HISTORY_DISPLAY) {
                                writeln!(out, "  {entry}").map_err(|e| e.to_string())?;
                            }
                        }
                        Err(e) => {
                            writeln!(out, "{e}").map_err(|e| e.to_string())?;
                        }
                    },
                    "/!!" => replay_and_report(ctx, &mut session, 1, out)?,
                    name if name.starts_with("/!") => match parse_replay(&name[1..]) {
                        Ok(n) => replay_and_report(ctx, &mut session, n, out)?,
                        Err(e) => writeln!(out, "{e}").map_err(|e| e.to_string())?,
                    },
                    other => {
                        writeln!(out, "unknown command: {other}").map_err(|e| e.to_string())?;
                        let matches = suggest(other);
                        if matches.is_empty() {
                            writeln!(out, "  run /help to list commands")
                                .map_err(|e| e.to_string())?;
                        } else {
                            let names: Vec<_> = matches.iter().map(|c| c.usage).collect();
                            writeln!(out, "  did you mean: {}", names.join(", "))
                                .map_err(|e| e.to_string())?;
                        }
                    }
                }
            }
            Input::Prompt(p) => {
                submit_prompt(ctx, &mut session, &p, true, out)?;
            }
        }
    }
    Ok(session)
}

/// Run a prompt as a fresh turn: seed the system prompt on a fresh session,
/// infer the title, record the prompt in input history immediately, then run
/// the turn and persist the session. History is written before the turn so a
/// failed turn still records what was typed (spec US12/US15); a history write
/// failure is non-fatal.
fn submit_prompt(
    ctx: &mut ReplCtx<'_>,
    session: &mut slimcode_agent::session::Session,
    prompt: &str,
    record: bool,
    out: &mut dyn Write,
) -> Result<(), String> {
    let mut messages = std::mem::take(&mut session.messages);
    messages = messages_for_prompt(messages, prompt);
    if session.title.is_none() {
        session.title = infer_title(&messages);
    }
    if record && let Err(e) = ctx.history.append(prompt) {
        writeln!(out, "history: {e}").map_err(|e| e.to_string())?;
    }
    let result = super::run_turn(ctx.provider, ctx.tools, messages, out)?;
    session.messages = result;
    ctx.store.save(session).map_err(|e| e.to_string())?;
    Ok(())
}

/// Re-run a stored input history entry as a fresh prompt (without re-recording
/// it in input history). `n` is 1-based, newest first.
fn replay_prompt(
    ctx: &mut ReplCtx<'_>,
    session: &mut slimcode_agent::session::Session,
    n: usize,
    out: &mut dyn Write,
) -> Result<(), String> {
    let entries = ctx.history.load().map_err(|e| e.to_string())?;
    let prompt =
        resolve_replay_index(&entries, n).ok_or_else(|| format!("no history entry {n}"))?;
    submit_prompt(ctx, session, prompt, false, out)
}

/// Re-run history entry `n`, printing (not propagating) any error so the REPL
/// keeps going.
fn replay_and_report(
    ctx: &mut ReplCtx<'_>,
    session: &mut slimcode_agent::session::Session,
    n: usize,
    out: &mut dyn Write,
) -> Result<(), String> {
    if let Err(e) = replay_prompt(ctx, session, n, out) {
        writeln!(out, "{e}").map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Build a fresh session with a new id and timestamp.
pub fn new_session(store: &SessionStore) -> slimcode_agent::session::Session {
    slimcode_agent::session::Session {
        id: store.new_id(),
        created_at: crate::session::now_rfc3339(),
        messages: Vec::new(),
        title: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::unique_temp_dir;
    use slimcode_agent::session::{Message, Role, Session};
    use std::fs;

    #[test]
    fn classify_distinguishes_kinds() {
        assert_eq!(classify("   "), Input::Empty);
        assert_eq!(classify(""), Input::Empty);
        assert_eq!(classify("/exit"), Input::Command("/exit".to_string()));
        assert_eq!(classify("  /help  "), Input::Command("/help".to_string()));
        assert_eq!(classify("hello"), Input::Prompt("hello".to_string()));
        assert_eq!(
            classify("  hi there  "),
            Input::Prompt("hi there".to_string())
        );
    }

    #[test]
    fn fresh_prompt_seeds_system_message() {
        let messages = messages_for_prompt(Vec::new(), "hello");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, Role::System);
        assert_eq!(messages[0].text_content(), crate::SYSTEM_PROMPT);
        assert_eq!(messages[1].role, Role::User);
        assert_eq!(messages[1].text_content(), "hello");
    }

    #[test]
    fn continued_prompt_does_not_reseed_system() {
        let history = vec![Message::text(Role::System, crate::SYSTEM_PROMPT)];
        let messages = messages_for_prompt(history, "again");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, Role::System);
        assert_eq!(messages[1].text_content(), "again");
    }

    /// Unique temp dir per test.
    fn temp_dir() -> std::path::PathBuf {
        unique_temp_dir("slimcode-repl-test")
    }

    /// A provider configured with a dummy key (client construction needs no
    /// network); never used to call the endpoint in these tests.
    fn dummy_provider() -> BailianProvider {
        BailianProvider::new(slimcode_ai::BailianConfig::new(
            "sk-test",
            "https://example.invalid/compatible-mode/v1",
            "qwen-plus",
        ))
        .unwrap()
    }

    #[test]
    fn is_continuation_detects_trailing_backslash() {
        assert!(is_continuation("foo \\"));
        assert!(is_continuation("foo \\  ")); // trailing space after `\`
        assert!(is_continuation("\\"));
        assert!(!is_continuation("foo"));
        assert!(!is_continuation(""));
        assert!(!is_continuation("   "));
    }

    #[test]
    fn strip_continuation_removes_marker() {
        assert_eq!(strip_continuation("foo \\"), "foo");
        assert_eq!(strip_continuation("foo \\  "), "foo");
        assert_eq!(strip_continuation("\\"), "");
    }

    #[test]
    fn accumulate_builds_multiline_prompt() {
        let mut pending = String::new();
        // Start and continue on `\` lines.
        assert_eq!(accumulate(&mut pending, "fn main() { \\"), None);
        assert_eq!(pending, "fn main() {".to_string());
        assert_eq!(accumulate(&mut pending, "    ok(); \\"), None);
        assert_eq!(pending, "fn main() {\n    ok();".to_string());
        // A non-`\` line commits the whole prompt.
        assert_eq!(
            accumulate(&mut pending, "}"),
            Some("fn main() {\n    ok();\n}".to_string())
        );
        assert!(pending.is_empty());
    }

    #[test]
    fn accumulate_preserves_leading_whitespace() {
        let mut pending = String::new();
        // First line keeps its indentation (the bug this guards: classify()
        // used to trim it before accumulate ever saw it).
        assert_eq!(accumulate(&mut pending, "    fn main() { \\"), None);
        assert_eq!(pending, "    fn main() {".to_string());
        assert_eq!(
            accumulate(&mut pending, "}"),
            Some("    fn main() {\n}".to_string())
        );
    }

    #[test]
    fn accumulate_treats_slash_line_as_content() {
        // Inside a continuation, a line starting with `/` is prompt content.
        let mut pending = String::new();
        assert_eq!(accumulate(&mut pending, "write a file \\"), None);
        assert_eq!(
            accumulate(&mut pending, "/tmp/x"),
            Some("write a file\n/tmp/x".to_string())
        );
    }

    #[test]
    fn accumulate_single_line_prompt_commits_immediately() {
        let mut pending = String::new();
        assert_eq!(accumulate(&mut pending, "hello"), Some("hello".to_string()));
        assert!(pending.is_empty());
    }

    #[test]
    fn accumulate_blank_line_commits_prompt() {
        let mut pending = String::new();
        assert_eq!(accumulate(&mut pending, "a \\"), None);
        assert_eq!(accumulate(&mut pending, ""), Some("a".to_string()));
    }

    #[test]
    fn parse_replay_handles_bang_commands() {
        assert_eq!(parse_replay("!!"), Ok(1));
        assert_eq!(parse_replay("!3"), Ok(3));
        assert!(parse_replay("!").is_err());
        assert!(parse_replay("!abc").is_err());
        assert!(parse_replay("!0").is_ok()); // resolved later; 0 maps to nothing
    }

    #[test]
    fn resolve_replay_index_newest_first() {
        let entries = vec!["one".to_string(), "two".to_string(), "three".to_string()];
        assert_eq!(resolve_replay_index(&entries, 1), Some("three"));
        assert_eq!(resolve_replay_index(&entries, 3), Some("one"));
        assert_eq!(resolve_replay_index(&entries, 4), None);
        assert_eq!(resolve_replay_index(&entries, 0), None);
    }

    #[test]
    fn render_history_newest_first_numbered() {
        let entries = vec!["one".to_string(), "two".to_string(), "three".to_string()];
        assert_eq!(
            render_history(&entries, 20),
            vec!["3: one", "2: two", "1: three"]
        );
    }

    #[test]
    fn render_history_respects_display_limit() {
        let entries: Vec<String> = (0..30).map(|i| format!("p{i}")).collect();
        let lines = render_history(&entries, 20);
        assert_eq!(lines.len(), 20);
        // Newest 20 shown (p10..=p29), newest first, 1 = newest.
        assert_eq!(lines.first().unwrap(), "20: p10");
        assert_eq!(lines.last().unwrap(), "1: p29");
    }

    #[test]
    fn render_history_collapses_multiline_and_long() {
        let entries = vec!["first line\nsecond line".to_string()];
        assert_eq!(render_history(&entries, 20), vec!["1: first line…"]);
        let long = vec!["x".repeat(80).to_string()];
        let lines = render_history(&long, 20);
        assert!(lines[0].starts_with("1: "), "got: {lines:?}");
        assert!(lines[0].ends_with('…'), "got: {lines:?}");
        assert!(lines[0].chars().count() <= 60, "got: {lines:?}");
    }

    #[test]
    fn history_command_lists_entries() {
        let dir = temp_dir();
        let store = SessionStore::new(&dir);
        let history = HistoryStore::new(dir.join("history.json"));
        history.append("one").unwrap();
        history.append("two").unwrap();

        let mut provider = dummy_provider();
        let tools: Vec<Tool> = Vec::new();
        let mut out = Vec::new();
        let mut input = "/history\n/exit\n".as_bytes();
        let mut ctx = ReplCtx {
            provider: &mut provider,
            tools: &tools,
            store: &store,
            history: &history,
        };
        run(&mut ctx, &dir, new_session(&store), &mut out, &mut input).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("1: two"), "got: {s}");
        assert!(s.contains("2: one"), "got: {s}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn replay_out_of_range_errors_and_continues() {
        let dir = temp_dir();
        let store = SessionStore::new(&dir);
        let history = HistoryStore::new(dir.join("history.json"));

        let mut provider = dummy_provider();
        let tools: Vec<Tool> = Vec::new();
        let mut out = Vec::new();
        let mut input = "/!99\n/exit\n".as_bytes();
        let mut ctx = ReplCtx {
            provider: &mut provider,
            tools: &tools,
            store: &store,
            history: &history,
        };
        // Errors are surfaced to the output and the REPL continues.
        run(&mut ctx, &dir, new_session(&store), &mut out, &mut input).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("no history entry 99"), "got: {s}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn replay_empty_history_errors_and_continues() {
        let dir = temp_dir();
        let store = SessionStore::new(&dir);
        let history = HistoryStore::new(dir.join("history.json"));

        let mut provider = dummy_provider();
        let tools: Vec<Tool> = Vec::new();
        let mut out = Vec::new();
        let mut input = "/!!\n/exit\n".as_bytes();
        let mut ctx = ReplCtx {
            provider: &mut provider,
            tools: &tools,
            store: &store,
            history: &history,
        };
        run(&mut ctx, &dir, new_session(&store), &mut out, &mut input).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("no history entry 1"), "got: {s}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn replay_malformed_input_errors_and_continues() {
        // `/!` and `/!abc` must print an error and keep the REPL alive, not
        // terminate it (ticket 06: errors print-and-continue).
        let dir = temp_dir();
        let store = SessionStore::new(&dir);
        let history = HistoryStore::new(dir.join("history.json"));

        let mut provider = dummy_provider();
        let tools: Vec<Tool> = Vec::new();
        let mut out = Vec::new();
        let mut input = "/!abc\n/!\n/exit\n".as_bytes();
        let mut ctx = ReplCtx {
            provider: &mut provider,
            tools: &tools,
            store: &store,
            history: &history,
        };
        run(&mut ctx, &dir, new_session(&store), &mut out, &mut input).unwrap();
        let s = String::from_utf8(out).unwrap();
        // Both `/!abc` and `/!` fail number parsing -> "bad replay index",
        // printed twice; the REPL keeps going.
        assert_eq!(s.matches("bad replay index").count(), 2, "got: {s}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_history_errors_and_continues() {
        // A corrupt history.json must not kill the REPL.
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        let store = SessionStore::new(&dir);
        let history = HistoryStore::new(dir.join("history.json"));
        fs::write(dir.join("history.json"), "not json").unwrap();

        let mut provider = dummy_provider();
        let tools: Vec<Tool> = Vec::new();
        let mut out = Vec::new();
        let mut input = "/history\n/exit\n".as_bytes();
        let mut ctx = ReplCtx {
            provider: &mut provider,
            tools: &tools,
            store: &store,
            history: &history,
        };
        run(&mut ctx, &dir, new_session(&store), &mut out, &mut input).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("history.json"), "got: {s}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_input_line_is_ignored() {
        // A blank line outside a continuation is ignored (classify Empty).
        let dir = temp_dir();
        let store = SessionStore::new(&dir);
        let history = HistoryStore::new(dir.join("history.json"));

        let mut provider = dummy_provider();
        let tools: Vec<Tool> = Vec::new();
        let mut out = Vec::new();
        let mut input = "\n  \n/exit\n".as_bytes();
        let mut ctx = ReplCtx {
            provider: &mut provider,
            tools: &tools,
            store: &store,
            history: &history,
        };
        run(&mut ctx, &dir, new_session(&store), &mut out, &mut input).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_resumes_a_saved_session() {
        let dir = temp_dir();
        let store = SessionStore::new(&dir);
        let history = HistoryStore::new(dir.join("history.json"));
        let saved = Session {
            id: "repl-restore-test".to_string(),
            created_at: crate::session::now_rfc3339(),
            title: Some("restored".to_string()),
            messages: vec![
                Message::text(Role::System, "sys"),
                Message::text(Role::User, "old"),
            ],
        };
        store.save(&saved).unwrap();

        let mut provider = dummy_provider();
        let tools: Vec<Tool> = Vec::new();
        let mut out = Vec::new();
        let mut input = "/load repl-restore-test\n/exit\n".as_bytes();
        let mut ctx = ReplCtx {
            provider: &mut provider,
            tools: &tools,
            store: &store,
            history: &history,
        };
        let final_session = run(&mut ctx, &dir, new_session(&store), &mut out, &mut input).unwrap();

        assert_eq!(final_session.id, "repl-restore-test");
        assert_eq!(final_session.title.as_deref(), Some("restored"));
        assert_eq!(final_session.messages.len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_without_id_errors() {
        let dir = temp_dir();
        let store = SessionStore::new(&dir);
        let history = HistoryStore::new(dir.join("history.json"));
        let mut provider = dummy_provider();
        let tools: Vec<Tool> = Vec::new();
        let mut out = Vec::new();
        let mut input = "/load\n/exit\n".as_bytes();
        let mut ctx = ReplCtx {
            provider: &mut provider,
            tools: &tools,
            store: &store,
            history: &history,
        };
        let err = run(&mut ctx, &dir, new_session(&store), &mut out, &mut input).unwrap_err();
        assert!(err.contains("session id"), "err: {err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_command_suggests_candidates() {
        // A typo'd `/` command is met with a predictive hint, not a dead end.
        let dir = temp_dir();
        let store = SessionStore::new(&dir);
        let history = HistoryStore::new(dir.join("history.json"));
        let mut provider = dummy_provider();
        let tools: Vec<Tool> = Vec::new();
        let mut out = Vec::new();
        let mut input = "/his\n/exit\n".as_bytes();
        let mut ctx = ReplCtx {
            provider: &mut provider,
            tools: &tools,
            store: &store,
            history: &history,
        };
        run(&mut ctx, &dir, new_session(&store), &mut out, &mut input).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("unknown command: /his"), "got: {s}");
        assert!(s.contains("did you mean: /history"), "got: {s}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_command_no_match_hints_help() {
        // A completely unknown command points at `/help` instead.
        let dir = temp_dir();
        let store = SessionStore::new(&dir);
        let history = HistoryStore::new(dir.join("history.json"));
        let mut provider = dummy_provider();
        let tools: Vec<Tool> = Vec::new();
        let mut out = Vec::new();
        let mut input = "/zzz\n/exit\n".as_bytes();
        let mut ctx = ReplCtx {
            provider: &mut provider,
            tools: &tools,
            store: &store,
            history: &history,
        };
        run(&mut ctx, &dir, new_session(&store), &mut out, &mut input).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("unknown command: /zzz"), "got: {s}");
        assert!(s.contains("run /help to list commands"), "got: {s}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn bare_slash_suggests_every_command() {
        // A lone `/` is the broadest prediction: every command is suggested.
        let dir = temp_dir();
        let store = SessionStore::new(&dir);
        let history = HistoryStore::new(dir.join("history.json"));
        let mut provider = dummy_provider();
        let tools: Vec<Tool> = Vec::new();
        let mut out = Vec::new();
        let mut input = "/\n/exit\n".as_bytes();
        let mut ctx = ReplCtx {
            provider: &mut provider,
            tools: &tools,
            store: &store,
            history: &history,
        };
        run(&mut ctx, &dir, new_session(&store), &mut out, &mut input).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("did you mean: /help"), "got: {s}");
        assert!(s.contains("/load <id>"), "got: {s}");
        assert!(s.contains("/!N"), "got: {s}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn help_lists_registered_commands() {
        // `/help` is generated from the shared registry: names, usage and
        // descriptions all come through.
        let dir = temp_dir();
        let store = SessionStore::new(&dir);
        let history = HistoryStore::new(dir.join("history.json"));
        let mut provider = dummy_provider();
        let tools: Vec<Tool> = Vec::new();
        let mut out = Vec::new();
        let mut input = "/help\n/exit\n".as_bytes();
        let mut ctx = ReplCtx {
            provider: &mut provider,
            tools: &tools,
            store: &store,
            history: &history,
        };
        run(&mut ctx, &dir, new_session(&store), &mut out, &mut input).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("list commands"), "got: {s}");
        assert!(s.contains("/load <id>"), "got: {s}");
        assert!(s.contains("load a saved session"), "got: {s}");
        assert!(s.contains("alias: /resume"), "got: {s}");
        assert!(s.contains("alias: /quit"), "got: {s}");
        let _ = fs::remove_dir_all(&dir);
    }
}
