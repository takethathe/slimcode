//! The pure-Rust host controller (spec §Implementation Decisions, ADR-0024
//! D4): the session registry (one current `AgentSession`), the worker-thread
//! turn runner, the GUI-wire bridge plumbing, the shared `SessionStore`
//! operations, and the settings assembly. This is the test seam — every
//! piece of host logic lives here, off the Tauri runtime; the Tauri command
//! layer ([`crate::commands`]) is only a thin forwarding shell.
//!
//! State machine: `Idle` / `Running` / `Disposed`. `prompt` / `compact_now`
//! spawn a worker thread (the SDK is synchronous and single-flight, ADR-0021
//! D4); the worker subscribes a listener that encodes each `SdkEvent` as a
//! [`GuiWireEvent`] and pushes it into the caller's sink (a Tauri `Channel`
//! in production, a recorder in tests). `cancel` only calls `abort()` —
//! cross-thread, touching only the shared cancel token (kills a running bash
//! child, ADR-0022 D3) and never waiting on the session lock. `dispose` joins
//! the worker and silences the session.
//!
//! The controller owns the settings ([`GuiSettings`]); a settings change is
//! picked up by the next `prompt` / `compact_now` / `new_session`, which
//! recreates the current session (resuming by id when it has a persisted log)
//! so the new overrides reach the provider — the next prompt uses the new
//! configuration without a restart (spec user story 22).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::SystemTime;

use slimcode_app::compaction;
use slimcode_app::config;
use slimcode_app::context_files::resolve_project_home;
use slimcode_app::session::{SessionStore, project_key};
use slimcode_app::{AgentMessage, TokenUsage};
use slimcode_sdk::{AgentSession, AgentSessionOptions, create_agent_session};

use crate::settings::GuiSettings;
use crate::wire::GuiWireEvent;

/// The controller's lifecycle state (spec D4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// No turn running; a fresh session is ready to prompt.
    Idle,
    /// A turn (or `compact_now`) is running on a worker thread.
    Running,
    /// The controller has been disposed; every later operation errors.
    Disposed,
}

/// One row of the left-sidebar session list (title + time), from the shared
/// session repository.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub title: Option<String>,
    /// Unix seconds of the log's last modification (the frontend formats it).
    pub modified: u64,
    /// Number of messages the log holds.
    pub messages: usize,
}

/// The context-window usage for the top bar (ADR-0020, same estimator and
/// window as the CLI footer).
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct ContextInfo {
    /// Estimated share of the window, 0–100.
    pub percent: f64,
    /// The estimated context window in tokens (`200k`).
    pub window: u64,
}

/// The full snapshot the frontend loads when it (re)connects to the current
/// session: metadata plus the whole history for block rendering.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionSnapshot {
    pub id: String,
    pub title: Option<String>,
    /// The resolved model name of the current session (overrides or the
    /// four-layer resolution).
    pub model: String,
    pub context: ContextInfo,
    pub messages: Vec<AgentMessage>,
    pub usage: TokenUsage,
    /// A non-fatal startup warning (e.g. an unreadable `gui.toml`).
    pub warning: Option<String>,
}

/// The mutable controller state, guarded by [`AppController::inner`]. All
/// mutations happen briefly under the lock; the worker only touches it once,
/// at turn end, to flip `Running` → `Idle` (see [`finish_worker`]).
struct ControllerInner {
    state: SessionState,
    session: Option<Arc<AgentSession>>,
    /// The running turn's handle, joined by the next mutating call (or by
    /// `dispose`). A finished handle stays until the next call joins it.
    worker: Option<JoinHandle<()>>,
    /// The resolved model name of the current session.
    model: String,
    settings: GuiSettings,
    /// Bumped by every `set_settings`; compared against
    /// `session_settings_version` to detect a mid-session settings change.
    settings_version: u64,
    /// The settings version the current session was created with.
    session_settings_version: u64,
    /// The project-scoped store sharing the CLI/TUI repository.
    store: SessionStore,
    home: PathBuf,
    cwd: PathBuf,
    /// A non-fatal startup warning (unreadable `gui.toml` and the like).
    warning: Option<String>,
    /// Test seam: a provider factory used when creating sessions. `None`
    /// (production) lets `create_agent_session` build its default provider.
    #[cfg(test)]
    provider_factory: Option<Box<dyn Fn() -> Box<dyn slimcode_app::Provider + Send> + Send + Sync>>,
    /// Test seam: the tool set injected into created sessions. `None`
    /// (production) lets `create_agent_session` build the default tools.
    #[cfg(test)]
    tools_factory: Option<Box<dyn Fn() -> Vec<slimcode_app::Tool> + Send + Sync>>,
}

/// The pure-Rust host controller. `Send + Sync`, so Tauri can manage it as
/// app state.
pub struct AppController {
    inner: Arc<Mutex<ControllerInner>>,
}

impl AppController {
    /// Build the controller for the given slimcode home and working
    /// directory. Infallible: an unreadable `gui.toml` falls back to default
    /// settings and is reported as a snapshot `warning`; the initial session
    /// is created lazily on first use (so a missing API key surfaces as an
    /// error the user can fix in the settings panel instead of crashing the
    /// app at startup).
    pub fn new(home: PathBuf, cwd: PathBuf) -> Self {
        let (settings, warning) = match GuiSettings::load(&home) {
            Ok(settings) => (settings, None),
            Err(message) => (GuiSettings::default(), Some(message)),
        };
        let store = build_store(&home, &cwd);
        Self {
            inner: Arc::new(Mutex::new(ControllerInner {
                state: SessionState::Idle,
                session: None,
                worker: None,
                model: String::new(),
                settings,
                settings_version: 1,
                session_settings_version: 1,
                store,
                home,
                cwd,
                warning,
                #[cfg(test)]
                provider_factory: None,
                #[cfg(test)]
                tools_factory: None,
            })),
        }
    }

    /// The current lifecycle state (used by the frontend to enable/disable
    /// controls and by tests).
    pub fn state(&self) -> SessionState {
        self.lock().state
    }

