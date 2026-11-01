//! Agent core: session state, the turn loop, and tool dispatch.
//!
//! Layering mirrors `codex-rs/core`: the core orchestrates model streams and
//! tool calls, emits `kura_protocol::Event`s, and owns conversation history.
//! It has no HTTP, filesystem, or sandbox knowledge of its own.

mod session;
mod tools;

pub use session::{CoreError, DEFAULT_MAX_PARALLEL_TOOL_CALLS, Session, TurnOutcome};
pub use tools::{Tool, ToolError, ToolInvocation, ToolOutput, ToolRegistry};
