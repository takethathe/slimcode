//! The thin crossterm/ratatui terminal loop that wraps the pure [`App`]
//! (spec §Implementation Decisions: the pure core owns all behavior, and the
//! terminal loop is a thin, untested shell). This module only pumps events,
//! fulfils the App's I/O effects, and draws frames; everything testable lives
//! in `crate::app` and is covered by `TestBackend` tests.
//!
//! Setup (provider + tools) happens **before** the alternate screen opens, so
//! config or API-key errors surface on the normal terminal (spec user story
//! 28). One turn's `run_turn` runs on a worker thread (ADR-0006 D6) streaming
//! `DisplayItem`s through the CLI's adapter into the TUI's own
//! [`RenderItem`](crate::render::RenderItem) channel (ADR-0014 D2); the UI loop
//! polls crossterm events and the channel with an 80ms timeout, so the status
//! spinner animates and Ctrl+C/Ctrl+D work while a turn runs (they arm
//! quit-after-turn; other keys are ignored). The `App` reducer and all
//! rendering decisions stay pure and tested; this file keeps only the raw
//! terminal I/O.

use std::io::stdout;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, SetTitle, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use slimcode_ai::{BailianConfig, BailianProvider};
use slimcode_app::context::{Context, ContextBuilder, Environment};
use slimcode_app::context_files::ContextFile;
use slimcode_app::history::{HISTORY_DISPLAY, HistoryStore, render_history, resolve_replay_index};
use slimcode_app::render::{DisplayItem, Renderer, usage_summary};
use slimcode_app::session::{SessionStore, infer_title};
use slimcode_app::skills::{
    SkillScope, SkillStore, find_skill, is_builtin_command, parse_install_args,
};
use slimcode_core::agent::{CancelToken, RunConfig, StopReason, Tool};
use slimcode_core::session::{AgentMessage, MessageStopReason, Role, Session};

use crate::app::{App, Effect};
use crate::git::{current_branch, terminal_title};
use crate::render::{RenderItem, SkillInfo};

/// One UI-loop frame: ~80ms, matching pi's loader interval so the status
/// spinner animates at the same rate and the input stays responsive.
const FRAME_MS: u64 = 80;

/// Builds the CLI's `DisplayItem → RenderItem` [`Renderer`] for one turn
/// (ADR-0014 D2). The TUI creates the turn's channel and hands the sender to
/// the factory; the adapter the CLI returns runs on the turn's worker thread
/// and owns that sender. Kept as a seam here because the display conversion
/// belongs to the CLI — ticket 05 replaces it with the `UiHandler`.
pub type AdapterFactory =
    Box<dyn Fn(mpsc::Sender<RenderItem>) -> Box<dyn Renderer + Send> + Send + Sync>;

/// Translate the store's skills into the TUI's own snapshot
/// ([`SkillInfo`]), which is all the transcript, `/skills` list and `/`
/// prediction need.
fn skill_infos(skills: &[slimcode_app::skills::Skill]) -> Vec<SkillInfo> {
    skills
        .iter()
        .map(|s| SkillInfo {
            name: s.name.clone(),
            description: s.description.clone(),
            disable_model_invocation: s.disable_model_invocation,
            scope: match s.scope {
                slimcode_app::skills::SkillScope::User => crate::render::SkillScope::User,
                slimcode_app::skills::SkillScope::Project => crate::render::SkillScope::Project,
            },
        })
        .collect()
}

/// Run the TUI to completion: build the runtime, open the alternate screen,
/// pump events, and restore the terminal on every exit path.
///
/// `config` is already fully resolved (four-layer precedence); construction of
/// the provider + tools happens here, before the screen opens.
#[allow(clippy::too_many_arguments)]
pub fn run(
    cwd: &Path,
    config: BailianConfig,
    store: &SessionStore,
    history: &HistoryStore,
    skills: &SkillStore,
    context_files: &[ContextFile],
    environment: Environment,
    adapter_factory: AdapterFactory,
) -> Result<i32, String> {
    let model = config.model.clone();
    // One token shared by the cancellable tool set and every turn's worker:
    // Esc cancels whichever phase the run is in (ticket 07).
    let cancel = CancelToken::new();
    let (provider, tools) = slimcode_app::setup::setup_with_cancel(cwd, config, &cancel)?;
    let session = store.new_session();
    let mut ui = match Tui::new(
        provider,
        tools,
        store,
        history,
        skills,
        session,
        cwd,
        model,
        cancel,
        context_files,
        environment,
        adapter_factory,
    ) {
        Ok(ui) => ui,
        Err(e) => {
            restore_terminal();
            return Err(e);
        }
    };
    set_title(&ui.session.id, &ui.cwd);
    ui.app.apply(RenderItem::Branch(current_branch(&ui.cwd)));
    let result = ui.run_loop();
    restore_terminal();
    result
}

