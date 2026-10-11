//! Shared scaffolding for the SDK's integration tests (spec Testing
//! Decisions: everything is driven through the public `AgentSession` API with
//! a scripted `FakeProvider` injected via `AgentSessionOptions.provider`).
//!
//! Each test file was previously self-contained with a byte-identical copy of
//! this module (~150 lines each); the shared copy keeps the behavior-level
//! assertions in the files and the plumbing here. `#![allow(dead_code)]`
//! silences the per-binary "unused" warnings for helpers a given file does
//! not happen to use.

#![allow(dead_code)]

use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use slimcode_app::config::ENV_HOME;
use slimcode_app::render::DisplayItem;
use slimcode_app::session::{SessionStore, project_key};
use slimcode_app::testutil::unique_temp_dir;
use slimcode_app::{
    AgentMessage, CancelToken, Delta, FinishReason, Message, Provider, ProviderConfig, TokenUsage,
    Tool, ToolSpec,
};

use slimcode_sdk::{AgentSession, AgentSessionOptions, SdkEvent, create_agent_session};

/// Serializes tests that mutate the process-global `SLIMCODE_HOME` env var
/// (they run in parallel; without the lock they clobber each other's setup —
/// the same pattern `cli/main.rs` uses for its env-isolated tests).
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Points `SLIMCODE_HOME` at `home` for the lifetime of the guard, restoring
/// the previous value on drop.
pub fn home_guard(home: &Path) -> HomeGuard {
    let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let old = env::var_os(ENV_HOME);
    unsafe { env::set_var(ENV_HOME, home) };
    HomeGuard { _lock: lock, old }
}

pub struct HomeGuard {
    _lock: MutexGuard<'static, ()>,
    old: Option<OsString>,
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        match &self.old {
            Some(v) => unsafe { env::set_var(ENV_HOME, v) },
            None => unsafe { env::remove_var(ENV_HOME) },
        }
    }
}

/// A fresh `home` (slimcode home / session store root) and `cwd` (a git repo,
/// so the project home resolves deterministically to `cwd` itself) for one
/// test.
pub fn scratch(tag: &str) -> (PathBuf, PathBuf) {
    let root = unique_temp_dir(&format!("slimcode-sdk-{tag}"));
    fs::create_dir_all(&root).expect("root");
    let home = root.join("home");
    let cwd = root.join("proj");
    fs::create_dir_all(&home).expect("home");
    fs::create_dir_all(&cwd).expect("cwd");
    fs::create_dir_all(cwd.join(".git")).expect("git marker");
    (home, cwd)
}

/// `scratch` + `SLIMCODE_HOME` pointed at the scratch home, for the whole
/// test.
pub fn test_env(tag: &str) -> (HomeGuard, PathBuf, PathBuf) {
    let (home, cwd) = scratch(tag);
    let guard = home_guard(&home);
    (guard, home, cwd)
}

/// Recursively count the `*.jsonl` session logs under `dir`; a store
/// directory that was never created counts as zero.
pub fn count_jsonl(dir: &Path) -> usize {
    if !dir.exists() {
        return 0;
    }
    let mut n = 0;
    for entry in fs::read_dir(dir).expect("read_dir") {
        let entry = entry.expect("dir entry");
        let ty = entry.file_type().expect("file type");
        n += if ty.is_dir() {
            count_jsonl(&entry.path())
        } else if entry.path().extension().is_some_and(|e| e == "jsonl") {
            1
        } else {
            0
        };
    }
    n
}

/// Build a session through the public API: explicit config to avoid env
/// dependence, cwd under the test's temp dir, and the injected
/// provider/tools.
pub fn make_session(
    cwd: &Path,
    provider: Box<dyn Provider + Send>,
    tools: Vec<Tool>,
) -> AgentSession {
    create_agent_session(
        AgentSessionOptions::new(Some(cwd.to_path_buf()))
            .with_api_key("sk-test")
            .with_base_url("https://example.invalid/v1")
            .with_model("test-model")
            .with_provider(provider)
            .with_tools(tools),
    )
    .expect("create_agent_session")
}

/// The same project-scoped store the SDK builds, so tests locate/replay the
/// persisted log through the same public app API the SDK uses.
pub fn store_for(home: &Path, cwd: &Path) -> SessionStore {
    SessionStore::new(home.join("sessions"), project_key(cwd), cwd)
}

/// A recording event subscriber: collects every event it receives.
#[derive(Clone, Default)]
pub struct Recorder {
    events: Arc<Mutex<Vec<SdkEvent>>>,
}

