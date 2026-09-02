//! channel_management route family (port of daemon/internal/api/channel_management.go
//! + channel_management_auth.go, plus the `/v1/channel-management/connectors` and
//! `/v1/channel-management/connectors/` registrations from server.go).
//!
//! Routes under `/v1/channel-management/connectors`:
//! - `GET` — connector list page (tenant-scoped, filtered, cursor-paginated)
//! - `GET /{id}` — connector detail (diagnostic summary + route policy + outcomes)
//! - `GET /{id}/diagnostics` — diagnostic states (needs credentials.inspect AND
//!   integrations.diagnostics.read)
//! - `POST /{id}/disable` / `POST /{id}/re-enable` — enablement mutations
//! - `POST /{id}/repair-actions` — repair-action ledger (202; reconnect and
//!   credential-rotation additionally need secrets.manage)
//! - `GET|PUT /{id}/route-policy` — route policy read/update
//! - `GET /{id}/reply-outcomes` / `GET /{id}/delivery-outcomes` — outcome ledgers
//! - `GET /{id}/support-evidence` — redacted support-evidence bundle generation
//!
//! Wire behavior mirrors the Go handlers: `writeChannelManagementDenial` (403
//! `{error: credential_access_denied, reasonCode: permission_missing}`),
//! `writeError`-style 500s via ApiError, and the Go mutation-error mapping
//! (connector not found -> 404, connector disabled -> 409, otherwise 500).
//!
//! Persistence is on the kura-store channel_management DAOs:
//! - channel_connector_enablement_states (disable/re-enable)
//! - channel_repair_actions + list (repair, detail, support-evidence refs)
//! - channel_management_audit_records + list (permission denials, disable/
//!   re-enable/repair/route-policy audit writes, support-evidence audit refs)
//! Handlers keep the exact Go response shapes and fail closed: the supervisor
//! mutation runs after the audit + enablement rows persist, so a persistence
//! failure leaves the connector state untouched.

use std::collections::HashMap;

use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json as AxumJson, Router};
use chrono::{DateTime, Utc};
use kura_connectors::{
    CapabilitySupport, ChannelConnectorDetail, Connector, ConnectorDiagnosticState,
    ConnectorsError, EnablementMutationResult, EnablementState, FreshnessState,
    ManagementActionKind, ManagementState, ProjectionInput, RedactionStatus, RemediationOwner,
    RepairAction, Status, build_connector_page, build_connector_projection,
    build_support_evidence_bundle, capability_profile_for_kind, default_route_policy,
    freshness_at, latest_diagnostic, management_state_for_connector, normalize_route_policy,
    retry_safety_for_repair_action, terminal_state_for_repair_action,
};
use kura_events::{
    ConnectorManagementEventInput, connector_management_redaction_failed,
    connector_management_retention_applied, connector_management_support_evidence_generated,
};
use kura_identity::{Permission, has_permission};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::error::ApiError;
use crate::middleware::TenantContext;
use crate::state::AppState;

/// Go `credentialDenialStableError` / `permission_missing` reason code.
const CREDENTIAL_DENIAL_STABLE_ERROR: &str = "credential_access_denied";
const CREDENTIAL_DENIAL_REASON: &str = "permission_missing";

/// Route family router for the /v1/channel-management/connectors prefix.
#[must_use]
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/channel-management/connectors", get(connector_list))
        .route(
            "/v1/channel-management/connectors/{connector_id}",
            get(connector_detail),
        )
        .route(
            "/v1/channel-management/connectors/{connector_id}/diagnostics",
            get(connector_diagnostics),
        )
        .route(
            "/v1/channel-management/connectors/{connector_id}/disable",
            post(connector_disable),
        )
        .route(
            "/v1/channel-management/connectors/{connector_id}/re-enable",
            post(connector_re_enable),
        )
        .route(
            "/v1/channel-management/connectors/{connector_id}/repair-actions",
            post(connector_repair),
        )
        .route(
            "/v1/channel-management/connectors/{connector_id}/route-policy",
            get(route_policy_get).put(route_policy_put),
        )
        .route(
            "/v1/channel-management/connectors/{connector_id}/reply-outcomes",
            get(reply_outcomes),
        )
        .route(
            "/v1/channel-management/connectors/{connector_id}/delivery-outcomes",
            get(delivery_outcomes),
        )
        .route(
            "/v1/channel-management/connectors/{connector_id}/support-evidence",
            get(support_evidence),
        )
}

// ---------------------------------------------------------------------------
// Request DTOs (Go channelManagementActionRequest)
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChannelManagementActionRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    reason_code: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    #[allow(dead_code)]
    note: String,
    #[serde(default)]
    action_kind: Option<ManagementActionKind>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    source_diagnostic_state_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    eligible_senders: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    eligible_conversations: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    eligible_rooms: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    eligible_channels: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    invocation_gates: Vec<String>,
    #[serde(default)]
    background_delivery_eligible: Option<bool>,
}

