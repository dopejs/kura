//! The dispatcher: provider registry, dispatch preparation/validation, and
//! the attempt loop with timeout, cancellation, retry, and partial-output
//! handling. Port of the `Dispatcher` in `daemon/internal/llm/dispatcher.go`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use parking_lot::RwLock;
use uuid::Uuid;

use crate::echo::EchoProvider;
use crate::provider::{
    CancelToken, Provider, ProviderError, ProviderRequest, ProviderResponse, StreamEmitter,
};
use crate::types::{CreateDispatchInput, Dispatch, DispatchStatus, StreamChunk, Usage};

/// Validation and lookup failures from [`Dispatcher::prepare`]. Messages
/// match the Go sentinel errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PrepareError {
    #[error("provider is required")]
    ProviderRequired,
    #[error("provider not found: {0}")]
    ProviderNotFound(String),
    #[error("model is required")]
    ModelRequired,
    #[error("messages are required")]
    MessagesRequired,
}

/// A settled-but-unsuccessful dispatch: the final dispatch record plus the
/// error that ended it, mirroring Go's `(Dispatch, error)` return pair.
#[derive(Debug, thiserror::Error)]
#[error("{error}")]
pub struct FailedDispatch {
    pub dispatch: Dispatch,
    #[source]
    pub error: ProviderError,
}

pub struct Dispatcher {
    inner: RwLock<Inner>,
}

struct Inner {
    providers: HashMap<String, Arc<dyn Provider>>,
    default_provider: String,
    default_model: String,
    default_timeout: Duration,
    default_retries: i64,
}

impl Default for Dispatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Dispatcher {
    pub fn new() -> Self {
        let dispatcher = Self {
            inner: RwLock::new(Inner {
                providers: HashMap::new(),
                default_provider: String::new(),
                default_model: String::new(),
                default_timeout: Duration::from_secs(30),
                default_retries: 0,
            }),
        };
        dispatcher.register_provider(Arc::new(EchoProvider::new()));
        dispatcher
    }

    pub fn register_provider(&self, provider: Arc<dyn Provider>) {
        let name = provider.name().to_string();
        self.inner.write().providers.insert(name, provider);
    }

    pub fn has_provider(&self, name: &str) -> bool {
        self.inner.read().providers.contains_key(name.trim())
    }

    /// An empty (or whitespace-only) name clears the default.
    pub fn set_default_provider(&self, name: &str) -> Result<(), PrepareError> {
        if name.trim().is_empty() {
            self.inner.write().default_provider.clear();
            return Ok(());
        }
        self.provider(name)?;
        self.inner.write().default_provider = name.to_string();
        Ok(())
    }

    pub fn set_default_model(&self, model: &str) {
        self.inner.write().default_model = model.trim().to_string();
    }

    /// Non-positive timeouts are ignored, matching the Go setter.
    pub fn set_default_timeout(&self, timeout: Duration) {
        if timeout.is_zero() {
            return;
        }
        self.inner.write().default_timeout = timeout;
    }

    /// Negative values clamp to zero, matching the Go setter.
    pub fn set_default_retries(&self, retries: i64) {
        self.inner.write().default_retries = retries.max(0);
    }

    /// Validates the input, applies defaults, and returns a queued dispatch.
    pub fn prepare(&self, input: CreateDispatchInput, stream: bool) -> Result<Dispatch, PrepareError> {
        let inner = self.inner.read();

        let mut provider_name = input.provider.trim().to_string();
        if provider_name.is_empty() {
            provider_name = inner.default_provider.clone();
        }
        if provider_name.is_empty() {
            return Err(PrepareError::ProviderRequired);
        }

        let mut model_name = input.model.trim().to_string();
        if model_name.is_empty() {
            model_name = inner.default_model.clone();
        }
        if model_name.is_empty() {
            return Err(PrepareError::ModelRequired);
        }

        // A message has to say something -- but text is no longer the only way
        // to say it. An assistant turn that only asks to call a tool carries
        // no text, and a tool result carries the id of the call it answers
        // even when the tool returned nothing. This guard predates tool calls
        // and rejected both, so the round after a model asked for a call could
        // not be prepared at all.
        if input.messages.is_empty() || input.messages.iter().any(|message| {
            message.content.trim().is_empty()
                && message.tool_calls.is_empty()
                && message.tool_call_id.is_empty()
        }) {
            return Err(PrepareError::MessagesRequired);
        }

        if !inner.providers.contains_key(&provider_name) {
            return Err(PrepareError::ProviderNotFound(provider_name));
        }

        let timeout_ms = if input.timeout_ms <= 0 {
            inner.default_timeout.as_millis() as i64
        } else {
            input.timeout_ms
        };
        let mut max_retries = input.max_retries.max(0);
        if max_retries == 0 {
            max_retries = inner.default_retries;
        }

        let now = Utc::now();
        Ok(Dispatch {
            dispatch_id: Uuid::new_v4().to_string(),
            provider: provider_name,
            model: model_name,
            messages: input.messages,
            tools: input.tools,
            stream,
            status: DispatchStatus::Queued,
            output: String::new(),
            tool_calls: Vec::new(),
            finish_reason: String::new(),
            usage: Usage::default(),
            error_code: String::new(),
            error: String::new(),
            timeout_ms,
            partial: false,
            max_retries,
            attempt_count: 0,
            created_at: now,
            updated_at: now,
            started_at: None,
            completed_at: None,
        })
    }