/// Set the OSC 0 terminal title (spec §Implementation Decisions "Terminal
/// title"). Best-effort: a title failure must not kill the TUI.
fn set_title(session_id: &str, cwd: &Path) {
    let _ = execute!(stdout(), SetTitle(terminal_title(session_id, cwd)));
}

/// What a turn's worker produced: the session at the log's position (the
/// pre-turn session plus every message that entered history), the turn result
/// with its stop reason, how many messages were appended to the log, and any
/// append failures the worker collected (persistence is best-effort).
struct TurnOutcome {
    session: Session,
    result: Result<(Vec<AgentMessage>, StopReason), String>,
    appended: usize,
    append_errors: Vec<String>,
}

/// The full TUI session state. Owns the pure [`App`] and everything the loop
/// needs to fulfil its effects.
struct Tui<'a> {
    app: App,
    terminal: Terminal<CrosstermBackend<std::io::Stdout>>,
    /// The provider lives on the worker thread only while a turn runs
    /// (`None` then); the running loop that polls the channel never touches
    /// it, and the shell restores it on join.
    provider: Option<BailianProvider>,
    tools: Vec<Tool>,
    store: &'a SessionStore,
    history: &'a HistoryStore,
    skills: &'a SkillStore,
    /// Discovered `AGENTS.md` files (global + project), injected into every
    /// fresh system message via `ContextBuilder::with_context_files`.
    context_files: Vec<ContextFile>,
    /// System environment info (OS, global home, project home), injected into
    /// every fresh system message via `ContextBuilder::with_environment`.
    /// Frozen at session start: a restored session keeps its first-turn value.
    environment: Environment,
    session: Session,
    cwd: PathBuf,
    /// Set while a turn runs when the user presses Ctrl+C/Ctrl+D: the current
    /// turn keeps streaming to completion, then the TUI exits.
    quit_after_turn: bool,
    /// Per-turn cancellation handle (ticket 07): reset at the start of every
    /// `drive_turn`, cancelled when the user presses Esc. The worker clone
    /// observes it at every runner boundary and inside the provider's body
    /// read, so an in-flight request / tool aborts promptly.
    cancel: CancelToken,
    /// The CLI's display adapter for each turn (ADR-0014 D2).
    adapter_factory: AdapterFactory,
}

impl<'a> Tui<'a> {
    /// Open raw mode + the alternate screen, seed the app, and draw the first
    /// frame. Any failure here restores the terminal via [`restore_terminal`]
    /// (the caller does that on `Err`).
    #[allow(clippy::too_many_arguments)]
    fn new(
        provider: BailianProvider,
        tools: Vec<Tool>,
        store: &'a SessionStore,
        history: &'a HistoryStore,
        skills: &'a SkillStore,
        session: Session,
        cwd: &Path,
        model: String,
        cancel: CancelToken,
        context_files: &[ContextFile],
        environment: Environment,
        adapter_factory: AdapterFactory,
    ) -> Result<Self, String> {
        enable_raw_mode().map_err(|e| format!("raw mode: {e}"))?;
        execute!(stdout(), EnterAlternateScreen).map_err(|e| format!("alternate screen: {e}"))?;
        let mut terminal =
            Terminal::new(CrosstermBackend::new(stdout())).map_err(|e| e.to_string())?;
        terminal.hide_cursor().map_err(|e| e.to_string())?;

        let mut app = App::new(
            cwd.display().to_string(),
            session.id.clone(),
            model,
            crate::VERSION,
            skill_infos(&skills.list().unwrap_or_default()),
        );
        // Seed the ↑/↓ recall snapshot from the shared HistoryStore.
        app.set_history(history.load().unwrap_or_default());

        let mut ui = Tui {
            app,
            terminal,
            provider: Some(provider),
            tools,
            store,
            history,
            skills,
            context_files: context_files.to_vec(),
            environment,
            session,
            cwd: cwd.to_path_buf(),
            quit_after_turn: false,
            cancel,
            adapter_factory,
        };
        ui.draw()?;
        Ok(ui)
    }

