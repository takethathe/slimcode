//! slimcode-sdk: programmatic, session-centric access to slimcode's agent
//! loop (ADR-0021, spec `.scratch/sdk/spec.md`).
//!
//! One call [`create_agent_session`] builds a persistent [`AgentSession`];
//! [`AgentSession::prompt`] runs a whole agent turn on the calling thread,
//! driving the code preset through the app layer's public Engine seam
//! ([`slimcode_app::engine::Engine`]) — no second turn loop.
//! Events are delivered to listeners in subscription order over the session's
//! internal bounded message-bus ([`AgentSession::subscribe`]: streamed
//! `Display` items, per-message `Message` events, and a final `TurnFinished`),
//! drained on a dispatcher thread so a slow subscriber never stalls the turn;
//! `prompt` / `compact_now` flush the bus before returning, so a host sees its
//! whole ordered event stream by the time the call returns. The session
//! persists as the existing JSONL session log
//! (project-partitioned, `/session`-picker compatible), replayable through
//! [`slimcode_app::session::SessionStore`].
//!
//! The crate depends only on `slimcode-app` (→ `ai` / `core` / `commands`),
//! never on a terminal crate, so non-terminal hosts can embed it
//! (ADR-0011 D4, ADR-0021 D1).
//!
//! Synchronous by design (spec D4): a host that needs non-blocking behavior
//! runs `prompt` on its own thread and cancels the turn with
//! [`AgentSession::abort`], which touches only the shared cancel token and
//! never waits on the session lock. [`AgentSession::dispose`] rejects every
//! later prompt, cancels an in-flight turn, and silences subscribers.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex};

mod bus;
mod facade;

use slimcode_app::config::{self, Overrides};
use slimcode_app::context::{ContextBuilder, Environment};
use slimcode_app::context_files::{load_context_files, resolve_project_home};
use slimcode_app::engine;
use slimcode_app::preset;
use slimcode_app::render::Renderer;
use slimcode_app::session::{SessionStore, infer_title, new_session_id, now_rfc3339, project_key};
use slimcode_app::skills::SkillStore;
use slimcode_app::tools;
use slimcode_app::{
    BailianProvider, CancelToken, MessageStopReason, ProviderConfig, Role, RunConfig, Session,
};

/// Re-exports of the `slimcode-app` types this crate's public API names
/// ([`Provider`] / [`Tool`] / [`Skill`] on [`AgentSessionOptions`],
/// [`AgentMessage`] / [`StopReason`] / [`TokenUsage`] / [`DisplayItem`] on the
/// event and accessor surface), so a host can depend on `slimcode-sdk` alone.
pub use slimcode_app::render::DisplayItem;
pub use slimcode_app::skills::Skill;
pub use slimcode_app::{AgentMessage, Provider, StopReason, TokenUsage, Tool};

/// The runtime-agnostic async facade over the synchronous kernel (ADR-0031 D3,
/// engine ticket 07): a [`PromptFuture`] for a turn driven on a dedicated OS
/// thread and an [`EventStream`] of the session's read-only events, reached
/// through [`AgentSession::prompt_async`] / [`AgentSession::event_stream`].
pub use facade::{EventStream, PromptFuture};

/// One event delivered to a subscriber during a turn, in subscription order.
///
/// Event order for one `prompt`: streamed `Display` items (text / reasoning /
/// tool start / tool result / per-message usage / stop) interleave with the
/// `Message` events for each message that enters history, and the last event
/// is always `TurnFinished { stop_reason }` for a `Completed` / `Cancelled`
/// turn. When turn-end auto-compaction replaced part of the history, one
/// `Compacted { before, after }` event follows that turn's `TurnFinished`
/// (spec §5 event order). Both turn-end events are emitted after the turn's
/// provider / tools / session state is put back, so a listener that reads the
/// session inside them (`messages()` / `usage()`) sees the turn's own result —
/// never the pre-turn snapshot the state was taken from.
#[derive(Clone, Debug)]
pub enum SdkEvent {
    /// A streamed display unit (text fragment, thinking line, tool start /
    /// result, usage, stop marker).
    Display(DisplayItem),
    /// A message entered history: the user prompt and tool results carry
    /// `usage: None`; assistant replies carry the request's usage.
    Message {
        message: AgentMessage,
        usage: Option<TokenUsage>,
    },
    /// The authoritative end signal of one `prompt` — always the last event
    /// of a `Completed` / `Cancelled` turn.
    TurnFinished { stop_reason: StopReason },
    /// Turn-end auto-compaction replaced part of the history, or an explicit
    /// [`AgentSession::compact_now`] call did: `before` / `after` are the
    /// estimated history tokens before / after the replacement (ticket 06).
    /// Emitted after the `TurnFinished` of the turn that crossed the threshold,
    /// or synchronously by `compact_now`; never emitted when compaction fails.
    Compacted { before: usize, after: usize },
}

/// Stop receiving events from a session. Dropping the returned value without
/// calling it keeps the listener subscribed.
pub type Unsubscribe = Box<dyn FnOnce() + Send + 'static>;

