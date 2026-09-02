//! computer_use route family (port of daemon/internal/api/computer_use.go +
//! the `/v1/runs/{run_id}/computer-use*` and `/v1/computer-use/artifacts*`
//! registrations in server.go).
//!
//! Surface (Go parity):
//! - `GET/POST /v1/runs/{run_id}/computer-use/sessions` — session list /
//!   create (Go handleRunComputerUseSessions). Create announces
//!   computer_use.session_created and answers 201; unknown runs 404 (Go
//!   runtime.ErrRunNotFound).
//! - `GET /v1/runs/{run_id}/computer-use/sessions/{session_id}` — session
//!   detail (handleRunComputerUseSessionByID).
//! - `POST /v1/runs/{run_id}/computer-use/sessions/{session_id}/close` —
//!   close (handleRunComputerUseSessionClose), announcing
//!   computer_use.session_status_changed.
//! - `GET/POST /v1/runs/{run_id}/computer-use/sessions/{session_id}/actions`
//!   — action list / create (handleRunComputerUseActions). Create persists
//!   the runtime step/tool-call tracking and any policy approval/decision,
//!   publishes the artifact-recorded / action_requested /
//!   action_status_changed (+ action_target_mismatch) events, and answers 409
//!   with `{action, approval, decision}` when the manager gates the action on
//!   approval, else 201 with the action.
//! - `GET /v1/runs/{run_id}/computer-use/sessions/{session_id}/actions/{action_id}`
//!   — action detail (handleRunComputerUseActionByID).
//! - `GET /v1/computer-use/artifacts/{artifact_id}` +
//!   `.../content` — artifact detail / base64 content
//!   (handleComputerUseArtifactRoutes).
//!
//! The by-id tenant guards ride on the same tables as Go:
//! - run-scoped routes -> runs.run_id (Go withByIDTenantGuard on /v1/runs/),
//! - artifact routes -> computer_use_artifacts.artifact_id.
//!
//! Divergences (documented, not silent):
//! - recordThreadApprovalProjection is skipped: kura-store has no thread
//!   runtime-projection DAO yet (same gap as resources.rs).
//! - The environment filter inside the computer-use manager is exercised via
//!   the manager's store seam; SQLiteStore does not implement
//!   kura_computeruse::Store yet, so route tests wire a test-local
//!   MemStore.

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::{Method, StatusCode, Uri};
use axum::routing::{get, post};
use axum::{Json as AxumJson, Router};
use chrono::{DateTime, Utc};

use kura_computeruse as computeruse;
use kura_events as events;
use kura_runtime as runtime;

use crate::error::ApiError;
use crate::middleware::{
    environment_scope_from_config, guard_resource_for_tenant, AuthenticatedToken, TenantContext,
};
use crate::response::Json;
use crate::state::AppState;
use crate::types::{
    ComputerUseActionListResponse, ComputerUseArtifactContentResponse,
    ComputerUseSessionListResponse, CreateComputerUseActionRequest,
    CreateComputerUseSessionRequest,
};

/// Route family router. Only the methods the Go handlers accept are
/// registered; axum answers the other methods with 405 (Go
/// w.WriteHeader(http.StatusMethodNotAllowed)).
#[must_use]
pub fn router() -> Router<AppState> {
    Router::new()
        // /v1/runs/{run_id}/computer-use/* (Go handleRunRoutes dispatch).
        .route(
            "/v1/runs/{run_id}/computer-use/sessions",
            get(list_run_computer_use_sessions).post(create_run_computer_use_session),
        )
        .route(
            "/v1/runs/{run_id}/computer-use/sessions/{session_id}",
            get(get_run_computer_use_session),
        )
        .route(
            "/v1/runs/{run_id}/computer-use/sessions/{session_id}/close",
            post(close_run_computer_use_session),
        )
        .route(
            "/v1/runs/{run_id}/computer-use/sessions/{session_id}/actions",
            get(list_run_computer_use_actions).post(create_run_computer_use_action),
        )
        .route(
            "/v1/runs/{run_id}/computer-use/sessions/{session_id}/actions/{action_id}",
            get(get_run_computer_use_action),
        )
        // /v1/computer-use/artifacts/ (Go server.go with by-id tenant guard on
        // computer_use_artifacts.artifact_id).
        .route(
            "/v1/computer-use/artifacts/{artifact_id}",
            get(get_computer_use_artifact),
        )
        .route(
            "/v1/computer-use/artifacts/{artifact_id}/content",
            get(get_computer_use_artifact_content),
        )
}

// ---------------------------------------------------------------------------
// Sessions (Go handleRunComputerUseSessions / ByID / Close)
// ---------------------------------------------------------------------------

/// GET /v1/runs/{run_id}/computer-use/sessions — the run's sessions.
async fn list_run_computer_use_sessions(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    method: Method,
    uri: Uri,
    Path(run_id): Path<String>,
) -> Result<Json<ComputerUseSessionListResponse>, ApiError> {
    let manager = computer_use_manager(&state)?;
    guard_run_resource(
        &state,
        &method,
        &uri,
        tenant.as_ref().map(|e| &e.0),
        &run_id,
    )
    .await?;
    let sessions = manager
        .list_sessions(&run_id)
        .map_err(ApiError::from_store)?;
    Ok(Json(ComputerUseSessionListResponse { items: sessions }))
}

