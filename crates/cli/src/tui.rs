//! The TUI's entry point: process lifecycle (raw mode, alternate screen, the
//! terminal title, the exit code) and the application handler the terminal
//! library drives (ADR-0013).
//!
//! The turn orchestration lives in the SDK: this module owns what is genuinely
//! frontend work — argv-side terminal setup, the session object (create,
//! switch, dispose), `/` command semantics, input history, the session picker
//! list, the display adapter — and consumes `slimcode-sdk` for everything a
//! turn does. The library keeps only what exists to keep the frame loop
//! responsive and knows the app through two vocabularies: [`Effect`]s out,
//! [`RenderItem`]s in.

use std::cell::Cell;
use std::io::{Write, stdout};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, SetTitle, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use slimcode_ai::{ProviderConfig, TokenUsage};
use slimcode_app::history::{HISTORY_DISPLAY, HistoryStore, render_history, resolve_replay_index};
use slimcode_app::render::usage_summary;
use slimcode_app::session::{SessionStore, format_minute, unix_secs};
use slimcode_app::skills::{
    PreparedPrompt, Skill, SkillScope, SkillStore, is_builtin_command, parse_install_args,
    prepare_user_prompt,
};
use slimcode_sdk::{
    AgentSession, AgentSessionOptions, Provider, SdkEvent, Tool, create_agent_session,
};
use slimcode_tui::app::{App, Effect};
use slimcode_tui::git::{current_branch, terminal_title};
use slimcode_tui::handler::{
    CompletionItem, CompletionProvider, ControlFlow, Prompt, TurnReport, UiHandler,
};
use slimcode_tui::render::{RenderItem, SessionRow};

use crate::render::{TuiAdapter, history_to_render_items, to_context_usage, to_footer_usage};

