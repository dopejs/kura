//! Model provider abstraction and concrete provider clients.
//!
//! Mirrors `codex-rs/model-provider`: the core talks to `dyn ModelProvider`
//! and never to HTTP directly, so provider quirks stay in this crate.

mod llm_bridge;
mod openai;
mod provider;

pub use llm_bridge::OpenAiCompatibleProvider;
pub use openai::{Credential, OpenAiCompatibleClient, OpenAiCompatibleImageClient, Sampling};
pub use provider::{
    GeneratedAsset, GenerationModality, GenerationProvider, GenerationRequest, GenerationStatus,
    ModelProvider, Prompt, ProviderError, ResponseEvent, ToolSpec,
};