/// One subscribed listener, tagged with its subscription id (so an
/// [`Unsubscribe`] can remove exactly it). Stored behind an `Arc` so `emit`
/// can snapshot the handles under the lock and invoke each listener outside
/// it (a listener may call its own `Unsubscribe` from inside a callback —
/// the std mutex is not reentrant). `+ Send + Sync` keeps
/// `Arc<dyn Fn …>` (and therefore [`AgentSession`]) `Send + Sync`.
type Listener = (usize, Arc<dyn Fn(SdkEvent) + Send + Sync>);

/// Options for [`create_agent_session`]. Explicit fields override the four
/// layer config resolution (overrides > env > `config.toml` > defaults);
/// `None` fields fall through. Built with [`AgentSessionOptions::new`] plus
/// the `with_*` setters.
pub struct AgentSessionOptions {
    /// Working directory the tools and project resolution bind to. Defaults
    /// to the process's current directory.
    pub cwd: Option<PathBuf>,
    /// Persist the session as a JSONL session log. Defaults to `true`.
    pub persist: bool,
    /// Resume an existing session by its id: on creation, the session is
    /// replayed leniently from the session store (messages / title restored,
    /// usage from zero). Requires `persist: true` — loading needs the store.
    pub session_id: Option<String>,
    /// Override the provider base URL.
    pub base_url: Option<String>,
    /// Override the provider model.
    pub model: Option<String>,
    /// Override the provider API key.
    pub api_key: Option<String>,
    /// Override the explicit-cache flag.
    pub cache: Option<bool>,
    /// Pin the slimcode home directory (session store root, skills and
    /// context-file discovery, `Environment.global_home`). Defaults to
    /// `config::slimcode_home()`.
    pub home: Option<PathBuf>,
    /// The provider to drive; defaults to a `BailianProvider`.
    pub provider: Option<Box<dyn Provider + Send>>,
    /// The tools the turn may invoke; defaults to the seven tools bound to
    /// `cwd`, cancellable through the session's own token (an `abort` kills
    /// a running bash child).
    pub tools: Option<Vec<Tool>>,
    /// The skills advertised to the model; `None` discovers from the user
    /// and project scopes.
    pub skills: Option<Vec<Skill>>,
    /// The turn-end component run once after a completed turn (auto-compaction)
    /// and by `compact_now`. `None` uses the code preset's default
    /// compaction (`slimcode_app::preset::compaction_turn_end`); a host swaps in
    /// its own consolidation, or disables the step with
    /// `slimcode_app::preset::noop_turn_end`.
    pub turn_end: Option<Arc<preset::TurnEndComponent>>,
}

impl AgentSessionOptions {
    /// Start with a working directory (or `None` for the process's current
    /// directory); every other option defaults to the CLI startup behavior.
    pub fn new(cwd: Option<PathBuf>) -> Self {
        Self {
            cwd,
            persist: true,
            session_id: None,
            base_url: None,
            model: None,
            api_key: None,
            cache: None,
            home: None,
            provider: None,
            tools: None,
            skills: None,
            turn_end: None,
        }
    }

    /// Choose persistence (defaults to `true`).
    pub fn with_persist(mut self, persist: bool) -> Self {
        self.persist = persist;
        self
    }

    /// Resume the session with the given id: [`create_agent_session`]
    /// replays the id's persisted log into the new session (messages / title
    /// restored, usage from zero) instead of starting fresh, and the session
    /// keeps appending to the same log. Loading requires the session store,
    /// so this forces `persist: true` (`with_persist(false)` errors).
    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Override the provider base URL.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }

    /// Override the provider model.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Override the provider API key.
    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// Override the explicit-cache flag.
    pub fn with_cache(mut self, cache: bool) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Pin the slimcode home directory. Without it, the process-level
    /// `SLIMCODE_HOME` (via `config::slimcode_home()`) is used.
    pub fn with_home(mut self, home: impl Into<PathBuf>) -> Self {
        self.home = Some(home.into());
        self
    }

    /// Inject the provider (the test / FFI-host seam).
    pub fn with_provider(mut self, provider: Box<dyn Provider + Send>) -> Self {
        self.provider = Some(provider);
        self
    }

    /// Inject the tool set (defaults to the seven tools bound to `cwd`).
    pub fn with_tools(mut self, tools: Vec<Tool>) -> Self {
        self.tools = Some(tools);
        self
    }

    /// Inject the skill list (defaults to discovery from the user + project
    /// scopes).
    pub fn with_skills(mut self, skills: Vec<Skill>) -> Self {
        self.skills = Some(skills);
        self
    }

    /// Replace or disable the turn-end component. Passing
    /// `slimcode_app::preset::noop_turn_end` (as an `Arc<dyn
    /// preset::TurnEndComponent>`) turns off auto-compaction and `compact_now`
    /// alike; a custom component runs in their place. `None` (the default) keeps
    /// the code preset's compaction.
    pub fn with_turn_end(mut self, turn_end: Arc<preset::TurnEndComponent>) -> Self {
        self.turn_end = Some(turn_end);
        self
    }
}

