//! Line-based REPL (grilling Q6+B): a prompt per line, `/` commands for
//! control. No readline dependency in v1; history and multi-line paste are out
//! of scope (a command can be pasted as one line).

use std::io::{BufRead, Write};

use slimcode_agent::agent::Tool;
use slimcode_agent::session::{Message, Role};
use slimcode_ai::BailianProvider;

use crate::render;
use crate::session::{SessionStore, infer_title};

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

/// The interactive loop. `cwd` is the working directory for the tools.
/// Returns the final session so callers can persist or inspect it after exit.
pub fn run(
    provider: &mut BailianProvider,
    tools: &[Tool],
    cwd: &std::path::Path,
    store: &SessionStore,
    mut session: slimcode_agent::session::Session,
    out: &mut dyn Write,
    input: &mut dyn BufRead,
) -> Result<slimcode_agent::session::Session, String> {
    writeln!(out, "slimcode REPL — cwd: {}", cwd.display()).map_err(|e| e.to_string())?;
    writeln!(out, "  /help  /new  /load  /sessions  /usage  /save  /exit")
        .map_err(|e| e.to_string())?;

    loop {
        write!(out, "slimcode> ").map_err(|e| e.to_string())?;
        out.flush().map_err(|e| e.to_string())?;
        let mut line = String::new();
        if input.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            writeln!(out).map_err(|e| e.to_string())?;
            break; // EOF
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
                        writeln!(
                            out,
                            "commands: /help /new /load <id> /sessions /usage /save /exit"
                        )
                        .map_err(|e| e.to_string())?;
                    }
                    "/exit" | "/quit" => break,
                    "/new" => {
                        session = new_session(store);
                        writeln!(out, "new session: {}", session.id).map_err(|e| e.to_string())?;
                    }
                    "/sessions" => {
                        for id in store.list().map_err(|e| e.to_string())? {
                            writeln!(out, "  {id}").map_err(|e| e.to_string())?;
                        }
                    }
                    "/usage" => {
                        let u = provider.total_usage;
                        writeln!(out, "{}", render::render_usage(&u)).map_err(|e| e.to_string())?;
                    }
                    "/save" => {
                        let path = store.save(&session).map_err(|e| e.to_string())?;
                        writeln!(out, "saved: {}", path.display()).map_err(|e| e.to_string())?;
                    }
                    "/load" | "/resume" => {
                        let id = arg
                            .filter(|a| !a.is_empty())
                            .ok_or_else(|| format!("{name} needs a session id"))?;
                        session = store.load(id).map_err(|e| e.to_string())?;
                        writeln!(out, "loaded session: {}", session.id)
                            .map_err(|e| e.to_string())?;
                        if let Some(t) = &session.title {
                            writeln!(out, "  title: {t}").map_err(|e| e.to_string())?;
                        }
                    }
                    other => {
                        writeln!(out, "unknown command: {other}").map_err(|e| e.to_string())?;
                    }
                }
            }
            Input::Prompt(p) => {
                let mut messages = std::mem::take(&mut session.messages);
                messages = messages_for_prompt(messages, &p);
                if session.title.is_none() {
                    session.title = infer_title(&messages);
                }
                let result = super::run_turn(provider, tools, messages, out)?;
                session.messages = result;
                store.save(&session).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(session)
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
    fn load_resumes_a_saved_session() {
        let dir = temp_dir();
        let store = SessionStore::new(&dir);
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
        let final_session = run(
            &mut provider,
            &tools,
            &dir,
            &store,
            new_session(&store),
            &mut out,
            &mut input,
        )
        .unwrap();

        assert_eq!(final_session.id, "repl-restore-test");
        assert_eq!(final_session.title.as_deref(), Some("restored"));
        assert_eq!(final_session.messages.len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_without_id_errors() {
        let dir = temp_dir();
        let store = SessionStore::new(&dir);
        let mut provider = dummy_provider();
        let tools: Vec<Tool> = Vec::new();
        let mut out = Vec::new();
        let mut input = "/load\n/exit\n".as_bytes();
        let err = run(
            &mut provider,
            &tools,
            &dir,
            &store,
            new_session(&store),
            &mut out,
            &mut input,
        )
        .unwrap_err();
        assert!(err.contains("session id"), "err: {err}");
        let _ = fs::remove_dir_all(&dir);
    }
}