/// POST /v1/runs/{run_id}/computer-use/sessions — create a session (Go
/// handleRunComputerUseSessions POST branch). Unknown runs -> 404; other
/// manager failures -> 400.
async fn create_run_computer_use_session(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    method: Method,
    uri: Uri,
    Path(run_id): Path<String>,
    body: Bytes,
) -> Result<(StatusCode, Json<computeruse::Session>), ApiError> {
    let manager = computer_use_manager(&state)?;
    guard_run_resource(
        &state,
        &method,
        &uri,
        tenant.as_ref().map(|e| &e.0),
        &run_id,
    )
    .await?;
    let request: CreateComputerUseSessionRequest = decode_json_body(&body)?;
    let session = manager
        .create_session(
            &run_id,
            &computeruse::CreateSessionInput {
                workflow_id: request.workflow_id.clone(),
                workflow_step_id: request.workflow_step_id.clone(),
                driver_kind: request.driver_kind.clone(),
                initial_url: request.initial_url.clone(),
            },
        )
        .map_err(|err| {
            if err == runtime::RuntimeError::RunNotFound.to_string() {
                ApiError::NotFound("not found".to_string())
            } else {
                ApiError::BadRequest(err)
            }
        })?;
    // Go handleRunComputerUseSessions: computer_use.session_created.
    let payload = serde_json::json!({
        "status": session.status,
        "computerUseSessionId": session.computer_use_session_id,
    });
    publish_event(
        &state,
        tenant.as_ref().map(|e| &e.0),
        events::Event {
            category: "capability".to_string(),
            name: "computer_use.session_created".to_string(),
            scope: events::Scope {
                run_id: run_id.clone(),
                computer_use_session_id: session.computer_use_session_id.clone(),
                ..events::Scope::default()
            },
            resource: events::Resource {
                kind: "computer_use_session".to_string(),
                id: session.computer_use_session_id.clone(),
            },
            payload: payload.as_object().cloned().unwrap_or_default(),
            ..events::Event::default()
        },
    )?;
    Ok((StatusCode::CREATED, Json(session)))
}

/// GET /v1/runs/{run_id}/computer-use/sessions/{session_id} — one session.
async fn get_run_computer_use_session(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    method: Method,
    uri: Uri,
    Path((run_id, session_id)): Path<(String, String)>,
) -> Result<Json<computeruse::Session>, ApiError> {
    let manager = computer_use_manager(&state)?;
    guard_run_resource(
        &state,
        &method,
        &uri,
        tenant.as_ref().map(|e| &e.0),
        &run_id,
    )
    .await?;
    let session = manager
        .get_session(&run_id, &session_id)
        .map_err(ApiError::from_store)?;
    session
        .ok_or_else(|| ApiError::NotFound("not found".to_string()))
        .map(Json)
}

/// POST /v1/runs/{run_id}/computer-use/sessions/{session_id}/close — close a
/// session (Go handleRunComputerUseSessionClose). Unknown sessions -> 404.
async fn close_run_computer_use_session(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    method: Method,
    uri: Uri,
    Path((run_id, session_id)): Path<(String, String)>,
) -> Result<Json<computeruse::Session>, ApiError> {
    let manager = computer_use_manager(&state)?;
    guard_run_resource(
        &state,
        &method,
        &uri,
        tenant.as_ref().map(|e| &e.0),
        &run_id,
    )
    .await?;
    let session = manager.close_session(&run_id, &session_id).map_err(|err| {
        if err == computeruse::ERR_SESSION_NOT_FOUND {
            ApiError::NotFound("not found".to_string())
        } else {
            ApiError::BadRequest(err)
        }
    })?;
    let payload = serde_json::json!({
        "status": session.status,
        "computerUseSessionId": session.computer_use_session_id,
    });
    publish_event(
        &state,
        tenant.as_ref().map(|e| &e.0),
        events::Event {
            category: "capability".to_string(),
            name: "computer_use.session_status_changed".to_string(),
            scope: events::Scope {
                run_id: run_id.clone(),
                computer_use_session_id: session.computer_use_session_id.clone(),
                ..events::Scope::default()
            },
            resource: events::Resource {
                kind: "computer_use_session".to_string(),
                id: session.computer_use_session_id.clone(),
            },
            payload: payload.as_object().cloned().unwrap_or_default(),
            ..events::Event::default()
        },
    )?;
    Ok(Json(session))
}

// ---------------------------------------------------------------------------
// Actions (Go handleRunComputerUseActions / ActionByID)
// ---------------------------------------------------------------------------

/// GET /v1/runs/{run_id}/computer-use/sessions/{session_id}/actions — the
/// session's action history (from the enriched session).
async fn list_run_computer_use_actions(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    method: Method,
    uri: Uri,
    Path((run_id, session_id)): Path<(String, String)>,
) -> Result<Json<ComputerUseActionListResponse>, ApiError> {
    let manager = computer_use_manager(&state)?;
    guard_run_resource(
        &state,
        &method,
        &uri,
        tenant.as_ref().map(|e| &e.0),
        &run_id,
    )
    .await?;
    let session = manager
        .get_session(&run_id, &session_id)
        .map_err(ApiError::from_store)?;
    let session = session.ok_or_else(|| ApiError::NotFound("not found".to_string()))?;
    Ok(Json(ComputerUseActionListResponse {
        items: session.actions,
    }))
}

