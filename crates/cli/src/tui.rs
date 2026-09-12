//! The TUI's entry point: process lifecycle (raw mode, alternate screen, the
//! terminal title, the exit code) and the application handler the terminal
//! library drives (ADR-0013).
//!
//! Everything that decides what should happen lives here: provider and tool
//! construction, the session store, input history, skills, context files, the
//! environment, command semantics, the per-turn session writes and the
//! cancelled/errored turn closing. The library keeps only what exists to keep
//! the frame loop responsive and knows the app through two vocabularies:
//! [`Effect`]s out, [`RenderItem`]s in.

use std::io::stdout;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, SetTitle, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use slimcode_ai::BailianConfig;
use slimcode_app::context::{ContextBuilder, Environment, skill_loaded_in};
use slimcode_app::context_files::ContextFile;
use slimcode_app::history::{HISTORY_DISPLAY, HistoryStore, render_history, resolve_replay_index};
use slimcode_app::render::usage_summary;
use slimcode_app::session::{SessionStore, infer_title};
use slimcode_app::skills::{
    Skill, SkillScope, SkillStore, find_skill, is_builtin_command, parse_install_args, skill_prompt,
};
use slimcode_core::agent::{CancelToken, Provider, RunConfig, StopReason, Tool};
use slimcode_core::session::{AgentMessage, MessageStopReason, Role, Session};
use slimcode_tui::app::{App, Effect};
use slimcode_tui::git::{current_branch, terminal_title};
use slimcode_tui::handler::{
    CompletionItem, CompletionProvider, ControlFlow, Prompt, TurnReport, UiHandler,
};
use slimcode_tui::render::RenderItem;

use crate::render::TuiAdapter;

/// Enter the TUI: build the runtime, take over the terminal, run the frame
/// loop, and restore the terminal on every exit path.
#[allow(clippy::too_many_arguments)]
pub fn run(
    cwd: &Path,
    config: BailianConfig,
    store: &SessionStore,
    history: &HistoryStore,
    skills: &SkillStore,
    context_files: &[ContextFile],
    environment: Environment,
) -> Result<i32, String> {
    let model = config.model.clone();
    // One token shared by the cancellable tool set and every turn: Esc cancels
    // whichever phase the run is in.
    let cancel = CancelToken::new();
    let (provider, tools) = slimcode_app::setup::setup_with_cancel(cwd, config, &cancel)?;
    let session = store.new_session();

    // The `/`-candidate pool (ADR-0014 D3): the command registry plus the
    // skills snapshot, shared with the handler so `/install-skill` refreshes
    // it without a restart.
    let completions = CliCompletions::new(skills.list().unwrap_or_default());
    let mut app = App::new(
        cwd.display().to_string(),
        session.id.clone(),
        model,
        slimcode_tui::VERSION,
        Box::new(completions.clone()),
    );
    app.set_history(history.load().unwrap_or_default());
    app.apply(RenderItem::Branch(current_branch(cwd)));

    let mut handler = TuiSession {
        cwd: cwd.to_path_buf(),
        store,
        history,
        skills,
        context_files: context_files.to_vec(),
        environment,
        completions,
        inner: Mutex::new(Inner {
            session,
            provider: Some(Box::new(provider)),
            tools,
            cancel,
        }),
    };

    // The terminal belongs to the CLI: raw mode, the alternate screen and the
    // title are all set up here, and the panic hook restores them.
    enable_raw_mode().map_err(|e| format!("raw mode: {e}"))?;
    execute!(stdout(), EnterAlternateScreen).map_err(|e| format!("alternate screen: {e}"))?;
    install_panic_hook();
    let mut terminal = match Terminal::new(CrosstermBackend::new(stdout())) {
        Ok(terminal) => terminal,
        Err(e) => {
            restore_terminal();
            return Err(e.to_string());
        }
    };
    if let Err(e) = terminal.hide_cursor() {
        restore_terminal();
        return Err(e.to_string());
    }
    set_title(&handler.session_id(), cwd);

    let result = slimcode_tui::run(&mut terminal, app, &mut handler);
    restore_terminal();
    result.map(|()| 0)
}

/// Leave raw mode and the alternate screen. Best-effort: the process is
/// exiting the TUI either way, so failures are swallowed.
fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(stdout(), LeaveAlternateScreen);
}

/// Restore the terminal when the TUI panics, so the shell is usable again.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        previous(info);
    }));
}

/// Set the OSC 0 terminal title. Best-effort: a title failure must not kill
/// the TUI.
fn set_title(session_id: &str, cwd: &Path) {
    let _ = execute!(stdout(), SetTitle(terminal_title(session_id, cwd)));
}

/// The CLI's `/`-candidate pool (ADR-0014 D3): the shared command registry
/// plus the skills snapshot the handler refreshes on `/install-skill`.
#[derive(Clone)]
struct CliCompletions {
    skills: std::sync::Arc<Mutex<Vec<Skill>>>,
}

impl CliCompletions {
    fn new(skills: Vec<Skill>) -> Self {
        Self {
            skills: std::sync::Arc::new(Mutex::new(skills)),
        }
    }