    /// Runs a prepared dispatch against its provider's `complete`.
    pub async fn dispatch(
        &self,
        dispatch: Dispatch,
        cancel: &CancelToken,
    ) -> Result<Dispatch, FailedDispatch> {
        let provider = match self.provider(&dispatch.provider) {
            Ok(provider) => provider,
            Err(err) => {
                let dispatch =
                    fail_prepared_dispatch(dispatch, "provider_not_found", &err.to_string());
                return Err(FailedDispatch {
                    dispatch,
                    error: ProviderError::provider("provider_not_found", err.to_string(), false),
                });
            }
        };
        self.execute(dispatch, provider, None, cancel).await
    }

    /// Runs a prepared dispatch against its provider's `stream`, forwarding
    /// chunks to `emit` with the aggregate output backfilled.
    pub async fn dispatch_stream(
        &self,
        dispatch: Dispatch,
        cancel: &CancelToken,
        emit: StreamEmitter<'_>,
    ) -> Result<Dispatch, FailedDispatch> {
        let provider = match self.provider(&dispatch.provider) {
            Ok(provider) => provider,
            Err(err) => {
                let dispatch =
                    fail_prepared_dispatch(dispatch, "provider_not_found", &err.to_string());
                return Err(FailedDispatch {
                    dispatch,
                    error: ProviderError::provider("provider_not_found", err.to_string(), false),
                });
            }
        };
        self.execute(dispatch, provider, Some(emit), cancel).await
    }

    fn provider(&self, name: &str) -> Result<Arc<dyn Provider>, PrepareError> {
        if name.trim().is_empty() {
            return Err(PrepareError::ProviderRequired);
        }
        self.inner
            .read()
            .providers
            .get(name)
            .cloned()
            .ok_or_else(|| PrepareError::ProviderNotFound(name.to_string()))
    }

