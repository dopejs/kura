//! Behavioral tests ported from `daemon/internal/llm/dispatcher_test.go`,
//! plus prepare-validation coverage for the Go sentinel error paths.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use kura_llm::{
    CancelToken, CreateDispatchInput, Dispatch, DispatchStatus, Dispatcher, FailedDispatch,
    Message, MessageRole, PrepareError, Provider, ProviderError, ProviderRequest, ProviderResponse,
    StreamChunk, StreamEmitter, Usage,
};
use futures::future::BoxFuture;

type CompleteFn =
    Box<dyn Fn(ProviderRequest) -> BoxFuture<'static, Result<ProviderResponse, ProviderError>> + Send + Sync>;
type StreamFn = Box<
    dyn for<'a> Fn(
            ProviderRequest,
            StreamEmitter<'a>,
        ) -> BoxFuture<'a, Result<ProviderResponse, ProviderError>>
        + Send
        + Sync,
>;

struct TestProvider {
    name: &'static str,
    complete: CompleteFn,
    stream: StreamFn,
    complete_calls: AtomicUsize,
    stream_calls: AtomicUsize,
}

impl TestProvider {
    fn new(name: &'static str, complete: CompleteFn, stream: StreamFn) -> Self {
        Self {
            name,
            complete,
            stream,
            complete_calls: AtomicUsize::new(0),
            stream_calls: AtomicUsize::new(0),
        }
    }
}

impl Provider for TestProvider {
    fn name(&self) -> &str {
        self.name
    }

    fn complete<'a>(
        &'a self,
        request: ProviderRequest,
    ) -> BoxFuture<'a, Result<ProviderResponse, ProviderError>> {
        self.complete_calls.fetch_add(1, Ordering::SeqCst);
        (self.complete)(request)
    }

    fn stream<'a>(
        &'a self,
        request: ProviderRequest,
        emit: StreamEmitter<'a>,
    ) -> BoxFuture<'a, Result<ProviderResponse, ProviderError>> {
        self.stream_calls.fetch_add(1, Ordering::SeqCst);
        (self.stream)(request, emit)
    }
}

fn unused_stream() -> StreamFn {
    Box::new(|_request, _emit| Box::pin(async { Err(ProviderError::other("not used")) }))
}

fn unused_complete() -> CompleteFn {
    Box::new(|_request| Box::pin(async { Err(ProviderError::other("not used")) }))
}

fn user_message(content: &str) -> Message {
    Message { role: MessageRole::User, content: content.into() }
}

fn failed(result: Result<Dispatch, FailedDispatch>) -> FailedDispatch {
    match result {
        Ok(dispatch) => panic!("expected failure, got completed dispatch: {dispatch:?}"),
        Err(failure) => failure,
    }
}

#[tokio::test]
async fn dispatches_successfully() {
    let dispatcher = Dispatcher::new();
    let provider = TestProvider::new(
        "test",
        Box::new(|_request| {
            Box::pin(async {
                Ok(ProviderResponse {
                    tool_calls: Vec::new(),
                    output: "done".into(),
                    finish_reason: "stop".into(),
                    usage: Usage { input_tokens: 3, output_tokens: 1, total_tokens: 0 },
                })
            })
        }),
        unused_stream(),
    );
    dispatcher.register_provider(Arc::new(provider));

    let dispatch = dispatcher
        .prepare(
            CreateDispatchInput {
                provider: "test".into(),
                model: "test-model".into(),
                messages: vec![user_message("hello world")],
                ..CreateDispatchInput::default()
            },
            false,
        )
        .unwrap();

    let final_dispatch = dispatcher.dispatch(dispatch, &CancelToken::new()).await.unwrap();
    assert_eq!(final_dispatch.status, DispatchStatus::Completed);
    assert_eq!(final_dispatch.output, "done");
    assert_eq!(final_dispatch.usage.total_tokens, 4);
    assert_eq!(final_dispatch.attempt_count, 1);
    assert!(final_dispatch.started_at.is_some());
    assert!(final_dispatch.completed_at.is_some());
}

