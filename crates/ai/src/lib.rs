//! slimcode-ai: unified LLM provider layer.
//!
//! Owns the LLM seam (ADR-0011 D1): the wire `Message` model, `Provider`,
//! `ToolSpec`, `Delta`, `FinishReason` and `CancelToken`, plus the Bailian
//! OpenAI-compatible provider. This crate depends on no other
//! slimcode crate, so adding a provider never requires the agent runtime.

pub mod config;
pub mod llm;
pub mod message;
pub mod provider;
pub mod wire;

pub use config::{BailianConfig, DEFAULT_BASE_URL, DEFAULT_MODEL};
pub use llm::{CancelToken, Delta, FinishReason, Provider, ToolSpec};
pub use message::{Message, Part, Role, ToolCall};
pub use provider::BailianProvider;
pub use wire::TokenUsage;
