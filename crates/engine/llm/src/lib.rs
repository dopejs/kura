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
    ToolCall,
};
// Re-exported because it appears in this crate's public API
// (`CreateDispatchInput::tools`): a consumer should not have to take a second
// dependency to name a type this one hands it.
pub use kura_protocol::ToolSpec;
pub use types::{
    CreateDispatchInput, Dispatch, DispatchStatus, Message, MessageRole, StreamChunk, Usage,
};
