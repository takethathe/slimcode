//! slimcode-ai: unified LLM provider layer.
//!
//! Implements the `Provider` seam from `slimcode-agent` for the Bailian
//! (阿里云百炼) OpenAI-compatible endpoint. Stack chosen in ticket 02; wire
//! behavior verified live in ticket 05's spike.

pub mod config;
pub mod provider;
pub mod wire;

pub use config::BailianConfig;
pub use provider::BailianProvider;
pub use wire::TokenUsage;