/// Query params for the connector list page (Go r.URL.Query() reads).
#[derive(Debug, Default, Deserialize)]
struct ListQuery {
    #[serde(default)]
    limit: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    kind: Option<String>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /v1/channel-management/connectors — Go handleChannelManagementList.
async fn connector_list(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<(StatusCode, AxumJson<serde_json::Value>), ApiError> {
    let Some(tc) = require_channel_management_permission(
        &state,
        tenant.as_ref().map(|e| &e.0.0),
        "",
        Permission::CredentialsInspect,
        "channel_management.list",
    ) else {
        return Ok(channel_management_denial());
    };
    let supervisor = connectors_supervisor(&state)?;
    let items = supervisor.list_for_tenant(&tc.tenant_id);
    let now = Utc::now();
    let mut diagnostics_by_connector: HashMap<String, Vec<ConnectorDiagnosticState>> =
        HashMap::new();
    {
        let store = state.store.lock();
        for connector in &items {
            let diagnostics = store
                .list_connector_diagnostic_states(&tc.tenant_id, &connector.connector_id, now)
                .map_err(ApiError::from_store)?;
            diagnostics_by_connector.insert(connector.connector_id.clone(), diagnostics);
        }
    }
    let response = build_connector_page(&ProjectionInput {
        tenant_id: tc.tenant_id.clone(),
        connectors: items,
        diagnostics: diagnostics_by_connector,
        now,
        limit: parse_channel_management_limit(query.limit.as_deref().unwrap_or("")),
        cursor: query.cursor.clone().unwrap_or_default(),
        state_filter: query.state.clone().unwrap_or_default(),
        kind_filter: query.kind.clone().unwrap_or_default(),
    });
    Ok((
        StatusCode::OK,
        AxumJson(serde_json::to_value(response).map_err(ApiError::from)?),
    ))
}

/// GET /v1/channel-management/connectors/{id} — Go handleChannelManagementDetail.
async fn connector_detail(
    State(state): State<AppState>,
    Path(connector_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<(StatusCode, AxumJson<serde_json::Value>), ApiError> {
    let Some(tc) = require_channel_management_permission(
        &state,
        tenant.as_ref().map(|e| &e.0.0),
        &connector_id,
        Permission::CredentialsInspect,
        "channel_management.detail",
    ) else {
        return Ok(channel_management_denial());
    };
    let supervisor = connectors_supervisor(&state)?;
    let Some(connector) = supervisor.get_for_tenant(&connector_id, &tc.tenant_id) else {
        return Err(ApiError::NotFound("not found".to_string()));
    };
    let detail =
        build_channel_connector_detail(&state, &tc, connector).map_err(ApiError::internal)?;
    Ok((
        StatusCode::OK,
        AxumJson(serde_json::to_value(detail).map_err(ApiError::from)?),
    ))
}

/// GET /v1/channel-management/connectors/{id}/diagnostics — Go
/// handleChannelManagementDiagnostics.
async fn connector_diagnostics(
    State(state): State<AppState>,
    Path(connector_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<(StatusCode, AxumJson<serde_json::Value>), ApiError> {
    let Some(tc) = require_channel_management_permissions(
        &state,
        tenant.as_ref().map(|e| &e.0.0),
        &connector_id,
        "channel_management.diagnostics",
        &[Permission::CredentialsInspect, Permission::IntegrationDiagnosticsRead],
    ) else {
        return Ok(channel_management_denial());
    };
    let items = state
        .store
        .lock()
        .list_connector_diagnostic_states(&tc.tenant_id, &connector_id, Utc::now())
        .map_err(ApiError::from_store)?;
    Ok((StatusCode::OK, AxumJson(json!({ "items": items }))))
}

/// POST /v1/channel-management/connectors/{id}/disable — Go
/// handleChannelManagementDisable.
async fn connector_disable(
    State(state): State<AppState>,
    Path(connector_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
    body: String,
) -> Result<(StatusCode, AxumJson<serde_json::Value>), ApiError> {
    let Some(tc) = require_channel_management_permission(
        &state,
        tenant.as_ref().map(|e| &e.0.0),
        &connector_id,
        Permission::ConnectorsManage,
        "channel_management.disable",
    ) else {
        return Ok(channel_management_denial());
    };
    let input = decode_action_body(&body).unwrap_or_default();
    let supervisor = connectors_supervisor(&state)?;
    let reason = coalesce_reason(&input.reason_code, "tenant_disabled");
    let mut result: Option<EnablementMutationResult> = None;
    let mut persistence_error: Option<String> = None;
    let mutation = supervisor.with_connector_mutation(&connector_id, || {
        if supervisor.get_for_tenant(&connector_id, &tc.tenant_id).is_none() {
            return Err(ConnectorsError::ConnectorNotFound);
        }
        let now = Utc::now();
        let audit = match record_channel_management_audit(
            &state,
            &tc,
            &connector_id,
            "channel_management.disable",
            "connectors.manage",
            "succeeded",
            &reason,
        ) {
            Ok(record) => record,
            Err(err) => {
                persistence_error = Some(err);
                return Err(ConnectorsError::CoreInvariantFailed);
            }
        };
        // Go persists the disabled EnablementState via
        // SaveChannelConnectorEnablementState before calling Disable; the store
        // write happens under the same mutation lock and a failure leaves the
        // supervisor state untouched (fail closed).
        let enablement = EnablementState {
            tenant_id: tc.tenant_id.clone(),
            connector_id: connector_id.clone(),
            state: "disabled".to_string(),
            reason_code: reason.clone(),
            changed_by_principal_id: tc.principal_id.clone(),
            changed_at: now,
            audit_event_id: audit.audit_event_id.clone(),
            ..Default::default()
        };
        if let Err(err) = state.store.lock().save_channel_connector_enablement_state(&enablement) {
            persistence_error = Some(err);
            return Err(ConnectorsError::CoreInvariantFailed);
        }
        supervisor.disable(&connector_id, &reason)?;
        result = Some(EnablementMutationResult {
            connector_id: connector_id.clone(),
            enablement_state: ManagementState::Disabled,
            delivery_eligible: false,
            audit_event_id: audit.audit_event_id,
            changed_at: now,
        });
        Ok(())
    });
    if let Some(err) = persistence_error {
        return Err(ApiError::from_store(err));
    }
    if let Err(err) = mutation {
        return Err(channel_management_mutation_error(err));
    }
    let result = result.ok_or_else(|| ApiError::internal("mutation did not produce a result"))?;
    Ok((
        StatusCode::OK,
        AxumJson(serde_json::to_value(result).map_err(ApiError::from)?),
    ))
}

/// POST /v1/channel-management/connectors/{id}/re-enable — Go
/// handleChannelManagementReEnable.
async fn connector_re_enable(
    State(state): State<AppState>,
    Path(connector_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<(StatusCode, AxumJson<serde_json::Value>), ApiError> {
    let Some(tc) = require_channel_management_permission(
        &state,
        tenant.as_ref().map(|e| &e.0.0),
        &connector_id,
        Permission::ConnectorsManage,
        "channel_management.re_enable",
    ) else {
        return Ok(channel_management_denial());
    };
    let supervisor = connectors_supervisor(&state)?;
    let now = Utc::now();
    let diagnostics = state
        .store
        .lock()
        .list_connector_diagnostic_states(&tc.tenant_id, &connector_id, now)
        .map_err(ApiError::from_store)?;
    if let Some(latest) = diagnostics.first() {
        if freshness_at(latest.evidence_timestamp, now) == FreshnessState::Stale {
            return Err(ApiError::Conflict("diagnostic state is stale".to_string()));
        }
        let registered = Connector {
            status: Status::Registered,
            ..Default::default()
        };
        if management_state_for_connector(&registered, Some(latest)) != ManagementState::Ready {
            return Err(ApiError::Conflict("diagnostic state is not ready".to_string()));
        }
    }
    let (policy, found) =
        get_or_default_channel_route_policy(&state, &tc.tenant_id, &connector_id)
            .map_err(ApiError::from_store)?;
    if found && policy.validation_state != "valid" {
        return Err(ApiError::Conflict("route policy is not valid".to_string()));
    }
    let mut result: Option<EnablementMutationResult> = None;
    let mut persistence_error: Option<String> = None;
    let mutation = supervisor.with_connector_mutation(&connector_id, || {
        let connector = supervisor
            .get_for_tenant(&connector_id, &tc.tenant_id)
            .ok_or(ConnectorsError::ConnectorNotFound)?;
        let audit = match record_channel_management_audit(
            &state,
            &tc,
            &connector_id,
            "channel_management.re_enable",
            "connectors.manage",
            "succeeded",
            "validated_re_enable",
        ) {
            Ok(record) => record,
            Err(err) => {
                persistence_error = Some(err);
                return Err(ConnectorsError::CoreInvariantFailed);
            }
        };
        // Go persists the enabled EnablementState (with validated_at) here; the
        // store write happens under the same mutation lock (fail closed).
        let enablement = EnablementState {
            tenant_id: tc.tenant_id.clone(),
            connector_id: connector_id.clone(),
            state: "enabled".to_string(),
            reason_code: "validated_re_enable".to_string(),
            changed_by_principal_id: tc.principal_id.clone(),
            changed_at: now,
            validated_at: Some(now),
            audit_event_id: audit.audit_event_id.clone(),
        };
        if let Err(err) = state.store.lock().save_channel_connector_enablement_state(&enablement) {
            persistence_error = Some(err);
            return Err(ConnectorsError::CoreInvariantFailed);
        }
        supervisor.re_enable(&connector.connector_id)?;
        result = Some(EnablementMutationResult {
            connector_id: connector_id.clone(),
            enablement_state: ManagementState::Ready,
            delivery_eligible: true,
            audit_event_id: audit.audit_event_id,
            changed_at: now,
        });
        Ok(())
    });
    if let Some(err) = persistence_error {
        return Err(ApiError::from_store(err));
    }
    if let Err(err) = mutation {
        return Err(channel_management_mutation_error(err));
    }
    let result = result.ok_or_else(|| ApiError::internal("mutation did not produce a result"))?;
    Ok((
        StatusCode::OK,
        AxumJson(serde_json::to_value(result).map_err(ApiError::from)?),
    ))
}

/// POST /v1/channel-management/connectors/{id}/repair-actions — Go
/// handleChannelManagementRepair.
async fn connector_repair(
    State(state): State<AppState>,
    Path(connector_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
    body: String,
) -> Result<(StatusCode, AxumJson<serde_json::Value>), ApiError> {
    let input: ChannelManagementActionRequest = decode_json_body(&body)?;
    let action_kind = input.action_kind.unwrap_or(ManagementActionKind::Repair);
    let mut required = vec![Permission::ConnectorsManage];
    if action_kind == ManagementActionKind::Reconnect
        || action_kind == ManagementActionKind::CredentialRotation
    {
        required.push(Permission::SecretsManage);
    }
    let Some(tc) = require_channel_management_permissions(
        &state,
        tenant.as_ref().map(|e| &e.0.0),
        &connector_id,
        "channel_management.repair",
        &required,
    ) else {
        return Ok(channel_management_denial());
    };
    let supervisor = connectors_supervisor(&state)?;
    let Some(connector) = supervisor.get_for_tenant(&connector_id, &tc.tenant_id) else {
        return Err(ApiError::NotFound("not found".to_string()));
    };
    let permission_gate = required
        .iter()
        .map(permission_string)
        .collect::<Vec<_>>()
        .join("+");
    let audit = record_channel_management_audit(
        &state,
        &tc,
        &connector_id,
        &format!("channel_management.{action_kind}"),
        &permission_gate,
        "succeeded",
        "repair_started",
    )
    .map_err(ApiError::from_store)?;
    let now = Utc::now();
    let action = RepairAction {
        repair_action_id: new_repair_action_id(),
        tenant_id: tc.tenant_id.clone(),
        connector_id: connector_id.clone(),
        connector_kind: connector.kind.clone(),
        actor_principal_id: tc.principal_id.clone(),
        action_kind,
        source_diagnostic_state_id: input.source_diagnostic_state_id,
        status: terminal_state_for_repair_action(
            action_kind,
            connector.status == Status::Disabled,
        ),
        retry_safety: Some(retry_safety_for_repair_action(action_kind)),
        remediation_owner: Some(RemediationOwner::Admin),
        started_at: now,
        audit_event_id: audit.audit_event_id,
        redaction_status: RedactionStatus::Redacted,
        ..Default::default()
    };
    // Go SaveChannelRepairAction generates the id internally; the DAO only
    // generates when unset, so pre-generate the same new_store_id shape to keep
    // the response (and the support-evidence repair refs) id non-empty.
    state
        .store
        .lock()
        .save_channel_repair_action(&action)
        .map_err(ApiError::from_store)?;
    Ok((
        StatusCode::ACCEPTED,
        AxumJson(serde_json::to_value(action).map_err(ApiError::from)?),
    ))
}

/// GET /v1/channel-management/connectors/{id}/route-policy — Go
/// handleChannelManagementRoutePolicy GET branch.
async fn route_policy_get(
    State(state): State<AppState>,
    Path(connector_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<(StatusCode, AxumJson<serde_json::Value>), ApiError> {
    let Some(tc) = require_channel_management_permission(
        &state,
        tenant.as_ref().map(|e| &e.0.0),
        &connector_id,
        Permission::CredentialsInspect,
        "channel_management.route_policy.read",
    ) else {
        return Ok(channel_management_denial());
    };
    let (policy, _found) =
        get_or_default_channel_route_policy(&state, &tc.tenant_id, &connector_id)
            .map_err(ApiError::from_store)?;
    Ok((
        StatusCode::OK,
        AxumJson(serde_json::to_value(policy).map_err(ApiError::from)?),
    ))
}

/// PUT /v1/channel-management/connectors/{id}/route-policy — Go
/// handleChannelManagementRoutePolicy PUT branch.
async fn route_policy_put(
    State(state): State<AppState>,
    Path(connector_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
    body: String,
) -> Result<(StatusCode, AxumJson<serde_json::Value>), ApiError> {
    let Some(tc) = require_channel_management_permission(
        &state,
        tenant.as_ref().map(|e| &e.0.0),
        &connector_id,
        Permission::ConnectorsManage,
        "channel_management.route_policy.update",
    ) else {
        return Ok(channel_management_denial());
    };
    let input: ChannelManagementActionRequest = decode_json_body(&body)?;
    let supervisor = connectors_supervisor(&state)?;
    let Some(connector) = supervisor.get_for_tenant(&connector_id, &tc.tenant_id) else {
        return Err(ApiError::NotFound("connector not found".to_string()));
    };
    if capability_profile_for_kind(&connector.kind).get("route-edit")
        == Some(&CapabilitySupport::Unsupported)
    {
        return Err(ApiError::Conflict(format!(
            "route editing is unsupported for connector kind {}",
            connector.kind
        )));
    }
    let background_delivery = input.background_delivery_eligible.unwrap_or(true);
    let mut save_result: Option<Result<(), String>> = None;
    let mut persistence_error: Option<String> = None;
    let mut saved_policy: Option<kura_connectors::RoutePolicy> = None;
    let mutation = supervisor.with_connector_mutation(&connector_id, || {
        // Go records the audit row inside the mutation before saving the policy.
        let audit = match record_channel_management_audit(
            &state,
            &tc,
            &connector_id,
            "channel_management.route_policy.update",
            "connectors.manage",
            "succeeded",
            "route_policy_updated",
        ) {
            Ok(record) => record,
            Err(err) => {
                persistence_error = Some(err);
                return Err(ConnectorsError::CoreInvariantFailed);
            }
        };
        let policy = normalize_route_policy(
            kura_connectors::RoutePolicy {
                tenant_id: tc.tenant_id.clone(),
                connector_id: connector_id.clone(),
                eligible_senders: input.eligible_senders,
                eligible_conversations: input.eligible_conversations,
                eligible_rooms: input.eligible_rooms,
                eligible_channels: input.eligible_channels,
                invocation_gates: input.invocation_gates,
                background_delivery_eligible: background_delivery,
                validation_state: "valid".to_string(),
                validated_at: Utc::now(),
                audit_event_id: audit.audit_event_id,
                redaction_status: RedactionStatus::Redacted,
                ..Default::default()
            },
            Utc::now(),
        );
        save_result = Some(state.store.lock().save_channel_route_policy(&policy));
        saved_policy = Some(policy);
        Ok(())
    });
    if let Some(err) = persistence_error {
        return Err(ApiError::from_store(err));
    }
    if let Err(err) = mutation {
        return Err(ApiError::internal(err));
    }
    if let Some(Err(err)) = save_result {
        return Err(ApiError::from_store(err));
    }
    let stored = state
        .store
        .lock()
        .get_channel_route_policy(&tc.tenant_id, &connector_id)
        .map_err(ApiError::from_store)?
        .unwrap_or_else(|| saved_policy.unwrap_or_default());
    Ok((
        StatusCode::OK,
        AxumJson(serde_json::to_value(stored).map_err(ApiError::from)?),
    ))
}

/// GET /v1/channel-management/connectors/{id}/reply-outcomes — Go
/// handleChannelManagementReplyOutcomes.
async fn reply_outcomes(
    State(state): State<AppState>,
    Path(connector_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<(StatusCode, AxumJson<serde_json::Value>), ApiError> {
    let Some(tc) = require_channel_management_permission(
        &state,
        tenant.as_ref().map(|e| &e.0.0),
        &connector_id,
        Permission::CredentialsInspect,
        "channel_management.reply_outcomes",
    ) else {
        return Ok(channel_management_denial());
    };
    let items = state
        .store
        .lock()
        .list_channel_foreground_reply_outcomes(&tc.tenant_id, &connector_id, Utc::now())
        .map_err(ApiError::from_store)?;
    Ok((StatusCode::OK, AxumJson(json!({ "items": items }))))
}

/// GET /v1/channel-management/connectors/{id}/delivery-outcomes — Go
/// handleChannelManagementDeliveryOutcomes.
async fn delivery_outcomes(
    State(state): State<AppState>,
    Path(connector_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<(StatusCode, AxumJson<serde_json::Value>), ApiError> {
    let Some(tc) = require_channel_management_permission(
        &state,
        tenant.as_ref().map(|e| &e.0.0),
        &connector_id,
        Permission::CredentialsInspect,
        "channel_management.delivery_outcomes",
    ) else {
        return Ok(channel_management_denial());
    };
    let items = state
        .store
        .lock()
        .list_channel_background_delivery_outcomes(&tc.tenant_id, &connector_id, Utc::now())
        .map_err(ApiError::from_store)?;
    Ok((StatusCode::OK, AxumJson(json!({ "items": items }))))
}

/// GET /v1/channel-management/connectors/{id}/support-evidence — Go
/// handleChannelManagementSupportEvidence.
async fn support_evidence(
    State(state): State<AppState>,
    Path(connector_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<(StatusCode, AxumJson<serde_json::Value>), ApiError> {
    let Some(tc) = require_channel_management_permission(
        &state,
        tenant.as_ref().map(|e| &e.0.0),
        &connector_id,
        Permission::CredentialsInspect,
        "channel_management.support_evidence",
    ) else {
        return Ok(channel_management_denial());
    };
    let supervisor = connectors_supervisor(&state)?;
    let Some(connector) = supervisor.get_for_tenant(&connector_id, &tc.tenant_id) else {
        return Err(ApiError::NotFound("not found".to_string()));
    };
    let now = Utc::now();
    if let Some(existing) = state
        .store
        .lock()
        .get_latest_channel_support_evidence(&tc.tenant_id, &connector_id, now)
        .map_err(ApiError::from_store)?
    {
        return Ok((
            StatusCode::OK,
            AxumJson(serde_json::to_value(existing).map_err(ApiError::from)?),
        ));
    }
    let mut bundle = build_support_evidence_bundle(
        &ProjectionInput {
            tenant_id: tc.tenant_id.clone(),
            ..Default::default()
        },
        &connector,
        &tc.principal_id,
        now,
    );
    // The store DAO generates support_evidence_id internally and returns only
    // Result<(), String>, so pre-generate the id (same `new_store_id` shape the
    // store uses) to keep the response and the generated event id non-empty.
    if bundle.support_evidence_id.trim().is_empty() {
        bundle.support_evidence_id = new_support_evidence_id();
    }
    enrich_channel_support_evidence_bundle(&state, &tc.tenant_id, &connector_id, now, &mut bundle)
        .map_err(ApiError::internal)?;
    let _ = state
        .store
        .lock()
        .save_channel_support_evidence(&bundle)
        .map_err(ApiError::from_store)?;
    state.event_bus.publish(connector_management_support_evidence_generated(
        ConnectorManagementEventInput {
            tenant_id: tc.tenant_id.clone(),
            connector_id: connector_id.clone(),
            evidence_id: bundle.support_evidence_id.clone(),
            action: "channel_management.support_evidence.generated".to_string(),
            outcome: "succeeded".to_string(),
            reason_code: "support_evidence_generated".to_string(),
            redaction_status: bundle.redaction_status.as_str().to_string(),
            occurred_at: bundle.generated_at,
        },
    ));
    Ok((
        StatusCode::OK,
        AxumJson(serde_json::to_value(bundle).map_err(ApiError::from)?),
    ))
}

// ---------------------------------------------------------------------------
// Helpers (Go buildChannelConnectorDetail / enrichChannelSupportEvidenceBundle /
// recordChannelManagementAudit / error mapping / parsing)
// ---------------------------------------------------------------------------

/// Go buildChannelConnectorDetail: projection + diagnostic summary + route
/// policy + recent decisions/outcomes + repair actions.
fn build_channel_connector_detail(
    state: &AppState,
    tc: &kura_identity::TenantContext,
    connector: Connector,
) -> Result<ChannelConnectorDetail, String> {
    let now = Utc::now();
    let diagnostics = state
        .store
        .lock()
        .list_connector_diagnostic_states(&tc.tenant_id, &connector.connector_id, now)?;
    let projection =
        build_connector_projection(connector.clone(), latest_diagnostic(&diagnostics), now);
    let (route_policy, _found) =
        get_or_default_channel_route_policy(state, &tc.tenant_id, &connector.connector_id)?;
    let recent_route_decisions = state
        .store
        .lock()
        .list_channel_routing_decisions(&tc.tenant_id, &connector.connector_id, now)?;
    let foreground_reply_outcomes = state
        .store
        .lock()
        .list_channel_foreground_reply_outcomes(&tc.tenant_id, &connector.connector_id, now)?;
    let background_delivery = state
        .store
        .lock()
        .list_channel_background_delivery_outcomes(&tc.tenant_id, &connector.connector_id, now)?;
    let repair_actions = state
        .store
        .lock()
        .list_channel_repair_actions(&tc.tenant_id, &connector.connector_id)?;
    Ok(ChannelConnectorDetail {
        projection,
        diagnostic_summary: latest_diagnostic(&diagnostics).cloned(),
        route_policy: Some(route_policy),
        recent_route_decisions,
        foreground_reply_outcomes,
        background_delivery,
        repair_actions,
        support_evidence_available: true,
        retention: HashMap::from([("defaultDays".to_string(), "90".to_string())]),
        ..Default::default()
    })
}

/// Go getOrDefaultChannelRoutePolicy: the stored policy when present, otherwise
/// a fresh default policy.
fn get_or_default_channel_route_policy(
    state: &AppState,
    tenant_id: &str,
    connector_id: &str,
) -> Result<(kura_connectors::RoutePolicy, bool), String> {
    if let Some(policy) = state
        .store
        .lock()
        .get_channel_route_policy(tenant_id, connector_id)?
    {
        return Ok((policy, true));
    }
    Ok((
        default_route_policy(tenant_id, connector_id, Utc::now()),
        false,
    ))
}

/// Go enrichChannelSupportEvidenceBundle: aggregates diagnostic/repair/decision/
/// outcome/audit refs into the bundle and emits redaction + retention events.
fn enrich_channel_support_evidence_bundle(
    state: &AppState,
    tenant_id: &str,
    connector_id: &str,
    now: DateTime<Utc>,
    bundle: &mut kura_connectors::SupportEvidenceBundle,
) -> Result<(), String> {
    let store = state.store.lock();
    let diagnostics = store.list_connector_diagnostic_states(tenant_id, connector_id, now)?;
    for item in diagnostics {
        bundle.diagnostic_refs.push(item.diagnostic_state_id.clone());
        if item.redaction_status == RedactionStatus::Failed
            || item.redaction_status == RedactionStatus::Suppressed
        {
            bundle.redaction_status = RedactionStatus::Suppressed;
            bundle.redactions.push("diagnostic_evidence".to_string());
            state.event_bus.publish(connector_management_redaction_failed(
                ConnectorManagementEventInput {
                    tenant_id: tenant_id.to_string(),
                    connector_id: connector_id.to_string(),
                    evidence_id: item.diagnostic_state_id.clone(),
                    action: "channel_management.support_evidence.redaction".to_string(),
                    outcome: "suppressed".to_string(),
                    reason_code: item.reason_code.as_str().to_string(),
                    redaction_status: item.redaction_status.as_str().to_string(),
                    occurred_at: now,
                },
            ));
        }
    }
    let repairs = store.list_channel_repair_actions(tenant_id, connector_id)?;
    for item in repairs {
        bundle.repair_refs.push(item.repair_action_id);
    }
    let decisions = store.list_channel_routing_decisions(tenant_id, connector_id, now)?;
    for item in decisions {
        bundle.routing_decision_refs.push(item.routing_decision_id);
    }
    let replies = store.list_channel_foreground_reply_outcomes(tenant_id, connector_id, now)?;
    for item in replies {
        bundle.reply_outcome_refs.push(item.reply_outcome_id);
    }
    let deliveries = store
        .list_channel_background_delivery_outcomes(tenant_id, connector_id, now)?;
    for item in deliveries {
        bundle.delivery_outcome_refs.push(item.delivery_outcome_id);
    }
    let audits = store.list_channel_management_audit_records(tenant_id, connector_id)?;
    for item in audits {
        bundle.audit_refs.push(item.audit_event_id);
    }
    let expired = store.list_expired_channel_support_evidence(tenant_id, connector_id, now)?;
    for item in expired {
        state.event_bus.publish(connector_management_retention_applied(
            ConnectorManagementEventInput {
                tenant_id: tenant_id.to_string(),
                connector_id: connector_id.to_string(),
                evidence_id: item.support_evidence_id.clone(),
                action: "channel_management.support_evidence.retention".to_string(),
                outcome: "expired".to_string(),
                reason_code: "retention_expired".to_string(),
                redaction_status: item.redaction_status.as_str().to_string(),
                occurred_at: now,
            },
        ));
    }
    Ok(())
}

/// Go requireChannelManagementPermission: tenant context + permission gate. On
/// denial Go records a `channel_management.<action>` audit row
/// (SaveChannelManagementAuditRecord) before returning the 403; the store write
/// is best-effort (Go ignores the error).
fn require_channel_management_permission(
    state: &AppState,
    tenant: Option<&kura_identity::TenantContext>,
    connector_id: &str,
    permission: Permission,
    action: &str,
) -> Option<kura_identity::TenantContext> {
    let tc = tenant?.clone();
    if tc.tenant_id.is_empty() {
        return None;
    }
    if has_permission(&tc.permissions, permission) {
        return Some(tc);
    }
    let record = kura_connectors::ConnectorAuditRecord {
        audit_event_id: new_audit_event_id(),
        tenant_id: tc.tenant_id.clone(),
        connector_id: connector_id.to_string(),
        principal_id: tc.principal_id.clone(),
        action: action.to_string(),
        permission_gate: permission_string(&permission),
        outcome: "denied".to_string(),
        reason_code: "permission_missing".to_string(),
        created_at: Utc::now(),
        redaction_status: RedactionStatus::Redacted,
    };
    let _ = state.store.lock().save_channel_management_audit_record(&record);
    None
}

/// Go requireChannelManagementPermissions: every listed permission must pass.
fn require_channel_management_permissions(
    state: &AppState,
    tenant: Option<&kura_identity::TenantContext>,
    connector_id: &str,
    action: &str,
    permissions: &[Permission],
) -> Option<kura_identity::TenantContext> {
    let mut tc: Option<kura_identity::TenantContext> = None;
    for permission in permissions {
        tc = Some(require_channel_management_permission(
            state,
            tenant,
            connector_id,
            *permission,
            action,
        )?);
    }
    tc
}

/// Go writeChannelManagementDenial: 403 `{error, reasonCode}` body.
fn channel_management_denial() -> (StatusCode, AxumJson<serde_json::Value>) {
    (
        StatusCode::FORBIDDEN,
        AxumJson(json!({
            "error": CREDENTIAL_DENIAL_STABLE_ERROR,
            "reasonCode": CREDENTIAL_DENIAL_REASON,
        })),
    )
}

/// Go recordChannelManagementAudit: persists the audit record (the id is
/// pre-generated in the store's `new_store_id` style) and returns it.
fn record_channel_management_audit(
    state: &AppState,
    tc: &kura_identity::TenantContext,
    connector_id: &str,
    action: &str,
    permission_gate: &str,
    outcome: &str,
    reason_code: &str,
) -> Result<kura_connectors::ConnectorAuditRecord, String> {
    let record = kura_connectors::ConnectorAuditRecord {
        audit_event_id: new_audit_event_id(),
        tenant_id: tc.tenant_id.clone(),
        connector_id: connector_id.to_string(),
        principal_id: tc.principal_id.clone(),
        action: action.to_string(),
        permission_gate: permission_gate.to_string(),
        outcome: outcome.to_string(),
        reason_code: reason_code.to_string(),
        created_at: Utc::now(),
        redaction_status: RedactionStatus::Redacted,
    };
    state.store.lock().save_channel_management_audit_record(&record)?;
    Ok(record)
}

/// Go `newStoreID("audit")`-style id for the local audit records.
fn new_audit_event_id() -> String {
    let hex = Uuid::new_v4().simple().to_string();
    format!("audit_{}", &hex[..16])
}

/// Go `newStoreID("channel_repair_action")`-style id (the DAO generates this
/// internally when unset; pre-generated so the response carries it).
fn new_repair_action_id() -> String {
    let hex = Uuid::new_v4().simple().to_string();
    format!("channel_repair_action_{}", &hex[..16])
}

/// Go `newStoreID("channel_support_evidence")`-style id (the store DAO would
/// generate this internally; see the support_evidence handler).
fn new_support_evidence_id() -> String {
    let hex = Uuid::new_v4().simple().to_string();
    format!("channel_support_evidence_{}", &hex[..16])
}

/// Go handleChannelManagementMutationError: not found -> 404, disabled -> 409,
/// anything else -> 500.
fn channel_management_mutation_error(err: ConnectorsError) -> ApiError {
    match err {
        ConnectorsError::ConnectorNotFound => ApiError::NotFound("not found".to_string()),
        ConnectorsError::ConnectorDisabled => ApiError::Conflict(err.to_string()),
        other => ApiError::internal(other),
    }
}

/// Go parseChannelManagementLimit: default 20, capped at 100.
fn parse_channel_management_limit(raw: &str) -> i64 {
    match raw.trim().parse::<i64>() {
        Ok(limit) if limit > 0 => limit.min(100),
        _ => 20,
    }
}

/// Go coalesceReason: trimmed value or the fallback.
fn coalesce_reason(value: &str, fallback: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    }
}

/// Go `string(permission)` — the stable wire literal for a permission.
fn permission_string(permission: &Permission) -> String {
    serde_json::to_string(permission)
        .map(|s| s.trim_matches('"').to_string())
        .unwrap_or_else(|_| format!("{permission:?}"))
}

/// Go `connectors.Supervisor` accessor: absent manager -> 500 (matching the Go
/// nil-supervisor writeError).
fn connectors_supervisor(state: &AppState) -> Result<&kura_connectors::Supervisor, ApiError> {
    state
        .connectors
        .as_deref()
        .ok_or_else(|| ApiError::Internal("connector supervisor is not configured".to_string()))
}

/// Go decodeJSONBody: empty body -> 400; malformed JSON -> 400.
fn decode_json_body<T: serde::de::DeserializeOwned>(body: &str) -> Result<T, ApiError> {
    if body.is_empty() {
        return Err(ApiError::BadRequest("request body is required".to_string()));
    }
    serde_json::from_str(body).map_err(|e| ApiError::BadRequest(e.to_string()))
}

/// Go disable`s optional decode: a missing/empty/invalid body is ignored.
fn decode_action_body(body: &str) -> Option<ChannelManagementActionRequest> {
    if body.is_empty() {
        return None;
    }
    serde_json::from_str(body).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use chrono::{Duration, Utc};
    use kura_connectors::{
        BackgroundDeliveryOutcome, ConnectorAuditRecord, DiagnosticInput, DiagnosticReasonCode,
        ForegroundReplyOutcome, LifecycleState, ManagementTerminalState, RegisterInput,
        RouteDecisionOutcome, RoutingDecision, SupportEvidenceBundle, classify_diagnostic,
    };
    use kura_store::SQLiteStore;
    use parking_lot::Mutex;
    use tower::ServiceExt;
    use uuid::Uuid;

    fn test_config() -> kura_config::Config {
        kura_config::Config {
            project_root: String::new(),
            environment: kura_config::Environment::Test,
            bind_addr: "127.0.0.1:19192".to_string(),
            data_dir: "/tmp/kura-api-channel-management-test".to_string(),
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
        let dir = std::env::temp_dir().join(format!(
            "kura-api-channel-management-{}",
            Uuid::now_v7()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let store = Arc::new(Mutex::new(
            SQLiteStore::new(dir.to_str().expect("path")).expect("store"),
        ));
        AppState::new(test_config(), Arc::new(kura_events::Bus::new()), store)
    }

    /// Request carrying a resolved tenant context extension (the protected()
    /// middleware installs this once auth is wired; tests inject it directly).
    fn channel_tenant_request(
        method: &str,
        uri: &str,
        body: Option<&str>,
        permissions: Vec<Permission>,
    ) -> Request<Body> {
        let builder = Request::builder().method(method).uri(uri);
        let req = match body {
            Some(payload) => builder
                .header("content-type", "application/json")
                .body(Body::from(payload.to_string()))
                .expect("request"),
            None => builder.body(Body::empty()).expect("request"),
        };
        let mut req = req;
        req.extensions_mut().insert(TenantContext(kura_identity::TenantContext {
            tenant_id: "ten_channels".to_string(),
            principal_id: "prn_channels".to_string(),
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

    fn register_connector(
        supervisor: &kura_connectors::Supervisor,
        tenant_id: &str,
        connector_id: &str,
        kind: &str,
        display_name: &str,
    ) {
        supervisor
            .register(RegisterInput {
                tenant_id: tenant_id.to_string(),
                connector_id: connector_id.to_string(),
                kind: kind.to_string(),
                display_name: display_name.to_string(),
                ..Default::default()
            })
            .expect("register");
    }

    // Port of TestChannelManagementRoutePolicyAndOutcomeHandlers.
    #[tokio::test]
    async fn route_policy_and_outcome_handlers() {
        let mut state = test_state();
        let supervisor = Arc::new(kura_connectors::Supervisor::new());
        state.connectors = Some(supervisor.clone());
        register_connector(&supervisor, "ten_channels", "matrix-main", "matrix", "Matrix Main");
        let app = router().with_state(state.clone());

        let req = channel_tenant_request(
            "PUT",
            "/v1/channel-management/connectors/matrix-main/route-policy",
            Some(r#"{"eligibleRooms":["room_redacted"],"backgroundDeliveryEligible":false}"#),
            vec![Permission::ConnectorsManage],
        );
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::OK, "route update body: {json}");
        assert_eq!(
            json["backgroundDeliveryEligible"],
            serde_json::Value::Bool(false),
            "route update body: {json}"
        );
        assert_eq!(json["eligibleRooms"], serde_json::json!(["room_redacted"]));
        let audit_event_id = json["auditEventId"].as_str().expect("auditEventId");
        assert!(!audit_event_id.is_empty(), "expected audit event id, got {json}");

        // Recent timestamps so the seeded decision/outcome rows are not expired
        // (retention is 90 days from occurred_at; the real clock is well past
        // any fixed 2026-05 fixture).
        let now = Utc::now();
        {
            let store = state.store.lock();
            store
                .save_channel_routing_decision(&RoutingDecision {
                    routing_decision_id: "decision_1".to_string(),
                    tenant_id: "ten_channels".to_string(),
                    connector_id: "matrix-main".to_string(),
                    connector_kind: "matrix".to_string(),
                    outcome: RouteDecisionOutcome::Blocked,
                    reason_code: "blocked_route".to_string(),
                    occurred_at: now,
                    retention_expires_at: now + Duration::days(90),
                    redaction_status: RedactionStatus::Redacted,
                    ..Default::default()
                })
                .expect("save decision");
            store
                .save_channel_foreground_reply_outcome(&ForegroundReplyOutcome {
                    reply_outcome_id: "reply_1".to_string(),
                    tenant_id: "ten_channels".to_string(),
                    connector_id: "matrix-main".to_string(),
                    routing_decision_id: "decision_1".to_string(),
                    status: "failed".to_string(),
                    reason_code: "provider_unavailable".to_string(),
                    occurred_at: now,
                    retention_expires_at: now + Duration::days(90),
                    redaction_status: RedactionStatus::Redacted,
                    ..Default::default()
                })
                .expect("save reply outcome");
            store
                .save_channel_background_delivery_outcome(&BackgroundDeliveryOutcome {
                    delivery_outcome_id: "delivery_1".to_string(),
                    tenant_id: "ten_channels".to_string(),
                    connector_id: "matrix-main".to_string(),
                    delivery_target_id: "target_redacted".to_string(),
                    status: "blocked".to_string(),
                    reason_code: "connector_disabled".to_string(),
                    occurred_at: now,
                    retention_expires_at: now + Duration::days(90),
                    redaction_status: RedactionStatus::Redacted,
                    ..Default::default()
                })
                .expect("save delivery outcome");
        }

        for path in ["reply-outcomes", "delivery-outcomes"] {
            let req = channel_tenant_request(
                "GET",
                &format!("/v1/channel-management/connectors/matrix-main/{path}"),
                None,
                vec![Permission::CredentialsInspect],
            );
            let (status, json) = send(&app, req).await;
            assert_eq!(status, StatusCode::OK, "{path} body: {json}");
            assert_eq!(json["items"].as_array().map(|v| v.len()), Some(1), "{path}");
        }
    }

    // Port of TestChannelManagementRoutePolicyRejectsUnsupportedConnectorKind.
    #[tokio::test]
    async fn route_policy_rejects_unsupported_connector_kind() {
        let mut state = test_state();
        let supervisor = Arc::new(kura_connectors::Supervisor::new());
        state.connectors = Some(supervisor.clone());
        register_connector(&supervisor, "ten_channels", "legacy-main", "legacy", "Legacy Main");
        let app = router().with_state(state.clone());

        let req = channel_tenant_request(
            "PUT",
            "/v1/channel-management/connectors/legacy-main/route-policy",
            Some(r#"{"eligibleRooms":["room_redacted"]}"#),
            vec![Permission::ConnectorsManage],
        );
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::CONFLICT, "route update body: {json}");
        assert!(
            json.to_string().contains("route editing is unsupported"),
            "expected unsupported capability error, body: {json}"
        );
    }

    // Port of TestChannelManagementListDetailDiagnosticsAreTenantScopedOrderedAndPermissioned.
    #[tokio::test]
    async fn list_detail_diagnostics_are_tenant_scoped_ordered_and_permissioned() {
        let mut state = test_state();
        let supervisor = Arc::new(kura_connectors::Supervisor::new());
        state.connectors = Some(supervisor.clone());
        register_connector(&supervisor, "ten_channels", "ready-main", "discord", "Ready Main");
        register_connector(&supervisor, "ten_channels", "broken-main", "slack", "Broken Main");
        register_connector(&supervisor, "ten_channels", "disabled-main", "telegram", "Disabled Main");
        register_connector(&supervisor, "ten_other", "other-main", "matrix", "Other Main");
        supervisor
            .disable("disabled-main", "tenant_disabled")
            .expect("disable");

        // Recent evidence timestamp so the 90-day retention the store derives
        // stays in the future relative to the real clock.
        let now = Utc::now() - Duration::minutes(5);
        let diagnostic = classify_diagnostic(DiagnosticInput {
            diagnostic_state_id: "diag_broken".to_string(),
            tenant_id: "ten_channels".to_string(),
            connector_id: "broken-main".to_string(),
            reason_code: Some(DiagnosticReasonCode::PermissionMissing),
            evidence_timestamp: Some(now),
            redaction_reliable: true,
            safe_evidence: HashMap::from([("workspace".to_string(), "workspace_redacted".to_string())]),
            ..Default::default()
        })
        .expect("classify");
        state
            .store
            .lock()
            .save_connector_diagnostic_state(&diagnostic)
            .expect("save diagnostic");
        let app = router().with_state(state.clone());

        // First page: limit 2 -> attention first (broken-main), then disabled.
        let req = channel_tenant_request(
            "GET",
            "/v1/channel-management/connectors?limit=2",
            None,
            vec![Permission::CredentialsInspect],
        );
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::OK, "list body: {json}");
        assert_eq!(json["tenantId"], "ten_channels");
        assert_eq!(json["items"].as_array().map(|v| v.len()), Some(2));
        assert_eq!(json["items"][0]["connectorId"], "broken-main");
        assert_eq!(json["items"][1]["connectorId"], "disabled-main");
        assert_ne!(json["page"]["nextCursor"], "");

        // Detail: diagnostic summary + default route policy.
        let req = channel_tenant_request(
            "GET",
            "/v1/channel-management/connectors/broken-main",
            None,
            vec![Permission::CredentialsInspect],
        );
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::OK, "detail body: {json}");
        assert_eq!(json["diagnosticSummary"]["diagnosticStateId"], "diag_broken");
        assert_eq!(
            json["routePolicy"]["backgroundDeliveryEligible"],
            serde_json::Value::Bool(true),
            "expected default route policy"
        );

        // Diagnostics needs credentials.inspect AND integrations.diagnostics.read.
        let req = channel_tenant_request(
            "GET",
            "/v1/channel-management/connectors/broken-main/diagnostics",
            None,
            vec![Permission::CredentialsInspect],
        );
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "diagnostics denial body: {json}");
        assert_eq!(json["error"], "credential_access_denied");
        assert_eq!(json["reasonCode"], "permission_missing");
    }

    // Port of TestChannelManagementDisableReEnablePersistsAuditAndRejectsStaleDiagnostics.
    #[tokio::test]
    async fn disable_re_enable_rejects_stale_diagnostics() {
        let mut state = test_state();
        let supervisor = Arc::new(kura_connectors::Supervisor::new());
        state.connectors = Some(supervisor.clone());
        register_connector(&supervisor, "ten_channels", "discord-main", "discord", "Discord Main");
        let app = router().with_state(state.clone());

        let req = channel_tenant_request(
            "POST",
            "/v1/channel-management/connectors/discord-main/disable",
            Some(r#"{"reasonCode":"maintenance"}"#),
            vec![Permission::ConnectorsManage],
        );
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::OK, "disable body: {json}");
        assert_eq!(json["enablementState"], "disabled");
        assert_eq!(json["deliveryEligible"], serde_json::Value::Bool(false));
        let audit_event_id = json["auditEventId"].as_str().expect("auditEventId");
        assert!(!audit_event_id.is_empty(), "expected audit event id, got {json}");

        // The disabled enablement state and the audit row are persisted (Go
        // SaveChannelConnectorEnablementState + recordChannelManagementAudit).
        let persisted = state
            .store
            .lock()
            .get_channel_connector_enablement_state("ten_channels", "discord-main")
            .expect("get enablement");
        let persisted = persisted.expect("persisted enablement state");
        assert_eq!(persisted.state, "disabled");
        assert_eq!(persisted.reason_code, "maintenance");
        assert_eq!(persisted.audit_event_id, audit_event_id);
        let audits = state
            .store
            .lock()
            .list_channel_management_audit_records("ten_channels", "discord-main")
            .expect("list audits");
        assert!(
            audits.iter().any(|record| {
                record.action == "channel_management.disable" && record.audit_event_id == audit_event_id
            }),
            "expected disable audit, got: {audits:?}"
        );

        // A stale diagnostic blocks re-enable (409).
        let stale = classify_diagnostic(DiagnosticInput {
            diagnostic_state_id: "diag_stale".to_string(),
            tenant_id: "ten_channels".to_string(),
            connector_id: "discord-main".to_string(),
            reason_code: Some(DiagnosticReasonCode::NetworkFailed),
            evidence_timestamp: Some(Utc::now() - Duration::minutes(20)),
            redaction_reliable: true,
            ..Default::default()
        })
        .expect("classify");
        state
            .store
            .lock()
            .save_connector_diagnostic_state(&stale)
            .expect("save stale diagnostic");

        let req = channel_tenant_request(
            "POST",
            "/v1/channel-management/connectors/discord-main/re-enable",
            None,
            vec![Permission::ConnectorsManage],
        );
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::CONFLICT, "stale re-enable body: {json}");
        assert_eq!(json["error"], "diagnostic state is stale");
    }

    // Port of TestChannelManagementRepairRequiresSecretsForReconnectAndKeepsDisabledTerminal.
    #[tokio::test]
    async fn repair_requires_secrets_for_reconnect_and_keeps_disabled_terminal() {
        let mut state = test_state();
        let supervisor = Arc::new(kura_connectors::Supervisor::new());
        state.connectors = Some(supervisor.clone());
        register_connector(&supervisor, "ten_channels", "slack-main", "slack", "Slack Main");
        supervisor.disable("slack-main", "maintenance").expect("disable");
        let app = router().with_state(state.clone());

        // reconnect without secrets.manage -> 403.
        let req = channel_tenant_request(
            "POST",
            "/v1/channel-management/connectors/slack-main/repair-actions",
            Some(r#"{"actionKind":"reconnect"}"#),
            vec![Permission::ConnectorsManage],
        );
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "reconnect denial body: {json}");

        // reconnect with secrets.manage -> 202, terminal state disabled.
        let req = channel_tenant_request(
            "POST",
            "/v1/channel-management/connectors/slack-main/repair-actions",
            Some(r#"{"actionKind":"reconnect","sourceDiagnosticStateId":"diag_1"}"#),
            vec![Permission::ConnectorsManage, Permission::SecretsManage],
        );
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::ACCEPTED, "repair body: {json}");
        assert_eq!(json["status"], "disabled");
        assert_eq!(json["sourceDiagnosticStateId"], "diag_1");
        let repair_action_id = json["repairActionId"].as_str().expect("repairActionId");
        assert!(!repair_action_id.is_empty(), "expected repair action id, got {json}");

        // The repair action and its audit row are persisted (Go
        // SaveChannelRepairAction + recordChannelManagementAudit).
        let repairs = state
            .store
            .lock()
            .list_channel_repair_actions("ten_channels", "slack-main")
            .expect("list repairs");
        assert_eq!(repairs.len(), 1, "repairs: {repairs:?}");
        assert_eq!(repairs[0].repair_action_id, repair_action_id);
        assert_eq!(repairs[0].action_kind, ManagementActionKind::Reconnect);
        assert!(!repairs[0].audit_event_id.is_empty());
        let audits = state
            .store
            .lock()
            .list_channel_management_audit_records("ten_channels", "slack-main")
            .expect("list audits");
        assert!(
            audits.iter().any(|record| {
                record.action == "channel_management.reconnect" && record.reason_code == "repair_started"
            }),
            "expected repair audit, got: {audits:?}"
        );
    }

    // Port of TestChannelManagementSupportEvidenceIsPermissionedAndMetadataOnly.
    #[tokio::test]
    async fn support_evidence_is_permissioned_and_metadata_only() {
        let bus = Arc::new(kura_events::Bus::new());
        let dir = std::env::temp_dir().join(format!(
            "kura-api-channel-support-{}",
            Uuid::now_v7()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let store = Arc::new(Mutex::new(
            SQLiteStore::new(dir.to_str().expect("path")).expect("store"),
        ));
        let mut state = AppState::new(test_config(), bus.clone(), store);
        let supervisor = Arc::new(kura_connectors::Supervisor::new());
        state.connectors = Some(supervisor.clone());
        register_connector(&supervisor, "ten_channels", "telegram-main", "telegram", "Telegram Main");
        let app = router().with_state(state.clone());

        // No permissions -> 403.
        let req = channel_tenant_request(
            "GET",
            "/v1/channel-management/connectors/telegram-main/support-evidence",
            None,
            vec![],
        );
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "support denial body: {json}");

        let req = channel_tenant_request(
            "GET",
            "/v1/channel-management/connectors/telegram-main/support-evidence",
            None,
            vec![Permission::CredentialsInspect],
        );
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::OK, "support body: {json}");
        assert_eq!(json["redactionStatus"], "redacted");
        assert_ne!(json["supportEvidenceId"], serde_json::Value::String(String::new()));

        let lower = json.to_string().to_lowercase();
        for forbidden in ["access_token", "bearer ", "message body:", "raw payload:"] {
            assert!(!lower.contains(forbidden), "support evidence leaked {forbidden:?}: {json}");
        }

        let published = bus.list(&kura_events::Filter {
            category: "connector".to_string(),
            ..Default::default()
        });
        assert_eq!(published.len(), 1, "published: {published:?}");
        assert_eq!(
            published[0].name,
            kura_events::CONNECTOR_EVENT_SUPPORT_EVIDENCE_GENERATED
        );
    }

    // Port of TestChannelManagementSupportEvidenceAggregatesIncidentReferences:
    // routing-decision, repair, and audit refs all aggregate into the bundle.
    #[tokio::test]
    async fn support_evidence_aggregates_incident_references() {
        let mut state = test_state();
        let supervisor = Arc::new(kura_connectors::Supervisor::new());
        state.connectors = Some(supervisor.clone());
        register_connector(&supervisor, "ten_channels", "matrix-main", "matrix", "Matrix Main");
        let now = Utc::now();
        {
            let store = state.store.lock();
            store
                .save_channel_routing_decision(&RoutingDecision {
                    routing_decision_id: "route_1".to_string(),
                    tenant_id: "ten_channels".to_string(),
                    connector_id: "matrix-main".to_string(),
                    connector_kind: "matrix".to_string(),
                    outcome: RouteDecisionOutcome::Blocked,
                    reason_code: "blocked_route".to_string(),
                    occurred_at: now,
                    retention_expires_at: now + Duration::days(90),
                    redaction_status: RedactionStatus::Redacted,
                    ..Default::default()
                })
                .expect("save decision");
            store
                .save_channel_repair_action(&RepairAction {
                    repair_action_id: "repair_1".to_string(),
                    tenant_id: "ten_channels".to_string(),
                    connector_id: "matrix-main".to_string(),
                    connector_kind: "matrix".to_string(),
                    action_kind: ManagementActionKind::Repair,
                    status: ManagementTerminalState::ActionRequired,
                    started_at: now,
                    redaction_status: RedactionStatus::Redacted,
                    ..Default::default()
                })
                .expect("save repair");
            store
                .save_channel_management_audit_record(&ConnectorAuditRecord {
                    audit_event_id: "audit_1".to_string(),
                    tenant_id: "ten_channels".to_string(),
                    connector_id: "matrix-main".to_string(),
                    action: "channel_management.disable".to_string(),
                    permission_gate: "connectors.manage".to_string(),
                    outcome: "succeeded".to_string(),
                    created_at: now,
                    redaction_status: RedactionStatus::Redacted,
                    ..Default::default()
                })
                .expect("save audit");
        }
        let app = router().with_state(state.clone());

        let req = channel_tenant_request(
            "GET",
            "/v1/channel-management/connectors/matrix-main/support-evidence",
            None,
            vec![Permission::CredentialsInspect],
        );
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::OK, "support body: {json}");
        let refs = json["routingDecisionRefs"]
            .as_array()
            .expect("routingDecisionRefs array");
        assert!(
            refs.iter().any(|v| v == "route_1"),
            "expected route_1 in routingDecisionRefs: {json}"
        );
        let repair_refs = json["repairRefs"]
            .as_array()
            .expect("repairRefs array");
        assert!(
            repair_refs.iter().any(|v| v == "repair_1"),
            "expected repair_1 in repairRefs: {json}"
        );
        let audit_refs = json["auditRefs"]
            .as_array()
            .expect("auditRefs array");
        assert!(
            audit_refs.iter().any(|v| v == "audit_1"),
            "expected audit_1 in auditRefs: {json}"
        );
    }

    // Port of TestChannelManagementSupportEvidenceEmitsRedactionAndRetentionEvents.
    #[tokio::test]
    async fn support_evidence_emits_redaction_and_retention_events() {
        let bus = Arc::new(kura_events::Bus::new());
        let dir = std::env::temp_dir().join(format!(
            "kura-api-channel-support-events-{}",
            Uuid::now_v7()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let store = Arc::new(Mutex::new(
            SQLiteStore::new(dir.to_str().expect("path")).expect("store"),
        ));
        let mut state = AppState::new(test_config(), bus.clone(), store);
        let supervisor = Arc::new(kura_connectors::Supervisor::new());
        state.connectors = Some(supervisor.clone());
        register_connector(&supervisor, "ten_channels", "slack-main", "slack", "Slack Main");
        let now = Utc::now();
        {
            let store = state.store.lock();
            store
                .save_connector_diagnostic_state(&ConnectorDiagnosticState {
                    diagnostic_state_id: "diagnostic_redaction_failed".to_string(),
                    tenant_id: "ten_channels".to_string(),
                    connector_id: "slack-main".to_string(),
                    status: LifecycleState::Failed,
                    reason_code: DiagnosticReasonCode::ReplyFailed,
                    remediation_owner: kura_connectors::RemediationOwner::Admin,
                    retry_safety: kura_connectors::RetrySafety::Retryable,
                    evidence_timestamp: now,
                    freshness_state: kura_connectors::FreshnessState::Fresh,
                    redaction_status: RedactionStatus::Failed,
                    retention_expires_at: now + Duration::hours(24),
                    ..Default::default()
                })
                .expect("save diagnostic");
            store
                .save_channel_support_evidence(&SupportEvidenceBundle {
                    support_evidence_id: "support_expired_1".to_string(),
                    tenant_id: "ten_channels".to_string(),
                    connector_id: "slack-main".to_string(),
                    generated_at: now - Duration::hours(48),
                    current_state: ManagementState::Ready,
                    retention_expires_at: now - Duration::hours(24),
                    redaction_status: RedactionStatus::Redacted,
                    ..Default::default()
                })
                .expect("save expired evidence");
        }
        let app = router().with_state(state.clone());

        let req = channel_tenant_request(
            "GET",
            "/v1/channel-management/connectors/slack-main/support-evidence",
            None,
            vec![Permission::CredentialsInspect],
        );
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::OK, "support body: {json}");
        assert_eq!(json["redactionStatus"], "suppressed");
        let redactions = json["redactions"].as_array().expect("redactions array");
        assert!(
            redactions.iter().any(|v| v == "diagnostic_evidence"),
            "expected diagnostic_evidence redaction: {json}"
        );

        let published = bus.list(&kura_events::Filter {
            category: "connector".to_string(),
            ..Default::default()
        });
        let names: std::collections::HashSet<&str> =
            published.iter().map(|e| e.name.as_str()).collect();
        assert!(
            names.contains(kura_events::CONNECTOR_EVENT_MANAGEMENT_REDACTION_FAILED),
            "published: {published:?}"
        );
        assert!(
            names.contains(kura_events::CONNECTOR_EVENT_MANAGEMENT_RETENTION_APPLIED),
            "published: {published:?}"
        );
    }
}