    /// Open a fresh empty session, disposing the previous one (if any).
    /// Rejects while a turn is running.
    pub fn new_session(&self) -> Result<(), String> {
        let mut inner = self.lock();
        if inner.state == SessionState::Disposed {
            return Err("controller is disposed".to_string());
        }
        if inner.state == SessionState::Running {
            return Err("a turn is already running".to_string());
        }
        if let Some(session) = inner.session.take() {
            session.dispose();
        }
        inner.session_settings_version = 0; // force recreation below
        ensure_session_locked(&mut inner)
    }

    /// Resume a session by id from the shared repository
    /// (`with_session_id`, persist defaults to true), disposing the previous
    /// session. Rejects while a turn is running.
    pub fn resume_session(&self, id: &str) -> Result<(), String> {
        let mut inner = self.lock();
        if inner.state == SessionState::Disposed {
            return Err("controller is disposed".to_string());
        }
        if inner.state == SessionState::Running {
            return Err("a turn is already running".to_string());
        }
        if let Some(session) = inner.session.take() {
            session.dispose();
        }
        let (options, model) = build_options(&inner.settings, &inner.home, &inner.cwd)?;
        let options = inject_test_seams(options, &inner).with_session_id(id);
        let session = create_agent_session(options).map_err(|e| format!("cannot resume: {e}"))?;
        inner.session = Some(Arc::new(session));
        inner.model = model;
        inner.session_settings_version = inner.settings_version;
        Ok(())
    }

    /// Run one whole turn for `text` on a worker thread, streaming
    /// [`GuiWireEvent`]s into `sink` (a Tauri `Channel` in production, a
    /// recorder in tests). Returns immediately; a failed turn surfaces as a
    /// `GuiWireEvent::Error` at the end of the stream. Rejects while a turn
    /// is already running ("turn already running").
    pub fn prompt(
        &self,
        text: &str,
        sink: impl Fn(GuiWireEvent) + Send + Sync + 'static,
    ) -> Result<(), String> {
        let mut inner = self.lock();
        if inner.state == SessionState::Disposed {
            return Err("controller is disposed".to_string());
        }
        if inner.state == SessionState::Running {
            return Err("a turn is already running".to_string());
        }
        // A settings change since the session was created rebuilds the
        // overrides: recreate the current session (resuming by id when it has
        // a persisted log, so history survives) so the next prompt uses the
        // new config without a restart.
        let settings_changed =
            inner.session.is_some() && inner.session_settings_version != inner.settings_version;
        let no_session = inner.session.is_none();
        if settings_changed {
            let old = inner.session.take().expect("session present");
            old.dispose();
            let id = old.session_id().to_string();
            let (options, model) = build_options(&inner.settings, &inner.home, &inner.cwd)?;
            let options = inject_test_seams(options, &inner);
            let options = if store_has_log(&inner.store, &id) {
                options.with_session_id(id)
            } else {
                options
            };
            let session = create_agent_session(options)
                .map_err(|e| format!("cannot rebuild session with new settings: {e}"))?;
            inner.session = Some(Arc::new(session));
            inner.model = model;
            inner.session_settings_version = inner.settings_version;
        } else if no_session {
            ensure_session_locked(&mut inner)?;
        }
        let session = Arc::clone(inner.session.as_ref().expect("session ensured"));
        let shared = Arc::clone(&self.inner);
        let text = text.to_string();
        let worker = spawn_worker(session, shared, sink, move |session| {
            session.prompt(&text).map(|_| ())
        });
        inner.worker = Some(worker);
        inner.state = SessionState::Running;
        Ok(())
    }

    /// Force an on-demand compaction of the current session's history on a
    /// worker thread (`/compact` semantics). Single-flight with `prompt`
    /// (the SDK shares the same seam); events stream over `sink`, and a
    /// failure (e.g. "nothing to compact") surfaces as `GuiWireEvent::Error`.
    pub fn compact_now(
        &self,
        sink: impl Fn(GuiWireEvent) + Send + Sync + 'static,
    ) -> Result<(), String> {
        let mut inner = self.lock();
        if inner.state == SessionState::Disposed {
            return Err("controller is disposed".to_string());
        }
        if inner.state == SessionState::Running {
            return Err("a turn is already running".to_string());
        }
        if inner.session.is_none() {
            ensure_session_locked(&mut inner)?;
        }
        let session = Arc::clone(inner.session.as_ref().expect("session ensured"));
        let shared = Arc::clone(&self.inner);
        let worker = spawn_worker(session, shared, sink, |session| {
            session.compact_now().map(|_| ())
        });
        inner.worker = Some(worker);
        inner.state = SessionState::Running;
        Ok(())
    }

    /// Cancel the running turn from any thread: only calls `abort()` (touches
    /// the shared cancel token; kills a running bash child) and never waits on
    /// the session lock.
    pub fn cancel(&self) {
        if let Some(session) = self.lock().session.clone() {
            session.abort();
        }
    }

    /// List the shared repository's sessions, newest first.
    pub fn list_sessions(&self) -> Result<Vec<SessionInfo>, String> {
        let inner = self.lock();
        inner
            .store
            .entries()?
            .into_iter()
            .map(|entry| {
                Ok(SessionInfo {
                    id: entry.id,
                    title: entry.title,
                    modified: unix_secs(entry.modified),
                    messages: entry.messages,
                })
            })
            .collect()
    }