/// POST /v1/runs/{run_id}/computer-use/sessions/{session_id}/actions —
/// create an action (Go handleRunComputerUseActions POST branch). The
/// manager gates high-risk actions on policy approval internally; this
/// handler persists the approval/decision, publishes the capability events
/// and answers 409 (approval pending) or 201 (executed).
async fn create_run_computer_use_action(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    token: Option<Extension<AuthenticatedToken>>,
    method: Method,
    uri: Uri,
    Path((run_id, session_id)): Path<(String, String)>,
    body: Bytes,
) -> Result<(StatusCode, AxumJson<serde_json::Value>), ApiError> {
    let manager = computer_use_manager(&state)?;
    guard_run_resource(
        &state,
        &method,
        &uri,
        tenant.as_ref().map(|e| &e.0),
        &run_id,
    )
    .await?;
    let request: CreateComputerUseActionRequest = decode_json_body(&body)?;
    let actor = current_actor(token.as_ref().map(|e| &e.0));
    let (result, approval, decision) = manager
        .create_action(
            &run_id,
            &session_id,
            &actor,
            computeruse::CreateActionInput {
                action_kind: request.action_kind,
                url: request.url.clone(),
                value: request.value.clone(),
                selected_value: request.selected_value.clone(),
                wait_ms: request.wait_ms,
                page_target: request.page_target.as_str().to_string(),
                target_match_context: request.target_match_context.clone(),
                rationale: request.rationale.clone(),
            },
        )
        .map_err(|err| {
            if err == computeruse::ERR_SESSION_NOT_FOUND {
                ApiError::NotFound("not found".to_string())
            } else {
                ApiError::BadRequest(err)
            }
        })?;
    let action = result.action;

    // Go persistComputerUseRuntimeTracking: persist the run/step/tool call
    // the manager created, then a run checkpoint.
    persist_computer_use_runtime_tracking(&state, &action)?;

    // Go publishComputerUseArtifacts (best-effort; errors ignored).
    publish_computer_use_artifacts(&state, tenant.as_ref().map(|e| &e.0), &action);
    if action.failure_class == computeruse::FailureClass::TargetMismatch.as_str() {
        publish_computer_use_target_mismatch(&state, tenant.as_ref().map(|e| &e.0), &action)?;
    }

    // Go persistApproval + persistDecision.
    if let Some(approval) = approval.as_ref() {
        state
            .store
            .lock()
            .upsert_approval(approval)
            .map_err(ApiError::from_store)?;
        // Go recordThreadApprovalProjection: skipped — kura-store has no
        // thread runtime-projection DAO yet (documented divergence).
    }
    if let Some(decision) = decision.as_ref() {
        state
            .store
            .lock()
            .upsert_decision(decision)
            .map_err(ApiError::from_store)?;
    }

    // Go computer_use.action_requested.
    publish_event(
        &state,
        tenant.as_ref().map(|e| &e.0),
        action_event("computer_use.action_requested", &action),
    )?;

    if result.pending {
        // Go handleRunComputerUseActions: pending -> 409 {action, approval,
        // decision} (approval/decision present only when non-nil).
        let mut payload = serde_json::Map::new();
        payload.insert(
            "action".to_string(),
            serde_json::to_value(&action).unwrap_or(serde_json::Value::Null),
        );
        if let Some(approval) = approval.as_ref() {
            payload.insert(
                "approval".to_string(),
                serde_json::to_value(approval).unwrap_or(serde_json::Value::Null),
            );
        }
        if let Some(decision) = decision.as_ref() {
            payload.insert(
                "decision".to_string(),
                serde_json::to_value(decision).unwrap_or(serde_json::Value::Null),
            );
        }
        return Ok((
            StatusCode::CONFLICT,
            AxumJson(serde_json::Value::Object(payload)),
        ));
    }

    // Go computer_use.action_status_changed.
    publish_event(
        &state,
        tenant.as_ref().map(|e| &e.0),
        action_event("computer_use.action_status_changed", &action),
    )?;
    Ok((
        StatusCode::CREATED,
        AxumJson(serde_json::to_value(action).unwrap_or(serde_json::Value::Null)),
    ))
}

/// GET /v1/runs/{run_id}/computer-use/sessions/{session_id}/actions/{action_id}
/// — one action (Go handleRunComputerUseActionByID: linear scan of the
/// enriched session's action list).
async fn get_run_computer_use_action(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    method: Method,
    uri: Uri,
    Path((run_id, session_id, action_id)): Path<(String, String, String)>,
) -> Result<Json<computeruse::Action>, ApiError> {
    let manager = computer_use_manager(&state)?;
    guard_run_resource(
        &state,
        &method,
        &uri,
        tenant.as_ref().map(|e| &e.0),
        &run_id,
    )
    .await?;
    let session = manager
        .get_session(&run_id, &session_id)
        .map_err(ApiError::from_store)?;
    let session = session.ok_or_else(|| ApiError::NotFound("not found".to_string()))?;
    for action in &session.actions {
        if action.computer_use_action_id == action_id {
            return Ok(Json(action.clone()));
        }
    }
    Err(ApiError::NotFound("not found".to_string()))
}

