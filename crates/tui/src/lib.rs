//! slimcode-tui: the interactive full-screen terminal frontend (ADR-0003),
//! built on ratatui + crossterm + tui-textarea.
//!
//! `slimcode-tui` is a terminal **library**, entered by the CLI (ADR-0013):
//! the CLI owns the process (raw mode, the alternate screen, the terminal
//! title, the exit code) and the application lifecycle, and implements
//! [`handler::UiHandler`]; this crate owns the pure [`app::App`] state machine
//! and the frame loop ([`run::run`]) that keeps input, drawing and one turn's
//! worker thread in step.
//!
//! It has no other `slimcode-*` dependency. The application layer's
//! `DisplayItem`s reach it as this crate's own [`render::RenderItem`]s, the
//! `/` completion pool arrives through [`handler::CompletionProvider`], and the
//! pure core renders through ratatui's `TestBackend` in tests.

pub mod app;
pub mod footer;
pub mod git;
pub mod handler;
pub mod markdown;
pub mod render;
pub mod run;
pub mod text;
pub mod theme;
pub mod toolcall;

pub use run::run;

/// slimcode's version, shown in the TUI startup header block.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