/// The per-session state behind the mutex, mirroring the TUI's take/put
/// pattern (ADR-0013 D2): `prompt` takes the provider/tools/session out for
/// the whole turn, so a concurrent `prompt` errors ("single-flight") and a
/// cross-thread `abort` never waits on this lock (the cancel token lives on
/// [`AgentSession`], outside the mutex).
struct Inner {
    session: Session,
    provider: Option<Box<dyn Provider + Send>>,
    tools: Vec<Tool>,
    disposed: bool,
}

/// A programmatic slimcode session: `prompt` one turn, `compact_now` the
/// history on demand, subscribe to events, and read the session's messages /
/// id / title / usage.
///
/// `Send + Sync`: the internals are `Arc`-shared; `prompt` and `compact_now`
/// are synchronous and share a single-flight seam (a second concurrent call
/// of either errors), events are delivered
/// synchronously on the calling thread in subscription order, and a
/// panicking listener is isolated (it is skipped; the turn and the other
/// listeners continue). A listener may call its own [`Unsubscribe`] from
/// inside a callback. `abort` / `dispose` may be called from any thread,
/// including while a turn is running.
pub struct AgentSession {
    /// Immutable session id (the shared `slimcode-<unix>-<pid>-<n>` scheme).
    id: String,
    inner: Arc<Mutex<Inner>>,
    /// The shared cancel token, deliberately outside [`Inner`] so `abort` /
    /// `dispose` cancel the in-flight turn without waiting on the session
    /// lock. `prompt` resets it at turn start (take) and hands clones into
    /// the runner / provider.
    cancel: CancelToken,
    listeners: Arc<Mutex<Vec<Listener>>>,
    /// Publisher handle onto the session's bounded event bus (ticket 06): every
    /// [`Self::emit`] sends here and a dedicated dispatcher thread delivers to
    /// [`Self::listeners`]. A `SyncSender` blocks on a full buffer, which is the
    /// backpressure a sustained-slow subscriber applies; a burst is absorbed by
    /// the capacity. `Send + Sync`, so the session stays both.
    bus_tx: SyncSender<bus::BusMsg>,
    next_listener_id: AtomicUsize,
    // Immutable per-session resources (never change; SessionStore methods
    // take &self), kept outside the mutex.
    config: ProviderConfig,
    environment: Environment,
    context_files: Vec<slimcode_app::context_files::ContextFile>,
    skills: Vec<Skill>,
    store: Option<SessionStore>,
    cwd: PathBuf,
    /// The turn-end component (auto-compaction / `compact_now`), shared so it
    /// stays `Send + Sync`; the code preset's default compaction unless
    /// [`AgentSessionOptions::with_turn_end`] swapped it out.
    turn_end: Arc<preset::TurnEndComponent>,
}

