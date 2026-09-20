//! llm dispatches route family (port of the /v1/llm/dispatches handlers in Go
//! daemon/internal/api/server.go, Roadmaps 3/13).
//!
//! Routes: GET/POST /v1/llm/dispatches, GET /v1/llm/dispatches/{dispatch_id},
//! POST /v1/llm/dispatches/stream (SSE: `llm.dispatch.started`, a
//! `llm.dispatch.delta` per chunk, then the terminal
//! `llm.dispatch.completed|failed|cancelled|partial_failed` frame).
//!
//! The provider profile resolution (Go resolveProviderDispatchInput) runs
//! through kura_providers when the manager is configured; prepare/validation
//! failures keep the Go llmPrepareStatusCode mapping and settled-but-failed
//! dispatches keep llmDispatchStatusCode (504 timeouts, 400
//! provider_not_found, 408 cancelled, 502 otherwise).
//!
//! Tenant isolation (Roadmap 75): when the protected() middleware resolves a
//! tenant context, listing/get use the tenant-scoped store reads and
//! persistence uses the tenant-safe upsert (Go filterLLMDispatchesByTenant /
//! UpsertLLMDispatchForTenantSafe). Deliberately not ported (documented
//! divergence): the OpenAI-compatible setup-wizard use gate (Go
//! enforceLLMProviderSetupGate), which requires the setup-session
//! dependent-use decision plumbing.

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event as SseEvent, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use std::convert::Infallible;
use tokio_stream::wrappers::ReceiverStream;

use kura_events as events;
use kura_llm as llm;
use kura_providers as providers;

use crate::error::ApiError;
use crate::middleware::{TenantContext, environment_scope_from_config};
use crate::state::AppState;

use super::decode_json_required;

/// Route family router.
#[must_use]
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/v1/llm/dispatches",
            get(list_dispatches).post(create_dispatch),
        )
        .route("/v1/llm/dispatches/stream", post(stream_dispatch))
        .route("/v1/llm/dispatches/{dispatch_id}", get(get_dispatch))
}

#[derive(Debug, Serialize)]
struct DispatchListResponse {
    items: Vec<llm::Dispatch>,
}

fn dispatcher(state: &AppState) -> Result<&llm::Dispatcher, ApiError> {
    state
        .llm
        .as_deref()
        .ok_or_else(|| ApiError::internal("llm dispatcher is not configured"))
}

/// Go llmPrepareStatusCode: validation/lookup failures before a dispatch
/// record exists map to 400; everything else is 500.
fn prepare_error(message: String, bad_request: bool) -> ApiError {
    if bad_request {
        ApiError::BadRequest(message)
    } else {
        ApiError::Internal(message)
    }
}

fn map_prepare_error(err: &llm::PrepareError) -> ApiError {
    // All four PrepareError variants are caller errors in Go's mapping.
    prepare_error(err.to_string(), true)
}

fn map_providers_error(err: &providers::ProvidersError) -> ApiError {
    match err {
        providers::ProvidersError::ModelNotSupported { .. }
        | providers::ProvidersError::ManagedAuthUnsupported => prepare_error(err.to_string(), true),
        providers::ProvidersError::Prepare(prepare) => map_prepare_error(prepare),
        _ => prepare_error(err.to_string(), false),
    }
}

/// Go llmDispatchStatusCode for a settled-but-failed dispatch.
fn dispatch_status_code(dispatch: &llm::Dispatch) -> StatusCode {
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

/// Go llmDispatchTerminalEventName.
fn terminal_event_name(dispatch: &llm::Dispatch) -> &'static str {
    match dispatch.status {
        llm::DispatchStatus::PartialFailed => "llm.dispatch.partial_failed",
        llm::DispatchStatus::Failed => "llm.dispatch.failed",
        llm::DispatchStatus::Cancelled => "llm.dispatch.cancelled",
        _ => "llm.dispatch.completed",
    }
}

