//! integrations route family (port of daemon/internal/api/integrations.go +
//! integration_diagnostics.go + the `/v1/integrations*` registrations in
//! server.go).
//!
//! Surface (Go parity):
//! - `GET/POST /v1/integrations` — list (tenant-scoped when a tenant context
//!   is resolved) / create. Fully ported on kura-integrations Manager +
//!   kura-store integrations DAO + integration.registered event.
//! - `GET/DELETE /v1/integrations/{id}` — detail / disconnect. The by-id
//!   tenant guard (Go withByIDTenantGuard on integrations.integration_id) is
//!   applied inline. Fully ported (manager.disconnect + integration.disconnected).
//! - `POST /v1/integrations/{id}/readiness` — readiness report (Go
//!   handleIntegrationReadiness): manager.update_readiness + persist +
//!   integration.updated / integration.readiness_changed.
//! - `POST /v1/integrations/{id}/default` — canonical-default selection (Go
//!   handleIntegrationDefault): persists every sibling in the same binding
//!   group + integration.updated / integration.default_changed.
//! - `/v1/integrations/{id}/diagnostics` + `{id}/diagnostics/runs` — the
//!   diagnostics list/create dispatch (Go handleIntegrationDiagnostics). The
//!   tenant/permission gates and cross-tenant non-disclosure (404) are fully
//!   ported; the handlers persist diagnostic results/runs through the kura-store
//!   integration_diagnostics DAOs (SaveIntegrationDiagnosticResult /
//!   SaveIntegrationDiagnosticRun / LatestIntegrationDiagnosticResults).
//! - `/v1/integration-diagnostics/runs` + `/runs/{run_id}` — list/detail
//!   (Go handleIntegrationDiagnosticRuns) on
//!   list/get_integration_diagnostic_run.
//! - `/v1/integration-diagnostics/smoke` — smoke report (Go
//!   handleIntegrationDiagnosticSmoke): full tenant/permission/risky-probe
//!   gating, probe inputs built through the integrations manager, per-probe
//!   diagnostic results persisted, and the smoke report returned + published.
//!   The smoke report row itself (Go SaveSmokeMatrixReport) has no Rust store
//!   DAO yet, so only the per-probe result rows are persisted.
//! - `/v1/integration-diagnostics/retention/apply` — expired retention records
//!   flipped to Expired via apply_expired_diagnostic_retention_records.
//! - `/v1/integration-diagnostics/reason-codes` — fully ported
//!   (default_diagnostic_reason_code_catalog()).
//! - `/v1/integration-diagnostics/reason-codes` — fully ported
//!   (default_diagnostic_reason_code_catalog()).
//! - `/v1/integrations/sync` + `/v1/integrations/{id}/adapter-rpc` —
//!   registered as 501 markers: the Go daemon has no HTTP handlers for these
//!   (adapter_rpc is an integrations.BackendKind used by the calendar/mail
//!   managers, not a REST surface; the manager-restore-from-store sync has no
//!   Go route either). No Go behavior exists to port, so they answer 501
//!   rather than inventing API surface.
//!
//! Go maps the nil-manager analogue to 500; a nil sqliteStore is impossible
//! here (AppState.store is required). Status codes / DTOs / validation mirror
//! the Go handlers: empty body -> 400, unknown integration -> 404, manager
//! validation failures -> 400, cross-tenant by-id access -> 404 (never
//! disclosed as 403).

use std::collections::HashMap;

use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{Method, StatusCode, Uri};
use axum::routing::{get, post};
use axum::{Json as AxumJson, Router};
use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

use kura_events as events;
use kura_identity::{can_inspect_credentials, has_permission, Permission};
use kura_integrations::{
    classify_provider_evidence, complete_diagnostic_run, diagnostic_defaults,
    diagnostic_remediation_hint, diagnostic_retention_expiry, first_non_empty,
    is_unavailable_probe_error, DiagnosticInspectionInput, DiagnosticManager,
    DiagnosticReasonCode, DiagnosticResult, DiagnosticResultFilter, DiagnosticRun,
    DiagnosticRunFilter, DiagnosticRunInput, DiagnosticRunStatus, DiagnosticStatus,
    ProbeKind, ProbeResult, ProviderDiagnosticEvidence, RedactionStatus,
};

use crate::error::ApiError;
use crate::middleware::{environment_scope_from_config, guard_resource_for_tenant, TenantContext};
use crate::response::Json;
use crate::state::AppState;
use crate::types::{
    CreateIntegrationDiagnosticRunRequest, CreateIntegrationDiagnosticSmokeProbe,
    CreateIntegrationDiagnosticSmokeRequest, CreateIntegrationRequest, IntegrationDiagnosticListResponse,
    IntegrationDiagnosticRunListResponse, IntegrationListResponse, ListResponse,
    ReportIntegrationReadinessRequest,
};

/// Route family router. Only the methods the Go handlers accept are
/// registered; axum answers the other methods with 405 (Go
/// w.WriteHeader(http.StatusMethodNotAllowed)).
#[must_use]
pub fn router() -> Router<AppState> {
    Router::new()
        // /v1/integrations (Go handleIntegrations).
        .route(
            "/v1/integrations",
            get(list_integrations).post(create_integration),
        )
        // /v1/integrations/ (Go handleIntegrationRoutes, wrapped in the
        // by-id tenant guard on integrations.integration_id).
        .route(
            "/v1/integrations/{integration_id}",
            get(get_integration).delete(disconnect_integration),
        )
        .route(
            "/v1/integrations/{integration_id}/readiness",
            post(update_integration_readiness),
        )
        .route(
            "/v1/integrations/{integration_id}/default",
            post(set_integration_default),
        )
        // /v1/integrations/{id}/diagnostics (Go handleIntegrationDiagnostics).
        .route(
            "/v1/integrations/{integration_id}/diagnostics",
            get(integration_diagnostic_list),
        )
        .route(
            "/v1/integrations/{integration_id}/diagnostics/runs",
            post(create_integration_diagnostic_run),
        )
        // /v1/integration-diagnostics/* (Go server.go registrations).
        .route(
            "/v1/integration-diagnostics/runs",
            get(list_integration_diagnostic_runs),
        )
        .route(
            "/v1/integration-diagnostics/runs/{run_id}",
            get(get_integration_diagnostic_run),
        )
        .route(
            "/v1/integration-diagnostics/smoke",
            post(run_integration_diagnostic_smoke),
        )
        .route(
            "/v1/integration-diagnostics/retention/apply",
            post(apply_integration_diagnostic_retention),
        )
        .route(
            "/v1/integration-diagnostics/reason-codes",
            get(integration_diagnostic_reason_codes),
        )
        // Rust-surface placeholders with no Go handler source (see module
        // docs): static segments win over the {integration_id} capture.
        .route("/v1/integrations/sync", post(integration_sync))
        .route(
            "/v1/integrations/{integration_id}/adapter-rpc",
            post(integration_adapter_rpc),
        )
}

// ---------------------------------------------------------------------------
// /v1/integrations — list / create (Go handleIntegrations)
// ---------------------------------------------------------------------------

