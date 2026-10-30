use kura_protocol::ResponseItem;
// Re-exported: the type moved down to the protocol crate so the dispatcher can
// carry it, and every caller here already imports it from this module.
use futures::future::BoxFuture;
use futures::stream::BoxStream;
pub use kura_protocol::ToolSpec;
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
    /// Token counts the provider reported for this response. May arrive more
    /// than once (Anthropic reports input at the start and output at the
    /// end); every count is cumulative, so a later value replaces an earlier
    /// one and a zero means "not reported here".
    Usage(TokenUsage),
    Completed,
}

/// Provider-reported token counts, normalized across wires. `input_tokens`
/// is the whole prompt including any cached part; the cache counts are
/// parts of it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_input_tokens: i64,
    pub cache_creation_input_tokens: i64,
}

impl TokenUsage {
    /// Folds a later report into this one: each count takes the larger
    /// value, so a report that omits a count does not erase it.
    pub fn merge(&mut self, later: TokenUsage) {
        self.input_tokens = self.input_tokens.max(later.input_tokens);
        self.output_tokens = self.output_tokens.max(later.output_tokens);
        self.cache_read_input_tokens = self
            .cache_read_input_tokens
            .max(later.cache_read_input_tokens);
        self.cache_creation_input_tokens = self
            .cache_creation_input_tokens
            .max(later.cache_creation_input_tokens);
    }
}

/// Reads a JSON number as a token count; absent or malformed is zero.
pub(crate) fn tokens(value: &serde_json::Value) -> i64 {
    value.as_i64().unwrap_or(0).max(0)
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

    /// Streams with `api_key` in place of the configured credential, for a
    /// dispatch that runs on someone else's key (a tenant's own, on a shared
    /// daemon). `None` means this client cannot, and the caller must refuse
    /// the request: falling back to the configured credential would spend
    /// the operator's account on the tenant's behalf.
    fn stream_with_api_key<'a>(
        &'a self,
        _prompt: &'a Prompt,
        _api_key: &'a str,
    ) -> Option<BoxStream<'a, Result<ResponseEvent, ProviderError>>> {
        None
    }
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
    fn poll<'a>(
        &'a self,
        job_id: &'a str,
    ) -> BoxFuture<'a, Result<GenerationStatus, ProviderError>> {
        let job_id = job_id.to_string();
        Box::pin(async move {
            Err(ProviderError::Malformed(format!(
                "provider does not queue generations; unknown job {job_id}"
            )))
        })
    }
}
