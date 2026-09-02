//! mcp + skills + webhook route families (port of the /v1/mcp*,
//! /v1/skills*, and /v1/webhooks* registrations and handlers in
//! daemon/internal/api/server.go and webhook.go).

use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::http::header::HeaderMap;
use axum::routing::{get, patch, post};
use axum::Router;
use serde::{Deserialize, Serialize};

use kura_identity::{has_permission, Permission};
use kura_mcp as mcp;
use kura_webhook as webhook;

use crate::error::ApiError;
use crate::middleware::TenantContext;
use crate::response::Json;
use crate::state::AppState;
use crate::types::{
    ListResponse, SkillDetailResponse, SkillFileResponse, SkillOverlayResponse,
    SkillRegistryResponse, SkillSummaryResponse,
};

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

/// Go CreateWebhookRequest.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateWebhookRequest {
    #[serde(default)]
    tenant_id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    target_kind: webhook::TargetKind,
    #[serde(default)]
    target_ref: String,
}

/// Go WebhookTenantRequest.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WebhookTenantRequest {
    #[serde(default)]
    tenant_id: String,
}

/// Go WebhookListResponse.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct WebhookListResponse {
    items: Vec<webhook::Endpoint>,
}

/// Query params for the webhook family (Go webhookTenant reads the tenantId
/// query parameter).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WebhookListQuery {
    #[serde(default)]
    tenant_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Route family router. Only the methods the Go handlers accept are registered;
/// axum answers the other methods with 405.
#[must_use]
pub fn router() -> Router<AppState> {
    Router::new()
        // MCP servers / transports / catalog
        .route("/v1/mcp/servers", get(list_mcp_servers).post(create_mcp_server))
        .route(
            "/v1/mcp/servers/{server_id}",
            get(get_mcp_server).patch(update_mcp_server),
        )
        .route("/v1/mcp/servers/{server_id}/start", post(mcp_server_start))
        .route("/v1/mcp/servers/{server_id}/refresh", post(mcp_server_refresh))
        .route(
            "/v1/mcp/servers/{server_id}/reinstall",
            post(mcp_server_reinstall),
        )
        .route(
            "/v1/mcp/servers/{server_id}/uninstall",
            post(mcp_server_uninstall),
        )
        .route(
            "/v1/mcp/servers/{server_id}/revalidate",
            post(mcp_server_revalidate),
        )
        .route("/v1/mcp/servers/{server_id}/stop", post(mcp_server_stop))
        .route("/v1/mcp/servers/{server_id}/restart", post(mcp_server_restart))
        .route("/v1/mcp/servers/{server_id}/cancel", post(mcp_server_cancel))
        .route("/v1/mcp/servers/{server_id}/tools", get(mcp_server_tools))
        .route(
            "/v1/mcp/servers/{server_id}/tools/{tool_name}",
            patch(mcp_server_tool_exposure),
        )
        .route(
            "/v1/mcp/servers/{server_id}/tools/{tool_name}/authorize",
            post(mcp_server_tool_authorize),
        )
        .route("/v1/mcp/transports", get(mcp_transports))
        .route("/v1/mcp/catalog", get(mcp_catalog))
        .route("/v1/mcp/catalog/{entry_id}", get(mcp_catalog_entry))
        .route(
            "/v1/mcp/catalog/{entry_id}/install",
            post(mcp_catalog_install),
        )
        // Skills
        .route("/v1/skills", get(skills_list))
        .route("/v1/skills/reload", post(skills_reload))
        .route("/v1/skills/{skill_id}", get(skill_detail))
        // Webhooks (CRUD; the signature-authenticated ingress lives in
        // ingress_router so it stays outside the protected() layer)
        .route("/v1/webhooks", get(list_webhooks).post(create_webhook))
        .route("/v1/webhooks/{webhook_id}", get(get_webhook))
        .route("/v1/webhooks/{webhook_id}/rotate", post(rotate_webhook))
        .route("/v1/webhooks/{webhook_id}/disable", post(disable_webhook))
}

/// The webhook trigger ingress. Go registers it without protected(): callers
/// authenticate with the per-webhook HMAC signature, not an operator token.
#[must_use]
pub fn ingress_router() -> Router<AppState> {
    Router::new().route("/v1/triggers/webhook/{webhook_id}", post(trigger_webhook))
}
// ---------------------------------------------------------------------------
// MCP servers
// ---------------------------------------------------------------------------

/// GET /v1/mcp/servers — server list, tenant-scoped when the caller carries a
/// resolved tenant context (Go handleMCPServers GET).
async fn list_mcp_servers(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Json<ListResponse<mcp::ServerResource>>, ApiError> {
    let manager = mcp_manager(&state)?;
    let items = match tenant.as_ref().map(|e| &e.0.0) {
        Some(tc) if !tc.tenant_id.trim().is_empty() => manager.list_servers_for_tenant(&tc.tenant_id),
        _ => manager.list_servers(),
    };
    Ok(Json(ListResponse { items }))
}

/// POST /v1/mcp/servers — create a server (201) or update in place (200).
/// Tenant contexts need mcp.manage (Go requireMCPPermissionIfTenant).
async fn create_mcp_server(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<(StatusCode, Json<mcp::ServerResource>), ApiError> {
    let manager = mcp_manager(&state)?;
    require_mcp_manage(tenant.as_ref().map(|e| &e.0.0))?;
    let mut input: mcp::CreateServerInput = decode_json_body(&body)?;
    // From the request, never from the body: a caller must not be able to
    // create a server belonging to someone else by saying so.
    input.tenant_id = tenant
        .as_ref()
        .map(|extension| extension.0.0.tenant_id.clone())
        .unwrap_or_default();
    let (resource, created) = manager.create_server(input).map_err(map_mcp_error)?;
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(resource)))
}

