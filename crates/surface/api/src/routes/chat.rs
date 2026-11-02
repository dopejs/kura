//! Chat query route family (Go handleChatQuery / handleChatQueryStream).
//!
//! Port of the core chat endpoints in daemon/internal/api/server.go:
//! - `POST /v1/chat/query` — one-shot chat query through `kura_chat::Service::query`.
//! - `POST /v1/chat/query/stream` — SSE streaming through
//!   `kura_chat::Service::stream_channel` (the crate's std::thread + std mpsc
//!   variant of the sync callback emitter) bridged onto an axum SSE response
//!   via a pump thread and a tokio mpsc channel.
//!
//! Status-code mapping mirrors the Go helpers: `llmPrepareStatusCode` for
//! prepare/validation failures (400 for bad input, 500 otherwise), and
//! `llmDispatchStatusCode` for dispatch execution failures (504 timeout family,
//! 400 provider_not_found, 408 cancelled, 502 otherwise). Only POST is
//! registered, so axum answers other methods with 405 like the Go method guard.

use std::convert::Infallible;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event as SseEvent, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use kura_chat::{
    CancellationToken, ChatError, QueryExecution, QueryInput, QueryResult, SteerError, StreamChunk,
};
use kura_llm::{Dispatch, DispatchStatus};
use kura_threads::ContinuityStatus;
use serde::Deserialize;
use tokio_stream::wrappers::ReceiverStream;

use crate::error::ApiError;
use crate::middleware::{ActingTenant, RequestContext, TenantContext};
use crate::response::Json;
use crate::state::AppState;
use crate::types::{
    ChatQueryResponse, ChatQueryStreamDelta, ChatQueryStreamStarted, ChatTurnCancelResponse,
    ChatTurnMessageResponse,
};

/// Go `chatQueryRequest` (`daemon/internal/api/server.go`). Field names map
/// through `rename_all = "camelCase"`: timeoutMs, maxRetries, threadId,
/// continuity.mode.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatQueryRequest {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub skills: Vec<String>,
    pub query: String,
    #[serde(default)]
    pub timeout_ms: i64,
    #[serde(default)]
    pub max_retries: i64,
    #[serde(default)]
    pub thread_id: String,
    #[serde(default)]
    pub continuity: Option<ContinuityRequest>,
    /// The reply must be JSON conforming to a schema (2.8, G7).
    #[serde(default)]
    pub response_format: Option<ResponseFormatRequest>,
}

/// `responseFormat` on the wire. `type` names the kind of constraint;
/// `json_schema` is the only one.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseFormatRequest {
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    pub schema: serde_json::Value,
}

/// Go `chatQueryRequest.Continuity` (mode string).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinuityRequest {
    #[serde(default)]
    pub mode: String,
}

/// Route family router. Only POST is registered (Go's handlers reject other
/// methods with 405; axum answers the unregistered methods with 405).
#[must_use]
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/chat/query", post(handle_chat_query))
        .route("/v1/chat/query/stream", post(handle_chat_query_stream))
        .route(
            "/v1/chat/turns/{request_id}/messages",
            post(handle_chat_turn_message),
        )
        .route(
            "/v1/chat/turns/{request_id}/cancel",
            post(handle_chat_turn_cancel),
        )
}

/// Body of `POST /v1/chat/turns/{requestId}/messages`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatTurnMessageRequest {
    pub message: String,
}

/// POST /v1/chat/turns/{requestId}/messages — says more to a running
/// non-streamed query (agent-runtime-program.md 2.8, G8). The turn is the
/// one whose query was sent with `X-Request-Id: {requestId}`, by the
/// caller's tenant; the model reads the message before its next call.
///
/// - 202 with how many messages the turn holds unread.
/// - 400 `invalid_steer_message`: empty, or over 32 KiB.
/// - 404 `turn_not_found`: no such turn is running for this tenant.
/// - 409 `turn_finished`: the turn has answered or failed.
/// - 429 `turn_inbox_full`: it already holds 8 messages it has not read.
pub async fn handle_chat_turn_message(
    State(state): State<AppState>,
    tenant: ActingTenant,
    Path(request_id): Path<String>,
    body: Bytes,
) -> Response {
    let Some(chat_service) = state.chat.as_deref() else {
        return ApiError::Internal("chat service is not configured".to_string()).into_response();
    };
    let request: ChatTurnMessageRequest = match decode_json_body(&body) {
        Ok(request) => request,
        Err(err) => return err.into_response(),
    };
    match chat_service.steer_turn(tenant.id(), &request_id, &request.message) {
        Ok(pending) => (
            StatusCode::ACCEPTED,
            Json(ChatTurnMessageResponse {
                request_id: request_id.trim().to_string(),
                pending,
            }),
        )
            .into_response(),
        Err(err) => steer_error(&err),
    }
}

/// POST /v1/chat/turns/{requestId}/cancel — stops a running non-streamed
/// query. The query itself then answers 408 with a cancelled dispatch.
/// 202 when the turn was told to stop; 404 and 409 as for messages.
pub async fn handle_chat_turn_cancel(
    State(state): State<AppState>,
    tenant: ActingTenant,
    Path(request_id): Path<String>,
) -> Response {
    let Some(chat_service) = state.chat.as_deref() else {
        return ApiError::Internal("chat service is not configured".to_string()).into_response();
    };
    match chat_service.cancel_turn(tenant.id(), &request_id) {
        Ok(()) => (
            StatusCode::ACCEPTED,
            Json(ChatTurnCancelResponse {
                request_id: request_id.trim().to_string(),
            }),
        )
            .into_response(),
        Err(err) => steer_error(&err),
    }
}

