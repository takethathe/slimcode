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

use std::io::{Write, stdout};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, SetTitle, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use slimcode_ai::ProviderConfig;
use slimcode_app::context::{ContextBuilder, Environment, skill_loaded_in};
use slimcode_app::context_files::ContextFile;
use slimcode_app::history::{HISTORY_DISPLAY, HistoryStore, render_history, resolve_replay_index};
use slimcode_app::render::usage_summary;
use slimcode_app::session::{SessionStore, format_minute, infer_title, unix_secs};
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
use slimcode_tui::render::{RenderItem, SessionRow};

use crate::render::{TuiAdapter, history_to_render_items};

/// Enter the TUI: build the runtime, take over the terminal, run the frame
/// loop, and restore the terminal on every exit path.
#[allow(clippy::too_many_arguments)]
pub fn run(
    cwd: &Path,
    config: ProviderConfig,
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
    let (provider, tools) = slimcode_app::setup::setup_with_cancel(cwd, &cancel)?;
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
        config,
        completions,
        inner: Mutex::new(Inner {
            session,
            provider: Some(Box::new(provider)),
            tools,
            cancel,
        }),
    };

    // The terminal belongs to the CLI: raw mode, the alternate screen, the
    // mouse subscription and the title are all set up here, and the panic hook
    // restores them.
    enable_raw_mode().map_err(|e| format!("raw mode: {e}"))?;
    execute!(stdout(), EnterAlternateScreen).map_err(|e| format!("alternate screen: {e}"))?;
    if let Err(e) = enable_wheel_scroll() {
        restore_terminal();
        return Err(format!("mouse scroll: {e}"));
    }
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

/// Subscribe to wheel scroll only: DECSET `?1000` (button press/release, which
/// is how a wheel notch arrives) plus `?1006` (SGR coordinates).
///
/// Deliberately *not* crossterm's all-in-one `EnableMouseCapture`, which also
/// sends `?1002`/`?1003`/`?1015` and thereby subscribes to mouse motion — the
/// frame loop implements no motion semantics and must not be flooded by move
/// events. `XTSHIFTESCAPE` is not sent either, so Shift+drag stays the
/// terminal's native selection (ADR-0017 D2/D3).
const ENABLE_WHEEL_SCROLL: &[u8] = b"\x1b[?1000h\x1b[?1006h";

/// The exact inverse of [`ENABLE_WHEEL_SCROLL`], sent from every exit path so
/// the shell gets its mouse back.
const DISABLE_WHEEL_SCROLL: &[u8] = b"\x1b[?1006l\x1b[?1000l";

/// Turn the wheel subscription on. Fails only if the terminal is unreachable.
fn enable_wheel_scroll() -> std::io::Result<()> {
    let mut out = stdout();
    out.write_all(ENABLE_WHEEL_SCROLL)?;
    out.flush()
}

