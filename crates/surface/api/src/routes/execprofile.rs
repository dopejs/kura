//! execution profile route family (port of daemon/internal/api/execprofile.go,
//! Roadmap 69).
//!
//! Routes: GET /v1/execution/profiles, GET /v1/execution/profiles/{profile_id},
//! POST /v1/execution/profiles/{profile_id}/select, and POST
//! /v1/execution/explain (which profiles could run a tool needing the given
//! capabilities). Tenant resolution mirrors Go execTenant (body first, then
//! ?tenantId=). Error mapping preserves Go writeExecError: not-found -> 404,
//! permission denied -> 403, unavailable / invalid -> 400.

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use kura_execprofile as execprofile;

use crate::error::ApiError;
use crate::state::AppState;

use super::{decode_json_or_default, decode_json_required};

/// Route family router.
#[must_use]
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/execution/profiles", get(list_profiles))
        .route("/v1/execution/profiles/{profile_id}", get(get_profile))
        .route(
            "/v1/execution/profiles/{profile_id}/select",
            post(select_profile),
        )
        .route("/v1/execution/explain", post(explain_execution))
}

/// Go SelectExecutionProfileRequest.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct SelectExecutionProfileRequest {
    tenant_id: String,
    actor: String,
}

/// Go ExplainExecutionRequest.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct ExplainExecutionRequest {
    required_capabilities: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct TenantQuery {
    tenant_id: String,
}

#[derive(Debug, Serialize)]
struct ExecutionProfileListResponse {
    items: Vec<execprofile::ProfileProjection>,
}

fn manager(state: &AppState) -> Result<&execprofile::Manager, ApiError> {
    state
        .exec_profiles
        .as_deref()
        .ok_or_else(|| ApiError::internal("execution profile manager is not configured"))
}

fn map_exec_error(err: execprofile::ExecProfileError) -> ApiError {
    let message = err.to_string();
    match err {
        execprofile::ExecProfileError::ProfileNotFound => ApiError::NotFound(message),
        execprofile::ExecProfileError::PermissionDenied => ApiError::Forbidden(message),
        execprofile::ExecProfileError::ProfileUnavailable
        | execprofile::ExecProfileError::InvalidProfile => ApiError::BadRequest(message),
    }
}

/// GET /v1/execution/profiles (Go handleExecutionProfiles).
async fn list_profiles(
    State(state): State<AppState>,
) -> Result<Json<ExecutionProfileListResponse>, ApiError> {
    let manager = manager(&state)?;
    Ok(Json(ExecutionProfileListResponse {
        items: manager.list_profiles(),
    }))
}

/// GET /v1/execution/profiles/{profile_id} (Go handleExecutionProfileRoutes
/// get branch).
async fn get_profile(
    State(state): State<AppState>,
    Path(profile_id): Path<String>,
) -> Result<Json<execprofile::ProfileProjection>, ApiError> {
    let manager = manager(&state)?;
    let projection = manager
        .get_profile(profile_id.trim())
        .map_err(map_exec_error)?;
    Ok(Json(projection))
}

/// POST /v1/execution/profiles/{profile_id}/select (Go
/// handleExecutionProfileRoutes select branch); an empty body is tolerated.
async fn select_profile(
    State(state): State<AppState>,
    Path(profile_id): Path<String>,
    Query(query): Query<TenantQuery>,
    body: Bytes,
) -> Result<Json<execprofile::Selection>, ApiError> {
    let request: SelectExecutionProfileRequest = decode_json_or_default(&body)?;
    let manager = manager(&state)?;
    let tenant = if request.tenant_id.trim().is_empty() {
        query.tenant_id.trim().to_string()
    } else {
        request.tenant_id.trim().to_string()
    };
    let selection = manager
        .select_profile(&tenant, profile_id.trim(), request.actor.trim())
        .map_err(map_exec_error)?;
    Ok(Json(selection))
}

/// POST /v1/execution/explain (Go handleExecutionExplain) — a body is
/// required (Go decodeJSONBody fails on EOF with 400).
async fn explain_execution(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<execprofile::DenialExplanation>, ApiError> {
    let request: ExplainExecutionRequest = decode_json_required(&body)?;
    let manager = manager(&state)?;
    Ok(Json(manager.explain_denial(&request.required_capabilities)))
}

#[cfg(test)]
mod tests {
    use super::super::tests_support::{request_json, test_state};
    use axum::http::StatusCode;
    use std::sync::Arc;

    fn state_with_manager() -> crate::state::AppState {
        let manager = kura_execprofile::Manager::new("test", None, None, None);
        manager
            .register_profile(kura_execprofile::ExecutionProfile {
                profile_id: "exec_profile_subprocess".to_string(),
                name: "subprocess".to_string(),
                backend_kind: kura_execprofile::BackendKind::Subprocess,
                provides: vec!["filesystem".to_string()],
                ..kura_execprofile::ExecutionProfile::default()
            })
            .expect("register profile");
        let mut state = test_state();
        state.exec_profiles = Some(Arc::new(manager));
        state
    }

    #[tokio::test]
    async fn list_get_select_and_explain_profiles() {
        let state = state_with_manager();
        let (status, listed) =
            request_json(state.clone(), "GET", "/v1/execution/profiles", None).await;
        assert_eq!(status, StatusCode::OK, "{listed}");
        assert_eq!(listed["items"].as_array().expect("items").len(), 1);

        let (status, fetched) = request_json(
            state.clone(),
            "GET",
            "/v1/execution/profiles/exec_profile_subprocess",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{fetched}");

        let (status, selection) = request_json(
            state.clone(),
            "POST",
            "/v1/execution/profiles/exec_profile_subprocess/select",
            Some(serde_json::json!({ "tenantId": "ten_a", "actor": "operator_a" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{selection}");

        let (status, explained) = request_json(
            state,
            "POST",
            "/v1/execution/explain",
            Some(serde_json::json!({ "requiredCapabilities": ["network"] })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{explained}");
    }

    #[tokio::test]
    async fn missing_profile_is_404_and_missing_body_is_400() {
        let state = state_with_manager();
        let (status, _) = request_json(
            state.clone(),
            "GET",
            "/v1/execution/profiles/exec_profile_missing",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, body) = request_json(state, "POST", "/v1/execution/explain", None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "request body is required");
    }
}
