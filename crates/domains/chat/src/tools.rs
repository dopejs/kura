//! Stage 9.0: the tool seam of the chat loop.
//!
//! The chat service knows nothing about which tools exist. A [`ToolHost`]
//! supplied by the assembly (`kura-app`) answers two questions per turn:
//! which tools this tenant may be offered, and what happens when the model
//! calls one. The service owns the loop, its bound, the persistence of every
//! round as its own dispatch record, the `chat/tool-call` hook point, and the
//! `chat.tool.called` event — so "model-visible = logged" holds for tool
//! traffic exactly as it does for messages.

use kura_llm::{ToolCall, ToolSpec};
use serde::{Deserialize, Serialize};

/// Default bound on tool rounds per turn. After the bound the model is
/// dispatched once more with no tools offered, so it has to answer in text
/// rather than loop forever.
pub const DEFAULT_MAX_TOOL_ROUNDS: usize = 4;

/// Tool output is truncated before it goes back to the model; a tool that
/// returns a megabyte should not be able to blow the context window.
pub const MAX_TOOL_RESULT_CHARS: usize = 16_000;

/// Who is asking, for the host to gate visibility and spend.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolContext {
    pub tenant_id: String,
    pub thread_id: String,
    pub agent_profile_id: String,
    pub dispatch_id: String,
    pub provider: String,
    pub model: String,
}

/// What a tool call produced. `is_error` is reported to the model as such so
/// it can recover (retry with other arguments, or answer without the tool).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolOutcome {
    pub content: String,
    pub is_error: bool,
}

impl ToolOutcome {
    #[must_use]
    pub fn ok(content: impl Into<String>) -> Self {
        ToolOutcome {
            content: content.into(),
            is_error: false,
        }
    }
    #[must_use]
    pub fn error(content: impl Into<String>) -> Self {
        ToolOutcome {
            content: content.into(),
            is_error: true,
        }
    }
}

/// The assembly's answer to "which tools, and what do they do". Calls are
/// synchronous because the chat service is (it bridges into async itself);
/// a host that needs async work builds its own runtime, as the service does.
pub trait ToolHost: Send + Sync {
    /// Tools to offer on this turn. Empty means the turn runs with no tools
    /// and the loop is skipped entirely.
    fn available_tools(&self, ctx: &ToolContext) -> Vec<ToolSpec>;
    /// Executes one call. Must not panic; failures are outcomes.
    fn call(&self, ctx: &ToolContext, call: &ToolCall) -> ToolOutcome;
}

/// One executed tool call, as reported on the query result and the
/// `chat.tool.called` event. `output` is the text the model saw (already
/// truncated).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolTraceEntry {
    pub round: usize,
    pub dispatch_id: String,
    pub call_id: String,
    pub name: String,
    pub arguments: String,
    pub output: String,
    pub is_error: bool,
    pub duration_ms: i64,
}

/// Truncates on a char boundary and marks the cut so the model knows the
/// output is partial.
#[must_use]
pub fn truncate_tool_output(text: &str) -> String {
    if text.chars().count() <= MAX_TOOL_RESULT_CHARS {
        return text.to_string();
    }
    let mut out: String = text.chars().take(MAX_TOOL_RESULT_CHARS).collect();
    out.push_str("\n…[truncated]");
    out
}