/// Go resolveProviderDispatchInput: no-op without a provider manager.
fn resolve_dispatch_input(
    state: &AppState,
    input: llm::CreateDispatchInput,
) -> Result<llm::CreateDispatchInput, ApiError> {
    let Some(manager) = state.providers.as_deref() else {
        return Ok(input);
    };
    manager
        .resolve_dispatch_input(input)
        .map(|(_, effective)| effective)
        .map_err(|err| map_providers_error(&err))
}

fn persist_dispatch(
    state: &AppState,
    tenant_id: &str,
    dispatch: &llm::Dispatch,
) -> Result<(), ApiError> {
    let store = state.store.lock();
    if tenant_id.is_empty() {
        store
            .upsert_llm_dispatch(dispatch)
            .map_err(ApiError::from_store)
    } else {
        store
            .upsert_llm_dispatch_for_tenant_safe(dispatch, tenant_id)
            .map_err(ApiError::from_store)
    }
}

fn tenant_id_from(tenant: &Option<Extension<TenantContext>>) -> String {
    tenant
        .as_ref()
        .map(|extension| extension.0.0.tenant_id.trim().to_string())
        .unwrap_or_default()
}

/// Go publishLLMDispatchRequested / publishLLMDispatchTerminal.
fn publish_dispatch_event(
    state: &AppState,
    name: &str,
    dispatch: &llm::Dispatch,
    terminal: bool,
) -> Result<(), ApiError> {
    let mut payload = serde_json::Map::new();
    payload.insert("provider".to_string(), serde_json::json!(dispatch.provider));
    payload.insert("model".to_string(), serde_json::json!(dispatch.model));
    payload.insert(
        "status".to_string(),
        serde_json::to_value(dispatch.status).unwrap_or(serde_json::Value::Null),
    );
    if terminal {
        payload.insert("partial".to_string(), serde_json::json!(dispatch.partial));
        payload.insert(
            "attemptCount".to_string(),
            serde_json::json!(dispatch.attempt_count),
        );
        payload.insert(
            "finishReason".to_string(),
            serde_json::json!(dispatch.finish_reason),
        );
        payload.insert(
            "usage".to_string(),
            serde_json::to_value(&dispatch.usage).unwrap_or(serde_json::Value::Null),
        );
        payload.insert(
            "errorCode".to_string(),
            serde_json::json!(dispatch.error_code),
        );
        payload.insert("error".to_string(), serde_json::json!(dispatch.error));
    } else {
        payload.insert("stream".to_string(), serde_json::json!(dispatch.stream));
        payload.insert(
            "timeoutMs".to_string(),
            serde_json::json!(dispatch.timeout_ms),
        );
        payload.insert(
            "maxRetries".to_string(),
            serde_json::json!(dispatch.max_retries),
        );
    }

    let event = events::Event {
        category: "llm".to_string(),
        name: name.to_string(),
        environment_scope: environment_scope_from_config(&state.config),
        resource: events::Resource {
            kind: "llm_dispatch".to_string(),
            id: dispatch.dispatch_id.clone(),
        },
        payload,
        ..events::Event::default()
    };
    let stored = state
        .store
        .lock()
        .append_event(&event)
        .map_err(ApiError::from_store)?;
    state.event_bus.publish(stored);
    Ok(())
}

/// GET /v1/llm/dispatches (Go handleLLMDispatches GET branch).
async fn list_dispatches(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Json<DispatchListResponse>, ApiError> {
    dispatcher(&state)?;
    let tenant_id = tenant_id_from(&tenant);
    let store = state.store.lock();
    let items = if tenant_id.is_empty() {
        store.list_llm_dispatches().map_err(ApiError::from_store)?
    } else {
        store
            .list_llm_dispatches_for_tenant_raw(&tenant_id)
            .map_err(ApiError::from_store)?
    };
    Ok(Json(DispatchListResponse { items }))
}

/// GET /v1/llm/dispatches/{dispatch_id} (Go handleLLMDispatchRoutes).
async fn get_dispatch(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(dispatch_id): Path<String>,
) -> Result<Json<llm::Dispatch>, ApiError> {
    let tenant_id = tenant_id_from(&tenant);
    let store = state.store.lock();
    let dispatch = if tenant_id.is_empty() {
        store
            .get_llm_dispatch(dispatch_id.trim())
            .map_err(ApiError::from_store)?
    } else {
        store
            .get_llm_dispatch_for_tenant_raw(&tenant_id, dispatch_id.trim())
            .map_err(ApiError::from_store)?
    };
    dispatch
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("not found".to_string()))
}

