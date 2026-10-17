//! The slimcode GUI desktop app (ADR-0024): a Tauri window hosting the
//! React frontend, driven by the embedded `slimcode-sdk` (see [`slimcode_gui`]).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Err(message) = slimcode_gui::run() {
        eprintln!("slimcode GUI failed to start: {message}");
        std::process::exit(1);
    }
}