#[tokio::test]
async fn retries_retryable_failure() {
    let dispatcher = Dispatcher::new();
    let provider = TestProvider::new(
        "retryable",
        Box::new(|request| {
            Box::pin(async move {
                if request.attempt == 1 {
                    return Err(ProviderError::provider(
                        "upstream_unavailable",
                        "upstream unavailable",
                        true,
                    ));
                }
                Ok(ProviderResponse {
                    tool_calls: Vec::new(),
                    output: "recovered".into(),
                    finish_reason: "stop".into(),
                    usage: Usage { input_tokens: 2, output_tokens: 1, total_tokens: 0 },
                })
            })
        }),
        unused_stream(),
    );
    let provider = Arc::new(provider);
    dispatcher.register_provider(provider.clone());

    let dispatch = dispatcher
        .prepare(
            CreateDispatchInput {
                provider: "retryable".into(),
                model: "test-model".into(),
                messages: vec![user_message("retry me")],
                max_retries: 2,
                ..CreateDispatchInput::default()
            },
            false,
        )
        .unwrap();

    let final_dispatch = dispatcher.dispatch(dispatch, &CancelToken::new()).await.unwrap();
    assert_eq!(final_dispatch.status, DispatchStatus::Completed);
    assert_eq!(final_dispatch.output, "recovered");
    assert_eq!(final_dispatch.attempt_count, 2);
    assert_eq!(provider.complete_calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn times_out_slow_complete() {
    let dispatcher = Dispatcher::new();
    let provider = TestProvider::new(
        "slow",
        Box::new(|_request| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_millis(200)).await;
                Ok(ProviderResponse {
                    tool_calls: Vec::new(), output: "too slow".into(), ..ProviderResponse::default() })
            })
        }),
        unused_stream(),
    );
    dispatcher.register_provider(Arc::new(provider));

    let dispatch = dispatcher
        .prepare(
            CreateDispatchInput {
                provider: "slow".into(),
                model: "test-model".into(),
                messages: vec![user_message("timeout")],
                timeout_ms: 25,
                max_retries: 0,
                ..CreateDispatchInput::default()
            },
            false,
        )
        .unwrap();

    let failure = failed(dispatcher.dispatch(dispatch, &CancelToken::new()).await);
    assert_eq!(failure.dispatch.error_code, "timeout");
    assert_eq!(failure.dispatch.error, "dispatch timed out");
    assert_eq!(failure.dispatch.status, DispatchStatus::Failed);
    assert_eq!(failure.error, ProviderError::Timeout);
}

#[tokio::test]
async fn streams_successfully() {
    let dispatcher = Dispatcher::new();
    let provider = TestProvider::new(
        "stream",
        unused_complete(),
        Box::new(|_request, emit| {
            Box::pin(async move {
                emit(StreamChunk { delta: "hello".into(), ..StreamChunk::default() })?;
                emit(StreamChunk { delta: " world".into(), ..StreamChunk::default() })?;
                Ok(ProviderResponse {
                    tool_calls: Vec::new(),
                    output: "hello world".into(),
                    finish_reason: "stop".into(),
                    usage: Usage { input_tokens: 2, output_tokens: 2, total_tokens: 0 },
                })
            })
        }),
    );
    dispatcher.register_provider(Arc::new(provider));

    let dispatch = dispatcher
        .prepare(
            CreateDispatchInput {
                provider: "stream".into(),
                model: "test-model".into(),
                messages: vec![user_message("hi")],
                ..CreateDispatchInput::default()
            },
            true,
        )
        .unwrap();
    assert!(dispatch.stream);

    let mut chunks = Vec::new();
    let final_dispatch = dispatcher
        .dispatch_stream(dispatch, &CancelToken::new(), &mut |chunk| {
            chunks.push(chunk.output);
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(final_dispatch.status, DispatchStatus::Completed);
    assert_eq!(chunks.join("|"), "hello|hello world");
}

#[tokio::test]
async fn cancels_interrupted_stream() {
    let dispatcher = Dispatcher::new();
    let provider = TestProvider::new(
        "interrupt",
        unused_complete(),
        Box::new(|_request, emit| {
            Box::pin(async move {
                emit(StreamChunk { delta: "partial".into(), ..StreamChunk::default() })?;
                Err(ProviderError::Cancelled)
            })
        }),
    );
    dispatcher.register_provider(Arc::new(provider));

    let dispatch = dispatcher
        .prepare(
            CreateDispatchInput {
                provider: "interrupt".into(),
                model: "test-model".into(),
                messages: vec![user_message("hi")],
                ..CreateDispatchInput::default()
            },
            true,
        )
        .unwrap();

    let failure = failed(
        dispatcher
            .dispatch_stream(dispatch, &CancelToken::new(), &mut |_chunk| {
                Err(ProviderError::Cancelled)
            })
            .await,
    );
    assert_eq!(failure.dispatch.status, DispatchStatus::Cancelled);
    assert_eq!(failure.dispatch.error_code, "cancelled");
    assert_eq!(failure.dispatch.error, "dispatch cancelled");
}

#[tokio::test]
async fn parent_cancellation_wins_over_provider_error() {
    let dispatcher = Dispatcher::new();
    let provider = TestProvider::new(
        "hang",
        Box::new(|_request| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_secs(60)).await;
                Ok(ProviderResponse::default())
            })
        }),
        unused_stream(),
    );
    dispatcher.register_provider(Arc::new(provider));

    let dispatch = dispatcher
        .prepare(
            CreateDispatchInput {
                provider: "hang".into(),
                model: "test-model".to_string(),
                messages: vec![user_message("hi")],
                timeout_ms: 60_000,
                ..CreateDispatchInput::default()
            },
            false,
        )
        .unwrap();

    let cancel = CancelToken::new();
    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancel_clone.cancel();
    });

    let failure = failed(dispatcher.dispatch(dispatch, &cancel).await);
    assert_eq!(failure.dispatch.status, DispatchStatus::Cancelled);
    assert_eq!(failure.dispatch.error_code, "cancelled");
}

