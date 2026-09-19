//! connectors route family (port of the /v1/connectors handlers in Go
//! daemon/internal/api/server.go, Roadmaps 2/8/11 supervision + ingress).
//!
//! Routes: GET/POST /v1/connectors, GET /v1/connectors/{connector_id}, POST
//! /v1/connectors/{connector_id}/{health|fail|restart}, GET
//! /v1/connectors/{connector_id}/{diagnostics|<platform>-setup|<platform>-smoke},
//! and POST /v1/connectors/{connector_id}/ingress/messages (the inbound
//! message → session → optional run pipeline).
//!
//! The hosted diagnostics/setup/smoke reads are tenant-context-gated (Go
//! semantics): without a resolved tenant context they answer the 403
//! `tenant_context_missing` credential denial; with one they require the
//! connectors-manage credential-read permission and the connector to belong
//! to the tenant, then read the per-platform hosted setup / smoke-evidence /
//! diagnostic ledgers.
//!
//! Deliberately not ported (documented divergence): the
//! `recordCredentialAudit` secret-use audit on accepted ingress (the audit
//! emitter is not wired into AppState; Go skips it when unset).

use std::collections::HashMap;

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use kura_connectors as connectors;
use kura_events as events;
use kura_imtypes as imtypes;
use kura_router as router_domain;
use kura_runtime as runtime;

use crate::error::ApiError;
use crate::middleware::{TenantContext, environment_scope_from_config};
use crate::state::AppState;

use super::decode_json_required;

/// Route family router.
#[must_use]
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/v1/connectors",
            get(list_connectors).post(register_connector),
        )
        .route("/v1/connectors/{connector_id}", get(get_connector))
        .route(
            "/v1/connectors/{connector_id}/{action}",
            post(connector_action),
        )
        .route(
            "/v1/connectors/{connector_id}/diagnostics",
            get(connector_diagnostics),
        )
        .route(
            "/v1/connectors/{connector_id}/discord-setup",
            get(discord_setup),
        )
        .route(
            "/v1/connectors/{connector_id}/discord-smoke",
            get(discord_smoke),
        )
        .route(
            "/v1/connectors/{connector_id}/telegram-setup",
            get(telegram_setup),
        )
        .route(
            "/v1/connectors/{connector_id}/telegram-smoke",
            get(telegram_smoke),
        )
        .route(
            "/v1/connectors/{connector_id}/slack-setup",
            get(slack_setup),
        )
        .route(
            "/v1/connectors/{connector_id}/slack-smoke",
            get(slack_smoke),
        )
        .route(
            "/v1/connectors/{connector_id}/matrix-setup",
            get(matrix_setup),
        )
        .route(
            "/v1/connectors/{connector_id}/matrix-smoke",
            get(matrix_smoke),
        )
        .route(
            "/v1/connectors/{connector_id}/ingress/messages",
            post(ingress_messages),
        )
}

