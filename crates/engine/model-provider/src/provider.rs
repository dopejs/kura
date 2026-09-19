use futures::stream::BoxStream;
use kura_protocol::ResponseItem;
use serde::Deserialize;
use serde::Serialize;

/// Everything needed for one model invocation: instructions, conversation
/// history, and the tools the model may call.
#[derive(Debug, Clone, Default)]
pub struct Prompt {
    pub instructions: Option<String>,
    pub input: Vec<ResponseItem>,
    pub tools: Vec<ToolSpec>,
}

/// A tool definition handed to the model (JSON Schema parameters).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Incremental model output. Function calls are emitted once complete;
/// providers that stream call fragments must accumulate before emitting.
#[derive(Debug, Clone, PartialEq)]
pub enum ResponseEvent {
    OutputTextDelta(String),
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    Completed,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("provider returned status {status}: {body}")]
    Status { status: u16, body: String },
    #[error("malformed stream: {0}")]
    Malformed(String),
}

pub trait ModelProvider: Send + Sync {
    fn stream<'a>(
        &'a self,
        prompt: &'a Prompt,
    ) -> BoxStream<'a, Result<ResponseEvent, ProviderError>>;
}