/// GET /v1/mcp/servers/{server_id} — server resource (Go handleMCPServerByID).
async fn get_mcp_server(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(server_id): Path<String>,
) -> Result<Json<mcp::ServerResource>, ApiError> {
    let manager = mcp_manager(&state)?;
    let resource = mcp_server_resource_for_request(manager, tenant.as_ref().map(|e| &e.0.0), &server_id)
        .ok_or_else(|| ApiError::NotFound("not found".to_string()))?;
    Ok(Json(resource))
}

/// PATCH /v1/mcp/servers/{server_id} — partial server update.
async fn update_mcp_server(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(server_id): Path<String>,
    body: Bytes,
) -> Result<Json<mcp::ServerResource>, ApiError> {
    let manager = mcp_manager(&state)?;
    if mcp_server_resource_for_request(manager, tenant.as_ref().map(|e| &e.0.0), &server_id).is_none() {
        return Err(ApiError::NotFound("not found".to_string()));
    }
    require_mcp_manage(tenant.as_ref().map(|e| &e.0.0))?;
    let input: mcp::UpdateServerInput = decode_json_body(&body)?;
    let resource = manager
        .update_server(&server_id, &input)
        .map_err(map_mcp_error)?;
    Ok(Json(resource))
}

/// POST /v1/mcp/servers/{server_id}/start — lifecycle start (Go
/// handleMCPServerStart).
async fn mcp_server_start(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(server_id): Path<String>,
) -> Result<Json<mcp::LifecycleResponse>, ApiError> {
    let manager = mcp_manager(&state)?;
    ensure_mcp_server_route_access(manager, tenant.as_ref().map(|e| &e.0.0), &server_id, true)?;
    let response = manager
        .start(&server_id, &current_actor(tenant.as_ref().map(|e| &e.0.0)))
        .map_err(map_mcp_error)?;
    Ok(Json(response))
}

/// POST /v1/mcp/servers/{server_id}/refresh — catalog refresh (200 when
/// completed, 409 when blocked/conflicted — Go handleMCPServerRefresh).
async fn mcp_server_refresh(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(server_id): Path<String>,
) -> Result<(StatusCode, Json<mcp::CatalogLifecycleResult>), ApiError> {
    let manager = mcp_manager(&state)?;
    ensure_mcp_server_route_access(manager, tenant.as_ref().map(|e| &e.0.0), &server_id, true)?;
    let response = manager
        .refresh_catalog_server(&server_id)
        .map_err(map_mcp_error)?;
    let status = lifecycle_status(&response.status);
    Ok((status, Json(response)))
}

/// POST /v1/mcp/servers/{server_id}/reinstall — catalog reinstall.
async fn mcp_server_reinstall(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(server_id): Path<String>,
) -> Result<(StatusCode, Json<mcp::CatalogLifecycleResult>), ApiError> {
    let manager = mcp_manager(&state)?;
    ensure_mcp_server_route_access(manager, tenant.as_ref().map(|e| &e.0.0), &server_id, true)?;
    let response = manager
        .reinstall_catalog_server(&server_id)
        .map_err(map_mcp_error)?;
    let status = lifecycle_status(&response.status);
    Ok((status, Json(response)))
}

/// POST /v1/mcp/servers/{server_id}/uninstall — catalog uninstall.
async fn mcp_server_uninstall(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(server_id): Path<String>,
) -> Result<(StatusCode, Json<mcp::CatalogLifecycleResult>), ApiError> {
    let manager = mcp_manager(&state)?;
    ensure_mcp_server_route_access(manager, tenant.as_ref().map(|e| &e.0.0), &server_id, true)?;
    let response = manager
        .uninstall_catalog_server(&server_id)
        .map_err(map_mcp_error)?;
    let status = lifecycle_status(&response.status);
    Ok((status, Json(response)))
}

/// POST /v1/mcp/servers/{server_id}/revalidate — catalog revalidation (200
/// when the server is ready, 409 otherwise — Go handleMCPServerRevalidate).
async fn mcp_server_revalidate(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(server_id): Path<String>,
) -> Result<(StatusCode, Json<mcp::CatalogRevalidationResult>), ApiError> {
    let manager = mcp_manager(&state)?;
    ensure_mcp_server_route_access(manager, tenant.as_ref().map(|e| &e.0.0), &server_id, true)?;
    let response = manager
        .revalidate_catalog_server(&server_id)
        .map_err(map_mcp_error)?;
    let status = if response.status == mcp::AvailabilityStatus::Ready {
        StatusCode::OK
    } else {
        StatusCode::CONFLICT
    };
    Ok((status, Json(response)))
}

/// POST /v1/mcp/servers/{server_id}/stop — lifecycle stop.
async fn mcp_server_stop(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(server_id): Path<String>,
) -> Result<Json<mcp::LifecycleResponse>, ApiError> {
    let manager = mcp_manager(&state)?;
    ensure_mcp_server_route_access(manager, tenant.as_ref().map(|e| &e.0.0), &server_id, true)?;
    let response = manager.stop(&server_id).map_err(map_mcp_error)?;
    Ok(Json(response))
}

/// POST /v1/mcp/servers/{server_id}/restart — lifecycle restart.
async fn mcp_server_restart(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(server_id): Path<String>,
) -> Result<Json<mcp::LifecycleResponse>, ApiError> {
    let manager = mcp_manager(&state)?;
    ensure_mcp_server_route_access(manager, tenant.as_ref().map(|e| &e.0.0), &server_id, true)?;
    let response = manager
        .restart(&server_id, &current_actor(tenant.as_ref().map(|e| &e.0.0)))
        .map_err(map_mcp_error)?;
    Ok(Json(response))
}

/// POST /v1/mcp/servers/{server_id}/cancel — lifecycle cancel.
async fn mcp_server_cancel(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(server_id): Path<String>,
) -> Result<Json<mcp::LifecycleResponse>, ApiError> {
    let manager = mcp_manager(&state)?;
    ensure_mcp_server_route_access(manager, tenant.as_ref().map(|e| &e.0.0), &server_id, true)?;
    let response = manager.cancel(&server_id).map_err(map_mcp_error)?;
    Ok(Json(response))
}