/// GET /v1/integrations — full list, or the caller's tenant list when a
/// tenant context is resolved. Tenant reads additionally require
/// credentials.inspect or integrations.manage (Go
/// requireHostedCredentialReadAny).
async fn list_integrations(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Json<IntegrationListResponse>, ApiError> {
    let manager = integrations_manager(&state)?;
    let items = match tenant {
        Some(tc) if !tc.0 .0.tenant_id.trim().is_empty() => {
            if !can_inspect_credentials(&tc.0 .0, &[Permission::IntegrationsManage]) {
                return Err(credential_denial());
            }
            manager.list_for_tenant(&tc.0 .0.tenant_id)
        }
        _ => manager.list(),
    };
    Ok(Json(IntegrationListResponse { items }))
}

/// POST /v1/integrations — create a resource (Go handleIntegrations POST
/// branch). Tenant mutations require integrations.manage; the resource is
/// persisted to the store and announced with integration.registered.
async fn create_integration(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<(StatusCode, Json<kura_integrations::Resource>), ApiError> {
    let manager = integrations_manager(&state)?;
    let mut tenant_id = String::new();
    if let Some(tc) = tenant.as_ref() {
        if !tc.0 .0.tenant_id.trim().is_empty() {
            require_permission(&tc.0 .0, Permission::IntegrationsManage)?;
            tenant_id = tc.0 .0.tenant_id.clone();
        }
    }
    let input: CreateIntegrationRequest = decode_json_body(&body)?;
    let item = manager
        .create(kura_integrations::CreateInput {
            tenant_id: tenant_id.clone(),
            integration_id: input.integration_id.clone(),
            domain_kind: input.domain_kind.clone(),
            display_name: input.display_name.clone(),
            account_binding: input.account_binding.clone(),
            backend_binding: kura_integrations::BackendBinding {
                backend_kind: input.backend_kind,
                backend_ref_id: input.backend_ref_id.clone(),
                backend_display_name: input.backend_display_name.clone(),
                source_kind: String::new(),
                supports_interactive_auth: false,
                // Go handleIntegrations: fake-local always supports probe
                // reads; otherwise the backend kind must support the domain.
                supports_probe_read: input.backend_kind
                    == kura_integrations::BackendKind::FakeLocal
                    || kura_integrations::backend_kind_supports_domain(
                        input.backend_kind,
                        &input.domain_kind,
                    ),
                supports_probe_mutation: input.backend_kind
                    == kura_integrations::BackendKind::FakeLocal,
            },
            canonical_default: input.canonical_default,
            environment_scope: environment_scope_from_config(&state.config),
        })
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    persist_integration(&state, &item)?;
    publish_integration_event(
        &state,
        tenant.as_ref().map(|e| &e.0),
        "integration.registered",
        &item,
    )?;
    Ok((StatusCode::CREATED, Json(item)))
}

// ---------------------------------------------------------------------------
// /v1/integrations/{id} — detail / disconnect (Go handleIntegrationRoutes)
// ---------------------------------------------------------------------------

/// GET /v1/integrations/{integration_id} — one resource. Tenant reads use
/// get_for_tenant (cross-tenant rows 404, never disclosed).
async fn get_integration(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    method: Method,
    uri: Uri,
    Path(integration_id): Path<String>,
) -> Result<Json<kura_integrations::Resource>, ApiError> {
    let manager = integrations_manager(&state)?;
    guard_integration_resource(
        &state,
        &method,
        &uri,
        tenant.as_ref().map(|e| &e.0),
        &integration_id,
    )
    .await?;
    let item = match tenant {
        Some(tc) if !tc.0 .0.tenant_id.trim().is_empty() => {
            if !can_inspect_credentials(&tc.0 .0, &[Permission::IntegrationsManage]) {
                return Err(credential_denial());
            }
            manager.get_for_tenant(&integration_id, &tc.0 .0.tenant_id)
        }
        _ => manager.get(&integration_id),
    };
    item.ok_or_else(|| ApiError::NotFound("not found".to_string()))
        .map(Json)
}

/// DELETE /v1/integrations/{integration_id} — disconnect (Go
/// handleIntegrationDisconnect). The reason query param defaults to "operator
/// disconnected integration".
async fn disconnect_integration(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    method: Method,
    uri: Uri,
    Path(integration_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<kura_integrations::Resource>, ApiError> {
    let manager = integrations_manager(&state)?;
    guard_integration_resource(
        &state,
        &method,
        &uri,
        tenant.as_ref().map(|e| &e.0),
        &integration_id,
    )
    .await?;
    if let Some(tc) = tenant.as_ref() {
        if !tc.0 .0.tenant_id.trim().is_empty() {
            require_permission(&tc.0 .0, Permission::IntegrationsManage)?;
        }
    }
    let reason = params
        .get("reason")
        .map(|raw| raw.trim().to_string())
        .filter(|trimmed| !trimmed.is_empty())
        .unwrap_or_else(|| "operator disconnected integration".to_string());
    let item = manager
        .disconnect(&integration_id, &reason)
        .map_err(map_integration_error)?;
    persist_integration(&state, &item)?;
    publish_integration_event(
        &state,
        tenant.as_ref().map(|e| &e.0),
        "integration.disconnected",
        &item,
    )?;
    Ok(Json(item))
}

// ---------------------------------------------------------------------------
// /v1/integrations/{id}/readiness (Go handleIntegrationReadiness)
// ---------------------------------------------------------------------------

/// POST /v1/integrations/{integration_id}/readiness — report readiness/
/// auth/health state. Mutations require integrations.manage and the resource
/// must belong to the caller's tenant (Go GetForTenant -> 404).
async fn update_integration_readiness(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    method: Method,
    uri: Uri,
    Path(integration_id): Path<String>,
    body: Bytes,
) -> Result<Json<kura_integrations::Resource>, ApiError> {
    let manager = integrations_manager(&state)?;
    guard_integration_resource(
        &state,
        &method,
        &uri,
        tenant.as_ref().map(|e| &e.0),
        &integration_id,
    )
    .await?;
    match tenant.as_ref() {
        Some(tc) if !tc.0 .0.tenant_id.trim().is_empty() => {
            require_permission(&tc.0 .0, Permission::IntegrationsManage)?;
            if manager
                .get_for_tenant(&integration_id, &tc.0 .0.tenant_id)
                .is_none()
            {
                return Err(ApiError::NotFound("not found".to_string()));
            }
        }
        _ => {
            if manager.get(&integration_id).is_none() {
                return Err(ApiError::NotFound("not found".to_string()));
            }
        }
    }
    let input: ReportIntegrationReadinessRequest = decode_json_body(&body)?;
    let item = manager
        .update_readiness(
            &integration_id,
            kura_integrations::UpdateReadinessInput {
                readiness_status: input.readiness_status,
                auth_state: input.auth_state.as_str().to_string(),
                health_state: input.health_state.as_str().to_string(),
                reason: input.reason.clone(),
                required_operator_action: input.required_operator_action.clone(),
                account_binding: Some(input.account_binding.clone()),
                secret_resolution: input.secret_resolution.clone(),
            },
        )
        .map_err(map_integration_error)?;
    persist_integration(&state, &item)?;
    publish_integration_event(
        &state,
        tenant.as_ref().map(|e| &e.0),
        "integration.updated",
        &item,
    )?;
    publish_integration_event(
        &state,
        tenant.as_ref().map(|e| &e.0),
        "integration.readiness_changed",
        &item,
    )?;
    Ok(Json(item))
}

// ---------------------------------------------------------------------------
// /v1/integrations/{id}/default (Go handleIntegrationDefault)
// ---------------------------------------------------------------------------

/// POST /v1/integrations/{integration_id}/default — promote the resource to
/// canonical default, demoting siblings in the same binding group (same
/// domain kind + environment scope + account key) and persisting every member
/// of the group.
async fn set_integration_default(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    method: Method,
    uri: Uri,
    Path(integration_id): Path<String>,
) -> Result<Json<kura_integrations::Resource>, ApiError> {
    let manager = integrations_manager(&state)?;
    guard_integration_resource(
        &state,
        &method,
        &uri,
        tenant.as_ref().map(|e| &e.0),
        &integration_id,
    )
    .await?;
    let tenant_id = match tenant.as_ref() {
        Some(tc) if !tc.0 .0.tenant_id.trim().is_empty() => {
            require_permission(&tc.0 .0, Permission::IntegrationsManage)?;
            if manager
                .get_for_tenant(&integration_id, &tc.0 .0.tenant_id)
                .is_none()
            {
                return Err(ApiError::NotFound("not found".to_string()));
            }
            tc.0 .0.tenant_id.clone()
        }
        _ => {
            if manager.get(&integration_id).is_none() {
                return Err(ApiError::NotFound("not found".to_string()));
            }
            String::new()
        }
    };
    let item = manager
        .set_canonical_default(&integration_id)
        .map_err(map_integration_error)?;
    // Go: persist every integration in the same binding group (the demotion
    // already happened inside the manager; persist the group so the store
    // matches the in-memory state).
    let to_persist = if tenant_id.is_empty() {
        manager.list()
    } else {
        manager.list_for_tenant(&tenant_id)
    };
    for integration in to_persist {
        if same_binding_group(&integration, &item) {
            persist_integration(&state, &integration)?;
        }
    }
    publish_integration_event(
        &state,
        tenant.as_ref().map(|e| &e.0),
        "integration.updated",
        &item,
    )?;
    publish_integration_event(
        &state,
        tenant.as_ref().map(|e| &e.0),
        "integration.default_changed",
        &item,
    )?;
    Ok(Json(item))
}

// ---------------------------------------------------------------------------
// Integration diagnostics (Go integration_diagnostics.go)
// ---------------------------------------------------------------------------

/// GET /v1/integrations/{integration_id}/diagnostics — latest diagnostic
/// state (Go handleIntegrationDiagnosticList): tenant context,
/// diagnostics.read permission, cross-tenant non-disclosure (404), then the
/// newest unexpired result rows for the integration (forceRefresh=true or an
/// empty store re-inspects through the DiagnosticManager and persists).
async fn integration_diagnostic_list(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    method: Method,
    uri: Uri,
    Path(integration_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<IntegrationDiagnosticListResponse>, ApiError> {
    let manager = integrations_manager(&state)?;
    guard_integration_resource(
        &state,
        &method,
        &uri,
        tenant.as_ref().map(|e| &e.0),
        &integration_id,
    )
    .await?;
    let tc = require_tenant(tenant.as_ref().map(|e| &e.0))?;
    require_permission(tc, Permission::IntegrationDiagnosticsRead)?;
    // Go handleIntegrationDiagnosticList: GetForTenant before touching the
    // store — cross-tenant lookups 404 without disclosing the row.
    let resource = manager
        .get_for_tenant(&integration_id, &tc.tenant_id)
        .ok_or_else(|| ApiError::NotFound("not found".to_string()))?;
    let now = Utc::now();
    let limit = parse_int_default(query.get("limit").map(String::as_str).unwrap_or(""), 50);
    let mut items = state
        .store
        .lock()
        .latest_integration_diagnostic_results(
            &DiagnosticResultFilter {
                tenant_id: tc.tenant_id.clone(),
                integration_id: integration_id.clone(),
                limit,
                include_expired: false,
                ..DiagnosticResultFilter::default()
            },
            now,
        )
        .map_err(ApiError::from_store)?;
    let force_refresh = query
        .get("forceRefresh")
        .map(|raw| raw.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if items.is_empty() || force_refresh {
        let result = DiagnosticManager::new().inspect(DiagnosticInspectionInput {
            resource: resource.clone(),
            capability: query.get("capability").cloned().unwrap_or_default(),
            checked_at: now,
            evidence_text: resource.readiness_reason.clone(),
            ..DiagnosticInspectionInput::default()
        });
        state
            .store
            .lock()
            .save_integration_diagnostic_result(&result)
            .map_err(ApiError::from_store)?;
        publish_diagnostic_result_events(&state, tenant.as_ref().map(|e| &e.0), &result)?;
        items = vec![result];
    }
    Ok(Json(IntegrationDiagnosticListResponse {
        integration_id,
        tenant_id: tc.tenant_id.clone(),
        freshness_summary: "latest diagnostic state".to_string(),
        items,
        next_cursor: String::new(),
    }))
}

/// POST /v1/integrations/{integration_id}/diagnostics/runs — start a
/// diagnostic run (Go handleCreateIntegrationDiagnosticRun): persist the
/// running run, inspect every checked capability (probing probe-capable
/// backends), persist the results, then persist the completed run.
async fn create_integration_diagnostic_run(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    method: Method,
    uri: Uri,
    Path(integration_id): Path<String>,
    body: Bytes,
) -> Result<(StatusCode, Json<DiagnosticRun>), ApiError> {
    let manager = integrations_manager(&state)?;
    guard_integration_resource(
        &state,
        &method,
        &uri,
        tenant.as_ref().map(|e| &e.0),
        &integration_id,
    )
    .await?;
    let tc = require_tenant(tenant.as_ref().map(|e| &e.0))?;
    require_permission(tc, Permission::IntegrationDiagnosticsRun)?;
    let resource = manager
        .get_for_tenant(&integration_id, &tc.tenant_id)
        .ok_or_else(|| ApiError::NotFound("not found".to_string()))?;
    let input: CreateIntegrationDiagnosticRunRequest = decode_json_body(&body)?;
    if input.client_key.trim().is_empty() {
        return Err(ApiError::BadRequest("clientKey is required".to_string()));
    }
    let now = Utc::now();
    let diagnostic_manager = DiagnosticManager::new();
    let mut run = diagnostic_manager.create_run(DiagnosticRunInput {
        resource: resource.clone(),
        requested_by: tc.principal_id.clone(),
        client_key: input.client_key.clone(),
        capabilities: input.capabilities.clone(),
        trigger: "operator_inspection".to_string(),
        started_at: now,
    });
    state
        .store
        .lock()
        .save_integration_diagnostic_run(&run)
        .map_err(ApiError::from_store)?;
    publish_event(
        &state,
        tenant.as_ref().map(|e| &e.0),
        events::integration_diagnostic_run_event(
            events::INTEGRATION_DIAGNOSTIC_RUN_STARTED_NAME,
            run.clone(),
        ),
    )?;
    let mut results: Vec<DiagnosticResult> = Vec::new();
    for capability in &run.checked_capabilities {
        let result = inspect_diagnostic_capability(
            Some(manager),
            &diagnostic_manager,
            &resource,
            DiagnosticInspectionInput {
                resource: resource.clone(),
                capability: capability.clone(),
                run_id: run.diagnostic_run_id.clone(),
                checked_at: now,
                evidence_text: first_non_empty(&[&resource.readiness_reason, &input.reason]),
                ..DiagnosticInspectionInput::default()
            },
        );
        state
            .store
            .lock()
            .save_integration_diagnostic_result(&result)
            .map_err(ApiError::from_store)?;
        results.push(result.clone());
        publish_diagnostic_result_events(&state, tenant.as_ref().map(|e| &e.0), &result)?;
    }
    run = complete_diagnostic_run(run, &results, now);
    state
        .store
        .lock()
        .save_integration_diagnostic_run(&run)
        .map_err(ApiError::from_store)?;
    publish_event(
        &state,
        tenant.as_ref().map(|e| &e.0),
        events::integration_diagnostic_run_event(
            events::INTEGRATION_DIAGNOSTIC_RUN_COMPLETED_NAME,
            run.clone(),
        ),
    )?;
    Ok((StatusCode::CREATED, Json(run)))
}

/// GET /v1/integration-diagnostics/runs — diagnostic-run list (Go
/// handleIntegrationDiagnosticRuns list branch) on
/// list_integration_diagnostic_runs with the query filters.
async fn list_integration_diagnostic_runs(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<IntegrationDiagnosticRunListResponse>, ApiError> {
    let tc = require_tenant(tenant.as_ref().map(|e| &e.0))?;
    require_permission(tc, Permission::IntegrationDiagnosticsRead)?;
    let items = state
        .store
        .lock()
        .list_integration_diagnostic_runs(
            &DiagnosticRunFilter {
                tenant_id: tc.tenant_id.clone(),
                integration_id: query.get("integrationId").cloned().unwrap_or_default(),
                provider_kind: query.get("providerKind").cloned().unwrap_or_default(),
                domain_kind: query.get("domainKind").cloned().unwrap_or_default(),
                status: parse_diagnostic_run_status(
                    query.get("status").map(String::as_str).unwrap_or(""),
                )
                .unwrap_or_default(),
                reason_code: parse_diagnostic_reason_code(
                    query.get("reasonCode").map(String::as_str).unwrap_or(""),
                )
                .unwrap_or_default(),
                limit: parse_int_default(query.get("limit").map(String::as_str).unwrap_or(""), 50),
                include_expired: false,
                ..DiagnosticRunFilter::default()
            },
            Utc::now(),
        )
        .map_err(ApiError::from_store)?;
    Ok(Json(IntegrationDiagnosticRunListResponse {
        items,
        next_cursor: String::new(),
    }))
}

/// GET /v1/integration-diagnostics/runs/{run_id} — run detail (Go
/// handleIntegrationDiagnosticRuns detail branch): tenant-scoped, expired
/// runs hidden.
async fn get_integration_diagnostic_run(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(run_id): Path<String>,
) -> Result<Json<DiagnosticRun>, ApiError> {
    let tc = require_tenant(tenant.as_ref().map(|e| &e.0))?;
    require_permission(tc, Permission::IntegrationDiagnosticsRead)?;
    let item = state
        .store
        .lock()
        .get_integration_diagnostic_run(&tc.tenant_id, &run_id, false, Utc::now())
        .map_err(ApiError::from_store)?
        .ok_or_else(|| ApiError::NotFound("not found".to_string()))?;
    Ok(Json(item))
}

/// POST /v1/integration-diagnostics/smoke — smoke report (Go
/// handleIntegrationDiagnosticSmoke). Tenant/permission/risky-probe gates,
/// probe inputs built through the integrations manager, per-probe diagnostic
/// results persisted, smoke-completed event published. The smoke report row
/// itself (Go SaveSmokeMatrixReport) has no kura-store DAO yet, so it is not
/// persisted.
async fn run_integration_diagnostic_smoke(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<(StatusCode, AxumJson<serde_json::Value>), ApiError> {
    let manager = integrations_manager(&state)?;
    let tc = require_tenant(tenant.as_ref().map(|e| &e.0))?;
    require_permission(tc, Permission::IntegrationDiagnosticsSmoke)?;
    let input: CreateIntegrationDiagnosticSmokeRequest = decode_json_body(&body)?;
    // Go smokeRequestContainsRiskyProbe: any probe that is not read-only or
    // reversible additionally requires diagnostics.smoke.risky.
    if smoke_request_contains_risky_probe(&input) {
        require_permission(tc, Permission::IntegrationDiagnosticsSmokeRisky)?;
    }
    // Go: `"smoke_" + strconv.FormatInt(time.Now().UTC().UnixNano(), 36)`.
    let report_id = if input.report_id.trim().is_empty() {
        format!(
            "smoke_{}",
            base36(Utc::now().timestamp_nanos_opt().unwrap_or_default())
        )
    } else {
        input.report_id.trim().to_string()
    };
    let now = Utc::now();
    let (probes, resources) = build_smoke_probe_inputs(manager, tc, &input)?;
    let outcomes: Vec<SmokeOutcomeBuild> = probes
        .iter()
        .enumerate()
        .map(|(index, probe)| smoke_probe_outcome(probe, &report_id, index, now))
        .collect();
    let report = build_smoke_report_json(&report_id, tc, now, &outcomes);
    // Go SaveSmokeMatrixReport has no Rust DAO yet; only the per-probe
    // diagnostic results below are persisted.
    let diagnostic_manager = DiagnosticManager::new();
    for (index, outcome) in outcomes.iter().enumerate() {
        let resource = &resources[index];
        let mut result = diagnostic_manager.inspect(DiagnosticInspectionInput {
            resource: resource.clone(),
            capability: outcome.probe_action.clone(),
            checked_at: outcome.checked_at,
            evidence_text: outcome.reason.as_str().to_string(),
            ..DiagnosticInspectionInput::default()
        });
        let (status, owner, retry_safety) = diagnostic_defaults(outcome.reason);
        result.status = status;
        result.reason_code = outcome.reason;
        result.remediation_owner = owner;
        result.retry_safety = retry_safety;
        result.remediation_hint = diagnostic_remediation_hint(outcome.reason);
        result.smoke_report_id = report_id.clone();
        result.artifact_refs = outcome.artifact_refs.clone();
        state
            .store
            .lock()
            .save_integration_diagnostic_result(&result)
            .map_err(ApiError::from_store)?;
        publish_diagnostic_result_events(&state, tenant.as_ref().map(|e| &e.0), &result)?;
    }
    // Go events.IntegrationDiagnosticSmokeCompletedEvent(report).
    let payload = serde_json::json!({
        "tenantId": tc.tenant_id,
        "smokeReportId": report_id,
        "status": report["status"],
        "domainSummary": report["domainSummary"],
        "artifactRefs": report["artifactRefs"],
    });
    publish_event(
        &state,
        tenant.as_ref().map(|e| &e.0),
        events::Event {
            tenant_id: tc.tenant_id.clone(),
            category: "integration".to_string(),
            name: events::INTEGRATION_DIAGNOSTIC_SMOKE_COMPLETED_NAME.to_string(),
            occurred_at: now,
            resource: events::Resource {
                kind: "integration_diagnostic_smoke_report".to_string(),
                id: report_id,
            },
            payload: payload.as_object().cloned().unwrap_or_default(),
            ..events::Event::default()
        },
    )?;
    Ok((StatusCode::CREATED, AxumJson(report)))
}

/// POST /v1/integration-diagnostics/retention/apply — flip every expired
/// active retention record to Expired (Go handleIntegrationDiagnosticRetentionApply)
/// and publish a retention-applied event per record.
async fn apply_integration_diagnostic_retention(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<(StatusCode, AxumJson<serde_json::Value>), ApiError> {
    let tc = require_tenant(tenant.as_ref().map(|e| &e.0))?;
    require_permission(tc, Permission::IntegrationDiagnosticsRun)?;
    let limit = parse_int_default(query.get("limit").map(String::as_str).unwrap_or(""), 50);
    let items = state
        .store
        .lock()
        .apply_expired_diagnostic_retention_records(&tc.tenant_id, Utc::now(), limit)
        .map_err(ApiError::from_store)?;
    for item in &items {
        publish_event(
            &state,
            tenant.as_ref().map(|e| &e.0),
            events::integration_diagnostic_retention_applied_event(item.clone()),
        )?;
    }
    Ok((StatusCode::OK, AxumJson(serde_json::json!({ "items": items }))))
}

/// GET /v1/integration-diagnostics/reason-codes — the reason-code catalog
/// (Go handleIntegrationDiagnosticReasonCodes). Fully ported.
async fn integration_diagnostic_reason_codes(
) -> Result<Json<ListResponse<kura_integrations::DiagnosticReasonCodeDefinition>>, ApiError> {
    Ok(Json(ListResponse {
        items: kura_integrations::default_diagnostic_reason_code_catalog(),
    }))
}

// ---------------------------------------------------------------------------
// Placeholder surfaces without a Go handler (see module docs)
// ---------------------------------------------------------------------------

/// POST /v1/integrations/sync — no Go route exists (the Go daemon restores
/// the manager from the store at startup; there is no sync endpoint). 501
/// marker until a spec defines the surface.
async fn integration_sync(
    State(state): State<AppState>,
) -> Result<(StatusCode, AxumJson<serde_json::Value>), ApiError> {
    let _ = &state;
    Ok(not_implemented(
        "integration_sync_not_implemented",
        "integration store sync has no Go handler to port",
    ))
}

/// POST /v1/integrations/{integration_id}/adapter-rpc — `adapter_rpc` is an
/// integrations.BackendKind (docs/runtime/integration-adapter-plane.md) used
/// by the calendar/mail managers; the Go API surface has no REST handler for
/// it. 501 marker.
async fn integration_adapter_rpc(
    State(state): State<AppState>,
    Path(_integration_id): Path<String>,
) -> Result<(StatusCode, AxumJson<serde_json::Value>), ApiError> {
    let _ = &state;
    Ok(not_implemented(
        "integration_adapter_rpc_not_implemented",
        "adapter rpc has no Go HTTP handler to port",
    ))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Go handleIntegrations / handleIntegrationRoutes nil-manager branch: 500
/// with the stable message.
fn integrations_manager(state: &AppState) -> Result<&kura_integrations::Manager, ApiError> {
    state
        .integrations
        .as_deref()
        .ok_or_else(|| ApiError::Internal("integrations manager is not configured".to_string()))
}

/// Go withByIDTenantGuard for the integrations table.
async fn guard_integration_resource(
    state: &AppState,
    method: &Method,
    uri: &Uri,
    tenant: Option<&TenantContext>,
    integration_id: &str,
) -> Result<(), ApiError> {
    let surface = format!("api:{} {}", method.as_str(), uri.path());
    guard_resource_for_tenant(
        state,
        tenant,
        &surface,
        "integrations",
        "integration_id",
        integration_id,
        "integration",
    )
    .await
}

/// Go requireHostedCredentialPermission: resolved tenant + exact permission.
fn require_permission(
    tc: &kura_identity::TenantContext,
    permission: Permission,
) -> Result<(), ApiError> {
    if !has_permission(&tc.permissions, permission) {
        return Err(credential_denial());
    }
    Ok(())
}

/// Go tenant-context-required gate for the diagnostics handlers
/// (writeCredentialDenial(403, "tenant_context_missing")).
fn require_tenant(
    tenant: Option<&TenantContext>,
) -> Result<&kura_identity::TenantContext, ApiError> {
    let Some(tc) = tenant else {
        return Err(credential_denial());
    };
    if tc.0.tenant_id.trim().is_empty() {
        return Err(credential_denial());
    }
    Ok(&tc.0)
}

/// Go writeCredentialDenial: the stable error string is
/// `credential_access_denied` (the Go body also carries a reasonCode; the
/// existing route-family ports use the stable string alone).
fn credential_denial() -> ApiError {
    ApiError::Forbidden("credential_access_denied".to_string())
}

/// Go handleIntegrationDisconnect / readiness / default error mapping:
/// IntegrationNotFound -> 404, everything else -> 400.
fn map_integration_error(err: kura_integrations::IntegrationError) -> ApiError {
    match err {
        kura_integrations::IntegrationError::IntegrationNotFound => {
            ApiError::NotFound("not found".to_string())
        }
        other => ApiError::BadRequest(other.to_string()),
    }
}

/// Go handleIntegrationDefault binding group: same domain kind, environment
/// scope and account key.
fn same_binding_group(
    left: &kura_integrations::Resource,
    right: &kura_integrations::Resource,
) -> bool {
    left.domain_kind.trim() == right.domain_kind.trim()
        && left.environment_scope.trim() == right.environment_scope.trim()
        && account_key(left) == account_key(right)
}

fn account_key(item: &kura_integrations::Resource) -> String {
    item.account_binding
        .as_ref()
        .map(|binding| binding.account_key.trim().to_string())
        .unwrap_or_default()
}

/// Go persistIntegration: upsert the resource document (the Rust store writes
/// the tenant column as NULL until tenancy wiring; the document carries the
/// tenant id).
fn persist_integration(
    state: &AppState,
    item: &kura_integrations::Resource,
) -> Result<(), ApiError> {
    state
        .store
        .lock()
        .upsert_integration(item)
        .map_err(ApiError::from_store)
}

/// Go publishEvent for the integration category: binds environment scope +
/// tenant, persists (tenant-owned path), then publishes on the bus. The
/// integration category is not global, so a resolved tenant is bound.
fn publish_integration_event(
    state: &AppState,
    tenant: Option<&TenantContext>,
    name: &str,
    item: &kura_integrations::Resource,
) -> Result<(), ApiError> {
    let account_key = account_key(item);
    let payload = match name {
        "integration.registered" | "integration.updated" => serde_json::json!({
            "integrationId": item.integration_id,
            "domainKind": item.domain_kind,
            "displayName": item.display_name,
            "environmentScope": item.environment_scope,
            "readinessStatus": item.readiness_status,
            "canonicalDefault": item.canonical_default,
            "backendKind": item.backend_binding.backend_kind,
            "accountKey": account_key,
        }),
        "integration.readiness_changed" => serde_json::json!({
            "integrationId": item.integration_id,
            "readinessStatus": item.readiness_status,
            "authState": item.auth_state,
            "healthState": item.health_state,
            "reason": item.readiness_reason,
            "requiredOperatorAction": item.required_operator_action,
            "accountKey": account_key,
            "backendKind": item.backend_binding.backend_kind,
        }),
        "integration.disconnected" => serde_json::json!({
            "tenantId": item.tenant_id,
            "integrationId": item.integration_id,
            "readinessStatus": item.readiness_status,
            "authState": item.auth_state,
            "healthState": item.health_state,
            "disabledReason": item.disabled_reason,
        }),
        "integration.default_changed" => serde_json::json!({
            "integrationId": item.integration_id,
            "domainKind": item.domain_kind,
            "environmentScope": item.environment_scope,
            "accountKey": account_key,
            "canonicalDefault": item.canonical_default,
        }),
        _ => serde_json::json!({}),
    };
    publish_event(
        state,
        tenant,
        events::Event {
            category: "integration".to_string(),
            name: name.to_string(),
            resource: events::Resource {
                kind: "integration".to_string(),
                id: item.integration_id.clone(),
            },
            payload: payload.as_object().cloned().unwrap_or_default(),
            ..events::Event::default()
        },
    )
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

/// Go smokeRequestContainsRiskyProbe: any probe that is not read-only or
/// reversible makes the request "risky".
fn smoke_request_contains_risky_probe(
    input: &crate::types::CreateIntegrationDiagnosticSmokeRequest,
) -> bool {
    input
        .probes
        .iter()
        .any(|probe| !probe.read_only_or_reversible)
}

/// Go shouldRunSmokeProbe: a probe runs unless deferred, unsupported, or a
/// required credential/approval gate is closed; mutating probes additionally
/// need both tenant-admin and operator approval.
fn should_run_smoke_probe(probe: &CreateIntegrationDiagnosticSmokeProbe) -> bool {
    !probe.operator_deferred
        && probe.supported
        && probe.safe_credentials_available
        && probe.tenant_approval_available
        && probe.provider_available
        && (probe.read_only_or_reversible || (probe.tenant_admin_approved && probe.operator_approved))
}

/// Go parseIntDefault: a missing/invalid/non-positive value falls back.
fn parse_int_default(raw: &str, fallback: i64) -> i64 {
    match raw.trim().parse::<i64>() {
        Ok(parsed) if parsed > 0 => parsed,
        _ => fallback,
    }
}

/// Wire-literal -> DiagnosticRunStatus (Go casts the string; unknown values
/// fall back to the default variant, matching the store's filter convention).
fn parse_diagnostic_run_status(raw: &str) -> Option<DiagnosticRunStatus> {
    match raw.trim() {
        "queued" => Some(DiagnosticRunStatus::Queued),
        "running" => Some(DiagnosticRunStatus::Running),
        "completed" => Some(DiagnosticRunStatus::Completed),
        "failed" => Some(DiagnosticRunStatus::Failed),
        "blocked" => Some(DiagnosticRunStatus::Blocked),
        _ => None,
    }
}

/// Wire-literal -> DiagnosticReasonCode (the Rust enum is closed; unknown
/// literals return None so callers can pick a stable fallback).
fn parse_diagnostic_reason_code(raw: &str) -> Option<DiagnosticReasonCode> {
    match raw.trim() {
        "healthy" => Some(DiagnosticReasonCode::Healthy),
        "app_authorization_missing" => Some(DiagnosticReasonCode::AppAuthorizationMissing),
        "bot_authorization_missing" => Some(DiagnosticReasonCode::BotAuthorizationMissing),
        "user_authorization_missing" => Some(DiagnosticReasonCode::UserAuthorizationMissing),
        "tenant_approval_pending" => Some(DiagnosticReasonCode::TenantApprovalPending),
        "scope_missing" => Some(DiagnosticReasonCode::ScopeMissing),
        "token_missing" => Some(DiagnosticReasonCode::TokenMissing),
        "token_expired" => Some(DiagnosticReasonCode::TokenExpired),
        "token_revoked" => Some(DiagnosticReasonCode::TokenRevoked),
        "refresh_credentials_missing" => Some(DiagnosticReasonCode::RefreshCredentialsMissing),
        "token_refresh_failed" => Some(DiagnosticReasonCode::TokenRefreshFailed),
        "tenant_mismatch" => Some(DiagnosticReasonCode::TenantMismatch),
        "rate_limited" => Some(DiagnosticReasonCode::RateLimited),
        "provider_unavailable" => Some(DiagnosticReasonCode::ProviderUnavailable),
        "transient_provider_failure" => Some(DiagnosticReasonCode::TransientProviderFailure),
        "network_failed" => Some(DiagnosticReasonCode::NetworkFailed),
        "ambiguous_downstream_commit" => Some(DiagnosticReasonCode::AmbiguousDownstreamCommit),
        "unsafe_to_retry" => Some(DiagnosticReasonCode::UnsafeToRetry),
        "operator_action_needed" => Some(DiagnosticReasonCode::OperatorActionNeeded),
        "limited_diagnostic" => Some(DiagnosticReasonCode::LimitedDiagnostic),
        "unsupported_diagnostic" => Some(DiagnosticReasonCode::UnsupportedDiagnostic),
        "redaction_failed_closed" => Some(DiagnosticReasonCode::RedactionFailedClosed),
        "unknown_provider_error" => Some(DiagnosticReasonCode::UnknownProviderError),
        _ => None,
    }
}

/// Wire-literal -> RedactionStatus.
fn parse_redaction_status(raw: &str) -> Option<RedactionStatus> {
    match raw.trim() {
        "redacted" => Some(RedactionStatus::Redacted),
        "suppressed" => Some(RedactionStatus::Suppressed),
        "failed_closed" => Some(RedactionStatus::FailedClosed),
        _ => None,
    }
}

/// Go diagnosticReasonFromProbeResult: the probe's reasonCode summary wins;
/// otherwise the failure class is classified; otherwise healthy.
fn diagnostic_reason_from_probe_result(result: &ProbeResult) -> DiagnosticReasonCode {
    if let Some(raw) = result.result_summary.get("reasonCode").and_then(Value::as_str) {
        if !raw.trim().is_empty() {
            return parse_diagnostic_reason_code(raw).unwrap_or(DiagnosticReasonCode::Healthy);
        }
    }
    if !result.failure_class.trim().is_empty() {
        return classify_provider_evidence(&ProviderDiagnosticEvidence {
            provider_error_class: result.failure_class.clone(),
            message: result.failure_class.clone(),
            ..ProviderDiagnosticEvidence::default()
        })
        .reason_code;
    }
    DiagnosticReasonCode::Healthy
}

/// Go inspectDiagnosticCapability: inspect, then overlay probe evidence when
/// the backend supports probe reads.
fn inspect_diagnostic_capability(
    manager: Option<&kura_integrations::Manager>,
    diagnostic_manager: &DiagnosticManager,
    resource: &kura_integrations::Resource,
    input: DiagnosticInspectionInput,
) -> DiagnosticResult {
    let mut result = diagnostic_manager.inspect(input.clone());
    let Some(manager) = manager else {
        return result;
    };
    if !resource.backend_binding.supports_probe_read {
        return result;
    }
    let mut probe_input = Map::new();
    probe_input.insert(
        "operationClass".to_string(),
        Value::String(input.capability.clone()),
    );
    if !input.evidence_text.trim().is_empty() {
        let mut evidence = Map::new();
        evidence.insert("code".to_string(), Value::String(input.evidence_text.clone()));
        evidence.insert("message".to_string(), Value::String(input.evidence_text.clone()));
        probe_input.insert("providerEvidence".to_string(), Value::Object(evidence));
    }
    let Ok((_, probe_result, _)) =
        manager.run_probe(&resource.integration_id, ProbeKind::Inspect, &probe_input)
    else {
        return result;
    };
    let reason = diagnostic_reason_from_probe_result(&probe_result);
    let (status, owner, retry_safety) = diagnostic_defaults(reason);
    result.status = status;
    result.reason_code = reason;
    result.remediation_owner = owner;
    result.retry_safety = retry_safety;
    result.remediation_hint = diagnostic_remediation_hint(reason);
    result.evidence_summary = reason.as_str().to_string();
    if let Some(raw) = probe_result
        .result_summary
        .get("redactionStatus")
        .and_then(Value::as_str)
    {
        if !raw.trim().is_empty() {
            result.redaction_status =
                parse_redaction_status(raw).unwrap_or(result.redaction_status);
        }
    }
    result
}

/// Go publishIntegrationDiagnosticResultEventsAndAudit: the event fan-out
/// (state-changed + redaction-failed) per persisted diagnostic result. The Go
/// tenant-audit write has no kura-store DAO yet and is not ported.
fn publish_diagnostic_result_events(
    state: &AppState,
    tenant: Option<&TenantContext>,
    result: &DiagnosticResult,
) -> Result<(), ApiError> {
    publish_event(
        state,
        tenant,
        events::integration_diagnostic_state_changed_event(result.clone(), DiagnosticStatus::Unknown),
    )?;
    if result.redaction_status == RedactionStatus::FailedClosed {
        publish_event(
            state,
            tenant,
            events::integration_diagnostic_redaction_failed_event(result.clone()),
        )?;
    }
    Ok(())
}

/// Build input for a smoke probe (Go buildSmokeProbeInputs): resolves the
/// integration, runs the probe when the gate opens, and computes the reason.
struct SmokeProbe {
    integration_id: String,
    integration_account_id: String,
    domain_kind: String,
    provider_kind: String,
    probe_action: String,
    safe_credentials_available: bool,
    tenant_approval_available: bool,
    provider_available: bool,
    supported: bool,
    read_only_or_reversible: bool,
    tenant_admin_approved: bool,
    operator_approved: bool,
    operator_deferred: bool,
    reason_code: DiagnosticReasonCode,
    artifact_refs: Vec<String>,
    checked_at: DateTime<Utc>,
}

/// Go buildSmokeProbeInputs. Cross-tenant probes 404 (never disclosed).
fn build_smoke_probe_inputs(
    manager: &kura_integrations::Manager,
    tc: &kura_identity::TenantContext,
    input: &CreateIntegrationDiagnosticSmokeRequest,
) -> Result<(Vec<SmokeProbe>, Vec<kura_integrations::Resource>), ApiError> {
    let probe_requests: Vec<CreateIntegrationDiagnosticSmokeProbe> = if input.probes.is_empty() {
        // Go buildSmokeProbeInputs: a missing probe list defaults to one
        // fully-gated read-only probe for the request integration.
        vec![CreateIntegrationDiagnosticSmokeProbe {
            integration_id: input.integration_id.clone(),
            domain_kind: String::new(),
            probe_action: String::new(),
            safe_credentials_available: true,
            tenant_approval_available: true,
            provider_available: true,
            supported: true,
            read_only_or_reversible: true,
            tenant_admin_approved: false,
            operator_approved: false,
            operator_deferred: false,
            reason_code: String::new(),
            provider_evidence: None,
            artifact_refs: Vec::new(),
        }]
    } else {
        input.probes.clone()
    };
    let mut probes = Vec::with_capacity(probe_requests.len());
    let mut resources = Vec::with_capacity(probe_requests.len());
    for probe_request in probe_requests {
        let integration_id = first_non_empty(&[
            probe_request.integration_id.trim(),
            input.integration_id.trim(),
        ]);
        let resource = manager
            .get_for_tenant(&integration_id, &tc.tenant_id)
            .ok_or_else(|| ApiError::NotFound("not found".to_string()))?;
        let mut reason = parse_diagnostic_reason_code(&probe_request.reason_code)
            .unwrap_or(DiagnosticReasonCode::Healthy);
        let mut artifact_refs = probe_request.artifact_refs.clone();
        if should_run_smoke_probe(&probe_request) {
            let probe_kind = if probe_request.read_only_or_reversible {
                ProbeKind::Inspect
            } else {
                ProbeKind::Mutate
            };
            let mut probe_input = Map::new();
            probe_input.insert(
                "probeAction".to_string(),
                Value::String(probe_request.probe_action.clone()),
            );
            probe_input.insert(
                "operationClass".to_string(),
                Value::String(probe_request.probe_action.clone()),
            );
            if let Some(evidence) = probe_request.provider_evidence.as_ref() {
                if !evidence.is_empty() {
                    probe_input.insert("providerEvidence".to_string(), Value::Object(evidence.clone()));
                }
            }
            match manager.run_probe(&resource.integration_id, probe_kind, &probe_input) {
                Ok((_, probe_result, _)) => {
                    if reason == DiagnosticReasonCode::Healthy {
                        reason = diagnostic_reason_from_probe_result(&probe_result);
                    }
                    artifact_refs.push(format!(
                        "probe:{}:{}",
                        resource.integration_id,
                        probe_kind.as_str()
                    ));
                }
                Err(err) if is_unavailable_probe_error(&err) => {
                    reason = DiagnosticReasonCode::OperatorActionNeeded;
                }
                Err(_) => {
                    reason = DiagnosticReasonCode::UnsupportedDiagnostic;
                }
            }
        }
        probes.push(SmokeProbe {
            integration_id: resource.integration_id.clone(),
            integration_account_id: resource
                .account_binding
                .as_ref()
                .map(|binding| binding.account_key.clone())
                .unwrap_or_default(),
            domain_kind: first_non_empty(&[probe_request.domain_kind.trim(), &resource.domain_kind]),
            provider_kind: resource.backend_binding.backend_kind.as_str().to_string(),
            probe_action: probe_request.probe_action.trim().to_string(),
            safe_credentials_available: probe_request.safe_credentials_available,
            tenant_approval_available: probe_request.tenant_approval_available,
            provider_available: probe_request.provider_available,
            supported: probe_request.supported,
            read_only_or_reversible: probe_request.read_only_or_reversible,
            tenant_admin_approved: probe_request.tenant_admin_approved,
            operator_approved: probe_request.operator_approved,
            operator_deferred: probe_request.operator_deferred,
            reason_code: reason,
            artifact_refs,
            checked_at: Utc::now(),
        });
        resources.push(resource);
    }
    Ok((probes, resources))
}

/// A built smoke probe outcome (Go opsreadiness.SmokeProbeOutcome shape),
/// carrying the enum reason for the diagnostic-result overlay.
struct SmokeOutcomeBuild {
    probe_outcome_id: String,
    integration_id: String,
    integration_account_id: String,
    domain_kind: String,
    provider_kind: String,
    probe_action: String,
    result: String,
    reason: DiagnosticReasonCode,
    blocked_or_skipped_reason: String,
    artifact_refs: Vec<String>,
    checked_at: DateTime<Utc>,
}

/// Go opsreadiness.buildSmokeProbeOutcome: gate order drives the outcome; a
/// non-healthy reason after all gates pass marks the probe failed.
fn smoke_probe_outcome(
    probe: &SmokeProbe,
    report_id: &str,
    index: usize,
    fallback_time: DateTime<Utc>,
) -> SmokeOutcomeBuild {
    let checked_at = if probe.checked_at == DateTime::<Utc>::default() {
        fallback_time
    } else {
        probe.checked_at
    };
    let (mut result, mut blocked_reason, mut reason) = (
        "passed".to_string(),
        String::new(),
        probe.reason_code,
    );
    if probe.operator_deferred {
        result = "skipped".to_string();
        blocked_reason = "operator_deferred".to_string();
        reason = DiagnosticReasonCode::OperatorActionNeeded;
    } else if !probe.supported {
        result = "skipped".to_string();
        blocked_reason = "unsupported_domain".to_string();
        reason = DiagnosticReasonCode::UnsupportedDiagnostic;
    } else if !probe.safe_credentials_available {
        result = "blocked".to_string();
        blocked_reason = "missing_safe_credentials".to_string();
        reason = DiagnosticReasonCode::TokenMissing;
    } else if !probe.tenant_approval_available {
        result = "blocked".to_string();
        blocked_reason = "tenant_approval_unavailable".to_string();
        reason = DiagnosticReasonCode::TenantApprovalPending;
    } else if !probe.provider_available {
        result = "blocked".to_string();
        blocked_reason = "provider_outage".to_string();
        reason = DiagnosticReasonCode::ProviderUnavailable;
    } else if !probe.read_only_or_reversible && !probe.tenant_admin_approved {
        result = "blocked".to_string();
        blocked_reason = "missing_tenant_admin_approval".to_string();
        reason = DiagnosticReasonCode::UnsafeToRetry;
    } else if !probe.read_only_or_reversible && !probe.operator_approved {
        result = "blocked".to_string();
        blocked_reason = "missing_operator_approval".to_string();
        reason = DiagnosticReasonCode::UnsafeToRetry;
    } else if reason != DiagnosticReasonCode::Healthy {
        result = "failed".to_string();
    }
    SmokeOutcomeBuild {
        probe_outcome_id: format!("{report_id}_probe_{}", index + 1),
        integration_id: probe.integration_id.clone(),
        integration_account_id: probe.integration_account_id.clone(),
        domain_kind: probe.domain_kind.clone(),
        provider_kind: probe.provider_kind.clone(),
        probe_action: probe.probe_action.clone(),
        result,
        reason,
        blocked_or_skipped_reason: blocked_reason,
        artifact_refs: probe.artifact_refs.clone(),
        checked_at,
    }
}

/// Go opsreadiness.BuildIntegrationDiagnosticSmokeReport: the report JSON
/// (status/domainSummary/artifactRefs + probe outcomes) in the Go wire shape.
fn build_smoke_report_json(
    report_id: &str,
    tc: &kura_identity::TenantContext,
    started_at: DateTime<Utc>,
    outcomes: &[SmokeOutcomeBuild],
) -> serde_json::Value {
    let mut report_status = "completed";
    for outcome in outcomes {
        if outcome.result == "failed" {
            report_status = "failed";
        } else if outcome.result == "blocked" && report_status == "completed" {
            report_status = "blocked";
        }
    }
    let mut domain_summary = Map::new();
    let mut artifact_refs: Vec<String> = Vec::new();
    for outcome in outcomes {
        domain_summary.insert(
            outcome.domain_kind.clone(),
            Value::String(outcome.result.clone()),
        );
        artifact_refs.extend(outcome.artifact_refs.iter().cloned());
    }
    let probe_outcomes: Vec<Value> = outcomes
        .iter()
        .map(|outcome| {
            let (_, _owner, retry_safety) = diagnostic_defaults(outcome.reason);
            serde_json::json!({
                "probeOutcomeId": outcome.probe_outcome_id,
                "tenantId": tc.tenant_id,
                "smokeReportId": report_id,
                "integrationId": outcome.integration_id,
                "integrationAccountId": outcome.integration_account_id,
                "domainKind": outcome.domain_kind,
                "providerKind": outcome.provider_kind,
                "probeAction": outcome.probe_action,
                "result": outcome.result,
                "reasonCode": outcome.reason.as_str(),
                "remediationHint": diagnostic_remediation_hint(outcome.reason),
                "retrySafety": retry_safety.as_str(),
                "blockedOrSkippedReason": outcome.blocked_or_skipped_reason,
                "approvalRefs": Value::Array(Vec::new()),
                "artifactRefs": outcome.artifact_refs,
                "checkedAt": outcome.checked_at,
                "redactionStatus": RedactionStatus::Redacted.as_str(),
                "retentionExpiresAt": diagnostic_retention_expiry(outcome.checked_at),
            })
        })
        .collect();
    serde_json::json!({
        "smokeReportId": report_id,
        "tenantId": tc.tenant_id,
        "reportKind": "diagnostic",
        "requestedBy": tc.principal_id,
        "status": report_status,
        "domainSummary": domain_summary,
        "startedAt": started_at,
        "completedAt": started_at,
        "publishedAt": started_at,
        "artifactRefs": artifact_refs,
        "retentionExpiresAt": diagnostic_retention_expiry(started_at),
        "probeOutcomes": probe_outcomes,
    })
}

/// Go strconv.FormatInt(value, 36) for the generated smoke report id.
fn base36(mut value: i64) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if value == 0 {
        return "0".to_string();
    }
    let mut digits = Vec::new();
    while value > 0 {
        digits.push(DIGITS[(value % 36) as usize]);
        value /= 36;
    }
    digits.reverse();
    String::from_utf8(digits).expect("base36 digits are ascii")
}

/// Go decodeJSONBody: an empty body maps to "request body is required" (400);
/// malformed JSON maps to the decoder error (400).
fn decode_json_body<T: serde::de::DeserializeOwned>(body: &Bytes) -> Result<T, ApiError> {
    if body.is_empty() {
        return Err(ApiError::BadRequest("request body is required".to_string()));
    }
    serde_json::from_slice(body).map_err(|err| ApiError::BadRequest(err.to_string()))
}

/// 501 `{error, code}` payload (resources.rs precedent for registered-but-
/// unported surfaces).
fn not_implemented(
    code: &'static str,
    message: &'static str,
) -> (StatusCode, AxumJson<serde_json::Value>) {
    (
        StatusCode::NOT_IMPLEMENTED,
        AxumJson(serde_json::json!({ "error": message, "code": code })),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use axum::body::{to_bytes, Body};
    use axum::http::header::CONTENT_TYPE;
    use axum::http::Request;
    use parking_lot::Mutex;
    use tower::ServiceExt;
    use uuid::Uuid;

    fn test_config() -> kura_config::Config {
        kura_config::Config {
            project_root: String::new(),
            environment: kura_config::Environment::Test,
            bind_addr: "127.0.0.1:19192".to_string(),
            data_dir: "/tmp/kura-api-integrations".to_string(),
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

    fn test_state() -> AppState {
        let dir = std::env::temp_dir().join(format!("kura-api-integrations-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let store = Arc::new(Mutex::new(
            kura_store::SQLiteStore::new(dir.to_str().expect("path")).expect("store"),
        ));
        AppState::new(test_config(), Arc::new(kura_events::Bus::new()), store)
    }

    fn with_manager(mut state: AppState) -> AppState {
        state.integrations = Some(Arc::new(kura_integrations::Manager::new("test")));
        state
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

    fn tenant_request(
        method: &str,
        uri: &str,
        body: Option<&str>,
        tenant_id: &str,
        permissions: Vec<Permission>,
    ) -> Request<Body> {
        let mut req = request(method, uri, body);
        req.extensions_mut()
            .insert(TenantContext(kura_identity::TenantContext {
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

    fn create_fixture(state: &AppState, tenant_id: &str, id: &str, display: &str) {
        create_fixture_with(
            state,
            tenant_id,
            id,
            display,
            kura_integrations::BackendKind::FakeLocal,
            false,
            false,
        );
    }

    fn create_fixture_with(
        state: &AppState,
        tenant_id: &str,
        id: &str,
        display: &str,
        backend_kind: kura_integrations::BackendKind,
        supports_probe_read: bool,
        supports_probe_mutation: bool,
    ) {
        let manager = state.integrations.as_ref().expect("manager");
        manager
            .create(kura_integrations::CreateInput {
                tenant_id: tenant_id.to_string(),
                integration_id: id.to_string(),
                domain_kind: "calendar".to_string(),
                display_name: display.to_string(),
                account_binding: kura_integrations::AccountBinding {
                    account_key: format!("acct_{id}"),
                    ..kura_integrations::AccountBinding::default()
                },
                backend_binding: kura_integrations::BackendBinding {
                    backend_kind,
                    supports_probe_read,
                    supports_probe_mutation,
                    ..kura_integrations::BackendBinding::default()
                },
                ..kura_integrations::CreateInput::default()
            })
            .expect("create fixture");
    }

    /// Port of the Go list/create/readiness/default/disconnect flows.
    #[tokio::test]
    async fn integration_crud_flow() {
        let state = with_manager(test_state());
        let app = router().with_state(state.clone());

        // Create -> 201 with the persisted resource.
        let (status, json) = send(
            &app,
            request(
                "POST",
                "/v1/integrations",
                Some(r#"{"integrationId":"integration_main","domainKind":"calendar","displayName":"Main Calendar","backendKind":"fake_local","accountBinding":{"accountKey":"acct_main","knownAfterAuth":false}}"#),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(json["integrationId"], "integration_main");
        assert_eq!(json["environmentScope"], "test");
        assert_eq!(json["readinessStatus"], "not_configured");
        assert_eq!(json["backendBinding"]["backendKind"], "fake_local");
        assert_eq!(json["backendBinding"]["supportsProbeRead"], true);

        // List -> contains the created resource.
        let (status, json) = send(&app, request("GET", "/v1/integrations", None)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["items"].as_array().expect("items").len(), 1);
        assert_eq!(json["items"][0]["integrationId"], "integration_main");

        // Get by id -> 200.
        let (status, json) = send(
            &app,
            request("GET", "/v1/integrations/integration_main", None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["integrationId"], "integration_main");

        // Readiness -> 200 with the updated projection.
        let (status, json) = send(
            &app,
            request(
                "POST",
                "/v1/integrations/integration_main/readiness",
                Some(r#"{"readinessStatus":"healthy","authState":"authorized","healthState":"healthy","reason":"all good","secretResolution":"resolved"}"#),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["readinessStatus"], "healthy");
        assert_eq!(json["authState"], "authorized");
        assert_eq!(json["healthState"], "healthy");

        // Default -> 200 canonical default.
        let (status, json) = send(
            &app,
            request("POST", "/v1/integrations/integration_main/default", None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["canonicalDefault"], true);

        // Disconnect -> 200 unavailable + disabled reason.
        let (status, json) = send(
            &app,
            request(
                "DELETE",
                "/v1/integrations/integration_main?reason=rotate",
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["readinessStatus"], "unavailable");
        assert_eq!(json["authState"], "revoked");
        assert_eq!(json["disabledReason"], "rotate");

        // Get after disconnect -> 200 with the disconnected state.
        let (status, json) = send(
            &app,
            request("GET", "/v1/integrations/integration_main", None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["readinessStatus"], "unavailable");

        // Store round-trip: the persisted document is reloadable.
        let stored = state
            .store
            .lock()
            .list_integrations("test")
            .expect("list store");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].integration_id, "integration_main");
        assert_eq!(
            stored[0].readiness_status,
            kura_integrations::ReadinessStatus::Unavailable
        );
    }

    #[tokio::test]
    async fn create_validation_maps_to_400() {
        let state = with_manager(test_state());
        let app = router().with_state(state.clone());
        let (status, json) = send(
            &app,
            request(
                "POST",
                "/v1/integrations",
                Some(r#"{"domainKind":"calendar"}"#),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json["code"], "bad_request");
    }

    #[tokio::test]
    async fn missing_manager_returns_500() {
        let state = test_state();
        let app = router().with_state(state);
        let (status, json) = send(&app, request("GET", "/v1/integrations", None)).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(json["error"]
            .as_str()
            .unwrap_or("")
            .contains("integrations manager is not configured"));
    }

    /// Port of the permission + cross-tenant non-disclosure shape: a tenant
    /// without integrations.manage is denied 403 on the list; a tenant with
    /// the permission never sees another tenant's integration (404).
    #[tokio::test]
    async fn tenant_list_denial_and_cross_tenant_non_disclosure() {
        let state = with_manager(test_state());
        create_fixture(&state, "ten_a", "integration_a", "Tenant A Calendar");
        create_fixture(&state, "ten_b", "integration_b", "Tenant B Secret Calendar");
        let app = router().with_state(state.clone());

        // No permission -> 403 credential denial.
        let (status, json) = send(
            &app,
            tenant_request("GET", "/v1/integrations", None, "ten_a", vec![]),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(json["error"], "credential_access_denied");

        // With integrations.manage -> tenant-scoped list only.
        let (status, json) = send(
            &app,
            tenant_request(
                "GET",
                "/v1/integrations",
                None,
                "ten_a",
                vec![Permission::IntegrationsManage],
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let items = json["items"].as_array().expect("items");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["integrationId"], "integration_a");

        // Cross-tenant by-id get -> 404, without leaking the resource.
        let (status, body) = send(
            &app,
            tenant_request(
                "GET",
                "/v1/integrations/integration_b",
                None,
                "ten_a",
                vec![Permission::IntegrationsManage],
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let raw = body.to_string();
        assert!(!raw.contains("Tenant B Secret Calendar") && !raw.contains("integration_b"));
    }

    /// Port of TestIntegrationDiagnosticsAPIRequiresPermissionAndDoesNotDiscloseCrossTenantState.
    #[tokio::test]
    async fn diagnostics_denial_and_cross_tenant_non_disclosure() {
        let state = with_manager(test_state());
        create_fixture(&state, "ten_a", "integration_a", "Tenant A Calendar");
        create_fixture(&state, "ten_b", "integration_b", "Tenant B Secret Calendar");
        let app = router().with_state(state.clone());

        // Tenant context without diagnostics.read -> 403.
        let (status, json) = send(
            &app,
            tenant_request(
                "GET",
                "/v1/integrations/integration_a/diagnostics",
                None,
                "ten_a",
                vec![],
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(json["error"], "credential_access_denied");

        // With the permission, a cross-tenant integration -> 404 (never 403).
        let (status, body) = send(
            &app,
            tenant_request(
                "GET",
                "/v1/integrations/integration_b/diagnostics",
                None,
                "ten_a",
                vec![Permission::IntegrationDiagnosticsRead],
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let raw = body.to_string();
        assert!(!raw.contains("Tenant B Secret Calendar") && !raw.contains("integration_b"));
    }

    /// Port of TestIntegrationDiagnosticsAPIReadsStartsListsAndInspectsRuns:
    /// POST a run -> 201 completed with a persisted result; GET diagnostics ->
    /// 200 with the classified (redacted) state; GET runs list/detail -> 200.
    #[tokio::test]
    async fn diagnostics_reads_starts_lists_and_inspects_runs() {
        let state = with_manager(test_state());
        create_fixture_with(
            &state,
            "ten_diag",
            "integration_feishu",
            "Feishu Calendar",
            kura_integrations::BackendKind::FeishuLark,
            false,
            false,
        );
        let manager = state.integrations.as_ref().expect("manager");
        manager
            .update_readiness(
                "integration_feishu",
                kura_integrations::UpdateReadinessInput {
                    readiness_status: kura_integrations::ReadinessStatus::Degraded,
                    auth_state: kura_integrations::AuthState::Authorized.as_str().to_string(),
                    health_state: kura_integrations::HealthState::Degraded.as_str().to_string(),
                    reason: "scope missing for calendar.read with bearer secret-token".to_string(),
                    required_operator_action: "grant calendar scope".to_string(),
                    ..kura_integrations::UpdateReadinessInput::default()
                },
            )
            .expect("update readiness");
        let app = router().with_state(state.clone());
        let permissions = vec![
            Permission::IntegrationDiagnosticsRead,
            Permission::IntegrationDiagnosticsRun,
        ];

        // POST runs -> 201 completed with one persisted result.
        let (status, json) = send(
            &app,
            tenant_request(
                "POST",
                "/v1/integrations/integration_feishu/diagnostics/runs",
                Some(r#"{"clientKey":"client-key-1","capabilities":["calendar.read"]}"#),
                "ten_diag",
                permissions.clone(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "run body: {json}");
        assert_eq!(json["status"], "completed");
        assert_eq!(json["resultIds"].as_array().map(|v| v.len()), Some(1));
        let run_id = json["diagnosticRunId"].as_str().expect("diagnosticRunId").to_string();
        assert!(!run_id.is_empty());

        // GET diagnostics -> 200 with the classified, redacted state.
        let (status, body) = send(
            &app,
            tenant_request(
                "GET",
                "/v1/integrations/integration_feishu/diagnostics",
                None,
                "ten_diag",
                permissions.clone(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "diagnostics body: {body}");
        let raw = body.to_string();
        assert!(raw.contains("\"reasonCode\":\"scope_missing\""), "body: {raw}");
        assert!(
            raw.contains("\"remediationOwner\":\"tenant_admin\""),
            "body: {raw}"
        );
        assert!(
            !raw.to_lowercase().contains("secret-token") && !raw.to_lowercase().contains("bearer"),
            "diagnostics leaked credential material: {raw}"
        );

        // GET runs?integrationId= -> 200 with the run.
        let (status, body) = send(
            &app,
            tenant_request(
                "GET",
                "/v1/integration-diagnostics/runs?integrationId=integration_feishu",
                None,
                "ten_diag",
                permissions.clone(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "runs list body: {body}");
        assert!(body.to_string().contains(&run_id), "runs list body: {body}");

        // GET runs/{id} -> 200 with the run id.
        let (status, body) = send(
            &app,
            tenant_request(
                "GET",
                &format!("/v1/integration-diagnostics/runs/{run_id}"),
                None,
                "ten_diag",
                permissions,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "run detail body: {body}");
        assert_eq!(
            body["diagnosticRunId"],
            serde_json::Value::String(run_id)
        );
    }

    /// An empty diagnostics store auto-inspects and persists (Go
    /// handleIntegrationDiagnosticList forceRefresh/empty branch).
    #[tokio::test]
    async fn diagnostics_empty_store_auto_inspects_and_persists() {
        let state = with_manager(test_state());
        create_fixture(&state, "ten_a", "integration_a", "Tenant A Calendar");
        let app = router().with_state(state.clone());

        let (status, json) = send(
            &app,
            tenant_request(
                "GET",
                "/v1/integrations/integration_a/diagnostics",
                None,
                "ten_a",
                vec![Permission::IntegrationDiagnosticsRead],
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "diagnostics body: {json}");
        assert_eq!(json["items"].as_array().map(|v| v.len()), Some(1));
        assert_eq!(json["integrationId"], "integration_a");
        let stored = state
            .store
            .lock()
            .latest_integration_diagnostic_results(
                &kura_integrations::DiagnosticResultFilter {
                    tenant_id: "ten_a".to_string(),
                    integration_id: "integration_a".to_string(),
                    ..Default::default()
                },
                Utc::now(),
            )
            .expect("list results");
        assert_eq!(stored.len(), 1);
    }

    /// Port of TestIntegrationDiagnosticSmokeAPIPersistsPublishesAndAuditsReport
    /// (the Go tenant-audit assertion has no Rust DAO and is not ported): a
    /// passed smoke report is returned 201, the probe result row is persisted
    /// with the smoke report id, and the smoke-completed event is published.
    #[tokio::test]
    async fn smoke_persists_results_and_publishes_event() {
        let bus = Arc::new(kura_events::Bus::new());
        let dir = std::env::temp_dir().join(format!(
            "kura-api-integration-smoke-{}",
            Uuid::now_v7()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let store = Arc::new(Mutex::new(
            kura_store::SQLiteStore::new(dir.to_str().expect("path")).expect("store"),
        ));
        let mut state = AppState::new(test_config(), bus.clone(), store);
        state.integrations = Some(Arc::new(kura_integrations::Manager::new("test")));
        create_fixture_with(
            &state,
            "ten_smoke",
            "integration_smoke",
            "Smoke Calendar",
            kura_integrations::BackendKind::FakeLocal,
            true,
            false,
        );
        let app = router().with_state(state.clone());

        let (status, json) = send(
            &app,
            tenant_request(
                "POST",
                "/v1/integration-diagnostics/smoke",
                Some(r#"{"reportId":"smoke_api_1","integrationId":"integration_smoke","probes":[{"domainKind":"calendar","probeAction":"calendar.readiness.inspect","safeCredentialsAvailable":true,"tenantApprovalAvailable":true,"providerAvailable":true,"supported":true,"readOnlyOrReversible":true}]}"#),
                "ten_smoke",
                vec![Permission::IntegrationDiagnosticsSmoke],
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "smoke body: {json}");
        assert_eq!(json["smokeReportId"], "smoke_api_1");
        assert_eq!(json["status"], "completed");
        assert_eq!(json["probeOutcomes"].as_array().map(|v| v.len()), Some(1));
        assert_eq!(json["probeOutcomes"][0]["result"], "passed");

        // The probe result row is persisted with the smoke report id.
        let results = state
            .store
            .lock()
            .latest_integration_diagnostic_results(
                &kura_integrations::DiagnosticResultFilter {
                    tenant_id: "ten_smoke".to_string(),
                    integration_id: "integration_smoke".to_string(),
                    ..Default::default()
                },
                Utc::now(),
            )
            .expect("list results");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].smoke_report_id, "smoke_api_1");

        // The smoke-completed event is published for the report.
        let published = bus.list(&kura_events::Filter {
            category: "integration".to_string(),
            ..Default::default()
        });
        assert!(
            published.iter().any(|event| {
                event.name == kura_events::INTEGRATION_DIAGNOSTIC_SMOKE_COMPLETED_NAME
                    && event.resource.id == "smoke_api_1"
            }),
            "published: {published:?}"
        );
    }

    /// Port of TestIntegrationDiagnosticRetentionApplyPublishesEventAndAudit
    /// (minus the tenant-audit write): an expired retention record is flipped
    /// to Expired and a retention-applied event is published.
    #[tokio::test]
    async fn retention_apply_flips_expired_records_and_publishes_event() {
        let bus = Arc::new(kura_events::Bus::new());
        let dir = std::env::temp_dir().join(format!(
            "kura-api-integration-retention-{}",
            Uuid::now_v7()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let store = Arc::new(Mutex::new(
            kura_store::SQLiteStore::new(dir.to_str().expect("path")).expect("store"),
        ));
        let state = AppState::new(test_config(), bus.clone(), store);
        let created_at = Utc::now() - chrono::Duration::days(91);
        let record = kura_integrations::new_diagnostic_retention_record(
            "ten_retention",
            "diagnostic_run",
            "diag_run_expired",
            created_at,
        );
        state
            .store
            .lock()
            .save_diagnostic_retention_record(&record)
            .expect("save retention record");
        let app = router().with_state(state.clone());

        let (status, json) = send(
            &app,
            tenant_request(
                "POST",
                "/v1/integration-diagnostics/retention/apply",
                None,
                "ten_retention",
                vec![Permission::IntegrationDiagnosticsRun],
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "retention body: {json}");
        let raw = json.to_string();
        assert!(raw.contains("\"retentionState\":\"expired\""), "body: {raw}");

        let published = bus.list(&kura_events::Filter {
            category: "integration".to_string(),
            ..Default::default()
        });
        assert!(
            published.iter().any(|event| {
                event.name == kura_events::INTEGRATION_DIAGNOSTIC_RETENTION_APPLIED_NAME
                    && event.resource.id == record.retention_record_id
            }),
            "published: {published:?}"
        );
    }

    /// Smoke with a risky (mutating) probe requires the extra
    /// diagnostics.smoke.risky permission (Go smokeRequestContainsRiskyProbe).
    #[tokio::test]
    async fn smoke_risky_probe_requires_risky_permission() {
        let state = with_manager(test_state());
        create_fixture(&state, "ten_a", "integration_a", "Tenant A Calendar");
        let app = router().with_state(state.clone());
        let body = r#"{"reportId":"smoke_1","integrationId":"integration_a","probes":[{"probeAction":"calendar.write","safeCredentialsAvailable":true,"tenantApprovalAvailable":true,"providerAvailable":true,"supported":true,"readOnlyOrReversible":false}]}"#;
        let (status, json) = send(
            &app,
            tenant_request(
                "POST",
                "/v1/integration-diagnostics/smoke",
                Some(body),
                "ten_a",
                vec![Permission::IntegrationDiagnosticsSmoke],
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(json["error"], "credential_access_denied");

        let (status, json) = send(
            &app,
            tenant_request(
                "POST",
                "/v1/integration-diagnostics/smoke",
                Some(body),
                "ten_a",
                vec![
                    Permission::IntegrationDiagnosticsSmoke,
                    Permission::IntegrationDiagnosticsSmokeRisky,
                ],
            ),
        )
        .await;
        // With the risky permission the smoke runs: a mutating probe without
        // approvals blocks (missing_tenant_admin_approval) -> 201 blocked report.
        assert_eq!(status, StatusCode::CREATED, "risky smoke body: {json}");
        assert_eq!(json["smokeReportId"], "smoke_1");
        assert_eq!(json["status"], "blocked");
    }

    /// Port of Go handleIntegrationDiagnosticReasonCodes: the catalog returns
    /// non-empty items.
    #[tokio::test]
    async fn reason_codes_catalog() {
        let state = test_state();
        let app = router().with_state(state);
        let (status, json) = send(
            &app,
            request("GET", "/v1/integration-diagnostics/reason-codes", None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let items = json["items"].as_array().expect("items");
        assert!(!items.is_empty());
        assert!(items.iter().any(|item| item["reasonCode"] == "healthy"));
        assert!(items
            .iter()
            .any(|item| item["reasonCode"] == "scope_missing"));
    }

    /// Readiness on an unknown integration -> 404 (Go GetForTenant/Get).
    #[tokio::test]
    async fn readiness_unknown_integration_404() {
        let state = with_manager(test_state());
        let app = router().with_state(state.clone());
        let (status, _) = send(
            &app,
            request(
                "POST",
                "/v1/integrations/integration_missing/readiness",
                Some(r#"{"readinessStatus":"healthy"}"#),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// The placeholder surfaces answer 501 and never invent behavior.
    #[tokio::test]
    async fn placeholder_routes_answer_501() {
        let state = with_manager(test_state());
        let app = router().with_state(state.clone());
        let (status, json) = send(&app, request("POST", "/v1/integrations/sync", None)).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
        assert_eq!(json["code"], "integration_sync_not_implemented");

        let (status, json) = send(
            &app,
            request("POST", "/v1/integrations/integration_a/adapter-rpc", None),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
        assert_eq!(json["code"], "integration_adapter_rpc_not_implemented");
    }
}