#[tokio::test]
async fn marks_partial_failed_after_visible_stream_output() {
    let dispatcher = Dispatcher::new();
    let provider = TestProvider::new(
        "partial-timeout",
        unused_complete(),
        Box::new(|_request, emit| {
            Box::pin(async move {
                emit(StreamChunk { delta: "hello".into(), ..StreamChunk::default() })?;
                Err(ProviderError::provider("idle_timeout", "stream stalled", true))
            })
        }),
    );
    dispatcher.register_provider(Arc::new(provider));

    let dispatch = dispatcher
        .prepare(
            CreateDispatchInput {
                provider: "partial-timeout".into(),
                model: "test-model".into(),
                messages: vec![user_message("hi")],
                ..CreateDispatchInput::default()
            },
            true,
        )
        .unwrap();

    let failure = failed(
        dispatcher.dispatch_stream(dispatch, &CancelToken::new(), &mut |_chunk| Ok(())).await,
    );
    assert_eq!(failure.dispatch.status, DispatchStatus::PartialFailed);
    assert!(failure.dispatch.partial);
    assert_eq!(failure.dispatch.output, "hello");
    assert_eq!(failure.dispatch.error_code, "idle_timeout");
    assert_eq!(failure.dispatch.error, "stream stalled");
}

#[test]
fn prepare_requires_provider_model_and_messages() {
    let dispatcher = Dispatcher::new();
    dispatcher.set_default_model("default-model");

    assert_eq!(
        dispatcher
            .prepare(
                CreateDispatchInput {
                    messages: vec![user_message("hi")],
                    ..CreateDispatchInput::default()
                },
                false,
            )
            .unwrap_err(),
        PrepareError::ProviderRequired
    );

    assert_eq!(
        dispatcher
            .prepare(
                CreateDispatchInput {
                    provider: "echo".into(),
                    messages: vec![user_message("hi")],
                    ..CreateDispatchInput::default()
                },
                false,
            )
            .unwrap()
            .model,
        "default-model",
        "default model applies when input model is blank"
    );

    dispatcher.set_default_model("");
    assert_eq!(
        dispatcher
            .prepare(
                CreateDispatchInput {
                    provider: "echo".into(),
                    messages: vec![user_message("hi")],
                    ..CreateDispatchInput::default()
                },
                false,
            )
            .unwrap_err(),
        PrepareError::ModelRequired
    );

    let base = || CreateDispatchInput {
        provider: "echo".into(),
        model: "m".into(),
        ..CreateDispatchInput::default()
    };
    assert_eq!(
        dispatcher.prepare(base(), false).unwrap_err(),
        PrepareError::MessagesRequired,
        "no messages"
    );
    let mut blank = base();
    blank.messages = vec![user_message("   ")];
    assert_eq!(
        dispatcher.prepare(blank, false).unwrap_err(),
        PrepareError::MessagesRequired,
        "blank message content"
    );

    let mut unknown = base();
    unknown.messages = vec![user_message("hi")];
    unknown.provider = "missing".into();
    assert_eq!(
        dispatcher.prepare(unknown, false).unwrap_err(),
        PrepareError::ProviderNotFound("missing".into())
    );
}