    /// Replace the skills half of the pool, so the popup, did-you-mean and
    /// dispatch see a freshly installed skill without a restart.
    fn set_skills(&self, skills: Vec<Skill>) {
        *self.skills.lock().expect("completions lock") = skills;
    }
}

impl CompletionProvider for CliCompletions {
    fn complete(&self, input: &str) -> Vec<CompletionItem> {
        let skills = self.skills.lock().expect("completions lock");
        slimcode_app::skills::complete(input, &skills)
            .into_iter()
            .map(|item| CompletionItem {
                value: item.value,
                description: item.description,
            })
            .collect()
    }
}

/// The state a running turn owns: taken out of the handler's mutex for the
/// duration of the turn so `cancel` never waits behind the provider.
struct TurnState {
    provider: Box<dyn Provider + Send>,
    tools: Vec<Tool>,
    session: Session,
    cancel: CancelToken,
}

/// The mutable state the worker thread touches: the live session, the provider
/// and tools (moved in and out per turn) and the cancel token.
struct Inner {
    session: Session,
    provider: Option<Box<dyn Provider + Send>>,
    tools: Vec<Tool>,
    cancel: CancelToken,
}

/// The CLI's application handler: everything the TUI needs from a frontend
/// that knows about sessions, skills and the model (ADR-0013 D1).
struct TuiSession<'a> {
    cwd: PathBuf,
    store: &'a SessionStore,
    history: &'a HistoryStore,
    skills: &'a SkillStore,
    context_files: Vec<ContextFile>,
    environment: Environment,
    completions: CliCompletions,
    inner: Mutex<Inner>,
}