/// The `ApiError` body shape, with the specific code: a client tells a
/// finished turn from a full one by it.
fn steer_error(err: &SteerError) -> Response {
    let status = match err {
        SteerError::NotFound(_) => StatusCode::NOT_FOUND,
        SteerError::Finished => StatusCode::CONFLICT,
        SteerError::Full => StatusCode::TOO_MANY_REQUESTS,
        SteerError::Invalid(_) => StatusCode::BAD_REQUEST,
    };
    let message = err.to_string();
    (
        status,
        Json(serde_json::json!({
            "code": err.code(),
            "message": message,
            "error": message,
        })),
    )
        .into_response()
}

/// POST /v1/chat/query — one-shot chat query (Go handleChatQuery).
///
/// - 500 `chat service is not configured` when `AppState.chat` is absent.
/// - 400 on a malformed/empty body.
/// - Prepare failures (`Err`) map through `llmPrepareStatusCode`.
/// - Dispatch execution failures (a result with `exec_error`) map through
///   `llmDispatchStatusCode` and carry the built response.
/// - Success is 200 with the built response.
#[allow(clippy::unused_async)]
pub async fn handle_chat_query(
    State(state): State<AppState>,
    tenant: ActingTenant,
    correlation: Option<Extension<RequestContext>>,
    body: Bytes,
) -> Response {
    let Some(chat_service) = state.chat.as_deref() else {
        return ApiError::Internal("chat service is not configured".to_string()).into_response();
    };
    let request: ChatQueryRequest = match decode_json_body(&body) {
        Ok(request) => request,
        Err(err) => return err.into_response(),
    };
    if let Err(err) = check_response_format(&request, false) {
        return err.into_response();
    }
    let mut input = query_input_from_request(&request, tenant.context());
    input.request_id = request_id(correlation.as_ref());
    // The chat service bridges into the async dispatcher through a per-call
    // current-thread Tokio runtime, which cannot be created on an axum worker
    // thread (tokio forbids entering a runtime inside a runtime). Run the
    // blocking query off the worker like Go's synchronous handler would.
    let service = chat_service.clone();
    let cancel = CancellationToken::new();
    let execution = match tokio::task::spawn_blocking(move || service.query(input, &cancel)).await {
        Ok(execution) => execution,
        Err(err) => {
            return ApiError::Internal(format!("chat query task failed: {err}")).into_response();
        }
    };
    match execution {
        Ok(execution) => {
            let status = if execution.exec_error.is_some() {
                llm_dispatch_status_code(&execution.result.dispatch)
            } else {
                StatusCode::OK
            };
            (status, Json(build_chat_query_response(&execution.result))).into_response()
        }
        Err(err) => llm_prepare_error(&err).into_response(),
    }
}

/// POST /v1/chat/query/stream — SSE chat stream (Go handleChatQueryStream).
///
/// The sync `kura_chat::Service::stream` runs on a blocking task; its emit
/// callback forwards each chunk as `chat.query.started` (first chunk) and
/// `chat.query.delta` frames over a tokio mpsc channel, and the terminal
/// `chat.query.completed|failed|cancelled|partial_failed` frame (with the
/// dispatch id, mirroring Go) closes the stream.
#[allow(clippy::unused_async)]
pub async fn handle_chat_query_stream(
    State(state): State<AppState>,
    tenant: ActingTenant,
    correlation: Option<Extension<RequestContext>>,
    body: Bytes,
) -> Response {
    let Some(chat_service) = state.chat.clone() else {
        return ApiError::Internal("chat service is not configured".to_string()).into_response();
    };
    let request: ChatQueryRequest = match decode_json_body(&body) {
        Ok(request) => request,
        Err(err) => return err.into_response(),
    };
    if let Err(err) = check_response_format(&request, true) {
        return err.into_response();
    }
    let mut input = query_input_from_request(&request, tenant.context());
    input.request_id = request_id(correlation.as_ref());

    let query_text = input.query.trim().to_string();
    // The chat service runs its own per-call current-thread Tokio runtime, so
    // it cannot run on an axum worker; `stream_channel` already runs the full
    // pipeline on a dedicated std::thread with a std mpsc chunk channel (the
    // emit callback runs inside the service's own runtime, where tokio's
    // blocking_send is forbidden). A pump thread forwards the chunk frames as
    // SSE events onto the tokio channel the response stream reads from.
    // Admitted before the response starts, so a refusal is a 429 and not a
    // 200 whose stream fails. Admission may wait for a slot, so it runs off
    // the worker.
    let admit = {
        let service = chat_service.clone();
        let tenant_id = input.tenant_id.clone();
        tokio::task::spawn_blocking(move || service.admit(&tenant_id)).await
    };
    let admitted = match admit {
        Ok(Ok(admitted)) => admitted,
        Ok(Err(err)) => return llm_prepare_error(&err).into_response(),
        Err(err) => {
            return ApiError::Internal(format!("chat admission task failed: {err}"))
                .into_response();
        }
    };
    let (chunks, handle) =
        match chat_service.stream_channel_admitted(admitted, input, CancellationToken::new(), 32) {
            Ok(pair) => pair,
            Err(err) => return llm_prepare_error(&err).into_response(),
        };
    let (sender, receiver) = tokio::sync::mpsc::channel::<Result<SseEvent, Infallible>>(32);
    std::thread::spawn(move || {
        pump_stream(chunks, handle, sender, query_text);
    });
    Sse::new(ReceiverStream::new(receiver)).into_response()
}