/// Enter the TUI: open the session (the SDK owns the store, skills, context
/// files, provider and tools), take over the terminal, run the frame loop, and
/// restore the terminal on every exit path.
pub fn run(
    cwd: &Path,
    home: &Path,
    config: ProviderConfig,
    store: &SessionStore,
    history: &HistoryStore,
    skills: &SkillStore,
) -> Result<i32, String> {
    let model = config.model.clone();
    // The CLI keeps the session object and the display state around it; the
    // SDK assembled everything else. A config / API-key failure surfaces here,
    // on the normal terminal, before the alternate screen opens.
    let mut handler = TuiSession::new(
        home.to_path_buf(),
        cwd.to_path_buf(),
        config,
        store,
        history,
        skills,
        None,
    )?;
    let session = handler.session();

    // The `/`-candidate pool (ADR-0014 D3): the command registry plus the
    // skills snapshot, shared with the handler so `/install-skill` refreshes
    // it without a restart.
    let mut app = App::new(
        cwd.display().to_string(),
        session.session_id().to_string(),
        model,
        slimcode_tui::VERSION,
        Box::new(handler.completions.clone()),
        to_context_usage(&session.messages()),
    );
    app.set_history(history.load().unwrap_or_default());
    app.apply(RenderItem::Branch(current_branch(cwd)));

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
    // Dispose the session on the way out: its listener (and the adapter it
    // holds) is dropped and an in-flight turn is cancelled.
    handler.session().dispose();
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

// The emit channel of whichever thread is driving a turn.
//
// The SDK delivers its events synchronously on the thread that called
// `prompt` / `compact_now`, and the session's listener has no other way to
// reach the library's `emit` callback (which is only valid for the duration
// of that call). `submit` (the worker thread) and `/compact` (the frame
// thread) therefore bind their own `emit` here for the call, and the listener
// reads the slot on the same thread: a thread-local, so there is no locking
// and no cross-thread access.
thread_local! {
    static EMIT_SLOT: Cell<Option<*mut ErasedEmit>> = const { Cell::new(None) };
}

/// `dyn FnMut(RenderItem)` with the (scoped) borrow's lifetime erased, so a
/// thread-local can hold a pointer to the caller's closure. The erasure is
/// only sound inside [`with_emit_slot`]'s scope; nothing else may build one.
type ErasedEmit = dyn FnMut(RenderItem) + 'static;

/// Bind `emit` as this thread's channel for the duration of `f`, clearing the
/// slot afterwards even on unwind.
fn with_emit_slot<R>(emit: &mut dyn FnMut(RenderItem), f: impl FnOnce() -> R) -> R {
    /// Clears the slot on every exit path.
    struct Clear;
    impl Drop for Clear {
        fn drop(&mut self) {
            EMIT_SLOT.with(|slot| slot.set(None));
        }
    }
    let pointer: *mut (dyn FnMut(RenderItem) + '_) = emit;
    // SAFETY: `pointer` is derived from `emit`, which stays borrowed for the
    // whole call, and the slot is cleared by the guard below before that
    // borrow ends — so the erased lifetime is never observed outside the
    // scope the pointer is valid in.
    let erased = unsafe {
        std::mem::transmute_copy::<*mut (dyn FnMut(RenderItem) + '_), *mut ErasedEmit>(&pointer)
    };
    EMIT_SLOT.with(|slot| slot.set(Some(erased)));
    let _clear = Clear;
    f()
}

/// Forward one render item to this thread's bound channel. An item arriving
/// with no channel bound is dropped: events only arrive on a thread driving a
/// turn, which always binds one.
fn emit_render_item(item: RenderItem) {
    let emit = EMIT_SLOT.with(|slot| slot.get());
    if let Some(emit) = emit {
        // SAFETY: the slot holds a pointer derived from the `&mut` that
        // `with_emit_slot` still borrows for the whole call, and it is set and
        // read on that thread only (the slot is thread-local) and cleared
        // before the borrow ends, so the reference cannot outlive it or race
        // another access.
        let emit = unsafe { &mut *emit };
        emit(item);
    }
}

/// The closure form of [`emit_render_item`], for the `emit` parameters.
fn slot_emitter() -> impl FnMut(RenderItem) {
    emit_render_item
}

/// Rebuilds one session's provider and tools.
///
/// An `AgentSession` owns the provider and tools it runs, so a `/new` or a load
/// needs fresh ones: this is the CLI's host / test seam, while production
/// passes `None` and every session gets the SDK's `BailianProvider` and the
/// seven default tools bound to `cwd`.
type SessionDeps = Arc<dyn Fn() -> (Box<dyn Provider + Send>, Vec<Tool>) + Send + Sync>;

/// The session the handler currently drives and the adapter its events render
/// through. Both are `Arc`-shared: a turn runs on the worker thread with a
/// clone of the session, and the session's listener holds the adapter.
struct Inner {
    session: Arc<AgentSession>,
    adapter: Arc<Mutex<TuiAdapter>>,
}

/// Open a session and the adapter its events render through.
///
/// The SDK assembles everything else: the session store under the pinned home,
/// the provider and tools (`deps` for a host / test seam, the SDK defaults
/// otherwise), skill discovery (from the explicit `skills` snapshot) and
/// context-file discovery. The frozen startup config rides along as overrides,
/// which win in the SDK's four-layer resolution — so a new session reproduces
/// the values the CLI resolved at startup instead of re-reading `config.toml`
/// (ADR-0016). `session_id` resumes a persisted session (`/session`) instead of
/// starting fresh.
fn open_session(
    cwd: &Path,
    home: &Path,
    config: &ProviderConfig,
    skills: Vec<Skill>,
    deps: Option<&SessionDeps>,
    session_id: Option<String>,
) -> Result<Inner, String> {
    let mut options = AgentSessionOptions::new(Some(cwd.to_path_buf()))
        .with_home(home.to_path_buf())
        .with_base_url(config.base_url.clone())
        .with_model(config.model.clone())
        .with_api_key(config.api_key.clone())
        .with_cache(config.cache)
        .with_skills(skills.clone());
    if let Some(id) = session_id {
        options = options.with_session_id(id);
    }
    if let Some(deps) = deps {
        let (provider, tools) = deps();
        options = options.with_provider(provider).with_tools(tools);
    }
    let session = Arc::new(create_agent_session(options)?);
    let adapter = TuiAdapter::new(skills, cwd.to_path_buf());
    // The live view starts in lockstep with the session: a fresh session is
    // empty, a resumed one already has a history whose footer estimate the
    // first usage render must report.
    adapter.reseed(session.messages(), session.usage());
    let adapter = Arc::new(Mutex::new(adapter));
    // The returned unsubscribe handle is dropped on purpose: the listener
    // lives for the session's whole life, and `dispose` clears it.
    let _unsubscribe = session.subscribe({
        let adapter = Arc::clone(&adapter);
        // A weak handle, not a clone: the listener lives inside the session's
        // listener list, so holding the session strongly here would leak it.
        let session = Arc::downgrade(&session);
        move |event| route_event(&adapter, session.upgrade().as_deref(), event)
    });
    Ok(Inner { session, adapter })
}

/// Route one SDK event: display items render through the adapter, messages
/// feed its live view (the streamed display already shows them, so nothing is
/// rendered), the turn end needs nothing, and a compaction re-reads the
/// session into the live view, refreshes the footer and reports the token
/// change — the same rendering `/compact` gets, because both paths end in the
/// same `Compacted` event.
///
/// `session` is `None` only when the session is already gone (the listener
/// cannot outlive it for long).
fn route_event(adapter: &Arc<Mutex<TuiAdapter>>, session: Option<&AgentSession>, event: SdkEvent) {
    let mut emit = slot_emitter();
    let adapter = adapter.lock().unwrap_or_else(|e| e.into_inner());
    match event {
        SdkEvent::Display(item) => adapter.render(&item, &mut emit),
        SdkEvent::Message { message, usage } => adapter.note_message(&message, usage.as_ref()),
        SdkEvent::TurnFinished { .. } => {}
        SdkEvent::Compacted { before, after } => {
            // The event carries only the token counts: the compacted history
            // itself comes from the session, so the footer's context segment
            // is the post-compaction estimate.
            if let Some(session) = session {
                adapter.reseed(session.messages(), session.usage());
            }
            adapter.note_compacted(before, after, &mut emit);
        }
    }
}

/// The CLI's application handler: everything the TUI needs from a frontend
/// that knows about sessions, skills and the model (ADR-0013 D1).
struct TuiSession<'a> {
    /// The pinned slimcode home: the session store root, and where the SDK
    /// discovers skills and context files (ADR-0009).
    home: PathBuf,
    cwd: PathBuf,
    store: &'a SessionStore,
    history: &'a HistoryStore,
    skills: &'a SkillStore,
    /// The provider config resolved once at startup (ADR-0016). Frozen: it is
    /// passed to every session as overrides, so `/new` and a load never drift
    /// from the model the footer shows (spec decision 4).
    config: ProviderConfig,
    /// The provider / tool seam (a host or a test builds sessions with its own
    /// provider); `None` = the SDK defaults.
    deps: Option<SessionDeps>,
    completions: CliCompletions,
    inner: Mutex<Inner>,
}

impl<'a> TuiSession<'a> {
    /// Wire the handler around its first session. The session itself is built
    /// by the SDK (`open_session`), so a provider / config error surfaces here
    /// before the terminal is taken over.
    #[allow(clippy::too_many_arguments)]
    fn new(
        home: PathBuf,
        cwd: PathBuf,
        config: ProviderConfig,
        store: &'a SessionStore,
        history: &'a HistoryStore,
        skills: &'a SkillStore,
        deps: Option<SessionDeps>,
    ) -> Result<TuiSession<'a>, String> {
        let inner = open_session(
            &cwd,
            &home,
            &config,
            skills.list().unwrap_or_default(),
            deps.as_ref(),
            None,
        )?;
        Ok(TuiSession {
            home,
            cwd,
            store,
            history,
            skills,
            config,
            deps,
            completions: CliCompletions::new(skills.list().unwrap_or_default()),
            inner: Mutex::new(inner),
        })
    }

    /// The session the handler currently drives, cloned out under a short
    /// lock: nothing is held across a turn, and `cancel` never waits for one.
    fn session(&self) -> Arc<AgentSession> {
        Arc::clone(&self.inner.lock().unwrap_or_else(|e| e.into_inner()).session)
    }

    /// The session and the adapter its events render through, cloned out the
    /// same way.
    fn current(&self) -> (Arc<AgentSession>, Arc<Mutex<TuiAdapter>>) {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        (Arc::clone(&inner.session), Arc::clone(&inner.adapter))
    }

    fn session_id(&self) -> String {
        self.session().session_id().to_string()
    }

    /// Build a replacement session: `/new` (`None`) or a load (`Some(id)`).
    fn open(&self, session_id: Option<String>) -> Result<Inner, String> {
        open_session(
            &self.cwd,
            &self.home,
            &self.config,
            self.skills_snapshot(),
            self.deps.as_ref(),
            session_id,
        )
    }

    /// Install a freshly built session as the current one, disposing the old
    /// one (which also drops its listener). Only the frame thread reaches this,
    /// and only while the worker is idle — the library ignores ordinary keys
    /// during a turn (ADR-0013 D2) — so no turn can race the swap.
    fn replace_session(&self, opened: Inner) {
        let old = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            let old = std::mem::replace(&mut inner.session, opened.session);
            inner.adapter = opened.adapter;
            old
        };
        old.dispose();
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
                "/compact" => return self.compact_now(emit),
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
        // Skill trigger: `/skill:name` or the canonical `/{name}`, expanded
        // through the shared Prompt preprocessing seam (ADR-0026) so the
        // one-shot, the GUI and the TUI inject a skill identically. The seam
        // dedups against this session's messages; a leading `/`-form that names
        // no installed skill falls through to the unknown-command notice below.
        let skills = self.skills_snapshot();
        let input = match arg {
            Some(arg) => format!("{name} {arg}"),
            None => name.to_string(),
        };
        if let PreparedPrompt::Skill {
            name,
            model_text,
            display,
        } = prepare_user_prompt(&input, &skills, &self.session().messages())
        {
            // The skill active block is shown as its own display unit before
            // the turn starts, like the replay path shows the boxed prompt. The
            // seam's `display` carries the inner content (name/`location` split
            // off), matching pi's `ParsedSkillBlock`; the TUI shows only the
            // name until Ctrl+O expands it.
            emit(RenderItem::Skill {
                name,
                content: display,
            });
            return ControlFlow::Submit(Prompt {
                text: model_text,
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
    /// branch/title. The footer's usage + context restart: the Usage item
    /// right after the SessionChanged re-establishes the context segment
    /// (hidden by the change, ticket 05) at the fresh session's 0%-estimate.
    ///
    /// The session is built by the SDK from the same frozen startup config, so
    /// `/new` cannot drift to another model or provider.
    fn new_session(&self, emit: &mut dyn FnMut(RenderItem)) -> ControlFlow {
        let opened = match self.open(None) {
            Ok(opened) => opened,
            Err(e) => {
                emit(RenderItem::Error(e));
                return ControlFlow::Continue;
            }
        };
        let id = opened.session.session_id().to_string();
        self.replace_session(opened);
        emit(RenderItem::SessionChanged { id: id.clone() });
        emit(RenderItem::Usage {
            usage: to_footer_usage(&TokenUsage::default()),
            context: Some(to_context_usage(&[])),
        });
        emit(RenderItem::Notice(format!("new session: {id}")));
        emit(RenderItem::Branch(current_branch(&self.cwd)));
        set_title(&id, &self.cwd);
        ControlFlow::Continue
    }

    /// Load a saved session: the SDK replays the id's log (title, messages,
    /// the newest compaction checkpoint as the boundary), the transcript shows
    /// the restored conversation again and the footer restarts from zero usage
    /// with the restored history's context estimate.
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
        let opened = match self.open(Some(id.to_string())) {
            Ok(opened) => opened,
            Err(e) => {
                emit(RenderItem::Error(e));
                return ControlFlow::Continue;
            }
        };
        let session = Arc::clone(&opened.session);
        let id = session.session_id().to_string();
        let title = session.title();
        // Usage is never persisted (ADR-0018 D4: it is a live-session
        // concern), so a loaded session starts at zero; its context estimate
        // comes from the restored history. Both feed the Usage item that
        // re-establishes the footer's context segment after the change
        // (hidden by SessionChanged, ticket 05).
        let messages = session.messages();
        let history_items = history_to_render_items(&messages);
        self.replace_session(opened);
        emit(RenderItem::SessionChanged { id: id.clone() });
        emit(RenderItem::Usage {
            usage: to_footer_usage(&session.usage()),
            context: Some(to_context_usage(&messages)),
        });
        if let Some(title) = &title {
            emit(RenderItem::Notice(format!("  title: {title}")));
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
    /// (`usage_summary`, ADR-0014 D4). The session is the display's source
    /// (ADR-0018 D4).
    fn show_usage(&self, emit: &mut dyn FnMut(RenderItem)) -> ControlFlow {
        let usage = self.session().usage();
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

    /// `/compact`: summarize the live session's older history on demand,
    /// regardless of the threshold (spec `.scratch/compact`).
    ///
    /// The frame thread drives the compaction, so it binds its own emit channel
    /// for the call: the session's listener renders the result through it —
    /// the footer's usage + context and the token change — exactly like the
    /// automatic path, which reaches the same `Compacted` event. Only a failure
    /// is reported here, from the return value (the automatic path stays silent
    /// for failures).
    fn compact_now(&self, emit: &mut dyn FnMut(RenderItem)) -> ControlFlow {
        let session = self.session();
        emit(RenderItem::Notice("compressing context…".to_string()));
        let result = with_emit_slot(emit, || session.compact_now());
        if let Err(e) = result {
            emit(RenderItem::Error(format!("compaction failed: {e}")));
        }
        ControlFlow::Continue
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
        // A short lock clones the session and its adapter out; the turn then
        // runs without holding anything, so `cancel` (the frame thread) only
        // ever touches the session's cancel token and never waits here.
        let (session, adapter) = self.current();
        // The typed prompt is recorded before the turn, so a failed turn still
        // records what the user wrote (input history stays a CLI concern,
        // ADR-0013 D3).
        if prompt.record
            && let Err(e) = self.history.append(&prompt.text)
        {
            emit(RenderItem::Notice(format!("history: {e}")));
        }
        // The adapter's live view starts this turn in lockstep with the
        // session: the per-message usage renders then report the session's
        // history and totals (ADR-0018 D4).
        adapter
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .reseed(session.messages(), session.usage());
        let before = session.messages().len();
        // The SDK delivers its events synchronously on this worker thread and
        // only while `prompt` runs, so binding this thread's emit channel here
        // is what keeps the turn streaming into the transcript.
        let result = with_emit_slot(emit, || session.prompt(&prompt.text));
        result.map(|_| TurnReport {
            // `appended` counts what the turn itself produced: everything that
            // entered history except the prompt it was built from.
            appended: session.messages().len().saturating_sub(before + 1),
        })
    }

    fn cancel(&self) {
        // Only sets the session's cancel token: never waits on the session
        // lock, so Esc reaches an in-flight turn (and its bash child) from the
        // frame thread (ADR-0013 D2).
        self.session().abort();
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

    use std::fs;
    use std::sync::Condvar;

    use slimcode_ai::wire::TokenUsage;
    use slimcode_ai::{Delta, FinishReason};
    use slimcode_app::context::ContextBuilder;
    use slimcode_app::session::project_key;
    use slimcode_core::agent::{CancelToken, ToolSpec};
    use slimcode_core::session::{Message, Role};
    use slimcode_sdk::AgentMessage;
    use slimcode_tui::footer::{ContextUsage, FooterUsage};

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

    /// A gate a test uses to hold a request mid-flight: the provider marks the
    /// request entered and blocks until the test opens the gate.
    #[derive(Clone)]
    struct Gate {
        state: Arc<(Mutex<(bool, bool)>, Condvar)>,
    }

    impl Gate {
        fn new() -> Self {
            Self {
                state: Arc::new((Mutex::new((false, false)), Condvar::new())),
            }
        }

        /// The provider's side: mark the request entered, then block until the
        /// test opens the gate.
        fn enter(&self) {
            let (lock, cv) = &*self.state;
            let mut state = lock.lock().expect("gate lock");
            state.0 = true;
            cv.notify_all();
            while !state.1 {
                state = cv.wait(state).expect("gate wait");
            }
        }

        /// The test's side: wait until a request entered the gate.
        fn wait_entered(&self) {
            let (lock, cv) = &*self.state;
            let mut state = lock.lock().expect("gate lock");
            while !state.0 {
                state = cv.wait(state).expect("gate wait");
            }
        }

        /// The test's side: release the request.
        fn open(&self) {
            let (lock, cv) = &*self.state;
            lock.lock().expect("gate lock").1 = true;
            cv.notify_all();
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
        /// Cancel the shared token before returning the Nth call's batch
        /// (1-based), simulating Esc arriving mid-request.
        cancel_on_call: Option<usize>,
        /// Hold the request in flight until the test opens the gate.
        gate: Option<Gate>,
    }

    impl FakeProvider {
        fn new(script: Vec<Vec<Delta>>) -> Self {
            Self {
                script,
                calls: 0,
                fail_at: None,
                usage: TokenUsage::default(),
                per_call: TokenUsage::default(),
                cancel_on_call: None,
                gate: None,
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
                cancel_on_call: None,
                gate: None,
            }
        }

        /// Accumulate `usage` on every successful call.
        fn with_per_call(mut self, usage: TokenUsage) -> Self {
            self.per_call = usage;
            self
        }

        /// Cancel the shared token as the Nth call (1-based) returns.
        fn cancels_on(mut self, call: usize) -> Self {
            self.cancel_on_call = Some(call);
            self
        }

        /// Block every request on `gate` until the test opens it.
        fn blocking(mut self, gate: Gate) -> Self {
            self.gate = Some(gate);
            self
        }
    }

    impl Provider for FakeProvider {
        fn chat(
            &mut self,
            _messages: &[Message],
            _tools: &[ToolSpec],
            _config: &ProviderConfig,
            cancel: &CancelToken,
            on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
        ) -> Result<Option<TokenUsage>, String> {
            if self.fail_at == Some(self.calls) {
                return Err("boom".to_string());
            }
            if self.cancel_on_call == Some(self.calls + 1) {
                cancel.cancel();
            }
            if let Some(gate) = &self.gate {
                gate.enter();
            }
            let batch = self.script.get(self.calls).cloned().unwrap_or_default();
            self.calls += 1;
            self.usage = self.usage.saturating_add(&self.per_call);
            // A scripted provider mirrors the live one (ADR-0019): deltas go
            // through the sink, in order, as the request produces them.
            for delta in batch {
                on_delta(delta)?;
            }
            // The request's usage rides the return value (ticket 01): what the
            // runner attaches to the assistant message event. The runner
            // discards it when the cancel above won the race.
            Ok(Some(self.per_call))
        }

        fn total_usage(&self) -> TokenUsage {
            self.usage
        }
    }

    /// A provider handle every session a handler opens shares, so a scripted
    /// call sequence survives `/new` and a load: the SDK moves its provider into
    /// the session it builds, so the state has to live behind an `Arc`.
    struct SharedProvider(Arc<Mutex<FakeProvider>>);

    impl Provider for SharedProvider {
        fn chat(
            &mut self,
            messages: &[Message],
            tools: &[ToolSpec],
            config: &ProviderConfig,
            cancel: &CancelToken,
            on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
        ) -> Result<Option<TokenUsage>, String> {
            self.0
                .lock()
                .expect("provider lock")
                .chat(messages, tools, config, cancel, on_delta)
        }

        fn total_usage(&self) -> TokenUsage {
            self.0.lock().expect("provider lock").total_usage()
        }
    }

    /// The tools a session gets, built per session: a `Tool` is a boxed closure
    /// and cannot be cloned, and one handler opens several sessions.
    type Tools = Arc<dyn Fn() -> Vec<Tool> + Send + Sync>;

    /// A session with no tools.
    fn no_tools() -> Tools {
        Arc::new(Vec::new)
    }

    /// A session with the `probe` tool the scripted turns call.
    fn probe_tools() -> Tools {
        Arc::new(|| {
            vec![Tool::new(
                "probe",
                "a probe",
                serde_json::json!({}),
                |_args| Ok("probed".to_string()),
            )]
        })
    }

    /// The stores one handler needs, kept alive for the handler's borrows, plus
    /// the pinned home and cwd every session is built under.
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
            // A `.git` marker makes the project home resolve to `cwd` itself,
            // so the SDK's session store (keyed by the project home) and the
            // CLI's picker list land in the same directory deterministically.
            fs::create_dir_all(cwd.join(".git")).expect("git marker");
            Self {
                store: SessionStore::new(home.join("sessions"), project_key(&cwd), cwd.clone()),
                history: HistoryStore::new(scratch.path("history.json")),
                skills: SkillStore::new(&home, &cwd),
                scratch,
            }
        }

        fn home(&self) -> PathBuf {
            self.scratch.path("home")
        }

        fn cwd(&self) -> PathBuf {
            self.scratch.path("proj")
        }

        /// A handler over a fresh SDK session driven by `provider`: the same
        /// provider and tool set seed every session the handler opens.
        fn handler(&self, provider: FakeProvider, tools: Tools) -> TuiSession<'_> {
            // One provider handle shared by every session the handler opens, so
            // a scripted call sequence survives `/new` and a load.
            let shared = Arc::new(Mutex::new(provider));
            let deps: SessionDeps =
                Arc::new(move || (Box::new(SharedProvider(Arc::clone(&shared))), tools()));
            TuiSession::new(
                self.home(),
                self.cwd(),
                ProviderConfig::new("test-key", "https://example.invalid/v1", "test-model"),
                &self.store,
                &self.history,
                &self.skills,
                Some(deps),
            )
            .expect("the session opens")
        }

        /// A handler whose current session resumes `id`: the CLI's own load
        /// path (the SDK replays the log through `with_session_id`), which is
        /// also the public-API way to seed a live history.
        fn resuming(&self, provider: FakeProvider, tools: Tools, id: &str) -> TuiSession<'_> {
            let mut handler = self.handler(provider, tools);
            let (flow, items) = drive(&mut handler, Effect::LoadSession { id: id.to_string() });
            assert_eq!(flow, ControlFlow::Continue);
            assert_eq!(handler.session_id(), id, "the load failed: {items:?}");
            handler
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
        saved_messages(
            store,
            vec![
                AgentMessage::text(Role::User, prompt),
                AgentMessage::text(Role::Assistant, reply),
            ],
            Some(prompt),
        )
    }

    /// Write a session log holding exactly `messages` (the last one creates the
    /// file, which persists the whole backlog) and return its id.
    fn saved_messages(
        store: &SessionStore,
        messages: Vec<AgentMessage>,
        title: Option<&str>,
    ) -> String {
        let mut session = store.new_session();
        session.title = title.map(str::to_string);
        session.messages = messages;
        let last = session
            .messages
            .last()
            .expect("a message to persist")
            .clone();
        store.append(&session, &last).expect("log created");
        session.id
    }

    fn user_message(text: &str) -> Prompt {
        Prompt {
            text: text.to_string(),
            record: true,
        }
    }

    /// The current session's messages.
    fn messages(handler: &TuiSession<'_>) -> Vec<AgentMessage> {
        handler.session().messages()
    }

    /// A string that alone pushes the estimated history past the 92% threshold.
    fn over_threshold_text() -> String {
        "y".repeat(slimcode_app::compaction::estimated_context_window() * 4)
    }

    #[test]
    fn help_lists_every_command() {
        let fx = Fixture::new("help");
        let mut handler = fx.handler(FakeProvider::new(Vec::new()), no_tools());
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
        let mut handler = fx.handler(FakeProvider::new(Vec::new()), no_tools());
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
        let mut handler = fx.handler(FakeProvider::new(Vec::new()), no_tools());
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
        let mut handler = fx.handler(FakeProvider::new(Vec::new()), no_tools());
        let items = command(&mut handler, "/gril");
        let lines = lines(&items);
        assert_eq!(lines[0], "unknown command: /gril");
        assert!(lines[1].contains("/skill:grill"), "{lines:?}");
    }

    #[test]
    fn skill_trigger_submits_the_skill_block_without_recording() {
        let fx = Fixture::new("skill-trigger");
        fx.write_user_skill("grill", "stress-test a plan");
        let mut handler = fx.handler(FakeProvider::new(Vec::new()), no_tools());
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
        // The skill active block is shown as its own display unit before the
        // turn starts (like the replay path shows the boxed prompt). The
        // display block carries only the inner content, not the XML wrapper.
        assert_eq!(
            items,
            vec![RenderItem::Skill {
                name: "grill".to_string(),
                content: slimcode_app::skills::parse_skill_block(&prompt.text)
                    .unwrap()
                    .1
                    .to_string(),
            }]
        );
        assert!(
            items
                .iter()
                .all(|i| !format!("{i:?}").contains("<skill name="))
        );
    }

    #[test]
    fn replay_emits_the_prompt_box_and_never_records() {
        let fx = Fixture::new("replay");
        fx.history.append("first").unwrap();
        fx.history.append("second").unwrap();
        let mut handler = fx.handler(FakeProvider::new(Vec::new()), no_tools());

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
        let mut handler = fx.handler(FakeProvider::new(Vec::new()), no_tools());
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
        let mut handler = fx.handler(FakeProvider::new(Vec::new()), no_tools());
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
        let mut handler = fx.handler(FakeProvider::new(Vec::new()), no_tools());

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
        let id = saved_messages(
            &fx.store,
            vec![AgentMessage::text(Role::Assistant, "only")],
            None,
        );
        let mut handler = fx.handler(FakeProvider::new(Vec::new()), no_tools());

        let items = command(&mut handler, "/session");
        let [RenderItem::SessionPicker { rows }] = &items[..] else {
            panic!("expected one picker item: {items:?}");
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, id);
        assert_eq!(rows[0].title, id, "no title record -> the id");
        assert!(rows[0].meta.starts_with("1 msg  "), "{}", rows[0].meta);
    }

    #[test]
    fn session_reports_a_scan_failure_without_opening_the_picker() {
        let fx = Fixture::new("picker-error");
        // A file where the project directory should be makes the scan fail.
        let sessions = fx.home().join("sessions");
        fs::create_dir_all(&sessions).expect("sessions dir");
        fs::write(sessions.join(project_key(&fx.cwd())), b"not a directory")
            .expect("blocking file");
        let mut handler = fx.handler(FakeProvider::new(Vec::new()), no_tools());
        let items = command(&mut handler, "/session");
        assert_eq!(items.len(), 1);
        assert!(matches!(items[0], RenderItem::Error(_)), "{items:?}");
    }

    #[test]
    fn load_session_emits_the_change_before_the_notices() {
        let fx = Fixture::new("load-effect");
        let id = saved_session(&fx.store, "the prompt", "the reply");
        let mut handler = fx.handler(FakeProvider::new(Vec::new()), no_tools());

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
        assert_eq!(messages(&handler).len(), 2);
        assert_eq!(handler.session().title().as_deref(), Some("the prompt"));
    }

    #[test]
    fn load_session_replays_the_history_into_the_transcript() {
        let fx = Fixture::new("load-replay");
        // A session with a tool round trip, so the replay covers prompt boxes,
        // assistant text, a tool start/result pair and the final answer.
        let history = vec![
            AgentMessage::text(Role::User, "list it"),
            AgentMessage::llm(Message {
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
            }),
            AgentMessage::tool_result("call_1", "marker.txt"),
            AgentMessage::text(Role::Assistant, "done listing"),
        ];
        let id = saved_messages(&fx.store, history, Some("replay"));

        let mut handler = fx.handler(FakeProvider::new(Vec::new()), no_tools());
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
        assert_eq!(messages(&handler).len(), 4);
    }

    #[test]
    fn loading_the_current_session_is_a_no_op() {
        let fx = Fixture::new("load-current");
        let provider = FakeProvider::new(vec![vec![
            Delta::Text("hi".to_string()),
            Delta::Done(FinishReason::Stop),
        ]]);
        let mut handler = fx.handler(provider, no_tools());
        handler.submit(user_message("one"), &mut |_| {}).unwrap();
        let id = handler.session_id();
        let before = messages(&handler);

        // Memory is where the turn's history lives; reloading would drop it.
        let (flow, items) = drive(&mut handler, Effect::LoadSession { id });
        assert_eq!(flow, ControlFlow::Continue);
        assert!(items.is_empty(), "{items:?}");
        assert_eq!(messages(&handler), before);
    }

    #[test]
    fn loading_a_missing_session_reports_the_error() {
        let fx = Fixture::new("load-missing");
        let mut handler = fx.handler(FakeProvider::new(Vec::new()), no_tools());
        let (_, items) = drive(
            &mut handler,
            Effect::LoadSession {
                id: "slimcode-does-not-exist".to_string(),
            },
        );
        let [RenderItem::Error(text)] = &items[..] else {
            panic!("expected one error line: {items:?}");
        };
        assert!(text.contains("cannot resume session"), "{text}");
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
        let provider = FakeProvider::new(vec![vec![
            Delta::Text("hi".to_string()),
            Delta::Done(FinishReason::Stop),
        ]])
        .with_per_call(usage);
        let mut handler = fx.handler(provider, no_tools());
        handler.submit(user_message("one"), &mut |_| {}).unwrap();
        // The session, not the provider, is the display's source (ADR-0018 D4).
        assert_eq!(handler.session().usage(), usage);
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
        let mut handler = fx.handler(provider, probe_tools());
        let id = handler.session_id();

        let mut items = Vec::new();
        handler
            .submit(user_message("go"), &mut |item| items.push(item))
            .expect("turn succeeds");

        // Two requests × (10 prompt + 5 completion), counted once each.
        assert_eq!(
            handler.session().usage(),
            TokenUsage {
                prompt_tokens: 20,
                completion_tokens: 10,
                total_tokens: 30,
                ..Default::default()
            }
        );
        // The footer updates per assistant message (ticket 03): one Usage
        // item per assistant reply, each carrying the session usage
        // accumulated so far (ADR-0018 D4 — the footer reads session.usage)
        // plus a context estimate (ticket 04). The tool result in between
        // carries none, so it emits no Usage item.
        let usages: Vec<(FooterUsage, bool)> = items
            .iter()
            .filter_map(|item| match item {
                RenderItem::Usage { usage, context } => Some((*usage, context.is_some())),
                _ => None,
            })
            .collect();
        assert_eq!(
            usages,
            vec![
                (FooterUsage::new(10, 5, 0, 0), true),
                (FooterUsage::new(20, 10, 0, 0), true),
            ]
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
        let handler = fx.handler(provider, no_tools());
        handler.submit(user_message("one"), &mut |_| {}).unwrap();
        handler.submit(user_message("two"), &mut |_| {}).unwrap();

        // Each turn adds its own request only; the total is not the running
        // counter read twice.
        assert_eq!(
            handler.session().usage().total_tokens,
            30,
            "two turns × one request each"
        );
    }

    #[test]
    fn a_cancelled_request_contributes_no_usage() {
        // Esc lands mid-request (ticket 02/03): the request earned no tokens,
        // so the session's usage stays zero and no Usage item is emitted for
        // it — the half message never enters history.
        let fx = Fixture::new("cancel-usage");
        let provider = FakeProvider::new(vec![vec![
            Delta::Text("half".to_string()),
            Delta::Done(FinishReason::Stop),
        ]])
        .with_per_call(TokenUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
            ..Default::default()
        })
        .cancels_on(1);
        let handler = fx.handler(provider, no_tools());
        let mut items = Vec::new();
        handler
            .submit(user_message("go"), &mut |item| items.push(item))
            .expect("a cancelled turn is not an error");
        assert_eq!(handler.session().usage(), TokenUsage::default());
        assert!(
            !items.iter().any(|i| matches!(i, RenderItem::Usage { .. })),
            "no usage for a cancelled request: {items:?}"
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
        let mut handler = fx.handler(provider, no_tools());
        handler.submit(user_message("one"), &mut |_| {}).unwrap();
        assert_eq!(handler.session().usage().total_tokens, 15);

        command(&mut handler, "/new");
        assert_eq!(handler.session().usage(), TokenUsage::default());
        assert_eq!(
            lines(&command(&mut handler, "/usage")),
            ["tokens: 0 prompt (0 cached, 0%) + 0 completion = 0 total"]
        );
    }

    #[test]
    fn new_session_re_establishes_the_footer_context() {
        // Ticket 05: the app hides the context segment on SessionChanged, so
        // the CLI must emit a follow-up Usage item right after it — here the
        // fresh session's `0%/200k`.
        let fx = Fixture::new("ctx-new");
        let mut handler = fx.handler(FakeProvider::new(Vec::new()), no_tools());
        let items = command(&mut handler, "/new");
        let changed = items
            .iter()
            .position(|i| matches!(i, RenderItem::SessionChanged { .. }))
            .expect("SessionChanged");
        let usage_at = items
            .iter()
            .position(|i| matches!(i, RenderItem::Usage { .. }))
            .expect("follow-up Usage");
        assert!(
            changed < usage_at,
            "context re-established after the change: {items:?}"
        );
        assert_eq!(
            items[usage_at],
            RenderItem::Usage {
                usage: FooterUsage::default(),
                context: Some(ContextUsage::new(0.0, 200_000)),
            }
        );
    }

    #[test]
    fn load_session_re_establishes_the_footer_context_from_its_history() {
        // Ticket 05: a loaded session starts at zero usage, and its context
        // segment is the estimate over the restored history — the same
        // estimator the compaction trigger uses.
        let fx = Fixture::new("ctx-load");
        let saved = saved_session(&fx.store, "old prompt", "old reply");
        let expected =
            crate::render::to_context_usage(&fx.store.load(&saved).unwrap().session.messages);
        let mut handler = fx.handler(FakeProvider::new(Vec::new()), no_tools());
        let (_, items) = drive(&mut handler, Effect::LoadSession { id: saved });
        let changed = items
            .iter()
            .position(|i| matches!(i, RenderItem::SessionChanged { .. }))
            .expect("SessionChanged");
        let usage_at = items
            .iter()
            .position(|i| matches!(i, RenderItem::Usage { .. }))
            .expect("follow-up Usage");
        assert!(
            changed < usage_at,
            "context re-established after the change: {items:?}"
        );
        match &items[usage_at] {
            RenderItem::Usage { usage, context } => {
                assert_eq!(*usage, FooterUsage::default());
                assert_eq!(*context, Some(expected));
            }
            other => panic!("expected Usage, got {other:?}"),
        }
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
        let mut handler = fx.handler(provider, no_tools());
        handler.submit(user_message("one"), &mut |_| {}).unwrap();
        assert_eq!(handler.session().usage().total_tokens, 15);

        // The previous session's numbers must not follow the load.
        drive(&mut handler, Effect::LoadSession { id: saved });
        assert_eq!(handler.session().usage(), TokenUsage::default());
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
        let mut handler = fx.handler(FakeProvider::new(Vec::new()), no_tools());
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
        let handler = fx.handler(provider, no_tools());
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
        let handler = fx.handler(provider, no_tools());
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
            cancel_on_call: None,
            gate: None,
        };
        let handler = fx.handler(provider, probe_tools());
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
            cancel_on_call: None,
            gate: None,
        };
        let handler = fx.handler(provider, no_tools());
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
        let handler = fx.handler(FakeProvider::failing_at_zero(), no_tools());
        let id = handler.session_id();
        handler
            .submit(user_message("only"), &mut |_| {})
            .expect_err("the provider fails");
        // Nothing worth persisting: the log must not exist (ADR-0009 D5).
        let path = fx.store.session_path(&id).expect("path");
        assert!(!path.exists(), "unexpected log at {}", path.display());
    }

    #[test]
    fn cancel_aborts_the_running_turn_and_the_next_turn_runs() {
        // Esc arrives from the frame thread while the worker is inside a
        // request: `cancel` must reach the session's token without waiting for
        // the worker, and the following turn must start uncancelled.
        let fx = Fixture::new("cancel");
        let gate = Gate::new();
        let provider = FakeProvider::new(vec![
            vec![
                Delta::Text("first".to_string()),
                Delta::Done(FinishReason::Stop),
            ],
            vec![
                Delta::Text("second".to_string()),
                Delta::Done(FinishReason::Stop),
            ],
        ])
        .blocking(gate.clone());
        let handler = fx.handler(provider, no_tools());

        std::thread::scope(|scope| {
            scope.spawn(|| {
                handler
                    .submit(user_message("go"), &mut |_| {})
                    .expect("a cancelled turn is not an error");
            });
            gate.wait_entered();
            handler.cancel();
            gate.open();
        });

        let report = handler
            .submit(user_message("again"), &mut |_| {})
            .expect("turn succeeds");
        assert_eq!(report.appended, 1);
    }

    #[test]
    fn a_second_turn_while_one_runs_is_rejected() {
        let fx = Fixture::new("busy");
        let gate = Gate::new();
        let provider = FakeProvider::new(vec![vec![
            Delta::Text("done".to_string()),
            Delta::Done(FinishReason::Stop),
        ]])
        .blocking(gate.clone());
        let handler = fx.handler(provider, no_tools());

        std::thread::scope(|scope| {
            scope.spawn(|| {
                handler
                    .submit(user_message("go"), &mut |_| {})
                    .expect("turn succeeds");
            });
            gate.wait_entered();
            // The SDK refuses a second turn while one is in flight.
            assert_eq!(
                handler.submit(user_message("go"), &mut |_| {}).unwrap_err(),
                "a turn is already running"
            );
            gate.open();
        });
    }

    #[test]
    fn a_completed_turn_over_the_threshold_auto_compacts() {
        let fx = Fixture::new("auto-compact");
        let provider = FakeProvider::new(vec![
            vec![
                Delta::Text(over_threshold_text()),
                Delta::Done(FinishReason::Stop),
            ],
            vec![
                Delta::Text("## Goal\ncompacted".to_string()),
                Delta::Done(FinishReason::Stop),
            ],
        ]);
        let handler = fx.handler(provider, no_tools());
        let id = handler.session_id();
        let mut items = Vec::new();
        handler
            .submit(user_message("long task"), &mut |i| items.push(i))
            .expect("turn succeeds");

        let session_messages = messages(&handler);
        assert_eq!(session_messages.len(), 1, "{session_messages:?}");
        assert!(session_messages[0].is_compact_summary());
        assert_eq!(session_messages[0].text_content(), "## Goal\ncompacted");
        assert!(
            items
                .iter()
                .any(|i| matches!(i, RenderItem::Notice(t) if t.contains("context compacted"))),
            "{items:?}"
        );

        // The checkpoint is persisted, and a reload drops the replaced span:
        // the resumed Session matches the model's view, not the raw log.
        let loaded = fx.store.load(&id).expect("log loads").session;
        assert_eq!(loaded.messages.len(), 1, "{:?}", loaded.messages);
        assert!(loaded.messages[0].is_compact_summary());
        // The assembled context injects the summary and never the old prompt.
        let context = ContextBuilder::new()
            .with_history(loaded.messages.clone())
            .with_user_prompt("next")
            .build()
            .unwrap();
        assert!(
            context.messages[0]
                .text_content()
                .contains("<summary>\n## Goal\ncompacted\n</summary>")
        );
        assert!(
            context
                .messages
                .iter()
                .all(|m| m.text_content() != "long task"),
            "the compacted-away prompt must not reach the request"
        );
    }

    /// The compaction's footer refresh reads the restored session, not the
    /// pre-turn snapshot the turn state was taken from: the `Usage` item
    /// emitted with the compaction notice carries the turn's request plus the
    /// summary request (two calls), and its context segment is the
    /// post-compaction estimate of the live history — never the stale zero
    /// snapshot that used to render (a first turn's empty history).
    #[test]
    fn turn_end_auto_compaction_renders_the_post_compaction_footer() {
        let fx = Fixture::new("auto-compact-footer");
        let per_call = TokenUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
            ..Default::default()
        };
        let provider = FakeProvider::new(vec![
            vec![
                Delta::Text(over_threshold_text()),
                Delta::Done(FinishReason::Stop),
            ],
            vec![
                Delta::Text("## Goal\ncompacted".to_string()),
                Delta::Done(FinishReason::Stop),
            ],
        ])
        .with_per_call(per_call);
        let handler = fx.handler(provider, no_tools());
        let mut items = Vec::new();
        handler
            .submit(user_message("long task"), &mut |i| items.push(i))
            .expect("turn succeeds");

        // `note_compacted` emits the footer refresh immediately before the
        // notice, so the refresh that belongs to the compaction is the last
        // `Usage` item preceding it.
        let notice_at = items
            .iter()
            .position(|i| matches!(i, RenderItem::Notice(t) if t.contains("context compacted")))
            .expect("a compaction notice");
        let (footer, context) = items[..notice_at]
            .iter()
            .rev()
            .find_map(|item| match item {
                RenderItem::Usage { usage, context } => Some((*usage, *context)),
                _ => None,
            })
            .expect("the compaction's usage refresh");
        assert_eq!(
            footer.input + footer.output,
            per_call.total_tokens * 2,
            "the compaction cost must be included"
        );
        let context = context.expect("a context estimate");
        let expected = slimcode_app::compaction::estimate_total_tokens(&messages(&handler)) as f64;
        assert!(context.percent > 0.0, "a zero snapshot: {context:?}");
        assert_eq!(
            context.percent,
            expected / context.window as f64 * 100.0,
            "the context segment must match the post-compaction history"
        );
    }

    #[test]
    fn a_small_completed_turn_does_not_compact() {
        let fx = Fixture::new("no-auto-compact");
        let provider = FakeProvider::new(vec![vec![
            Delta::Text("short answer".to_string()),
            Delta::Done(FinishReason::Stop),
        ]]);
        let handler = fx.handler(provider, no_tools());
        let mut items = Vec::new();
        handler
            .submit(user_message("small task"), &mut |i| items.push(i))
            .expect("turn succeeds");
        let session_messages = messages(&handler);
        assert_eq!(session_messages.len(), 2);
        assert!(session_messages.iter().all(|m| !m.is_compact_summary()));
        assert!(
            !items
                .iter()
                .any(|i| matches!(i, RenderItem::Notice(t) if t.contains("compacting"))),
            "{items:?}"
        );
    }

    /// A saved log whose history is over the compaction threshold: the CLI's
    /// load path brings it into a live session without running a turn.
    fn long_history(fx: &Fixture, title: &str) -> String {
        saved_messages(
            &fx.store,
            vec![
                AgentMessage::text(Role::User, "write a lot"),
                AgentMessage::text(Role::Assistant, over_threshold_text()),
            ],
            Some(title),
        )
    }

    #[test]
    fn compact_command_summarizes_on_demand() {
        let fx = Fixture::new("compact-cmd");
        let long = long_history(&fx, "long");
        let provider = FakeProvider::new(vec![vec![
            Delta::Text("## Goal\ndone".to_string()),
            Delta::Done(FinishReason::Stop),
        ]]);
        let mut handler = fx.resuming(provider, no_tools(), &long);

        let items = command(&mut handler, "/compact");
        // The usage footer refresh rides the same batch.
        assert!(
            items.iter().any(|i| matches!(i, RenderItem::Usage { .. })),
            "{items:?}"
        );
        let lines: Vec<&str> = items
            .iter()
            .filter_map(|item| match item {
                RenderItem::Notice(t) | RenderItem::Error(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert!(lines[0].contains("compressing context"), "{lines:?}");
        assert!(
            lines.iter().any(|l| l.contains("context compacted:")),
            "{lines:?}"
        );
        let session_messages = messages(&handler);
        assert_eq!(session_messages.len(), 1, "{session_messages:?}");
        assert!(session_messages[0].is_compact_summary());
    }

    #[test]
    fn compact_command_on_a_short_history_reports_nothing_to_do() {
        let fx = Fixture::new("compact-short");
        let short = saved_session(&fx.store, "hi", "hello");
        let mut handler = fx.resuming(FakeProvider::new(Vec::new()), no_tools(), &short);

        let items = command(&mut handler, "/compact");
        let lines = lines(&items);
        assert!(
            lines
                .last()
                .is_some_and(|l| l.contains("nothing to compact")),
            "{lines:?}"
        );
    }

    #[test]
    fn a_failed_compaction_keeps_the_history() {
        let fx = Fixture::new("compact-fail");
        let long = long_history(&fx, "long");
        let mut handler = fx.resuming(FakeProvider::failing_at_zero(), no_tools(), &long);

        let items = command(&mut handler, "/compact");
        let lines = lines(&items);
        assert!(
            lines.iter().any(|l| l.starts_with("compaction failed:")),
            "{lines:?}"
        );
        // The turn's history is untouched.
        let session_messages = messages(&handler);
        assert_eq!(session_messages.len(), 2);
        assert!(session_messages.iter().all(|m| !m.is_compact_summary()));
    }
}
