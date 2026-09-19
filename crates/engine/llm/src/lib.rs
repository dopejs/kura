//! Port of `daemon/internal/llm`: dispatch lifecycle types, the provider
//! registry/dispatch state machine, and the echo test provider.
//!
//! The Go package's `openai_compatible_provider.go` is intentionally not
//! ported: `kura-model-provider`'s `OpenAiCompatibleClient` already
//! implements OpenAI-compatible streaming for the Rust workspace.

mod dispatcher;
mod echo;
mod provider;
mod types;

pub use dispatcher::{Dispatcher, FailedDispatch, PrepareError};
pub use echo::EchoProvider;
pub use provider::{
    CancelToken, Provider, ProviderError, ProviderRequest, ProviderResponse, StreamEmitter,
};
pub use types::{
    CreateDispatchInput, Dispatch, DispatchStatus, Message, MessageRole, StreamChunk, ToolCall,
    ToolSpec, Usage,
};
