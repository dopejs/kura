//! triage route family (port of daemon/internal/api/triage.go, Roadmap 65).
//!
//! Routes: GET/POST /v1/triage/policies, GET /v1/triage/policies/{policy_id},
//! POST /v1/triage/policies/{policy_id}/run. Triage evaluates explicit-rule
//! policies over caller-supplied message sets; it never scans a mailbox on its
//! own. Error mapping preserves Go writeTriageError: policy-not-found -> 404,
//! invalid rule/policy -> 400, everything else -> 500.

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use kura_triage as triage;

use crate::error::ApiError;
use crate::middleware::TenantContext;
use crate::state::AppState;

use super::{
    bind_document_tenant, decode_json_or_default, decode_json_required, guard_document_tenant,
    tenant_visible_document_ids,
};

/// Manager-document kind backing this family; ownership lives on that row.
const DOC_KIND: &str = triage::DOC_KIND_POLICY;

/// Route family router. Unregistered methods answer 405 like the Go
/// MethodNotAllowed branches.
#[must_use]
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/v1/triage/policies",
            get(list_policies).post(create_policy),
        )
        .route("/v1/triage/policies/{policy_id}", get(get_policy))
        .route("/v1/triage/policies/{policy_id}/run", post(run_policy))
}

/// Go CreateTriagePolicyRequest.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct CreateTriagePolicyRequest {
    name: String,
    rules: Vec<triage::Rule>,
    default_classification: triage::Classification,
}

/// Go RunTriageRequest — the caller selects which messages to triage.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct RunTriageRequest {
    messages: Vec<triage::Message>,
}

#[derive(Debug, Serialize)]
struct TriagePolicyListResponse {
    items: Vec<triage::Policy>,
}

fn manager(state: &AppState) -> Result<&triage::Manager, ApiError> {
    state
        .triage
        .as_deref()
        .ok_or_else(|| ApiError::internal("triage manager is not configured"))
}

fn map_triage_error(err: triage::TriageError) -> ApiError {
    let message = err.to_string();
    match err {
        triage::TriageError::PolicyNotFound => ApiError::NotFound(message),
        triage::TriageError::InvalidRule | triage::TriageError::InvalidPolicy => {
            ApiError::BadRequest(message)
        }
    }
}