/// GET /v1/mcp/servers/{server_id}/tools — tool list, tenant-scoped when a
/// tenant context is present (Go handleMCPServerTools).
async fn mcp_server_tools(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(server_id): Path<String>,
) -> Result<Json<ListResponse<mcp::ToolResource>>, ApiError> {
    let manager = mcp_manager(&state)?;
    let items = match tenant.as_ref().map(|e| &e.0.0) {
        Some(tc) if !tc.tenant_id.trim().is_empty() => {
            manager.list_tools_for_tenant(&server_id, &tc.tenant_id)
        }
        _ => manager.list_tools(&server_id),
    }
    .map_err(map_mcp_error)?;
    Ok(Json(ListResponse { items }))
}

/// PATCH /v1/mcp/servers/{server_id}/tools/{tool_name} — update a tool's
/// exposure rule (Go handleMCPServerToolExposure).
async fn mcp_server_tool_exposure(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path((server_id, tool_name)): Path<(String, String)>,
    body: Bytes,
) -> Result<Json<mcp::ToolResource>, ApiError> {
    let manager = mcp_manager(&state)?;
    ensure_mcp_server_route_access(manager, tenant.as_ref().map(|e| &e.0.0), &server_id, true)?;
    let input: mcp::UpdateExposureInput = decode_json_body(&body)?;
    let resource = manager
        .update_tool_exposure(&server_id, &tool_name, &input)
        .map_err(map_mcp_error)?;
    Ok(Json(resource))
}

/// POST /v1/mcp/servers/{server_id}/tools/{tool_name}/authorize — authorize a
/// tool use. The response status follows the authorization result: allowed ->
/// 200, pending -> 409, rejected -> 403, otherwise 409 (Go
/// handleMCPServerToolAuthorize).
async fn mcp_server_tool_authorize(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path((server_id, tool_name)): Path<(String, String)>,
    body: Bytes,
) -> Result<(StatusCode, Json<mcp::ToolAuthorizationResponse>), ApiError> {
    let manager = mcp_manager(&state)?;
    ensure_mcp_server_route_access(manager, tenant.as_ref().map(|e| &e.0.0), &server_id, false)?;
    let mut input: mcp::AuthorizeToolInput = decode_optional_json_body(&body)?;
    if input.requested_by.trim().is_empty() {
        input.requested_by = current_actor(tenant.as_ref().map(|e| &e.0.0));
    }
    let response = manager
        .authorize_tool(&server_id, &tool_name, &input)
        .map_err(map_mcp_error)?;
    let status = match response.status {
        mcp::ToolAuthorizationStatus::Allowed => StatusCode::OK,
        mcp::ToolAuthorizationStatus::Pending => StatusCode::CONFLICT,
        mcp::ToolAuthorizationStatus::Rejected => StatusCode::FORBIDDEN,
        mcp::ToolAuthorizationStatus::Blocked => StatusCode::CONFLICT,
    };
    Ok((status, Json(response)))
}

// ---------------------------------------------------------------------------
// MCP transports + catalog
// ---------------------------------------------------------------------------

/// GET /v1/mcp/transports — transport capability records (Go
/// handleMCPTransports).
async fn mcp_transports(
    State(state): State<AppState>,
) -> Result<Json<ListResponse<mcp::TransportCapability>>, ApiError> {
    let manager = mcp_manager(&state)?;
    Ok(Json(ListResponse {
        items: manager.list_transport_capabilities(),
    }))
}

/// GET /v1/mcp/catalog — the bundled catalog entries.
async fn mcp_catalog(
    State(state): State<AppState>,
) -> Result<Json<ListResponse<mcp::CatalogEntry>>, ApiError> {
    let manager = mcp_manager(&state)?;
    Ok(Json(ListResponse {
        items: manager.list_catalog(),
    }))
}

/// GET /v1/mcp/catalog/{entry_id} — one catalog entry.
async fn mcp_catalog_entry(
    State(state): State<AppState>,
    Path(entry_id): Path<String>,
) -> Result<Json<mcp::CatalogEntry>, ApiError> {
    let manager = mcp_manager(&state)?;
    let entry = manager
        .get_catalog_entry(&entry_id)
        .ok_or_else(|| ApiError::NotFound("not found".to_string()))?;
    Ok(Json(entry))
}

/// POST /v1/mcp/catalog/{entry_id}/install — install a catalog entry (201 when
/// installed, 409 when blocked — Go handleMCPCatalogRoutes). The empty-body
/// install input is allowed (Go tolerates io.EOF).
async fn mcp_catalog_install(
    State(state): State<AppState>,
    Path(entry_id): Path<String>,
    body: Bytes,
) -> Result<(StatusCode, Json<mcp::CatalogInstallResult>), ApiError> {
    let manager = mcp_manager(&state)?;
    let input: mcp::CatalogInstallInput = decode_optional_json_body(&body)?;
    let result = manager
        .install_catalog_entry(&entry_id, &input, mcp::InstallMethod::Api)
        .map_err(map_mcp_error)?;
    let status = if result.status == "installed" {
        StatusCode::CREATED
    } else {
        StatusCode::CONFLICT
    };
    Ok((status, Json(result)))
}

// ---------------------------------------------------------------------------
// Skills
// ---------------------------------------------------------------------------

/// GET /v1/skills — the registry snapshot (Go handleSkills GET +
/// buildSkillRegistryResponse).
async fn skills_list(
    State(state): State<AppState>,
) -> Result<Json<SkillRegistryResponse>, ApiError> {
    let registry = skills_registry(&state)?;
    Ok(Json(build_skill_registry_response(&registry.snapshot())))
}

/// POST /v1/skills/reload — rescan the skill roots and return the new snapshot
/// (Go handleSkillRoutes "reload" branch).
async fn skills_reload(
    State(state): State<AppState>,
) -> Result<Json<SkillRegistryResponse>, ApiError> {
    let registry = skills_registry(&state)?;
    registry
        .reload()
        .map_err(|err| ApiError::internal(err.to_string()))?;
    Ok(Json(build_skill_registry_response(&registry.snapshot())))
}