/// The `reply` a delta frame carries: the current round's text so far. A
/// streamed turn runs the agent loop, so a new dispatch id is a new round and
/// the text starts over; what came before a tool call is interim, as in a
/// query, whose answer is the last round's.
#[derive(Default)]
struct RoundReply {
    dispatch_id: String,
    text: String,
}

impl RoundReply {
    fn push(&mut self, dispatch_id: &str, delta: &str) -> &str {
        if dispatch_id != self.dispatch_id {
            self.dispatch_id = dispatch_id.to_string();
            self.text.clear();
        }
        self.text.push_str(delta);
        &self.text
    }
}

/// Pump thread: reads stream chunks off the std mpsc channel (the
/// `stream_channel` thread's emit side) and forwards SSE frames onto the
/// tokio channel the response stream reads from. Mirrors Go's handler flow:
/// started on the first chunk (or once after the stream closes when no chunk
/// arrived but a dispatch exists), then a delta per chunk, then the terminal
/// event named from the dispatch status.
fn pump_stream(
    chunks: std::sync::mpsc::Receiver<StreamChunk>,
    handle: std::thread::JoinHandle<Result<QueryExecution, ChatError>>,
    sender: tokio::sync::mpsc::Sender<Result<SseEvent, Infallible>>,
    query_text: String,
) {
    let mut started = false;
    let mut round = RoundReply::default();
    while let Ok(chunk) = chunks.recv() {
        if !started {
            started = true;
            let event = SseEvent::default()
                .event("chat.query.started")
                .json_data(started_from_chunk(&chunk, &query_text))
                .unwrap_or_else(|err| {
                    SseEvent::default()
                        .event("chat.query.started")
                        .data(err.to_string())
                });
            if sender.blocking_send(Ok(event)).is_err() {
                return;
            }
        }
        let reply = round.push(&chunk.dispatch_id, &chunk.delta).to_string();
        let event = SseEvent::default()
            .event("chat.query.delta")
            .json_data(ChatQueryStreamDelta {
                dispatch_id: chunk.dispatch_id.clone(),
                delta: chunk.delta.clone(),
                reply,
            })
            .unwrap_or_else(|err| {
                SseEvent::default()
                    .event("chat.query.delta")
                    .data(err.to_string())
            });
        if sender.blocking_send(Ok(event)).is_err() {
            return;
        }
    }

    // Chunk channel closed: the stream thread settled the execution.
    let (result, exec_error) = match handle.join() {
        Ok(Ok(execution)) => (execution.result, execution.exec_error.is_some()),
        Ok(Err(_)) => (QueryResult::default(), true),
        Err(_) => (QueryResult::default(), true),
    };

    // Go: when no chunk arrived (e.g. an immediate failure) but a dispatch was
    // prepared, emit the started frame from the settled result.
    if !started && !result.dispatch.dispatch_id.is_empty() {
        let event = SseEvent::default()
            .event("chat.query.started")
            .json_data(started_from_result(&result, &query_text))
            .unwrap_or_else(|err| {
                SseEvent::default()
                    .event("chat.query.started")
                    .data(err.to_string())
            });
        if sender.blocking_send(Ok(event)).is_err() {
            return;
        }
    }

    let terminal_name = chat_query_terminal_event(exec_error, &result.dispatch);
    let mut terminal = SseEvent::default().event(terminal_name);
    if !result.dispatch.dispatch_id.is_empty() {
        terminal = terminal.id(result.dispatch.dispatch_id.clone());
    }
    let terminal = terminal
        .json_data(build_chat_query_response(&result))
        .unwrap_or_else(|err| {
            SseEvent::default()
                .event("chat.query.failed")
                .data(err.to_string())
        });
    let _ = sender.blocking_send(Ok(terminal));
}

/// Go's first-chunk started frame (built from the chunk's identity fields).
fn started_from_chunk(chunk: &StreamChunk, query: &str) -> ChatQueryStreamStarted {
    ChatQueryStreamStarted {
        dispatch_id: chunk.dispatch_id.clone(),
        provider: chunk.provider.clone(),
        model: chunk.model.clone(),
        skills: chunk.skills.clone(),
        skill_contracts: chunk.skill_contracts.clone(),
        query: query.to_string(),
        thread_id: chunk.thread_id.clone(),
        session_segment_id: chunk.session_segment_id.clone(),
        request_turn_id: chunk.request_turn_id.clone(),
        continuity_preview_id: chunk.continuity_preview_id.clone(),
        continuity_applied: optional_bool(chunk.continuity_applied, !chunk.thread_id.is_empty()),
        continuity_status: continuity_status_str(chunk.continuity_status).to_string(),
    }
}