/// GET /v1/triage/policies (Go handleTriagePolicies GET branch).
async fn list_policies(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Json<TriagePolicyListResponse>, ApiError> {
    let manager = manager(&state)?;
    let visible = tenant_visible_document_ids(&state, tenant.as_ref().map(|e| &e.0), DOC_KIND)?;
    let items = manager
        .list_policies()
        .into_iter()
        .filter(|policy| match &visible {
            Some(ids) => ids.contains(policy.policy_id.trim()),
            None => true,
        })
        .collect();
    Ok(Json(TriagePolicyListResponse { items }))
}

/// POST /v1/triage/policies (Go handleTriagePolicies POST branch) — 201.
async fn create_policy(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<(StatusCode, Json<triage::Policy>), ApiError> {
    let request: CreateTriagePolicyRequest = decode_json_required(&body)?;
    let manager = manager(&state)?;
    let policy = manager
        .create_policy(
            request.name.trim(),
            request.rules,
            request.default_classification,
        )
        .map_err(map_triage_error)?;
    bind_document_tenant(
        &state,
        tenant.as_ref().map(|e| &e.0),
        DOC_KIND,
        policy.policy_id.trim(),
    )?;
    Ok((StatusCode::CREATED, Json(policy)))
}

/// GET /v1/triage/policies/{policy_id} (Go handleTriagePolicyRoutes get).
async fn get_policy(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(policy_id): Path<String>,
) -> Result<Json<triage::Policy>, ApiError> {
    let manager = manager(&state)?;
    guard_document_tenant(
        &state,
        tenant.as_ref().map(|e| &e.0),
        DOC_KIND,
        policy_id.trim(),
    )?;
    manager
        .get_policy(policy_id.trim())
        .map(Json)
        .ok_or_else(|| map_triage_error(triage::TriageError::PolicyNotFound))
}

/// POST /v1/triage/policies/{policy_id}/run (Go handleTriagePolicyRoutes run
/// branch) — 201; an empty body runs the policy over zero messages (Go
/// tolerates the EOF decode error).
async fn run_policy(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(policy_id): Path<String>,
    body: Bytes,
) -> Result<(StatusCode, Json<triage::Run>), ApiError> {
    let request: RunTriageRequest = decode_json_or_default(&body)?;
    let manager = manager(&state)?;
    guard_document_tenant(
        &state,
        tenant.as_ref().map(|e| &e.0),
        DOC_KIND,
        policy_id.trim(),
    )?;
    let run = manager
        .run(policy_id.trim(), &request.messages)
        .map_err(map_triage_error)?;
    Ok((StatusCode::CREATED, Json(run)))
}

#[cfg(test)]
mod tests {
    use super::super::tests_support::{request_json, request_json_as_tenant, test_state};
    use axum::http::StatusCode;
    use std::sync::Arc;

    fn state_with_manager() -> crate::state::AppState {
        let mut state = test_state();
        // Mirror the production assembly (`plugins.rs:1295`); see the note in
        // the routines tests.
        let mut manager = kura_triage::Manager::new("test");
        manager.with_store(state.store.clone());
        state.triage = Some(Arc::new(manager));
        state
    }

    #[tokio::test]
    async fn create_list_get_and_run_policy() {
        let state = state_with_manager();
        let (status, created) = request_json(
            state.clone(),
            "POST",
            "/v1/triage/policies",
            Some(serde_json::json!({
                "name": "vip",
                "rules": [],
                "defaultClassification": "urgent"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let policy_id = created["policyId"].as_str().expect("policyId").to_string();

        let (status, listed) =
            request_json(state.clone(), "GET", "/v1/triage/policies", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(listed["items"].as_array().expect("items").len(), 1);

        let (status, fetched) = request_json(
            state.clone(),
            "GET",
            &format!("/v1/triage/policies/{policy_id}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(fetched["policyId"], policy_id.as_str());

        // Empty body is tolerated: the run covers zero messages.
        let (status, run) = request_json(
            state,
            "POST",
            &format!("/v1/triage/policies/{policy_id}/run"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{run}");
        assert_eq!(run["policyId"], policy_id.as_str());
    }

    #[tokio::test]
    async fn missing_policy_is_404_and_missing_manager_is_500() {
        let (status, _) = request_json(
            state_with_manager(),
            "GET",
            "/v1/triage/policies/triage_policy_missing",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, body) = request_json(test_state(), "GET", "/v1/triage/policies", None).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["error"], "triage manager is not configured");
    }

    /// Triage policies follow the same manager-document ownership model as
    /// routines.
    #[tokio::test]
    async fn triage_policies_are_scoped_to_the_acting_tenant() {
        let state = state_with_manager();
        let body = serde_json::json!({
            "name": "vip",
            "rules": [],
            "defaultClassification": "urgent"
        });

        let (status, created) = request_json_as_tenant(
            state.clone(),
            "tnt_a",
            "POST",
            "/v1/triage/policies",
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let policy_id = created["policyId"].as_str().expect("policyId").to_string();

        let (status, listed) =
            request_json_as_tenant(state.clone(), "tnt_a", "GET", "/v1/triage/policies", None)
                .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(listed["items"].as_array().expect("items").len(), 1);

        let (status, listed) =
            request_json_as_tenant(state.clone(), "tnt_b", "GET", "/v1/triage/policies", None)
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
            &format!("/v1/triage/policies/{policy_id}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // Running another tenant's policy is refused on the same guard.
        let (status, _) = request_json_as_tenant(
            state.clone(),
            "tnt_b",
            "POST",
            &format!("/v1/triage/policies/{policy_id}/run"),
            Some(serde_json::json!({ "messages": [] })),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
