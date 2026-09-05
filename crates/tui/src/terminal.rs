//! The thin crossterm/ratatui terminal loop that wraps the pure [`App`]
//! (spec §Implementation Decisions: the pure core owns all behavior, and the
//! terminal loop is a thin, untested shell). This module only pumps events,
//! fulfils the App's I/O effects, and draws frames; everything testable lives
//! in `crate::app` and is covered by `TestBackend` tests.
//!
//! Setup (provider + tools) happens **before** the alternate screen opens, so
//! config or API-key errors surface on the normal terminal (spec user story
//! 28). During a run the loop does not poll keys — the `Provider` seam is
//! synchronous, so `run_turn` streams events to the renderer and the renderer
//! redraws live; the loop returns to `event::read` only after the turn
//! finishes.

use std::io::stdout;
use std::path::Path;

use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use slimcode_agent::agent::{RunConfig, Tool};
use slimcode_agent::session::{Message, Session};
use slimcode_ai::{BailianConfig, BailianProvider};
use slimcode_common::context::ContextBuilder;
use slimcode_common::history::{
    HISTORY_DISPLAY, HistoryStore, render_history, resolve_replay_index,
};
use slimcode_common::render::{DisplayItem, Renderer};
use slimcode_common::session::{SessionStore, infer_title};
use slimcode_common::skills::{
    SkillScope, SkillStore, find_skill, is_builtin_command, parse_install_args,
};

use crate::app::{App, Effect};

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
) -> Result<i32, String> {
    let model = config.model.clone();
    let (provider, tools) = slimcode_common::setup::setup(cwd, config)?;
    let session = store.new_session();
    let mut ui = match Tui::new(provider, tools, store, history, skills, session, cwd, model) {
        Ok(ui) => ui,
        Err(e) => {
            restore_terminal();
            return Err(e);
        }
    };
    let result = ui.run_loop();
    restore_terminal();
    result
}

/// The full TUI session state. Owns the pure [`App`] and everything the loop
/// needs to fulfil its effects.
struct Tui<'a> {
    app: App,
    terminal: Terminal<CrosstermBackend<std::io::Stdout>>,
    provider: BailianProvider,
    tools: Vec<Tool>,
    store: &'a SessionStore,
    history: &'a HistoryStore,
    skills: &'a SkillStore,
    session: Session,
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
            skills.list().unwrap_or_default(),
        );
        // Seed the ↑/↓ recall snapshot from the shared HistoryStore.
        app.set_history(history.load().unwrap_or_default());

        let mut ui = Tui {
            app,
            terminal,
            provider,
            tools,
            store,
            history,
            skills,
            session,
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
            self.draw()?;
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
            Effect::SubmitPrompt(prompt) => self.submit_prompt(prompt, true),
            Effect::ReplayPrompt(prompt) => self.submit_prompt(prompt, false),
            Effect::ReplayHistory(n) => self.replay_history(n),
            Effect::TriggerSkill { name, arg } => self.trigger_skill(&name, arg.as_deref()),
            Effect::NewSession => {
                self.session = self.store.new_session();
                self.app.clear_for_new_session(&self.session.id);
                Ok(())
            }
            Effect::LoadSession(id) => {
                let loaded = self.store.load(&id)?;
                if let Some(title) = &loaded.title {
                    self.app.push_notice(format!("  title: {title}"));
                }
                self.session = loaded;
                self.app.apply_loaded_session(&self.session.id);
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
                let usage = self.provider.total_usage;
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

    /// Run a prompt as a fresh turn. `record` controls whether the raw prompt
    /// is appended to input history (true for typed prompts, false for
    /// replays and skill triggers). The context is built from a clone of the
    /// session messages so a failed turn leaves the conversation intact (the
    /// session is persisted either way, see [`Tui::finish_turn`]).
    fn submit_prompt(&mut self, prompt: String, record: bool) -> Result<(), String> {
        let skills = self.skills.list().unwrap_or_default();
        let context = ContextBuilder::new()
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
        // `> prompt` echo the recall path uses).
        self.app.push_notice(format!("> {prompt}"));
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
            .with_skills(&skills)
            .with_history(self.session.messages.clone())
            .with_skill(skill, arg)
            .build()?;
        self.finish_turn(context, None)
    }

    /// Shared tail of a prompt/skill turn: infer the title, record history
    /// (before the turn, so a failed turn still records what was typed),
    /// stream the turn live into the transcript, persist the session (even on
    /// a failed turn, per spec), and show token usage.
    fn finish_turn(&mut self, context: Vec<Message>, record: Option<&str>) -> Result<(), String> {
        if self.session.title.is_none() {
            self.session.title = infer_title(&context);
        }
        if let Some(prompt) = record
            && let Err(e) = self.history.append(prompt)
        {
            self.app.push_notice(format!("history: {e}"));
        }
        self.app.set_running(true);
        let updated = self.drive_turn(context);
        self.app.set_running(false);
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
        let usage = self.provider.total_usage;
        self.app.render(&DisplayItem::Usage(usage))?;
        Ok(())
    }

    /// Drive one turn through the shared runner, streaming every display item
    /// into the app and redrawing the terminal live as it streams.
    fn drive_turn(&mut self, messages: Vec<Message>) -> Result<Vec<Message>, String> {
        let cfg = RunConfig::default();
        let mut live = LiveRenderer {
            app: &mut self.app,
            terminal: &mut self.terminal,
        };
        slimcode_common::runner::run_turn(
            &mut self.provider,
            &self.tools,
            messages,
            &cfg,
            &mut live,
        )
    }

    /// Draw one frame.
    fn draw(&mut self) -> Result<(), String> {
        self.terminal
            .draw(|frame| self.app.draw(frame, frame.area()))
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

/// A [`Renderer`] that streams into the pure [`App`] and redraws the terminal
/// after every item, so output draws live during a synchronous run.
struct LiveRenderer<'a> {
    app: &'a mut App,
    terminal: &'a mut Terminal<CrosstermBackend<std::io::Stdout>>,
}

impl Renderer for LiveRenderer<'_> {
    fn render(&mut self, item: &DisplayItem) -> Result<(), String> {
        self.app.render(item)?;
        self.terminal
            .draw(|frame| self.app.draw(frame, frame.area()))
            .map_err(|e| e.to_string())?;
        Ok(())
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
