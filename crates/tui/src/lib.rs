//! slimcode-tui: the interactive full-screen terminal frontend (ADR-0003),
//! built on ratatui + crossterm + tui-textarea.
//!
//! The crate is split into a **pure `App` core** ([`app`]) — a testable state
//! machine with a draw-to-frame function and an on-key reducer — and a thin
//! crossterm/ratatui terminal loop that wraps it ([`terminal`]). The pure core
//! renders through ratatui's `TestBackend` in tests, with no real terminal
//! required, and depends on the shared renderer/runner seam in
//! `slimcode-common`.

pub mod app;
pub mod terminal;