// ---------------------------------------------------------------------------
// Artifacts (Go handleComputerUseArtifactRoutes)
// ---------------------------------------------------------------------------

/// GET /v1/computer-use/artifacts/{artifact_id} — artifact detail.
async fn get_computer_use_artifact(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    method: Method,
    uri: Uri,
    Path(artifact_id): Path<String>,
) -> Result<Json<computeruse::Artifact>, ApiError> {
    let manager = computer_use_manager(&state)?;
    guard_artifact_resource(
        &state,
        &method,
        &uri,
        tenant.as_ref().map(|e| &e.0),
        &artifact_id,
    )
    .await?;
    let artifact = manager
        .get_artifact(&artifact_id)
        .map_err(ApiError::from_store)?;
    artifact
        .ok_or_else(|| ApiError::NotFound("not found".to_string()))
        .map(Json)
}

/// GET /v1/computer-use/artifacts/{artifact_id}/content — artifact content as
/// base64 (Go ComputerUseArtifactContentResponse).
async fn get_computer_use_artifact_content(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    method: Method,
    uri: Uri,
    Path(artifact_id): Path<String>,
) -> Result<Json<ComputerUseArtifactContentResponse>, ApiError> {
    let manager = computer_use_manager(&state)?;
    guard_artifact_resource(
        &state,
        &method,
        &uri,
        tenant.as_ref().map(|e| &e.0),
        &artifact_id,
    )
    .await?;
    let (artifact, content, ok) = manager
        .read_artifact_content(&artifact_id)
        .map_err(ApiError::from_store)?;
    if !ok {
        return Err(ApiError::NotFound("not found".to_string()));
    }
    Ok(Json(ComputerUseArtifactContentResponse {
        artifact_id: artifact.artifact_id.clone(),
        mime_type: artifact.mime_type.clone(),
        file_name: artifact.file_name.clone(),
        status: artifact.status.as_str().to_string(),
        content: base64_encode(&content),
    }))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Go handleRunComputerUse* nil-manager branch: 500 with the stable message.
fn computer_use_manager(state: &AppState) -> Result<&computeruse::Manager, ApiError> {
    state
        .computer_use
        .as_deref()
        .ok_or_else(|| ApiError::Internal("computer-use manager is not configured".to_string()))
}

/// Go withByIDTenantGuard on runs.run_id (the /v1/runs/ registration wraps
/// every run-scoped route, including the computer-use family).
async fn guard_run_resource(
    state: &AppState,
    method: &Method,
    uri: &Uri,
    tenant: Option<&TenantContext>,
    run_id: &str,
) -> Result<(), ApiError> {
    let surface = format!("api:{} {}", method.as_str(), uri.path());
    guard_resource_for_tenant(state, tenant, &surface, "runs", "run_id", run_id, "run").await
}

/// Go withByIDTenantGuard on computer_use_artifacts.artifact_id.
async fn guard_artifact_resource(
    state: &AppState,
    method: &Method,
    uri: &Uri,
    tenant: Option<&TenantContext>,
    artifact_id: &str,
) -> Result<(), ApiError> {
    let surface = format!("api:{} {}", method.as_str(), uri.path());
    guard_resource_for_tenant(
        state,
        tenant,
        &surface,
        "computer_use_artifacts",
        "artifact_id",
        artifact_id,
        "computer_use_artifact",
    )
    .await
}

/// Go currentActor: the authenticated token's label, or its token id.
fn current_actor(token: Option<&AuthenticatedToken>) -> String {
    let Some(token) = token else {
        return String::new();
    };
    if !token.0.label.trim().is_empty() {
        token.0.label.clone()
    } else {
        token.0.token_id.clone()
    }
}

/// Go persistComputerUseRuntimeTracking: persist the runtime step/tool call
/// the manager created (plus the owning run), then a run checkpoint. Skipped
/// when no runtime manager is configured (Go returns nil then).
pub(crate) fn persist_computer_use_runtime_tracking(
    state: &AppState,
    action: &computeruse::Action,
) -> Result<(), ApiError> {
    let Some(manager) = state.runtime.as_ref() else {
        return Ok(());
    };
    {
        let store = state.store.lock();
        if let Some(run) = manager.get_run(&action.run_id) {
            store.upsert_run(&run).map_err(ApiError::from_store)?;
        }
        if let Some(step) = manager.get_step(&action.run_id, &action.step_id) {
            store.upsert_step(&step).map_err(ApiError::from_store)?;
        }
        if let Some(tool_call) =
            manager.get_tool_call(&action.run_id, &action.step_id, &action.tool_call_id)
        {
            store
                .upsert_tool_call(&tool_call)
                .map_err(ApiError::from_store)?;
        }
    }
    if let Some(checkpoints) = state.checkpoints.as_ref() {
        checkpoints
            .save_run_checkpoint(&action.run_id)
            .map_err(ApiError::from_store)?;
    }
    Ok(())
}

/// Go publishComputerUseArtifacts: one computer_use.artifact_recorded event
/// per artifact on the action (best-effort in Go too).
pub(crate) fn publish_computer_use_artifacts(
    state: &AppState,
    tenant: Option<&TenantContext>,
    action: &computeruse::Action,
) {
    for artifact in &action.artifacts {
        let payload = serde_json::json!({
            "artifactId": artifact.artifact_id,
            "artifactKind": artifact.kind,
            "captureStatus": artifact.status,
        });
        let _ = publish_event(
            state,
            tenant,
            events::Event {
                category: "capability".to_string(),
                name: "computer_use.artifact_recorded".to_string(),
                scope: events::Scope {
                    run_id: action.run_id.clone(),
                    computer_use_session_id: action.computer_use_session_id.clone(),
                    computer_use_action_id: action.computer_use_action_id.clone(),
                    ..events::Scope::default()
                },
                resource: events::Resource {
                    kind: "computer_use_artifact".to_string(),
                    id: artifact.artifact_id.clone(),
                },
                payload: payload.as_object().cloned().unwrap_or_default(),
                ..events::Event::default()
            },
        );
    }
}

/// Go publishComputerUseTargetMismatch: computer_use.action_target_mismatch.
pub(crate) fn publish_computer_use_target_mismatch(
    state: &AppState,
    tenant: Option<&TenantContext>,
    action: &computeruse::Action,
) -> Result<(), ApiError> {
    publish_event(
        state,
        tenant,
        action_event("computer_use.action_target_mismatch", action),
    )
}

/// Capability-category action event (Go computer_use.action_requested /
/// action_status_changed / action_target_mismatch share this shape).
fn action_event(name: &str, action: &computeruse::Action) -> events::Event {
    let payload = serde_json::json!({
        "status": action.status,
        "actionKind": action.action_kind,
        "failureClass": action.failure_class,
        "approvalId": action.approval_id,
        "computerUseSessionId": action.computer_use_session_id,
        "computerUseActionId": action.computer_use_action_id,
    });
    events::Event {
        category: "capability".to_string(),
        name: name.to_string(),
        scope: events::Scope {
            run_id: action.run_id.clone(),
            step_id: action.step_id.clone(),
            computer_use_session_id: action.computer_use_session_id.clone(),
            computer_use_action_id: action.computer_use_action_id.clone(),
            ..events::Scope::default()
        },
        resource: events::Resource {
            kind: "computer_use_action".to_string(),
            id: action.computer_use_action_id.clone(),
        },
        payload: payload.as_object().cloned().unwrap_or_default(),
        ..events::Event::default()
    }
}

/// Go publishEvent (see calendar.rs for the shared shape): bind environment
/// scope + tenant, persist (tenant-owned or global path), then bus publish.
fn publish_event(
    state: &AppState,
    tenant: Option<&TenantContext>,
    event: events::Event,
) -> Result<(), ApiError> {
    let mut prepared = event;
    if prepared.environment_scope.is_empty() {
        prepared.environment_scope = environment_scope_from_config(&state.config);
    }
    if prepared.tenant_id.is_empty() {
        if let Some(tc) = tenant {
            if !tc.0.tenant_id.is_empty() && !events::is_global_category(&prepared.category) {
                prepared.tenant_id = tc.0.tenant_id.clone();
            }
        }
    }
    if prepared.event_id.is_empty() {
        prepared.event_id = new_event_id();
    }
    if prepared.occurred_at == DateTime::<Utc>::MIN_UTC {
        prepared.occurred_at = Utc::now();
    }
    let persisted = if prepared.tenant_id.is_empty() {
        state.store.lock().append_event(&prepared)
    } else {
        state
            .store
            .lock()
            .append_event_for_tenant_raw(&prepared, &prepared.tenant_id)
    }
    .map_err(ApiError::from_store)?;
    let _ = state.event_bus.publish(persisted);
    Ok(())
}

fn new_event_id() -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("evt_{}", &hex[..16])
}

