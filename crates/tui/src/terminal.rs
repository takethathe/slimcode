//! The thin crossterm/ratatui terminal loop that wraps the pure [`App`]
//! (spec §Implementation Decisions: the pure core owns all behavior, and the
//! terminal loop is a thin, untested shell). This module only pumps events,
//! fulfils the App's I/O effects, and draws frames; everything testable lives
//! in `crate::app` and is covered by `TestBackend` tests.
//!
//! Setup (provider + tools) happens **before** the alternate screen opens, so
//! config or API-key errors surface on the normal terminal (spec user story
//! 28). One turn's `run_turn` runs on a worker thread (ADR-0006 D6) streaming
//! `DisplayItem`s over an mpsc channel; the UI loop polls crossterm events and
//! the channel with an 80ms timeout, so the status spinner animates and
//! Ctrl+C/Ctrl+D work while a turn runs (they arm quit-after-turn; other keys
//! are ignored). The `App` reducer and all rendering decisions stay pure and
//! tested; this file keeps only the raw terminal I/O.

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

use slimcode_agent::agent::{CancelToken, RunConfig, Tool};
use slimcode_agent::session::{Message, Session};
use slimcode_ai::{BailianConfig, BailianProvider};
use slimcode_common::context::{ContextBuilder, Environment};
use slimcode_common::context_files::ContextFile;
use slimcode_common::history::{
    HISTORY_DISPLAY, HistoryStore, render_history, resolve_replay_index,
};
use slimcode_common::render::{DisplayItem, Renderer};
use slimcode_common::session::{SessionStore, infer_title};
use slimcode_common::skills::{
    SkillScope, SkillStore, find_skill, is_builtin_command, parse_install_args,
};

use crate::app::{App, Effect};
use crate::footer::FooterUsage;
use crate::git::{current_branch, terminal_title};

/// One UI-loop frame: ~80ms, matching pi's loader interval so the status
/// spinner animates at the same rate and the input stays responsive.
const FRAME_MS: u64 = 80;

/// Run the TUI to completion: build the runtime, open the alternate screen,
/// pump events, and restore the terminal on every exit path.
///
/// `config` is already fully resolved (four-layer precedence); construction of
/// the provider + tools happens here, before the screen opens.
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
    // One token shared by the cancellable tool set and every turn's worker:
    // Esc cancels whichever phase the run is in (ticket 07).
    let cancel = CancelToken::new();
    let (provider, tools) = slimcode_common::setup::setup_with_cancel(cwd, config, &cancel)?;
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
    ) {
        Ok(ui) => ui,
        Err(e) => {
            restore_terminal();
            return Err(e);
        }
    };
    set_title(&ui.session.id, &ui.cwd);
    ui.app.set_branch(current_branch(&ui.cwd));
    let result = ui.run_loop();
    restore_terminal();
    result
}