    /// The event loop: pump keys, run the reducer, fulfil effects, redraw.
    fn run_loop(&mut self) -> Result<i32, String> {
        loop {
            let event = event::read().map_err(|e| e.to_string())?;
            match event {
                Event::Key(key) => {
                    if let Some(effect) = self.app.handle_key(key)
                        && self.handle_effect(effect)?
                    {
                        break;
                    }
                }
                // Resize re-renders: ratatui's draw autoresizes, so the next
                // frame already uses the new size.
                Event::Resize(..) => {}
                _ => {}
            }
            // Ctrl+C/Ctrl+D while a turn runs arm quit-after-turn; once the
            // turn's effect completes (turn finished), leave the loop.
            if self.quit_after_turn {
                break;
            }
            self.draw()?;
            self.app.tick();
        }
        Ok(0)
    }

    /// Fulfil one effect the reducer surfaced. Command-level errors are pushed
    /// into the transcript (print-and-continue, matching the REPL); only
    /// terminal-level failures propagate.
    fn handle_effect(&mut self, effect: Effect) -> Result<bool, String> {
        let mut quit = false;
        if let Err(e) = self.apply_effect(effect, &mut quit) {
            self.app.apply(RenderItem::Error(e));
            self.draw()?;
        }
        Ok(quit)
    }

    /// Apply one effect without swallowing errors.
    fn apply_effect(&mut self, effect: Effect, quit: &mut bool) -> Result<(), String> {
        match effect {
            Effect::Quit => {
                *quit = true;
                Ok(())
            }
            // The running loop handles this directly and it never reaches
            // `apply_effect`; the arm exists for exhaustiveness.
            Effect::QuitAfterTurn => {
                self.quit_after_turn = true;
                Ok(())
            }
            // Also handled inside `drive_turn`'s running loop (it cancels the
            // shared token); the arm exists for exhaustiveness.
            Effect::CancelRunning => Ok(()),
            Effect::SubmitPrompt(prompt) => self.submit_prompt(prompt, true),
            Effect::ReplayPrompt(prompt) => self.submit_prompt(prompt, false),
            Effect::ReplayHistory(n) => self.replay_history(n),
            Effect::TriggerSkill { name, arg } => self.trigger_skill(&name, arg.as_deref()),
            Effect::NewSession => {
                self.session = self.store.new_session();
                self.app.apply(RenderItem::SessionChanged {
                    id: self.session.id.clone(),
                });
                self.app.apply(RenderItem::Notice(format!(
                    "new session: {}",
                    self.session.id
                )));
                self.app
                    .apply(RenderItem::Branch(current_branch(&self.cwd)));
                set_title(&self.session.id, &self.cwd);
                Ok(())
            }
            Effect::LoadSession(id) => {
                let outcome = self.store.load(&id)?;
                if let Some(title) = &outcome.session.title {
                    self.app
                        .apply(RenderItem::Notice(format!("  title: {title}")));
                }
                // Lenient replay surfaced something (ADR-0009 D3): say so.
                if outcome.skipped_records > 0 {
                    self.app.apply(RenderItem::Notice(format!(
                        "  skipped {} unreadable record(s)",
                        outcome.skipped_records
                    )));
                }
                if outcome.repaired_tool_calls > 0 {
                    self.app.apply(RenderItem::Notice(format!(
                        "  repaired {} interrupted tool call(s)",
                        outcome.repaired_tool_calls
                    )));
                }
                self.session = outcome.session;
                self.app.apply(RenderItem::SessionChanged {
                    id: self.session.id.clone(),
                });
                self.app.apply(RenderItem::Notice(format!(
                    "loaded session: {}",
                    self.session.id
                )));
                self.app
                    .apply(RenderItem::Branch(current_branch(&self.cwd)));
                set_title(&self.session.id, &self.cwd);
                Ok(())
            }
            Effect::ListSessions => {
                for id in self.store.list()? {
                    self.app.apply(RenderItem::Notice(format!("  {id}")));
                }
                Ok(())
            }
            Effect::ShowUsage => {
                // Wording stays the app layer's (`usage_summary`); the CLI
                // renders it as a notice, exactly as before (ADR-0014 D1).
                let usage = self
                    .provider
                    .as_ref()
                    .map(|p| p.total_usage)
                    .unwrap_or_default();
                self.app.apply(RenderItem::Notice(usage_summary(&usage)));
                Ok(())
            }
            Effect::ListHistory => {
                let entries = self.history.load()?;
                for line in render_history(&entries, HISTORY_DISPLAY) {
                    self.app.apply(RenderItem::Notice(line));
                }
                Ok(())
            }
            Effect::InstallSkill(reference) => {
                let result = parse_install_args(Some(&reference))
                    .and_then(|(path, scope)| install_skill(self.skills, &path, scope));
                match result {
                    Ok(msg) => {
                        self.app.apply(RenderItem::Notice(msg));
                        // Re-read the store so `/` completion, `did you mean`,
                        // and skill dispatch see the new skill without a restart.
                        let fresh = self.skills.list().unwrap_or_default();
                        self.app.apply(RenderItem::Skills(skill_infos(&fresh)));
                    }
                    Err(e) => self.app.apply(RenderItem::Error(e)),
                }
                Ok(())
            }
        }
    }