// ---------------------------------------------------------------------------
// DTOs (Go SessionRouteRequest / ConnectorIngress*)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct ConnectorListResponse {
    items: Vec<connectors::Connector>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct SessionRouteRequest {
    kind: router_domain::SessionKind,
    channel: String,
    account_id: String,
    peer_id: String,
    thread_id: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct ConnectorIngressMessage {
    message_id: String,
    connector_account_id: String,
    #[serde(rename = "channelOrConversationId")]
    channel_or_conversation_id: String,
    provider_message_id: String,
    equivalent_rule_id: String,
    text: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct ConnectorIngressRunRequest {
    entrypoint: String,
    goal: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct ConnectorIngressMessageRequest {
    tenant_id: String,
    route: SessionRouteRequest,
    message: ConnectorIngressMessage,
    run: Option<ConnectorIngressRunRequest>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectorIngressMessageResponse {
    ingress_id: String,
    connector_id: String,
    outcome: &'static str,
    reason_code: String,
    redaction_status: &'static str,
    accepted_at: chrono::DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session: Option<router_domain::Session>,
    session_created: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    run: Option<runtime::Run>,
    run_created: bool,
}

/// Go writeCredentialDenial body.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CredentialDenial {
    error: &'static str,
    reason_code: &'static str,
}

fn credential_denial(reason_code: &'static str) -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(CredentialDenial {
            error: "credential_access_denied",
            reason_code,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn supervisor(state: &AppState) -> Result<&connectors::Supervisor, ApiError> {
    state
        .connectors
        .as_deref()
        .ok_or_else(|| ApiError::internal("connector supervisor is not configured"))
}

fn map_connectors_error(err: connectors::ConnectorsError) -> ApiError {
    match err {
        connectors::ConnectorsError::ConnectorNotFound => {
            ApiError::NotFound("not found".to_string())
        }
        connectors::ConnectorsError::ConnectorDisabled => ApiError::Conflict(err.to_string()),
        other => ApiError::BadRequest(other.to_string()),
    }
}

fn persist_connector(state: &AppState, connector: &connectors::Connector) -> Result<(), ApiError> {
    state
        .store
        .lock()
        .upsert_connector(connector)
        .map_err(ApiError::from_store)
}

fn publish_event(state: &AppState, mut event: events::Event) -> Result<events::Event, ApiError> {
    event.environment_scope = environment_scope_from_config(&state.config);
    let stored = state
        .store
        .lock()
        .append_event(&event)
        .map_err(ApiError::from_store)?;
    Ok(state.event_bus.publish(stored))
}

fn connector_event(
    name: &str,
    connector_id: &str,
    payload: serde_json::Map<String, serde_json::Value>,
) -> events::Event {
    events::Event {
        category: "connector".to_string(),
        name: name.to_string(),
        scope: events::Scope {
            connector_id: connector_id.to_string(),
            ..events::Scope::default()
        },
        resource: events::Resource {
            kind: "connector".to_string(),
            id: connector_id.to_string(),
        },
        payload,
        ..events::Event::default()
    }
}

fn json_value<T: Serialize>(value: &T) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or(serde_json::Value::Null)
}

/// Go projectConnectorResource: redacted secret summaries plus a default
/// account binding for tenant-owned connectors.
fn project_connector_resource(mut connector: connectors::Connector) -> connectors::Connector {
    connector.secret_summary = connector
        .secret_refs
        .iter()
        .map(|secret_ref| connectors::RedactedSecretSummary {
            secret_ref: secret_ref.clone(),
            ..connectors::RedactedSecretSummary::default()
        })
        .collect();
    if connector.account_binding.is_empty() && !connector.tenant_id.is_empty() {
        let mut binding = serde_json::Map::new();
        binding.insert(
            "tenantId".to_string(),
            serde_json::json!(connector.tenant_id),
        );
        binding.insert(
            "connectorId".to_string(),
            serde_json::json!(connector.connector_id),
        );
        binding.insert(
            "connectorAccountId".to_string(),
            serde_json::json!(connector.connector_id),
        );
        binding.insert("redactionStatus".to_string(), serde_json::json!("redacted"));
        binding.insert("updatedAt".to_string(), json_value(&connector.updated_at));
        connector.account_binding = binding;
    }
    connector
}

fn first_non_empty(values: &[&str]) -> String {
    values
        .iter()
        .map(|value| value.trim())
        .find(|value| !value.is_empty())
        .unwrap_or("")
        .to_string()
}

/// Go newIngressID: 8 random bytes, hex encoded.
fn new_ingress_id() -> String {
    let bytes = Uuid::new_v4().into_bytes();
    let hex: String = bytes[..8].iter().map(|b| format!("{b:02x}")).collect();
    format!("ingress_{hex}")
}

// ---------------------------------------------------------------------------
// Supervision handlers
// ---------------------------------------------------------------------------

/// GET /v1/connectors (Go handleConnectors GET branch, local path).
async fn list_connectors(
    State(state): State<AppState>,
) -> Result<Json<ConnectorListResponse>, ApiError> {
    let supervisor = supervisor(&state)?;
    Ok(Json(ConnectorListResponse {
        items: supervisor
            .list()
            .into_iter()
            .map(project_connector_resource)
            .collect(),
    }))
}

/// POST /v1/connectors (Go handleConnectors POST branch) — 201 on first
/// registration, 200 on re-registration.
async fn register_connector(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<(StatusCode, Json<connectors::Connector>), ApiError> {
    let input: connectors::RegisterInput = decode_json_required(&body)?;
    let supervisor = supervisor(&state)?;
    let (connector, created) = supervisor.register(input).map_err(map_connectors_error)?;
    persist_connector(&state, &connector)?;
    let mut payload = serde_json::Map::new();
    payload.insert(
        "tenantId".to_string(),
        serde_json::json!(connector.tenant_id),
    );
    payload.insert("kind".to_string(), serde_json::json!(connector.kind));
    payload.insert("status".to_string(), json_value(&connector.status));
    payload.insert("created".to_string(), serde_json::json!(created));
    payload.insert(
        "displayName".to_string(),
        serde_json::json!(connector.display_name),
    );
    payload.insert(
        "secretRefs".to_string(),
        serde_json::json!(connector.secret_refs),
    );
    publish_event(
        &state,
        connector_event("connector.registered", &connector.connector_id, payload),
    )?;
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(project_connector_resource(connector))))
}

/// GET /v1/connectors/{connector_id} (Go handleConnectorByID, local path).
async fn get_connector(
    State(state): State<AppState>,
    Path(connector_id): Path<String>,
) -> Result<Json<connectors::Connector>, ApiError> {
    let supervisor = supervisor(&state)?;
    supervisor
        .get(connector_id.trim())
        .map(|connector| Json(project_connector_resource(connector)))
        .ok_or_else(|| ApiError::NotFound("not found".to_string()))
}

/// POST /v1/connectors/{connector_id}/{health|fail|restart} (Go
/// handleConnectorHealth / Fail / Restart); unknown actions are 404 and a
/// disabled connector answers 409.
async fn connector_action(
    State(state): State<AppState>,
    Path((connector_id, action)): Path<(String, String)>,
    body: Bytes,
) -> Result<Json<connectors::Connector>, ApiError> {
    let supervisor = supervisor(&state)?;
    let connector_id = connector_id.trim();
    if supervisor.get(connector_id).is_none() {
        return Err(ApiError::NotFound("not found".to_string()));
    }
    let (connector, event_name, payload) = match action.as_str() {
        "health" => {
            let input: connectors::ReportHealthInput = decode_json_required(&body)?;
            let connector = supervisor
                .report_health(connector_id, input)
                .map_err(map_connectors_error)?;
            let mut payload = serde_json::Map::new();
            payload.insert(
                "tenantId".to_string(),
                serde_json::json!(connector.tenant_id),
            );
            payload.insert("status".to_string(), json_value(&connector.status));
            (connector, "connector.health_changed", payload)
        }
        "fail" => {
            let input: connectors::ReportFailureInput = decode_json_required(&body)?;
            let connector = supervisor
                .report_failure(connector_id, input)
                .map_err(map_connectors_error)?;
            let mut payload = serde_json::Map::new();
            payload.insert(
                "tenantId".to_string(),
                serde_json::json!(connector.tenant_id),
            );
            payload.insert("status".to_string(), json_value(&connector.status));
            payload.insert(
                "failureCount".to_string(),
                serde_json::json!(connector.failure_count),
            );
            payload.insert(
                "backoffSeconds".to_string(),
                serde_json::json!(connector.backoff_seconds),
            );
            payload.insert(
                "reason".to_string(),
                serde_json::json!(connector.last_failure_reason),
            );
            (connector, "connector.failure_reported", payload)
        }
        "restart" => {
            let connector = supervisor
                .restart(connector_id)
                .map_err(map_connectors_error)?;
            let mut payload = serde_json::Map::new();
            payload.insert(
                "tenantId".to_string(),
                serde_json::json!(connector.tenant_id),
            );
            payload.insert("status".to_string(), json_value(&connector.status));
            payload.insert(
                "restartCount".to_string(),
                serde_json::json!(connector.restart_count),
            );
            payload.insert(
                "disabledReason".to_string(),
                serde_json::json!(connector.disabled_reason),
            );
            (connector, "connector.restart_scheduled", payload)
        }
        _ => return Err(ApiError::NotFound("not found".to_string())),
    };
    persist_connector(&state, &connector)?;
    publish_event(
        &state,
        connector_event(event_name, &connector.connector_id, payload),
    )?;
    Ok(Json(connector))
}

// ---------------------------------------------------------------------------
// Hosted diagnostics / setup / smoke reads (tenant-context-gated in Go)
// ---------------------------------------------------------------------------

/// Shared preamble for the hosted tenant-gated reads: a resolved tenant
/// context with credential-read permission, and the connector must belong to
/// the tenant (Go handleConnectorDiagnostics / *Setup / *Smoke).
fn hosted_read_tenant(
    state: &AppState,
    tenant: &Option<Extension<TenantContext>>,
    connector_id: &str,
) -> Result<String, Response> {
    let Some(tc) = tenant.as_ref().map(|extension| &extension.0.0) else {
        return Err(credential_denial("tenant_context_missing"));
    };
    if tc.tenant_id.trim().is_empty() {
        return Err(credential_denial("tenant_context_missing"));
    }
    if !kura_identity::can_inspect_credentials(tc, &[kura_identity::Permission::ConnectorsManage]) {
        return Err(credential_denial("missing_permission"));
    }
    match supervisor(state) {
        Ok(supervisor) => {
            if supervisor
                .get_for_tenant(connector_id, &tc.tenant_id)
                .is_none()
            {
                return Err(ApiError::NotFound("not found".to_string()).into_response());
            }
        }
        Err(err) => return Err(err.into_response()),
    }
    Ok(tc.tenant_id.clone())
}

fn json_or_not_found<T: Serialize>(value: Result<Option<T>, String>) -> Response {
    match value {
        Ok(Some(item)) => Json(item).into_response(),
        Ok(None) => ApiError::NotFound("not found".to_string()).into_response(),
        Err(err) => ApiError::from_store(err).into_response(),
    }
}

/// GET /v1/connectors/{id}/diagnostics.
async fn connector_diagnostics(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(connector_id): Path<String>,
) -> Response {
    let tenant_id = match hosted_read_tenant(&state, &tenant, connector_id.trim()) {
        Ok(tenant_id) => tenant_id,
        Err(response) => return response,
    };
    match state.store_pool.read().list_connector_diagnostic_states(
        &tenant_id,
        connector_id.trim(),
        Utc::now(),
    ) {
        Ok(items) => Json(serde_json::json!({ "items": items })).into_response(),
        Err(err) => ApiError::from_store(err).into_response(),
    }
}

macro_rules! hosted_setup_read {
    ($name:ident, $dao:ident, $doc:literal) => {
        #[doc = $doc]
        async fn $name(
            State(state): State<AppState>,
            tenant: Option<Extension<TenantContext>>,
            Path(connector_id): Path<String>,
        ) -> Response {
            let tenant_id = match hosted_read_tenant(&state, &tenant, connector_id.trim()) {
                Ok(tenant_id) => tenant_id,
                Err(response) => return response,
            };
            json_or_not_found(state.store.lock().$dao(&tenant_id, connector_id.trim()))
        }
    };
}

macro_rules! hosted_smoke_read {
    ($name:ident, $dao:ident, $doc:literal) => {
        #[doc = $doc]
        async fn $name(
            State(state): State<AppState>,
            tenant: Option<Extension<TenantContext>>,
            Path(connector_id): Path<String>,
        ) -> Response {
            let tenant_id = match hosted_read_tenant(&state, &tenant, connector_id.trim()) {
                Ok(tenant_id) => tenant_id,
                Err(response) => return response,
            };
            json_or_not_found(
                state
                    .store
                    .lock()
                    .$dao(&tenant_id, connector_id.trim(), Utc::now()),
            )
        }
    };
}

hosted_setup_read!(
    discord_setup,
    get_discord_hosted_setup,
    "GET /v1/connectors/{id}/discord-setup."
);
hosted_smoke_read!(
    discord_smoke,
    latest_discord_smoke_evidence,
    "GET /v1/connectors/{id}/discord-smoke."
);
hosted_setup_read!(
    telegram_setup,
    get_telegram_hosted_setup,
    "GET /v1/connectors/{id}/telegram-setup."
);
hosted_smoke_read!(
    telegram_smoke,
    latest_telegram_smoke_evidence,
    "GET /v1/connectors/{id}/telegram-smoke."
);
hosted_setup_read!(
    slack_setup,
    get_slack_hosted_setup,
    "GET /v1/connectors/{id}/slack-setup."
);
hosted_smoke_read!(
    slack_smoke,
    latest_slack_smoke_evidence,
    "GET /v1/connectors/{id}/slack-smoke."
);
hosted_setup_read!(
    matrix_setup,
    get_matrix_hosted_setup,
    "GET /v1/connectors/{id}/matrix-setup."
);
hosted_smoke_read!(
    matrix_smoke,
    latest_matrix_smoke_evidence,
    "GET /v1/connectors/{id}/matrix-smoke."
);

// ---------------------------------------------------------------------------
// Ingress (Go handleConnectorIngressMessages)
// ---------------------------------------------------------------------------

fn routing_decision(
    tenant_id: &str,
    connector: &connectors::Connector,
    outcome: connectors::RouteDecisionOutcome,
    reason_code: &str,
    route_input: &router_domain::RouteInput,
    message_id: &str,
) -> connectors::RoutingDecision {
    let now = Utc::now();
    let mut safe_evidence = HashMap::new();
    safe_evidence.insert("messageId".to_string(), message_id.to_string());
    safe_evidence.insert("kind".to_string(), route_input.kind.as_str().to_string());
    safe_evidence.insert("channel".to_string(), route_input.channel.clone());
    safe_evidence.insert("peerId".to_string(), route_input.peer_id.clone());
    safe_evidence.insert("threadId".to_string(), route_input.thread_id.clone());
    connectors::RoutingDecision {
        tenant_id: first_non_empty(&[tenant_id, &connector.tenant_id]),
        connector_id: connector.connector_id.clone(),
        connector_kind: connector.kind.clone(),
        outcome,
        reason_code: reason_code.to_string(),
        occurred_at: now,
        safe_evidence,
        redaction_status: connectors::RedactionStatus::Redacted,
        retention_expires_at: now + Duration::days(90),
        ..connectors::RoutingDecision::default()
    }
}

fn persist_routing_decision(
    state: &AppState,
    decision: &connectors::RoutingDecision,
) -> Result<(), ApiError> {
    if decision.connector_id.is_empty() {
        return Ok(());
    }
    state
        .store
        .lock()
        .save_channel_routing_decision(decision)
        .map_err(ApiError::from_store)
}

/// Go resolveConnectorRouteInput.
fn resolve_route_input(
    connector: &connectors::Connector,
    request: &SessionRouteRequest,
) -> Result<router_domain::RouteInput, String> {
    if !request.channel.is_empty() && request.channel != connector.kind {
        return Err("route channel must match connector kind".to_string());
    }
    Ok(router_domain::RouteInput {
        kind: request.kind,
        channel: connector.kind.clone(),
        account_id: request.account_id.clone(),
        peer_id: request.peer_id.clone(),
        thread_id: request.thread_id.clone(),
    })
}

fn contains_route_value(allowed: &[String], values: &[&str]) -> bool {
    let allowed: Vec<&str> = allowed
        .iter()
        .map(|item| item.trim())
        .filter(|item| !item.is_empty())
        .collect();
    if allowed.is_empty() {
        return true;
    }
    values.iter().any(|value| allowed.contains(&value.trim()))
}

/// Go channelRoutePolicyAllows.
fn route_policy_allows(
    state: &AppState,
    tenant_id: &str,
    connector_id: &str,
    route_input: &router_domain::RouteInput,
    message: &ConnectorIngressMessage,
) -> Result<(bool, &'static str), ApiError> {
    let policy = state
        .store_pool
        .read()
        .get_channel_route_policy(tenant_id, connector_id)
        .map_err(ApiError::from_store)?;
    let Some(policy) = policy else {
        return Ok((true, ""));
    };
    if policy.validation_state.as_str() != "valid" {
        return Ok((false, "route_policy_invalid"));
    }
    if !contains_route_value(
        &policy.eligible_senders,
        &[&route_input.peer_id, &message.connector_account_id],
    ) {
        return Ok((false, "sender_not_allowed"));
    }
    if !contains_route_value(
        &policy.eligible_conversations,
        &[
            &message.channel_or_conversation_id,
            &route_input.thread_id,
            &route_input.peer_id,
        ],
    ) {
        return Ok((false, "conversation_not_allowed"));
    }
    if !contains_route_value(
        &policy.eligible_rooms,
        &[&route_input.thread_id, &message.channel_or_conversation_id],
    ) {
        return Ok((false, "room_not_allowed"));
    }
    if !contains_route_value(
        &policy.eligible_channels,
        &[
            &message.channel_or_conversation_id,
            &route_input.thread_id,
            &route_input.peer_id,
        ],
    ) {
        return Ok((false, "channel_not_allowed"));
    }
    Ok((true, ""))
}

/// Go publishSessionRouteEvents.
fn publish_session_route_events(
    state: &AppState,
    session: &router_domain::Session,
    created: bool,
    extra: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), ApiError> {
    let base_payload = |session: &router_domain::Session| {
        let mut payload = serde_json::Map::new();
        payload.insert("kind".to_string(), json_value(&session.kind));
        payload.insert("channel".to_string(), serde_json::json!(session.channel));
        payload.insert(
            "routingKey".to_string(),
            serde_json::json!(session.routing_key),
        );
        payload.insert(
            "generation".to_string(),
            serde_json::json!(session.generation),
        );
        for (key, value) in extra {
            payload.insert(key.clone(), value.clone());
        }
        payload
    };
    let session_event = |name: &str, payload| events::Event {
        category: "session".to_string(),
        name: name.to_string(),
        scope: events::Scope {
            session_id: session.session_id.clone(),
            ..events::Scope::default()
        },
        resource: events::Resource {
            kind: "session".to_string(),
            id: session.session_id.clone(),
        },
        payload,
        ..events::Event::default()
    };
    if created {
        publish_event(
            state,
            session_event("session.created", base_payload(session)),
        )?;
    }
    publish_event(
        state,
        session_event("session.routed", base_payload(session)),
    )?;
    Ok(())
}

fn blocked_response(
    state: &AppState,
    ingress_id: &str,
    connector: &connectors::Connector,
    reason_code: &str,
    accepted_at: chrono::DateTime<Utc>,
) -> (StatusCode, Json<ConnectorIngressMessageResponse>) {
    let _ = state;
    (
        StatusCode::ACCEPTED,
        Json(ConnectorIngressMessageResponse {
            ingress_id: ingress_id.to_string(),
            connector_id: connector.connector_id.clone(),
            outcome: "blocked",
            reason_code: reason_code.to_string(),
            redaction_status: "redacted",
            accepted_at,
            session: None,
            session_created: false,
            run: None,
            run_created: false,
        }),
    )
}

/// POST /v1/connectors/{connector_id}/ingress/messages — 202 with the
/// accepted / blocked / duplicate outcome (Go handleConnectorIngressMessages).
#[allow(clippy::too_many_lines)]
async fn ingress_messages(
    State(state): State<AppState>,
    Path(connector_id): Path<String>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let request: ConnectorIngressMessageRequest = decode_json_required(&body)?;
    if request.message.message_id.trim().is_empty() {
        return Err(ApiError::BadRequest("messageId is required".to_string()));
    }
    if let Some(run) = &request.run {
        if run.entrypoint.trim().is_empty() {
            return Err(ApiError::BadRequest(
                "run entrypoint is required".to_string(),
            ));
        }
    }
    let supervisor = supervisor(&state)?;
    let session_router = state
        .router
        .as_deref()
        .ok_or_else(|| ApiError::internal("session router is not configured"))?;

    let mut tenant_id = request.tenant_id.trim().to_string();
    let connector_id = connector_id.trim();
    let connector = match supervisor.require_inbound_ready(connector_id, &tenant_id) {
        Ok(connector) => connector,
        Err(connectors::ConnectorsError::ConnectorNotFound) => {
            return Err(ApiError::NotFound("not found".to_string()));
        }
        Err(connectors::ConnectorsError::ConnectorDisabled) => {
            if let Some(connector) = supervisor.get_for_tenant(connector_id, &tenant_id) {
                persist_routing_decision(
                    &state,
                    &routing_decision(
                        &tenant_id,
                        &connector,
                        connectors::RouteDecisionOutcome::Disabled,
                        "connector_disabled",
                        &router_domain::RouteInput::default(),
                        &request.message.message_id,
                    ),
                )?;
            }
            return Err(ApiError::Conflict("connector is disabled".to_string()));
        }
        Err(other) => return Err(ApiError::BadRequest(other.to_string())),
    };
    if connector.status == connectors::Status::Failed
        || connector.status == connectors::Status::BackingOff
    {
        persist_routing_decision(
            &state,
            &routing_decision(
                &first_non_empty(&[&tenant_id, &connector.tenant_id]),
                &connector,
                connectors::RouteDecisionOutcome::Failed,
                "connector_not_accepting_ingress",
                &router_domain::RouteInput::default(),
                &request.message.message_id,
            ),
        )?;
        return Err(ApiError::Conflict(
            "connector is not accepting ingress".to_string(),
        ));
    }

    let ingress_id = new_ingress_id();
    let accepted_at = Utc::now();
    tenant_id = first_non_empty(&[&request.tenant_id, &connector.tenant_id]);

    let route_input = match resolve_route_input(&connector, &request.route) {
        Ok(route_input) => route_input,
        Err(message) => {
            persist_routing_decision(
                &state,
                &routing_decision(
                    &tenant_id,
                    &connector,
                    connectors::RouteDecisionOutcome::Blocked,
                    "blocked_route",
                    &router_domain::RouteInput::default(),
                    &request.message.message_id,
                ),
            )?;
            let mut payload = serde_json::Map::new();
            payload.insert("tenantId".to_string(), serde_json::json!(tenant_id));
            payload.insert(
                "messageId".to_string(),
                serde_json::json!(request.message.message_id),
            );
            payload.insert("outcome".to_string(), serde_json::json!("blocked"));
            payload.insert("reasonCode".to_string(), serde_json::json!("blocked_route"));
            payload.insert("error".to_string(), serde_json::json!(message));
            payload.insert("redactionStatus".to_string(), serde_json::json!("redacted"));
            publish_event(
                &state,
                connector_event(
                    "connector.route_outcome_recorded",
                    &connector.connector_id,
                    payload,
                ),
            )?;
            return Ok(blocked_response(
                &state,
                &ingress_id,
                &connector,
                "blocked_route",
                accepted_at,
            )
            .into_response());
        }
    };

    let (allowed, reason_code) = route_policy_allows(
        &state,
        &tenant_id,
        &connector.connector_id,
        &route_input,
        &request.message,
    )?;
    if !allowed {
        persist_routing_decision(
            &state,
            &routing_decision(
                &tenant_id,
                &connector,
                connectors::RouteDecisionOutcome::Blocked,
                reason_code,
                &route_input,
                &request.message.message_id,
            ),
        )?;
        return Ok(
            blocked_response(&state, &ingress_id, &connector, reason_code, accepted_at)
                .into_response(),
        );
    }
    persist_routing_decision(
        &state,
        &routing_decision(
            &tenant_id,
            &connector,
            connectors::RouteDecisionOutcome::Accepted,
            "accepted",
            &route_input,
            &request.message.message_id,
        ),
    )?;

    let (session, created_session) = session_router
        .route(route_input.clone())
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    state
        .store
        .lock()
        .upsert_session(&session)
        .map_err(ApiError::from_store)?;
    let mut extra = serde_json::Map::new();
    extra.insert("source".to_string(), serde_json::json!("connector.ingress"));
    extra.insert(
        "connectorId".to_string(),
        serde_json::json!(connector.connector_id),
    );
    extra.insert(
        "messageId".to_string(),
        serde_json::json!(request.message.message_id),
    );
    publish_session_route_events(&state, &session, created_session, &extra)?;

    let now = Utc::now();
    let channel_or_conversation_id = first_non_empty(&[
        &request.message.channel_or_conversation_id,
        &route_input.thread_id,
        &route_input.peer_id,
    ]);
    let provider_message_id = first_non_empty(&[
        &request.message.provider_message_id,
        &request.message.message_id,
    ]);
    let (message_record, created_message) = state
        .store
        .lock()
        .create_connector_message_if_absent(&imtypes::MessageRecord {
            delivery_id: ingress_id.clone(),
            tenant_id: tenant_id.clone(),
            connector_id: connector.connector_id.clone(),
            direction: imtypes::DeliveryDirection::Inbound,
            external_message_id: request.message.message_id.clone(),
            connector_account_id: first_non_empty(&[
                &request.message.connector_account_id,
                &route_input.account_id,
            ]),
            channel_or_conversation_id: channel_or_conversation_id.clone(),
            provider_message_id: provider_message_id.clone(),
            equivalent_rule_id: first_non_empty(&[
                &request.message.equivalent_rule_id,
                "standard_provider_message_id",
            ]),
            session_id: session.session_id.clone(),
            channel_id: first_non_empty(&[
                &route_input.thread_id,
                &route_input.peer_id,
                &channel_or_conversation_id,
            ]),
            peer_id: route_input.peer_id.clone(),
            thread_id: route_input.thread_id.clone(),
            content: request.message.text.clone(),
            status: imtypes::DeliveryStatus::Received,
            created_at: now,
            updated_at: now,
            ..imtypes::MessageRecord::default()
        })
        .map_err(ApiError::from_store)?;
    if !created_message {
        let mut payload = serde_json::Map::new();
        payload.insert("tenantId".to_string(), serde_json::json!(tenant_id));
        payload.insert(
            "messageId".to_string(),
            serde_json::json!(request.message.message_id),
        );
        payload.insert(
            "providerMessageId".to_string(),
            serde_json::json!(provider_message_id),
        );
        payload.insert(
            "existingDeliveryId".to_string(),
            serde_json::json!(message_record.delivery_id),
        );
        payload.insert("outcome".to_string(), serde_json::json!("duplicate"));
        payload.insert(
            "reasonCode".to_string(),
            serde_json::json!("duplicate_inbound"),
        );
        payload.insert("redactionStatus".to_string(), serde_json::json!("redacted"));
        publish_event(
            &state,
            events::Event {
                category: "connector".to_string(),
                name: "connector.inbound_duplicate_detected".to_string(),
                scope: events::Scope {
                    session_id: message_record.session_id.clone(),
                    connector_id: connector.connector_id.clone(),
                    ..events::Scope::default()
                },
                resource: events::Resource {
                    kind: "connector_message".to_string(),
                    id: message_record.delivery_id.clone(),
                },
                payload,
                ..events::Event::default()
            },
        )?;
        return Ok((
            StatusCode::ACCEPTED,
            Json(ConnectorIngressMessageResponse {
                ingress_id,
                connector_id: connector.connector_id.clone(),
                outcome: "duplicate",
                reason_code: "duplicate_inbound".to_string(),
                redaction_status: "redacted",
                accepted_at,
                session: Some(session),
                session_created: false,
                run: None,
                run_created: false,
            }),
        )
            .into_response());
    }

    let mut run: Option<runtime::Run> = None;
    let mut run_created = false;
    if let Some(run_request) = &request.run {
        let runtime_manager = state
            .runtime
            .as_ref()
            .ok_or_else(|| ApiError::internal("runtime manager is not configured"))?;
        let created_run = runtime_manager
            .create_run(runtime::CreateRunInput {
                session_id: session.session_id.clone(),
                entrypoint: run_request.entrypoint.clone(),
                goal: run_request.goal.clone(),
                ..runtime::CreateRunInput::default()
            })
            .map_err(|err| ApiError::BadRequest(err.to_string()))?;
        state
            .store
            .lock()
            .upsert_run(&created_run)
            .map_err(ApiError::from_store)?;
        if let Some(checkpoints) = &state.checkpoints {
            checkpoints
                .save_run_checkpoint(&created_run.run_id)
                .map_err(ApiError::from_store)?;
        }
        let mut payload = serde_json::Map::new();
        payload.insert(
            "entrypoint".to_string(),
            serde_json::json!(created_run.entrypoint),
        );
        payload.insert("goal".to_string(), serde_json::json!(created_run.goal));
        payload.insert("status".to_string(), json_value(&created_run.status));
        payload.insert("source".to_string(), serde_json::json!("connector.ingress"));
        payload.insert(
            "messageId".to_string(),
            serde_json::json!(request.message.message_id),
        );
        publish_event(
            &state,
            events::Event {
                category: "run".to_string(),
                name: "run.created".to_string(),
                scope: events::Scope {
                    session_id: created_run.session_id.clone(),
                    run_id: created_run.run_id.clone(),
                    connector_id: connector.connector_id.clone(),
                    ..events::Scope::default()
                },
                resource: events::Resource {
                    kind: "run".to_string(),
                    id: created_run.run_id.clone(),
                },
                payload,
                ..events::Event::default()
            },
        )?;
        run = Some(created_run);
        run_created = true;
    }

    let mut payload = serde_json::Map::new();
    payload.insert("ingressId".to_string(), serde_json::json!(ingress_id));
    payload.insert("outcome".to_string(), serde_json::json!("accepted"));
    payload.insert("reasonCode".to_string(), serde_json::json!("accepted"));
    payload.insert("kind".to_string(), json_value(&session.kind));
    payload.insert("channel".to_string(), serde_json::json!(session.channel));
    payload.insert(
        "messageId".to_string(),
        serde_json::json!(request.message.message_id),
    );
    payload.insert(
        "connectorAccountId".to_string(),
        serde_json::json!(first_non_empty(&[
            &request.message.connector_account_id,
            &request.route.account_id,
        ])),
    );
    payload.insert(
        "channelOrConversationId".to_string(),
        serde_json::json!(first_non_empty(&[
            &request.message.channel_or_conversation_id,
            &request.route.thread_id,
            &request.route.peer_id,
        ])),
    );
    payload.insert(
        "providerMessageId".to_string(),
        serde_json::json!(provider_message_id),
    );
    payload.insert(
        "equivalentRuleId".to_string(),
        serde_json::json!(request.message.equivalent_rule_id),
    );
    payload.insert("redactionStatus".to_string(), serde_json::json!("redacted"));
    payload.insert(
        "sessionCreated".to_string(),
        serde_json::json!(created_session),
    );
    payload.insert("runCreated".to_string(), serde_json::json!(run_created));
    publish_event(
        &state,
        events::Event {
            category: "connector".to_string(),
            name: "connector.ingress_accepted".to_string(),
            scope: events::Scope {
                session_id: session.session_id.clone(),
                connector_id: connector.connector_id.clone(),
                run_id: run.as_ref().map(|r| r.run_id.clone()).unwrap_or_default(),
                ..events::Scope::default()
            },
            resource: events::Resource {
                kind: "connector".to_string(),
                id: connector.connector_id.clone(),
            },
            payload,
            ..events::Event::default()
        },
    )?;

    // Spec 058 phase 2 W1: capture the accepted inbound message as an L0
    // memory ref (fire-and-forget; never affects the ingress outcome). A
    // due turn-trigger runs consolidation off this path.
    let captured = super::memory::capture_l0(
        &state,
        &tenant_id,
        kura_memory::Actor {
            kind: kura_memory::ActorKind::System,
            id: format!("connector:{}", connector.connector_id),
        },
        "inbound_message",
        &request.message.text,
        vec![
            kura_memory::SourceLink {
                kind: kura_memory::SourceKind::Message,
                id: message_record.delivery_id.clone(),
                ..kura_memory::SourceLink::default()
            },
            kura_memory::SourceLink {
                kind: kura_memory::SourceKind::Thread,
                id: session.session_id.clone(),
                ..kura_memory::SourceLink::default()
            },
        ],
    );
    if captured.is_some_and(|(_, due)| due) {
        let state = state.clone();
        let trigger_tenant = tenant_id.clone();
        std::thread::spawn(move || {
            if let Err(err) =
                super::memory::execute_consolidation(&state, &trigger_tenant, "turns", None)
            {
                eprintln!("memory: ingress turn-trigger consolidation failed: {err:?}");
            }
        });
    }

    Ok((
        StatusCode::ACCEPTED,
        Json(ConnectorIngressMessageResponse {
            ingress_id,
            connector_id: connector.connector_id.clone(),
            outcome: "accepted",
            reason_code: "accepted".to_string(),
            redaction_status: "redacted",
            accepted_at,
            session: Some(session),
            session_created: created_session,
            run,
            run_created,
        }),
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::super::tests_support::{request_json, test_state};
    use axum::http::StatusCode;
    use std::sync::Arc;

    fn state_with_supervisor() -> crate::state::AppState {
        let mut state = test_state();
        state.connectors = Some(Arc::new(kura_connectors::Supervisor::new()));
        state.router = Some(Arc::new(kura_router::SessionRouter::new()));
        state
    }

    fn register_body() -> serde_json::Value {
        serde_json::json!({
            "connectorId": "discord-test",
            "kind": "discord",
            "displayName": "Discord Test"
        })
    }

    #[tokio::test]
    async fn register_list_get_and_fail_connector() {
        let state = state_with_supervisor();
        let (status, registered) = request_json(
            state.clone(),
            "POST",
            "/v1/connectors",
            Some(register_body()),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{registered}");

        let (status, listed) = request_json(state.clone(), "GET", "/v1/connectors", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(listed["items"].as_array().expect("items").len(), 1);

        let (status, _) =
            request_json(state.clone(), "GET", "/v1/connectors/discord-test", None).await;
        assert_eq!(status, StatusCode::OK);

        let (status, failed) = request_json(
            state.clone(),
            "POST",
            "/v1/connectors/discord-test/fail",
            Some(serde_json::json!({ "reason": "gateway drop" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{failed}");
        assert_eq!(failed["failureCount"], 1);

        // Tenant-gated hosted reads answer the Go credential denial without a
        // resolved tenant context.
        let (status, denial) = request_json(
            state,
            "GET",
            "/v1/connectors/discord-test/diagnostics",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{denial}");
        assert_eq!(denial["reasonCode"], "tenant_context_missing");
    }

    #[tokio::test]
    async fn ingress_accepts_a_message_and_creates_session_and_run() {
        let state = state_with_supervisor();
        let (status, _) = request_json(
            state.clone(),
            "POST",
            "/v1/connectors",
            Some(register_body()),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);

        let body = serde_json::json!({
            "route": { "kind": "direct", "peerId": "user_1" },
            "message": { "messageId": "msg_1", "text": "hello" }
        });
        let (status, accepted) = request_json(
            state.clone(),
            "POST",
            "/v1/connectors/discord-test/ingress/messages",
            Some(body.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
        assert_eq!(accepted["outcome"], "accepted");
        assert_eq!(accepted["sessionCreated"], true);

        // Same message id again: duplicate detection.
        let (status, duplicate) = request_json(
            state.clone(),
            "POST",
            "/v1/connectors/discord-test/ingress/messages",
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{duplicate}");
        assert_eq!(duplicate["outcome"], "duplicate");

        let (status, _) = request_json(
            state,
            "POST",
            "/v1/connectors/connector-missing/ingress/messages",
            Some(serde_json::json!({ "message": { "messageId": "msg_2" } })),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
