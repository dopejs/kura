//! sandboxes route family (port of the /v1/sandboxes handlers in Go
//! daemon/internal/api/server.go, Roadmap 16).
//!
//! Routes: GET /v1/sandboxes/profiles, POST /v1/sandboxes/profiles/reload,
//! GET /v1/sandboxes/profiles/{profile_id}, GET/POST /v1/sandboxes/executions,
//! GET /v1/sandboxes/executions/{execution_id}, POST
//! /v1/sandboxes/executions/{execution_id}/cancel, POST /v1/sandboxes/explain.
//!
//! Error mapping preserves the Go handlers: nil manager -> 500, missing
//! command -> 400, unknown execution -> 404.
//!
//! Deliberately not ported (documented divergence): the Go explain handler's
//! credential-inspection redaction (redactSandboxDecisionCredentialInspection)
//! keys off the resolved tenant context, which the router only carries once
//! the protected() middleware attachment task in Roadmap 74 lands; Go skips
//! the redaction when no tenant context is resolved, which is the only state
//! the current router produces.

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;

use kura_sandbox as sandbox;

use crate::error::ApiError;
use crate::middleware::{AuthenticatedToken, TenantContext, require_daemon_global_operator};
use crate::state::AppState;

use super::decode_json_required;

/// Route family router.
#[must_use]
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/sandboxes/profiles", get(list_profiles))
        .route("/v1/sandboxes/profiles/reload", post(reload_profiles))
        .route("/v1/sandboxes/profiles/{profile_id}", get(get_profile))
        .route(
            "/v1/sandboxes/executions",
            get(list_executions).post(start_execution),
        )
        .route(
            "/v1/sandboxes/executions/{execution_id}",
            get(get_execution),
        )
        .route(
            "/v1/sandboxes/executions/{execution_id}/cancel",
            post(cancel_execution),
        )
        .route("/v1/sandboxes/explain", post(explain))
}

#[derive(Debug, Serialize)]
struct ProfileListResponse {
    items: Vec<sandbox::Profile>,
}

#[derive(Debug, Serialize)]
struct ExecutionListResponse {
    items: Vec<sandbox::Execution>,
}

/// Go SandboxExplainResponse.
#[derive(Debug, Serialize)]
struct SandboxExplainResponse {
    decision: sandbox::Decision,
}

fn manager(state: &AppState) -> Result<&sandbox::Manager, ApiError> {
    state
        .sandboxes
        .as_deref()
        .ok_or_else(|| ApiError::internal("sandbox manager is not configured"))
}

/// Go currentActor: the authenticated token's label, or its token id (same
/// helper as the computer-use family).
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

fn map_sandbox_error(err: sandbox::SandboxError) -> ApiError {
    match err {
        sandbox::SandboxError::CommandRequired => ApiError::BadRequest(err.to_string()),
        sandbox::SandboxError::ExecutionNotFound => ApiError::NotFound("not found".to_string()),
        other => ApiError::Internal(other.to_string()),
    }
}

/// GET /v1/sandboxes/profiles (Go handleSandboxProfiles).
async fn list_profiles(
    State(state): State<AppState>,
) -> Result<Json<ProfileListResponse>, ApiError> {
    let manager = manager(&state)?;
    Ok(Json(ProfileListResponse {
        items: manager.list_profiles(),
    }))
}

/// POST /v1/sandboxes/profiles/reload (Go handleSandboxProfileRoutes reload
/// branch).
async fn reload_profiles(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Json<ProfileListResponse>, ApiError> {
    // Execution profiles are operator-defined daemon configuration, not
    // per-principal data; reloading them changes what every tenant runs under.
    require_daemon_global_operator(
        tenant.as_ref().map(|e| &e.0),
        "POST /v1/sandboxes/profiles/reload",
    )?;
    let manager = manager(&state)?;
    Ok(Json(ProfileListResponse {
        items: manager.reload(),
    }))
}

/// GET /v1/sandboxes/profiles/{profile_id} (Go handleSandboxProfileRoutes get
/// branch).
async fn get_profile(
    State(state): State<AppState>,
    Path(profile_id): Path<String>,
) -> Result<Json<sandbox::Profile>, ApiError> {
    let manager = manager(&state)?;
    manager
        .get_profile(profile_id.trim())
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("not found".to_string()))
}