/// Create an [`AgentSession`], assembling the services exactly like the CLI
/// startup path: four-layer config resolution, the slimcode home directory
/// (pinnable via [`AgentSessionOptions::with_home`]), the project home /
/// project key, and the project-scoped session store.
///
/// With [`AgentSessionOptions::with_session_id`], the session is instead
/// resumed from the store by id: the id's persisted log is replayed leniently
/// into the new session (messages / title restored, usage from zero) and the
/// session keeps appending to the same log. Resuming requires the store, so
/// `persist: false` errors.
///
/// Provider and tools are injectable (the test / FFI-host seam); the defaults
/// are a `BailianProvider` and the seven tools bound to `cwd` — cancellable
/// through the session's own token, so `abort` / `dispose` also terminate a
/// running bash child. AGENTS.md
/// context files are discovered from `cwd` (and the slimcode home for the
/// global file) and skills from the user + project scopes — both exactly like
/// the CLI — with explicit `options.skills` bypassing discovery. The default
/// code-preset phase-hook bundle is registered on the run: its `pre_tool`
/// interception serves a model `read` of a skill's own `SKILL.md` as the skill
/// activation block (ADR-0031 D5).
pub fn create_agent_session(options: AgentSessionOptions) -> Result<AgentSession, String> {
    let cwd = options
        .cwd
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    // A resume loads from the session store, so `persist: false` (no store)
    // is rejected up front — validated before the store is built.
    if let Some(id) = &options.session_id
        && !options.persist
    {
        return Err(format!(
            "cannot resume session {id:?}: loading requires the session store, so persist must be true"
        ));
    }
    // Four-layer config resolution with the explicit options on top
    // (overrides > env > config.toml > defaults).
    let app = config::load_app_config(Overrides {
        base_url: options.base_url,
        model: options.model,
        cache: options.cache,
        api_key: options.api_key,
    })?;
    // `with_home` pins the slimcode home (session store root, skills and
    // context-file discovery, `Environment.global_home`); without it the
    // process-level `SLIMCODE_HOME` (or the default user home) is used,
    // exactly like the CLI startup path.
    let home = match options.home {
        Some(home) => home,
        None => {
            config::slimcode_home().ok_or_else(|| "cannot determine home directory".to_string())?
        }
    };
    let user_home = std::env::var_os("HOME").map(PathBuf::from);
    let project_home = resolve_project_home(&cwd, user_home.as_deref());
    // The session store is scoped to the current project; startup empty-log
    // cleanup is best-effort and never blocks creation.
    let store = if options.persist {
        let store = SessionStore::new(
            home.join("sessions"),
            project_key(&project_home),
            project_home.clone(),
        )
        .with_max_bytes(app.sessions_max_bytes);
        let _ = store.cleanup_empty();
        Some(store)
    } else {
        None
    };
    let provider = match options.provider {
        Some(provider) => provider,
        None => Box::new(BailianProvider::new()?),
    };
    // The default tool set is cancellable and shares the session's own cancel
    // token (spec D1, ticket 01): the token is created before the tools so a
    // `bash` child can observe an `abort` / `dispose`; it is reset at prompt
    // start and set on abort / dispose, exactly like the TUI. Injected tools
    // are used as-is (the host owns their cancellation).
    let session_cancel = CancelToken::new();
    let tools = options
        .tools
        .unwrap_or_else(|| tools::build_tools_with_cancel(&cwd, &session_cancel));
    // Resource assembly mirrors the CLI startup path (spec D2): AGENTS.md
    // context files are discovered from the slimcode home (global) and the
    // working directory (project), and skills are discovered from the user
    // (`<home>/skills`) and project (`<cwd>/.slimcode/skills`) scopes. Skill
    // discovery failure must not block session creation (mirroring the CLI's
    // `unwrap_or_default`); context-file discovery is infallible. Explicit
    // `options.skills` bypasses discovery.
    let context_files = load_context_files(&home, &cwd);
    let skills = options
        .skills
        .unwrap_or_else(|| SkillStore::new(&home, &cwd).list().unwrap_or_default());
    let environment = Environment {
        os: std::env::consts::OS.to_string(),
        global_home: home.clone(),
        project_home: project_home.clone(),
    };
    let session = match &options.session_id {
        // Lenient replay (ADR-0009 D3): `SessionStore::load` skips malformed
        // or unknown records and repairs torn tails, and a compaction
        // checkpoint boundary is honored; a missing log is a loud error. The
        // restored session carries its persisted messages and title; usage is
        // never persisted, so it starts at zero (ADR-0018 D4).
        Some(id) => {
            // The persist check above guarantees the store exists; the guard
            // keeps this total if that invariant ever changes.
            let store = store.as_ref().ok_or_else(|| {
                format!("cannot resume session {id:?}: the session store is unavailable (persist disabled)")
            })?;
            store
                .load(id)
                .map_err(|e| format!("cannot resume session {id:?}: {e}"))?
                .session
        }
        None => Session {
            id: new_session_id(),
            created_at: now_rfc3339(),
            messages: Vec::new(),
            title: None,
            usage: TokenUsage::default(),
        },
    };
    // The bounded event bus (ticket 06): a `SyncSender`/`Receiver` pair over
    // `BusMsg` plus a dispatcher thread that drains it and delivers to the
    // shared subscriber list. Publishing never runs a subscriber on the turn's
    // own thread, so a slow one is absorbed by the buffer (backpressure only on
    // sustained overflow); `prompt` / `compact_now` flush before returning so
    // the host still observes a fully-delivered, ordered stream.
    let listeners: Arc<Mutex<Vec<Listener>>> = Arc::new(Mutex::new(Vec::new()));
    let (bus_tx, bus_rx) = mpsc::sync_channel(bus::EVENT_BUS_CAPACITY);
    std::thread::spawn({
        let listeners = Arc::clone(&listeners);
        move || bus::run(bus_rx, listeners)
    });
    Ok(AgentSession {
        id: session.id.clone(),
        inner: Arc::new(Mutex::new(Inner {
            session,
            provider: Some(provider),
            tools,
            disposed: false,
        })),
        cancel: session_cancel,
        listeners,
        bus_tx,
        next_listener_id: AtomicUsize::new(0),
        config: app.provider,
        environment,
        context_files,
        skills,
        store,
        cwd,
        turn_end: options
            .turn_end
            .unwrap_or_else(|| Arc::new(preset::compaction_turn_end)),
    })
}

/// What one turn produced, so `prompt` can put the taken state back even when
/// the turn failed — and, once it did, emit the turn's closing events
/// (`TurnFinished`, then `Compacted`) against the restored session.
struct TurnOutcome {
    provider: Box<dyn Provider + Send>,
    tools: Vec<Tool>,
    session: Session,
    result: Result<StopReason, String>,
    /// The turn-end auto-compaction's token change, if one ran: emitted as
    /// [`SdkEvent::Compacted`] right after this turn's `TurnFinished`.
    /// Always `None` on a failed turn (only the `Completed` arm compacts).
    compacted: Option<(usize, usize)>,
}