/// Go's post-stream started frame (built from the settled result).
fn started_from_result(result: &QueryResult, query: &str) -> ChatQueryStreamStarted {
    ChatQueryStreamStarted {
        dispatch_id: result.dispatch.dispatch_id.clone(),
        provider: result.dispatch.provider.clone(),
        model: result.dispatch.model.clone(),
        skills: result.skills.clone(),
        skill_contracts: result.skill_contracts.clone(),
        query: query.to_string(),
        thread_id: result.thread_id.clone(),
        session_segment_id: result.session_segment_id.clone(),
        request_turn_id: result.request_turn_id.clone(),
        continuity_preview_id: result.continuity_preview_id.clone(),
        continuity_applied: optional_bool(result.continuity_applied, !result.thread_id.is_empty()),
        continuity_status: continuity_status_str(result.continuity_status).to_string(),
    }
}

// ---------------------------------------------------------------------------
// Helpers (Go decodeJSONBody / buildChatQueryResponse / optionalBool /
// llmPrepareStatusCode / llmDispatchStatusCode mappings)
// ---------------------------------------------------------------------------

/// Go `decodeJSONBody` (empty body -> "request body is required", parse errors
/// surfaced verbatim); both map to 400.
fn decode_json_body<T: serde::de::DeserializeOwned>(body: &Bytes) -> Result<T, ApiError> {
    if body.is_empty() {
        return Err(ApiError::BadRequest("request body is required".to_string()));
    }
    serde_json::from_slice(body).map_err(|err| ApiError::BadRequest(err.to_string()))
}

/// The request id the access log and the response header carry, so the
/// turn's trace can be found from either.
fn request_id(correlation: Option<&Extension<RequestContext>>) -> String {
    correlation
        .map(|extension| extension.0.0.lock().request_id.clone())
        .unwrap_or_default()
}

/// Maps the wire request onto `kura_chat::QueryInput`, trimming strings and
/// taking the tenant id from the resolved `TenantContext` extension when
/// present (Go `tenantContextFromContext`).
/// What the wire shape alone can refuse about `responseFormat`; the
/// schema itself is checked by the chat service.
fn check_response_format(request: &ChatQueryRequest, streamed: bool) -> Result<(), ApiError> {
    let Some(format) = &request.response_format else {
        return Ok(());
    };
    if streamed {
        // Refused before the stream opens, so it is a status, not a
        // failure event on a stream that already said 200.
        return Err(ApiError::BadRequest(
            "responseFormat is not available on a streamed query; use /v1/chat/query".to_string(),
        ));
    }
    if format.kind != "json_schema" {
        return Err(ApiError::BadRequest(format!(
            "responseFormat.type must be json_schema, not {:?}",
            format.kind
        )));
    }
    Ok(())
}

fn query_input_from_request(
    request: &ChatQueryRequest,
    tenant: Option<&TenantContext>,
) -> QueryInput {
    QueryInput {
        query: request.query.trim().to_string(),
        provider: request.provider.trim().to_string(),
        model: request.model.trim().to_string(),
        skills: request.skills.clone(),
        timeout_ms: request.timeout_ms,
        max_retries: request.max_retries,
        tenant_id: tenant
            .map(|context| context.0.tenant_id.trim().to_string())
            .unwrap_or_default(),
        thread_id: request.thread_id.trim().to_string(),
        continuity_mode: request.continuity.as_ref().map(|continuity| {
            let mode = continuity.mode.trim();
            if mode == "disabled" {
                kura_threads::ContinuityMode::Disabled
            } else {
                kura_threads::ContinuityMode::Auto
            }
        }),
        response_format: request
            .response_format
            .as_ref()
            .map(|format| kura_llm::ResponseFormat {
                name: format.name.trim().to_string(),
                schema: format.schema.clone(),
            }),
        ..QueryInput::default()
    }
}

/// Go `buildChatQueryResponse`: maps the `QueryResult` (plus its `Dispatch`)
/// onto the wire `ChatQueryResponse`. The continuity/thread fields are only
/// populated when a thread id exists, with `continuity_applied`/counts as
/// optional values.
fn build_chat_query_response(result: &QueryResult) -> ChatQueryResponse {
    let mut response = ChatQueryResponse {
        turn_id: result.turn_id.clone(),
        dispatch_id: result.dispatch.dispatch_id.clone(),
        provider: result.dispatch.provider.clone(),
        model: result.dispatch.model.clone(),
        skills: result.skills.clone(),
        skill_contracts: result.skill_contracts.clone(),
        query: result.query.trim().to_string(),
        status: dispatch_status_str(&result.dispatch.status).to_string(),
        partial: result.dispatch.partial,
        reply: result.dispatch.output.clone(),
        finish_reason: result.dispatch.finish_reason.clone(),
        usage: result.dispatch.usage,
        error_code: result.dispatch.error_code.clone(),
        error: result.dispatch.error.clone(),
        thread_id: String::new(),
        session_segment_id: String::new(),
        request_turn_id: String::new(),
        response_turn_id: String::new(),
        continuity_preview_id: String::new(),
        continuity_applied: None,
        continuity_status: String::new(),
        continuity_included_count: None,
        continuity_excluded_count: None,
        structured_output: result.structured_output.clone(),
    };
    if !result.thread_id.is_empty() {
        response.thread_id = result.thread_id.clone();
        response.session_segment_id = result.session_segment_id.clone();
        response.request_turn_id = result.request_turn_id.clone();
        response.response_turn_id = result.response_turn_id.clone();
        response.continuity_preview_id = result.continuity_preview_id.clone();
        response.continuity_applied = Some(result.continuity_applied);
        response.continuity_status = continuity_status_str(result.continuity_status).to_string();
        response.continuity_included_count = Some(result.continuity_included_count);
        response.continuity_excluded_count = Some(result.continuity_excluded_count);
    }
    response
}