    /// Run a prompt as a fresh turn. `record` controls whether the raw prompt
    /// is appended to input history (true for typed prompts, false for
    /// replays and skill triggers). This turn's new messages (the first-turn
    /// system message plus the user prompt) enter the session history first,
    /// and each is appended to the session log as it enters memory — the
    /// append is a no-op before the first assistant message, so no file exists
    /// until the model replies (ADR-0009 D5). The context sent to the runner
    /// is built from that same history, so memory, log and context are one
    /// message sequence.
    fn submit_prompt(&mut self, prompt: String, record: bool) -> Result<(), String> {
        let skills = self.skills.list().unwrap_or_default();
        let context = ContextBuilder::new()
            .with_environment(self.environment.clone())
            .with_context_files(&self.context_files)
            .with_skills(&skills)
            .with_history(self.session.messages.clone())
            .with_user_prompt(&prompt)
            .build()?;
        // A turn adds exactly one message to history — the user prompt. The
        // system prompt is assembled per turn and never stored (ADR-0012 D3).
        let prompt_msg = context
            .messages
            .last()
            .expect("context always has a prompt message")
            .clone();
        self.session.messages.push(prompt_msg.clone());
        if let Err(e) = self.store.append(&self.session, &prompt_msg) {
            self.app
                .apply(RenderItem::Notice(format!("session log: {e}")));
        }
        let record = if record { Some(prompt.as_str()) } else { None };
        self.finish_turn(context, record)
    }

    /// Re-run input history entry `n` (1 = newest) as a fresh turn without
    /// re-recording it.
    fn replay_history(&mut self, n: usize) -> Result<(), String> {
        let entries = self.history.load()?;
        let prompt =
            resolve_replay_index(&entries, n).ok_or_else(|| format!("no history entry {n}"))?;
        // Replay confirmation: spec requires replay confirmations to render as
        // frontend-owned display entries appended to the transcript (the same
        // boxed user-prompt block the typed path uses).
        self.app.apply(RenderItem::UserPrompt(prompt.to_string()));
        self.submit_prompt(prompt.to_string(), false)
    }