impl Recorder {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn listener(&self) -> impl Fn(SdkEvent) + Send + Sync + 'static {
        let this = self.clone();
        move |event| this.events.lock().expect("recorder lock").push(event)
    }
    pub fn events(&self) -> Vec<SdkEvent> {
        self.events.lock().expect("recorder lock").clone()
    }
    pub fn displays(&self) -> Vec<DisplayItem> {
        self.events()
            .into_iter()
            .filter_map(|e| match e {
                SdkEvent::Display(item) => Some(item),
                _ => None,
            })
            .collect()
    }
    pub fn message_events(&self) -> Vec<(AgentMessage, Option<TokenUsage>)> {
        self.events()
            .into_iter()
            .filter_map(|e| match e {
                SdkEvent::Message { message, usage } => Some((message, usage)),
                _ => None,
            })
            .collect()
    }
}

/// A scripted provider (the cli/tui + app/runner precedent): each `chat`
/// returns the next delta batch; `fails_on` makes one call error, `cancels_on`
/// cancels the shared token on a 1-based call, and `with_per_call` makes every
/// successful call report a fixed usage. `records_into` captures the message
/// list every call was handed, for history-continuity assertions.
pub struct FakeProvider {
    script: Vec<Vec<Delta>>,
    calls: usize,
    fail_at: Option<usize>,
    cancel_on_call: Option<usize>,
    per_call: TokenUsage,
    seen: Option<Arc<Mutex<Vec<Vec<Message>>>>>,
}

impl FakeProvider {
    pub fn new(script: Vec<Vec<Delta>>) -> Self {
        Self {
            script,
            calls: 0,
            fail_at: None,
            cancel_on_call: None,
            per_call: TokenUsage::default(),
            seen: None,
        }
    }
    pub fn with_per_call(mut self, usage: TokenUsage) -> Self {
        self.per_call = usage;
        self
    }
    pub fn fails_on(mut self, call: usize) -> Self {
        self.fail_at = Some(call);
        self
    }
    pub fn cancels_on(mut self, call: usize) -> Self {
        self.cancel_on_call = Some(call);
        self
    }
    pub fn records_into(mut self, seen: Arc<Mutex<Vec<Vec<Message>>>>) -> Self {
        self.seen = Some(seen);
        self
    }
}

impl Provider for FakeProvider {
    fn chat(
        &mut self,
        messages: &[Message],
        _tools: &[ToolSpec],
        _config: &ProviderConfig,
        cancel: &CancelToken,
        on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
    ) -> Result<Option<TokenUsage>, String> {
        if self.fail_at == Some(self.calls) {
            return Err("provider exploded".to_string());
        }
        if self.cancel_on_call == Some(self.calls + 1) {
            cancel.cancel();
        }
        if let Some(seen) = &self.seen {
            seen.lock().expect("seen lock").push(messages.to_vec());
        }
        let batch = self.script.get(self.calls).cloned().unwrap_or_default();
        self.calls += 1;
        for delta in batch {
            on_delta(delta)?;
        }
        Ok(Some(self.per_call))
    }
}

/// A simple deterministic tool the scripted provider can call.
pub fn probe_tool() -> Tool {
    Tool::new(
        "probe",
        "returns a probe result",
        serde_json::json!({}),
        |_| Ok("probed".to_string()),
    )
}

/// A provider that parks inside `chat` until the test releases it (used to
/// hold a turn open while `abort` / a second `prompt` is exercised).
pub struct BlockingProvider {
    pub started: Arc<Mutex<bool>>,
    pub release: Arc<Mutex<bool>>,
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
        *self.started.lock().expect("started lock") = true;
        loop {
            if *self.release.lock().expect("release lock") {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        on_delta(text("done"))?;
        on_delta(done_stop())?;
        Ok(None)
    }
}

pub fn text(t: &str) -> Delta {
    Delta::Text(t.to_string())
}
pub fn done_stop() -> Delta {
    Delta::Done(FinishReason::Stop)
}
pub fn done_tools() -> Delta {
    Delta::Done(FinishReason::ToolCalls)
}
pub fn tc_start(index: usize, id: &str, name: &str) -> Delta {
    Delta::ToolCallStart {
        index,
        id: id.to_string(),
        name: name.to_string(),
    }
}
pub fn tc_args(index: usize, fragment: &str) -> Delta {
    Delta::ToolCallArgs {
        index,
        fragment: fragment.to_string(),
    }
}
pub fn usage(prompt: u64, completion: u64) -> TokenUsage {
    TokenUsage {
        prompt_tokens: prompt,
        completion_tokens: completion,
        total_tokens: prompt + completion,
        prompt_tokens_details: None,
    }
}

/// Block until `flag` is set (blocking providers set it as soon as `chat`
/// starts).
pub fn wait_until(flag: &Arc<Mutex<bool>>) {
    loop {
        if *flag.lock().expect("flag lock") {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}