    /// Remove a session's JSONL log (with the current session disposed first
    /// when it is the one deleted, then a fresh session opened). Rejects
    /// while a turn is running.
    pub fn delete_session(&self, id: &str) -> Result<(), String> {
        let mut inner = self.lock();
        if inner.state == SessionState::Disposed {
            return Err("controller is disposed".to_string());
        }
        if inner.state == SessionState::Running {
            return Err("a turn is already running".to_string());
        }
        let path = inner.store.session_path(id)?;
        if path.exists() {
            fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        // Deleting the current session switches to a fresh one.
        if inner.session.as_ref().is_some_and(|s| s.session_id() == id) {
            if let Some(session) = inner.session.take() {
                session.dispose();
            }
            ensure_session_locked(&mut inner)?;
        }
        Ok(())
    }

    /// The current session's full snapshot for the frontend's (re)load. The
    /// first call creates the launch session (the app opens on a fresh empty
    /// session).
    pub fn get_session(&self) -> Result<SessionSnapshot, String> {
        let mut inner = self.lock();
        if inner.state == SessionState::Disposed {
            return Err("controller is disposed".to_string());
        }
        if inner.session.is_none() {
            ensure_session_locked(&mut inner)?;
        }
        let session = inner.session.as_ref().expect("session ensured");
        let messages = session.messages();
        Ok(SessionSnapshot {
            id: session.session_id().to_string(),
            title: session.title(),
            model: inner.model.clone(),
            context: context_info(&messages),
            messages,
            usage: session.usage(),
            warning: inner.warning.clone(),
        })
    }

    /// The GUI settings (model / base_url / cache / optional api_key).
    pub fn get_settings(&self) -> GuiSettings {
        self.lock().settings.clone()
    }

    /// Persist new GUI settings (never `config.toml`; the `gui.toml` write
    /// tightens permissions to 0600). The change applies to the next prompt
    /// (the session is recreated with the new overrides) without a restart.
    /// A persistence failure still applies the settings for this run; the
    /// error reports that saving to disk failed.
    pub fn set_settings(&self, settings: GuiSettings) -> Result<(), String> {
        let mut inner = self.lock();
        if inner.state == SessionState::Disposed {
            return Err("controller is disposed".to_string());
        }
        inner.settings = settings.clone();
        inner.settings_version = inner.settings_version.saturating_add(1);
        settings.save(&inner.home)
    }

    /// Dispose: reject every later operation, dispose the current session
    /// (clearing listeners + cancelling any in-flight turn), and join the
    /// worker so the turn's thread has exited. Idempotent.
    pub fn dispose(&self) {
        let (session, worker) = {
            let mut inner = self.lock();
            inner.state = SessionState::Disposed;
            (inner.session.take(), inner.worker.take())
        };
        if let Some(session) = session {
            session.dispose();
        }
        if let Some(worker) = worker {
            let _ = worker.join();
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ControllerInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Drop for AppController {
    /// On teardown (Tauri drops the managed state), dispose the session and
    /// join the worker so no background thread outlives the app (spec user
    /// story 23).
    fn drop(&mut self) {
        self.dispose();
    }
}

/// Ensure `inner.session` exists, creating it with the current settings as
/// the highest-precedence overrides. Also refreshes the resolved model name.
fn ensure_session_locked(inner: &mut ControllerInner) -> Result<(), String> {
    let (options, model) = build_options(&inner.settings, &inner.home, &inner.cwd)?;
    let options = inject_test_seams(options, inner);
    let session = create_agent_session(options)?;
    inner.session = Some(Arc::new(session));
    inner.model = model;
    inner.session_settings_version = inner.settings_version;
    inner.warning = None;
    Ok(())
}

/// Build the session options from the current settings (highest-precedence
/// SDK overrides), plus the pinned home and working directory. Also resolves
/// and returns the model name (`load_app_config` mirrors `create_agent_session`'s
/// own four-layer resolution, so the two agree).
fn build_options(
    settings: &GuiSettings,
    home: &Path,
    cwd: &Path,
) -> Result<(AgentSessionOptions, String), String> {
    let app = config::load_app_config(settings.overrides())?;
    let model = app.provider.model.clone();
    let options = AgentSessionOptions::new(Some(cwd.to_path_buf())).with_home(home);
    let options = match &settings.base_url {
        Some(url) => options.with_base_url(url.clone()),
        None => options,
    };
    let options = match &settings.model {
        Some(model) => options.with_model(model.clone()),
        None => options,
    };
    let options = match &settings.api_key {
        Some(key) => options.with_api_key(key.clone()),
        None => options,
    };
    let options = match settings.cache {
        Some(cache) => options.with_cache(cache),
        None => options,
    };
    Ok((options, model))
}

/// The project-scoped store mirroring `create_agent_session`'s, so the GUI
/// lists / resumes the exact sessions the CLI and TUI share.
fn build_store(home: &Path, cwd: &Path) -> SessionStore {
    let user_home = std::env::var_os("HOME").map(PathBuf::from);
    let project_home = resolve_project_home(cwd, user_home.as_deref());
    SessionStore::new(
        home.join("sessions"),
        project_key(&project_home),
        project_home,
    )
}

/// Whether the store holds a log for `id` (a session with no persisted
/// history cannot be resumed; it is recreated fresh instead).
fn store_has_log(store: &SessionStore, id: &str) -> bool {
    store.session_path(id).is_ok_and(|path| path.exists())
}

/// The context-window usage the top bar shows — the same estimator and window
/// as the CLI footer (ticket 06), so the GUI's percentage always agrees with
/// the auto-compaction trigger.
fn context_info(messages: &[AgentMessage]) -> ContextInfo {
    let window = compaction::estimated_context_window() as u64;
    let total = compaction::estimate_total_tokens(messages) as u64;
    let percent = if window == 0 {
        0.0
    } else {
        total as f64 / window as f64 * 100.0
    };
    ContextInfo { percent, window }
}

/// The worker's final act: flip `Running` → `Idle` (unless the controller was
/// disposed while the turn ran — `dispose` sets `Disposed` first and joins, so
/// this must never overwrite that).
fn finish_worker(shared: Arc<Mutex<ControllerInner>>) {
    let mut inner = shared.lock().unwrap_or_else(|e| e.into_inner());
    if inner.state == SessionState::Running {
        inner.state = SessionState::Idle;
    }
}

/// Spawn the single-flight worker shared by `prompt` and `compact_now`: run
/// `op` on a fresh thread with a session listener attached, surface a failure
/// as a `GuiWireEvent::Error`, then return the controller to `Idle`. The op's
/// `Ok` value is discarded (the events stream over the sink; nothing else is
/// needed), so both callers pass their differently-typed results mapped to
/// `Result<(), String>`.
fn spawn_worker<S, O>(
    session: Arc<AgentSession>,
    shared: Arc<Mutex<ControllerInner>>,
    sink: S,
    op: O,
) -> std::thread::JoinHandle<()>
where
    S: Fn(GuiWireEvent) + Send + Sync + 'static,
    O: FnOnce(&AgentSession) -> Result<(), String> + Send + 'static,
{
    std::thread::spawn(move || {
        // Subscribe before running: every event is delivered synchronously on
        // this thread in subscription order.
        let sink = Arc::new(sink);
        let sink_for_listener = Arc::clone(&sink);
        let unsubscribe = session.subscribe(move |event| {
            sink_for_listener(GuiWireEvent::from(&event));
        });
        let result = op(&session);
        drop(unsubscribe);
        if let Err(message) = result {
            sink(GuiWireEvent::Error { message });
        }
        finish_worker(shared);
    })
}

/// `SystemTime` → unix seconds, clamped to 0 for a pre-epoch clock.
fn unix_secs(time: SystemTime) -> u64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Apply the test seams' provider and tool factories (no-ops in production
/// builds, where both factories are always `None`).
#[cfg(test)]
fn inject_test_seams(options: AgentSessionOptions, inner: &ControllerInner) -> AgentSessionOptions {
    let options = match &inner.provider_factory {
        Some(factory) => options.with_provider(factory()),
        None => options,
    };
    match &inner.tools_factory {
        Some(factory) => options.with_tools(factory()),
        None => options,
    }
}

#[cfg(not(test))]
fn inject_test_seams(
    options: AgentSessionOptions,
    _inner: &ControllerInner,
) -> AgentSessionOptions {
    options
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use slimcode_app::testutil::unique_temp_dir;
    use slimcode_app::{
        CancelToken, Delta, FinishReason, Message, Provider, ProviderConfig, Role, ToolSpec,
    };
    use slimcode_sdk::StopReason;

    /// A fresh scratch `home` + `cwd` (a git repo, so the project home
    /// resolves deterministically to `cwd` itself) for one test, with
    /// `SLIMCODE_HOME` pointed at it so `config::load_app_config` reads the
    /// scratch `config.toml` (absent) instead of the developer's real home.
    struct Scratch {
        home: PathBuf,
        cwd: PathBuf,
        _guard: EnvGuard,
    }

    impl Scratch {
        fn new(tag: &str) -> Self {
            let root = unique_temp_dir(&format!("slimcode-gui-{tag}"));
            let home = root.join("home");
            let cwd = root.join("proj");
            std::fs::create_dir_all(&home).expect("home");
            std::fs::create_dir_all(&cwd).expect("cwd");
            std::fs::create_dir_all(cwd.join(".git")).expect("git marker");
            // Point `SLIMCODE_HOME` at the scratch home (so the four-layer
            // config resolution reads the scratch `config.toml`, absent) and
            // provide a dummy API key (the resolution requires one; the
            // injected FakeProvider never talks to a real endpoint).
            let _guard = EnvGuard::set("SLIMCODE_HOME", home.clone());
            let _guard = _guard.and("DASHSCOPE_API_KEY", "sk-test");
            Scratch { home, cwd, _guard }
        }

        /// A controller whose sessions use `provider` (cloned per session, so
        /// each `create_agent_session` gets a fresh instance while the tests
        /// share the provider's recorded state).
        fn controller<P: Provider + Clone + Send + Sync + 'static>(
            &self,
            provider: P,
        ) -> AppController {
            let controller = AppController::new(self.home.clone(), self.cwd.clone());
            controller
                .inner
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .provider_factory = Some(Box::new(move || Box::new(provider.clone())));
            controller
        }

        /// Like [`Self::controller`], with a tool factory injected into every
        /// created session (the SDK's `with_tools` seam).
        fn controller_with_tools<P, F>(&self, provider: P, tools: F) -> AppController
        where
            P: Provider + Clone + Send + Sync + 'static,
            F: Fn() -> Vec<slimcode_app::Tool> + Send + Sync + 'static,
        {
            let controller = self.controller(provider);
            controller
                .inner
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .tools_factory = Some(Box::new(tools));
            controller
        }
    }

    /// A scripted provider (the SDK's pattern): each `chat` returns the next
    /// delta batch, records the `ProviderConfig` it was handed (so tests can
    /// assert the settings overrides reached the provider), and optionally
    /// cancels the shared token before a 1-based call index.
    #[derive(Clone)]
    struct FakeProvider {
        script: Vec<Vec<Delta>>,
        per_call: TokenUsage,
        /// Number of `chat` calls so far (0 before the first).
        calls: usize,
        /// Cancel the shared token before the `n`-th `chat` call (1-based).
        cancels_on: Option<usize>,
        /// `(base_url, model, cache)` seen per call, appended by `chat`.
        seen: Arc<Mutex<Vec<(String, String, bool)>>>,
    }

    impl FakeProvider {
        fn text_turn(text: &str) -> Self {
            Self {
                script: vec![vec![Delta::Text(text.to_string()), done_stop()]],
                per_call: usage(10, 5),
                calls: 0,
                cancels_on: None,
                seen: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl Provider for FakeProvider {
        fn chat(
            &mut self,
            _messages: &[Message],
            _tools: &[ToolSpec],
            config: &ProviderConfig,
            cancel: &CancelToken,
            on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
        ) -> Result<Option<TokenUsage>, String> {
            if self.cancels_on == Some(self.calls + 1) {
                cancel.cancel();
            }
            self.calls += 1;
            self.seen.lock().expect("seen lock").push((
                config.base_url.clone(),
                config.model.clone(),
                config.cache,
            ));
            let batch = self.script.get(self.calls - 1).cloned().unwrap_or_default();
            for delta in batch {
                on_delta(delta)?;
            }
            Ok(Some(self.per_call))
        }
    }

    /// A provider that parks inside `chat` until released — used to hold a
    /// turn open while `cancel` / a second `prompt` is exercised.
    #[derive(Clone)]
    struct BlockingProvider {
        started: Arc<AtomicBool>,
        release: Arc<AtomicBool>,
    }

    impl Provider for BlockingProvider {
        fn chat(
            &mut self,
            _messages: &[Message],
            _tools: &[ToolSpec],
            _config: &ProviderConfig,
            _cancel: &CancelToken,
            on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
        ) -> Result<Option<TokenUsage>, String> {
            self.started.store(true, Ordering::SeqCst);
            while !self.release.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(1));
            }
            on_delta(Delta::Text("partial".to_string()))?;
            Ok(None)
        }
    }

    /// A provider that parks inside `chat` until the shared cancel token is
    /// set (the SDK runner's interruptible-read analogue, mirroring the
    /// lifecycle test's `CancelAwareBlockingProvider`): lets a test drive
    /// `controller.cancel()` — the lock → `abort()` glue — against a turn
    /// that is provably in flight.
    #[derive(Clone)]
    struct CancelAwareBlockingProvider {
        started: Arc<AtomicBool>,
    }

    impl Provider for CancelAwareBlockingProvider {
        fn chat(
            &mut self,
            _messages: &[Message],
            _tools: &[ToolSpec],
            _config: &ProviderConfig,
            cancel: &CancelToken,
            _on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
        ) -> Result<Option<TokenUsage>, String> {
            self.started.store(true, Ordering::SeqCst);
            while !cancel.is_cancelled() {
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(None)
        }
    }

    /// A deterministic tool the scripted provider can call.
    fn probe_tool() -> slimcode_app::Tool {
        slimcode_app::Tool::new(
            "probe",
            "returns a probe result",
            serde_json::json!({}),
            |_| Ok("probed".to_string()),
        )
    }

    fn text_delta(t: &str) -> Delta {
        Delta::Text(t.to_string())
    }
    fn done_stop() -> Delta {
        Delta::Done(FinishReason::Stop)
    }
    fn done_tools() -> Delta {
        Delta::Done(FinishReason::ToolCalls)
    }
    fn tc_start(index: usize, id: &str, name: &str) -> Delta {
        Delta::ToolCallStart {
            index,
            id: id.to_string(),
            name: name.to_string(),
        }
    }
    fn tc_args(index: usize, fragment: &str) -> Delta {
        Delta::ToolCallArgs {
            index,
            fragment: fragment.to_string(),
        }
    }

    fn usage(prompt: u64, completion: u64) -> TokenUsage {
        TokenUsage {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: prompt + completion,
            prompt_tokens_details: None,
        }
    }

    /// A recording event sink collecting every wire event it receives.
    #[derive(Default)]
    struct Recorder {
        events: Arc<Mutex<Vec<GuiWireEvent>>>,
    }

    impl Recorder {
        fn new() -> Self {
            Self::default()
        }
        fn sink(&self) -> impl Fn(GuiWireEvent) + Send + Sync + 'static {
            let events = Arc::clone(&self.events);
            move |event| events.lock().expect("recorder lock").push(event)
        }
        fn events(&self) -> Vec<GuiWireEvent> {
            self.events.lock().expect("recorder lock").clone()
        }
    }

    fn kind_of(event: &GuiWireEvent) -> &'static str {
        match event {
            GuiWireEvent::Turn { .. } => "turn",
            GuiWireEvent::Reasoning { .. } => "reasoning",
            GuiWireEvent::Text { .. } => "text",
            GuiWireEvent::ToolStart { .. } => "tool_start",
            GuiWireEvent::ToolResult { .. } => "tool_result",
            GuiWireEvent::Stop { .. } => "stop",
            GuiWireEvent::Usage { .. } => "usage",
            GuiWireEvent::Message { .. } => "message",
            GuiWireEvent::TurnFinished { .. } => "turn_finished",
            GuiWireEvent::Compacted { .. } => "compacted",
            GuiWireEvent::Error { .. } => "error",
        }
    }

    /// Block until `predicate` holds or a generous timeout elapses (the
    /// worker turns run on separate threads).
    fn wait_until(predicate: impl Fn() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !predicate() {
            if std::time::Instant::now() > deadline {
                panic!("timed out waiting for condition");
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// An environment guard restoring a variable's previous value on drop.
    struct EnvGuard {
        pairs: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    impl EnvGuard {
        fn set(name: &'static str, value: PathBuf) -> Self {
            let old = std::env::var_os(name);
            unsafe { std::env::set_var(name, value) };
            EnvGuard {
                pairs: vec![(name, old)],
            }
        }

        fn and(mut self, name: &'static str, value: &str) -> Self {
            let old = std::env::var_os(name);
            unsafe { std::env::set_var(name, value) };
            self.pairs.push((name, old));
            self
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (name, old) in self.pairs.drain(..).rev() {
                match old {
                    Some(value) => unsafe { std::env::set_var(name, value) },
                    None => unsafe { std::env::remove_var(name) },
                }
            }
        }
    }

    // --- tests --------------------------------------------------------------

    #[test]
    fn first_get_session_creates_a_fresh_empty_session() {
        let scratch = Scratch::new("launch");
        let controller = scratch.controller(FakeProvider::text_turn("hi"));
        let snapshot = controller.get_session().expect("launch session");
        assert!(snapshot.messages.is_empty());
        // No settings: the model resolves through the four-layer defaults
        // (the same `DEFAULT_MODEL` the CLI uses).
        assert_eq!(snapshot.model, slimcode_app::config::DEFAULT_MODEL);
        assert_eq!(controller.state(), SessionState::Idle);
    }

    #[test]
    fn full_text_turn_streams_ordered_events() {
        let scratch = Scratch::new("full-turn");
        let (provider, _seen) = fake_provider(
            vec![vec![text_delta("hello "), text_delta("world"), done_stop()]],
            usage(10, 5),
        );
        let controller = scratch.controller(provider);
        let recorder = Recorder::new();
        controller
            .prompt("build something", recorder.sink())
            .expect("prompt starts");
        wait_until(|| {
            recorder
                .events()
                .iter()
                .any(|e| matches!(e, GuiWireEvent::TurnFinished { .. }))
        });
        wait_until(|| controller.state() == SessionState::Idle);

        let events = recorder.events();
        assert!(
            events.len() >= 7,
            "expected a full turn stream, got {events:?}"
        );
        // Opens with the user prompt message.
        assert_eq!(kind_of(&events[0]), "message");
        assert!(matches!(
            &events[0],
            GuiWireEvent::Message { message, usage } if message.role() == &Role::User && usage.is_none()
        ));
        // The streamed text fragments are all delivered, in order.
        let text: String = events
            .iter()
            .filter_map(|e| match e {
                GuiWireEvent::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "hello world");
        // stop / usage / message(assistant) / turn_finished close the turn.
        assert!(events.iter().any(|e| kind_of(e) == "stop"));
        assert!(events.iter().any(|e| kind_of(e) == "usage"));
        assert_eq!(kind_of(events.last().unwrap()), "turn_finished");
        assert!(!events.iter().any(|e| kind_of(e) == "error"));

        // The streamed text landed in the session history.
        let snapshot = controller.get_session().expect("session");
        let assistant_texts: Vec<String> = snapshot
            .messages
            .iter()
            .filter(|m| m.role() == &Role::Assistant)
            .map(|m| m.text_content())
            .collect();
        assert!(
            assistant_texts.iter().any(|t| t.contains("hello world")),
            "{assistant_texts:?}"
        );
    }

    /// A tool-calling turn streams the card's two events in order
    /// (`tool_start` before the tool `message`, `tool_result` after it) and
    /// runs the tool through the real runner — the automatic counterpart of
    /// ticket 03's manual "tool turn → three-state card" acceptance.
    #[test]
    fn tool_turn_streams_tool_start_then_result() {
        let scratch = Scratch::new("tool-turn");
        let (provider, _seen) = fake_provider(
            vec![
                vec![
                    tc_start(0, "call_1", "probe"),
                    tc_args(0, "{}"),
                    done_tools(),
                ],
                vec![text_delta("done"), done_stop()],
            ],
            usage(10, 5),
        );
        let controller = scratch.controller_with_tools(provider, || vec![probe_tool()]);
        let recorder = Recorder::new();
        controller
            .prompt("use the probe tool", recorder.sink())
            .expect("prompt starts");
        // The turn spans two provider calls (tool batch, then the final
        // reply), so wait for the authoritative end rather than Idle.
        wait_until(|| {
            recorder
                .events()
                .iter()
                .any(|e| matches!(e, GuiWireEvent::TurnFinished { .. }))
        });
        wait_until(|| controller.state() == SessionState::Idle);

        let events = recorder.events();
        let kinds: Vec<&str> = events.iter().map(kind_of).collect();
        let start = kinds
            .iter()
            .position(|k| *k == "tool_start")
            .expect("tool_start");
        let result = kinds
            .iter()
            .position(|k| *k == "tool_result")
            .expect("tool_result");
        assert!(
            start < result,
            "tool_start must precede tool_result: {kinds:?}"
        );
        assert_eq!(kind_of(events.last().unwrap()), "turn_finished");
        assert!(!events.iter().any(|e| kind_of(e) == "error"), "{events:?}");

        // The card payload the frontend renders: name + ok + result.
        let (name, ok, result_text) = events
            .iter()
            .find_map(|e| match e {
                GuiWireEvent::ToolResult {
                    name, ok, result, ..
                } => Some((name.clone(), *ok, result.clone())),
                _ => None,
            })
            .expect("a tool_result payload");
        assert_eq!(name, "probe");
        assert!(ok);
        assert_eq!(result_text, "probed");

        // One tool message entered history (`probe_tool` is deterministic
        // and side-effect free).
        let snapshot = controller.get_session().expect("session");
        assert!(
            snapshot
                .messages
                .iter()
                .any(|m| m.role() == &Role::Tool && m.text_content().contains("probed")),
            "tool result must enter history: {:?}",
            snapshot.messages
        );
    }

    #[test]
    fn second_prompt_while_running_errors_then_idles() {
        let scratch = Scratch::new("single-flight");
        let started = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let provider = BlockingProvider {
            started: Arc::clone(&started),
            release: Arc::clone(&release),
        };
        let controller = scratch.controller(provider);
        controller
            .prompt("first", Recorder::new().sink())
            .expect("first prompt starts");
        wait_until(|| started.load(Ordering::SeqCst));
        assert_eq!(controller.state(), SessionState::Running);

        // A concurrent prompt is rejected.
        let err = controller
            .prompt("second", Recorder::new().sink())
            .unwrap_err();
        assert!(err.contains("already running"), "{err}");

        // Release the turn; it completes and the controller returns to Idle.
        release.store(true, Ordering::SeqCst);
        wait_until(|| controller.state() == SessionState::Idle);
        controller
            .prompt("third", Recorder::new().sink())
            .expect("third prompt starts after idle");
        wait_until(|| controller.state() == SessionState::Idle);
    }

    #[test]
    fn cancel_returns_cancelled_and_partial_text_stays_out_of_history() {
        let scratch = Scratch::new("cancel");
        // Turn 1 completes; turn 2 streams partial text but is cancelled by
        // the SDK's own cancel-aware provider path (the shared token is set
        // before the second `chat` call).
        let (provider, _seen) = fake_provider(
            vec![
                vec![text_delta("first reply"), done_stop()],
                vec![text_delta("partial"), done_stop()],
            ],
            usage(1, 1),
        );
        let mut provider = provider;
        provider.cancels_on = Some(2);
        let controller = scratch.controller(provider);
        let recorder = Recorder::new();
        controller
            .prompt("first", recorder.sink())
            .expect("first prompt");
        wait_until(|| controller.state() == SessionState::Idle);

        controller
            .prompt("second", recorder.sink())
            .expect("second prompt");
        wait_until(|| controller.state() == SessionState::Idle);

        let events = recorder.events();
        let finished = events
            .iter()
            .rev()
            .find(|e| matches!(e, GuiWireEvent::TurnFinished { .. }))
            .expect("a cancelled turn still ends with turn_finished");
        assert!(matches!(
            finished,
            GuiWireEvent::TurnFinished {
                stop_reason: StopReason::Cancelled
            }
        ));

        // The cancelled turn's partial text must not enter history: the
        // session holds user1, assistant1, user2 and the closing message.
        let snapshot = controller.get_session().expect("session");
        let all_text: Vec<String> = snapshot.messages.iter().map(|m| m.text_content()).collect();
        assert_eq!(all_text.len(), 4, "{all_text:?}");
        assert!(
            all_text.iter().all(|t| !t.contains("partial")),
            "partial text leaked into history: {all_text:?}"
        );
        assert!(
            all_text.iter().any(|t| t.contains("cancelled")),
            "closing message missing: {all_text:?}"
        );
    }

    #[test]
    fn controller_cancel_aborts_an_in_flight_turn() {
        let scratch = Scratch::new("cancel-glue");
        let started = Arc::new(AtomicBool::new(false));
        let provider = CancelAwareBlockingProvider {
            started: Arc::clone(&started),
        };
        let controller = scratch.controller(provider);
        let recorder = Recorder::new();
        controller
            .prompt("run", recorder.sink())
            .expect("prompt starts");
        wait_until(|| started.load(Ordering::SeqCst));
        assert_eq!(controller.state(), SessionState::Running);

        // `controller.cancel()` must call `abort()` (the shared cancel token)
        // and return without waiting on the worker or the session lock.
        controller.cancel();
        wait_until(|| controller.state() == SessionState::Idle);

        // The turn ended cancelled, and the UI stayed responsive throughout.
        let events = recorder.events();
        let finished = events
            .iter()
            .rev()
            .find(|e| matches!(e, GuiWireEvent::TurnFinished { .. }))
            .expect("turn_finished");
        assert!(matches!(
            finished,
            GuiWireEvent::TurnFinished {
                stop_reason: StopReason::Cancelled
            }
        ));
        // No partial text entered history (the provider streamed nothing).
        let snapshot = controller.get_session().expect("session");
        let texts: Vec<String> = snapshot.messages.iter().map(|m| m.text_content()).collect();
        assert!(texts.iter().all(|t| !t.contains("partial")), "{texts:?}");
    }

    #[test]
    fn dispose_joins_an_in_flight_worker() {
        let scratch = Scratch::new("dispose-in-flight");
        let started = Arc::new(AtomicBool::new(false));
        let provider = CancelAwareBlockingProvider {
            started: Arc::clone(&started),
        };
        let controller = scratch.controller(provider);
        controller
            .prompt("run", Recorder::new().sink())
            .expect("prompt starts");
        wait_until(|| started.load(Ordering::SeqCst));
        assert_eq!(controller.state(), SessionState::Running);

        // dispose() must join the parked worker (its own cancel releases the
        // token-aware provider) and leave the state Disposed — the worker's
        // `finish_worker` must not flip it back to Idle.
        controller.dispose();
        assert_eq!(controller.state(), SessionState::Disposed);
        let err = controller
            .prompt("after dispose", Recorder::new().sink())
            .unwrap_err();
        assert!(err.contains("disposed"), "{err}");
    }

    #[test]
    fn dispose_rejects_prompt_and_joins_worker() {
        let scratch = Scratch::new("dispose");
        let controller = scratch.controller(FakeProvider::text_turn("hi"));
        controller
            .prompt("run", Recorder::new().sink())
            .expect("prompt starts");
        wait_until(|| controller.state() == SessionState::Idle);

        controller.dispose();
        assert_eq!(controller.state(), SessionState::Disposed);
        let err = controller
            .prompt("after dispose", Recorder::new().sink())
            .unwrap_err();
        assert!(err.contains("disposed"), "{err}");
        let err = controller.get_session().unwrap_err();
        assert!(err.contains("disposed"), "{err}");
    }

    #[test]
    fn failed_turn_surfaces_an_error_event_and_returns_to_idle() {
        let scratch = Scratch::new("failed-turn");
        let controller = scratch.controller(ExplodingProvider);
        let recorder = Recorder::new();
        controller
            .prompt("boom", recorder.sink())
            .expect("prompt starts");
        wait_until(|| controller.state() == SessionState::Idle);

        let events = recorder.events();
        let errors: Vec<&GuiWireEvent> = events
            .iter()
            .filter(|e| matches!(e, GuiWireEvent::Error { .. }))
            .collect();
        assert!(!errors.is_empty(), "no error event surfaced: {events:?}");
        // The session stays usable after the failure.
        controller.get_session().expect("session still usable");
    }

    #[test]
    fn settings_change_rebuilds_overrides_on_next_prompt() {
        let scratch = Scratch::new("settings-rebuild");
        let (provider, seen) = fake_provider(
            vec![
                vec![text_delta("a"), done_stop()],
                vec![text_delta("b"), done_stop()],
            ],
            usage(1, 1),
        );
        let controller = scratch.controller(provider);
        controller
            .set_settings(GuiSettings {
                model: Some("model-a".into()),
                base_url: Some("https://a.invalid/v1".into()),
                cache: Some(true),
                api_key: Some("sk-a".into()),
                ..GuiSettings::default()
            })
            .expect("set settings");

        controller
            .prompt("first", Recorder::new().sink())
            .expect("first prompt");
        wait_until(|| controller.state() == SessionState::Idle);
        assert_eq!(seen.lock().expect("seen lock").len(), 1);
        assert_eq!(
            seen.lock().expect("seen lock")[0],
            (
                "https://a.invalid/v1".to_string(),
                "model-a".to_string(),
                true
            )
        );

        // Change the model/base_url — no restart.
        controller
            .set_settings(GuiSettings {
                model: Some("model-b".into()),
                base_url: Some("https://b.invalid/v1".into()),
                cache: Some(false),
                api_key: Some("sk-b".into()),
                ..GuiSettings::default()
            })
            .expect("set settings");

        controller
            .prompt("second", Recorder::new().sink())
            .expect("second prompt");
        wait_until(|| controller.state() == SessionState::Idle);
        let seen = seen.lock().expect("seen lock");
        assert_eq!(seen.len(), 2);
        assert_eq!(
            seen[1],
            (
                "https://b.invalid/v1".to_string(),
                "model-b".to_string(),
                false
            )
        );
    }

    #[test]
    fn resumed_session_keeps_full_history() {
        let scratch = Scratch::new("resume");
        let (provider, _) = fake_provider(vec![vec![text_delta("one"), done_stop()]], usage(1, 1));
        let controller = scratch.controller(provider);
        controller
            .prompt("first turn", Recorder::new().sink())
            .expect("first prompt");
        wait_until(|| controller.state() == SessionState::Idle);
        let first_id = controller.get_session().expect("session").id;

        // A second controller resumes the same id from the shared store.
        let controller2 = scratch.controller(FakeProvider::text_turn("two"));
        controller2.resume_session(&first_id).expect("resume by id");
        let snapshot = controller2.get_session().expect("session");
        assert_eq!(snapshot.id, first_id);
        let texts: Vec<String> = snapshot.messages.iter().map(|m| m.text_content()).collect();
        assert!(
            texts.iter().any(|t| t.contains("first turn")),
            "history lost: {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t.contains("one")),
            "assistant history lost: {texts:?}"
        );
    }

    #[test]
    fn cli_created_sessions_are_listed() {
        let scratch = Scratch::new("cli-shared");
        // A session created through the public SDK (what the CLI thin shell
        // would do) with one completed turn.
        let provider = FakeProvider::text_turn("from cli");
        let (home, cwd) = (scratch.home.clone(), scratch.cwd.clone());
        let options = AgentSessionOptions::new(Some(cwd))
            .with_home(home)
            .with_model("m")
            .with_base_url("https://x.invalid/v1")
            .with_api_key("sk-x")
            .with_provider(Box::new(provider));
        let session = create_agent_session(options).expect("create cli session");
        session.prompt("cli turn").expect("cli turn");
        let cli_id = session.session_id().to_string();
        session.dispose();

        let controller = scratch.controller(FakeProvider::text_turn("x"));
        let list = controller.list_sessions().expect("list");
        assert!(
            list.iter().any(|s| s.id == cli_id),
            "CLI-created session not visible: {list:?}"
        );
        assert!(
            list.iter().all(|s| s.modified > 0 && s.messages > 0),
            "list rows carry time + message counts: {list:?}"
        );
    }

    #[test]
    fn delete_session_removes_log_and_switches_current_to_fresh() {
        let scratch = Scratch::new("delete");
        let controller = scratch.controller(FakeProvider::text_turn("hi"));
        controller
            .prompt("create me", Recorder::new().sink())
            .expect("prompt");
        wait_until(|| controller.state() == SessionState::Idle);
        let current_id = controller.get_session().expect("session").id;
        assert!(!controller.list_sessions().expect("list").is_empty());

        // Deleting the current session disposes it and opens a fresh one.
        controller.delete_session(&current_id).expect("delete");
        let snapshot = controller.get_session().expect("session");
        assert_ne!(snapshot.id, current_id);
        assert!(snapshot.messages.is_empty(), "fresh session is empty");
        assert!(
            controller
                .list_sessions()
                .expect("list")
                .iter()
                .all(|s| s.id != current_id),
            "deleted session still listed"
        );
    }

    #[test]
    fn context_usage_matches_cli_estimator() {
        let scratch = Scratch::new("context");
        let controller = scratch.controller(FakeProvider::text_turn("hi"));
        controller
            .prompt("a message", Recorder::new().sink())
            .expect("prompt");
        wait_until(|| controller.state() == SessionState::Idle);
        let snapshot = controller.get_session().expect("session");
        let window = compaction::estimated_context_window() as u64;
        assert_eq!(snapshot.context.window, window);
        let total = compaction::estimate_total_tokens(&snapshot.messages) as u64;
        let expected = total as f64 / window as f64 * 100.0;
        assert!((snapshot.context.percent - expected).abs() < 1e-6);
    }

    // --- helpers for the tests above ----------------------------------------

    /// Build a `(FakeProvider, shared seen-recorder)` pair.
    fn fake_provider(
        script: Vec<Vec<Delta>>,
        per_call: TokenUsage,
    ) -> (FakeProvider, Arc<Mutex<Vec<(String, String, bool)>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let provider = FakeProvider {
            script,
            per_call,
            calls: 0,
            cancels_on: None,
            seen: Arc::clone(&seen),
        };
        (provider, seen)
    }

    /// A provider that always fails, for the failed-turn test.
    #[derive(Clone)]
    struct ExplodingProvider;

    impl Provider for ExplodingProvider {
        fn chat(
            &mut self,
            _messages: &[Message],
            _tools: &[ToolSpec],
            _config: &ProviderConfig,
            _cancel: &CancelToken,
            _on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
        ) -> Result<Option<TokenUsage>, String> {
            Err("provider exploded".to_string())
        }
    }

    /// The IPC payloads serialize to exactly the JSON keys/types the
    /// frontend `types.ts` declares — a shape drift would break the app at
    /// runtime, so it is locked here instead.
    #[test]
    fn ipc_payloads_serialize_with_frontend_shapes() {
        let scratch = Scratch::new("serialize");
        let controller = scratch.controller(FakeProvider::text_turn("hi"));
        controller.prompt("hello", |_| {}).expect("prompt starts");
        wait_until(|| controller.state() == SessionState::Idle);

        let snapshot = controller.get_session().expect("session");
        let json = serde_json::to_value(&snapshot).expect("snapshot serializes");
        // Metadata keys (the top bar / sidebar read these).
        assert!(json.get("id").and_then(|v| v.as_str()).is_some());
        assert!(json.get("model").and_then(|v| v.as_str()).is_some());
        assert!(json.get("title").is_some());
        assert!(json.get("warning").is_some());
        assert!(json.get("usage").is_some());
        // The context shape: `{percent, window}` (top bar badge).
        let context = json.get("context").expect("context");
        assert!(context.get("percent").and_then(|v| v.as_f64()).is_some());
        assert!(context.get("window").and_then(|v| v.as_u64()).is_some());
        // History: every message is the `AgentMessage` wire shape with a
        // `kind` tag (`llm` / `compact_summary`).
        let messages = json
            .get("messages")
            .and_then(|v| v.as_array())
            .expect("messages");
        assert!(!messages.is_empty());
        for message in messages {
            assert!(message.get("kind").and_then(|v| v.as_str()).is_some());
        }

        // The sidebar rows carry `{id, title, modified, messages}`.
        let list = controller.list_sessions().expect("list");
        let row = serde_json::to_value(&list[0]).expect("row serializes");
        assert!(row.get("id").and_then(|v| v.as_str()).is_some());
        assert!(row.get("title").is_some());
        assert!(row.get("modified").and_then(|v| v.as_u64()).is_some());
        assert!(row.get("messages").and_then(|v| v.as_u64()).is_some());
    }
}