/// Go `optionalBool` / `optionalInt`: `None` when the include guard is false.
fn optional_bool(value: bool, include: bool) -> Option<bool> {
    if include { Some(value) } else { None }
}

/// Go `string(llm.DispatchStatus)`.
fn dispatch_status_str(status: &DispatchStatus) -> &'static str {
    match status {
        DispatchStatus::Queued => "queued",
        DispatchStatus::Running => "running",
        DispatchStatus::Completed => "completed",
        DispatchStatus::PartialFailed => "partial_failed",
        DispatchStatus::Failed => "failed",
        DispatchStatus::Cancelled => "cancelled",
    }
}

/// Go `string(threads.ContinuityStatus)` with the empty string for `None`
/// (Go's zero-value continuity status).
fn continuity_status_str(status: Option<ContinuityStatus>) -> &'static str {
    match status {
        None => "",
        Some(ContinuityStatus::Applied) => "applied",
        Some(ContinuityStatus::Empty) => "empty",
        Some(ContinuityStatus::Disabled) => "disabled",
        Some(ContinuityStatus::Blocked) => "blocked",
        Some(ContinuityStatus::Partial) => "partial",
        Some(ContinuityStatus::Failed) => "failed",
    }
}

/// Go `llmPrepareStatusCode`: prepare/validation failures before a dispatch
/// exists. 400 for bad input (missing provider/model/messages, unknown skill,
/// missing query), 500 for everything else.
fn llm_prepare_status_code(err: &ChatError) -> StatusCode {
    match err {
        ChatError::QueryRequired => StatusCode::BAD_REQUEST,
        ChatError::InvalidResponseFormat(_) => StatusCode::BAD_REQUEST,
        ChatError::Prepare(_) => StatusCode::BAD_REQUEST,
        ChatError::Skills(message) if message.starts_with("skill not found") => {
            StatusCode::BAD_REQUEST
        }
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// ApiError for a prepare failure, carrying the Go `llmPrepareStatusCode`.
/// A plugin hook veto (pluginization phase 2) is a policy decision, not a
/// server fault: 403.
fn llm_prepare_error(err: &ChatError) -> ApiError {
    if let ChatError::Throttled(throttled) = err {
        return ApiError::TooManyRequests {
            code: throttled.reason.code(),
            message: err.to_string(),
            retry_after_secs: throttled.retry_after.as_secs_f64().ceil() as u64,
        };
    }
    if matches!(err, ChatError::HookVetoed { .. }) {
        return ApiError::Forbidden(err.to_string());
    }
    // The request was sound; the model's reply was not.
    if matches!(err, ChatError::StructuredOutputInvalid(_)) {
        return ApiError::BadGateway(err.to_string());
    }
    if llm_prepare_status_code(err) == StatusCode::BAD_REQUEST {
        ApiError::BadRequest(err.to_string())
    } else {
        ApiError::Internal(err.to_string())
    }
}

/// Go `llmDispatchStatusCode`: dispatch execution failures keyed off the
/// settled dispatch's error code.
fn llm_dispatch_status_code(dispatch: &Dispatch) -> StatusCode {
    match dispatch.error_code.as_str() {
        "timeout"
        | "connect_timeout"
        | "first_chunk_timeout"
        | "idle_timeout"
        | "max_duration_exceeded" => StatusCode::GATEWAY_TIMEOUT,
        "provider_not_found" => StatusCode::BAD_REQUEST,
        "cancelled" => StatusCode::REQUEST_TIMEOUT,
        _ => StatusCode::BAD_GATEWAY,
    }
}

/// Go `handleChatQueryStream` terminal event name: an exec error or a failed
/// dispatch names `chat.query.failed`; cancelled and partial_failed statuses
/// override it (matching the Go if-chain order).
fn chat_query_terminal_event(exec_error: bool, dispatch: &Dispatch) -> &'static str {
    match dispatch.status {
        DispatchStatus::PartialFailed => "chat.query.partial_failed",
        DispatchStatus::Cancelled => "chat.query.cancelled",
        DispatchStatus::Failed => "chat.query.failed",
        _ if exec_error => "chat.query.failed",
        _ => "chat.query.completed",
    }
}

// ---------------------------------------------------------------------------
// Tests (axum Router::oneshot over a test chat service)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use axum::body::Body;
    use axum::body::to_bytes;
    use axum::http::Request as HttpRequest;
    use kura_chat::Service as ChatService;
    use kura_events::Bus;
    use kura_llm::Dispatcher;
    use kura_store::SQLiteStore;
    use parking_lot::Mutex;
    use tower::ServiceExt;
    use uuid::Uuid;

    fn test_config() -> kura_config::Config {
        kura_config::Config {
            project_root: String::new(),
            store: Default::default(),
            environment: kura_config::Environment::Test,
            bind_addr: "127.0.0.1:19192".to_string(),
            data_dir: "/tmp/kura-api-test".to_string(),
            log_level: "info".to_string(),
            version: "0.1.0".to_string(),
            llm: kura_config::LlmConfig::default(),
            connectors: kura_config::ConnectorConfig {
                discord: kura_config::DiscordConnectorConfig {
                    enabled: false,
                    ..Default::default()
                },
                telegram: kura_config::TelegramConnectorConfig {
                    enabled: false,
                    ..Default::default()
                },
                slack: kura_config::SlackConnectorConfig {
                    enabled: false,
                    ..Default::default()
                },
                matrix: kura_config::MatrixConnectorConfig {
                    enabled: false,
                    ..Default::default()
                },
            },
            egress: Default::default(),
        }
    }

    fn new_store() -> Arc<Mutex<SQLiteStore>> {
        let dir = std::env::temp_dir().join(format!("kura-api-chat-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        Arc::new(Mutex::new(
            SQLiteStore::new(dir.to_str().expect("path")).expect("store"),
        ))
    }

    /// A chat service wired to the default echo dispatcher (Go's EchoProvider)
    /// with no store/skills: the query/stream happy path needs nothing else.
    fn chat_service() -> Arc<ChatService> {
        Arc::new(ChatService::new_service(
            Arc::new(Dispatcher::new()),
            None,
            None,
            Some(Bus::new()),
            None,
        ))
    }

    fn state_with(chat: Option<Arc<ChatService>>) -> AppState {
        let mut state = AppState::new(test_config(), Arc::new(Bus::new()), new_store());
        state.chat = chat;
        state
    }

    fn request(method: &str, uri: &str, body: Option<&str>) -> HttpRequest<Body> {
        HttpRequest::builder()
            .method(method)
            .uri(uri)
            .body(match body {
                Some(body) => Body::from(body.to_string()),
                None => Body::empty(),
            })
            .expect("request")
    }

    #[tokio::test]
    async fn chat_query_happy_path_returns_built_response() {
        let app = crate::routes::router(state_with(Some(chat_service())));
        let response = app
            .oneshot(request(
                "POST",
                "/v1/chat/query",
                Some(r#"{"provider":"echo","model":"echo-model","query":"hello world"}"#),
            ))
            .await
            .expect("oneshot");
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json body");
        assert_eq!(json["provider"], "echo");
        assert_eq!(json["model"], "echo-model");
        assert_eq!(json["query"], "hello world");
        assert_eq!(json["status"], "completed");
        assert_eq!(json["reply"], "hello world");
        assert_eq!(json["finishReason"], "stop");
        assert!(!json["dispatchId"].as_str().unwrap().is_empty());
        assert_eq!(json["usage"]["totalTokens"], 4);
    }

    #[tokio::test]
    async fn chat_query_returns_500_when_service_is_not_configured() {
        let app = crate::routes::router(state_with(None));
        let response = app
            .oneshot(request(
                "POST",
                "/v1/chat/query",
                Some(r#"{"query":"hello"}"#),
            ))
            .await
            .expect("oneshot");
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json body");
        assert_eq!(json["error"], "chat service is not configured");
    }

    async fn post_query(body: &str, path: &str) -> (StatusCode, serde_json::Value) {
        let app = crate::routes::router(state_with(Some(chat_service())));
        let response = app
            .oneshot(request("POST", path, Some(body)))
            .await
            .expect("oneshot");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        (status, serde_json::from_slice(&bytes).unwrap_or_default())
    }

    fn structured_body(query: &str, kind: &str, schema: serde_json::Value) -> String {
        serde_json::json!({
            "provider": "echo",
            "model": "echo-model",
            "query": query,
            "responseFormat": {"type": kind, "name": "point", "schema": schema},
        })
        .to_string()
    }

    fn point_schema() -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {"x": {"type": "integer"}},
            "required": ["x"]
        })
    }

    /// Answers every request with the same text.
    struct Fixed(&'static str);

    impl kura_llm::Provider for Fixed {
        fn name(&self) -> &str {
            "fixed"
        }

        fn complete<'a>(
            &'a self,
            _request: kura_llm::ProviderRequest,
        ) -> futures::future::BoxFuture<
            'a,
            Result<kura_llm::ProviderResponse, kura_llm::ProviderError>,
        > {
            let output = self.0.to_string();
            Box::pin(async move {
                Ok(kura_llm::ProviderResponse {
                    output,
                    finish_reason: "stop".to_string(),
                    ..Default::default()
                })
            })
        }

        fn stream<'a>(
            &'a self,
            request: kura_llm::ProviderRequest,
            _emit: kura_llm::StreamEmitter<'a>,
        ) -> futures::future::BoxFuture<
            'a,
            Result<kura_llm::ProviderResponse, kura_llm::ProviderError>,
        > {
            self.complete(request)
        }
    }

    #[tokio::test]
    async fn chat_query_returns_structured_output_for_a_conforming_reply() {
        let dispatcher = Dispatcher::new();
        dispatcher.register_provider(Arc::new(Fixed(r#"{"x": 3}"#)));
        let chat = Arc::new(ChatService::new_service(
            Arc::new(dispatcher),
            None,
            None,
            Some(Bus::new()),
            None,
        ));
        let app = crate::routes::router(state_with(Some(chat)));
        let body = structured_body("where?", "json_schema", point_schema())
            .replace("\"echo\"", "\"fixed\"");
        let response = app
            .oneshot(request("POST", "/v1/chat/query", Some(&body)))
            .await
            .expect("oneshot");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json body");
        assert_eq!(status, StatusCode::OK, "{json}");
        assert_eq!(json["structuredOutput"], serde_json::json!({"x": 3}));
        assert_eq!(json["reply"], r#"{"x": 3}"#);
    }

    #[tokio::test]
    async fn chat_query_answers_502_when_the_reply_never_conforms() {
        let (status, json) = post_query(
            &structured_body("not json", "json_schema", point_schema()),
            "/v1/chat/query",
        )
        .await;
        assert_eq!(status, StatusCode::BAD_GATEWAY, "{json}");
        assert_eq!(json["code"], "bad_gateway");
        assert!(json.get("structuredOutput").is_none());
    }

    #[tokio::test]
    async fn chat_query_refuses_a_response_format_it_cannot_enforce() {
        for (body, path) in [
            (
                structured_body("{}", "json_schema", serde_json::json!({"not": {}})),
                "/v1/chat/query",
            ),
            (
                structured_body("{}", "json_object", point_schema()),
                "/v1/chat/query",
            ),
            (
                structured_body("{}", "json_schema", point_schema()),
                "/v1/chat/query/stream",
            ),
        ] {
            let (status, json) = post_query(&body, path).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{path} {body}: {json}");
        }
    }

    #[tokio::test]
    async fn chat_query_returns_400_on_bad_json() {
        let app = crate::routes::router(state_with(Some(chat_service())));
        let response = app
            .oneshot(request("POST", "/v1/chat/query", Some(r#"{not json"#)))
            .await
            .expect("oneshot");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn chat_query_returns_405_on_non_post() {
        let app = crate::routes::router(state_with(Some(chat_service())));
        let response = app
            .oneshot(request("GET", "/v1/chat/query", None))
            .await
            .expect("oneshot");
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn chat_query_stream_returns_sse_with_delta_and_terminal_events() {
        let app = crate::routes::router(state_with(Some(chat_service())));
        let response = app
            .oneshot(request(
                "POST",
                "/v1/chat/query/stream",
                Some(r#"{"provider":"echo","model":"echo-model","query":"hello world"}"#),
            ))
            .await
            .expect("oneshot");
        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(
            content_type.starts_with("text/event-stream"),
            "expected text/event-stream, got {content_type}"
        );
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body = String::from_utf8(bytes.to_vec()).expect("utf8 body");
        assert!(
            body.contains("event: chat.query.started"),
            "missing started event:\n{body}"
        );
        assert!(
            body.contains("event: chat.query.delta"),
            "missing delta event:\n{body}"
        );
        assert!(
            body.contains("event: chat.query.completed"),
            "missing completed event:\n{body}"
        );
        assert!(
            body.contains("\"reply\":\"hello world\""),
            "unexpected reply:\n{body}"
        );
        assert!(
            body.contains("\"delta\":\"hello\""),
            "unexpected delta:\n{body}"
        );
    }

    #[tokio::test]
    async fn chat_query_stream_returns_500_when_service_is_not_configured() {
        let app = crate::routes::router(state_with(None));
        let response = app
            .oneshot(request(
                "POST",
                "/v1/chat/query/stream",
                Some(r#"{"query":"hello"}"#),
            ))
            .await
            .expect("oneshot");
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    /// Answers with the last user message, but holds its first answer until
    /// `open` is set: the turn is running, and reachable, until then.
    struct GateProvider {
        open: Arc<std::sync::atomic::AtomicBool>,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl kura_llm::Provider for GateProvider {
        fn name(&self) -> &str {
            "gate"
        }

        fn complete<'a>(
            &'a self,
            request: kura_llm::ProviderRequest,
        ) -> futures::future::BoxFuture<
            'a,
            Result<kura_llm::ProviderResponse, kura_llm::ProviderError>,
        > {
            let first = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0;
            Box::pin(async move {
                while first && !self.open.load(std::sync::atomic::Ordering::SeqCst) {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                let last = request
                    .messages
                    .iter()
                    .rev()
                    .find(|message| message.role == kura_llm::MessageRole::User)
                    .map(|message| message.content.clone())
                    .unwrap_or_default();
                Ok(kura_llm::ProviderResponse {
                    output: last,
                    finish_reason: "stop".to_string(),
                    ..kura_llm::ProviderResponse::default()
                })
            })
        }

        fn stream<'a>(
            &'a self,
            request: kura_llm::ProviderRequest,
            _emit: kura_llm::StreamEmitter<'a>,
        ) -> futures::future::BoxFuture<
            'a,
            Result<kura_llm::ProviderResponse, kura_llm::ProviderError>,
        > {
            self.complete(request)
        }
    }

    async fn send(app: &axum::Router, uri: &str, body: &str) -> (StatusCode, serde_json::Value) {
        let response = app
            .clone()
            .oneshot(request("POST", uri, Some(body)))
            .await
            .expect("oneshot");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        (status, serde_json::from_slice(&bytes).unwrap_or_default())
    }

    #[tokio::test]
    async fn a_running_query_reads_a_message_sent_to_its_request_id() {
        let open = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let dispatcher = Arc::new(Dispatcher::new());
        dispatcher.register_provider(Arc::new(GateProvider {
            open: Arc::clone(&open),
            calls: std::sync::atomic::AtomicUsize::new(0),
        }));
        let service = Arc::new(ChatService::new_service(
            dispatcher,
            None,
            None,
            Some(Bus::new()),
            None,
        ));
        let app = crate::routes::router(state_with(Some(service)));

        let query = tokio::spawn({
            let app = app.clone();
            async move {
                let request = HttpRequest::builder()
                    .method("POST")
                    .uri("/v1/chat/query")
                    .header(crate::middleware::REQUEST_ID_HEADER, "req_steer_e2e")
                    .body(Body::from(
                        r#"{"provider":"gate","model":"m","query":"2+2?"}"#,
                    ))
                    .expect("request");
                let response = app.oneshot(request).await.expect("oneshot");
                let status = response.status();
                let bytes = to_bytes(response.into_body(), usize::MAX)
                    .await
                    .expect("body");
                let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
                (status, json)
            }
        });

        // The query registers its turn once it starts running.
        let uri = "/v1/chat/turns/req_steer_e2e/messages";
        let mut accepted = None;
        for _ in 0..400 {
            let (status, json) = send(&app, uri, r#"{"message":"in French"}"#).await;
            if status == StatusCode::ACCEPTED {
                accepted = Some(json);
                break;
            }
            assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        let accepted = accepted.expect("the turn became reachable");
        assert_eq!(accepted["requestId"], "req_steer_e2e");
        assert_eq!(accepted["pending"], 1);
        open.store(true, std::sync::atomic::Ordering::SeqCst);

        let (status, json) = query.await.expect("query task");
        assert_eq!(status, StatusCode::OK, "{json}");
        // The first answer became interim; the reply is to the message.
        assert_eq!(json["reply"], "in French");

        let (status, json) = send(&app, uri, r#"{"message":"late"}"#).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(json["code"], "turn_not_found");
    }

    #[tokio::test]
    async fn reaching_a_turn_that_is_not_running() {
        let app = crate::routes::router(state_with(Some(chat_service())));

        let (status, json) =
            send(&app, "/v1/chat/turns/req_x/messages", r#"{"message":"hi"}"#).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(json["code"], "turn_not_found");

        let (status, json) = send(&app, "/v1/chat/turns/req_x/cancel", "").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(json["code"], "turn_not_found");

        let (status, json) =
            send(&app, "/v1/chat/turns/req_x/messages", r#"{"message":"  "}"#).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json["code"], "invalid_steer_message");

        let (status, _) = send(&app, "/v1/chat/turns/req_x/messages", "").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn steer_errors_keep_their_codes_and_statuses() {
        for (error, status, code) in [
            (SteerError::Finished, StatusCode::CONFLICT, "turn_finished"),
            (
                SteerError::Full,
                StatusCode::TOO_MANY_REQUESTS,
                "turn_inbox_full",
            ),
        ] {
            let response = steer_error(&error);
            assert_eq!(response.status(), status);
            assert_eq!(error.code(), code);
        }
    }

    /// A chat service whose tenants may start one turn a minute.
    fn rate_limited_service() -> Arc<ChatService> {
        let mut service = ChatService::new_service(
            Arc::new(Dispatcher::new()),
            None,
            None,
            Some(Bus::new()),
            None,
        );
        service.set_turn_limits(kura_chat::TurnLimits {
            global_concurrency: 0,
            tenant_concurrency: 0,
            turns_per_minute: 1,
            tokens_per_minute: 0,
            tenant_queue_depth: 0,
            global_queue_depth: 0,
            queue_timeout: std::time::Duration::from_secs(5),
            exempt_tenants: Vec::new(),
        });
        Arc::new(service)
    }

    fn tenant_request(uri: &str, tenant_id: &str) -> HttpRequest<Body> {
        let mut req = request(
            "POST",
            uri,
            Some(r#"{"provider":"echo","model":"echo-model","query":"hello"}"#),
        );
        req.extensions_mut()
            .insert(TenantContext(kura_identity::TenantContext {
                tenant_id: tenant_id.to_string(),
                ..Default::default()
            }));
        req
    }

    #[tokio::test]
    async fn a_turn_over_the_tenant_rate_is_429_with_retry_after() {
        let app = crate::routes::router(state_with(Some(rate_limited_service())));
        let first = app
            .clone()
            .oneshot(tenant_request("/v1/chat/query", "ten_a"))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);

        let second = app
            .clone()
            .oneshot(tenant_request("/v1/chat/query", "ten_a"))
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
        let retry_after: u64 = second.headers()[axum::http::header::RETRY_AFTER]
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!(
            (59..=60).contains(&retry_after),
            "retry after {retry_after}"
        );
        let body = to_bytes(second.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], "turn_rate_limited");

        // Another tenant's rate is its own.
        let other = app
            .oneshot(tenant_request("/v1/chat/query", "ten_b"))
            .await
            .unwrap();
        assert_eq!(other.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn a_refused_stream_is_a_429_and_not_a_failed_stream() {
        let app = crate::routes::router(state_with(Some(rate_limited_service())));
        let first = app
            .clone()
            .oneshot(tenant_request("/v1/chat/query/stream", "ten_a"))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        // Drain it so the turn finishes.
        let _ = to_bytes(first.into_body(), usize::MAX).await.unwrap();

        let second = app
            .oneshot(tenant_request("/v1/chat/query/stream", "ten_a"))
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(
            second
                .headers()
                .contains_key(axum::http::header::RETRY_AFTER)
        );
        let body = to_bytes(second.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], "turn_rate_limited");
    }

    #[test]
    fn a_delta_frames_reply_is_its_rounds_text_and_starts_over_each_round() {
        let mut reply = RoundReply::default();
        assert_eq!(reply.push("d1", "Let me "), "Let me ");
        assert_eq!(reply.push("d1", "check."), "Let me check.");
        assert_eq!(reply.push("d2", "The answer"), "The answer");
        assert_eq!(reply.push("d2", " is 42."), "The answer is 42.");
    }
}
