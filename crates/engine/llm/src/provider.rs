//! Provider abstraction: the trait dispatchers route through, its
//! request/response payloads, the error taxonomy used for retry/failure
//! classification, and the cancellation token standing in for Go's
//! `context.Context`.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::future::BoxFuture;
use kura_protocol::ToolSpec;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::types::{Message, StreamChunk, Usage};

/// Cancellation signal shared with providers, analogous to `context.Context`
/// in the Go package. Cheap to clone; every clone observes the same state.
#[derive(Clone, Default)]
pub struct CancelToken {
    state: Arc<CancelState>,
}

#[derive(Default)]
struct CancelState {
    cancelled: AtomicBool,
    notify: Notify,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.state.cancelled.store(true, Ordering::SeqCst);
        self.state.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.state.cancelled.load(Ordering::SeqCst)
    }

    /// Resolves once the token is cancelled; returns immediately if it
    /// already is.
    pub async fn wait(&self) {
        loop {
            let notified = self.state.notify.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

impl fmt::Debug for CancelToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CancelToken").field("cancelled", &self.is_cancelled()).finish()
    }
}

/// Errors a provider can fail a request with. Mirrors Go's sentinel errors
/// (`context.Canceled`, `context.DeadlineExceeded`) plus the `ProviderError`
/// struct: the dispatcher classifies each variant into a dispatch status,
/// wire error code, and retryability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    /// The caller (or the emitter) cancelled the dispatch.
    Cancelled,
    /// The per-attempt deadline elapsed. Retryable, like Go's
    /// `context.DeadlineExceeded`.
    Timeout,
    /// A failure reported by the provider itself.
    Provider {
        code: String,
        message: String,
        retryable: bool,
    },
    /// An unclassified failure; reported with code `provider_error`.
    Other(String),
}

impl ProviderError {
    pub fn provider(code: impl Into<String>, message: impl Into<String>, retryable: bool) -> Self {
        Self::Provider { code: code.into(), message: message.into(), retryable }
    }

    pub fn other(message: impl Into<String>) -> Self {
        Self::Other(message.into())
    }

    /// The wire error code the dispatcher records for this failure.
    pub fn code(&self) -> &str {
        match self {
            Self::Cancelled => "cancelled",
            Self::Timeout => "timeout",
            Self::Provider { code, .. } => code,
            Self::Other(_) => "provider_error",
        }
    }

    /// Whether the dispatcher may retry after this failure.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Cancelled => false,
            Self::Timeout => true,
            Self::Provider { retryable, .. } => *retryable,
            Self::Other(_) => false,
        }
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => write!(f, "context canceled"),
            Self::Timeout => write!(f, "context deadline exceeded"),
            // Go's ProviderError.Error(): message if set, otherwise the code.
            Self::Provider { code, message, .. } => {
                if message.is_empty() { write!(f, "{code}") } else { write!(f, "{message}") }
            }
            Self::Other(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ProviderError {}

/// Caller-supplied callback receiving streamed deltas. Mirrors Go's
/// `StreamEmitter`; returning an error aborts the stream.
pub type StreamEmitter<'a> =
    &'a mut (dyn FnMut(StreamChunk) -> Result<(), ProviderError> + Send + 'a);

/// One attempt against a provider. The stream timeout knobs exist for
/// providers that need them (the Go OpenAI-compatible provider); the
/// dispatcher itself does not populate them.
#[derive(Debug, Clone, Default)]
pub struct ProviderRequest {
    pub dispatch_id: String,
    pub provider: String,
    pub model: String,
    pub messages: Vec<Message>,
    /// Tools the model may call. Empty is the ordinary case: a plain chat
    /// request offers none, and a provider that cannot use them ignores them.
    pub tools: Vec<ToolSpec>,
    pub attempt: i64,
    pub timeout_ms: i64,
    pub stream_first_chunk_timeout_ms: i64,
    pub stream_idle_timeout_ms: i64,
    pub stream_max_duration_ms: i64,
    pub cancel: CancelToken,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderResponse {
    pub output: String,
    /// Calls the model asked for, in the order it asked for them.
    ///
    /// Carried rather than discarded. The adapter between an HTTP provider and
    /// this interface used to drop them on the floor -- with a comment saying
    /// so -- which is why a tool-capable model could be configured, told about
    /// tools, and still only ever produce prose.
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: String,
    pub usage: Usage,
}

/// One call the model asked for.
///
/// Serializable because it is persisted with the dispatch: a turn that called
/// a tool cannot be explained afterwards from the text alone.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub call_id: String,
    pub name: String,
    /// JSON, as the model produced it. Not parsed here: whether it satisfies
    /// the tool's schema is the tool's judgement, and a malformed argument is
    /// something the model is told about rather than something that fails the
    /// dispatch.
    pub arguments: String,
}

/// Object-safe provider interface, mirroring Go's `Provider`. Futures are
/// boxed per the workspace conventions.
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;

    fn complete<'a>(
        &'a self,
        request: ProviderRequest,
    ) -> BoxFuture<'a, Result<ProviderResponse, ProviderError>>;

    fn stream<'a>(
        &'a self,
        request: ProviderRequest,
        emit: StreamEmitter<'a>,
    ) -> BoxFuture<'a, Result<ProviderResponse, ProviderError>>;
}