    /// Trigger a skill as a fresh turn; never recorded in input history. The
    /// skill's system message (first turn only) and the user prompt enter the
    /// session history and log first, mirroring [`Tui::submit_prompt`].
    fn trigger_skill(&mut self, name: &str, arg: Option<&str>) -> Result<(), String> {
        let skills = self.skills.list().unwrap_or_default();
        let Some(skill) = find_skill(&skills, name) else {
            self.app
                .apply(RenderItem::Error(format!("no such skill: /{name}")));
            return Ok(());
        };
        let context = ContextBuilder::new()
            .with_environment(self.environment.clone())
            .with_context_files(&self.context_files)
            .with_skills(&skills)
            .with_history(self.session.messages.clone())
            .with_skill(skill, arg)
            .build()?;
        let prompt_msg = context
            .messages
            .last()
            .expect("context always has a prompt message")
            .clone();
        self.session.messages.push(prompt_msg.clone());
        if let Err(e) = self.store.append(&self.session, &prompt_msg) {
            self.app
                .apply(RenderItem::Notice(format!("session log: {e}")));
        }
        self.finish_turn(context, None)
    }

    /// Shared tail of a prompt/skill turn: infer the title, record history
    /// (before the turn, so a failed turn still records what was typed),
    /// stream the turn live into the transcript, adopt the worker's session
    /// (memory reaches the log's position), and feed the footer usage totals.
    /// A turn that failed or was cancelled after its first assistant message
    /// closes the log with a short assistant message (`stop_reason` =
    /// `error`/`aborted`); a turn that never produced an assistant message
    /// leaves no log at all (ADR-0009 D5).
    fn finish_turn(&mut self, context: Context, record: Option<&str>) -> Result<(), String> {
        let title_was_none = self.session.title.is_none();
        if title_was_none {
            self.session.title = infer_title(&context.messages);
        }
        // A title that only became known now (e.g. a restored session whose
        // earlier turn had an empty prompt) must reach the log: the log may
        // already exist, and then the title cannot ride the creation header
        // block any more — it needs its own record. If the log does not exist
        // yet, the title rides the header block at creation instead
        // (ADR-0009 D1).
        if title_was_none
            && let Some(title) = self.session.title.as_ref()
            && let Ok(path) = self.store.session_path(&self.session.id)
            && path.exists()
            && let Err(e) = self.store.append_title(&self.session, title)
        {
            self.app
                .apply(RenderItem::Notice(format!("session log: {e}")));
        }
        if let Some(prompt) = record
            && let Err(e) = self.history.append(prompt)
        {
            self.app.apply(RenderItem::Notice(format!("history: {e}")));
        }
        let outcome = self.drive_turn(context)?;
        // Adopt the worker's session: memory and the log now agree.
        self.session = outcome.session;
        for e in &outcome.append_errors {
            self.app
                .apply(RenderItem::Notice(format!("session log: {e}")));
        }
        match outcome.result {
            Ok((_, StopReason::Completed)) => {}
            Ok((_, StopReason::Cancelled)) => {
                // Esc: the partial turn is adopted; the log is closed on an
                // assistant boundary so it never ends on a dangling tool
                // batch or a trailing tool result (ADR-0009 D3).
                if outcome.appended > 0 {
                    self.close_turn(MessageStopReason::Aborted, None);
                }
            }
            Err(e) => {
                // A failed turn still leaves the session persisted, with its
                // prior messages intact; the failure closes the log on an
                // assistant boundary (or leaves no file at all if nothing was
                // appended). The failure reason still shows in the transcript.
                if outcome.appended > 0 {
                    self.close_turn(MessageStopReason::Error, Some(e.clone()));
                }
                self.app.apply(RenderItem::Error(e));
            }
        }
        Ok(())
    }

    /// Close the log after a failed/cancelled turn: an assistant message with
    /// a short, non-empty text enters memory and is appended, so the live
    /// session adopts the partial turn and the log ends on an assistant
    /// boundary (ADR-0009 D3). The turn's reason rides the record envelope
    /// (ADR-0012 D4), never the message payload.
    fn close_turn(&mut self, reason: MessageStopReason, error: Option<String>) {
        let text = match &error {
            Some(e) => format!("The turn ended with an error: {e}"),
            None => "The turn was cancelled.".to_string(),
        };
        let message = AgentMessage::text(Role::Assistant, text);
        self.session.messages.push(message.clone());
        if let Err(e) =
            self.store
                .append_closing(&self.session, &message, &reason, error.as_deref())
        {
            self.app
                .apply(RenderItem::Notice(format!("session log: {e}")));
        }
    }