#[test]
fn prepare_applies_default_timeouts_and_retries() {
    let dispatcher = Dispatcher::new();
    dispatcher.set_default_retries(3);
    dispatcher.set_default_timeout(Duration::from_millis(1_500));
    // Ignored, like the Go setter ignores non-positive timeouts.
    dispatcher.set_default_timeout(Duration::ZERO);

    let dispatch = dispatcher
        .prepare(
            CreateDispatchInput {
                provider: "echo".into(),
                model: "m".into(),
                messages: vec![user_message("hi")],
                ..CreateDispatchInput::default()
            },
            false,
        )
        .unwrap();
    assert_eq!(dispatch.status, DispatchStatus::Queued);
    assert_eq!(dispatch.timeout_ms, 1_500);
    assert_eq!(dispatch.max_retries, 3);
    assert!(!dispatch.dispatch_id.is_empty());

    // Negative input retries clamp to zero, then fall back to the default.
    let dispatch = dispatcher
        .prepare(
            CreateDispatchInput {
                provider: "echo".into(),
                model: "m".into(),
                messages: vec![user_message("hi")],
                max_retries: -2,
                ..CreateDispatchInput::default()
            },
            false,
        )
        .unwrap();
    assert_eq!(dispatch.max_retries, 3);
}

#[test]
fn set_default_provider_validates_and_clears() {
    let dispatcher = Dispatcher::new();
    assert_eq!(
        dispatcher.set_default_provider("missing").unwrap_err(),
        PrepareError::ProviderNotFound("missing".into())
    );
    dispatcher.set_default_provider("echo").unwrap();
    assert!(dispatcher.has_provider(" echo "));
    // Clearing removes the default again.
    dispatcher.set_default_provider("  ").unwrap();
    assert_eq!(
        dispatcher
            .prepare(
                CreateDispatchInput {
                    model: "m".into(),
                    messages: vec![user_message("hi")],
                    ..CreateDispatchInput::default()
                },
                false,
            )
            .unwrap_err(),
        PrepareError::ProviderRequired
    );
}

#[test]
fn dispatch_unknown_provider_fails_prepared_dispatch() {
    let dispatcher = Dispatcher::new();
    let mut dispatch = dispatcher
        .prepare(
            CreateDispatchInput {
                provider: "echo".into(),
                model: "m".into(),
                messages: vec![user_message("hi")],
                ..CreateDispatchInput::default()
            },
            false,
        )
        .unwrap();
    dispatch.provider = "ghost".into();

    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let failure = runtime.block_on(async {
        failed(dispatcher.dispatch(dispatch, &CancelToken::new()).await)
    });
    assert_eq!(failure.dispatch.status, DispatchStatus::Failed);
    assert_eq!(failure.dispatch.error_code, "provider_not_found");
    assert_eq!(failure.dispatch.error, "provider not found: ghost");
    assert!(failure.dispatch.completed_at.is_some());
}

#[tokio::test]
async fn echo_provider_round_trips_through_dispatcher() {
    let dispatcher = Dispatcher::new();
    let dispatch = dispatcher
        .prepare(
            CreateDispatchInput {
                provider: "echo".into(),
                model: "echo-1".into(),
                messages: vec![user_message("hello there world")],
                ..CreateDispatchInput::default()
            },
            false,
        )
        .unwrap();

    let final_dispatch = dispatcher.dispatch(dispatch, &CancelToken::new()).await.unwrap();
    assert_eq!(final_dispatch.status, DispatchStatus::Completed);
    assert_eq!(final_dispatch.output, "hello there world");
    assert_eq!(final_dispatch.finish_reason, "stop");
    assert_eq!(final_dispatch.usage.input_tokens, 3);
    assert_eq!(final_dispatch.usage.output_tokens, 3);
    assert_eq!(final_dispatch.usage.total_tokens, 6);
}
