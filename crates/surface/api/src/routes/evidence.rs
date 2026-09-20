//! support evidence bundle route family (port of
//! daemon/internal/api/evidence.go, Roadmap 71).
//!
//! Routes: GET/POST /v1/support/evidence-bundles and GET
//! /v1/support/evidence-bundles/{bundle_id}. Bundles are redacted and
//! permission-gated; tenant/actor resolve from the body first and then the
//! `tenantId` / `actor` query parameters (Go evidenceTenant / evidenceActor).
//! Error mapping preserves Go writeEvidenceError: not-found -> 404,
//! permission / cross-tenant -> 403, invalid scope -> 400, and redaction
//! failures fail closed as 422.

use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use kura_evidence as evidence;

use crate::error::ApiError;
use crate::middleware::TenantContext;
use crate::state::AppState;

use super::decode_json_required;

/// Route family router.
#[must_use]
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/v1/support/evidence-bundles",
            get(list_bundles).post(generate_bundle),
        )
        .route("/v1/support/evidence-bundles/{bundle_id}", get(get_bundle))
}

/// Go GenerateEvidenceBundleRequest.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct GenerateEvidenceBundleRequest {
    tenant_id: String,
    actor: String,
    scope: evidence::Scope,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct EvidenceQuery {
    tenant_id: String,
    actor: String,
}

#[derive(Debug, Serialize)]
struct EvidenceBundleListResponse {
    items: Vec<evidence::Bundle>,
}

fn manager(state: &AppState) -> Result<&evidence::Manager, ApiError> {
    state
        .evidence
        .as_deref()
        .ok_or_else(|| ApiError::internal("evidence manager is not configured"))
}

/// Go evidenceTenant / evidenceActor: body value first, query fallback.
fn resolve(value: &str, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback.trim().to_string()
    } else {
        trimmed.to_string()
    }
}

/// A resolved tenant context overrides any caller-supplied tenant.
fn context_tenant(tenant: &Option<Extension<TenantContext>>, resolved: String) -> String {
    if let Some(tc) = tenant.as_ref().map(|extension| &extension.0.0) {
        if !tc.tenant_id.trim().is_empty() {
            return tc.tenant_id.trim().to_string();
        }
    }
    resolved
}

fn map_evidence_error(err: evidence::EvidenceError) -> ApiError {
    let message = err.to_string();
    match err {
        evidence::EvidenceError::BundleNotFound => ApiError::NotFound(message),
        evidence::EvidenceError::PermissionDenied | evidence::EvidenceError::CrossTenantAccess => {
            ApiError::Forbidden(message)
        }
        evidence::EvidenceError::InvalidScope => ApiError::BadRequest(message),
        // Fail closed: redaction could not guarantee secret removal.
        evidence::EvidenceError::RedactionFailed => ApiError::Unprocessable(message),
        evidence::EvidenceError::Collect(_) => ApiError::Internal(message),
    }
}

/// GET /v1/support/evidence-bundles (Go handleEvidenceBundles GET branch).
async fn list_bundles(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Query(query): Query<EvidenceQuery>,
) -> Result<Json<EvidenceBundleListResponse>, ApiError> {
    let manager = manager(&state)?;
    let tenant_id = context_tenant(&tenant, query.tenant_id.trim().to_string());
    let bundles = manager
        .list_for_tenant(&tenant_id, query.actor.trim())
        .map_err(map_evidence_error)?;
    Ok(Json(EvidenceBundleListResponse { items: bundles }))
}

/// POST /v1/support/evidence-bundles (Go handleEvidenceBundles POST branch)
/// — 201 with the redacted bundle.
async fn generate_bundle(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Query(query): Query<EvidenceQuery>,
    body: Bytes,
) -> Result<(StatusCode, Json<evidence::Bundle>), ApiError> {
    let request: GenerateEvidenceBundleRequest = decode_json_required(&body)?;
    let manager = manager(&state)?;
    let bundle = manager
        .generate(
            &context_tenant(&tenant, resolve(&request.tenant_id, &query.tenant_id)),
            &resolve(&request.actor, &query.actor),
            request.scope,
        )
        .map_err(map_evidence_error)?;
    Ok((StatusCode::CREATED, Json(bundle)))
}

/// GET /v1/support/evidence-bundles/{bundle_id} (Go
/// handleEvidenceBundleRoutes).
async fn get_bundle(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(bundle_id): Path<String>,
    Query(query): Query<EvidenceQuery>,
) -> Result<Json<evidence::Bundle>, ApiError> {
    let manager = manager(&state)?;
    let tenant_id = context_tenant(&tenant, query.tenant_id.trim().to_string());
    let bundle = manager
        .get(&tenant_id, query.actor.trim(), bundle_id.trim())
        .map_err(map_evidence_error)?;
    Ok(Json(bundle))
}

#[cfg(test)]
mod tests {
    use super::super::tests_support::{request_json, test_state};
    use axum::http::StatusCode;
    use std::sync::Arc;

    fn state_with_manager() -> crate::state::AppState {
        let mut state = test_state();
        state.evidence = Some(Arc::new(kura_evidence::Manager::new("test", None, None)));
        state
    }

    #[tokio::test]
    async fn generate_list_and_get_bundle() {
        let state = state_with_manager();
        let (status, generated) = request_json(
            state.clone(),
            "POST",
            "/v1/support/evidence-bundles",
            Some(serde_json::json!({
                "tenantId": "ten_a",
                "actor": "support_a",
                "scope": { "kind": "run", "ref": "run_1" }
            })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{generated}");
        let bundle_id = generated["bundleId"]
            .as_str()
            .expect("bundleId")
            .to_string();

        let (status, listed) = request_json(
            state.clone(),
            "GET",
            "/v1/support/evidence-bundles?tenantId=ten_a&actor=support_a",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{listed}");
        assert_eq!(listed["items"].as_array().expect("items").len(), 1);

        let (status, fetched) = request_json(
            state.clone(),
            "GET",
            &format!("/v1/support/evidence-bundles/{bundle_id}?tenantId=ten_a&actor=support_a"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{fetched}");

        // Cross-tenant access fails closed (Go 403).
        let (status, _) = request_json(
            state,
            "GET",
            &format!("/v1/support/evidence-bundles/{bundle_id}?tenantId=ten_b&actor=support_b"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn missing_bundle_is_404() {
        let (status, _) = request_json(
            state_with_manager(),
            "GET",
            "/v1/support/evidence-bundles/evidence_bundle_missing?tenantId=ten_a&actor=support_a",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