    async fn execute(
        &self,
        mut dispatch: Dispatch,
        provider: Arc<dyn Provider>,
        emit: Option<StreamEmitter<'_>>,
        cancel: &CancelToken,
    ) -> Result<Dispatch, FailedDispatch> {
        let started_at = Utc::now();
        dispatch.status = DispatchStatus::Running;
        dispatch.started_at = Some(started_at);
        dispatch.updated_at = started_at;
        dispatch.output.clear();
        dispatch.error.clear();
        dispatch.error_code.clear();

        let streaming = emit.is_some();
        let mut emit = emit;
        let max_attempts = (dispatch.max_retries + 1).max(1);

        for attempt in 1..=max_attempts {
            dispatch.attempt_count = attempt;
            dispatch.updated_at = Utc::now();
            dispatch.partial = false;

            let request = ProviderRequest {
                dispatch_id: dispatch.dispatch_id.clone(),
                provider: dispatch.provider.clone(),
                model: dispatch.model.clone(),
                messages: dispatch.messages.clone(),
                tools: dispatch.tools.clone(),
                attempt,
                timeout_ms: dispatch.timeout_ms,
                cancel: cancel.clone(),
                ..ProviderRequest::default()
            };

            // Text streamed so far this attempt; backfilled into each chunk's
            // `output` and used as the fallback output on failure.
            let mut aggregate = String::new();
            let result: Result<ProviderResponse, ProviderError> = if let Some(emit) = emit.as_deref_mut()
            {
                let mut forwarding = |mut chunk: StreamChunk| {
                    aggregate.push_str(&chunk.delta);
                    chunk.output = aggregate.clone();
                    emit(chunk)
                };
                tokio::select! {
                    // Streaming attempts carry no per-attempt deadline, like Go.
                    _ = cancel.wait() => Err(ProviderError::Cancelled),
                    outcome = provider.stream(request, &mut forwarding) => outcome,
                }
            } else {
                let timeout = Duration::from_millis(dispatch.timeout_ms.max(0) as u64);
                tokio::select! {
                    _ = cancel.wait() => Err(ProviderError::Cancelled),
                    outcome = tokio::time::timeout(timeout, provider.complete(request)) => {
                        match outcome {
                            Ok(result) => result,
                            Err(_) => Err(ProviderError::Timeout),
                        }
                    }
                }
            };

            match result {
                Ok(mut response) => {
                    if response.output.is_empty() {
                        response.output = aggregate;
                    }
                    let completed_at = Utc::now();
                    dispatch.status = DispatchStatus::Completed;
                    dispatch.output = response.output;
                    dispatch.tool_calls = response.tool_calls;
                    dispatch.partial = false;
                    dispatch.finish_reason = response.finish_reason;
                    dispatch.usage = normalize_usage(response.usage);
                    dispatch.error.clear();
                    dispatch.error_code.clear();
                    dispatch.updated_at = completed_at;
                    dispatch.completed_at = Some(completed_at);
                    return Ok(dispatch);
                }
                Err(err) => {
                    let outcome = classify_dispatch_error(cancel.is_cancelled(), &err);
                    let partial_output = streaming && !aggregate.trim().is_empty();
                    if outcome.retryable && attempt < max_attempts && !partial_output {
                        continue;
                    }
                    let completed_at = Utc::now();
                    dispatch.status = outcome.status;
                    if partial_output && dispatch.status == DispatchStatus::Failed {
                        dispatch.status = DispatchStatus::PartialFailed;
                        dispatch.partial = true;
                    }
                    dispatch.output = aggregate;
                    dispatch.error_code = outcome.code;
                    dispatch.error = outcome.message;
                    dispatch.finish_reason.clear();
                    dispatch.usage = Usage::default();
                    dispatch.updated_at = completed_at;
                    dispatch.completed_at = Some(completed_at);
                    return Err(FailedDispatch { dispatch, error: err });
                }
            }
        }

        unreachable!("the attempt loop always returns: the final attempt never continues")
    }
}

struct ClassifiedError {
    status: DispatchStatus,
    code: String,
    message: String,
    retryable: bool,
}

/// Mirrors `classifyDispatchError`: caller cancellation wins over any
/// provider-reported error, then the error's own kind decides status, wire
/// code, and retryability.
fn classify_dispatch_error(parent_cancelled: bool, err: &ProviderError) -> ClassifiedError {
    if parent_cancelled || matches!(err, ProviderError::Cancelled) {
        return ClassifiedError {
            status: DispatchStatus::Cancelled,
            code: "cancelled".into(),
            message: "dispatch cancelled".into(),
            retryable: false,
        };
    }
    match err {
        ProviderError::Cancelled => unreachable!("handled above"),
        ProviderError::Timeout => ClassifiedError {
            status: DispatchStatus::Failed,
            code: "timeout".into(),
            message: "dispatch timed out".into(),
            retryable: true,
        },
        ProviderError::Provider { code, message, retryable } => ClassifiedError {
            status: DispatchStatus::Failed,
            code: code.clone(),
            message: if message.is_empty() { code.clone() } else { message.clone() },
            retryable: *retryable,
        },
        ProviderError::Other(message) => ClassifiedError {
            status: DispatchStatus::Failed,
            code: "provider_error".into(),
            message: message.clone(),
            retryable: false,
        },
    }
}

fn fail_prepared_dispatch(mut dispatch: Dispatch, code: &str, message: &str) -> Dispatch {
    let completed_at = Utc::now();
    dispatch.status = DispatchStatus::Failed;
    dispatch.error_code = code.to_string();
    dispatch.error = message.to_string();
    dispatch.updated_at = completed_at;
    dispatch.completed_at = Some(completed_at);
    dispatch
}

fn normalize_usage(mut usage: Usage) -> Usage {
    if usage.total_tokens == 0 {
        usage.total_tokens = usage.input_tokens + usage.output_tokens;
    }
    usage
}