/// GET /v1/skills/{skill_id} — one skill's detail view (Go handleSkillRoutes
/// get branch + buildSkillDetailResponse).
async fn skill_detail(
    State(state): State<AppState>,
    Path(skill_id): Path<String>,
) -> Result<Json<SkillDetailResponse>, ApiError> {
    let registry = skills_registry(&state)?;
    let skill = registry
        .get(&skill_id)
        .ok_or_else(|| ApiError::NotFound("skill not found".to_string()))?;
    Ok(Json(build_skill_detail_response(&skill)))
}

// ---------------------------------------------------------------------------
// Webhooks
// ---------------------------------------------------------------------------

/// GET /v1/webhooks — endpoints for the tenant from the tenantId query
/// parameter (Go handleWebhooks GET).
async fn list_webhooks(
    State(state): State<AppState>,
    Query(query): Query<WebhookListQuery>,
) -> Result<Json<WebhookListResponse>, ApiError> {
    let manager = webhook_manager(&state)?;
    let tenant_id = query.tenant_id.unwrap_or_default();
    Ok(Json(WebhookListResponse {
        items: manager.list_for_tenant(&webhook_tenant(&tenant_id)),
    }))
}

/// POST /v1/webhooks — register a webhook endpoint; the plaintext signing
/// secret is returned exactly once (Go handleWebhooks POST).
async fn create_webhook(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<(StatusCode, Json<webhook::CreateSecret>), ApiError> {
    let manager = webhook_manager(&state)?;
    let request: CreateWebhookRequest = decode_json_body(&body)?;
    let tenant_id = webhook_tenant(&request.tenant_id);
    let created = manager
        .create(
            &tenant_id,
            request.name.trim(),
            request.target_kind,
            request.target_ref.trim(),
        )
        .map_err(|err| write_webhook_error(&err))?;
    Ok((StatusCode::CREATED, Json(created)))
}

/// GET /v1/webhooks/{webhook_id} — one endpoint for the tenant (Go
/// handleWebhookRoutes get branch).
async fn get_webhook(
    State(state): State<AppState>,
    Query(query): Query<WebhookListQuery>,
    Path(webhook_id): Path<String>,
) -> Result<Json<webhook::Endpoint>, ApiError> {
    let manager = webhook_manager(&state)?;
    let tenant_id = webhook_tenant(query.tenant_id.as_deref().unwrap_or_default());
    let endpoint = manager
        .get(&tenant_id, &webhook_id)
        .ok_or_else(|| write_webhook_error(&webhook::WebhookError::EndpointNotFound))?;
    Ok(Json(endpoint))
}

/// POST /v1/webhooks/{webhook_id}/rotate — issue a new signing secret (Go
/// handleWebhookRoutes "rotate").
async fn rotate_webhook(
    State(state): State<AppState>,
    Query(query): Query<WebhookListQuery>,
    Path(webhook_id): Path<String>,
    body: Bytes,
) -> Result<Json<webhook::CreateSecret>, ApiError> {
    let manager = webhook_manager(&state)?;
    let request: WebhookTenantRequest = decode_optional_json_body(&body)?;
    let tenant_id = webhook_tenant(&first_non_empty(&[
        request.tenant_id.as_str(),
        query.tenant_id.as_deref().unwrap_or_default(),
    ]));
    let rotated = manager
        .rotate(&tenant_id, &webhook_id)
        .map_err(|err| write_webhook_error(&err))?;
    Ok(Json(rotated))
}

/// POST /v1/webhooks/{webhook_id}/disable — deactivate an endpoint (Go
/// handleWebhookRoutes "disable").
async fn disable_webhook(
    State(state): State<AppState>,
    Query(query): Query<WebhookListQuery>,
    Path(webhook_id): Path<String>,
    body: Bytes,
) -> Result<Json<webhook::Endpoint>, ApiError> {
    let manager = webhook_manager(&state)?;
    let request: WebhookTenantRequest = decode_optional_json_body(&body)?;
    let tenant_id = webhook_tenant(&first_non_empty(&[
        request.tenant_id.as_str(),
        query.tenant_id.as_deref().unwrap_or_default(),
    ]));
    let disabled = manager
        .disable(&tenant_id, &webhook_id)
        .map_err(|err| write_webhook_error(&err))?;
    Ok(Json(disabled))
}

/// POST /v1/triggers/webhook/{webhook_id} — inbound ingress authenticated by
/// the X-Webhook-Signature header (Go handleWebhookTrigger). Always answers
/// with the redacted trigger record; the status encodes the outcome.
async fn trigger_webhook(
    State(state): State<AppState>,
    Path(webhook_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<webhook::TriggerRecord>), ApiError> {
    let manager = webhook_manager(&state)?;
    let webhook_id = webhook_id.trim().to_string();
    if webhook_id.is_empty() {
        return Err(ApiError::NotFound("not found".to_string()));
    }
    let signature = header_value(&headers, "x-webhook-signature");
    let idempotency_key = header_value(&headers, "x-webhook-idempotency-key");
    let (record, result) = manager.trigger_signed(&webhook_id, &signature, &idempotency_key, body.to_vec());
    let status = match result {
        Ok(()) => StatusCode::ACCEPTED,
        Err(err) => webhook_trigger_status(&err),
    };
    Ok((status, Json(record)))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Go nil-manager guard: 500 "mcp manager is not configured".
fn mcp_manager(state: &AppState) -> Result<&mcp::Manager, ApiError> {
    state
        .mcp
        .as_deref()
        .ok_or_else(|| ApiError::internal("mcp manager is not configured"))
}

/// Go nil-registry guard: 500 "skills registry is not configured".
fn skills_registry(state: &AppState) -> Result<&kura_skills::Registry, ApiError> {
    state
        .skills
        .as_deref()
        .ok_or_else(|| ApiError::internal("skills registry is not configured"))
}

/// Go nil-manager guard: 500 "webhook manager is not configured".
fn webhook_manager(state: &AppState) -> Result<&webhook::Manager, ApiError> {
    state
        .webhooks
        .as_deref()
        .ok_or_else(|| ApiError::internal("webhook manager is not configured"))
}

/// Go requireMCPPermissionIfTenant: local (no tenant context) requests skip
/// the check; tenant contexts need mcp.manage or answer the stable credential
/// denial (403).
fn require_mcp_manage(tenant: Option<&kura_identity::TenantContext>) -> Result<(), ApiError> {
    if let Some(tc) = tenant {
        if !tc.tenant_id.trim().is_empty()
            && !has_permission(&tc.permissions, Permission::McpManage)
        {
            return Err(ApiError::Forbidden("credential_access_denied".to_string()));
        }
    }
    Ok(())
}

/// Go mcpServerResourceForRequest: tenant contexts look up the resource through
/// the tenant-scoped projection (cross-tenant reads hide as None).
fn mcp_server_resource_for_request(
    manager: &mcp::Manager,
    tenant: Option<&kura_identity::TenantContext>,
    server_id: &str,
) -> Option<mcp::ServerResource> {
    match tenant {
        Some(tc) if !tc.tenant_id.trim().is_empty() => {
            manager.get_server_resource_for_tenant(server_id, &tc.tenant_id)
        }
        _ => manager.get_server_resource(server_id),
    }
}

/// Go ensureMCPServerRouteAccess: the server must resolve for the request's
/// tenant scope (404), and tenant contexts need mcp.manage when manage is
/// true (403).
fn ensure_mcp_server_route_access(
    manager: &mcp::Manager,
    tenant: Option<&kura_identity::TenantContext>,
    server_id: &str,
    manage: bool,
) -> Result<(), ApiError> {
    if mcp_server_resource_for_request(manager, tenant, server_id).is_none() {
        return Err(ApiError::NotFound("not found".to_string()));
    }
    if manage {
        require_mcp_manage(tenant)?;
    }
    Ok(())
}

/// Go currentActor: the acting principal id from the tenant context.
fn current_actor(tenant: Option<&kura_identity::TenantContext>) -> String {
    tenant.map(|tc| tc.principal_id.clone()).unwrap_or_default()
}

/// Go mcp handler error mapping: server/approval not found -> 404, invalid
/// approval id -> 400, everything else -> 400 (writeError).
fn map_mcp_error(err: mcp::McpError) -> ApiError {
    match err {
        mcp::McpError::ServerNotFound => ApiError::NotFound("not found".to_string()),
        mcp::McpError::ApprovalNotFound => ApiError::NotFound(err.to_string()),
        mcp::McpError::ApprovalIDInvalid => ApiError::BadRequest(err.to_string()),
        other => ApiError::BadRequest(other.to_string()),
    }
}

/// Go handleMCPServerRefresh/Reinstall/Uninstall status: completed -> 200,
/// otherwise 409.
fn lifecycle_status(status: &mcp::CatalogActionStatus) -> StatusCode {
    if *status == mcp::CatalogActionStatus::Completed {
        StatusCode::OK
    } else {
        StatusCode::CONFLICT
    }
}

/// Go webhookTenant: the body tenant wins; otherwise the query tenant.
fn webhook_tenant(body_tenant: &str) -> String {
    let trimmed = body_tenant.trim();
    if !trimmed.is_empty() {
        trimmed.to_string()
    } else {
        String::new()
    }
}

/// Go firstNonEmpty (trimmed).
fn first_non_empty(values: &[&str]) -> String {
    for value in values {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    String::new()
}

fn header_value(headers: &HeaderMap, name: &str) -> String {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .unwrap_or_default()
        .to_string()
}

/// Go writeWebhookError.
fn write_webhook_error(err: &webhook::WebhookError) -> ApiError {
    match err {
        webhook::WebhookError::EndpointNotFound => ApiError::NotFound(err.to_string()),
        webhook::WebhookError::CrossTenant => ApiError::Forbidden(err.to_string()),
        webhook::WebhookError::InvalidEndpoint => ApiError::BadRequest(err.to_string()),
        other => ApiError::internal(other.to_string()),
    }
}

/// Go webhookTriggerStatusCode.
fn webhook_trigger_status(err: &webhook::WebhookError) -> StatusCode {
    match err {
        webhook::WebhookError::MissingAuth
        | webhook::WebhookError::BadSignature
        | webhook::WebhookError::CrossTenant => StatusCode::UNAUTHORIZED,
        webhook::WebhookError::EndpointNotFound => StatusCode::NOT_FOUND,
        webhook::WebhookError::Disabled => StatusCode::FORBIDDEN,
        webhook::WebhookError::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        webhook::WebhookError::QuotaDenied => StatusCode::TOO_MANY_REQUESTS,
        _ => StatusCode::BAD_REQUEST,
    }
}

/// Go decodeJSONBody: empty body -> 400 "request body is required".
fn decode_json_body<T: serde::de::DeserializeOwned>(body: &Bytes) -> Result<T, ApiError> {
    if body.is_empty() {
        return Err(ApiError::BadRequest("request body is required".to_string()));
    }
    serde_json::from_slice(body).map_err(|err| ApiError::BadRequest(err.to_string()))
}

/// Go decodeJSONBody with the io.EOF tolerance: an empty body decodes to the
/// default (used by catalog install / tool authorize / webhook rotate+disable).
fn decode_optional_json_body<T: serde::de::DeserializeOwned + Default>(
    body: &Bytes,
) -> Result<T, ApiError> {
    if body.is_empty() {
        return Ok(T::default());
    }
    serde_json::from_slice(body).map_err(|err| ApiError::BadRequest(err.to_string()))
}

/// Go buildSkillSummaryResponse.
fn build_skill_summary_response(skill: &kura_skills::Skill) -> SkillSummaryResponse {
    SkillSummaryResponse {
        skill_id: skill.skill_id.clone(),
        name: skill.name.clone(),
        description: skill.description.clone(),
        source: skill.source.as_str().to_string(),
        root_path: skill.root_path.clone(),
        skill_path: skill.skill_path.clone(),
        instruction_path: skill.instruction_path.clone(),
        files: skill
            .files
            .iter()
            .map(|file| SkillFileResponse {
                path: file.path.clone(),
                size_bytes: file.size_bytes,
            })
            .collect(),
        frontmatter: skill.frontmatter.clone(),
        execution_manifest: skill.execution_manifest.clone(),
        availability_status: skill.availability_status.as_str().to_string(),
        availability_reason: skill.availability_reason.clone(),
        sandbox: skill.sandbox.clone(),
    }
}

/// Go buildSkillDetailResponse.
fn build_skill_detail_response(skill: &kura_skills::Skill) -> SkillDetailResponse {
    SkillDetailResponse {
        summary: build_skill_summary_response(skill),
        frontmatter_raw: skill.frontmatter_raw.clone(),
        body: skill.body.clone(),
    }
}

/// Go buildSkillRegistryResponse.
fn build_skill_registry_response(snapshot: &kura_skills::Snapshot) -> SkillRegistryResponse {
    SkillRegistryResponse {
        loaded_at: snapshot.loaded_at,
        items: snapshot.skills.iter().map(build_skill_summary_response).collect(),
        overlays: snapshot
            .overlays
            .iter()
            .map(|overlay| SkillOverlayResponse {
                overlay_id: overlay.overlay_id.clone(),
                source: overlay.source.as_str().to_string(),
                path: overlay.path.clone(),
                size_bytes: overlay.size_bytes,
                modified_at: overlay.modified_at,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use axum::http::header::CONTENT_TYPE;
    use parking_lot::Mutex;
    use tower::ServiceExt;
    use uuid::Uuid;

    fn test_config() -> kura_config::Config {
        kura_config::Config {
            environment: kura_config::Environment::Test,
            bind_addr: "127.0.0.1:19192".to_string(),
            data_dir: "/tmp/kura-api-mcp".to_string(),
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
        let dir = std::env::temp_dir().join(format!("kura-api-mcp-{}", Uuid::now_v7()));
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

    /// Builds a request with a resolved tenant context extension.
    fn tenant_request(
        method: &str,
        uri: &str,
        body: Option<&str>,
        tenant_id: &str,
        permissions: Vec<Permission>,
    ) -> Request<Body> {
        let mut req = request(method, uri, body);
        req.extensions_mut().insert(TenantContext(kura_identity::TenantContext {
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

    fn app(state: AppState) -> axum::Router {
        router().merge(ingress_router()).with_state(state)
    }

    fn with_mcp(state: &mut AppState) {
        state.mcp = Some(Arc::new(mcp::Manager::default()));
    }

    fn create_server_body(server_id: &str, command: &str) -> String {
        format!(
            r#"{{"serverId":"{server_id}","displayName":"Test MCP","enabled":true,"sandboxProfileId":"subprocess_default","declarationId":"mcp_server:{server_id}:lifecycle.start","transportKind":"stdio","command":"{command}","args":[],"workingDir":"/tmp","secretRefs":[],"autoRestart":true}}"#
        )
    }

    /// Port of TestMCPServerRoutes (registration/inspection half; lifecycle
    /// start needs the sandbox helper process, so it is not ported).
    #[tokio::test]
    async fn mcp_servers_create_list_get() {
        let mut state = test_state();
        with_mcp(&mut state);
        let app = app(state);

        let body = create_server_body("api-mcp", "/bin/echo");
        let (status, json) = send(&app, request("POST", "/v1/mcp/servers", Some(&body))).await;
        assert_eq!(status, StatusCode::CREATED, "create should be 201: {json}");
        assert_eq!(json["serverId"], "api-mcp");

        let (status, json) = send(&app, request("GET", "/v1/mcp/servers", None)).await;
        assert_eq!(status, StatusCode::OK);
        let items = json["items"].as_array().expect("items array");
        assert!(items.iter().any(|item| item["serverId"] == "api-mcp"));

        let (status, json) = send(&app, request("GET", "/v1/mcp/servers/api-mcp", None)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["serverId"], "api-mcp");

        let (status, _) = send(&app, request("GET", "/v1/mcp/servers/missing", None)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // Empty create body -> 400 (Go decodeJSONBody).
        let (status, _) = send(&app, request("POST", "/v1/mcp/servers", None)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    /// Port of TestMCPTransportInspectionRoutes (transport + create halves) and
    /// the catalog inspection portion of
    /// TestMCPCatalogInstallAndRuntimeToolInvocation.
    #[tokio::test]
    async fn mcp_transports_and_catalog_inspection() {
        let mut state = test_state();
        with_mcp(&mut state);
        let app = app(state);

        let (status, json) = send(&app, request("GET", "/v1/mcp/transports", None)).await;
        assert_eq!(status, StatusCode::OK);
        let transports = json["items"].as_array().expect("transports array");
        assert!(transports.len() >= 3, "expected additive transport capability records: {json}");

        let (status, json) = send(&app, request("GET", "/v1/mcp/catalog", None)).await;
        assert_eq!(status, StatusCode::OK);
        let items = json["items"].as_array().expect("catalog array");
        assert!(
            items.iter().any(|item| item["id"] == "filesystem"),
            "bundled catalog must contain filesystem: {json}"
        );

        let (status, json) = send(&app, request("GET", "/v1/mcp/catalog/filesystem", None)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["id"], "filesystem");

        let (status, _) = send(&app, request("GET", "/v1/mcp/catalog/missing", None)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// Port of TestMCPCatalogMaintenanceRoutes: install -> refresh -> modify ->
    /// conflict refresh -> reinstall -> stop -> uninstall -> 404.
    #[tokio::test]
    async fn mcp_catalog_install_and_maintenance() {
        let mut state = test_state();
        with_mcp(&mut state);
        let app = app(state);

        let install_body = r#"{"serverId":"filesystem-test","command":"/bin/echo","workingDir":"/tmp"}"#;
        let (status, json) = send(&app, request("POST", "/v1/mcp/catalog/filesystem/install", Some(install_body))).await;
        assert_eq!(status, StatusCode::CREATED, "install should be 201: {json}");
        assert_eq!(json["status"], "installed");
        assert_eq!(json["server"]["serverId"], "filesystem-test");
        assert_eq!(json["server"]["originKind"], "catalog");
        assert_eq!(json["server"]["catalogEntryId"], "filesystem");

        let (status, json) = send(&app, request("POST", "/v1/mcp/servers/filesystem-test/refresh", None)).await;
        assert_eq!(status, StatusCode::OK, "refresh should be 200: {json}");
        assert_eq!(json["status"], "completed");

        // Local modification flips refresh to a 409 conflict (Go drift guard).
        let (status, _) = send(&app, request("PATCH", "/v1/mcp/servers/filesystem-test", Some(r#"{"displayName":"Filesystem Modified"}"#))).await;
        assert_eq!(status, StatusCode::OK);

        let (status, json) = send(&app, request("POST", "/v1/mcp/servers/filesystem-test/refresh", None)).await;
        assert_eq!(status, StatusCode::CONFLICT, "modified refresh should be 409: {json}");
        assert_eq!(json["failureClass"], "conflict");

        // Reinstall after local modification is also drift-blocked (Go
        // fail_on_modified for refresh/reinstall), so uninstall is the way out.
        let (status, json) = send(&app, request("POST", "/v1/mcp/servers/filesystem-test/reinstall", None)).await;
        assert_eq!(status, StatusCode::CONFLICT, "modified reinstall should be 409: {json}");
        assert_eq!(json["failureClass"], "conflict");

        // Stop is idempotent for a never-started server (Go stop_or_cancel).
        let (status, _) = send(&app, request("POST", "/v1/mcp/servers/filesystem-test/stop", None)).await;
        assert_eq!(status, StatusCode::OK);

        let (status, json) = send(&app, request("POST", "/v1/mcp/servers/filesystem-test/uninstall", None)).await;
        assert_eq!(status, StatusCode::OK, "uninstall should be 200: {json}");
        assert_eq!(json["status"], "completed");
        assert_eq!(json["removed"], true);

        let (status, _) = send(&app, request("GET", "/v1/mcp/servers/filesystem-test", None)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "uninstalled server must 404");

        // Uninstalling again -> 404 (server no longer resolves).
        let (status, _) = send(&app, request("POST", "/v1/mcp/servers/filesystem-test/uninstall", None)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// Port of the tenant permission gate in handleMCPServers: tenant contexts
    /// without mcp.manage are denied 403; local (no tenant) requests pass.
    #[tokio::test]
    async fn mcp_create_requires_manage_for_tenant_contexts() {
        let mut state = test_state();
        with_mcp(&mut state);
        let app = app(state);

        let body = create_server_body("tenant-mcp", "/bin/echo");
        let (status, _) = send(&app, tenant_request("POST", "/v1/mcp/servers", Some(&body), "ten_mcp", vec![])).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "tenant without mcp.manage must be denied");

        let (status, _) = send(&app, tenant_request("POST", "/v1/mcp/servers", Some(&body), "ten_mcp", vec![Permission::McpManage])).await;
        assert_eq!(status, StatusCode::CREATED, "tenant with mcp.manage may create");
    }

    /// Go handleMCPServers' nil-manager guard: 500 when unconfigured.
    #[tokio::test]
    async fn mcp_manager_not_configured_returns_500() {
        let app = app(test_state());
        let (status, json) = send(&app, request("GET", "/v1/mcp/servers", None)).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(json["error"], "mcp manager is not configured");
    }

    // -- skills ---------------------------------------------------------------

    /// Builds a registry over a temp data root containing one SKILL.md bundle.
    fn skills_state() -> AppState {
        let mut state = test_state();
        let root = std::env::temp_dir().join(format!("kura-api-skills-{}", Uuid::now_v7()));
        let skill_dir = root.join("skills").join("demo-skill");
        std::fs::create_dir_all(&skill_dir).expect("mkdir skill");
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: Demo Skill\ndescription: A demo skill\n---\nHello body",
        )
        .expect("write skill");
        let home = root.join("home");
        let registry =
            kura_skills::Registry::with_roots(&home.to_string_lossy(), root.to_str().expect("path"))
                .expect("registry");
        state.skills = Some(Arc::new(registry));
        state
    }

    /// Port of handleSkills / handleSkillRoutes: list, detail, reload, 404.
    #[tokio::test]
    async fn skills_list_detail_and_reload() {
        let app = app(skills_state());

        let (status, json) = send(&app, request("GET", "/v1/skills", None)).await;
        assert_eq!(status, StatusCode::OK, "skills list should be 200: {json}");
        let items = json["items"].as_array().expect("items array");
        assert_eq!(items.len(), 1, "expected the seeded skill: {json}");
        assert_eq!(items[0]["skillId"], "demo skill");
        assert_eq!(items[0]["source"], "data_dir");

        let (status, json) = send(&app, request("GET", "/v1/skills/demo%20skill", None)).await;
        assert_eq!(status, StatusCode::OK, "detail should be 200: {json}");
        assert_eq!(json["skillId"], "demo skill");
        assert_eq!(json["body"], "Hello body");

        let (status, _) = send(&app, request("GET", "/v1/skills/missing", None)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, json) = send(&app, request("POST", "/v1/skills/reload", None)).await;
        assert_eq!(status, StatusCode::OK, "reload should be 200: {json}");
        assert_eq!(json["items"][0]["skillId"], "demo skill");
    }

    /// Go handleSkills' nil-registry guard: 500 when unconfigured.
    #[tokio::test]
    async fn skills_registry_not_configured_returns_500() {
        let app = app(test_state());
        let (status, json) = send(&app, request("GET", "/v1/skills", None)).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(json["error"], "skills registry is not configured");
    }

    // -- webhooks -------------------------------------------------------------

    fn webhooks_state() -> AppState {
        let mut state = test_state();
        state.webhooks = Some(Arc::new(webhook::Manager::new("test", None, None)));
        state
    }

    fn create_webhook_body(name: &str) -> String {
        format!(
            r#"{{"tenantId":"ten_webhook","name":"{name}","targetKind":"workflow","targetRef":"ref_1"}}"#
        )
    }

    /// Port of handleWebhooks / handleWebhookRoutes CRUD: create (201 with the
    /// one-time secret), list, get, rotate, disable, cross-tenant 404.
    #[tokio::test]
    async fn webhooks_crud_lifecycle() {
        let app = app(webhooks_state());

        let body = create_webhook_body("Ship");
        let (status, json) = send(&app, request("POST", "/v1/webhooks", Some(&body))).await;
        assert_eq!(status, StatusCode::CREATED, "create should be 201: {json}");
        let webhook_id = json["endpoint"]["webhookId"].as_str().expect("webhook id").to_string();
        let secret = json["secret"].as_str().expect("secret").to_string();
        assert!(!secret.is_empty(), "the plaintext secret is returned exactly once");

        let (status, json) = send(&app, request("GET", "/v1/webhooks?tenantId=ten_webhook", None)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["items"][0]["webhookId"], webhook_id);

        let (status, json) = send(
            &app,
            request("GET", &format!("/v1/webhooks/{webhook_id}?tenantId=ten_webhook"), None),
        ).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["webhookId"], webhook_id);

        let (status, _) = send(
            &app,
            request("GET", &format!("/v1/webhooks/{webhook_id}?tenantId=ten_other"), None),
        ).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "cross-tenant get must hide the endpoint");

        let (status, json) = send(
            &app,
            request("POST", &format!("/v1/webhooks/{webhook_id}/rotate?tenantId=ten_webhook"), None),
        ).await;
        assert_eq!(status, StatusCode::OK);
        let rotated = json["secret"].as_str().expect("rotated secret").to_string();
        assert_ne!(rotated, secret, "rotation issues a new signing secret");

        let (status, json) = send(
            &app,
            request("POST", &format!("/v1/webhooks/{webhook_id}/disable?tenantId=ten_webhook"), None),
        ).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["status"], "disabled");

        // Invalid create (missing name/ref) -> 400 (Go ErrInvalidEndpoint).
        let (status, _) = send(
            &app,
            request(
                "POST",
                "/v1/webhooks",
                Some(r#"{"tenantId":"ten_webhook","name":"","targetKind":"workflow","targetRef":""}"#),
            ),
        ).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    /// Port of handleWebhookTrigger: signature-authenticated ingress with the
    /// outcome status mapping (202 / 401 / 404 / 403 / 413).
    #[tokio::test]
    async fn webhooks_trigger_signed() {
        let app = app(webhooks_state());

        let body = create_webhook_body("Trigger");
        let (status, json) = send(&app, request("POST", "/v1/webhooks", Some(&body))).await;
        assert_eq!(status, StatusCode::CREATED);
        let webhook_id = json["endpoint"]["webhookId"].as_str().expect("webhook id").to_string();
        let secret = json["secret"].as_str().expect("secret").to_string();

        let payload = r#"{"event":"deploy"}"#;
        let signature = webhook::sign(&secret, payload.as_bytes());

        let signed = Request::builder()
            .method("POST")
            .uri(format!("/v1/triggers/webhook/{webhook_id}"))
            .header("x-webhook-signature", &signature)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(payload.to_string()))
            .expect("request");
        let (status, json) = send(&app, signed).await;
        assert_eq!(status, StatusCode::ACCEPTED, "signed trigger should be 202: {json}");
        assert_eq!(json["status"], "fired");

        // Missing signature -> 401.
        let unsigned = request("POST", &format!("/v1/triggers/webhook/{webhook_id}"), Some(payload));
        let (status, _) = send(&app, unsigned).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // Bad signature -> 401.
        let bad = Request::builder()
            .method("POST")
            .uri(format!("/v1/triggers/webhook/{webhook_id}"))
            .header("x-webhook-signature", "deadbeef")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(payload.to_string()))
            .expect("request");
        let (status, _) = send(&app, bad).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // Unknown webhook -> 404.
        let missing = Request::builder()
            .method("POST")
            .uri("/v1/triggers/webhook/webhook_missing")
            .header("x-webhook-signature", &signature)
            .body(Body::from(payload.to_string()))
            .expect("request");
        let (status, _) = send(&app, missing).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // Disabled endpoint -> 403.
        let (status, _) = send(
            &app,
            request("POST", &format!("/v1/webhooks/{webhook_id}/disable?tenantId=ten_webhook"), None),
        ).await;
        assert_eq!(status, StatusCode::OK);
        let disabled = Request::builder()
            .method("POST")
            .uri(format!("/v1/triggers/webhook/{webhook_id}"))
            .header("x-webhook-signature", &signature)
            .body(Body::from(payload.to_string()))
            .expect("request");
        let (status, _) = send(&app, disabled).await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        // Payload over the 64KiB bound -> 413.
        let body = create_webhook_body("Big");
        let (status, json) = send(&app, request("POST", "/v1/webhooks", Some(&body))).await;
        assert_eq!(status, StatusCode::CREATED);
        let big_id = json["endpoint"]["webhookId"].as_str().expect("webhook id").to_string();
        let big_secret = json["secret"].as_str().expect("secret").to_string();
        let big_payload = "a".repeat(webhook::MAX_PAYLOAD_BYTES + 1);
        let big_signature = webhook::sign(&big_secret, big_payload.as_bytes());
        let big = Request::builder()
            .method("POST")
            .uri(format!("/v1/triggers/webhook/{big_id}"))
            .header("x-webhook-signature", &big_signature)
            .body(Body::from(big_payload))
            .expect("request");
        let (status, _) = send(&app, big).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    }

    /// Go handleWebhooks' nil-manager guard: 500 when unconfigured.
    #[tokio::test]
    async fn webhooks_manager_not_configured_returns_500() {
        let app = app(test_state());
        let (status, json) = send(&app, request("GET", "/v1/webhooks?tenantId=ten_a", None)).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(json["error"], "webhook manager is not configured");
    }
}