    /// Drive one turn on a worker thread (ADR-0006 D6). The worker runs the
    /// shared runner against the moved provider + tools, streaming every
    /// `DisplayItem` through the CLI's adapter, which converts it into a
    /// [`RenderItem`] on the TUI's channel (ADR-0014 D2); every message that
    /// enters history is forwarded to a sink that appends it to the session
    /// log and to a session clone, so the log grows per message as the turn
    /// runs and the clone mirrors memory (ADR-0009 D2/D5). `thread::scope` lets
    /// the worker borrow the store; the UI loop polls terminal events and the
    /// channel at [`FRAME_MS`], draining items into the app and redrawing so
    /// the spinner animates. Ctrl+C/Ctrl+D record quit-after-turn (the turn
    /// keeps streaming); bare Esc cancels the running turn (ticket 07 — the
    /// token is reset here so every turn starts uncancelled); all other keys
    /// are ignored while running. The provider and tools are always restored
    /// before returning.
    fn drive_turn(&mut self, context: Context) -> Result<TurnOutcome, String> {
        self.cancel.reset();
        self.app.set_running(true);
        let cfg = RunConfig::default();
        let (tx, rx) = mpsc::channel::<RenderItem>();
        let mut adapter = (self.adapter_factory)(tx);
        let mut provider = self.provider.take().expect("provider present while idle");
        let tools = std::mem::take(&mut self.tools);
        let cancel = self.cancel.clone();
        let store = self.store;
        let mut session_clone = self.session.clone();
        let mut appended = 0usize;
        let mut append_errors: Vec<String> = Vec::new();

        let worker = thread::scope(|scope| {
            let handle = scope.spawn(move || {
                let result = slimcode_app::runner::run_turn(
                    &mut provider,
                    &tools,
                    context,
                    &cfg,
                    &cancel,
                    &mut *adapter,
                    &mut |msg: &AgentMessage| -> Result<(), String> {
                        // The message entered history: mirror it into the
                        // session clone and append it to the log. Persistence
                        // is best-effort — a storage hiccup must not abort the
                        // turn, so failures are collected and reported as
                        // notices after the turn.
                        session_clone.messages.push(msg.clone());
                        appended += 1;
                        if let Err(e) = store.append(&session_clone, msg) {
                            append_errors.push(e);
                        }
                        Ok(())
                    },
                );
                // Feed the footer totals through the same adapter, so the
                // `TokenUsage → FooterUsage` conversion stays the CLI's
                // (ADR-0014 D1). The session totals are what the turn just
                // produced plus everything before it.
                let _ = adapter.render(&DisplayItem::Usage(provider.total_usage));
                (
                    result,
                    provider,
                    tools,
                    session_clone,
                    appended,
                    append_errors,
                )
            });

            // The UI loop runs until the worker finishes. A terminal/UI error
            // is remembered but the loop still drains the channel and joins,
            // so the provider is always restored and the turn result is never
            // lost. The closure returns a Result so a mid-loop terminal
            // failure propagates exactly like the pre-scope code did; a join
            // panic is the same unrecoverable worker loss as before.
            let mut ui_error: Option<String> = None;
            let mut quit_after_turn = false;
            let joined = loop {
                // Polling is also the frame timer (~80ms); on a failed poll
                // once an error is recorded we still sleep so the loop is not
                // hot.
                match event::poll(Duration::from_millis(FRAME_MS)) {
                    Ok(true) => match event::read() {
                        Ok(Event::Key(key)) if ui_error.is_none() => {
                            match self.app.handle_key_running(key) {
                                Some(Effect::QuitAfterTurn) => quit_after_turn = true,
                                // Esc: abort the in-flight request / tool at
                                // the next runner boundary; the worker drains
                                // and joins as usual and the turn ends
                                // Cancelled.
                                Some(Effect::CancelRunning) => self.cancel.cancel(),
                                _ => {}
                            }
                        }
                        Ok(_) => {}
                        Err(e) if ui_error.is_none() => ui_error = Some(format!("event: {e}")),
                        Err(_) => {}
                    },
                    Ok(false) => {}
                    Err(e) if ui_error.is_none() => ui_error = Some(format!("event poll: {e}")),
                    Err(_) => {}
                }
                drain_channel(&rx, &mut self.app);
                if handle.is_finished() {
                    drain_channel(&rx, &mut self.app);
                    break handle
                        .join()
                        .map_err(|_| "turn worker panicked".to_string())?;
                }
                if ui_error.is_none() {
                    self.draw()?;
                }
                self.app.tick();
            };
            Ok::<_, String>((joined, ui_error, quit_after_turn))
        })?;
        let (
            (result, provider, tools, session_clone, appended, append_errors),
            ui_error,
            quit_after_turn,
        ) = worker;
        self.quit_after_turn |= quit_after_turn;
        self.provider = Some(provider);
        self.tools = tools;
        self.app.set_running(false);
        if let Some(err) = ui_error {
            return Err(err);
        }
        Ok(TurnOutcome {
            session: session_clone,
            result,
            appended,
            append_errors,
        })
    }

