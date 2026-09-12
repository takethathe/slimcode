//! slimcode-core: agent runtime, tools, and session state.
//!
//! Owns the agent runtime (ADR-0011 D1): the loop and its `AgentEvent`
//! stream, hooks seam, executable `Tool { spec, run }`, and the session
//! message model. It depends only on `slimcode-ai`, which owns the LLM seam
//! (`Provider` / `Message` / `ToolSpec` / `Delta` / `CancelToken`).

pub mod agent;
pub mod session;
pub mod tools;
