//! slimcode-tui: the interactive full-screen terminal frontend (ADR-0003),
//! built on ratatui + crossterm + tui-textarea.
//!
//! The crate is split into a **pure `App` core** ([`app`]) — a testable state
//! machine with a draw-to-frame function and an on-key reducer — and a thin
//! crossterm/ratatui terminal loop that wraps it ([`terminal`]). The pure core
//! renders through ratatui's `TestBackend` in tests, with no real terminal
//! required. Its display vocabulary is its own ([`render::RenderItem`]): the
//! CLI converts the application layer's `DisplayItem`s into `RenderItem`s
//! (ADR-0014).

pub mod app;
pub mod footer;
pub mod git;
pub mod markdown;
pub mod render;
pub mod terminal;
pub mod text;
pub mod theme;
pub mod toolcall;

/// slimcode's version, shown in the TUI startup header block.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