impl TuiSession<'_> {
    fn session_id(&self) -> String {
        self.inner.lock().expect("session lock").session.id.clone()
    }

    fn skills_snapshot(&self) -> Vec<Skill> {
        self.skills.list().unwrap_or_default()
    }

    /// Answer one `/` command (ADR-0013 D3). The reducer only says "the user
    /// typed a slash command"; every meaning — including the unknown-command
    /// notice and the skill-name resolution — is decided here.
    fn command(
        &mut self,
        name: &str,
        arg: Option<&str>,
        emit: &mut dyn FnMut(RenderItem),
    ) -> ControlFlow {
        if let Some(command) = slimcode_commands::find(name) {
            match command.name {
                "/help" => return self.help(emit),
                "/new" => return self.new_session(emit),
                "/load" => {
                    return match arg.filter(|id| !id.is_empty()) {
                        Some(id) => self.load_session(id, emit),
                        None => {
                            emit(RenderItem::Error(
                                "/load needs a session id — /sessions lists them".to_string(),
                            ));
                            ControlFlow::Continue
                        }
                    };
                }
                "/sessions" => return self.list_sessions(emit),
                "/usage" => return self.show_usage(emit),
                "/history" => return self.list_history(emit),
                "/skills" => return self.list_skills(emit),
                "/install-skill" => return self.install_skill(arg, emit),
                "/exit" => return ControlFlow::Quit,
                "/!!" => return self.replay(1, emit),
                "/!" => {
                    // `/!` alone, or `/!N` with the index as its argument.
                    return match arg.and_then(parse_replay_index) {
                        Some(n) => self.replay(n, emit),
                        None => {
                            emit(RenderItem::Error(
                                "/!N needs a number (1 = newest)".to_string(),
                            ));
                            ControlFlow::Continue
                        }
                    };
                }
                _ => {}
            }
        }
        // Numbered replay is not an exact spelling: `/!3` lands here.
        if let Some(n) = parse_replay_index(name) {
            return self.replay(n, emit);
        }
        // Skill trigger: `/skill-name` or the canonical `/skill:name`.
        let skills = self.skills_snapshot();
        if let Some(skill) = find_skill(&skills, name) {
            let history = self
                .inner
                .lock()
                .expect("session lock")
                .session
                .messages
                .clone();
            let already_loaded = skill_loaded_in(&history, skill);
            return ControlFlow::Submit(Prompt {
                text: skill_prompt(skill, arg, already_loaded),
                record: false,
            });
        }
        // Unknown command: the predictive notice (commands + skills).
        let suggestions = slimcode_app::skills::combined_suggestions(&skills, name);
        emit(RenderItem::Error(format!("unknown command: {name}")));
        if suggestions.is_empty() {
            emit(RenderItem::Notice(
                "  run /help to list commands".to_string(),
            ));
        } else {
            emit(RenderItem::Notice(format!(
                "  did you mean: {}",
                suggestions.join(", ")
            )));
        }
        ControlFlow::Continue
    }

    /// The built-in command list, rendered by the CLI.
    fn help(&self, emit: &mut dyn FnMut(RenderItem)) -> ControlFlow {
        emit(RenderItem::Notice("commands:".to_string()));
        let width = slimcode_commands::COMMANDS
            .iter()
            .map(|c| c.usage.chars().count())
            .max()
            .unwrap_or(0);
        for command in slimcode_commands::COMMANDS {
            let mut line = format!("  {:<width$}  {}", command.usage, command.description);
            if !command.aliases.is_empty() {
                line.push_str(&format!("  (alias: {})", command.aliases.join(", ")));
            }
            emit(RenderItem::Notice(line));
        }
        emit(RenderItem::Notice(
            "multi-line: Shift+Enter inserts a newline; Enter submits".to_string(),
        ));
        emit(RenderItem::Notice(
            "skills: /skills lists installed skills; /skill:<name> runs one".to_string(),
        ));
        ControlFlow::Continue
    }

    /// Start a fresh session: a new id, a cleared transcript, and a fresh
    /// branch/title.
    fn new_session(&self, emit: &mut dyn FnMut(RenderItem)) -> ControlFlow {
        let session = self.store.new_session();
        let id = session.id.clone();
        self.inner.lock().expect("session lock").session = session;
        emit(RenderItem::SessionChanged { id: id.clone() });
        emit(RenderItem::Notice(format!("new session: {id}")));
        emit(RenderItem::Branch(current_branch(&self.cwd)));
        set_title(&id, &self.cwd);
        ControlFlow::Continue
    }

    /// Load a saved session, reporting the lenient-replay repair counters.
    fn load_session(&self, id: &str, emit: &mut dyn FnMut(RenderItem)) -> ControlFlow {
        let outcome = match self.store.load(id) {
            Ok(outcome) => outcome,
            Err(e) => {
                emit(RenderItem::Error(e));
                return ControlFlow::Continue;
            }
        };
        if let Some(title) = &outcome.session.title {
            emit(RenderItem::Notice(format!("  title: {title}")));
        }
        if outcome.skipped_records > 0 {
            emit(RenderItem::Notice(format!(
                "  skipped {} unreadable record(s)",
                outcome.skipped_records
            )));
        }
        if outcome.repaired_tool_calls > 0 {
            emit(RenderItem::Notice(format!(
                "  repaired {} interrupted tool call(s)",
                outcome.repaired_tool_calls
            )));
        }
        let session = outcome.session;
        let id = session.id.clone();
        self.inner.lock().expect("session lock").session = session;
        emit(RenderItem::SessionChanged { id: id.clone() });
        emit(RenderItem::Notice(format!("loaded session: {id}")));
        emit(RenderItem::Branch(current_branch(&self.cwd)));
        set_title(&id, &self.cwd);
        ControlFlow::Continue
    }

    fn list_sessions(&self, emit: &mut dyn FnMut(RenderItem)) -> ControlFlow {
        match self.store.list() {
            Ok(ids) => {
                for id in ids {
                    emit(RenderItem::Notice(format!("  {id}")));
                }
            }
            Err(e) => emit(RenderItem::Error(e)),
        }
        ControlFlow::Continue
    }

    /// The session's cumulative token usage, worded by the app layer
    /// (`usage_summary`, ADR-0014 D4).
    fn show_usage(&self, emit: &mut dyn FnMut(RenderItem)) -> ControlFlow {
        let usage = {
            let inner = self.inner.lock().expect("session lock");
            inner
                .provider
                .as_ref()
                .map(|p| p.total_usage())
                .unwrap_or_default()
        };
        emit(RenderItem::Notice(usage_summary(&usage)));
        ControlFlow::Continue
    }

    fn list_history(&self, emit: &mut dyn FnMut(RenderItem)) -> ControlFlow {
        match self.history.load() {
            Ok(entries) => {
                for line in render_history(&entries, HISTORY_DISPLAY) {
                    emit(RenderItem::Notice(line));
                }
            }
            Err(e) => emit(RenderItem::Error(e)),
        }
        ControlFlow::Continue
    }

    /// The installed skills, as a `/skills` list (the CLI owns the format).
    fn list_skills(&self, emit: &mut dyn FnMut(RenderItem)) -> ControlFlow {
        let skills = self.skills_snapshot();
        if skills.is_empty() {
            emit(RenderItem::Notice("no skills installed".to_string()));
            return ControlFlow::Continue;
        }
        emit(RenderItem::Notice("skills:".to_string()));
        let width = skills
            .iter()
            .map(|s| s.name.chars().count())
            .max()
            .unwrap_or(0);
        for skill in &skills {
            let scope = match skill.scope {
                SkillScope::User => "user",
                SkillScope::Project => "project",
            };
            let manual = if skill.disable_model_invocation {
                " (manual only)"
            } else {
                ""
            };
            emit(RenderItem::Notice(format!(
                "  /skill:{:<width$}  {}{}  [{}]",
                skill.name, skill.description, manual, scope
            )));
        }
        ControlFlow::Continue
    }

    /// Install a skill, then refresh the `/`-candidate pool.
    fn install_skill(
        &mut self,
        arg: Option<&str>,
        emit: &mut dyn FnMut(RenderItem),
    ) -> ControlFlow {
        let result = parse_install_args(arg)
            .and_then(|(path, scope)| install_skill(self.skills, &path, scope));
        match result {
            Ok(msg) => {
                emit(RenderItem::Notice(msg));
                self.completions.set_skills(self.skills_snapshot());
            }
            Err(e) => emit(RenderItem::Error(e)),
        }
        ControlFlow::Continue
    }

    /// Re-run input-history entry `n` (1 = newest) as a fresh turn, without
    /// re-recording it.
    fn replay(&self, n: usize, emit: &mut dyn FnMut(RenderItem)) -> ControlFlow {
        let entries = match self.history.load() {
            Ok(entries) => entries,
            Err(e) => {
                emit(RenderItem::Error(e));
                return ControlFlow::Continue;
            }
        };
        let Some(prompt) = resolve_replay_index(&entries, n) else {
            emit(RenderItem::Error(format!("no history entry {n}")));
            return ControlFlow::Continue;
        };
        // Replay confirmation: the boxed user-prompt block the typed path
        // shows, applied before the turn starts.
        emit(RenderItem::UserPrompt(prompt.to_string()));
        ControlFlow::Submit(Prompt {
            text: prompt.to_string(),
            record: false,
        })
    }

    /// Take the turn state out of the mutex, so the turn can run without
    /// holding the lock (Esc has to reach the cancel token while it does).
    fn take_turn(&self) -> Result<TurnState, String> {
        let mut inner = self.inner.lock().expect("session lock");
        let Some(provider) = inner.provider.take() else {
            return Err("a turn is already running".to_string());
        };
        // Esc cancels the turn in flight; a fresh turn starts uncancelled.
        inner.cancel.reset();
        Ok(TurnState {
            provider,
            tools: std::mem::take(&mut inner.tools),
            session: inner.session.clone(),
            cancel: inner.cancel.clone(),
        })
    }

    /// Put the provider, tools and the turn's resulting session back.
    fn put_turn(&self, state: TurnState) {
        let mut inner = self.inner.lock().expect("session lock");
        inner.provider = Some(state.provider);
        inner.tools = state.tools;
        inner.session = state.session;
    }

    /// Close the session log after a failed/cancelled turn: an assistant
    /// message with a short, non-empty text enters memory and is appended, so
    /// the live session adopts the partial turn and the log ends on an
    /// assistant boundary (ADR-0009 D3). The turn's reason rides the record
    /// envelope (ADR-0012 D4), never the message payload.
    fn close_turn(
        &self,
        state: &mut TurnState,
        reason: MessageStopReason,
        error: Option<String>,
        emit: &mut dyn FnMut(RenderItem),
    ) {
        let text = match &error {
            Some(e) => format!("The turn ended with an error: {e}"),
            None => "The turn was cancelled.".to_string(),
        };
        let message = AgentMessage::text(Role::Assistant, text);
        state.session.messages.push(message.clone());
        if let Err(e) =
            self.store
                .append_closing(&state.session, &message, &reason, error.as_deref())
        {
            emit(RenderItem::Notice(format!("session log: {e}")));
        }
    }

    /// Run one whole turn: assemble the context, record the prompt, stream the
    /// agent loop into the transcript, append every message that enters
    /// history, and close the log on failure (ADR-0009 D2/D5).
    fn run_turn(
        &self,
        state: &mut TurnState,
        skills: &[Skill],
        prompt: Prompt,
        emit: &mut dyn FnMut(RenderItem),
    ) -> Result<TurnReport, String> {
        let title_was_none = state.session.title.is_none();
        let context = ContextBuilder::new()
            .with_environment(self.environment.clone())
            .with_context_files(&self.context_files)
            .with_skills(skills)
            .with_history(state.session.messages.clone())
            .with_user_prompt(&prompt.text)
            .build()?;
        // A turn adds exactly one message to history — the user prompt. The
        // system prompt is assembled per turn and never stored (ADR-0012 D3).
        let prompt_msg = context
            .messages
            .last()
            .expect("context always has a prompt message")
            .clone();
        state.session.messages.push(prompt_msg.clone());
        if let Err(e) = self.store.append(&state.session, &prompt_msg) {
            emit(RenderItem::Notice(format!("session log: {e}")));
        }
        if title_was_none {
            state.session.title = infer_title(&context.messages);
        }
        // A title that only became known now must reach an existing log: the
        // creation header is already written, so it needs its own record
        // (ADR-0009 D1).
        if title_was_none
            && let Some(title) = state.session.title.as_ref()
            && let Ok(path) = self.store.session_path(&state.session.id)
            && path.exists()
            && let Err(e) = self.store.append_title(&state.session, title)
        {
            emit(RenderItem::Notice(format!("session log: {e}")));
        }
        // Record the typed prompt before the turn, so a failed turn still
        // records what the user wrote.
        if prompt.record
            && let Err(e) = self.history.append(&prompt.text)
        {
            emit(RenderItem::Notice(format!("history: {e}")));
        }

        let cfg = RunConfig::default();
        let mut appended = 0usize;
        let mut append_errors: Vec<String> = Vec::new();
        let TurnState {
            provider,
            tools,
            session,
            cancel,
        } = state;
        let result = slimcode_app::runner::run_turn(
            provider,
            tools,
            context,
            &cfg,
            cancel,
            &mut TuiAdapter::new(emit),
            &mut |msg: &AgentMessage| -> Result<(), String> {
                // The message entered history: mirror it into the session and
                // append it to the log. Persistence is best-effort — a storage
                // hiccup must not abort the turn.
                session.messages.push(msg.clone());
                appended += 1;
                if let Err(e) = self.store.append(session, msg) {
                    append_errors.push(e);
                }
                Ok(())
            },
        );
        for e in &append_errors {
            emit(RenderItem::Notice(format!("session log: {e}")));
        }
        // Footer totals: read after the turn, while the provider is still
        // here, and converted by the CLI (ADR-0014 D1).
        emit(RenderItem::Usage(crate::render::to_footer_usage(
            &provider.total_usage(),
        )));
        match result {
            Ok((_, StopReason::Completed)) => {}
            Ok((_, StopReason::Cancelled)) => {
                // Esc: the partial turn is adopted; the log is closed on an
                // assistant boundary so it never ends on a dangling tool batch
                // (ADR-0009 D3).
                if appended > 0 {
                    self.close_turn(state, MessageStopReason::Aborted, None, emit);
                }
            }
            Err(e) => {
                // A failed turn still leaves the session persisted, with its
                // prior messages intact; the failure closes the log on an
                // assistant boundary (or leaves no file at all if nothing was
                // appended). The library shows the returned error.
                if appended > 0 {
                    self.close_turn(state, MessageStopReason::Error, Some(e.clone()), emit);
                }
                return Err(e);
            }
        }
        Ok(TurnReport { appended })
    }
}

