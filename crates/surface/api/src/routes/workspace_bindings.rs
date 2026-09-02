//! workspace_bindings route family — port of
//! daemon/internal/api/workspace_bindings.go (workspace CRUD, binding rules,
//! and capability visibility), backed by the kura-store workspace/binding DAOs
//! (`crates/persistence/store`).
//!
//! Routes under /v1:
//! - GET/POST /v1/workspaces, GET/PATCH /v1/workspaces/{workspace_id}
//! - GET/POST /v1/bindings, GET/PATCH/DELETE /v1/bindings/{binding_id},
//!   POST /v1/bindings/{binding_id}/repair
//! - GET/PUT /v1/capability-visibility
//!
//! Permission gates mirror the Go handlers: reads need bindings.inspect,
//! mutations need bindings.manage; the denial body is the stable
//! credential_access_denied 403. Mutations append a binding/workspace
//! lifecycle event (or a validation_failed denial event) through the store
//! event table and publish to the bus, exactly like Go publishBindingEvent.

use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Router;
use axum::Json as AxumJson;
use serde::Serialize;
use std::collections::HashMap;

use kura_bindings::{
    BindingError, SetVisibilityRequest, VisibilityScopeKind, WorkspaceResource,
    to_binding_resource, to_capability_visibility_resource, to_workspace_resource,
};
use kura_identity::{has_permission, Permission};

use crate::error::ApiError;
use crate::middleware::TenantContext;
use crate::response::Json;
use crate::state::AppState;

/// Route family router (Go handleWorkspaceRoutes / handleBindingRoutes /
/// handleCapabilityVisibilityRoutes).
#[must_use]
pub fn router() -> Router<AppState> {
    Router::new()
        // Workspaces.
        .route("/v1/workspaces", get(list_workspaces).post(create_workspace))
        .route(
            "/v1/workspaces/{workspace_id}",
            get(get_workspace).patch(update_workspace),
        )
        // Binding rules.
        .route("/v1/bindings", get(list_bindings).post(create_binding))
        .route(
            "/v1/bindings/{binding_id}",
            get(get_binding).patch(update_binding).delete(delete_binding),
        )
        .route("/v1/bindings/{binding_id}/repair", post(repair_binding))
        // Capability visibility.
        .route(
            "/v1/capability-visibility",
            get(list_capability_visibility).put(set_capability_visibility),
        )
}