    /// Draw one frame.
    fn draw(&mut self) -> Result<(), String> {
        self.terminal
            .draw(|frame| self.app.draw(frame, frame.area()))
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

/// Drain every item the worker has queued into the app, in order. Applying an
/// item cannot fail, so the drive loop only has to report its own terminal
/// errors.
fn drain_channel(rx: &mpsc::Receiver<RenderItem>, app: &mut App) {
    while let Ok(item) = rx.try_recv() {
        app.apply(item);
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

/// Leave raw mode + the alternate screen. Best-effort: the process is exiting
/// the TUI either way, so failures are swallowed.
fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(stdout(), LeaveAlternateScreen);
}

#[cfg(test)]
mod tests {
    use super::*;
    use slimcode_app::render::DisplayItem;
    use slimcode_app::runner::run_turn;
    use slimcode_core::agent::{Delta, FinishReason, Provider, RunConfig, Tool, ToolSpec};
    use slimcode_core::session::{Message, Role};

    // A scripted provider, mirroring the agent crate's FakeProvider: each
    // `chat` call returns the next delta batch from the script.
    struct FakeProvider {
        script: Vec<Vec<Delta>>,
        calls: usize,
    }

    impl Provider for FakeProvider {
        fn chat(
            &mut self,
            _m: &[Message],
            _tools: &[ToolSpec],
            _cancel: &slimcode_core::agent::CancelToken,
        ) -> Result<Vec<Delta>, String> {
            let d = self.script.get(self.calls).cloned().unwrap_or_default();
            self.calls += 1;
            Ok(d)
        }
    }

    impl FakeProvider {
        fn new(script: Vec<Vec<Delta>>) -> Self {
            Self { script, calls: 0 }
        }
    }

    fn weather_tool() -> Tool {
        Tool::new(
            "get_weather",
            "weather for a city",
            serde_json::json!({}),
            |args: serde_json::Value| {
                let city = args
                    .get("city")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("?");
                Ok(format!("{{\"city\": \"{city}\", \"temp\": \"25C\"}}"))
            },
        )
    }

    /// The test's stand-in for the CLI's adapter (ADR-0014 D1): it maps the
    /// streamed agent output onto the TUI's vocabulary and drops the two items
    /// the TUI never renders. The real mapping — and the token-usage
    /// conversion — is the CLI's `TuiAdapter`, covered by its own tests.
    struct TestAdapter {
        tx: mpsc::Sender<RenderItem>,
    }

    impl Renderer for TestAdapter {
        fn render(&mut self, item: &DisplayItem) -> Result<(), String> {
            let mapped = match item {
                DisplayItem::Text(t) => RenderItem::Text(t.clone()),
                DisplayItem::Reasoning(t) => RenderItem::Reasoning(t.clone()),
                DisplayItem::ToolStart {
                    tool_call_id,
                    name,
                    arguments,
                } => RenderItem::ToolStart {
                    tool_call_id: tool_call_id.clone(),
                    name: name.clone(),
                    arguments: arguments.clone(),
                },
                DisplayItem::ToolResult {
                    tool_call_id,
                    name,
                    ok,
                    result,
                } => RenderItem::ToolResult {
                    tool_call_id: tool_call_id.clone(),
                    name: name.clone(),
                    ok: *ok,
                    result: result.clone(),
                },
                // Dropped: the transcript has no turn markers and renders
                // nothing for a stop; `Usage` never comes from the runner.
                DisplayItem::Turn { .. } | DisplayItem::Stop(_) | DisplayItem::Usage(_) => {
                    return Ok(());
                }
            };
            self.tx.send(mapped).map_err(|e| e.to_string())
        }
    }

    /// The channel worker, end to end: a scripted provider + the display
    /// adapter on a worker thread stream an ordered `RenderItem` stream over
    /// the channel, and `run_turn`'s result is the final message list
    /// (ADR-0014 D2, ADR-0006 D6).
    #[test]
    fn channel_worker_streams_ordered_items_and_returns_messages() {
        let provider = FakeProvider::new(vec![
            // Batch 1: streamed text, then one tool call (weather answers,
            // no error). The ToolCalls finish makes the loop call `chat` again.
            vec![
                Delta::Text("hello ".to_string()),
                Delta::Text("world".to_string()),
                Delta::ToolCallStart {
                    index: 0,
                    id: "tc-1".to_string(),
                    name: "get_weather".to_string(),
                },
                Delta::ToolCallArgs {
                    index: 0,
                    fragment: r#"{"city": "Tokyo"}"#.to_string(),
                },
                Delta::Done(FinishReason::ToolCalls),
            ],
            // Batch 2: the answer after the tool result ends the turn.
            vec![
                Delta::Text("it is 25C".to_string()),
                Delta::Done(FinishReason::Stop),
            ],
        ]);
        let system = Message::text(Role::System, "sys");
        let messages = vec![AgentMessage::text(Role::User, "hi")];
        let context = slimcode_app::context::Context { system, messages };
        let cfg = RunConfig::default();
        let (tx, rx) = mpsc::channel::<RenderItem>();

        let handle = thread::spawn(move || {
            let mut provider = provider;
            let tools = vec![weather_tool()];
            let cancel = slimcode_core::agent::CancelToken::new();
            let result = run_turn(
                &mut provider,
                &tools,
                context,
                &cfg,
                &cancel,
                &mut TestAdapter { tx },
                &mut |_| Ok(()),
            );
            (result, provider.calls)
        });

        let mut items: Vec<RenderItem> = Vec::new();
        while let Ok(item) = rx.recv() {
            items.push(item);
        }
        let (result, calls) = handle.join().unwrap();
        let (updated, _) = result.expect("turn succeeds");

        // The ordered stream the UI loop applies: the streamed text fragments
        // (each delta its own item) and the tool start/result pair. The turn
        // markers and the stop line were dropped by the adapter.
        let kinds: Vec<&str> = items
            .iter()
            .map(|i| match i {
                RenderItem::Reasoning(_) => "reasoning",
                RenderItem::Text(_) => "text",
                RenderItem::ToolStart { .. } => "tool-start",
                RenderItem::ToolResult { .. } => "tool-result",
                other => panic!("unexpected item from the runner: {other:?}"),
            })
            .collect();
        assert_eq!(
            kinds,
            vec!["text", "text", "tool-start", "tool-result", "text"]
        );
        // The individual items match the scripted stream.
        assert!(items.contains(&RenderItem::Text("hello ".to_string())));
        assert!(items.contains(&RenderItem::Text("world".to_string())));
        assert!(items.contains(&RenderItem::ToolStart {
            tool_call_id: "tc-1".to_string(),
            name: "get_weather".to_string(),
            arguments: r#"{"city": "Tokyo"}"#.to_string(),
        }));
        assert!(items.contains(&RenderItem::ToolResult {
            tool_call_id: "tc-1".to_string(),
            name: "get_weather".to_string(),
            ok: true,
            result: r#"{"city": "Tokyo", "temp": "25C"}"#.to_string(),
        }));
        assert!(items.contains(&RenderItem::Text("it is 25C".to_string())));
        // Two provider calls: the streaming+tool batch, then the answer batch.
        assert_eq!(calls, 2);
        // The final messages include the assistant turn + tool result.
        assert!(updated.iter().any(|m| m.role() == &Role::Assistant));
    }
}
