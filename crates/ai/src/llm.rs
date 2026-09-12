//! The LLM seam owned by `slimcode-ai` (ADR-0011 D1).
//!
//! `Provider` is the trait a concrete LLM client implements; `Message` (the
//! wire message), `Delta`, `FinishReason`, `ToolSpec` and `CancelToken` are
//! the data types that cross it. A provider sees tools as a schema only —
//! execution lives in `slimcode-core`'s `Tool { spec, run }`. This crate
//! therefore depends on no other slimcode crate, so adding a provider never
//! requires importing the agent runtime.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;

use crate::message::Message;

/// A shared cancellation handle threaded through a run: one `Arc<AtomicBool>`
/// observed by the runner boundaries, the provider's interruptible body read,
/// and cancellable tool executions (bash). Cloning shares the same flag; the
/// caller `reset()`s it at the start of every turn and `cancel()`s it when the
/// user interrupts.
#[derive(Clone, Debug, Default)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
}

impl CancelToken {
    /// A fresh, uncancelled token.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the flag: every boundary and interruptible read notices on its
    /// next check.
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }

    /// Clear the flag (the caller does this before each new turn).
    pub fn reset(&self) {
        self.flag.store(false, Ordering::SeqCst);
    }

    /// Whether a cancel has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
}

/// Why a turn of generation ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FinishReason {
    Stop,
    ToolCalls,
}

/// One streaming delta from the provider — the unit surfaced to the runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Delta {
    /// Thinking tokens (qwen-style `reasoning_content`).
    Reasoning(String),
    /// Assistant text fragment.
    Text(String),
    /// First fragment of a tool call: carries `id` + `name` (and `index`).
    ToolCallStart {
        index: usize,
        id: String,
        name: String,
    },
    /// Subsequent fragment: only an arguments slice (id/name are empty on the
    /// wire for these).
    ToolCallArgs { index: usize, fragment: String },
    /// End-of-turn marker.
    Done(FinishReason),
}

/// A tool as the provider sees it: name, description and JSON-schema
/// parameters. The executable half lives in `slimcode-core::agent::Tool`,
/// which carries a `ToolSpec` plus a run closure.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

impl ToolSpec {
    pub fn new(name: impl Into<String>, description: impl Into<String>, parameters: Value) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
        }
    }
}

/// The provider seam: one turn of generation over `messages`, with `tools`
/// advertised as schemas only.
pub trait Provider {
    /// One turn of generation over `messages` with `tools` available.
    /// Returns the raw delta stream for this turn.
    ///
    /// `cancel` lets an in-flight request interrupt itself: the provider
    /// checks it between body chunks and aborts the read as soon as it is
    /// set (returning an error the runner maps to a silent cancelled stop).
    fn chat(
        &mut self,
        messages: &[Message],
        tools: &[ToolSpec],
        cancel: &CancelToken,
    ) -> Result<Vec<Delta>, String>;
}
