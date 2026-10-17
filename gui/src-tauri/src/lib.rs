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

    tauri::Builder::default()
        .manage(controller)
        .invoke_handler(tauri::generate_handler![
            commands::new_session,
            commands::resume_session,
            commands::delete_session,
            commands::list_sessions,
            commands::get_session,
            commands::prompt,
            commands::cancel,
            commands::compact_now,
            commands::get_settings,
            commands::set_settings,
        ])
        .run(tauri::generate_context!())
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
}