// ---------------------------------------------------------------------------
// Response DTOs (Go inline map[string]any bodies)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceListResponse {
    tenant_id: String,
    workspaces: Vec<WorkspaceResource>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BindingListResponse {
    tenant_id: String,
    bindings: Vec<kura_bindings::BindingResource>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CapabilityVisibilityListResponse {
    tenant_id: String,
    policies: Vec<kura_bindings::CapabilityVisibilityResource>,
}

// ---------------------------------------------------------------------------
// Workspaces
// ---------------------------------------------------------------------------

/// GET /v1/workspaces — tenant workspace list with a default-first order
/// (Go handleListWorkspaces).
async fn list_workspaces(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<WorkspaceListResponse>, ApiError> {
    let tc = require_binding_permission(tenant.as_ref().map(|e| &e.0), Permission::BindingsInspect)?;
    let limit = parse_limit(params.get("limit"));
    let items = state
        .store
        .lock()
        .list_workspaces(&tc.tenant_id, limit)
        .map_err(ApiError::from_store)?;
    let resources = items.iter().map(to_workspace_resource).collect();
    Ok(Json(WorkspaceListResponse { tenant_id: tc.tenant_id.clone(), workspaces: resources }))
}

/// GET /v1/workspaces/{workspace_id} — one workspace (Go handleGetWorkspace).
async fn get_workspace(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(workspace_id): Path<String>,
) -> Result<Json<WorkspaceResource>, ApiError> {
    let tc = require_binding_permission(tenant.as_ref().map(|e| &e.0), Permission::BindingsInspect)?;
    let ws = state
        .store
        .lock()
        .get_workspace(&tc.tenant_id, &workspace_id)
        .map_err(ApiError::from_store)?
        .ok_or_else(|| ApiError::NotFound("workspace not found".to_string()))?;
    Ok(Json(to_workspace_resource(&ws)))
}

/// POST /v1/workspaces — create a workspace (201; Go handleCreateWorkspace).
async fn create_workspace(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<(StatusCode, AxumJson<WorkspaceResource>), ApiError> {
    let tc = require_binding_permission(tenant.as_ref().map(|e| &e.0), Permission::BindingsManage)?;
    let req: kura_bindings::CreateWorkspaceRequest = decode_json_body(&body)?;
    let (ws, audit_id) = state
        .store
        .lock()
        .create_workspace(tc, &req.display_name)
        .map_err(map_binding_error)?;
    publish_binding_event(
        &state,
        kura_events::binding_lifecycle_event(kura_events::BindingLifecycleInput {
            tenant_id: tc.tenant_id.clone(),
            workspace_id: ws.workspace_id.clone(),
            actor_principal_id: tc.principal_id.clone(),
            event_name: "workspace.created".to_string(),
            outcome: "succeeded".to_string(),
            reason_code: "user_created_workspace".to_string(),
            permission_gate: permission_string(&Permission::BindingsManage),
            safe_summary: "Workspace created".to_string(),
            audit_event_id: audit_id,
            ..Default::default()
        }),
    )?;
    Ok((StatusCode::CREATED, AxumJson(to_workspace_resource(&ws))))
}

/// PATCH /v1/workspaces/{workspace_id} — archive/disable/reactivate a
/// workspace (Go handleUpdateWorkspace).
async fn update_workspace(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(workspace_id): Path<String>,
    body: Bytes,
) -> Result<Json<WorkspaceResource>, ApiError> {
    let tc = require_binding_permission(tenant.as_ref().map(|e| &e.0), Permission::BindingsManage)?;
    let req: kura_bindings::UpdateWorkspaceRequest = decode_json_body(&body)?;
    let (ws, audit_id) = state
        .store
        .lock()
        .update_workspace_status(tc, &workspace_id, req.status.clone())
        .map_err(map_binding_error)?;
    publish_binding_event(
        &state,
        kura_events::binding_lifecycle_event(kura_events::BindingLifecycleInput {
            tenant_id: tc.tenant_id.clone(),
            workspace_id: ws.workspace_id.clone(),
            actor_principal_id: tc.principal_id.clone(),
            event_name: format!("workspace.{}", ws.status.as_str()),
            outcome: "succeeded".to_string(),
            reason_code: "user_updated_workspace".to_string(),
            permission_gate: permission_string(&Permission::BindingsManage),
            safe_summary: "Workspace updated".to_string(),
            audit_event_id: audit_id,
            ..Default::default()
        }),
    )?;
    Ok(Json(to_workspace_resource(&ws)))
}

// ---------------------------------------------------------------------------
// Binding rules
// ---------------------------------------------------------------------------

/// GET /v1/bindings — tenant binding rules with fresh repair status
/// (Go handleListBindings).
async fn list_bindings(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<BindingListResponse>, ApiError> {
    let tc = require_binding_permission(tenant.as_ref().map(|e| &e.0), Permission::BindingsInspect)?;
    let limit = parse_limit(params.get("limit"));
    let items = state
        .store
        .lock()
        .list_binding_rules(&tc.tenant_id, limit)
        .map_err(ApiError::from_store)?;
    let resources = items.iter().map(to_binding_resource).collect();
    Ok(Json(BindingListResponse { tenant_id: tc.tenant_id.clone(), bindings: resources }))
}

/// GET /v1/bindings/{binding_id} — one binding rule (Go handleGetBinding).
async fn get_binding(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(binding_id): Path<String>,
) -> Result<Json<kura_bindings::BindingResource>, ApiError> {
    let tc = require_binding_permission(tenant.as_ref().map(|e| &e.0), Permission::BindingsInspect)?;
    let rule = state
        .store
        .lock()
        .get_binding_rule(&tc.tenant_id, &binding_id)
        .map_err(ApiError::from_store)?
        .ok_or_else(|| ApiError::NotFound("binding not found".to_string()))?;
    Ok(Json(to_binding_resource(&rule)))
}

/// POST /v1/bindings — create a binding rule (201; Go handleCreateBinding).
/// Validation failures publish a `binding.validation_failed` denial event.
async fn create_binding(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<(StatusCode, AxumJson<kura_bindings::BindingResource>), ApiError> {
    let tc = require_binding_permission(tenant.as_ref().map(|e| &e.0), Permission::BindingsManage)?;
    let req: kura_bindings::CreateBindingRequest = decode_json_body(&body)?;
    // The store guard must be dropped before the denied event publishes
    // (parking_lot is not reentrant).
    let created = {
        let store = state.store.lock();
        store.create_binding_rule(tc, &req)
    };
    let (rule, audit_id) = match created {
        Ok(result) => result,
        Err(message) => {
            // Go: a failed create publishes a denied lifecycle event first.
            publish_binding_event(
                &state,
                kura_events::binding_lifecycle_event(kura_events::BindingLifecycleInput {
                    tenant_id: tc.tenant_id.clone(),
                    actor_principal_id: tc.principal_id.clone(),
                    event_name: "binding.validation_failed".to_string(),
                    outcome: "denied".to_string(),
                    reason_code: binding_reason_code(&message),
                    permission_gate: permission_string(&Permission::BindingsManage),
                    safe_summary: "Binding validation failed".to_string(),
                    ..Default::default()
                }),
            )?;
            return Err(map_binding_error(message));
        }
    };
    publish_binding_event(
        &state,
        kura_events::binding_lifecycle_event(kura_events::BindingLifecycleInput {
            tenant_id: tc.tenant_id.clone(),
            binding_id: rule.binding_id.clone(),
            actor_principal_id: tc.principal_id.clone(),
            event_name: "binding.created".to_string(),
            outcome: "succeeded".to_string(),
            reason_code: "user_created_binding".to_string(),
            permission_gate: permission_string(&Permission::BindingsManage),
            safe_summary: "Binding created".to_string(),
            resulting_selection_summary: rule.resulting_selection_summary.clone(),
            audit_event_id: audit_id,
            ..Default::default()
        }),
    )?;
    Ok((StatusCode::CREATED, AxumJson(to_binding_resource(&rule))))
}

/// PATCH /v1/bindings/{binding_id} — update or disable a binding rule
/// (Go handleUpdateBinding).
async fn update_binding(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(binding_id): Path<String>,
    body: Bytes,
) -> Result<Json<kura_bindings::BindingResource>, ApiError> {
    let tc = require_binding_permission(tenant.as_ref().map(|e| &e.0), Permission::BindingsManage)?;
    let req: kura_bindings::UpdateBindingRequest = decode_json_body(&body)?;
    let (rule, audit_id) = state
        .store
        .lock()
        .update_binding_rule(tc, &binding_id, &req)
        .map_err(map_binding_error)?;
    let event_name = if rule.status == kura_bindings::BindingStatus::DISABLED {
        "binding.disabled"
    } else {
        "binding.updated"
    };
    publish_binding_event(
        &state,
        kura_events::binding_lifecycle_event(kura_events::BindingLifecycleInput {
            tenant_id: tc.tenant_id.clone(),
            binding_id: rule.binding_id.clone(),
            actor_principal_id: tc.principal_id.clone(),
            event_name: event_name.to_string(),
            outcome: "succeeded".to_string(),
            reason_code: "user_updated_binding".to_string(),
            permission_gate: permission_string(&Permission::BindingsManage),
            safe_summary: "Binding updated".to_string(),
            previous_selection_summary: rule.previous_selection_summary.clone(),
            resulting_selection_summary: rule.resulting_selection_summary.clone(),
            audit_event_id: audit_id,
            ..Default::default()
        }),
    )?;
    Ok(Json(to_binding_resource(&rule)))
}

/// DELETE /v1/bindings/{binding_id} — remove a binding rule (204;
/// Go handleDeleteBinding).
async fn delete_binding(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(binding_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let tc = require_binding_permission(tenant.as_ref().map(|e| &e.0), Permission::BindingsManage)?;
    let audit_id = state
        .store
        .lock()
        .remove_binding_rule(tc, &binding_id)
        .map_err(map_binding_error)?;
    publish_binding_event(
        &state,
        kura_events::binding_lifecycle_event(kura_events::BindingLifecycleInput {
            tenant_id: tc.tenant_id.clone(),
            binding_id: binding_id.clone(),
            actor_principal_id: tc.principal_id.clone(),
            event_name: "binding.removed".to_string(),
            outcome: "succeeded".to_string(),
            reason_code: "user_removed_binding".to_string(),
            permission_gate: permission_string(&Permission::BindingsManage),
            safe_summary: "Binding removed".to_string(),
            audit_event_id: audit_id,
            ..Default::default()
        }),
    )?;
    Ok(StatusCode::NO_CONTENT)
}

/// POST /v1/bindings/{binding_id}/repair — recompute the binding repair
/// status (Go handleRepairBinding).
async fn repair_binding(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(binding_id): Path<String>,
) -> Result<Json<kura_bindings::BindingResource>, ApiError> {
    let tc = require_binding_permission(tenant.as_ref().map(|e| &e.0), Permission::BindingsManage)?;
    let (rule, audit_id) = state
        .store
        .lock()
        .repair_binding_rule(tc, &binding_id)
        .map_err(map_binding_error)?;
    publish_binding_event(
        &state,
        kura_events::binding_lifecycle_event(kura_events::BindingLifecycleInput {
            tenant_id: tc.tenant_id.clone(),
            binding_id: rule.binding_id.clone(),
            actor_principal_id: tc.principal_id.clone(),
            event_name: "binding.repaired".to_string(),
            outcome: "succeeded".to_string(),
            reason_code: "user_repaired_binding".to_string(),
            permission_gate: permission_string(&Permission::BindingsManage),
            safe_summary: "Binding repair evaluated".to_string(),
            audit_event_id: audit_id,
            ..Default::default()
        }),
    )?;
    Ok(Json(to_binding_resource(&rule)))
}

// ---------------------------------------------------------------------------
// Capability visibility
// ---------------------------------------------------------------------------

/// GET /v1/capability-visibility?scopeKind=profile|workspace&scopeRef=...
/// (Go handleListCapabilityVisibility).
async fn list_capability_visibility(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<CapabilityVisibilityListResponse>, ApiError> {
    let tc = require_binding_permission(tenant.as_ref().map(|e| &e.0), Permission::BindingsInspect)?;
    let scope_kind = VisibilityScopeKind::new(params.get("scopeKind").cloned().unwrap_or_default().trim());
    if scope_kind != VisibilityScopeKind::PROFILE && scope_kind != VisibilityScopeKind::WORKSPACE {
        return Err(ApiError::BadRequest("scopeKind must be profile or workspace".to_string()));
    }
    let scope_ref = params.get("scopeRef").cloned().unwrap_or_default().trim().to_string();
    let items = state
        .store
        .lock()
        .list_capability_visibility(&tc.tenant_id, &scope_kind, &scope_ref)
        .map_err(ApiError::from_store)?;
    let resources = items.iter().map(to_capability_visibility_resource).collect();
    Ok(Json(CapabilityVisibilityListResponse { tenant_id: tc.tenant_id.clone(), policies: resources }))
}

/// PUT /v1/capability-visibility — set one capability visibility policy
/// (Go handleSetCapabilityVisibility).
async fn set_capability_visibility(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<Json<kura_bindings::CapabilityVisibilityResource>, ApiError> {
    let tc = require_binding_permission(tenant.as_ref().map(|e| &e.0), Permission::BindingsManage)?;
    let req: SetVisibilityRequest = decode_json_body(&body)?;
    let (policy, audit_id) = state
        .store
        .lock()
        .set_capability_visibility(tc, &req)
        .map_err(map_binding_error)?;
    publish_binding_event(
        &state,
        kura_events::capability_visibility_changed_event(
            kura_events::CapabilityVisibilityChangedInput {
                tenant_id: tc.tenant_id.clone(),
                actor_principal_id: tc.principal_id.clone(),
                scope_kind: policy.scope_kind.clone(),
                scope_ref: policy.scope_ref.clone(),
                capability_id: policy.capability_id.clone(),
                visibility: policy.visibility.clone(),
                audit_event_id: audit_id,
            },
        ),
    )?;
    Ok(Json(to_capability_visibility_resource(&policy)))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Go requireBindingPermission: resolved tenant context + the given
/// permission, else the stable credential denial (403).
fn require_binding_permission(
    tenant: Option<&TenantContext>,
    permission: Permission,
) -> Result<&kura_identity::TenantContext, ApiError> {
    let Some(tc) = tenant else {
        return Err(credential_denial());
    };
    if tc.0.tenant_id.trim().is_empty() || !has_permission(&tc.0.permissions, permission) {
        return Err(credential_denial());
    }
    Ok(&tc.0)
}

/// Go writeTenantDenial(403): the stable credential_access_denied body.
fn credential_denial() -> ApiError {
    ApiError::Forbidden("credential_access_denied".to_string())
}

/// Go decodeJSONBody: empty body -> 400 "request body is required".
fn decode_json_body<T: serde::de::DeserializeOwned>(body: &Bytes) -> Result<T, ApiError> {
    if body.is_empty() {
        return Err(ApiError::BadRequest("request body is required".to_string()));
    }
    serde_json::from_slice(body).map_err(|err| ApiError::BadRequest(err.to_string()))
}

/// Go `string(permission)` — the stable wire literal for a permission
/// (channel_management.rs precedent).
fn permission_string(permission: &Permission) -> String {
    serde_json::to_string(permission)
        .map(|s| s.trim_matches('"').to_string())
        .unwrap_or_else(|_| format!("{permission:?}"))
}

/// Go limit parse: unparseable/zero -> 0 (the store applies its default).
fn parse_limit(raw: Option<&String>) -> i64 {
    match raw {
        Some(value) if !value.trim().is_empty() => value.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

/// Go writeBindingError: sentinel mapping for the store error strings.
fn map_binding_error(message: String) -> ApiError {
    if message == "binding not found" {
        return ApiError::NotFound("binding not found".to_string());
    }
    if message == "workspace not found" {
        return ApiError::NotFound("workspace not found".to_string());
    }
    if message == BindingError::ExplicitActorRequired.to_string() {
        return credential_denial();
    }
    if message == BindingError::InvalidBinding.to_string()
        || message.starts_with("binding validation failed")
    {
        return ApiError::BadRequest(message);
    }
    ApiError::from_store(message)
}

/// Go reasonCodeForBindingError: the stable reason code for a failed binding
/// mutation (used in the validation_failed denial event).
fn binding_reason_code(message: &str) -> String {
    if message == BindingError::InvalidBinding.to_string() {
        return "binding_validation_failed".to_string();
    }
    if let Some(code) = message.strip_prefix("binding validation failed: ") {
        let code = code.trim();
        if code.is_empty() {
            return "binding_validation_failed".to_string();
        }
        return code.to_string();
    }
    if message == "binding not found" {
        return "binding_not_found".to_string();
    }
    if message == BindingError::ExplicitActorRequired.to_string() {
        return "explicit_actor_required".to_string();
    }
    "binding_operation_failed".to_string()
}

/// Go publishBindingEvent (bus path): append to the store event table, then
/// publish to the in-process bus. The environment scope is filled from the
/// daemon config when the builder left it empty.
fn publish_binding_event(state: &AppState, event: kura_events::Event) -> Result<(), ApiError> {
    let mut event = event;
    if event.environment_scope.is_empty() {
        event.environment_scope = crate::middleware::environment_scope_from_config(&state.config);
    }
    let stored = state
        .store
        .lock()
        .append_event(&event)
        .map_err(ApiError::from_store)?;
    state.event_bus.publish(stored);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use axum::http::header::CONTENT_TYPE;
    use kura_identity::TenantContext as IdentityTenantContext;
    use parking_lot::Mutex;
    use tower::ServiceExt;
    use uuid::Uuid;

    fn test_config() -> kura_config::Config {
        kura_config::Config {
            project_root: String::new(),
            environment: kura_config::Environment::Test,
            bind_addr: "127.0.0.1:19192".to_string(),
            data_dir: "/tmp/kura-api-workspace-bindings".to_string(),
            log_level: "info".to_string(),
            version: "0.1.0".to_string(),
            llm: kura_config::LlmConfig::default(),
            connectors: kura_config::ConnectorConfig {
                discord: kura_config::DiscordConnectorConfig { enabled: false, ..Default::default() },
                telegram: kura_config::TelegramConnectorConfig { enabled: false, ..Default::default() },
                slack: kura_config::SlackConnectorConfig { enabled: false, ..Default::default() },
                matrix: kura_config::MatrixConnectorConfig { enabled: false, ..Default::default() },
            },
        }
    }

    fn test_state() -> AppState {
        let dir = std::env::temp_dir().join(format!("kura-api-workspace-bindings-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let store = Arc::new(Mutex::new(
            kura_store::SQLiteStore::new(dir.to_str().expect("path")).expect("store"),
        ));
        AppState::new(test_config(), Arc::new(kura_events::Bus::new()), store)
    }

    fn request(method: &str, uri: &str, body: Option<&str>) -> Request<Body> {
        let builder = Request::builder()
            .method(method)
            .uri(uri)
            .header(CONTENT_TYPE, "application/json");
        let req = match body {
            Some(payload) => builder.body(Body::from(payload.to_string())).expect("request"),
            None => builder.body(Body::empty()).expect("request"),
        };
        req
    }

    fn tenant_request(
        method: &str,
        uri: &str,
        body: Option<&str>,
        tenant_id: &str,
        permissions: Vec<Permission>,
    ) -> Request<Body> {
        let mut req = request(method, uri, body);
        req.extensions_mut().insert(TenantContext(IdentityTenantContext {
            tenant_id: tenant_id.to_string(),
            principal_id: format!("prn_{tenant_id}"),
            permissions,
            ..Default::default()
        }));
        req
    }

    async fn send(app: &axum::Router, req: Request<Body>) -> (StatusCode, serde_json::Value) {
        let response = app.clone().oneshot(req).await.expect("oneshot");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.expect("body");
        let json = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("json body")
        };
        (status, json)
    }

    // Port of TestWorkspaceBindingAPILifecycleAndPermissions.
    #[tokio::test]
    async fn workspace_binding_lifecycle_and_permissions() {
        let state = test_state();
        {
            let store = state.store.lock();
            store
                .ensure_default_agent_profile("ten_bindings")
                .expect("default profile");
        }
        let app = crate::routes::router(state.clone());
        let admin = vec![Permission::BindingsInspect, Permission::BindingsManage];
        let viewer = Vec::new();

        // List workspaces (inspect) lazily provisions the default.
        let (status, json) = send(
            &app,
            tenant_request("GET", "/v1/workspaces", None, "ten_bindings", admin.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "list workspaces: {json}");
        let workspaces = json["workspaces"].as_array().expect("workspaces array");
        assert_eq!(workspaces.len(), 1);
        assert_eq!(workspaces[0]["isDefault"], true);
        let default_workspace_id = workspaces[0]["workspaceId"].as_str().expect("id").to_string();

        // Create a binding (manage).
        let profile_id = {
            let store = state.store.lock();
            store
                .active_agent_profile_selection("ten_bindings")
                .expect("selection")
                .expect("found")
                .0
                .profile_id
        };
        let body = serde_json::json!({
            "scopeKind": "channel",
            "scopeRef": "discord:c1",
            "selectedProfileId": profile_id,
            "selectedWorkspaceId": default_workspace_id,
        })
        .to_string();
        let (status, json) = send(
            &app,
            tenant_request("POST", "/v1/bindings", Some(&body), "ten_bindings", admin.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "create binding: {json}");
        let binding_id = json["bindingId"].as_str().expect("binding id").to_string();

        // Create denied for a viewer (no existence leak — pure 403).
        let (status, _) = send(
            &app,
            tenant_request("POST", "/v1/bindings", Some(&body), "ten_bindings", viewer.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        // Inspect denied for a viewer.
        let (status, _) = send(
            &app,
            tenant_request("GET", &format!("/v1/bindings/{binding_id}"), None, "ten_bindings", viewer.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        // Disable the binding.
        let (status, json) = send(
            &app,
            tenant_request("PATCH", &format!("/v1/bindings/{binding_id}"), Some(r#"{"disable":true}"#), "ten_bindings", admin.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "disable: {json}");
        assert_eq!(json["status"], "disabled");

        // Set capability visibility (manage) at workspace scope.
        let vis_body = serde_json::json!({
            "scopeKind": "workspace",
            "scopeRef": default_workspace_id,
            "capabilityId": "tool.shell",
            "visibility": "hidden",
        })
        .to_string();
        let (status, json) = send(
            &app,
            tenant_request("PUT", "/v1/capability-visibility", Some(&vis_body), "ten_bindings", admin.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "set visibility: {json}");
        assert_eq!(json["visibility"], "hidden");

        // List capability visibility.
        let (status, json) = send(
            &app,
            tenant_request(
                "GET",
                &format!("/v1/capability-visibility?scopeKind=workspace&scopeRef={default_workspace_id}"),
                None,
                "ten_bindings",
                admin.clone(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "list visibility: {json}");
        assert_eq!(json["policies"].as_array().map(|v| v.len()), Some(1));

        // Remove the binding (204).
        let (status, _) = send(
            &app,
            tenant_request("DELETE", &format!("/v1/bindings/{binding_id}"), None, "ten_bindings", admin.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        // Binding lifecycle events were recorded.
        let events = state
            .event_bus
            .list(&kura_events::Filter { category: "binding".to_string(), ..Default::default() });
        assert!(!events.is_empty(), "expected binding lifecycle events");
    }

    // Port of TestWorkspaceBindingAPIValidationFailure.
    #[tokio::test]
    async fn binding_create_validation_failure_is_safe() {
        let state = test_state();
        let app = crate::routes::router(state.clone());
        let admin = vec![Permission::BindingsInspect, Permission::BindingsManage];
        let (status, json) = send(
            &app,
            tenant_request(
                "POST",
                "/v1/bindings",
                Some(r#"{"scopeKind":"channel","scopeRef":"discord:x","selectedProfileId":"prof_missing"}"#),
                "ten_bindings",
                admin,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "validation body: {json}");
        assert!(
            json.to_string().contains("selected_profile_unavailable"),
            "expected safe reason code: {json}"
        );
    }

    // Missing binding / workspace answers 404 without leaking.
    #[tokio::test]
    async fn missing_resources_answer_404() {
        let state = test_state();
        let app = crate::routes::router(state.clone());
        let admin = vec![Permission::BindingsInspect, Permission::BindingsManage];
        let (status, json) = send(
            &app,
            tenant_request("GET", "/v1/bindings/b_missing", None, "ten_bindings", admin.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "body: {json}");
        assert_eq!(json["message"], "binding not found");

        let (status, json) = send(
            &app,
            tenant_request("GET", "/v1/workspaces/ws_missing", None, "ten_bindings", admin.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "body: {json}");
        assert_eq!(json["message"], "workspace not found");
    }

    // Cross-tenant scoping: another tenant never sees the records.
    #[tokio::test]
    async fn cross_tenant_lookups_are_scoped() {
        let state = test_state();
        {
            let store = state.store.lock();
            store.ensure_default_agent_profile("ten_a").expect("profile a");
            store.ensure_default_agent_profile("ten_b").expect("profile b");
        }
        let app = crate::routes::router(state.clone());
        let admin_a = vec![Permission::BindingsInspect, Permission::BindingsManage];
        let admin_b = vec![Permission::BindingsInspect, Permission::BindingsManage];

        let (status, json) = send(
            &app,
            tenant_request("GET", "/v1/workspaces", None, "ten_a", admin_a.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let ws_id = json["workspaces"][0]["workspaceId"].as_str().expect("id").to_string();

        // ten_b cannot read ten_a workspace.
        let (status, _) = send(
            &app,
            tenant_request("GET", &format!("/v1/workspaces/{ws_id}"), None, "ten_b", admin_b.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}