impl UiHandler for TuiSession<'_> {
    fn on_effect(&mut self, effect: Effect, emit: &mut dyn FnMut(RenderItem)) -> ControlFlow {
        match effect {
            // The library runs a turn on its worker thread.
            Effect::SubmitPrompt(prompt) => ControlFlow::Submit(prompt),
            Effect::Command { name, arg } => self.command(&name, arg.as_deref(), emit),
            Effect::Quit => ControlFlow::Quit,
            // Both are handled inside the running loop and never reach here;
            // the arms keep the match exhaustive.
            Effect::QuitAfterTurn | Effect::CancelRunning => ControlFlow::Continue,
        }
    }

    fn submit(
        &self,
        prompt: Prompt,
        emit: &mut dyn FnMut(RenderItem),
    ) -> Result<TurnReport, String> {
        let skills = self.skills_snapshot();
        let mut state = self.take_turn()?;
        let result = self.run_turn(&mut state, &skills, prompt, emit);
        self.put_turn(state);
        result
    }

    fn cancel(&self) {
        self.inner.lock().expect("session lock").cancel.cancel();
    }
}

/// Install a skill after validating the source and rejecting built-in name
/// collisions. Returns a human-readable confirmation line.
fn install_skill(skills: &SkillStore, path: &Path, scope: SkillScope) -> Result<String, String> {
    let inspected = skills.inspect(path, scope)?;
    if is_builtin_command(&inspected.name) {
        return Err(format!(
            "skill name {:?} conflicts with a built-in command",
            inspected.name
        ));
    }
    let skill = skills.install(path, scope)?;
    let target = skills.skill_dir(scope, &skill.name);
    Ok(format!(
        "installed skill /skill:{} to {}",
        skill.name,
        target.display()
    ))
}