impl AgentSession {
    /// Run one whole agent turn for `text`, returning the stop reason.
    ///
    /// Synchronous and single-flight: runs on the calling thread, drives the
    /// code preset through the public turn seam
    /// ([`slimcode_app::engine::Engine`]) and
    /// mirrors the CLI's turn orchestration (prompt recording, title
    /// inference, best-effort persistence, closing messages on failure /
    /// cancellation). `text` is plain user input — `/` commands are a
    /// frontend concern and are not interpreted. Empty / whitespace-only
    /// prompts are rejected. A concurrent `prompt` or [`Self::compact_now`]
    /// errors with `"a turn is already running"`.
    pub fn prompt(&self, text: &str) -> Result<StopReason, String> {
        if text.trim().is_empty() {
            return Err("prompt text must not be empty".to_string());
        }
        // Take the turn state out of the lock so the whole turn runs without
        // holding it (a concurrent `abort` must never wait here).
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "session lock poisoned".to_string())?;
        if inner.disposed {
            return Err("session is disposed".to_string());
        }
        let Some(provider) = inner.provider.take() else {
            return Err("a turn is already running".to_string());
        };
        let tools = std::mem::take(&mut inner.tools);
        let session = inner.session.clone();
        // A fresh turn starts uncancelled. Reset while still holding the lock
        // so a concurrent `dispose` (which needs the lock to set its flag)
        // cannot have its cancel wiped by this turn's start.
        self.cancel.reset();
        let cancel = self.cancel.clone();
        drop(inner);

        let outcome = self.run_turn(provider, tools, session, cancel, text);