/// Go decodeJSONBody: an empty body maps to "request body is required" (400);
/// malformed JSON maps to the decoder error (400).
fn decode_json_body<T: serde::de::DeserializeOwned>(body: &Bytes) -> Result<T, ApiError> {
    if body.is_empty() {
        return Err(ApiError::BadRequest("request body is required".to_string()));
    }
    serde_json::from_slice(body).map_err(|err| ApiError::BadRequest(err.to_string()))
}

/// RFC 4648 base64 (std-lib only; no base64 dependency in kura-api).
fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap as StdHashMap;
    use std::sync::{Arc, Mutex as StdMutex};

    use axum::body::{to_bytes, Body};
    use axum::http::header::CONTENT_TYPE;
    use axum::http::Request;
    use kura_computeruse::{
        Action, Artifact, ArtifactCaptureRequest, ArtifactRecorder, ArtifactStatus, Dependencies,
        Store,
    };
    use kura_policy::Engine as PolicyEngine;
    use parking_lot::Mutex;
    use tower::ServiceExt;
    use uuid::Uuid;

    // ------------------------------------------------------------------
    // Test seams: in-memory Store + ArtifactRecorder (mirrors
    // the computeruse manager tests; SQLiteStore does not implement
    // kura_computeruse::Store yet).
    // ------------------------------------------------------------------

    #[derive(Default)]
    struct MemStore {
        sessions: StdMutex<StdHashMap<String, computeruse::Session>>,
        actions: StdMutex<StdHashMap<String, computeruse::Action>>,
        artifacts: StdMutex<StdHashMap<String, Artifact>>,
    }

    impl Store for MemStore {
        fn upsert_computer_use_session(
            &self,
            session: &computeruse::Session,
        ) -> Result<(), String> {
            self.sessions
                .lock()
                .expect("lock")
                .insert(session.computer_use_session_id.clone(), session.clone());
            Ok(())
        }
        fn list_computer_use_sessions(
            &self,
            _env: &str,
            run_id: &str,
        ) -> Result<Vec<computeruse::Session>, String> {
            Ok(self
                .sessions
                .lock()
                .expect("lock")
                .values()
                .filter(|s| s.run_id == run_id)
                .cloned()
                .collect())
        }
        fn get_computer_use_session(
            &self,
            _env: &str,
            _run_id: &str,
            session_id: &str,
        ) -> Result<Option<computeruse::Session>, String> {
            Ok(self.sessions.lock().expect("lock").get(session_id).cloned())
        }
        fn upsert_computer_use_action(&self, action: &Action) -> Result<(), String> {
            self.actions
                .lock()
                .expect("lock")
                .insert(action.computer_use_action_id.clone(), action.clone());
            Ok(())
        }
        fn list_computer_use_actions(
            &self,
            _env: &str,
            _run_id: &str,
            session_id: &str,
        ) -> Result<Vec<Action>, String> {
            Ok(self
                .actions
                .lock()
                .expect("lock")
                .values()
                .filter(|a| a.computer_use_session_id == session_id)
                .cloned()
                .collect())
        }
        fn get_computer_use_action(
            &self,
            _env: &str,
            _run_id: &str,
            _session_id: &str,
            action_id: &str,
        ) -> Result<Option<Action>, String> {
            Ok(self.actions.lock().expect("lock").get(action_id).cloned())
        }
        fn find_pending_computer_use_action_by_approval(
            &self,
            _env: &str,
            _approval_id: &str,
        ) -> Result<Option<Action>, String> {
            Ok(None)
        }
        fn upsert_computer_use_artifact(&self, artifact: &Artifact) -> Result<(), String> {
            self.artifacts
                .lock()
                .expect("lock")
                .insert(artifact.artifact_id.clone(), artifact.clone());
            Ok(())
        }
        fn list_computer_use_artifacts_for_action(
            &self,
            _env: &str,
            _run_id: &str,
            action_id: &str,
        ) -> Result<Vec<Artifact>, String> {
            Ok(self
                .artifacts
                .lock()
                .expect("lock")
                .values()
                .filter(|a| a.computer_use_action_id == action_id)
                .cloned()
                .collect())
        }
        fn get_computer_use_artifact(
            &self,
            _env: &str,
            artifact_id: &str,
        ) -> Result<Option<Artifact>, String> {
            Ok(self
                .artifacts
                .lock()
                .expect("lock")
                .get(artifact_id)
                .cloned())
        }
        fn mark_in_flight_computer_use_interrupted(
            &self,
            _env: &str,
            _now: DateTime<Utc>,
        ) -> Result<(Vec<computeruse::Session>, Vec<Action>), String> {
            Ok((Vec::new(), Vec::new()))
        }
    }

    #[derive(Default)]
    struct MemArtifactRecorder {
        contents: StdMutex<StdHashMap<String, Vec<u8>>>,
    }

    impl ArtifactRecorder for MemArtifactRecorder {
        fn save_computer_use_artifact(
            &self,
            input: ArtifactCaptureRequest,
        ) -> Result<Artifact, String> {
            let hex = Uuid::new_v4().simple().to_string();
            let artifact_id = format!("cuart_{}", &hex[..16]);
            self.contents
                .lock()
                .expect("lock")
                .insert(artifact_id.clone(), input.content.clone());
            Ok(Artifact {
                artifact_id: artifact_id.clone(),
                run_id: input.run_id,
                computer_use_session_id: input.computer_use_session_id,
                computer_use_action_id: input.computer_use_action_id,
                kind: input.kind,
                status: ArtifactStatus::Available,
                mime_type: input.mime_type,
                file_name: input.file_name,
                byte_size: input.estimated_byte_size,
                storage_key: artifact_id.clone(),
                created_at: Utc::now(),
                ..Artifact::default()
            })
        }
        fn read_computer_use_artifact_content(&self, storage_key: &str) -> Result<Vec<u8>, String> {
            self.contents
                .lock()
                .expect("lock")
                .get(storage_key)
                .cloned()
                .ok_or_else(|| "artifact content not found".to_string())
        }
    }

    fn test_config() -> kura_config::Config {
        kura_config::Config {
            project_root: String::new(),
            environment: kura_config::Environment::Test,
            bind_addr: "127.0.0.1:19192".to_string(),
            data_dir: "/tmp/kura-api-computer-use".to_string(),
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
        }
    }

    /// Builds a state with a runtime manager (with one run), a policy engine
    /// and a computer-use manager wired to in-memory store/recorder seams.
    fn test_state() -> (AppState, Arc<kura_runtime::Manager>, String) {
        let dir = std::env::temp_dir().join(format!("kura-api-computer-use-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let store = Arc::new(Mutex::new(
            kura_store::SQLiteStore::new(dir.to_str().expect("path")).expect("store"),
        ));
        let runtime = Arc::new(kura_runtime::Manager::new());
        let run = runtime
            .create_run(kura_runtime::CreateRunInput {
                entrypoint: "browse".to_string(),
                goal: "computer-use api test".to_string(),
                ..kura_runtime::CreateRunInput::default()
            })
            .expect("create run");
        let policy = Arc::new(PolicyEngine::new());
        let computer_use = Arc::new(computeruse::Manager::new(Dependencies {
            environment_scope: "test".to_string(),
            runtime: Some(runtime.clone()),
            policy: Some(policy.clone()),
            store: Arc::new(MemStore::default()),
            driver: None,
            artifacts: Some(Arc::new(MemArtifactRecorder::default())),
        }));
        let mut state = AppState::new(test_config(), Arc::new(kura_events::Bus::new()), store);
        state.runtime = Some(runtime.clone());
        state.policy = Some(policy);
        state.computer_use = Some(computer_use);
        (state, runtime, run.run_id)
    }

    fn request(method: &str, uri: &str, body: Option<&str>) -> Request<Body> {
        let builder = Request::builder()
            .method(method)
            .uri(uri)
            .header(CONTENT_TYPE, "application/json");
        let req = match body {
            Some(payload) => builder
                .body(Body::from(payload.to_string()))
                .expect("request"),
            None => builder.body(Body::empty()).expect("request"),
        };
        req
    }

    async fn send(app: &axum::Router, req: Request<Body>) -> (StatusCode, serde_json::Value) {
        let response = app.clone().oneshot(req).await.expect("oneshot");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let json = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("json body")
        };
        (status, json)
    }

    async fn create_session(app: &axum::Router, run_id: &str) -> computeruse::Session {
        let (status, json) = send(
            app,
            request(
                "POST",
                &format!("/v1/runs/{run_id}/computer-use/sessions"),
                Some(r#"{"driverKind":"browser","initialUrl":"https://example.com"}"#),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        serde_json::from_value(json).expect("session")
    }

    /// Port of the Go session lifecycle leg of
    /// TestComputerUseSessionAndApprovalRoutes: create/list/get/close + the
    /// session_created / session_status_changed events.
    #[tokio::test]
    async fn session_lifecycle() {
        let (state, _runtime, run_id) = test_state();
        let app = router().with_state(state.clone());
        let session = create_session(&app, &run_id).await;
        assert_eq!(session.status, computeruse::SessionStatus::Active);
        assert_eq!(session.driver_kind, "browser");

        let (status, json) = send(
            &app,
            request(
                "GET",
                &format!("/v1/runs/{run_id}/computer-use/sessions"),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["items"].as_array().expect("items").len(), 1);
        assert_eq!(
            json["items"][0]["computerUseSessionId"],
            session.computer_use_session_id
        );

        let (status, json) = send(
            &app,
            request(
                "GET",
                &format!(
                    "/v1/runs/{run_id}/computer-use/sessions/{}",
                    session.computer_use_session_id
                ),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            json["computerUseSessionId"],
            session.computer_use_session_id
        );

        let (status, json) = send(
            &app,
            request(
                "POST",
                &format!(
                    "/v1/runs/{run_id}/computer-use/sessions/{}/close",
                    session.computer_use_session_id
                ),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["status"], "closed");

        let capability_events = state.event_bus.list(&kura_events::Filter {
            run_id: run_id.clone(),
            category: "capability".to_string(),
            ..kura_events::Filter::default()
        });
        let names: Vec<&str> = capability_events.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"computer_use.session_created"));
        assert!(names.contains(&"computer_use.session_status_changed"));
    }

    /// Port of the approval-gate leg of TestComputerUseSessionAndApprovalRoutes
    /// (the resolution leg rides the /v1/policy/approvals family, which is not
    /// in this wave): a high-risk action answers 409 with the gated action and
    /// a pending approval, and the approval/decision are persisted.
    #[tokio::test]
    async fn high_risk_action_gates_approval() {
        let (state, _runtime, run_id) = test_state();
        let app = router().with_state(state.clone());
        let session = create_session(&app, &run_id).await;

        let (status, json) = send(
            &app,
            request(
                "POST",
                &format!("/v1/runs/{run_id}/computer-use/sessions/{}/actions", session.computer_use_session_id),
                Some(r##"{"actionKind":"input","value":"Phase 26","targetMatchContext":{"matchStrategy":"dom_selector","expectedSelector":"#name"}}"##),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        let approval_id = json["approval"]["approvalId"]
            .as_str()
            .expect("approvalId")
            .to_string();
        assert!(!approval_id.is_empty());
        assert_eq!(json["action"]["status"], "waiting_approval");
        assert_eq!(json["decision"]["outcome"], "requires_approval");

        // The approval + decision were persisted to the store.
        let approvals = state.store.lock().list_approvals().expect("list approvals");
        assert_eq!(approvals.len(), 1);
        assert_eq!(approvals[0].approval_id, approval_id);
        assert_eq!(approvals[0].action, "computer_use.action.execute");
        let decisions = state.store.lock().list_decisions().expect("list decisions");
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].approval_id, approval_id);
    }

    /// Port of the navigation-failure leg of
    /// TestComputerUseApprovalDenialAndNavigationFailureAreInspectable: a
    /// low-risk navigate (no url) executes and answers 201 with a failed
    /// navigation action.
    #[tokio::test]
    async fn navigation_failure_is_inspectable() {
        let (state, _runtime, run_id) = test_state();
        let app = router().with_state(state.clone());
        let session = create_session(&app, &run_id).await;

        let (status, json) = send(
            &app,
            request(
                "POST",
                &format!(
                    "/v1/runs/{run_id}/computer-use/sessions/{}/actions",
                    session.computer_use_session_id
                ),
                Some(r#"{"actionKind":"navigate"}"#),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(json["status"], "failed");
        assert_eq!(json["failureClass"], "navigation_failure");

        let action_id = json["computerUseActionId"].as_str().expect("action id");
        let (status, json) = send(
            &app,
            request(
                "GET",
                &format!(
                    "/v1/runs/{run_id}/computer-use/sessions/{}/actions/{action_id}",
                    session.computer_use_session_id
                ),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["failureClass"], "navigation_failure");
    }

    /// Port of the artifact legs of TestComputerUseSessionAndApprovalRoutes: a
    /// completed snapshot produces an artifact; detail + base64 content read
    /// back; unknown artifacts 404.
    #[tokio::test]
    async fn artifact_detail_and_content() {
        let (state, _runtime, run_id) = test_state();
        let app = router().with_state(state.clone());
        let session = create_session(&app, &run_id).await;

        let (status, json) = send(
            &app,
            request(
                "POST",
                &format!(
                    "/v1/runs/{run_id}/computer-use/sessions/{}/actions",
                    session.computer_use_session_id
                ),
                Some(r#"{"actionKind":"snapshot"}"#),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(json["status"], "completed");
        let artifacts = json["artifacts"].as_array().expect("artifacts");
        assert!(!artifacts.is_empty(), "expected evidence artifacts");
        let artifact_id = artifacts[0]["artifactId"]
            .as_str()
            .expect("artifact id")
            .to_string();

        let (status, json) = send(
            &app,
            request(
                "GET",
                &format!("/v1/computer-use/artifacts/{artifact_id}"),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["artifactId"], artifact_id);

        let (status, json) = send(
            &app,
            request(
                "GET",
                &format!("/v1/computer-use/artifacts/{artifact_id}/content"),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let content = json["content"].as_str().expect("content");
        // Round-trip the base64 manually (no base64 dependency in kura-api).
        let decoded = base64_decode_for_test(content);
        assert!(!decoded.is_empty());

        let (status, _) = send(
            &app,
            request("GET", "/v1/computer-use/artifacts/cuart_missing", None),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = send(
            &app,
            request(
                "GET",
                "/v1/computer-use/artifacts/cuart_missing/content",
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// The manager-absent branch answers 500 (Go writeError).
    #[tokio::test]
    async fn missing_manager_returns_500() {
        let dir =
            std::env::temp_dir().join(format!("kura-api-computer-use-none-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let store = Arc::new(Mutex::new(
            kura_store::SQLiteStore::new(dir.to_str().expect("path")).expect("store"),
        ));
        let state = AppState::new(test_config(), Arc::new(kura_events::Bus::new()), store);
        let app = router().with_state(state);
        let (status, json) = send(
            &app,
            request("GET", "/v1/runs/run_x/computer-use/sessions", None),
        )
        .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(json["error"]
            .as_str()
            .unwrap_or("")
            .contains("computer-use manager is not configured"));
    }

    /// Unknown run -> 404 (Go runtime.ErrRunNotFound), unknown session -> 404.
    #[tokio::test]
    async fn unknown_run_and_session_404() {
        let (state, _runtime, _run_id) = test_state();
        let app = router().with_state(state.clone());
        let (status, _) = send(
            &app,
            request(
                "POST",
                "/v1/runs/run_missing/computer-use/sessions",
                Some(r#"{"driverKind":"browser"}"#),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, _) = send(
            &app,
            request(
                "GET",
                "/v1/runs/run_x/computer-use/sessions/cusess_missing",
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// Minimal base64 decoder used only by tests (mirrors base64_encode).
    fn base64_decode_for_test(input: &str) -> Vec<u8> {
        fn value(byte: u8) -> Option<u8> {
            match byte {
                b'A'..=b'Z' => Some(byte - b'A'),
                b'a'..=b'z' => Some(byte - b'a' + 26),
                b'0'..=b'9' => Some(byte - b'0' + 52),
                b'+' => Some(62),
                b'/' => Some(63),
                _ => None,
            }
        }
        let mut out = Vec::new();
        let mut acc: u32 = 0;
        let mut bits = 0;
        for byte in input.bytes() {
            if byte == b'=' {
                break;
            }
            let Some(v) = value(byte) else { continue };
            acc = (acc << 6) | u32::from(v);
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((acc >> bits) as u8);
                acc &= (1 << bits) - 1;
            }
        }
        out
    }
}