/// GET /v1/sandboxes/executions (Go handleSandboxExecutions GET branch).
async fn list_executions(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Json<ExecutionListResponse>, ApiError> {
    let manager = manager(&state)?;
    let visible = visible_execution_ids(&state, tenant.as_ref().map(|e| &e.0))?;
    let items = manager
        .list_executions()
        .into_iter()
        .filter(|execution| match &visible {
            Some(ids) => ids.contains(execution.execution_id.trim()),
            None => true,
        })
        .collect();
    Ok(Json(ExecutionListResponse { items }))
}

/// Execution ids the acting tenant may enumerate, or `None` in the
/// single-user assembly. The sandbox manager holds executions in memory for
/// the whole daemon; ownership lives on the `sandbox_executions` row.
fn visible_execution_ids(
    state: &AppState,
    tenant: Option<&TenantContext>,
) -> Result<Option<std::collections::HashSet<String>>, ApiError> {
    let Some(tc) = tenant else { return Ok(None) };
    let tenant_id = tc.0.tenant_id.trim();
    if tenant_id.is_empty() {
        return Ok(None);
    }
    let ids = state
        .store_pool
        .read()
        .list_sandbox_execution_ids_for_tenant(tenant_id)
        .map_err(ApiError::from_store)?;
    Ok(Some(ids.into_iter().collect()))
}

/// POST /v1/sandboxes/executions (Go handleSandboxExecutions POST branch) —
/// 201 with the started execution.
async fn start_execution(
    State(state): State<AppState>,
    token: Option<Extension<AuthenticatedToken>>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<(StatusCode, Json<sandbox::Execution>), ApiError> {
    let mut request: sandbox::ExecutionRequest = decode_json_required(&body)?;
    if request.requested_by.trim().is_empty() {
        request.requested_by = current_actor(token.as_ref().map(|extension| &extension.0));
    }
    let manager = manager(&state)?;
    let execution = manager
        .start_execution(request)
        .map_err(map_sandbox_error)?;
    if let Some(tc) = tenant.as_ref() {
        let tenant_id = tc.0.0.tenant_id.trim();
        if !tenant_id.is_empty() {
            state
                .store
                .lock()
                .bind_row_tenant(
                    "sandbox_executions",
                    "execution_id",
                    execution.execution_id.trim(),
                    tenant_id,
                )
                .map_err(ApiError::from_store)?;
        }
    }
    Ok((StatusCode::CREATED, Json(execution)))
}

/// GET /v1/sandboxes/executions/{execution_id} (Go
/// handleSandboxExecutionRoutes get branch).
async fn get_execution(
    State(state): State<AppState>,
    Path(execution_id): Path<String>,
) -> Result<Json<sandbox::Execution>, ApiError> {
    let manager = manager(&state)?;
    manager
        .get_execution(execution_id.trim())
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("not found".to_string()))
}

/// POST /v1/sandboxes/executions/{execution_id}/cancel (Go
/// handleSandboxExecutionRoutes cancel branch).
async fn cancel_execution(
    State(state): State<AppState>,
    Path(execution_id): Path<String>,
) -> Result<Json<sandbox::Execution>, ApiError> {
    let manager = manager(&state)?;
    let (execution, _) = manager
        .cancel_execution(execution_id.trim())
        .map_err(map_sandbox_error)?;
    Ok(Json(execution))
}

/// POST /v1/sandboxes/explain (Go handleSandboxExplain) — the sandbox
/// decision for a request without executing it.
async fn explain(
    State(state): State<AppState>,
    token: Option<Extension<AuthenticatedToken>>,
    body: Bytes,
) -> Result<Json<SandboxExplainResponse>, ApiError> {
    let mut request: sandbox::ExecutionRequest = decode_json_required(&body)?;
    if request.requested_by.trim().is_empty() {
        request.requested_by = current_actor(token.as_ref().map(|extension| &extension.0));
    }
    let manager = manager(&state)?;
    let decision = manager.explain(request).map_err(map_sandbox_error)?;
    Ok(Json(SandboxExplainResponse { decision }))
}

#[cfg(test)]
mod tests {
    use super::super::tests_support::{request_json, request_json_as_tenant, test_state};
    use axum::http::StatusCode;
    use std::sync::Arc;

    fn state_with_manager() -> crate::state::AppState {
        let mut state = test_state();
        let manager = kura_sandbox::Manager::new(
            state.config.clone(),
            None,
            kura_events::Bus::new(),
            kura_policy::Engine::new(),
        );
        state.sandboxes = Some(Arc::new(manager));
        state
    }

    #[tokio::test]
    async fn profiles_list_get_and_reload() {
        let state = state_with_manager();
        let (status, listed) =
            request_json(state.clone(), "GET", "/v1/sandboxes/profiles", None).await;
        assert_eq!(status, StatusCode::OK, "{listed}");
        let items = listed["items"].as_array().expect("items");
        assert!(!items.is_empty(), "{listed}");
        let profile_id = items[0]["profileId"]
            .as_str()
            .expect("profileId")
            .to_string();

        let (status, fetched) = request_json(
            state.clone(),
            "GET",
            &format!("/v1/sandboxes/profiles/{profile_id}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{fetched}");

        let (status, reloaded) =
            request_json(state.clone(), "POST", "/v1/sandboxes/profiles/reload", None).await;
        assert_eq!(status, StatusCode::OK, "{reloaded}");

        let (status, _) = request_json(
            state,
            "GET",
            "/v1/sandboxes/profiles/sandbox_profile_missing",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn explain_requires_a_command_and_missing_execution_is_404() {
        let state = state_with_manager();
        let (status, body) = request_json(
            state.clone(),
            "POST",
            "/v1/sandboxes/explain",
            Some(serde_json::json!({ "command": "" })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

        let (status, _) = request_json(
            state.clone(),
            "GET",
            "/v1/sandboxes/executions/sbx_exec_missing",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, listed) = request_json(state, "GET", "/v1/sandboxes/executions", None).await;
        assert_eq!(status, StatusCode::OK, "{listed}");
    }

    /// Audit F2 (2026-09-19): this family was closed in 8.2 without a
    /// two-tenant test. Executions are per-principal work; one tenant must
    /// neither enumerate nor address another's.
    #[tokio::test]
    async fn executions_are_scoped_to_the_acting_tenant() {
        let mut state = test_state();
        // The manager persists executions only when it has a store; mirror
        // production and open one on the same data dir (it wants a std
        // Mutex, AppState holds a parking_lot one).
        // `test_state` opens its store on a unique temp dir while
        // `config.data_dir` is a fixed placeholder; the manager must share the
        // store's real directory or it persists into a different database.
        let data_dir = state.store.lock().data_dir().to_string();
        let store = kura_store::SQLiteStore::new(&data_dir).expect("store");
        let manager = kura_sandbox::Manager::new(
            state.config.clone(),
            Some(Arc::new(std::sync::Mutex::new(store))),
            kura_events::Bus::new(),
            kura_policy::Engine::new(),
        );
        state.sandboxes = Some(Arc::new(manager));

        let (status, started) = request_json_as_tenant(
            state.clone(),
            "tnt_a",
            "POST",
            "/v1/sandboxes/executions",
            Some(serde_json::json!({ "command": "/bin/echo", "args": ["hi"], "access": {} })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{started}");
        let execution_id = started["executionId"]
            .as_str()
            .expect("executionId")
            .to_string();

        let (status, listed) = request_json_as_tenant(
            state.clone(),
            "tnt_a",
            "GET",
            "/v1/sandboxes/executions",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            listed["items"].as_array().expect("items").len(),
            1,
            "{listed}"
        );

        let (status, listed) = request_json_as_tenant(
            state.clone(),
            "tnt_b",
            "GET",
            "/v1/sandboxes/executions",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            listed["items"].as_array().expect("items").is_empty(),
            "{listed}"
        );

        let (status, _) = request_json_as_tenant(
            state.clone(),
            "tnt_b",
            "GET",
            &format!("/v1/sandboxes/executions/{execution_id}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = request_json_as_tenant(
            state.clone(),
            "tnt_b",
            "POST",
            &format!("/v1/sandboxes/executions/{execution_id}/cancel"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = request_json_as_tenant(
            state.clone(),
            "tnt_a",
            "GET",
            &format!("/v1/sandboxes/executions/{execution_id}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
}
