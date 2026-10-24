//! slimcode-gui: the Tauri host behind the GUI frontend (ADR-0024).
//!
//! The Rust process embeds `slimcode-sdk` directly (no FFI, no subprocess,
//! no second turn loop) and bridges `SdkEvent`s to the webview as GUI wire
//! events over a per-prompt Tauri `Channel`. The host logic converges in the
//! pure-Rust [`controller::AppController`] (the test seam); the
//! [`commands`] module is a thin forwarding shell.
//!
//! The crate lives outside the workspace at `gui/src-tauri` with its own
//! gates (`cargo fmt` / clippy / `cargo test`); the workspace gates and the
//! `architecture.rs` dependency matrix stay untouched (ADR-0024 D3).

pub mod commands;
pub mod controller;
pub mod settings;
pub mod wire;

/// The debug-only webview screenshot watcher: built into debug macOS builds
/// only, so release bundles and other platforms carry none of it (see the
/// module docs and `docs/development.md`).
#[cfg(all(debug_assertions, target_os = "macos"))]
pub mod devshot;

use std::path::PathBuf;

use slimcode_app::config;

use controller::AppController;

/// The app's entry: build the Tauri application with the controller as
/// managed state and the command shell registered. Returns the Tauri
/// `Result` so `main` can report startup failures (e.g. an unresolvable
/// slimcode home).
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    // The slimcode home (session repository root + `gui.toml`) pins every
    // session; the working directory is the process cwd, exactly like the
    // CLI's default (ADR-0022 D3).
    let home = config::slimcode_home()
        .ok_or_else(|| "cannot determine the slimcode home directory".to_string())?;
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let controller = AppController::new(home, cwd);

    // Runtime backstop for the "the webview never navigates away" invariant
    // (ADR-0028): the front end cancels link activation on every pointer path,
    // but the webview's CSP is null, so the trusted bound is here. A navigation
    // is allowed only for the app's own origin — any other-scheme or
    // cross-origin target (an `http(s)` page, `file://`, a stray middle-click
    // the DOM missed) is cancelled. Registered as a plugin because Tauri v2
    // exposes `on_navigation` only through the webview/plugin builders, not the
    // app builder, and windows are declared in `tauri.conf.json`.
    let nav_guard = tauri::plugin::Builder::<tauri::Wry>::new("nav-guard")
        .on_navigation(|webview, target| {
            webview
                .url()
                .map(|current| current.origin() == target.origin())
                .unwrap_or(false)
        })
        .build();

    tauri::Builder::default()
        // Link clicks in the transcript open the system browser instead of
        // navigating the webview away from the conversation (ADR-0028).
        .plugin(tauri_plugin_opener::init())
        .plugin(nav_guard)
        .manage(controller)
        .setup(|app| {
            // Start the debug screenshot watcher before the window is drawn;
            // it is a background thread that only reacts to a trigger file.
            #[cfg(all(debug_assertions, target_os = "macos"))]
            devshot::spawn(app.handle().clone());
            // Where the watcher is not compiled in, setup has no work left.
            #[cfg(not(all(debug_assertions, target_os = "macos")))]
            let _ = app;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::new_session,
            commands::resume_session,
            commands::delete_session,
            commands::list_sessions,
            commands::get_session,
            commands::complete,
            commands::prompt,
            commands::cancel,
            commands::compact_now,
            commands::get_settings,
            commands::set_settings,
        ])
        .run(tauri::generate_context!())
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
}