/// Leave raw mode, the mouse subscription and the alternate screen. Best-effort:
/// the process is exiting the TUI either way, so failures are swallowed.
fn restore_terminal() {
    let mut out = stdout();
    let _ = out.write_all(DISABLE_WHEEL_SCROLL);
    let _ = disable_raw_mode();
    let _ = execute!(out, LeaveAlternateScreen);
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
    /// The resolved provider config (ADR-0016). Held here for the whole
    /// session and threaded into every turn's `run_turn` call; the provider
    /// instance stays stateless.
    config: ProviderConfig,
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
                "/session" => return self.open_session_picker(emit),
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
    ///
    /// Picking the session that is already current is a no-op (ADR-0018 D3):
    /// reloading would re-read the log, drop the in-memory history that is
    /// ahead of it, clear the screen and reset usage, none of which a row the
    /// cursor often starts on should do.
    ///
    /// `SessionChanged` is emitted *before* the load notices: it clears the
    /// transcript, so notices emitted first would be wiped (ADR-0018 D2). The
    /// notices stay right under the header, and the loaded history is then
    /// replayed after them so the fresh view shows the conversation again
    /// instead of ending at the header.
    fn load_session(&self, id: &str, emit: &mut dyn FnMut(RenderItem)) -> ControlFlow {
        if id == self.session_id() {
            return ControlFlow::Continue;
        }
        let outcome = match self.store.load(id) {
            Ok(outcome) => outcome,
            Err(e) => {
                emit(RenderItem::Error(e));
                return ControlFlow::Continue;
            }
        };
        let title = outcome.session.title.clone();
        let skipped_records = outcome.skipped_records;
        let repaired_tool_calls = outcome.repaired_tool_calls;
        let session = outcome.session;
        let id = session.id.clone();
        // Replay the loaded conversation into the transcript (before the
        // session moves into `self.inner`).
        let history_items = history_to_render_items(&session.messages);
        self.inner.lock().expect("session lock").session = session;
        emit(RenderItem::SessionChanged { id: id.clone() });
        if let Some(title) = &title {
            emit(RenderItem::Notice(format!("  title: {title}")));
        }
        if skipped_records > 0 {
            emit(RenderItem::Notice(format!(
                "  skipped {skipped_records} unreadable record(s)"
            )));
        }
        if repaired_tool_calls > 0 {
            emit(RenderItem::Notice(format!(
                "  repaired {repaired_tool_calls} interrupted tool call(s)"
            )));
        }
        emit(RenderItem::Notice(format!("loaded session: {id}")));
        for item in history_items {
            emit(item);
        }
        emit(RenderItem::Branch(current_branch(&self.cwd)));
        set_title(&id, &self.cwd);
        ControlFlow::Continue
    }

    /// Open the session picker (ADR-0018 D2/D5): this project's saved
    /// sessions, scanned and built into rows the library renders and selects.
    /// The scan happens here, synchronously, so the library owns no filesystem
    /// knowledge; a scan failure is an error line and no picker.
    fn open_session_picker(&self, emit: &mut dyn FnMut(RenderItem)) -> ControlFlow {
        match self.store.entries() {
            Ok(entries) => {
                let rows = entries
                    .into_iter()
                    .map(|entry| SessionRow {
                        title: entry.title.clone().unwrap_or_else(|| entry.id.clone()),
                        id: entry.id,
                        meta: format!(
                            "{}  {}",
                            message_count(entry.messages),
                            format_minute(unix_secs(entry.modified))
                        ),
                    })
                    .collect();
                emit(RenderItem::SessionPicker { rows });
            }
            Err(e) => emit(RenderItem::Error(e)),
        }
        ControlFlow::Continue
    }

    /// The live session's own cumulative token usage, worded by the app layer
    /// (`usage_summary`, ADR-0014 D4). The provider's running total is the
    /// runtime's counter, not the display's source (ADR-0018 D4).
    fn show_usage(&self, emit: &mut dyn FnMut(RenderItem)) -> ControlFlow {
        let usage = self.inner.lock().expect("session lock").session.usage;
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
        // A turn that dies before producing anything must still close an
        // existing log on an assistant boundary; a turn with no log yet leaves
        // none (the closing message would otherwise be the file's first
        // assistant message and create one — ADR-0009 D5).
        let log_exists = self
            .store
            .session_path(&state.session.id)
            .map(|path| path.exists())
            .unwrap_or(false);
        // Usage belongs to the live session (ADR-0018 D4): snapshot the
        // provider's running total before the turn, then add the diff after
        // it. A turn can issue several requests (the tool loop), so the diff
        // — not one request's numbers — is what the session earned, and the
        // snapshot keeps earlier turns (and earlier sessions) out of it.
        let usage_before = state.provider.total_usage();
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
            &self.config,
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
        // Footer totals: the session's own accumulated usage, converted by the
        // CLI (ADR-0014 D1).
        session.usage = session
            .usage
            .saturating_add(&provider.total_usage().saturating_sub(&usage_before));
        emit(RenderItem::Usage(crate::render::to_footer_usage(
            &session.usage,
        )));
        match result {
            Ok((_, StopReason::Completed)) => {}
            Ok((_, StopReason::Cancelled)) => {
                // Esc: the partial turn is adopted; the log is closed on an
                // assistant boundary so it never ends on a dangling tool batch
                // (ADR-0009 D3).
                if appended > 0 || log_exists {
                    self.close_turn(state, MessageStopReason::Aborted, None, emit);
                }
            }
            Err(e) => {
                // A failed turn still leaves the session persisted, with its
                // prior messages intact; the failure closes the log on an
                // assistant boundary (or leaves no file at all if the turn was
                // the session's first and produced nothing). The library shows
                // the returned error.
                if appended > 0 || log_exists {
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
            // The library's picker picked a row; the CLI loads it.
            Effect::LoadSession { id } => self.load_session(&id, emit),
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

/// The picker's message-count column: `1 msg` (singular) or `N msgs`.
fn message_count(n: usize) -> String {
    if n == 1 {
        "1 msg".to_string()
    } else {
        format!("{n} msgs")
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

    use slimcode_tui::footer::FooterUsage;
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
        /// Added to `usage` on every successful `chat`, the way the real
        /// provider's running total grows with each request of a turn.
        per_call: TokenUsage,
    }

    impl FakeProvider {
        fn new(script: Vec<Vec<Delta>>) -> Self {
            Self {
                script,
                calls: 0,
                fail_at: None,
                usage: TokenUsage::default(),
                per_call: TokenUsage::default(),
            }
        }

        /// A provider whose very first call fails, the way a network error
        /// does on a session's opening turn.
        fn failing_at_zero() -> Self {
            Self {
                script: Vec::new(),
                calls: 0,
                fail_at: Some(0),
                usage: TokenUsage::default(),
                per_call: TokenUsage::default(),
            }
        }

        fn with_usage(usage: TokenUsage) -> Self {
            Self {
                script: Vec::new(),
                calls: 0,
                fail_at: None,
                usage,
                per_call: TokenUsage::default(),
            }
        }

        /// Accumulate `usage` on every successful call.
        fn with_per_call(mut self, usage: TokenUsage) -> Self {
            self.per_call = usage;
            self
        }
    }

    impl Provider for FakeProvider {
        fn chat(
            &mut self,
            _messages: &[Message],
            _tools: &[slimcode_core::agent::ToolSpec],
            _config: &ProviderConfig,
            _cancel: &CancelToken,
            on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
        ) -> Result<(), String> {
            if self.fail_at == Some(self.calls) {
                return Err("boom".to_string());
            }
            let batch = self.script.get(self.calls).cloned().unwrap_or_default();
            self.calls += 1;
            self.usage = self.usage.saturating_add(&self.per_call);
            // A scripted provider mirrors the live one (ADR-0019): deltas go
            // through the sink, in order, as the request produces them.
            for delta in batch {
                on_delta(delta)?;
            }
            Ok(())
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
                config: ProviderConfig::new("test-key", "https://example.invalid/v1", "test-model"),
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

    /// Whether `s` has the picker's minute timestamp shape, `YYYY-MM-DD HH:MM`.
    fn looks_like_minute(s: &str) -> bool {
        let b = s.as_bytes();
        b.len() == 16
            && b[4] == b'-'
            && b[7] == b'-'
            && b[10] == b' '
            && b[13] == b':'
            && s.bytes()
                .enumerate()
                .all(|(i, c)| matches!(i, 4 | 7 | 10 | 13) || c.is_ascii_digit())
    }

    /// Write a session log with a title and one user/assistant pair, as the
    /// first assistant message does at runtime, and return its id.
    fn saved_session(store: &SessionStore, prompt: &str, reply: &str) -> String {
        let mut session = store.new_session();
        session.title = Some(prompt.to_string());
        session
            .messages
            .push(AgentMessage::text(Role::User, prompt));
        let assistant = AgentMessage::text(Role::Assistant, reply);
        session.messages.push(assistant.clone());
        store.append(&session, &assistant).expect("log created");
        session.id
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
        assert!(lines.iter().any(|l| l.contains("/session")), "{lines:?}");
        assert!(!lines.iter().any(|l| l.contains("/load")), "{lines:?}");
        assert!(!lines.iter().any(|l| l.contains("/sessions")), "{lines:?}");
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
    fn session_opens_a_picker_with_title_count_and_time_rows() {
        let fx = Fixture::new("picker");
        let a = saved_session(&fx.store, "first prompt", "first reply");
        let b = saved_session(&fx.store, "second prompt", "second reply");
        let mut handler = fx.handler(Box::new(FakeProvider::new(Vec::new())), Vec::new());

        let items = command(&mut handler, "/session");
        let [RenderItem::SessionPicker { rows }] = &items[..] else {
            panic!("expected one picker item: {items:?}");
        };
        // Newest first, with the title, the record count and the mtime.
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, b);
        assert_eq!(rows[0].title, "second prompt");
        let (count, time) = rows[0].meta.split_once("  ").expect("count + time");
        assert_eq!(count, "2 msgs");
        assert!(looks_like_minute(time), "{}", rows[0].meta);
        assert_eq!(rows[1].id, a);
        assert_eq!(rows[1].title, "first prompt");
    }

    #[test]
    fn picker_rows_fall_back_to_the_id_and_the_singular_count() {
        let fx = Fixture::new("picker-fallback");
        // No title record, and a single message record.
        let mut one = fx.store.new_session();
        let msg = AgentMessage::text(Role::Assistant, "only");
        one.messages.push(msg.clone());
        fx.store.append(&one, &msg).unwrap();
        let mut handler = fx.handler(Box::new(FakeProvider::new(Vec::new())), Vec::new());

        let items = command(&mut handler, "/session");
        let [RenderItem::SessionPicker { rows }] = &items[..] else {
            panic!("expected one picker item: {items:?}");
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, one.id);
        assert_eq!(rows[0].title, one.id, "no title record -> the id");
        assert!(rows[0].meta.starts_with("1 msg  "), "{}", rows[0].meta);
    }

    #[test]
    fn session_reports_a_scan_failure_without_opening_the_picker() {
        let fx = Fixture::new("picker-error");
        // A file where the project directory should be makes the scan fail.
        fs::create_dir_all(fx.scratch.path("sessions")).expect("sessions dir");
        fs::write(fx.scratch.path("sessions/proj"), b"not a directory").expect("blocking file");
        let mut handler = fx.handler(Box::new(FakeProvider::new(Vec::new())), Vec::new());
        let items = command(&mut handler, "/session");
        assert_eq!(items.len(), 1);
        assert!(matches!(items[0], RenderItem::Error(_)), "{items:?}");
    }

    #[test]
    fn load_session_emits_the_change_before_the_notices() {
        let fx = Fixture::new("load-effect");
        let id = saved_session(&fx.store, "the prompt", "the reply");
        let mut handler = fx.handler(Box::new(FakeProvider::new(Vec::new())), Vec::new());

        let (flow, items) = drive(&mut handler, Effect::LoadSession { id: id.clone() });
        assert_eq!(flow, ControlFlow::Continue);
        // The change clears the transcript, so it has to come first or the
        // notices would be wiped.
        assert_eq!(items[0], RenderItem::SessionChanged { id: id.clone() });
        assert!(
            items
                .iter()
                .any(|i| matches!(i, RenderItem::Notice(t) if t.starts_with("loaded session:"))),
            "{items:?}"
        );
        assert!(items.iter().any(|i| matches!(i, RenderItem::Branch(_))));
        assert_eq!(handler.session_id(), id);
        assert_eq!(handler.inner.lock().unwrap().session.messages.len(), 2);
    }

    #[test]
    fn load_session_keeps_the_repair_notices_after_the_change() {
        let fx = Fixture::new("load-repair");
        let id = saved_session(&fx.store, "the prompt", "the reply");
        // Append an unreadable record line by hand, so the load skips one.
        let path = fx.store.session_path(&id).expect("path");
        let mut log = fs::read_to_string(&path).expect("log");
        log.push_str("not json\n");
        fs::write(&path, log).expect("corrupt log");
        let mut handler = fx.handler(Box::new(FakeProvider::new(Vec::new())), Vec::new());

        let (_, items) = drive(&mut handler, Effect::LoadSession { id });
        let changed = items
            .iter()
            .position(|i| matches!(i, RenderItem::SessionChanged { .. }))
            .expect("session change");
        let skipped = items
            .iter()
            .position(|i| matches!(i, RenderItem::Notice(t) if t.contains("skipped 1")))
            .expect("skip notice");
        assert!(
            changed < skipped,
            "notice must survive the clear: {items:?}"
        );
    }

    #[test]
    fn load_session_replays_the_history_into_the_transcript() {
        let fx = Fixture::new("load-replay");
        // A session with a tool round trip, so the replay covers prompt boxes,
        // assistant text, a tool start/result pair and the final answer.
        let mut session = fx.store.new_session();
        session.title = Some("replay".to_string());
        session
            .messages
            .push(AgentMessage::text(Role::User, "list it"));
        session.messages.push(AgentMessage::llm(Message {
            role: Role::Assistant,
            parts: vec![slimcode_ai::message::Part::Text {
                text: String::new(),
            }],
            tool_calls: vec![slimcode_core::session::ToolCall {
                id: "call_1".to_string(),
                name: "bash".to_string(),
                arguments: r#"{"command":"ls"}"#.to_string(),
            }],
            tool_call_id: None,
        }));
        session
            .messages
            .push(AgentMessage::tool_result("call_1", "marker.txt"));
        session
            .messages
            .push(AgentMessage::text(Role::Assistant, "done listing"));
        let last = session.messages.last().unwrap().clone();
        let id = session.id.clone();
        fx.store.append(&session, &last).expect("log created");

        let mut handler = fx.handler(Box::new(FakeProvider::new(Vec::new())), Vec::new());
        let (_, items) = drive(&mut handler, Effect::LoadSession { id });

        // The change clears first, then the notices, then the replayed
        // history restores the conversation onto the fresh view.
        let changed = items
            .iter()
            .position(|i| matches!(i, RenderItem::SessionChanged { .. }))
            .expect("session change");
        let replay: Vec<&RenderItem> = items
            .iter()
            .filter(|i| {
                matches!(
                    i,
                    RenderItem::UserPrompt(_)
                        | RenderItem::Text(_)
                        | RenderItem::ToolStart { .. }
                        | RenderItem::ToolResult { .. }
                )
            })
            .collect();
        assert!(
            changed < items.len(),
            "history must follow the clear: {items:?}"
        );
        assert_eq!(
            replay,
            vec![
                &RenderItem::UserPrompt("list it".to_string()),
                &RenderItem::ToolStart {
                    tool_call_id: "call_1".to_string(),
                    name: "bash".to_string(),
                    arguments: r#"{"command":"ls"}"#.to_string(),
                },
                &RenderItem::ToolResult {
                    tool_call_id: "call_1".to_string(),
                    name: "bash".to_string(),
                    ok: true,
                    result: "marker.txt".to_string(),
                },
                &RenderItem::Text("done listing".to_string()),
            ]
        );
        assert_eq!(handler.inner.lock().unwrap().session.messages.len(), 4);
    }

    #[test]
    fn loading_the_current_session_is_a_no_op() {
        let fx = Fixture::new("load-current");
        let mut handler = fx.handler(Box::new(FakeProvider::new(Vec::new())), Vec::new());
        let id = handler.session_id();
        // Memory is ahead of disk: reloading would drop this.
        handler
            .inner
            .lock()
            .unwrap()
            .session
            .messages
            .push(AgentMessage::text(Role::User, "in memory"));

        let (flow, items) = drive(&mut handler, Effect::LoadSession { id });
        assert_eq!(flow, ControlFlow::Continue);
        assert!(items.is_empty(), "{items:?}");
        assert_eq!(handler.inner.lock().unwrap().session.messages.len(), 1);
    }

    #[test]
    fn usage_reports_the_sessions_totals() {
        let fx = Fixture::new("usage");
        let usage = TokenUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
            ..Default::default()
        };
        let mut handler = fx.handler(Box::new(FakeProvider::with_usage(usage)), Vec::new());
        // The session, not the provider, is the display's source (ADR-0018 D4).
        handler.inner.lock().unwrap().session.usage = usage;
        assert_eq!(
            lines(&command(&mut handler, "/usage")),
            ["tokens: 10 prompt (0 cached, 0%) + 5 completion = 15 total"]
        );
    }

    #[test]
    fn a_turn_accumulates_every_request_in_its_usage() {
        let fx = Fixture::new("usage-turn");
        // Two requests in one turn: a tool call, then the final answer. Each
        // adds the same usage, so the turn must count both and only both.
        let provider = FakeProvider::new(vec![
            vec![
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
            ],
            vec![
                Delta::Text("done".to_string()),
                Delta::Done(FinishReason::Stop),
            ],
        ])
        .with_per_call(TokenUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
            ..Default::default()
        });
        let tool = Tool::new("probe", "a probe", serde_json::json!({}), |_args| {
            Ok("probed".to_string())
        });
        let mut handler = fx.handler(Box::new(provider), vec![tool]);
        let id = handler.session_id();

        let mut items = Vec::new();
        handler
            .submit(user_message("go"), &mut |item| items.push(item))
            .expect("turn succeeds");

        // Two requests × (10 prompt + 5 completion), counted once each.
        assert_eq!(
            handler.inner.lock().unwrap().session.usage,
            TokenUsage {
                prompt_tokens: 20,
                completion_tokens: 10,
                total_tokens: 30,
                ..Default::default()
            }
        );
        assert_eq!(
            items.iter().rev().find_map(|item| match item {
                RenderItem::Usage(usage) => Some(*usage),
                _ => None,
            }),
            Some(FooterUsage::new(20, 10, 0, 0))
        );
        assert_eq!(
            lines(&command(&mut handler, "/usage")),
            ["tokens: 20 prompt (0 cached, 0%) + 10 completion = 30 total"]
        );
        // Usage is memory-only: the log stays free of it (ADR-0018 D4).
        let log = fs::read_to_string(fx.store.session_path(&id).unwrap()).unwrap();
        assert!(!log.contains("\"usage\""), "{log}");
    }

    #[test]
    fn usage_accumulates_across_turns_without_double_counting() {
        let fx = Fixture::new("usage-turns");
        let provider = FakeProvider::new(vec![
            vec![
                Delta::Text("one".to_string()),
                Delta::Done(FinishReason::Stop),
            ],
            vec![
                Delta::Text("two".to_string()),
                Delta::Done(FinishReason::Stop),
            ],
        ])
        .with_per_call(TokenUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
            ..Default::default()
        });
        let handler = fx.handler(Box::new(provider), Vec::new());
        handler.submit(user_message("one"), &mut |_| {}).unwrap();
        handler.submit(user_message("two"), &mut |_| {}).unwrap();

        // Each turn adds its own request only; the total is not the running
        // counter read twice.
        assert_eq!(
            handler.inner.lock().unwrap().session.usage.total_tokens,
            30,
            "two turns × one request each"
        );
    }

    #[test]
    fn new_session_resets_the_usage_to_zero() {
        let fx = Fixture::new("usage-new");
        let provider = FakeProvider::new(vec![vec![
            Delta::Text("hi".to_string()),
            Delta::Done(FinishReason::Stop),
        ]])
        .with_per_call(TokenUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
            ..Default::default()
        });
        let mut handler = fx.handler(Box::new(provider), Vec::new());
        handler.submit(user_message("one"), &mut |_| {}).unwrap();
        assert_eq!(handler.inner.lock().unwrap().session.usage.total_tokens, 15);

        command(&mut handler, "/new");
        assert_eq!(
            handler.inner.lock().unwrap().session.usage,
            TokenUsage::default()
        );
        assert_eq!(
            lines(&command(&mut handler, "/usage")),
            ["tokens: 0 prompt (0 cached, 0%) + 0 completion = 0 total"]
        );
    }

    #[test]
    fn loading_a_session_resets_the_usage_to_zero() {
        let fx = Fixture::new("usage-load");
        let saved = saved_session(&fx.store, "old prompt", "old reply");
        let provider = FakeProvider::new(vec![vec![
            Delta::Text("hi".to_string()),
            Delta::Done(FinishReason::Stop),
        ]])
        .with_per_call(TokenUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
            ..Default::default()
        });
        let mut handler = fx.handler(Box::new(provider), Vec::new());
        handler.submit(user_message("one"), &mut |_| {}).unwrap();
        assert_eq!(handler.inner.lock().unwrap().session.usage.total_tokens, 15);

        // The previous session's numbers must not follow the load.
        drive(&mut handler, Effect::LoadSession { id: saved });
        assert_eq!(
            handler.inner.lock().unwrap().session.usage,
            TokenUsage::default()
        );
        assert_eq!(
            lines(&command(&mut handler, "/usage")),
            ["tokens: 0 prompt (0 cached, 0%) + 0 completion = 0 total"]
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
            per_call: TokenUsage::default(),
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
    fn a_turn_that_dies_on_an_existing_log_still_closes_it() {
        let fx = Fixture::new("submit-fail-existing");
        // Turn 1 completes (the log is created); turn 2 dies before producing
        // anything, so only the gating on an existing log can close it.
        let provider = FakeProvider {
            script: vec![vec![
                Delta::Text("first".to_string()),
                Delta::Done(FinishReason::Stop),
            ]],
            calls: 0,
            fail_at: Some(1),
            usage: TokenUsage::default(),
            per_call: TokenUsage::default(),
        };
        let handler = fx.handler(Box::new(provider), Vec::new());
        let id = handler.session_id();
        handler
            .submit(user_message("one"), &mut |_| {})
            .expect("first turn succeeds");

        let err = handler
            .submit(user_message("two"), &mut |_| {})
            .expect_err("second turn fails");
        assert_eq!(err, "boom");

        let loaded = fx.store.load(&id).expect("log loads").session;
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
    fn a_first_turn_that_dies_leaves_no_log() {
        let fx = Fixture::new("submit-fail-first");
        let handler = fx.handler(Box::new(FakeProvider::failing_at_zero()), Vec::new());
        let id = handler.session_id();
        handler
            .submit(user_message("only"), &mut |_| {})
            .expect_err("the provider fails");
        // Nothing worth persisting: the log must not exist (ADR-0009 D5).
        let path = fx.store.session_path(&id).expect("path");
        assert!(!path.exists(), "unexpected log at {}", path.display());
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