/// POST /v1/llm/dispatches (Go handleLLMDispatches POST branch): prepare,
/// persist + publish requested, run the dispatch, persist + publish the
/// terminal event, and answer 201 (success) or the Go dispatch status code.
async fn create_dispatch(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Response {
    let tenant_id = tenant_id_from(&tenant);
    let input: llm::CreateDispatchInput = match decode_json_required(&body) {
        Ok(input) => input,
        Err(err) => return err.into_response(),
    };
    let dispatcher = match dispatcher(&state) {
        Ok(dispatcher) => dispatcher,
        Err(err) => return err.into_response(),
    };
    let input = match resolve_dispatch_input(&state, input) {
        Ok(input) => input,
        Err(err) => return err.into_response(),
    };
    let dispatch = match dispatcher.prepare(input, false) {
        Ok(dispatch) => dispatch,
        Err(err) => return map_prepare_error(&err).into_response(),
    };
    if let Err(err) = persist_dispatch(&state, &tenant_id, &dispatch) {
        return err.into_response();
    }
    if let Err(err) = publish_dispatch_event(&state, "llm.dispatch.requested", &dispatch, false) {
        return err.into_response();
    }

    let cancel = llm::CancelToken::new();
    let (final_dispatch, failed) = match dispatcher.dispatch(dispatch, &cancel).await {
        Ok(final_dispatch) => (final_dispatch, false),
        Err(failure) => (failure.dispatch, true),
    };
    if let Err(err) = persist_dispatch(&state, &tenant_id, &final_dispatch) {
        return err.into_response();
    }
    if let Err(err) = publish_dispatch_event(
        &state,
        terminal_event_name(&final_dispatch),
        &final_dispatch,
        true,
    ) {
        return err.into_response();
    }
    if failed {
        return (dispatch_status_code(&final_dispatch), Json(final_dispatch)).into_response();
    }
    (StatusCode::CREATED, Json(final_dispatch)).into_response()
}

/// POST /v1/llm/dispatches/stream (Go handleLLMDispatchStream): SSE with the
/// started frame, one delta per stream chunk, and the terminal frame (except
/// after an error-cancelled stream, matching Go).
async fn stream_dispatch(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Response {
    let tenant_id = tenant_id_from(&tenant);
    let input: llm::CreateDispatchInput = match decode_json_required(&body) {
        Ok(input) => input,
        Err(err) => return err.into_response(),
    };
    let Some(dispatcher) = state.llm.clone() else {
        return ApiError::Internal("llm dispatcher is not configured".to_string()).into_response();
    };
    let input = match resolve_dispatch_input(&state, input) {
        Ok(input) => input,
        Err(err) => return err.into_response(),
    };
    let dispatch = match dispatcher.prepare(input, true) {
        Ok(dispatch) => dispatch,
        Err(err) => return map_prepare_error(&err).into_response(),
    };
    if let Err(err) = persist_dispatch(&state, &tenant_id, &dispatch) {
        return err.into_response();
    }
    if let Err(err) = publish_dispatch_event(&state, "llm.dispatch.requested", &dispatch, false) {
        return err.into_response();
    }

    let (sender, receiver) = tokio::sync::mpsc::channel::<Result<SseEvent, Infallible>>(32);
    let started = sse_json_event("llm.dispatch.started", &dispatch.dispatch_id, &dispatch);
    if sender.try_send(Ok(started)).is_err() {
        return ApiError::Internal("stream channel closed".to_string()).into_response();
    }

    let stream_state = state.clone();
    let stream_tenant_id = tenant_id.clone();
    tokio::spawn(async move {
        let cancel = llm::CancelToken::new();
        let delta_sender = sender.clone();
        let mut emit = |chunk: llm::StreamChunk| {
            let event = sse_json_event("llm.dispatch.delta", "", &chunk);
            // A closed channel means the client went away; surface it as a
            // provider error so the dispatcher cancels like Go's broken pipe.
            delta_sender.try_send(Ok(event)).map_err(|_| {
                llm::ProviderError::provider("cancelled", "client disconnected".to_string(), false)
            })
        };
        let (final_dispatch, failed) = match dispatcher
            .dispatch_stream(dispatch.clone(), &cancel, &mut emit)
            .await
        {
            Ok(final_dispatch) => (final_dispatch, false),
            Err(failure) => (failure.dispatch, true),
        };
        if persist_dispatch(&stream_state, &stream_tenant_id, &final_dispatch).is_err() {
            return;
        }
        if publish_dispatch_event(
            &stream_state,
            terminal_event_name(&final_dispatch),
            &final_dispatch,
            true,
        )
        .is_err()
        {
            return;
        }
        // Go: skip the terminal frame only for an error-settled cancellation.
        if !failed || final_dispatch.status != llm::DispatchStatus::Cancelled {
            let terminal = sse_json_event(
                terminal_event_name(&final_dispatch),
                &dispatch.dispatch_id,
                &final_dispatch,
            );
            let _ = sender.send(Ok(terminal)).await;
        }
    });

    Sse::new(ReceiverStream::new(receiver)).into_response()
}

/// Go writeSSEEvent: id (when non-empty), event name, JSON data.
fn sse_json_event<T: Serialize>(name: &str, id: &str, payload: &T) -> SseEvent {
    let mut event = SseEvent::default().event(name);
    if !id.is_empty() {
        event = event.id(id);
    }
    event
        .json_data(payload)
        .unwrap_or_else(|err| SseEvent::default().event(name).data(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::super::tests_support::{request_json, test_state};
    use axum::http::StatusCode;
    use std::sync::Arc;

    fn state_with_dispatcher() -> crate::state::AppState {
        // Dispatcher::new() registers the builtin echo provider.
        let dispatcher = kura_llm::Dispatcher::new();
        let _ = dispatcher.set_default_provider("echo");
        dispatcher.set_default_model("echo-1");
        let mut state = test_state();
        state.llm = Some(Arc::new(dispatcher));
        state
    }

    fn dispatch_body() -> serde_json::Value {
        serde_json::json!({
            "provider": "echo",
            "model": "echo-1",
            "messages": [{ "role": "user", "content": "hello" }]
        })
    }

    #[tokio::test]
    async fn create_get_and_list_dispatches() {
        let state = state_with_dispatcher();
        let (status, created) = request_json(
            state.clone(),
            "POST",
            "/v1/llm/dispatches",
            Some(dispatch_body()),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        assert_eq!(created["status"], "completed", "{created}");
        let dispatch_id = created["dispatchId"]
            .as_str()
            .expect("dispatchId")
            .to_string();

        let (status, fetched) = request_json(
            state.clone(),
            "GET",
            &format!("/v1/llm/dispatches/{dispatch_id}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{fetched}");

        let (status, listed) = request_json(state.clone(), "GET", "/v1/llm/dispatches", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(listed["items"].as_array().expect("items").len(), 1);

        let (status, _) = request_json(
            state,
            "GET",
            "/v1/llm/dispatches/llm_dispatch_missing",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn invalid_input_is_400_and_missing_dispatcher_is_500() {
        let state = state_with_dispatcher();
        let (status, body) = request_json(
            state,
            "POST",
            "/v1/llm/dispatches",
            Some(serde_json::json!({ "provider": "", "model": "", "messages": [] })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

        let (status, body) = request_json(test_state(), "GET", "/v1/llm/dispatches", None).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["error"], "llm dispatcher is not configured");
    }
}