/// A numbered replay index (`/!3` → `3`), or `None` when the text is not one.
/// `/!!` is the registry's rerun command, not a number.
fn parse_replay_index(text: &str) -> Option<usize> {
    let spec = text.strip_prefix("/!")?;
    if spec.is_empty() || !spec.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let n = spec.parse::<usize>().ok()?;
    (n >= 1).then_some(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use slimcode_ai::wire::TokenUsage;
    use slimcode_ai::{Delta, FinishReason};
    use slimcode_core::session::Message;

    /// A scratch directory for one test, removed on drop.
    struct Scratch {
        dir: PathBuf,
    }

    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "slimcode-cli-tui-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("scratch dir");
            Self { dir }
        }

        fn path(&self, rel: &str) -> PathBuf {
            self.dir.join(rel)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    /// A scripted provider: each `chat` returns the next delta batch, and
    /// `fail_at` makes one call fail the way a network error does.
    struct FakeProvider {
        script: Vec<Vec<Delta>>,
        calls: usize,
        fail_at: Option<usize>,
        usage: TokenUsage,
    }

    impl FakeProvider {
        fn new(script: Vec<Vec<Delta>>) -> Self {
            Self {
                script,
                calls: 0,
                fail_at: None,
                usage: TokenUsage::default(),
            }
        }

        fn with_usage(usage: TokenUsage) -> Self {
            Self {
                script: Vec::new(),
                calls: 0,
                fail_at: None,
                usage,
            }
        }
    }

    impl Provider for FakeProvider {
        fn chat(
            &mut self,
            _messages: &[Message],
            _tools: &[slimcode_core::agent::ToolSpec],
            _cancel: &CancelToken,
        ) -> Result<Vec<Delta>, String> {
            if self.fail_at == Some(self.calls) {
                return Err("boom".to_string());
            }
            let batch = self.script.get(self.calls).cloned().unwrap_or_default();
            self.calls += 1;
            Ok(batch)
        }

        fn total_usage(&self) -> TokenUsage {
            self.usage
        }
    }

    /// The stores one handler needs, kept alive for the handler's borrows.
    struct Fixture {
        scratch: Scratch,
        store: SessionStore,
        history: HistoryStore,
        skills: SkillStore,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let scratch = Scratch::new(tag);
            let home = scratch.path("home");
            let cwd = scratch.path("proj");
            fs::create_dir_all(&home).expect("home");
            fs::create_dir_all(&cwd).expect("cwd");
            Self {
                store: SessionStore::new(scratch.path("sessions"), "proj", &cwd),
                history: HistoryStore::new(scratch.path("history.json")),
                skills: SkillStore::new(&home, &cwd),
                scratch,
            }
        }

        fn handler(&self, provider: Box<dyn Provider + Send>, tools: Vec<Tool>) -> TuiSession<'_> {
            let session = self.store.new_session();
            let completions = CliCompletions::new(self.skills.list().unwrap_or_default());
            TuiSession {
                cwd: self.scratch.path("proj"),
                store: &self.store,
                history: &self.history,
                skills: &self.skills,
                context_files: Vec::new(),
                environment: Environment {
                    os: "test".to_string(),
                    global_home: self.scratch.path("home"),
                    project_home: self.scratch.path("proj"),
                },
                completions,
                inner: Mutex::new(Inner {
                    session,
                    provider: Some(provider),
                    tools,
                    cancel: CancelToken::new(),
                }),
            }
        }

        /// Write a skill into the user skills dir, the way a store scan finds
        /// it: `<home>/skills/<name>/SKILL.md`.
        fn write_user_skill(&self, name: &str, description: &str) {
            let dir = self.scratch.path("home").join("skills").join(name);
            fs::create_dir_all(&dir).expect("skill dir");
            fs::write(
                dir.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: {description}\n---\nBody.\n"),
            )
            .expect("skill file");
        }
    }

    /// Drive the handler the way the library does, recording what it emits.
    fn drive(handler: &mut TuiSession<'_>, effect: Effect) -> (ControlFlow, Vec<RenderItem>) {
        let mut items = Vec::new();
        let flow = handler.on_effect(effect, &mut |item| items.push(item));
        (flow, items)
    }

    /// Drive one slash command, returning its emitted items.
    fn command(handler: &mut TuiSession<'_>, line: &str) -> Vec<RenderItem> {
        let (name, arg) = line
            .split_once(' ')
            .map_or((line, None), |(n, a)| (n, Some(a)));
        let (flow, items) = drive(
            handler,
            Effect::Command {
                name: name.to_string(),
                arg: arg.map(str::to_string),
            },
        );
        assert_eq!(flow, ControlFlow::Continue, "command {line} ended the loop");
        items
    }

    /// The notice/error lines of a batch of display items, in order.
    fn lines(items: &[RenderItem]) -> Vec<&str> {
        items
            .iter()
            .map(|item| match item {
                RenderItem::Notice(text) | RenderItem::Error(text) => text.as_str(),
                other => panic!("not a text line: {other:?}"),
            })
            .collect()
    }

    fn user_message(text: &str) -> Prompt {
        Prompt {
            text: text.to_string(),
            record: true,
        }
    }

    #[test]
    fn help_lists_every_command() {
        let fx = Fixture::new("help");
        let mut handler = fx.handler(Box::new(FakeProvider::new(Vec::new())), Vec::new());
        let items = command(&mut handler, "/help");
        let lines = lines(&items);
        assert_eq!(lines[0], "commands:");
        assert!(lines.iter().any(|l| l.contains("/load <id>")), "{lines:?}");
        assert!(
            lines.iter().any(|l| l.contains("(alias: /resume)")),
            "{lines:?}"
        );
        assert!(lines.iter().any(|l| l.contains("Shift+Enter")), "{lines:?}");
    }

    #[test]
    fn exit_quits_the_loop() {
        let fx = Fixture::new("exit");
        let mut handler = fx.handler(Box::new(FakeProvider::new(Vec::new())), Vec::new());
        let (flow, items) = drive(
            &mut handler,
            Effect::Command {
                name: "/exit".to_string(),
                arg: None,
            },
        );
        assert_eq!(flow, ControlFlow::Quit);
        assert!(items.is_empty());
    }

    #[test]
    fn skills_list_reports_none_and_then_each_skill() {
        let fx = Fixture::new("skills");
        let mut handler = fx.handler(Box::new(FakeProvider::new(Vec::new())), Vec::new());
        assert_eq!(
            lines(&command(&mut handler, "/skills")),
            ["no skills installed"]
        );

        fx.write_user_skill("grill", "stress-test a plan");
        let items = command(&mut handler, "/skills");
        let lines = lines(&items).join("\n");
        assert!(lines.contains("/skill:grill"), "{lines}");
        assert!(lines.contains("[user]"), "{lines}");
    }

    #[test]
    fn unknown_command_gets_a_did_you_mean_notice() {
        let fx = Fixture::new("unknown");
        fx.write_user_skill("grill", "stress-test a plan");
        let mut handler = fx.handler(Box::new(FakeProvider::new(Vec::new())), Vec::new());
        let items = command(&mut handler, "/gril");
        let lines = lines(&items);
        assert_eq!(lines[0], "unknown command: /gril");
        assert!(lines[1].contains("/skill:grill"), "{lines:?}");
    }

    #[test]
    fn skill_trigger_submits_the_skill_block_without_recording() {
        let fx = Fixture::new("skill-trigger");
        fx.write_user_skill("grill", "stress-test a plan");
        let mut handler = fx.handler(Box::new(FakeProvider::new(Vec::new())), Vec::new());
        let (flow, items) = drive(
            &mut handler,
            Effect::Command {
                name: "/grill".to_string(),
                arg: Some("my plan".to_string()),
            },
        );
        let ControlFlow::Submit(prompt) = flow else {
            panic!("expected a turn, got {flow:?}");
        };
        assert!(!prompt.record, "skill triggers are never recorded");
        assert!(
            prompt.text.starts_with("<skill name=\"grill\""),
            "{}",
            prompt.text
        );
        assert!(prompt.text.ends_with("my plan"), "{}", prompt.text);
        assert!(prompt.text.contains("Body."), "{}", prompt.text);
        assert!(items.is_empty());
    }

    #[test]
    fn replay_emits_the_prompt_box_and_never_records() {
        let fx = Fixture::new("replay");
        fx.history.append("first").unwrap();
        fx.history.append("second").unwrap();
        let mut handler = fx.handler(Box::new(FakeProvider::new(Vec::new())), Vec::new());

        // `/!!` reruns the newest entry: the boxed prompt the typed path
        // shows, then the turn (never recorded again).
        let (flow, items) = drive(
            &mut handler,
            Effect::Command {
                name: "/!!".to_string(),
                arg: None,
            },
        );
        assert_eq!(items, [RenderItem::UserPrompt("second".to_string())]);
        assert_eq!(
            flow,
            ControlFlow::Submit(Prompt {
                text: "second".to_string(),
                record: false,
            })
        );

        // `/!N` picks the Nth-newest entry.
        let (flow, _) = drive(
            &mut handler,
            Effect::Command {
                name: "/!2".to_string(),
                arg: None,
            },
        );
        assert_eq!(
            flow,
            ControlFlow::Submit(Prompt {
                text: "first".to_string(),
                record: false,
            })
        );
        assert_eq!(fx.history.load().unwrap(), ["first", "second"]);
    }

    #[test]
    fn bad_replay_indexes_are_errors() {
        let fx = Fixture::new("replay-bad");
        let mut handler = fx.handler(Box::new(FakeProvider::new(Vec::new())), Vec::new());
        assert_eq!(
            lines(&command(&mut handler, "/!")),
            ["/!N needs a number (1 = newest)"]
        );
        assert_eq!(lines(&command(&mut handler, "/!9")), ["no history entry 9"]);
        // A malformed index is not a numbered replay: it falls through to the
        // skill/unknown-command path.
        let items = command(&mut handler, "/!x");
        let lines = lines(&items);
        assert_eq!(lines[0], "unknown command: /!x");
    }

    #[test]
    fn new_session_changes_the_session_and_clears_the_transcript() {
        let fx = Fixture::new("new");
        let mut handler = fx.handler(Box::new(FakeProvider::new(Vec::new())), Vec::new());
        let before = handler.session_id();
        let items = command(&mut handler, "/new");
        let after = handler.session_id();
        assert_ne!(before, after);
        assert!(items.contains(&RenderItem::SessionChanged { id: after.clone() }));
        assert!(items.contains(&RenderItem::Notice(format!("new session: {after}"))));
        assert!(items.iter().any(|i| matches!(i, RenderItem::Branch(_))));
    }

    #[test]
    fn load_without_an_id_is_an_error() {
        let fx = Fixture::new("load-bad");
        let mut handler = fx.handler(Box::new(FakeProvider::new(Vec::new())), Vec::new());
        assert_eq!(
            lines(&command(&mut handler, "/load")),
            ["/load needs a session id — /sessions lists them"]
        );
        assert_eq!(
            lines(&command(&mut handler, "/sessions")),
            Vec::<&str>::new(),
            "no saved sessions in a fresh store"
        );
    }

    #[test]
    fn usage_reports_the_providers_totals() {
        let fx = Fixture::new("usage");
        let usage = TokenUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
            ..Default::default()
        };
        let mut handler = fx.handler(Box::new(FakeProvider::with_usage(usage)), Vec::new());
        assert_eq!(
            lines(&command(&mut handler, "/usage")),
            ["tokens: 10 prompt (0 cached, 0%) + 5 completion = 15 total"]
        );
    }

    #[test]
    fn install_skill_refreshes_the_candidate_pool() {
        let fx = Fixture::new("install");
        let source = fx.scratch.path("src-skill");
        fs::create_dir_all(&source).unwrap();
        fs::write(
            source.join("SKILL.md"),
            "---\nname: grill\ndescription: stress-test a plan\n---\nBody.\n",
        )
        .unwrap();
        let mut handler = fx.handler(Box::new(FakeProvider::new(Vec::new())), Vec::new());
        assert!(
            handler.completions.complete("/gr").is_empty(),
            "nothing installed yet"
        );

        let line = format!("/install-skill {} --user", source.display());
        let items = command(&mut handler, &line);
        let lines = lines(&items);
        assert!(
            lines[0].contains("installed skill /skill:grill"),
            "{lines:?}"
        );

        let values: Vec<String> = handler
            .completions
            .complete("/gr")
            .into_iter()
            .map(|item| item.value)
            .collect();
        assert_eq!(values, ["/skill:grill"]);
    }

    #[test]
    fn submit_streams_a_turn_and_persists_every_message() {
        let fx = Fixture::new("submit");
        let provider = FakeProvider::new(vec![vec![
            Delta::Text("hello ".to_string()),
            Delta::Text("there".to_string()),
            Delta::Done(FinishReason::Stop),
        ]]);
        let handler = fx.handler(Box::new(provider), Vec::new());
        let id = handler.session_id();

        let mut items = Vec::new();
        let report = handler
            .submit(user_message("hi"), &mut |item| items.push(item))
            .expect("turn succeeds");

        // The user prompt plus the assistant reply entered the log.
        // `appended` counts what the turn itself produced: the assistant
        // reply (the user prompt is recorded by the CLI before it runs).
        assert_eq!(report.appended, 1);
        assert!(items.contains(&RenderItem::Text("hello ".to_string())));
        assert!(items.contains(&RenderItem::Text("there".to_string())));
        // The typed prompt is recorded in input history.
        assert_eq!(fx.history.load().unwrap(), ["hi"]);

        let loaded = fx.store.load(&id).expect("log written").session;
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.messages[0].role(), &Role::User);
        assert_eq!(loaded.messages[1].role(), &Role::Assistant);
        assert_eq!(loaded.messages[1].text_content(), "hello there");
        assert_eq!(loaded.title.as_deref(), Some("hi"));
    }

    #[test]
    fn a_replayed_prompt_is_not_recorded() {
        let fx = Fixture::new("submit-replay");
        let provider = FakeProvider::new(vec![vec![
            Delta::Text("ok".to_string()),
            Delta::Done(FinishReason::Stop),
        ]]);
        let handler = fx.handler(Box::new(provider), Vec::new());
        handler
            .submit(
                Prompt {
                    text: "replayed".to_string(),
                    record: false,
                },
                &mut |_| {},
            )
            .unwrap();
        assert!(fx.history.load().unwrap().is_empty());
    }

    #[test]
    fn failed_turn_closes_the_log_and_returns_err() {
        let fx = Fixture::new("submit-fail");
        // Call 0 asks for a tool, call 1 fails: the assistant message and the
        // tool result are already in the log when the turn dies.
        let provider = FakeProvider {
            script: vec![vec![
                Delta::ToolCallStart {
                    index: 0,
                    id: "c1".to_string(),
                    name: "probe".to_string(),
                },
                Delta::ToolCallArgs {
                    index: 0,
                    fragment: "{}".to_string(),
                },
                Delta::Done(FinishReason::ToolCalls),
            ]],
            calls: 0,
            fail_at: Some(1),
            usage: TokenUsage::default(),
        };
        let tool = Tool::new("probe", "a probe", serde_json::json!({}), |_args| {
            Ok("probed".to_string())
        });
        let handler = fx.handler(Box::new(provider), vec![tool]);
        let id = handler.session_id();

        let err = handler
            .submit(user_message("do it"), &mut |_| {})
            .expect_err("the provider fails");
        assert_eq!(err, "boom");

        // The log ends on an assistant boundary carrying the failure.
        let loaded = fx.store.load(&id).expect("log written").session;
        let last = loaded.messages.last().expect("closing message");
        assert_eq!(last.role(), &Role::Assistant);
        assert!(
            last.text_content()
                .contains("The turn ended with an error: boom"),
            "{}",
            last.text_content()
        );
    }

    #[test]
    fn cancel_flips_the_token_and_a_new_turn_resets_it() {
        let fx = Fixture::new("cancel");
        let provider = FakeProvider::new(vec![vec![
            Delta::Text("done".to_string()),
            Delta::Done(FinishReason::Stop),
        ]]);
        let handler = fx.handler(Box::new(provider), Vec::new());
        handler.cancel();
        assert!(
            handler.inner.lock().unwrap().cancel.is_cancelled(),
            "Esc reached the token"
        );

        // A fresh turn starts uncancelled, so it runs to completion.
        let report = handler
            .submit(user_message("go"), &mut |_| {})
            .expect("turn succeeds");
        assert_eq!(report.appended, 1);
    }

    #[test]
    fn a_second_turn_while_one_runs_is_rejected() {
        let fx = Fixture::new("busy");
        let handler = fx.handler(Box::new(FakeProvider::new(Vec::new())), Vec::new());
        // Take the provider out, as a running turn does.
        let state = handler.take_turn().expect("first turn");
        assert_eq!(
            handler.submit(user_message("go"), &mut |_| {}).unwrap_err(),
            "a turn is already running"
        );
        handler.put_turn(state);
        assert!(handler.submit(user_message("go"), &mut |_| {}).is_ok());
    }
}