        // Put everything back, whatever the outcome.
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "session lock poisoned".to_string())?;
        inner.provider = Some(outcome.provider);
        inner.tools = outcome.tools;
        inner.session = outcome.session;
        drop(inner);

        // The turn's closing events go out only once the state is back inside
        // the lock, so a listener that reads the session (`messages()` /
        // `usage()`) inside them sees the turn's own result — not the pre-turn
        // snapshot the state was taken from (the CLI's footer refresh on
        // `Compacted` depends on it). Order and the failure contract are
        // unchanged: `TurnFinished` only after a `Completed` / `Cancelled`
        // turn, and a turn-end `Compacted` right after it.
        if let Ok(stop_reason) = &outcome.result {
            self.emit(SdkEvent::TurnFinished {
                stop_reason: stop_reason.clone(),
            });
            if let Some((before, after)) = outcome.compacted {
                self.emit(SdkEvent::Compacted { before, after });
            }
        }
        // Turn-end barrier: deliver this turn's whole stream (every event above
        // and the ones published during the loop) before returning, so a
        // subscriber reads a fully-drained, ordered sequence and — inside the
        // terminal events — the put-back session state, exactly as the old
        // synchronous `emit` gave on the calling thread.
        self.flush_events();
        outcome.result
    }

    /// The whole turn orchestration, mirroring the CLI's `TuiSession::run_turn`
    /// (context assembly → prompt recording + title inference → shared turn
    /// loop with an SDK renderer and a persisting message sink → closing
    /// messages on failure / cancellation). It does not emit the closing
    /// `TurnFinished` / `Compacted` events itself: they are the caller's, sent
    /// after this turn's state is put back so listeners read the restored
    /// session (see [`Self::prompt`]).
    fn run_turn(
        &self,
        mut provider: Box<dyn Provider + Send>,
        tools: Vec<Tool>,
        mut session: Session,
        cancel: CancelToken,
        text: &str,
    ) -> TurnOutcome {
        let title_was_none = session.title.is_none();
        let context = match ContextBuilder::new()
            .with_environment(self.environment.clone())
            .with_context_files(&self.context_files)
            .with_skills(&self.skills)
            .with_history(session.messages.clone())
            .with_user_prompt(text)
            .build()
        {
            Ok(context) => context,
            Err(e) => {
                return TurnOutcome {
                    provider,
                    tools,
                    session,
                    result: Err(e),
                    compacted: None,
                };
            }
        };
        // A turn adds exactly one message to history — the user prompt. The
        // system prompt is assembled per turn and never stored (ADR-0012 D3).
        let prompt_msg = context
            .messages
            .last()
            .expect("context always has a prompt message")
            .clone();
        session.messages.push(prompt_msg.clone());
        // The prompt is the first history entry of the turn, so it emits the
        // first `Message` event (usage `None`, like any non-assistant entry)
        // before any streamed display (spec §5 event order, User Story 5).
        self.emit(SdkEvent::Message {
            message: prompt_msg.clone(),
            usage: None,
        });
        self.append_best_effort(&session, &prompt_msg);
        // Title inference reads the raw history, not the assembled context: a
        // compaction checkpoint is not a prompt (ADR-0009 D1).
        if title_was_none {
            session.title = infer_title(&session.messages);
        }
        if title_was_none
            && let Some(title) = session.title.as_ref()
            && let Some(store) = &self.store
            && let Ok(path) = store.session_path(&session.id)
            && path.exists()
        {
            let _ = store.append_title(&session, title);
        }

        let cfg = RunConfig::default();
        let mut appended = 0usize;
        // A turn that dies before producing anything must still close an
        // existing log on an assistant boundary; a turn with no log yet
        // leaves none (ADR-0009 D5).
        let log_exists = self
            .store
            .as_ref()
            .and_then(|s| s.session_path(&session.id).ok())
            .map(|path| path.exists())
            .unwrap_or(false);
        // Skill dedup history: the messages in play when the turn starts,
        // including this turn's prompt (the CLI's `history_for_skill_dedup`).
        // A frozen snapshot, handed to the default `pre_tool` interception so a
        // second activation of the same skill in one turn still reads as
        // already-loaded exactly as before the pipeline migration.
        let history_for_skill_dedup = session.messages.clone();
        // The code preset rides the public Engine seam (ADR-0031 D1/D5): the SDK
        // builds its turn the same way any external host would — through
        // [`slimcode_app::engine::Engine`], injecting the code default hook
        // bundle rather than a privileged internal path. The Engine borrows the
        // provider for the turn, so it lives in its own scope and hands the
        // provider back before the outcome is assembled.
        let result = {
            let mut engine = engine::Engine::code(
                &mut provider,
                &tools,
                &self.config,
                &self.skills,
                &self.cwd,
                history_for_skill_dedup,
            )
            .with_run_config(cfg);
            engine.run_turn(
                context,
                &cancel,
                &mut SdkRenderer { session: self },
                &mut |msg: &AgentMessage, usage: Option<&TokenUsage>| {
                    // The message entered history: mirror it into the session and
                    // append it to the log. Persistence is best-effort — a
                    // storage hiccup must not abort the turn.
                    session.messages.push(msg.clone());
                    appended += 1;
                    // The request's usage (None on tool results) accumulates into
                    // the session (ADR-0018 D4).
                    if let Some(u) = usage {
                        session.usage = session.usage.saturating_add(u);
                    }
                    self.emit(SdkEvent::Message {
                        message: msg.clone(),
                        usage: usage.copied(),
                    });
                    self.append_best_effort(&session, msg);
                    Ok(())
                },
            )
        };
        let stop_reason = match result {
            Ok((_, reason)) => reason,
            Err(e) => {
                // A failed turn still leaves the session persisted with its
                // prior messages intact; the failure closes the log on an
                // assistant boundary (or leaves no file at all if the turn was
                // the session's first and produced nothing, ADR-0009 D5).
                if appended > 0 || log_exists {
                    self.close_turn(&mut session, MessageStopReason::Error, Some(e.clone()));
                }
                return TurnOutcome {
                    provider,
                    tools,
                    session,
                    result: Err(e),
                    compacted: None,
                };
            }
        };
        // Turn-end auto-compaction, captured so the caller can deliver the
        // `Compacted` event after `TurnFinished` and after the taken session
        // state is restored (spec §5 event order). Only a completed turn can
        // reach it: the trigger sits inside the `Completed` arm, so cancelled
        // and failed turns never compact.
        let mut compacted: Option<(usize, usize)> = None;
        match &stop_reason {
            StopReason::Completed => {
                // Auto-compaction (ticket 06): a completed turn whose history
                // crossed the threshold replaces the old span with a summary
                // checkpoint; a failure keeps the full history silently.
                compacted = self
                    .apply_turn_end(&mut provider, &mut session, &cancel, false)
                    .unwrap_or_default();
            }
            // An interrupted turn adopts its partial history and closes the log
            // on an assistant boundary (never compacts). The guardrail stops
            // (ticket 08) join this arm for completeness: the code preset leaves
            // `max_iterations` / `timeout` unset, so only `Cancelled` arises on
            // this path (the equivalence gate).
            StopReason::Cancelled | StopReason::MaxIterations | StopReason::Timeout => {
                // Esc: the partial turn is adopted; the log is closed on an
                // assistant boundary so it never ends on a dangling tool batch
                // (ADR-0009 D3). A first turn that produced nothing has no log
                // to close and adds no closing message (ADR-0009 D5).
                if appended > 0 || log_exists {
                    self.close_turn(&mut session, MessageStopReason::Aborted, None);
                }
            }
        }
        TurnOutcome {
            provider,
            tools,
            session,
            result: Ok(stop_reason),
            compacted,
        }
    }

    /// Run the session's turn-end component over the finished history. This is
    /// the seam ADR-0031 lifts auto-compaction and `compact_now` onto: the
    /// component (default `slimcode_app::preset::compaction_turn_end`,
    /// replaceable or disable-able via [`AgentSessionOptions::with_turn_end`])
    /// does the work, and this method only assembles its
    /// `slimcode_app::preset::TurnEndCtx` — binding
    /// `persist` to the session's own best-effort log append.
    ///
    /// `force` is `false` on the auto path (threshold-gated) and `true` for
    /// `compact_now`. Returns the component's token change (`Ok(Some(..))`),
    /// `Ok(None)` when it ran but changed nothing, or `Err` on failure (auto
    /// swallows it, `compact_now` propagates it).
    fn apply_turn_end(
        &self,
        provider: &mut Box<dyn Provider + Send>,
        session: &mut Session,
        cancel: &CancelToken,
        force: bool,
    ) -> Result<Option<(usize, usize)>, String> {
        let mut persist = |session: &Session, message: &AgentMessage| {
            self.append_best_effort(session, message);
        };
        let mut ctx = preset::TurnEndCtx {
            provider,
            config: &self.config,
            cancel,
            session,
            force,
            persist: &mut persist,
        };
        (self.turn_end)(&mut ctx)
    }

    /// Force an on-demand compaction of the session's history, ignoring the
    /// auto-compaction threshold (`/compact` semantics). Single-flight with
    /// [`Self::prompt`] (take/put): a turn currently running rejects the call
    /// with `"a turn is already running"`, and a disposed session errors.
    ///
    /// On success the history is replaced by a `CompactSummary` checkpoint
    /// (persisted best-effort, ADR-0020 D2), the summary request's token cost
    /// folds into [`Self::usage`] (ADR-0018 D4), a synchronous
    /// [`SdkEvent::Compacted { before, after }`](SdkEvent::Compacted) is
    /// emitted, and `Ok((before, after))` reports the token change. A failure
    /// returns the provider error detail (or a clear "nothing to compact"
    /// error) with the history untouched and no event emitted.
    pub fn compact_now(&self) -> Result<(usize, usize), String> {
        // Take the turn state out of the lock so compaction runs without
        // holding it (the same take/put seam as `prompt`). The provider slot
        // is the single-flight marker: a running turn holds it, so a
        // concurrent `compact_now` errors and vice versa.
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "session lock poisoned".to_string())?;
        if inner.disposed {
            return Err("session is disposed".to_string());
        }
        let Some(mut provider) = inner.provider.take() else {
            return Err("a turn is already running".to_string());
        };
        let mut session = inner.session.clone();
        // A fresh compaction starts uncancelled. Reset while still holding the
        // lock so a concurrent `dispose` (which needs the lock to set its flag)
        // cannot have its cancel wiped by this call's start.
        self.cancel.reset();
        let cancel = self.cancel.clone();
        drop(inner);

        let outcome = self.apply_turn_end(&mut provider, &mut session, &cancel, true);

        // Put everything back, whatever the outcome (a failure leaves the
        // history and this turn's result untouched).
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "session lock poisoned".to_string())?;
        inner.provider = Some(provider);
        inner.session = session;
        drop(inner);

        match outcome {
            Ok(Some((before, after))) => {
                self.emit(SdkEvent::Compacted { before, after });
                // Same turn-end barrier as `prompt`: the `Compacted` event has
                // reached every subscriber (the FFI queue holds it) by the time
                // this returns.
                self.flush_events();
                Ok((before, after))
            }
            // The default compaction component always changes something on a
            // forced call (or errors), so this only happens when a host swapped
            // in a component that no-ops (e.g. `preset::noop_turn_end`): report
            // the absence of change rather than emitting a hollow `Compacted`.
            Ok(None) => Err("turn-end component made no change".to_string()),
            Err(e) => Err(e),
        }
    }

    /// Request cancellation of the running turn from any thread.
    ///
    /// Cancellation only sets the shared [`CancelToken`], so it never waits
    /// on the session lock and returns immediately even while a turn is
    /// running with the turn state taken out (the take/put seam, ADR-0013
    /// D2). The running `prompt` ends at the next runner boundary (or the
    /// next interruptible provider read) with `StopReason::Cancelled`, and a
    /// later `prompt` starts from a fresh, uncancelled token. Calling `abort`
    /// with no turn running leaves the next turn uncancelled.
    pub fn abort(&self) {
        self.cancel.cancel();
    }

    /// Dispose the session: reject every later `prompt`, cancel an in-flight
    /// turn, and drop all subscribers.
    ///
    /// Drops every listener first, so once a turn is released by the cancel
    /// below it can no longer race an event into a listener that dispose is
    /// meant to drop; then sets the disposed flag so later `prompt` calls
    /// fail with an error containing `"session is disposed"`, and cancels the
    /// shared token so an in-flight turn ends `Cancelled` at its next
    /// boundary. Idempotent.
    pub fn dispose(&self) {
        self.listeners
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .disposed = true;
        self.cancel.cancel();
        // Stop the dispatcher. Clearing the subscribers above already silences
        // every later delivery; this additionally retires the thread so a
        // disposed-but-not-yet-dropped session (the FFI keeps its handle) does
        // not park forever on an empty bus. A turn still in flight finds the bus
        // gone: its later publishes fail (dropped) and its flush is a no-op, so
        // it unwinds cleanly rather than blocking.
        let _ = self.bus_tx.send(bus::BusMsg::Shutdown);
    }

    /// Persist one message, swallowing failures (best-effort, ADR-0009 D6).
    fn append_best_effort(&self, session: &Session, message: &AgentMessage) {
        if let Some(store) = &self.store {
            let _ = store.append(session, message);
        }
    }

    /// Close the session log after a failed/cancelled turn: an assistant
    /// message with a short, non-empty text enters memory and is appended, so
    /// the live session adopts the partial turn and the log ends on an
    /// assistant boundary (ADR-0009 D3). The turn's reason rides the record
    /// envelope (ADR-0012 D4), never the message payload.
    ///
    /// The caller only reaches this on a turn that produced a message or a
    /// log that already exists (ADR-0009 D5): a first turn that produced
    /// nothing must not create a log just to hold the closing message, exactly
    /// like the CLI's `TuiSession::close_turn`.
    fn close_turn(&self, session: &mut Session, reason: MessageStopReason, error: Option<String>) {
        let text = match &error {
            Some(e) => format!("The turn ended with an error: {e}"),
            None => "The turn was cancelled.".to_string(),
        };
        let message = AgentMessage::text(Role::Assistant, text);
        session.messages.push(message.clone());
        if let Some(store) = &self.store {
            let _ = store.append_closing(session, &message, &reason, error.as_deref());
        }
    }

    /// Publish one event onto the session's bounded bus (ticket 06). Delivery
    /// itself happens on the dispatcher thread, in subscription order, with each
    /// listener isolated by `catch_unwind` (see [`bus::run`]) — this call only
    /// hands the event over. A `SyncSender::send` blocks while the buffer is
    /// full, which is the backpressure a sustained-slow subscriber applies (a
    /// burst stays absorbed by the capacity); a send failure means the bus is
    /// gone (disposed / dropped), i.e. the silenced-subscriber case, never an
    /// error to surface to the turn.
    fn emit(&self, event: SdkEvent) {
        let _ = self.bus_tx.send(bus::BusMsg::Event(event));
    }

    /// Block until the dispatcher has delivered every event published so far on
    /// this thread. The turn-end barrier that keeps `prompt` / `compact_now`
    /// synchronous from the host's point of view even though delivery runs on
    /// the dispatcher thread: when the call returns, its whole ordered event
    /// stream has gone out (the FFI queue is populated, the equivalence baseline
    /// is complete). A dead bus (disposed / dropped) returns immediately.
    fn flush_events(&self) {
        let (ack_tx, ack_rx) = mpsc::sync_channel(1);
        if self.bus_tx.send(bus::BusMsg::Flush(ack_tx)).is_ok() {
            let _ = ack_rx.recv();
        }
    }

    /// Subscribe to this session's events. The listener is called on the bus's
    /// dispatcher thread, in subscription order, for every event of each
    /// subsequent `prompt` (delivered before the call returns, thanks to its
    /// turn-end flush). The returned [`Unsubscribe`]
    /// removes it when called (a listener may call it from inside its own
    /// callback: the current event has already been delivered, later ones
    /// stop). `+ Sync` is required so the listener can be shared across the
    /// `Arc`-backed snapshot the dispatcher delivers each event through.
    pub fn subscribe(&self, listener: impl Fn(SdkEvent) + Send + Sync + 'static) -> Unsubscribe {
        let id = self.next_listener_id.fetch_add(1, Ordering::SeqCst);
        let listener: Arc<dyn Fn(SdkEvent) + Send + Sync + 'static> = Arc::new(listener);
        self.listeners
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((id, listener));
        let listeners = Arc::clone(&self.listeners);
        Box::new(move || {
            let mut list = listeners.lock().unwrap_or_else(|e| e.into_inner());
            list.retain(|(existing, _)| *existing != id);
        })
    }

    /// The session's message history so far (a snapshot copy).
    pub fn messages(&self) -> Vec<AgentMessage> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .session
            .messages
            .clone()
    }

    /// The session id (`slimcode-<unix>-<pid>-<n>`).
    pub fn session_id(&self) -> &str {
        &self.id
    }

    /// The inferred session title, if the first prompt has produced one.
    pub fn title(&self) -> Option<String> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .session
            .title
            .clone()
    }

    /// The token usage accumulated across all turns of this session
    /// (in-memory only, ADR-0018 D4).
    pub fn usage(&self) -> TokenUsage {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .session
            .usage
    }
}

impl Drop for AgentSession {
    /// Retire the dispatcher thread when the session goes away: a best-effort
    /// [`bus::BusMsg::Shutdown`] so it does not block forever on an empty bus.
    /// Pending events need not drain — the owner is dropping the session; and a
    /// not-yet-started `Shutdown` behind live subscribers is harmless.
    fn drop(&mut self) {
        let _ = self.bus_tx.send(bus::BusMsg::Shutdown);
    }
}

/// The SDK's renderer: forwards every [`DisplayItem`] to subscribers as
/// [`SdkEvent::Display`] and never fails (a display hiccup must not abort a
/// turn; the runner's renderer-error path stays unused).
struct SdkRenderer<'a> {
    session: &'a AgentSession,
}

impl Renderer for SdkRenderer<'_> {
    fn render(&mut self, item: &DisplayItem) -> Result<(), String> {
        self.session.emit(SdkEvent::Display(item.clone()));
        Ok(())
    }
}
