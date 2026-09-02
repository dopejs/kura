use kura_protocol::ResponseItem;
// Re-exported: the type moved down to the protocol crate so the dispatcher can
// carry it, and every caller here already imports it from this module.
pub use kura_protocol::ToolSpec;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
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

// ---------------------------------------------------------------------------
// Visual generation
// ---------------------------------------------------------------------------

/// A generated modality. Text is served by [`ModelProvider`]; these are the
/// modalities whose call shape does not fit a token stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GenerationModality {
    Image,
    Video,
}

/// One generation request.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GenerationRequest {
    pub model: String,
    pub prompt: String,
    /// Provider-specific knobs (size, duration, seed, ...). Deliberately
    /// opaque: modelling every provider's parameter set here would make the
    /// trait churn on each new backend, and the runtime has no reason to
    /// interpret them.
    #[serde(default)]
    pub options: serde_json::Value,
}

/// Where a finished asset lives. Providers differ, and the difference is not
/// hideable: inline bytes cost memory, a URL may expire before the caller
/// fetches it, so the caller must see which one it got.
#[derive(Debug, Clone, PartialEq)]
pub enum GeneratedAsset {
    Bytes {
        media_type: String,
        data: bytes::Bytes,
    },
    Url {
        media_type: String,
        url: String,
    },
}

/// Progress of a generation.
///
/// Image providers usually answer immediately; video providers usually queue.
/// Both shapes are represented so a caller can treat them uniformly: a
/// synchronous provider simply never returns `Pending`.
#[derive(Debug, Clone, PartialEq)]
pub enum GenerationStatus {
    Pending { job_id: String },
    Ready(Vec<GeneratedAsset>),
    Failed(String),
}

/// Object-safe interface for image and video generation. Futures are boxed per
/// the workspace conventions, matching [`ModelProvider`].
pub trait GenerationProvider: Send + Sync {
    fn modality(&self) -> GenerationModality;

    /// Start a generation. A synchronous provider returns
    /// [`GenerationStatus::Ready`] directly.
    fn generate<'a>(
        &'a self,
        request: &'a GenerationRequest,
    ) -> BoxFuture<'a, Result<GenerationStatus, ProviderError>>;

    /// Poll a queued generation. Providers that never return `Pending` inherit
    /// the default, which reports the job as unknown rather than pretending it
    /// succeeded.
    fn poll<'a>(&'a self, job_id: &'a str) -> BoxFuture<'a, Result<GenerationStatus, ProviderError>> {
        let job_id = job_id.to_string();
        Box::pin(async move {
            Err(ProviderError::Malformed(format!(
                "provider does not queue generations; unknown job {job_id}"
            )))
        })
    }
}
