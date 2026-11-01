//! Agent core: session state, the turn loop, and tool dispatch.
//!
//! Layering mirrors `codex-rs/core`: the core orchestrates model streams and
//! tool calls, emits `kura_protocol::Event`s, and owns conversation history.
//! It has no HTTP, filesystem, or sandbox knowledge of its own.

mod answer;
mod checkpoint;
mod session;
mod steer;
mod tools;

pub use answer::{AnswerCheck, AnswerRejection};
pub use checkpoint::{
    CallState, CheckpointCall, CheckpointRound, INTERRUPTED_CALL_OUTPUT, ResumeStats,
    TurnCheckpoint, TurnJournal,
};
pub use session::{CoreError, DEFAULT_MAX_PARALLEL_TOOL_CALLS, Session, TurnOutcome};
pub use steer::TurnInbox;
pub use tools::{Tool, ToolError, ToolInvocation, ToolOutput, ToolRegistry};
