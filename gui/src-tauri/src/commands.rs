//! The Tauri command layer: a thin forwarding shell over the
//! [`AppController`] (spec §Implementation Decisions — the Tauri command
//! surface is a thin forwarding layer carrying no logic). Every command
//! validates nothing beyond what the controller reports; all host logic lives
//! in the pure-Rust controller so it is testable off the Tauri runtime.

use tauri::State;
use tauri::ipc::Channel;

use crate::controller::{AppController, CompletionItem, SessionInfo, SessionSnapshot};
use crate::settings::GuiSettings;
use crate::wire::GuiWireEvent;

/// Open a fresh empty session.
#[tauri::command]
pub fn new_session(state: State<'_, AppController>) -> Result<(), String> {
    state.new_session()
}

/// Resume a session by id from the shared session repository.
#[tauri::command]
pub fn resume_session(state: State<'_, AppController>, id: String) -> Result<(), String> {
    state.resume_session(&id)
}

/// Remove a session's log; the current session is disposed first when it is
/// the one deleted.
#[tauri::command]
pub fn delete_session(state: State<'_, AppController>, id: String) -> Result<(), String> {
    state.delete_session(&id)
}

/// The shared repository's session list (title + time), newest first.
#[tauri::command]
pub fn list_sessions(state: State<'_, AppController>) -> Result<Vec<SessionInfo>, String> {
    state.list_sessions()
}

/// The current session's full snapshot (history / title / model / context
/// usage) for the frontend's (re)load.
#[tauri::command]
pub fn get_session(state: State<'_, AppController>) -> Result<SessionSnapshot, String> {
    state.get_session()
}

/// Ranked `/` completion candidates (Commands + installed Skills, filtered to
/// the GUI-applicable set) for the frontend's Completion popup, fetched per
/// keystroke (ADR-0026).
#[tauri::command]
pub fn complete(state: State<'_, AppController>, input: String) -> Vec<CompletionItem> {
    state.complete(&input)
}

/// Run one whole turn for `text`, streaming `GuiWireEvent`s over `channel`.
#[tauri::command]
pub fn prompt(
    state: State<'_, AppController>,
    text: String,
    channel: Channel<GuiWireEvent>,
) -> Result<(), String> {
    state.prompt(&text, move |event| {
        // Best-effort: a dead webview (window closed) must not abort the turn.
        let _ = channel.send(event);
    })
}

/// Cancel the running turn (`abort()`; kills a running bash child).
#[tauri::command]
pub fn cancel(state: State<'_, AppController>) -> Result<(), String> {
    state.cancel();
    Ok(())
}

/// Force an on-demand compaction, streaming `GuiWireEvent`s over `channel`.
#[tauri::command]
pub fn compact_now(
    state: State<'_, AppController>,
    channel: Channel<GuiWireEvent>,
) -> Result<(), String> {
    state.compact_now(move |event| {
        let _ = channel.send(event);
    })
}

/// The GUI settings (model / base_url / cache / optional api_key).
#[tauri::command]
pub fn get_settings(state: State<'_, AppController>) -> Result<GuiSettings, String> {
    Ok(state.get_settings())
}

/// Persist new GUI settings to `gui.toml` (never `config.toml`).
#[tauri::command]
pub fn set_settings(state: State<'_, AppController>, settings: GuiSettings) -> Result<(), String> {
    state.set_settings(settings)
}