/// Set the OSC 0 terminal title (spec §Implementation Decisions "Terminal
/// title"). Best-effort: a title failure must not kill the TUI.
fn set_title(session_id: &str, cwd: &Path) {
    let _ = execute!(stdout(), SetTitle(terminal_title(session_id, cwd)));
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
            skills.list().unwrap_or_default(),
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
            self.app.push_error(e);
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
                self.app.clear_for_new_session(&self.session.id);
                self.app.set_branch(current_branch(&self.cwd));
                set_title(&self.session.id, &self.cwd);
                Ok(())
            }
            Effect::LoadSession(id) => {
                let loaded = self.store.load(&id)?;
                if let Some(title) = &loaded.title {
                    self.app.push_notice(format!("  title: {title}"));
                }
                self.session = loaded;
                self.app.apply_loaded_session(&self.session.id);
                self.app.set_branch(current_branch(&self.cwd));
                set_title(&self.session.id, &self.cwd);
                Ok(())
            }
            Effect::SaveSession => {
                let path = self.store.save(&self.session)?;
                self.app.push_notice(format!("saved: {}", path.display()));
                Ok(())
            }
            Effect::ListSessions => {
                for id in self.store.list()? {
                    self.app.push_notice(format!("  {id}"));
                }
                Ok(())
            }
            Effect::ShowUsage => {
                let usage = self.total_usage();
                self.app.render(&DisplayItem::Usage(usage))?;
                Ok(())
            }
            Effect::ListHistory => {
                let entries = self.history.load()?;
                for line in render_history(&entries, HISTORY_DISPLAY) {
                    self.app.push_notice(line);
                }
                Ok(())
            }
            Effect::InstallSkill(reference) => {
                let result = parse_install_args(Some(&reference))
                    .and_then(|(path, scope)| install_skill(self.skills, &path, scope));
                match result {
                    Ok(msg) => {
                        self.app.push_notice(msg);
                        // Re-read the store so `/` completion, `did you mean`,
                        // and skill dispatch see the new skill without a restart.
                        let fresh = self.skills.list().unwrap_or_default();
                        self.app.set_skills(fresh);
                    }
                    Err(e) => self.app.push_error(e),
                }
                Ok(())
            }
        }
    }

    /// The provider's cumulative usage (session totals for the footer).
    fn total_usage(&self) -> slimcode_ai::TokenUsage {
        self.provider
            .as_ref()
            .map(|p| p.total_usage)
            .unwrap_or_default()
    }

    /// Run a prompt as a fresh turn. `record` controls whether the raw prompt
    /// is appended to input history (true for typed prompts, false for
    /// replays and skill triggers). The context is built from a clone of the
    /// session messages so a failed turn leaves the conversation intact (the
    /// session is persisted either way, see [`Tui::finish_turn`]).
    fn submit_prompt(&mut self, prompt: String, record: bool) -> Result<(), String> {
        let skills = self.skills.list().unwrap_or_default();
        let context = ContextBuilder::new()
            .with_environment(self.environment.clone())
            .with_context_files(&self.context_files)
            .with_skills(&skills)
            .with_history(self.session.messages.clone())
            .with_user_prompt(&prompt)
            .build()?;
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
        self.app.push_user_prompt(prompt);
        self.submit_prompt(prompt.to_string(), false)
    }

    /// Trigger a skill as a fresh turn; never recorded in input history.
    fn trigger_skill(&mut self, name: &str, arg: Option<&str>) -> Result<(), String> {
        let skills = self.skills.list().unwrap_or_default();
        let Some(skill) = find_skill(&skills, name) else {
            self.app.push_error(format!("no such skill: /{name}"));
            return Ok(());
        };
        let context = ContextBuilder::new()
            .with_environment(self.environment.clone())
            .with_context_files(&self.context_files)
            .with_skills(&skills)
            .with_history(self.session.messages.clone())
            .with_skill(skill, arg)
            .build()?;
        self.finish_turn(context, None)
    }

    /// Shared tail of a prompt/skill turn: infer the title, record history
    /// (before the turn, so a failed turn still records what was typed),
    /// stream the turn live into the transcript, persist the session (even on
    /// a failed turn, per spec), and feed the footer usage totals.
    fn finish_turn(&mut self, context: Vec<Message>, record: Option<&str>) -> Result<(), String> {
        if self.session.title.is_none() {
            self.session.title = infer_title(&context);
        }
        if let Some(prompt) = record
            && let Err(e) = self.history.append(prompt)
        {
            self.app.push_notice(format!("history: {e}"));
        }
        let updated = self.drive_turn(context);
        let updated = match updated {
            Ok(updated) => updated,
            Err(e) => {
                // Spec: a failed turn still leaves the session persisted, with
                // its prior messages intact (the context was built from a
                // clone, so `session.messages` was never emptied).
                if let Err(save_err) = self.store.save(&self.session) {
                    self.app
                        .push_notice(format!("session save failed: {save_err}"));
                }
                return Err(e);
            }
        };
        self.session.messages = updated;
        self.store.save(&self.session)?;
        self.app.set_usage(FooterUsage::from(&self.total_usage()));
        Ok(())
    }

    /// Drive one turn on a worker thread (ADR-0006 D6). The worker runs the
    /// shared runner against the moved provider + tools, streaming every
    /// `DisplayItem` over an mpsc channel; this UI loop polls terminal events
    /// and the channel at [`FRAME_MS`], draining items into the app and
    /// redrawing so the spinner animates. Ctrl+C/Ctrl+D record quit-after-turn
    /// (the turn keeps streaming); bare Esc cancels the running turn (ticket
    /// 07 — the token is reset here so every turn starts uncancelled); all
    /// other keys are ignored while running. The provider and tools are always
    /// restored before returning.
    fn drive_turn(&mut self, messages: Vec<Message>) -> Result<Vec<Message>, String> {
        self.cancel.reset();
        self.app.set_running(true);
        let cfg = RunConfig::default();
        let (tx, rx) = mpsc::channel::<DisplayItem>();
        let mut provider = self.provider.take().expect("provider present while idle");
        let tools = std::mem::take(&mut self.tools);
        let cancel = self.cancel.clone();
        let handle = thread::spawn(move || {
            let result = slimcode_common::runner::run_turn(
                &mut provider,
                &tools,
                messages,
                &cfg,
                &cancel,
                &mut ChannelRenderer { tx },
            );
            (result, provider, tools)
        });

        // The UI loop runs until the worker finishes. A terminal/UI error is
        // remembered but the loop still drains the channel and joins, so the
        // provider is always restored and the turn result is never lost.
        let mut ui_error: Option<String> = None;
        let mut quit_after_turn = false;
        let (result, provider, tools) = loop {
            // Polling is also the frame timer (~80ms); on a failed poll once
            // an error is recorded we still sleep so the loop is not hot.
            match event::poll(Duration::from_millis(FRAME_MS)) {
                Ok(true) => match event::read() {
                    Ok(Event::Key(key)) if ui_error.is_none() => {
                        match self.app.handle_key_running(key) {
                            Some(Effect::QuitAfterTurn) => quit_after_turn = true,
                            // Esc: abort the in-flight request / tool at the
                            // next runner boundary; the worker drains and
                            // joins as usual and the turn ends Cancelled.
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
            drain_channel(&rx, &mut self.app, &mut ui_error)?;
            if handle.is_finished() {
                drain_channel(&rx, &mut self.app, &mut ui_error)?;
                break handle
                    .join()
                    .map_err(|_| "turn worker panicked".to_string())?;
            }
            if ui_error.is_none() {
                self.draw()?;
            }
            self.app.tick();
        };

        self.quit_after_turn |= quit_after_turn;
        self.provider = Some(provider);
        self.tools = tools;
        self.app.set_running(false);
        if let Some(err) = ui_error {
            return Err(err);
        }
        result
    }

    /// Draw one frame.
    fn draw(&mut self) -> Result<(), String> {
        self.terminal
            .draw(|frame| self.app.draw(frame, frame.area()))
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

/// Drain every item the worker has queued into the app's transcript,
/// recording a render failure into `ui_error` (the drive loop reports it
/// after the worker joins). Always drains what it can first.
fn drain_channel(
    rx: &mpsc::Receiver<DisplayItem>,
    app: &mut App,
    ui_error: &mut Option<String>,
) -> Result<(), String> {
    while let Ok(item) = rx.try_recv() {
        if let Err(e) = app.render(&item) {
            if ui_error.is_none() {
                *ui_error = Some(e);
            }
            // Re-rendering after a failure is unlikely to succeed; stop.
            break;
        }
    }
    Ok(())
}

/// A [`Renderer`] that forwards display items across the thread boundary to
/// the UI loop's channel (ADR-0006 D6). The worker thread never touches the
/// [`App`]; the UI loop applies items on the main thread.
struct ChannelRenderer {
    tx: mpsc::Sender<DisplayItem>,
}

impl Renderer for ChannelRenderer {
    fn render(&mut self, item: &DisplayItem) -> Result<(), String> {
        self.tx.send(item.clone()).map_err(|e| e.to_string())
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
    use slimcode_agent::agent::{Delta, FinishReason, Provider, RunConfig, StopReason, Tool};
    use slimcode_agent::session::{Message, Role};
    use slimcode_common::render::DisplayItem;
    use slimcode_common::runner::run_turn;

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
            _tools: &[Tool],
            _cancel: &slimcode_agent::agent::CancelToken,
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

    /// The channel worker, end to end: a scripted provider + ChannelRenderer
    /// on a worker thread stream an ordered DisplayItem stream over the
    /// channel, and `run_turn`'s result is the final message list (ticket 03
    /// acceptance; ADR-0006 D6).
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
        let messages = vec![Message::text(Role::User, "hi")];
        let cfg = RunConfig::default();
        let (tx, rx) = mpsc::channel::<DisplayItem>();

        let handle = thread::spawn(move || {
            let mut provider = provider;
            let tools = vec![weather_tool()];
            let cancel = slimcode_agent::agent::CancelToken::new();
            let result = run_turn(
                &mut provider,
                &tools,
                messages,
                &cfg,
                &cancel,
                &mut ChannelRenderer { tx },
            );
            (result, provider.calls)
        });

        let mut items: Vec<DisplayItem> = Vec::new();
        while let Ok(item) = rx.recv() {
            items.push(item);
        }
        let (result, calls) = handle.join().unwrap();
        let updated = result.expect("turn succeeds");

        // The ordered stream the UI loop would apply: two turn markers, the
        // streamed text fragments (each delta its own item), the tool
        // start/result pair, and the completed stop. No Usage item: token
        // usage is frontend-owned (the shell feeds the footer separately).
        let kinds: Vec<&str> = items
            .iter()
            .map(|i| match i {
                DisplayItem::Turn { .. } => "turn",
                DisplayItem::Reasoning(_) => "reasoning",
                DisplayItem::Text(_) => "text",
                DisplayItem::ToolStart { .. } => "tool-start",
                DisplayItem::ToolResult { .. } => "tool-result",
                DisplayItem::Stop(_) => "stop",
                DisplayItem::Usage(_) => "usage",
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                "turn",
                "text",
                "text",
                "tool-start",
                "tool-result",
                "turn",
                "text",
                "stop"
            ]
        );
        // The individual items match the scripted stream.
        assert!(items.contains(&DisplayItem::Text("hello ".to_string())));
        assert!(items.contains(&DisplayItem::Text("world".to_string())));
        assert!(items.contains(&DisplayItem::ToolStart {
            name: "get_weather".to_string(),
            arguments: r#"{"city": "Tokyo"}"#.to_string(),
        }));
        assert!(items.contains(&DisplayItem::ToolResult {
            name: "get_weather".to_string(),
            ok: true,
            result: r#"{"city": "Tokyo", "temp": "25C"}"#.to_string(),
        }));
        assert!(items.contains(&DisplayItem::Text("it is 25C".to_string())));
        assert!(items.contains(&DisplayItem::Stop(StopReason::Completed)));
        // Two provider calls: the streaming+tool batch, then the answer batch.
        assert_eq!(calls, 2);
        // The final messages include the assistant turn + tool result.
        assert!(updated.iter().any(|m| m.role == Role::Assistant));
    }
}
