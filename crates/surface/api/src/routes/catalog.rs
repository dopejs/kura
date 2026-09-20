//! operator catalog route family (port of daemon/internal/api/catalog.go,
//! Roadmap 68).
//!
//! Routes: GET/POST /v1/catalog/items, GET /v1/catalog/items/{item_id}
//! (tenant-scoped inspection), and POST
//! /v1/catalog/items/{item_id}/{enable|disable|rollback}. The tenant comes
//! from the request body when present, falling back to the `tenantId` query
//! parameter (Go catalogTenant). Error mapping preserves Go
//! writeCatalogError: item/version/rollback-target not found -> 404,
//! permission denied -> 403, unmet requirements / invalid item -> 400.

use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use kura_catalog as catalog;

use crate::error::ApiError;
use crate::middleware::TenantContext;
use crate::state::AppState;

use super::{decode_json_or_default, decode_json_required};

/// Route family router.
#[must_use]
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/catalog/items", get(list_items).post(register_item))
        .route("/v1/catalog/items/{item_id}", get(inspect_item))
        .route(
            "/v1/catalog/items/{item_id}/{action}",
            axum::routing::post(enablement_action),
        )
}

/// Go RegisterCatalogItemRequest.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct RegisterCatalogItemRequest {
    item: catalog::CatalogItem,
}

/// Go CatalogEnablementRequest.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct CatalogEnablementRequest {
    tenant_id: String,
    version: String,
    actor: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct TenantQuery {
    tenant_id: String,
}

#[derive(Debug, Serialize)]
struct CatalogItemListResponse {
    items: Vec<catalog::CatalogItem>,
}

fn manager(state: &AppState) -> Result<&catalog::Manager, ApiError> {
    state
        .catalog
        .as_deref()
        .ok_or_else(|| ApiError::internal("catalog manager is not configured"))
}

/// Go catalogTenant + the tenant-context override: a resolved tenant context
/// wins over the body tenant, which wins over ?tenantId=.
fn resolve_tenant(
    tenant: &Option<Extension<TenantContext>>,
    body_tenant: &str,
    query: &TenantQuery,
) -> String {
    if let Some(tc) = tenant.as_ref().map(|extension| &extension.0.0) {
        if !tc.tenant_id.trim().is_empty() {
            return tc.tenant_id.trim().to_string();
        }
    }
    let trimmed = body_tenant.trim();
    if trimmed.is_empty() {
        query.tenant_id.trim().to_string()
    } else {
        trimmed.to_string()
    }
}

fn map_catalog_error(err: catalog::CatalogError) -> ApiError {
    let message = err.to_string();
    match err {
        catalog::CatalogError::ItemNotFound
        | catalog::CatalogError::VersionNotFound
        | catalog::CatalogError::NoRollbackTarget => ApiError::NotFound(message),
        catalog::CatalogError::PermissionDenied => ApiError::Forbidden(message),
        catalog::CatalogError::RequirementsUnmet | catalog::CatalogError::InvalidCatalogItem => {
            ApiError::BadRequest(message)
        }
    }
}

/// GET /v1/catalog/items (Go handleCatalogItems GET branch).
async fn list_items(
    State(state): State<AppState>,
) -> Result<Json<CatalogItemListResponse>, ApiError> {
    let manager = manager(&state)?;
    Ok(Json(CatalogItemListResponse {
        items: manager.list_items(),
    }))
}

/// POST /v1/catalog/items (Go handleCatalogItems POST branch) — 201.
async fn register_item(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<(StatusCode, Json<catalog::CatalogItem>), ApiError> {
    let request: RegisterCatalogItemRequest = decode_json_required(&body)?;
    let manager = manager(&state)?;
    let item = manager
        .register_item(request.item)
        .map_err(map_catalog_error)?;
    Ok((StatusCode::CREATED, Json(item)))
}

/// GET /v1/catalog/items/{item_id} (Go handleCatalogItemRoutes inspect
/// branch) — the tenant-scoped inspection projection.
async fn inspect_item(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(item_id): Path<String>,
    Query(query): Query<TenantQuery>,
) -> Result<Json<catalog::Inspection>, ApiError> {
    let manager = manager(&state)?;
    let inspection = manager
        .inspect(&resolve_tenant(&tenant, "", &query), item_id.trim())
        .map_err(map_catalog_error)?;
    Ok(Json(inspection))
}

/// POST /v1/catalog/items/{item_id}/{enable|disable|rollback} (Go
/// handleCatalogItemRoutes enablement branch); an empty body is tolerated and
/// unknown actions are 404.
async fn enablement_action(
    State(state): State<AppState>,
    tenant_context: Option<Extension<TenantContext>>,
    Path((item_id, action)): Path<(String, String)>,
    Query(query): Query<TenantQuery>,
    body: Bytes,
) -> Result<Json<catalog::Enablement>, ApiError> {
    let request: CatalogEnablementRequest = decode_json_or_default(&body)?;
    let manager = manager(&state)?;
    let tenant = resolve_tenant(&tenant_context, &request.tenant_id, &query);
    let item_id = item_id.trim();
    let actor = request.actor.trim();
    let enablement = match action.as_str() {
        "enable" => manager.enable(&tenant, item_id, request.version.trim(), actor),
        "disable" => manager.disable(&tenant, item_id, actor),
        "rollback" => manager.rollback(&tenant, item_id, actor),
        _ => return Err(ApiError::NotFound("not found".to_string())),
    }
    .map_err(map_catalog_error)?;
    Ok(Json(enablement))
}

#[cfg(test)]
mod tests {
    use super::super::tests_support::{request_json, test_state};
    use axum::http::StatusCode;
    use std::sync::Arc;

    fn state_with_manager() -> crate::state::AppState {
        let mut state = test_state();
        state.catalog = Some(Arc::new(kura_catalog::Manager::new("test", None, None)));
        state
    }

    fn item_body() -> serde_json::Value {
        serde_json::json!({
            "item": {
                "itemId": "cat_item_summarize",
                "kind": "skill",
                "name": "summarize",
                "trustTier": "official",
                "versions": [
                    {
                        "version": "1.0.0",
                        "source": "registry://skills/summarize",
                        "publishedAt": "2026-08-01T00:00:00Z"
                    }
                ],
                "createdAt": "2026-08-01T00:00:00Z",
                "updatedAt": "2026-08-01T00:00:00Z"
            }
        })
    }

    #[tokio::test]
    async fn register_list_inspect_and_enable_item() {
        let state = state_with_manager();
        let (status, registered) = request_json(
            state.clone(),
            "POST",
            "/v1/catalog/items",
            Some(item_body()),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{registered}");
        let item_id = registered["itemId"].as_str().expect("itemId").to_string();

        let (status, listed) = request_json(state.clone(), "GET", "/v1/catalog/items", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(listed["items"].as_array().expect("items").len(), 1);

        let (status, inspected) = request_json(
            state.clone(),
            "GET",
            &format!("/v1/catalog/items/{item_id}?tenantId=ten_a"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{inspected}");

        let (status, enabled) = request_json(
            state,
            "POST",
            &format!("/v1/catalog/items/{item_id}/enable"),
            Some(serde_json::json!({
                "tenantId": "ten_a",
                "version": "1.0.0",
                "actor": "operator_a"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{enabled}");
        assert_eq!(enabled["tenantId"], "ten_a");
    }

    #[tokio::test]
    async fn missing_item_is_404() {
        let (status, _) = request_json(
            state_with_manager(),
            "GET",
            "/v1/catalog/items/cat_item_missing?tenantId=ten_a",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
