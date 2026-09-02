//! evaluation2 route family — port of daemon/internal/api/evaluation.go,
//! the store-backed half of evaluation_product.go, and live_validation.go.
//!
//! Routes under `/v1/evaluation/*`:
//! - `GET/POST /v1/evaluation/replay-candidates` — list / upsert a replay
//!   candidate (201); fixture-kind candidates and missing candidateIds are
//!   rejected with 400
//! - `GET /v1/evaluation/replay-candidates/{candidateId}` — one candidate (404
//!   when absent)
//! - `POST /v1/evaluation/replay-candidates/{candidateId}/attempts` — create a
//!   replay attempt (202); live-validation mode is rejected (400); billing
//!   reservation denials surface as the Go `writeBillingDenial` (429/503 with
//!   the stable denial payload)
//! - `POST /v1/evaluation/replay-candidates/{candidateId}/live-validations` —
//!   hand off a candidate to the live-validation manager (202; 409 blocked;
//!   503 disabled)
//! - `GET /v1/evaluation/replay-attempts`, `GET .../{attemptId}`,
//!   `POST .../{attemptId}/compare` (201)
//! - `GET /v1/evaluation/comparisons`, `GET .../{comparisonId}`
//! - `GET /v1/evaluation/fixtures`
//!
//! Routes under `/v1/live-validations/*`:
//! - `GET/POST /v1/live-validations` — list attempts / start one (202; 409
//!   when a gate blocks with the `StartResult` body; 503 when disabled)
//! - `GET .../{validationId}`, `GET .../{validationId}/ledger`,
//!   `POST .../{validationId}/abort`, `GET .../{validationId}/retention`,
//!   `POST .../{validationId}/compare` (202),
//!   `POST .../{validationId}/reconciliations/{ambiguousCommitId}/resolve`
//! - `GET /v1/live-validations/support-matrix`,
//!   `GET/POST /v1/live-validations/kill-switches`
//! - connector smoke/conformance evidence:
//!   `GET .../{discord|telegram|slack|matrix}-smoke`,
//!   `POST .../matrix-smoke` (non-safe-live records only),
//!   `GET .../{discord|telegram|slack|matrix}-conformance`
//!
//! The tenant-scoped evaluation product family (discovery policies/runs,
//! discovered candidates, product fixtures + revisions + review/suppress,
//! suppressions, replay campaigns, dashboard projections, tool-call
//! inspections, retention/apply) is fully ported on the kura-store
//! evaluation_product DAOs (`crates/persistence/store`) with the
//! kura_evaluation domain helpers (build_discovery_run_from_policy,
//! create_product_fixture_from_candidate, create_replay_campaign, ...). The
//! tenant is read from the resolved tenant context (400 when absent, matching
//! Go evaluationProductTenantIDFromRequest); capability-gated mutations check
//! the specific permission or the evaluation.manage wildcard. Note: Go answers
//! 501 for POST /v1/evaluation/retention/apply ("mutations are not enabled");
//! this wave implements the real handler on the store apply_retention DAO.
//!
//! NOT PORTED (manager/store method missing — reported, not duplicated):
//! - `POST /v1/live-validations/{telegram|slack}-smoke`: the Go api layer
//!   delegates the evidence build to the connectors packages; kura-api does not
//!   depend on kura-telegram/kura-slack, so the recorders are not ported.
//! - matrix safe-live smoke: Go runs a provider probe through a
//!   `matrixSmokeExecutor`; no Rust equivalent exists (non-safe-live records
//!   are built inline exactly like Go's `matrixSmokeRecordFromRequest`).
//! - `recordLiveValidationAudit`: kura-audit has no live-validation event
//!   builder yet (best-effort in Go; errors are ignored there).
//! - the store-persistence half of Go `publishEvent`: the Rust bus is
//!   in-memory; events are published to `state.event_bus` only.
//!
//! Middleware note: the Go registrations wrap these routes with
//! `protected()` only (no by-id tenant guard); the outer app assembly
//! applies the middleware. Handlers read the `TenantContext` extension when
//! present and behave like the Go nil-auth path otherwise.
//!
//! Tenant scoping: the replay-ledger routes are environment-scoped (the
//! manager fills the scope); the live-validation manager reads the resolved
//! tenant from the `kura_identity::tenantctx` task-local; the connector
//! smoke/conformance routes require a resolved tenant context plus
//! credential-inspection authority (403 credential denial otherwise).

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use axum::Json as AxumJson;
use chrono::{DateTime, Utc};
use kura_evaluation::{
    CandidateFilter, ComparisonFilter, ComparisonResult, CreateComparisonInput,
    CreateReplayAttemptInput, EvaluationError, FixtureFilter, ReplayAttempt, ReplayAttemptStatus,
    ReplayCandidate, ReplayMode, RegressionFixture,
};
use kura_identity::{has_permission, Permission};
use kura_livevalidation::{
    ApprovalMode, ApprovalStatus, ApprovalTarget, Attempt, AttemptFilter, AttemptStatus, Comparison,
    FreshApproval, KillSwitch, KillSwitchFilter, KillSwitchScope, LiveValidationError, MatrixRow,
    ReconciliationResolution, ReconciliationResolutionValue, RetentionPolicy, SafetyClass,
    SideEffectLedgerEntry, SideEffectScope, StartFailure, StartInput, StartResult, ToolClass,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::ApiError;
use crate::middleware::{TenantContext, environment_scope_from_config};
use crate::response::Json;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Handler error
// ---------------------------------------------------------------------------

/// Handler error carrying either the canonical ApiError mapping or the two Go
/// bodies that escape that mapping: the stable credential denial
/// (writeCredentialDenial, 403 {error, reasonCode}) and the billing denial
/// (writeBillingDenial, 429/503 with the DenialPayload).
#[derive(Debug)]
enum EvaluationApiError {
    Api(ApiError),
    ServiceUnavailable(String),
    CredentialDenial { reason_code: String },
    BillingDenial { status: StatusCode, body: serde_json::Value },
}

impl From<ApiError> for EvaluationApiError {
    fn from(err: ApiError) -> Self {
        Self::Api(err)
    }
}

impl IntoResponse for EvaluationApiError {
    fn into_response(self) -> Response {
        match self {
            Self::Api(err) => err.into_response(),
            Self::ServiceUnavailable(message) => (
                StatusCode::SERVICE_UNAVAILABLE,
                AxumJson(serde_json::json!({
                    "code": "internal",
                    "message": message,
                    "error": message,
                })),
            )
                .into_response(),
            Self::CredentialDenial { reason_code } => (
                StatusCode::FORBIDDEN,
                AxumJson(serde_json::json!({
                    "error": "credential_access_denied",
                    "reasonCode": reason_code,
                })),
            )
                .into_response(),
            Self::BillingDenial { status, body } => (status, AxumJson(body)).into_response(),
        }
    }
}

fn bad_request(message: impl Into<String>) -> EvaluationApiError {
    EvaluationApiError::Api(ApiError::BadRequest(message.into()))
}

// ---------------------------------------------------------------------------
// Response DTOs (port of the Go api-package response types)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReplayCandidateListResponse {
    environment_scope: String,
    items: Vec<ReplayCandidate>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReplayAttemptListResponse {
    environment_scope: String,
    items: Vec<ReplayAttempt>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReplayComparisonListResponse {
    environment_scope: String,
    items: Vec<ComparisonResult>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReplayFixtureListResponse {
    environment_scope: String,
    items: Vec<RegressionFixture>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LiveValidationAttemptListResponse {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    tenant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    environment_scope: String,
    items: Vec<Attempt>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LiveValidationSupportMatrixResponse {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    environment_scope: String,
    version: String,
    items: Vec<MatrixRow>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LiveValidationDiscordConformanceResponse {
    tenant_id: String,
    connector_id: String,
    items: Vec<kura_connectors::ConformanceResult>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LiveValidationLedgerResponse {
    validation_id: String,
    tenant_id: String,
    items: Vec<SideEffectLedgerEntry>,
}

/// Go slackSmokeEvidenceResource (setupwizard.go): the tenant-safe projection
/// of a Slack smoke evidence record.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SlackSmokeEvidenceResource {
    smoke_evidence_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    tenant_id: String,
    connector_id: String,
    workspace_binding_id: String,
    status: String,
    authorization_mode: String,
    owner: String,
    reason: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    remaining_risk: String,
    validated_at: DateTime<Utc>,
    retention_expires_at: DateTime<Utc>,
    redaction_status: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    safe_evidence: HashMap<String, String>,
}
// ---------------------------------------------------------------------------
// Request DTOs (ports of the Go api-package request types)
// ---------------------------------------------------------------------------

/// Go CreateLiveValidationRequest = livevalidation.StartInput.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateLiveValidationRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    validation_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    candidate_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    source_attempt_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    candidate_tool_classes: Vec<String>,
    #[serde(default)]
    requested_scope: Option<CreateLiveValidationScope>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    fresh_approvals: Vec<CreateLiveValidationApproval>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    client_key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    change_window_label: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateLiveValidationScope {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    scope_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    included_tool_classes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    excluded_tool_classes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    included_actions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    excluded_actions: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    approval_mode: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    declared_by: String,
    #[serde(default)]
    declared_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateLiveValidationApproval {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    approval_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    validation_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    tenant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    approval_target: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    tool_class: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    safety_class: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    action_ref: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    approved_scope: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    requested_by: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    resolved_by: String,
    #[serde(default)]
    requested_at: Option<DateTime<Utc>>,
    #[serde(default)]
    resolved_at: Option<DateTime<Utc>>,
}

impl CreateLiveValidationRequest {
    fn into_start_input(self) -> StartInput {
        let scope = self
            .requested_scope
            .map(|scope| SideEffectScope {
                scope_id: scope.scope_id,
                validation_id: self.validation_id.clone(),
                included_tool_classes: scope
                    .included_tool_classes
                    .into_iter()
                    .map(ToolClass::new)
                    .collect(),
                excluded_tool_classes: scope
                    .excluded_tool_classes
                    .into_iter()
                    .map(ToolClass::new)
                    .collect(),
                included_actions: scope.included_actions,
                excluded_actions: scope.excluded_actions,
                approval_mode: ApprovalMode::new(scope.approval_mode),
                declared_by: scope.declared_by,
                declared_at: scope.declared_at.unwrap_or_default(),
            })
            .unwrap_or_default();
        StartInput {
            validation_id: self.validation_id,
            candidate_id: self.candidate_id,
            source_attempt_id: self.source_attempt_id,
            candidate_tool_classes: self
                .candidate_tool_classes
                .into_iter()
                .map(ToolClass::new)
                .collect(),
            requested_scope: scope,
            fresh_approvals: self
                .fresh_approvals
                .into_iter()
                .map(|approval| FreshApproval {
                    approval_id: approval.approval_id,
                    validation_id: approval.validation_id,
                    tenant_id: approval.tenant_id,
                    approval_target: ApprovalTarget::new(approval.approval_target),
                    tool_class: ToolClass::new(approval.tool_class),
                    safety_class: SafetyClass::new(approval.safety_class),
                    action_ref: approval.action_ref,
                    approved_scope: approval.approved_scope,
                    status: ApprovalStatus::new(approval.status),
                    requested_by: approval.requested_by,
                    resolved_by: approval.resolved_by,
                    requested_at: approval.requested_at.unwrap_or_default(),
                    resolved_at: approval.resolved_at,
                })
                .collect(),
            client_key: self.client_key,
            change_window_label: self.change_window_label,
        }
    }
}

/// Go ResolveLiveValidationReconciliationRequest.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolveLiveValidationReconciliationRequest {
    resolution: String,
    reason: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    evidence_refs: Vec<String>,
}

/// Go UpdateLiveValidationKillSwitchRequest.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateLiveValidationKillSwitchRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    scope: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    tenant_id: String,
    enabled: bool,
    reason: String,
    #[serde(default)]
    expires_at: Option<DateTime<Utc>>,
}

/// Go RecordMatrixSmokeRequest.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecordMatrixSmokeRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    connector_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    homeserver_binding_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    authorization_mode: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    owner: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    reason: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    remaining_risk: String,
    #[serde(default)]
    validated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    safe_evidence: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// Manager accessors and shared helpers
// ---------------------------------------------------------------------------

/// Go manager == nil check for the evaluation manager (500).
fn evaluation_manager(state: &AppState) -> Result<Arc<kura_evaluation::Manager>, ApiError> {
    state.evaluation.clone().ok_or_else(|| {
        ApiError::Internal("evaluation manager is not configured".to_string())
    })
}

/// Go manager == nil check for the live-validation manager (500).
fn live_validation_manager(state: &AppState) -> Result<Arc<kura_livevalidation::Manager>, ApiError> {
    state.live_validation.clone().ok_or_else(|| {
        ApiError::Internal("live validation manager is not configured".to_string())
    })
}

/// Go queryInt: an absent or unparseable value is 0.
fn query_int(params: &HashMap<String, String>, name: &str) -> i64 {
    match params.get(name) {
        Some(raw) if !raw.trim().is_empty() => raw.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

/// Tolerant closed-enum parse for query filters. Go casts query strings onto
/// open string enums (unknown values filter nothing); the Rust closed enums
/// cannot represent unknown values, so an unparseable value degrades to the
/// default (no filter).
fn parse_enum<T: serde::de::DeserializeOwned + Default>(raw: &str) -> T {
    let raw = raw.trim();
    if raw.is_empty() {
        return T::default();
    }
    serde_json::from_value(serde_json::Value::String(raw.to_string())).unwrap_or_default()
}

/// Go firstNonEmptyString.
fn first_non_empty_string<'a>(primary: &'a str, fallback: &'a str) -> String {
    if primary.trim().is_empty() {
        fallback.to_string()
    } else {
        primary.trim().to_string()
    }
}

/// Go liveValidationToolClasses: trims, dedupes, and drops empties.
fn live_validation_tool_classes(items: &[String]) -> Vec<ToolClass> {
    let mut classes: Vec<ToolClass> = Vec::new();
    for item in items {
        let tool_class = ToolClass::new(item.trim());
        if tool_class.is_empty() || classes.contains(&tool_class) {
            continue;
        }
        classes.push(tool_class);
    }
    classes
}

/// Go requireHostedCredentialReadAny: a resolved tenant context plus
/// credential-inspection authority (or one of the manage permissions).
fn require_hosted_credential_read_any(
    tenant: Option<&TenantContext>,
    manage_permissions: &[kura_identity::Permission],
) -> Result<(), EvaluationApiError> {
    let Some(tc) = tenant else {
        return Err(EvaluationApiError::CredentialDenial {
            reason_code: "credential_denied:missing_tenant".to_string(),
        });
    };
    if tc.0.tenant_id.trim().is_empty() {
        return Err(EvaluationApiError::CredentialDenial {
            reason_code: "credential_denied:missing_tenant".to_string(),
        });
    }
    if !kura_identity::can_inspect_credentials(&tc.0, manage_permissions) {
        return Err(EvaluationApiError::CredentialDenial {
            reason_code: "credential_denied:missing_permission".to_string(),
        });
    }
    Ok(())
}

/// Go smoke-recorder permission gate: identity.HasPermission(...,
/// PermissionLiveValidationExecute).
fn require_live_validation_execute(
    tenant: Option<&TenantContext>,
) -> Result<(), EvaluationApiError> {
    let Some(tc) = tenant else {
        return Err(EvaluationApiError::CredentialDenial {
            reason_code: "credential_denied:missing_tenant".to_string(),
        });
    };
    if tc.0.tenant_id.trim().is_empty() {
        return Err(EvaluationApiError::CredentialDenial {
            reason_code: "credential_denied:missing_tenant".to_string(),
        });
    }
    if !kura_identity::has_permission(
        &tc.0.permissions,
        kura_identity::Permission::LiveValidationExecute,
    ) {
        return Err(EvaluationApiError::CredentialDenial {
            reason_code: "live_validation_execute_required".to_string(),
        });
    }
    Ok(())
}

/// Resolved tenant id for response/filter fields (Go tenantctx.FromContext).
/// Prefers the request's TenantContext extension (installed by protected())
/// and falls back to the kura_identity tenantctx task-local.
fn resolved_tenant_id(tenant: Option<&TenantContext>) -> String {
    if let Some(tc) = tenant {
        let id = tc.0.tenant_id.trim().to_string();
        if !id.is_empty() {
            return id;
        }
    }
    kura_identity::tenantctx::from_context()
        .map(|tc| tc.tenant_id)
        .unwrap_or_default()
}

/// Runs the future with the resolved tenant installed in the tenantctx
/// task-local (which the live-validation manager reads). Prefers an existing
/// task-local; falls back to the request's TenantContext extension.
async fn with_tenant_context<T, F>(tenant: Option<&TenantContext>, fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    if kura_identity::tenantctx::from_context().is_some() {
        fut.await
    } else if let Some(tc) = tenant {
        kura_identity::tenantctx::scope(tc.0.clone(), fut).await
    } else {
        fut.await
    }
}

/// Go writeBillingDenial: 503 unless the cause is a quota denial (429); the
/// stable DenialPayload body when present, otherwise writeError.
fn billing_reservation_error(
    reservation: kura_evaluation::BillingReservationError,
) -> EvaluationApiError {
    let status = if matches!(reservation.error, kura_billing::BillingError::QuotaDenied) {
        StatusCode::TOO_MANY_REQUESTS
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let body = match &reservation.result.denial {
        Some(denial) => serde_json::to_value(denial).unwrap_or(serde_json::Value::Null),
        None => serde_json::json!({ "error": reservation.error.to_string() }),
    };
    EvaluationApiError::BillingDenial { status, body }
}

// ---------------------------------------------------------------------------
// Event publishing (Go publishEvaluationReplayEvent /
// publishEvaluationComparisonEvent)
// ---------------------------------------------------------------------------

fn publish_evaluation_replay_event(state: &AppState, name: &str, attempt: &ReplayAttempt) {
    let mut payload = serde_json::Map::new();
    payload.insert("candidateId".to_string(), json!(attempt.candidate_id));
    payload.insert("attemptId".to_string(), json!(attempt.attempt_id));
    payload.insert("mode".to_string(), json!(attempt.mode.as_str()));
    payload.insert("status".to_string(), json!(attempt.status.as_str()));
    payload.insert("environmentScope".to_string(), json!(attempt.environment_scope));
    payload.insert("resultRunId".to_string(), json!(attempt.result_run_id));
    payload.insert("resultWorkflowId".to_string(), json!(attempt.result_workflow_id));
    payload.insert("blockedReasons".to_string(), json!(attempt.blocked_reasons));
    let event = kura_events::Event {
        category: "evaluation".to_string(),
        name: name.to_string(),
        scope: kura_events::Scope {
            run_id: attempt.result_run_id.clone(),
            workflow_id: attempt.result_workflow_id.clone(),
            ..kura_events::Scope::default()
        },
        resource: kura_events::Resource {
            kind: "replay_attempt".to_string(),
            id: attempt.attempt_id.clone(),
        },
        payload,
        ..kura_events::Event::default()
    };
    state.event_bus.publish(event);
}

fn publish_evaluation_comparison_event(state: &AppState, comparison: &ComparisonResult) {
    let planes: Vec<String> = comparison
        .drift_findings
        .iter()
        .map(|finding| finding.plane.as_str().to_string())
        .collect();
    let mut payload = serde_json::Map::new();
    payload.insert("candidateId".to_string(), json!(comparison.candidate_id));
    payload.insert("attemptId".to_string(), json!(comparison.attempt_id));
    payload.insert("comparisonId".to_string(), json!(comparison.comparison_id));
    payload.insert("terminalStatus".to_string(), json!(comparison.terminal_status.as_str()));
    payload.insert("environmentScope".to_string(), json!(comparison.environment_scope));
    payload.insert("driftPlanes".to_string(), json!(planes));
    let event = kura_events::Event {
        category: "evaluation".to_string(),
        name: "evaluation.comparison_completed".to_string(),
        resource: kura_events::Resource {
            kind: "replay_comparison".to_string(),
            id: comparison.comparison_id.clone(),
        },
        payload,
        ..kura_events::Event::default()
    };
    state.event_bus.publish(event);
}

/// Go publishLiveValidationStartEvent: event name follows the attempt status
/// (blocked / awaiting-approval / started).
fn publish_live_validation_start_event(state: &AppState, result: &StartResult) {
    let name = match result.attempt.status.as_str() {
        AttemptStatus::BLOCKED => kura_events::LIVE_VALIDATION_BLOCKED_NAME,
        AttemptStatus::AWAITING_APPROVAL => kura_events::LIVE_VALIDATION_AWAITING_APPROVAL_NAME,
        _ => kura_events::LIVE_VALIDATION_STARTED_NAME,
    };
    let event = kura_events::live_validation_attempt_event(
        name,
        result.attempt.clone(),
        &result.denials,
    );
    state.event_bus.publish(event);
}

// ---------------------------------------------------------------------------
// Live-validation start (shared by the collection and the candidate handoff)
// ---------------------------------------------------------------------------

async fn run_live_validation_start(
    state: &AppState,
    manager: &kura_livevalidation::Manager,
    input: StartInput,
) -> Result<(StatusCode, StartResult), EvaluationApiError> {
    match manager.start(input).await {
        Ok(result) => {
            publish_live_validation_start_event(state, &result);
            // Go recordLiveValidationAudit — no kura_audit live-validation
            // builder yet (best-effort in Go; errors ignored).
            Ok((StatusCode::ACCEPTED, result))
        }
        Err(StartFailure::Disabled) => Err(EvaluationApiError::ServiceUnavailable(
            "live validation is disabled".to_string(),
        )),
        Err(StartFailure::Blocked(result)) => {
            publish_live_validation_start_event(state, &result);
            // Go recordLiveValidationAudit — deferred (see above).
            Ok((StatusCode::CONFLICT, result))
        }
        Err(StartFailure::Internal(err)) => Err(bad_request(err.to_string())),
    }
}

// ---------------------------------------------------------------------------
// Evaluation: replay candidates
// ---------------------------------------------------------------------------

/// GET /v1/evaluation/replay-candidates — list (Go handleEvaluationReplayCandidates GET).
#[allow(clippy::unused_async)]
async fn list_replay_candidates(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<ReplayCandidateListResponse>, ApiError> {
    let manager = evaluation_manager(&state)?;
    let filter = CandidateFilter {
        candidate_kind: parse_enum(params.get("candidateKind").map(String::as_str).unwrap_or("")),
        source_kind: parse_enum(params.get("sourceKind").map(String::as_str).unwrap_or("")),
        readiness_status: parse_enum(
            params.get("readinessStatus").map(String::as_str).unwrap_or(""),
        ),
        limit: query_int(&params, "limit"),
        ..CandidateFilter::default()
    };
    let items = manager
        .list_replay_candidates(&filter)
        .map_err(ApiError::internal)?;
    Ok(Json(ReplayCandidateListResponse {
        environment_scope: environment_scope_from_config(&state.config),
        items,
    }))
}

/// POST /v1/evaluation/replay-candidates — upsert a curated replay candidate (201).
#[allow(clippy::unused_async)]
async fn create_replay_candidate(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<(StatusCode, AxumJson<ReplayCandidate>), ApiError> {
    let manager = evaluation_manager(&state)?;
    let input: ReplayCandidate = if body.is_empty() {
        ReplayCandidate::default()
    } else {
        // Go decodes the body straight into the resource and the manager fills
        // zero timestamps; the Rust serde shape requires them, so inject the
        // manager's effective "now" when the client omitted them.
        let mut value: serde_json::Value = serde_json::from_slice(&body)
            .map_err(|err| ApiError::BadRequest(err.to_string()))?;
        if value.get("createdAt").is_none() {
            value["createdAt"] = serde_json::json!(Utc::now());
        }
        if value.get("updatedAt").is_none() {
            value["updatedAt"] = serde_json::json!(Utc::now());
        }
        serde_json::from_value(value).map_err(|err| ApiError::BadRequest(err.to_string()))?
    };
    // Go: fixture replay candidates are managed by repo fixtures.
    if input.candidate_kind == kura_evaluation::CandidateKind::Fixture {
        return Err(ApiError::BadRequest(
            "fixture replay candidates are managed by repo fixtures".to_string(),
        ));
    }
    if input.candidate_id.trim().is_empty() {
        return Err(ApiError::BadRequest("candidateId is required".to_string()));
    }
    manager
        .upsert_replay_candidate(input.clone())
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    let created = manager
        .get_replay_candidate(&input.candidate_id)
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::internal("created replay candidate not found"))?;
    Ok((StatusCode::CREATED, AxumJson(created)))
}

/// GET /v1/evaluation/replay-candidates/{candidateId} — one candidate.
#[allow(clippy::unused_async)]
async fn get_replay_candidate(
    State(state): State<AppState>,
    Path(candidate_id): Path<String>,
) -> Result<Json<ReplayCandidate>, ApiError> {
    let manager = evaluation_manager(&state)?;
    let item = manager
        .get_replay_candidate(&candidate_id)
        .map_err(ApiError::internal)?;
    match item {
        Some(item) => Ok(Json(item)),
        None => Err(ApiError::NotFound("replay candidate not found".to_string())),
    }
}

/// POST /v1/evaluation/replay-candidates/{candidateId}/attempts — create a
/// replay attempt (202; Go handleEvaluationReplayCandidateRoutes attempts).
async fn replay_candidate_attempts(
    State(state): State<AppState>,
    Path(candidate_id): Path<String>,
    body: Bytes,
) -> Result<(StatusCode, AxumJson<ReplayAttempt>), EvaluationApiError> {
    let manager = evaluation_manager(&state)?;
    let input: CreateReplayAttemptInput = if body.is_empty() {
        CreateReplayAttemptInput::default()
    } else {
        serde_json::from_slice(&body)
            .map_err(|err| EvaluationApiError::Api(ApiError::BadRequest(err.to_string())))?
    };
    if input.mode == Some(ReplayMode::LiveValidation) {
        return Err(bad_request(
            "live validation attempts must use /v1/live-validations",
        ));
    }
    let attempt = match manager.create_replay_attempt(&candidate_id, input).await {
        Ok(attempt) => attempt,
        Err(EvaluationError::BillingReservation(reservation)) => {
            return Err(billing_reservation_error(reservation));
        }
        Err(err) => return Err(bad_request(err.to_string())),
    };
    publish_evaluation_replay_event(&state, "evaluation.replay_started", &attempt);
    match attempt.status {
        ReplayAttemptStatus::Completed => {
            publish_evaluation_replay_event(&state, "evaluation.replay_completed", &attempt);
        }
        ReplayAttemptStatus::Blocked => {
            publish_evaluation_replay_event(&state, "evaluation.replay_blocked", &attempt);
        }
        ReplayAttemptStatus::Unreplayable => {
            publish_evaluation_replay_event(&state, "evaluation.replay_unreplayable", &attempt);
        }
        ReplayAttemptStatus::Failed => {
            publish_evaluation_replay_event(&state, "evaluation.replay_failed", &attempt);
        }
        _ => {}
    }
    Ok((StatusCode::ACCEPTED, AxumJson(attempt)))
}

/// POST /v1/evaluation/replay-candidates/{candidateId}/live-validations — hand
/// off a replay candidate to the live-validation manager (202 / 409 / 503).
async fn replay_candidate_live_validations(
    State(state): State<AppState>,
    Path(candidate_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<(StatusCode, AxumJson<StartResult>), EvaluationApiError> {
    let evaluation = evaluation_manager(&state)?;
    let live_validation = live_validation_manager(&state)?;
    let candidate = evaluation
        .prepare_live_validation_handoff(&candidate_id)
        .map_err(|err| bad_request(err.to_string()))?;
    let mut input: CreateLiveValidationRequest = if body.is_empty() {
        CreateLiveValidationRequest::default()
    } else {
        serde_json::from_slice(&body)
            .map_err(|err| EvaluationApiError::Api(ApiError::BadRequest(err.to_string())))?
    };
    if !input.candidate_id.is_empty() && input.candidate_id != candidate_id {
        return Err(bad_request("candidateId must match the replay candidate route"));
    }
    input.candidate_id = candidate_id;
    if input.candidate_tool_classes.is_empty() {
        input.candidate_tool_classes = live_validation_tool_classes(&candidate.tool_classes)
            .into_iter()
            .map(|tool_class| tool_class.to_string())
            .collect();
    }
    let input = input.into_start_input();
    let (status, result) = with_tenant_context(
        tenant.as_ref().map(|t| &t.0),
        run_live_validation_start(&state, &live_validation, input),
    )
    .await?;
    Ok((status, AxumJson(result)))
}

// ---------------------------------------------------------------------------
// Evaluation: replay attempts / comparisons / fixtures
// ---------------------------------------------------------------------------

/// GET /v1/evaluation/replay-attempts — list.
#[allow(clippy::unused_async)]
async fn list_replay_attempts(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<ReplayAttemptListResponse>, ApiError> {
    let manager = evaluation_manager(&state)?;
    let filter = kura_evaluation::AttemptFilter {
        candidate_id: params.get("candidateId").cloned().unwrap_or_default(),
        status: parse_enum(params.get("status").map(String::as_str).unwrap_or("")),
        limit: query_int(&params, "limit"),
        ..kura_evaluation::AttemptFilter::default()
    };
    let items = manager
        .list_replay_attempts(&filter)
        .map_err(ApiError::internal)?;
    Ok(Json(ReplayAttemptListResponse {
        environment_scope: environment_scope_from_config(&state.config),
        items,
    }))
}

/// GET /v1/evaluation/replay-attempts/{attemptId} — one attempt.
#[allow(clippy::unused_async)]
async fn get_replay_attempt(
    State(state): State<AppState>,
    Path(attempt_id): Path<String>,
) -> Result<Json<ReplayAttempt>, ApiError> {
    let manager = evaluation_manager(&state)?;
    let item = manager
        .get_replay_attempt(&attempt_id)
        .map_err(ApiError::internal)?;
    match item {
        Some(item) => Ok(Json(item)),
        None => Err(ApiError::NotFound("replay attempt not found".to_string())),
    }
}

/// POST /v1/evaluation/replay-attempts/{attemptId}/compare — generate a
/// plane-level comparison (201).
#[allow(clippy::unused_async)]
async fn replay_attempt_compare(
    State(state): State<AppState>,
    Path(attempt_id): Path<String>,
    body: Bytes,
) -> Result<(StatusCode, AxumJson<ComparisonResult>), ApiError> {
    let manager = evaluation_manager(&state)?;
    let input: CreateComparisonInput = if body.is_empty() {
        CreateComparisonInput::default()
    } else {
        serde_json::from_slice(&body).map_err(|err| ApiError::BadRequest(err.to_string()))?
    };
    let comparison = manager
        .create_comparison(&attempt_id, input)
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    publish_evaluation_comparison_event(&state, &comparison);
    Ok((StatusCode::CREATED, AxumJson(comparison)))
}

/// GET /v1/evaluation/comparisons — list.
#[allow(clippy::unused_async)]
async fn list_comparisons(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<ReplayComparisonListResponse>, ApiError> {
    let manager = evaluation_manager(&state)?;
    let filter = ComparisonFilter {
        candidate_id: params.get("candidateId").cloned().unwrap_or_default(),
        attempt_id: params.get("attemptId").cloned().unwrap_or_default(),
        terminal_status: parse_enum(
            params.get("terminalStatus").map(String::as_str).unwrap_or(""),
        ),
        limit: query_int(&params, "limit"),
        ..ComparisonFilter::default()
    };
    let items = manager
        .list_comparisons(&filter)
        .map_err(ApiError::internal)?;
    Ok(Json(ReplayComparisonListResponse {
        environment_scope: environment_scope_from_config(&state.config),
        items,
    }))
}

/// GET /v1/evaluation/comparisons/{comparisonId} — one comparison.
#[allow(clippy::unused_async)]
async fn get_comparison(
    State(state): State<AppState>,
    Path(comparison_id): Path<String>,
) -> Result<Json<ComparisonResult>, ApiError> {
    let manager = evaluation_manager(&state)?;
    let item = manager
        .get_comparison(&comparison_id)
        .map_err(ApiError::internal)?;
    match item {
        Some(item) => Ok(Json(item)),
        None => Err(ApiError::NotFound("comparison not found".to_string())),
    }
}

/// GET /v1/evaluation/fixtures — list regression fixtures.
#[allow(clippy::unused_async)]
async fn list_fixtures(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<ReplayFixtureListResponse>, ApiError> {
    let manager = evaluation_manager(&state)?;
    let filter = FixtureFilter {
        domain_class: parse_enum(params.get("domainClass").map(String::as_str).unwrap_or("")),
        limit: query_int(&params, "limit"),
        ..FixtureFilter::default()
    };
    let items = manager.list_fixtures(&filter).map_err(ApiError::internal)?;
    Ok(Json(ReplayFixtureListResponse {
        environment_scope: environment_scope_from_config(&state.config),
        items,
    }))
}

// ---------------------------------------------------------------------------
// Live validations: collection + item routes
// ---------------------------------------------------------------------------

/// GET /v1/live-validations — list attempts (Go handleLiveValidationCollection GET).
async fn list_live_validations(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Json<LiveValidationAttemptListResponse>, ApiError> {
    let manager = live_validation_manager(&state)?;
    let tenant_id = resolved_tenant_id(tenant.as_ref().map(|t| &t.0));
    let filter = AttemptFilter {
        tenant_id: tenant_id.clone(),
        candidate_id: params.get("candidateId").cloned().unwrap_or_default(),
        status: AttemptStatus::new(params.get("status").cloned().unwrap_or_default()),
        limit: query_int(&params, "limit"),
        ..AttemptFilter::default()
    };
    let items = manager.list_attempts(filter).await.map_err(ApiError::internal)?;
    Ok(Json(LiveValidationAttemptListResponse {
        tenant_id,
        environment_scope: manager.environment_scope().to_string(),
        items,
    }))
}

/// POST /v1/live-validations — start an attempt (202 / 409 / 503).
async fn start_live_validation(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<(StatusCode, AxumJson<StartResult>), EvaluationApiError> {
    let manager = live_validation_manager(&state)?;
    let input: CreateLiveValidationRequest = if body.is_empty() {
        CreateLiveValidationRequest::default()
    } else {
        serde_json::from_slice(&body)
            .map_err(|err| EvaluationApiError::Api(ApiError::BadRequest(err.to_string())))?
    };
    let (status, result) = with_tenant_context(
        tenant.as_ref().map(|t| &t.0),
        run_live_validation_start(&state, &manager, input.into_start_input()),
    )
    .await?;
    Ok((status, AxumJson(result)))
}

/// GET /v1/live-validations/{validationId} — one attempt (404 when absent).
async fn get_live_validation(
    State(state): State<AppState>,
    Path(validation_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Json<Attempt>, ApiError> {
    let manager = live_validation_manager(&state)?;
    let item = with_tenant_context(tenant.as_ref().map(|t| &t.0), manager.get_attempt(&validation_id))
        .await
        .map_err(ApiError::internal)?;
    match item {
        Some(item) => Ok(Json(item)),
        None => Err(ApiError::NotFound("live validation not found".to_string())),
    }
}

/// GET /v1/live-validations/{validationId}/ledger — side-effect ledger entries.
async fn live_validation_ledger(
    State(state): State<AppState>,
    Path(validation_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Json<LiveValidationLedgerResponse>, ApiError> {
    let manager = live_validation_manager(&state)?;
    let tenant_id = resolved_tenant_id(tenant.as_ref().map(|t| &t.0));
    let filter = kura_livevalidation::LedgerFilter {
        tenant_id: tenant_id.clone(),
        validation_id: validation_id.clone(),
        tool_class: ToolClass::new(params.get("toolClass").cloned().unwrap_or_default()),
        // The outcome query filter is not ported: LedgerOutcome is defined in
        // the private ledger module and not re-exported by kura-livevalidation.
        limit: query_int(&params, "limit"),
        ..kura_livevalidation::LedgerFilter::default()
    };
    let items = manager.list_ledger_entries(filter).await.map_err(ApiError::internal)?;
    Ok(Json(LiveValidationLedgerResponse {
        validation_id,
        tenant_id,
        items,
    }))
}

/// POST /v1/live-validations/{validationId}/abort — abort an attempt.
async fn live_validation_abort(
    State(state): State<AppState>,
    Path(validation_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Json<Attempt>, ApiError> {
    let manager = live_validation_manager(&state)?;
    let item = with_tenant_context(tenant.as_ref().map(|t| &t.0), manager.abort(&validation_id))
        .await
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    let event = kura_events::live_validation_attempt_event(
        kura_events::LIVE_VALIDATION_ABORTED_NAME,
        item.clone(),
        &[],
    );
    state.event_bus.publish(event);
    Ok(Json(item))
}

/// GET /v1/live-validations/{validationId}/retention — default retention policy.
#[allow(clippy::unused_async)]
async fn live_validation_retention(
    State(state): State<AppState>,
    Path(_validation_id): Path<String>,
) -> Result<Json<RetentionPolicy>, ApiError> {
    let manager = live_validation_manager(&state)?;
    Ok(Json(manager.default_retention_policy()))
}

/// POST /v1/live-validations/{validationId}/compare — outcome comparison (202).
async fn live_validation_compare(
    State(state): State<AppState>,
    Path(validation_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<(StatusCode, AxumJson<Comparison>), ApiError> {
    let manager = live_validation_manager(&state)?;
    let comparison = with_tenant_context(tenant.as_ref().map(|t| &t.0), manager.create_comparison(&validation_id))
        .await
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    let event = kura_events::live_validation_comparison_event(comparison.clone());
    state.event_bus.publish(event);
    Ok((StatusCode::ACCEPTED, AxumJson(comparison)))
}

/// POST /v1/live-validations/{validationId}/reconciliations/{ambiguousCommitId}/resolve
/// — operator resolution of an ambiguous commit (403 without authority).
async fn live_validation_reconcile(
    State(state): State<AppState>,
    Path((_validation_id, ambiguous_commit_id)): Path<(String, String)>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<Json<ReconciliationResolution>, ApiError> {
    let manager = live_validation_manager(&state)?;
    let request: ResolveLiveValidationReconciliationRequest = if body.is_empty() {
        ResolveLiveValidationReconciliationRequest::default()
    } else {
        serde_json::from_slice(&body).map_err(|err| ApiError::BadRequest(err.to_string()))?
    };
    let resolution = with_tenant_context(
        tenant.as_ref().map(|t| &t.0),
        manager.resolve_reconciliation(ReconciliationResolution {
            ambiguous_commit_id,
            resolution: ReconciliationResolutionValue::new(request.resolution),
            reason: request.reason,
            evidence_refs: request.evidence_refs,
            ..ReconciliationResolution::default()
        }),
    )
    .await
    .map_err(|err| match err {
            LiveValidationError::ReconciliationPermissionDenied => {
                ApiError::Forbidden(err.to_string())
            }
            other => ApiError::BadRequest(other.to_string()),
        })?;
    let event = kura_events::live_validation_reconciliation_event(resolution.clone());
    state.event_bus.publish(event);
    Ok(Json(resolution))
}

// ---------------------------------------------------------------------------
// Live validations: kill switches + support matrix
// ---------------------------------------------------------------------------

/// GET /v1/live-validations/kill-switches — list (Go handleLiveValidationKillSwitches GET).
async fn list_kill_switches(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let manager = live_validation_manager(&state)?;
    let tenant_id = params
        .get("tenantId")
        .cloned()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            let resolved = resolved_tenant_id(tenant.as_ref().map(|t| &t.0));
            (!resolved.is_empty()).then_some(resolved)
        })
        .unwrap_or_default();
    let filter = KillSwitchFilter {
        tenant_id: tenant_id.clone(),
        scope: KillSwitchScope::new(params.get("scope").cloned().unwrap_or_default()),
        limit: query_int(&params, "limit"),
        ..KillSwitchFilter::default()
    };
    let items = manager.list_kill_switches(filter).await.map_err(ApiError::internal)?;
    Ok(Json(json!({ "tenantId": tenant_id, "items": items })))
}

/// POST /v1/live-validations/kill-switches — set a kill switch (403 without
/// reconciliation authority).
async fn set_kill_switch(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<Json<KillSwitch>, ApiError> {
    let manager = live_validation_manager(&state)?;
    let request: UpdateLiveValidationKillSwitchRequest = if body.is_empty() {
        UpdateLiveValidationKillSwitchRequest::default()
    } else {
        serde_json::from_slice(&body).map_err(|err| ApiError::BadRequest(err.to_string()))?
    };
    let item = with_tenant_context(
        tenant.as_ref().map(|t| &t.0),
        manager.set_kill_switch(KillSwitch {
            scope: KillSwitchScope::new(request.scope),
            tenant_id: request.tenant_id,
            enabled: request.enabled,
            reason: request.reason,
            expires_at: request.expires_at,
            ..KillSwitch::default()
        }),
    )
    .await
    .map_err(|err| match err {
            LiveValidationError::KillSwitchPermissionDenied => {
                ApiError::Forbidden(err.to_string())
            }
            other => ApiError::BadRequest(other.to_string()),
        })?;
    let event = kura_events::live_validation_attempt_event(
        kura_events::LIVE_VALIDATION_KILL_SWITCH_CHANGED_NAME,
        Attempt {
            tenant_id: item.tenant_id.clone(),
            validation_id: item.kill_switch_id.clone(),
            status: AttemptStatus::from(AttemptStatus::ABORTED),
            updated_at: item.changed_at,
            ..Attempt::default()
        },
        &[],
    );
    state.event_bus.publish(event);
    Ok(Json(item))
}

/// GET /v1/live-validations/support-matrix — the v1 support matrix.
#[allow(clippy::unused_async)]
async fn live_validation_support_matrix(
    State(state): State<AppState>,
) -> Result<Json<LiveValidationSupportMatrixResponse>, ApiError> {
    let manager = live_validation_manager(&state)?;
    let matrix = manager.support_matrix().map_err(ApiError::internal)?;
    Ok(Json(LiveValidationSupportMatrixResponse {
        environment_scope: manager.environment_scope().to_string(),
        version: "v1".to_string(),
        items: matrix.rows(),
    }))
}

// ---------------------------------------------------------------------------
// Live validations: connector conformance + smoke evidence
// ---------------------------------------------------------------------------

/// GET /v1/live-validations/{discord|telegram|slack|matrix}-conformance — the
/// tenant's connector conformance results (404 when none).
async fn live_validation_connector_conformance(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<LiveValidationDiscordConformanceResponse>, EvaluationApiError> {
    let _manager = live_validation_manager(&state)?;
    require_hosted_credential_read_any(
        tenant.as_ref().map(|t| &t.0),
        &[kura_identity::Permission::ConnectorsManage],
    )?;
    let tenant_id = tenant.as_ref().map(|t| t.0.0.tenant_id.clone()).unwrap_or_default();
    let connector_id = params
        .get("connectorId")
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    if connector_id.is_empty() {
        return Err(bad_request("connectorId is required"));
    }
    let items = state
        .store
        .lock()
        .list_connector_conformance_results(&tenant_id, &connector_id, Utc::now())
        .map_err(ApiError::from_store)?;
    if items.is_empty() {
        // Go: http.NotFound (plain-text 404); the JSON body is the crate's
        // standard not-found shape.
        return Err(EvaluationApiError::Api(ApiError::NotFound("not found".to_string())));
    }
    Ok(Json(LiveValidationDiscordConformanceResponse {
        tenant_id,
        connector_id,
        items,
    }))
}

/// GET /v1/live-validations/discord-smoke — latest Discord smoke evidence.
async fn live_validation_discord_smoke(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<kura_store::DiscordSmokeEvidenceRecord>, EvaluationApiError> {
    let _manager = live_validation_manager(&state)?;
    require_hosted_credential_read_any(
        tenant.as_ref().map(|t| &t.0),
        &[kura_identity::Permission::ConnectorsManage],
    )?;
    let tenant_id = tenant.as_ref().map(|t| t.0.0.tenant_id.clone()).unwrap_or_default();
    let connector_id = params
        .get("connectorId")
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    if connector_id.is_empty() {
        return Err(bad_request("connectorId is required"));
    }
    let evidence = state
        .store
        .lock()
        .latest_discord_smoke_evidence(&tenant_id, &connector_id, Utc::now())
        .map_err(ApiError::from_store)?;
    match evidence {
        Some(evidence) => Ok(Json(evidence)),
        None => Err(EvaluationApiError::Api(ApiError::NotFound("not found".to_string()))),
    }
}

/// GET /v1/live-validations/telegram-smoke — latest Telegram smoke evidence.
async fn live_validation_telegram_smoke(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<kura_store::TelegramSmokeEvidenceRecord>, EvaluationApiError> {
    let _manager = live_validation_manager(&state)?;
    require_hosted_credential_read_any(
        tenant.as_ref().map(|t| &t.0),
        &[kura_identity::Permission::ConnectorsManage],
    )?;
    let tenant_id = tenant.as_ref().map(|t| t.0.0.tenant_id.clone()).unwrap_or_default();
    let connector_id = params
        .get("connectorId")
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    if connector_id.is_empty() {
        return Err(bad_request("connectorId is required"));
    }
    let evidence = state
        .store
        .lock()
        .latest_telegram_smoke_evidence(&tenant_id, &connector_id, Utc::now())
        .map_err(ApiError::from_store)?;
    match evidence {
        Some(evidence) => Ok(Json(evidence)),
        None => Err(EvaluationApiError::Api(ApiError::NotFound("not found".to_string()))),
    }
}

/// GET /v1/live-validations/slack-smoke — latest Slack smoke evidence, projected
/// to the tenant-safe resource shape.
async fn live_validation_slack_smoke(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<SlackSmokeEvidenceResource>, EvaluationApiError> {
    let _manager = live_validation_manager(&state)?;
    require_hosted_credential_read_any(
        tenant.as_ref().map(|t| &t.0),
        &[kura_identity::Permission::ConnectorsManage],
    )?;
    let tenant_id = tenant.as_ref().map(|t| t.0.0.tenant_id.clone()).unwrap_or_default();
    let connector_id = params
        .get("connectorId")
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    if connector_id.is_empty() {
        return Err(bad_request("connectorId is required"));
    }
    let evidence = state
        .store
        .lock()
        .latest_slack_smoke_evidence(&tenant_id, &connector_id, Utc::now())
        .map_err(ApiError::from_store)?;
    match evidence {
        Some(evidence) => Ok(Json(project_slack_smoke_evidence_resource(&evidence))),
        None => Err(EvaluationApiError::Api(ApiError::NotFound("not found".to_string()))),
    }
}

/// GET /v1/live-validations/matrix-smoke — latest Matrix smoke evidence.
async fn live_validation_matrix_smoke(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<kura_store::MatrixSmokeEvidenceRecord>, EvaluationApiError> {
    let _manager = live_validation_manager(&state)?;
    require_hosted_credential_read_any(
        tenant.as_ref().map(|t| &t.0),
        &[kura_identity::Permission::ConnectorsManage],
    )?;
    let tenant_id = tenant.as_ref().map(|t| t.0.0.tenant_id.clone()).unwrap_or_default();
    let connector_id = params
        .get("connectorId")
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    if connector_id.is_empty() {
        return Err(bad_request("connectorId is required"));
    }
    let evidence = state
        .store
        .lock()
        .latest_matrix_smoke_evidence(&tenant_id, &connector_id, Utc::now())
        .map_err(ApiError::from_store)?;
    match evidence {
        Some(evidence) => Ok(Json(evidence)),
        None => Err(EvaluationApiError::Api(ApiError::NotFound("not found".to_string()))),
    }
}

/// POST /v1/live-validations/matrix-smoke — record structured smoke evidence.
/// Safe-live records require a provider-probe executor that has no Rust
/// equivalent yet (Go: 400 "matrix safe-live smoke executor is not configured").
async fn record_matrix_smoke(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Query(params): Query<HashMap<String, String>>,
    body: Bytes,
) -> Result<(StatusCode, AxumJson<kura_store::MatrixSmokeEvidenceRecord>), EvaluationApiError> {
    let _manager = live_validation_manager(&state)?;
    require_live_validation_execute(tenant.as_ref().map(|t| &t.0))?;
    let tenant_id = tenant.as_ref().map(|t| t.0.0.tenant_id.clone()).unwrap_or_default();
    let mut request: RecordMatrixSmokeRequest = if body.is_empty() {
        RecordMatrixSmokeRequest::default()
    } else {
        serde_json::from_slice(&body)
            .map_err(|err| EvaluationApiError::Api(ApiError::BadRequest(err.to_string())))?
    };
    if request.connector_id.trim().is_empty() {
        request.connector_id = params.get("connectorId").cloned().unwrap_or_default();
    }
    if request.connector_id.trim().is_empty() {
        return Err(bad_request("connectorId is required"));
    }
    if request.authorization_mode.trim() == "safe_live" {
        return Err(bad_request("matrix safe-live smoke executor is not configured"));
    }
    let record = matrix_smoke_record_from_request(&tenant_id, &request)?;
    state
        .store
        .lock()
        .save_matrix_smoke_evidence(&record)
        .map_err(ApiError::from_store)?;
    Ok((StatusCode::CREATED, AxumJson(record)))
}

/// Go matrixSmokeRecordFromRequest (live_validation.go): validates the
/// status/authorization-mode combination and builds the store record.
fn matrix_smoke_record_from_request(
    tenant_id: &str,
    input: &RecordMatrixSmokeRequest,
) -> Result<kura_store::MatrixSmokeEvidenceRecord, EvaluationApiError> {
    let mode = input.authorization_mode.trim();
    let status = input.status.trim();
    let mode = if mode.is_empty() { "unavailable" } else { mode };
    let status = if status.is_empty() { "skipped" } else { status };
    match status {
        "skipped" => {
            if mode != "unavailable" {
                return Err(bad_request(
                    "skipped Matrix smoke must use unavailable authorization mode",
                ));
            }
        }
        "passed" | "failed" => {
            if mode != "fake_matrix" && mode != "safe_live" {
                return Err(bad_request(
                    "passed or failed Matrix smoke requires fake_matrix or safe_live authorization mode",
                ));
            }
        }
        _ => return Err(bad_request("status must be passed, failed, or skipped")),
    }
    let validated_at = input.validated_at.unwrap_or_else(Utc::now).with_timezone(&Utc);
    let connector_id = input.connector_id.trim().to_string();
    let binding_id = first_non_empty_string(
        &input.homeserver_binding_id,
        &format!("matrix_homeserver_{connector_id}"),
    );
    let owner = first_non_empty_string(&input.owner, "operator");
    let reason = first_non_empty_string(&input.reason, "safe_matrix_authorization_unavailable");
    let mut remaining_risk = input.remaining_risk.clone();
    if remaining_risk.trim().is_empty() && status == "skipped" {
        remaining_risk =
            "No live Matrix hosted smoke was run; release review must consume this structured skip."
                .to_string();
    }
    Ok(kura_store::MatrixSmokeEvidenceRecord {
        smoke_evidence_id: format!("matrix_smoke_{connector_id}"),
        tenant_id: tenant_id.to_string(),
        connector_id,
        homeserver_binding_id: binding_id,
        status: status.to_string(),
        authorization_mode: mode.to_string(),
        owner,
        reason,
        remaining_risk,
        validated_at,
        retention_expires_at: validated_at + chrono::Duration::days(90),
        redaction_status: "redacted".to_string(),
        safe_evidence: input.safe_evidence.clone(),
    })
}

/// Go projectSlackSmokeEvidenceResource (setupwizard.go).
fn project_slack_smoke_evidence_resource(
    record: &kura_store::SlackSmokeEvidenceRecord,
) -> SlackSmokeEvidenceResource {
    SlackSmokeEvidenceResource {
        smoke_evidence_id: record.smoke_evidence_id.clone(),
        tenant_id: record.tenant_id.clone(),
        connector_id: record.connector_id.clone(),
        workspace_binding_id: record.workspace_binding_id.clone(),
        status: record.status.clone(),
        authorization_mode: record.authorization_mode.clone(),
        owner: record.owner.clone(),
        reason: record.reason.clone(),
        remaining_risk: record.remaining_risk.clone(),
        validated_at: record.validated_at,
        retention_expires_at: record.retention_expires_at,
        redaction_status: record.redaction_status.clone(),
        safe_evidence: record.safe_evidence.clone(),
    }
}

// ---------------------------------------------------------------------------
// Router assembly
// ---------------------------------------------------------------------------

/// Route family router.
///
/// The tenant-scoped evaluation product routes (discovery-policies,
/// discovery-runs, discovered-candidates, product-fixtures, suppressions,
/// campaigns, dashboard, tool-call-inspections, retention/apply) are not
/// registered: their SQLiteStore DAOs do not exist in kura-store (see the
/// module doc).
#[must_use]
// ---------------------------------------------------------------------------
// Evaluation product family (Go evaluation_product.go) — discovery policies,
// discovery runs, discovered candidates, product fixtures + revisions,
// suppressions, replay campaigns, dashboard projections, tool-call
// inspections, and retention/apply. All routes read the resolved tenant
// context (400 when absent) and answer on the kura-store evaluation_product
// DAOs; mutation routes requiring a capability check it with the
// evaluation.manage wildcard (Go evaluationProductRequestHasPermission).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct EvaluationProductListResponse<T> {
    tenant_id: String,
    page: kura_evaluation::ProductPage,
    items: Vec<T>,
}

/// Go upsertDiscoveryPolicyRequest.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpsertDiscoveryPolicyRequest {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    source_kinds: Vec<kura_evaluation::SourceKind>,
    #[serde(default)]
    window_start: DateTime<Utc>,
    #[serde(default)]
    window_end: DateTime<Utc>,
    #[serde(default)]
    max_inspected_records: i64,
    #[serde(default)]
    max_emitted_candidates: i64,
    #[serde(default)]
    cost_budget: i64,
    #[serde(default)]
    sensitive_field_rules: Vec<String>,
    #[serde(default)]
    retention_policy_ref: String,
    #[allow(dead_code)]
    #[serde(default)]
    idempotency_key: String,
}

/// Go startDiscoveryRunRequest.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartDiscoveryRunRequest {
    #[serde(default)]
    policy_id: String,
    #[serde(default)]
    window_start: DateTime<Utc>,
    #[serde(default)]
    window_end: DateTime<Utc>,
    #[serde(default)]
    source_kinds: Vec<kura_evaluation::SourceKind>,
    #[serde(default)]
    max_inspected_records: i64,
    #[serde(default)]
    max_emitted_candidates: i64,
    #[serde(default)]
    cost_budget: i64,
    #[serde(default)]
    cursor: String,
    #[serde(default)]
    idempotency_key: String,
}

/// Go materializeProductFixtureRequest.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MaterializeProductFixtureRequest {
    #[serde(default)]
    fixture_id: String,
    display_name: String,
    #[serde(default)]
    domain_class: kura_evaluation::FixtureDomainClass,
    #[serde(default)]
    fixture_payload: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    change_summary: String,
    #[serde(default)]
    idempotency_key: String,
}

/// Go createFixtureRevisionRequest.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateFixtureRevisionRequest {
    #[serde(default)]
    revision_id: String,
    #[serde(default)]
    fixture_payload: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    content_summary: String,
    #[serde(default)]
    change_summary: String,
    #[serde(default)]
    source_evidence_refs: Vec<String>,
    #[allow(dead_code)]
    #[serde(default)]
    idempotency_key: String,
}

/// Go reviewProductFixtureRequest.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewProductFixtureRequest {
    revision_id: String,
    decision: String,
    #[allow(dead_code)]
    #[serde(default)]
    reason: String,
}

/// Go createCampaignRequest.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateCampaignRequest {
    #[serde(default)]
    campaign_id: String,
    display_name: String,
    #[serde(default)]
    scope_summary: String,
    #[serde(default)]
    source_selections: Vec<CampaignSourceSelectionRequest>,
    #[serde(default)]
    start_immediately: bool,
    #[serde(default)]
    idempotency_key: String,
}

/// Go campaignSourceSelectionRequest.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CampaignSourceSelectionRequest {
    source_type: kura_evaluation::ProductResourceKind,
    source_id: String,
    #[serde(default)]
    source_snapshot: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    selection_reason: String,
}

/// Go productFixtureMutationResponse.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductFixtureMutationResponse {
    fixture: kura_evaluation::ProductManagedFixture,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    revision: Option<kura_evaluation::FixtureRevision>,
}

/// Go evaluationProductTenantIDFromRequest: a resolved tenant context is
/// required (400 with the stable product-tenant message otherwise).
fn evaluation_product_tenant(
    tenant: Option<&TenantContext>,
) -> Result<&kura_identity::TenantContext, ApiError> {
    match tenant {
        Some(tc) if !tc.0.tenant_id.trim().is_empty() => Ok(&tc.0),
        _ => Err(ApiError::BadRequest(
            kura_evaluation::EvaluationError::ProductTenantRequired.to_string(),
        )),
    }
}

/// Go evaluationProductRequestHasPermission: the specific permission or the
/// evaluation.manage wildcard.
fn evaluation_product_permission(
    tc: &kura_identity::TenantContext,
    permission: Permission,
) -> bool {
    has_permission(&tc.permissions, permission)
        || has_permission(&tc.permissions, Permission::EvaluationManage)
}

/// Go productPageFromRequest.
fn product_page(params: &HashMap<String, String>) -> kura_evaluation::ProductPage {
    kura_evaluation::ProductPage {
        cursor: params.get("cursor").cloned().unwrap_or_default(),
        limit: kura_evaluation::normalize_product_limit(query_int(params, "limit")),
    }
}

/// GET /v1/evaluation/discovery-policies (Go handleEvaluationProductDiscoveryPolicies).
async fn list_discovery_policies(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<EvaluationProductListResponse<kura_evaluation::DiscoveryPolicy>>, ApiError> {
    let tc = evaluation_product_tenant(tenant.as_ref().map(|e| &e.0))?;
    let enabled = match params.get("enabled") {
        Some(raw) if !raw.trim().is_empty() => Some(
            raw.trim().parse::<bool>().map_err(|_| ApiError::BadRequest("enabled must be a boolean".to_string()))?,
        ),
        _ => None,
    };
    let filter = kura_evaluation::DiscoveryPolicyFilter {
        base: kura_evaluation::ProductListFilter {
            tenant_id: tc.tenant_id.clone(),
            cursor: params.get("cursor").cloned().unwrap_or_default(),
            limit: query_int(&params, "limit"),
        },
        enabled,
    };
    let items = state.store.lock().list_discovery_policies(&filter).map_err(ApiError::from_store)?;
    Ok(Json(EvaluationProductListResponse {
        tenant_id: tc.tenant_id.clone(),
        page: product_page(&params),
        items,
    }))
}

/// GET /v1/evaluation/discovery-policies/{policy_id} — one policy.
async fn get_discovery_policy(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(policy_id): Path<String>,
) -> Result<Json<kura_evaluation::DiscoveryPolicy>, ApiError> {
    let tc = evaluation_product_tenant(tenant.as_ref().map(|e| &e.0))?;
    let item = state
        .store
        .lock()
        .get_discovery_policy(&tc.tenant_id, &policy_id)
        .map_err(ApiError::from_store)?
        .ok_or_else(|| ApiError::NotFound("discovery policy not found".to_string()))?;
    Ok(Json(item))
}

/// PUT /v1/evaluation/discovery-policies/{policy_id} — upsert a policy (Go
/// handleEvaluationProductDiscoveryPolicyRoutes PUT branch).
async fn upsert_discovery_policy(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(policy_id): Path<String>,
    body: Bytes,
) -> Result<Json<kura_evaluation::DiscoveryPolicy>, ApiError> {
    let tc = evaluation_product_tenant(tenant.as_ref().map(|e| &e.0))?;
    let input: UpsertDiscoveryPolicyRequest = decode_optional_json_body(&body)?;
    let now = Utc::now();
    let item = kura_evaluation::DiscoveryPolicy {
        policy_id: policy_id.clone(),
        tenant_id: tc.tenant_id.clone(),
        enabled: input.enabled,
        source_kinds: input.source_kinds,
        window_start: input.window_start,
        window_end: input.window_end,
        max_inspected_records: input.max_inspected_records,
        max_emitted_candidates: input.max_emitted_candidates,
        cost_budget: input.cost_budget,
        sensitive_field_rules: input.sensitive_field_rules,
        retention_policy_ref: input.retention_policy_ref,
        created_by: tc.principal_id.clone(),
        created_at: now,
        updated_at: now,
    };
    state
        .store
        .lock()
        .upsert_discovery_policy(item.clone())
        .map_err(ApiError::BadRequest)?;
    Ok(Json(item))
}

/// GET /v1/evaluation/discovery-runs (Go
/// handleEvaluationProductDiscoveryRuns GET branch).
async fn list_discovery_runs(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<EvaluationProductListResponse<kura_evaluation::DiscoveryRun>>, ApiError> {
    let tc = evaluation_product_tenant(tenant.as_ref().map(|e| &e.0))?;
    let filter = kura_evaluation::DiscoveryRunFilter {
        base: kura_evaluation::ProductListFilter {
            tenant_id: tc.tenant_id.clone(),
            cursor: params.get("cursor").cloned().unwrap_or_default(),
            limit: query_int(&params, "limit"),
        },
        status: parse_enum::<kura_evaluation::ProductLifecycleStatus>(params.get("status").cloned().unwrap_or_default().as_str()),
        source_kind: parse_enum::<kura_evaluation::SourceKind>(params.get("sourceKind").cloned().unwrap_or_default().as_str()),
    };
    let items = state.store.lock().list_discovery_runs(&filter).map_err(ApiError::from_store)?;
    Ok(Json(EvaluationProductListResponse {
        tenant_id: tc.tenant_id.clone(),
        page: product_page(&params),
        items,
    }))
}

/// POST /v1/evaluation/discovery-runs — start a discovery run (202; Go
/// handleEvaluationProductDiscoveryRuns POST branch).
async fn start_discovery_run(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<(StatusCode, AxumJson<kura_evaluation::DiscoveryRun>), ApiError> {
    let tc = evaluation_product_tenant(tenant.as_ref().map(|e| &e.0))?;
    let input: StartDiscoveryRunRequest = decode_optional_json_body(&body)?;
    let now = Utc::now();
    let policy = if input.policy_id.trim().is_empty() {
        kura_evaluation::DiscoveryPolicy {
            policy_id: String::new(),
            tenant_id: tc.tenant_id.clone(),
            enabled: true,
            source_kinds: input.source_kinds.clone(),
            window_start: input.window_start,
            window_end: input.window_end,
            max_inspected_records: input.max_inspected_records,
            max_emitted_candidates: input.max_emitted_candidates,
            cost_budget: input.cost_budget,
            ..Default::default()
        }
    } else {
        let existing = state
            .store
            .lock()
            .get_discovery_policy(&tc.tenant_id, &input.policy_id)
            .map_err(ApiError::from_store)?
            .ok_or_else(|| ApiError::NotFound("discovery policy not found".to_string()))?;
        existing
    };
    let run = kura_evaluation::build_discovery_run_from_policy(
        policy,
        kura_evaluation::StartDiscoveryRunInput {
            window_start: input.window_start,
            window_end: input.window_end,
            source_kinds: input.source_kinds,
            max_inspected_records: input.max_inspected_records,
            max_emitted_candidates: input.max_emitted_candidates,
            cost_budget: input.cost_budget,
            cursor: input.cursor,
            started_by: tc.principal_id.clone(),
            idempotency_key: input.idempotency_key,
        },
        now,
    )
    .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    state.store.lock().save_discovery_run(run.clone()).map_err(ApiError::from_store)?;
    Ok((StatusCode::ACCEPTED, AxumJson(run)))
}

/// GET /v1/evaluation/discovery-runs/{discovery_run_id} (Go
/// handleEvaluationProductDiscoveryRunRoutes).
async fn get_discovery_run(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(discovery_run_id): Path<String>,
) -> Result<Json<kura_evaluation::DiscoveryRun>, ApiError> {
    let tc = evaluation_product_tenant(tenant.as_ref().map(|e| &e.0))?;
    let item = state
        .store
        .lock()
        .get_discovery_run(&tc.tenant_id, &discovery_run_id)
        .map_err(ApiError::from_store)?
        .ok_or_else(|| ApiError::NotFound("discovery run not found".to_string()))?;
    Ok(Json(item))
}

/// GET /v1/evaluation/discovered-candidates (Go
/// handleEvaluationProductDiscoveredCandidates).
async fn list_discovered_candidates(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<EvaluationProductListResponse<kura_evaluation::DiscoveredCandidate>>, ApiError> {
    let tc = evaluation_product_tenant(tenant.as_ref().map(|e| &e.0))?;
    let filter = kura_evaluation::DiscoveredCandidateFilter {
        base: kura_evaluation::ProductListFilter {
            tenant_id: tc.tenant_id.clone(),
            cursor: params.get("cursor").cloned().unwrap_or_default(),
            limit: query_int(&params, "limit"),
        },
        discovery_run_id: params.get("discoveryRunId").cloned().unwrap_or_default(),
        source_kind: parse_enum::<kura_evaluation::SourceKind>(params.get("sourceKind").cloned().unwrap_or_default().as_str()),
        readiness_status: parse_enum::<kura_evaluation::ReadinessStatus>(params.get("readinessStatus").cloned().unwrap_or_default().as_str()),
        suppression_state: parse_enum::<kura_evaluation::SuppressionState>(params.get("suppressionState").cloned().unwrap_or_default().as_str()),
        score_band: parse_enum::<kura_evaluation::ScoreBand>(params.get("scoreBand").cloned().unwrap_or_default().as_str()),
    };
    let items = state.store.lock().list_discovered_candidates(&filter).map_err(ApiError::from_store)?;
    Ok(Json(EvaluationProductListResponse {
        tenant_id: tc.tenant_id.clone(),
        page: product_page(&params),
        items,
    }))
}

/// GET /v1/evaluation/discovered-candidates/{discovered_candidate_id} (Go
/// handleEvaluationProductDiscoveredCandidateRoutes GET branch).
async fn get_discovered_candidate(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(discovered_candidate_id): Path<String>,
) -> Result<Json<kura_evaluation::DiscoveredCandidate>, ApiError> {
    let tc = evaluation_product_tenant(tenant.as_ref().map(|e| &e.0))?;
    let item = state
        .store
        .lock()
        .get_discovered_candidate(&tc.tenant_id, &discovered_candidate_id)
        .map_err(ApiError::from_store)?
        .ok_or_else(|| ApiError::NotFound("discovered candidate not found".to_string()))?;
    Ok(Json(item))
}

/// POST /v1/evaluation/discovered-candidates/{id}/product-fixtures —
/// materialize a product fixture from a discovered candidate (201; Go
/// handleEvaluationProductFixtureMaterialization). Requires
/// evaluation.fixture.manage.
async fn materialize_product_fixture(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(discovered_candidate_id): Path<String>,
    body: Bytes,
) -> Result<(StatusCode, AxumJson<ProductFixtureMutationResponse>), ApiError> {
    let tc = evaluation_product_tenant(tenant.as_ref().map(|e| &e.0))?;
    if !evaluation_product_permission(tc, Permission::EvaluationFixtureManage) {
        return Err(ApiError::Forbidden("evaluation.fixture.manage is required".to_string()));
    }
    let input: MaterializeProductFixtureRequest = decode_optional_json_body(&body)?;
    let candidate = state
        .store
        .lock()
        .get_discovered_candidate(&tc.tenant_id, &discovered_candidate_id)
        .map_err(ApiError::from_store)?
        .ok_or_else(|| ApiError::NotFound("discovered candidate not found".to_string()))?;
    let evidence = state
        .store
        .lock()
        .get_latest_candidate_evidence(&tc.tenant_id, &discovered_candidate_id)
        .map_err(ApiError::from_store)?
        .ok_or_else(|| ApiError::BadRequest("candidate evidence not found".to_string()))?;
    let (fixture, revision) = kura_evaluation::create_product_fixture_from_candidate(
        kura_evaluation::ProductFixtureInput {
            fixture_id: input.fixture_id,
            tenant_id: tc.tenant_id.clone(),
            display_name: input.display_name,
            domain_class: input.domain_class,
            source_candidate: candidate,
            source_evidence: evidence,
            fixture_payload: input.fixture_payload,
            change_summary: input.change_summary,
            created_by: tc.principal_id.clone(),
            idempotency_key: input.idempotency_key,
        },
        Utc::now(),
    )
    .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    state
        .store
        .lock()
        .upsert_product_fixture(fixture.clone())
        .map_err(ApiError::from_store)?;
    state
        .store
        .lock()
        .save_fixture_revision(revision.clone())
        .map_err(ApiError::from_store)?;
    Ok((StatusCode::CREATED, AxumJson(ProductFixtureMutationResponse {
        fixture,
        revision: Some(revision),
    })))
}

/// GET /v1/evaluation/product-fixtures (Go handleEvaluationProductFixtures).
/// Requires evaluation.fixture.read.
async fn list_product_fixtures(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<EvaluationProductListResponse<kura_evaluation::ProductManagedFixture>>, ApiError> {
    let tc = evaluation_product_tenant(tenant.as_ref().map(|e| &e.0))?;
    if !evaluation_product_permission(tc, Permission::EvaluationFixtureRead) {
        return Err(ApiError::Forbidden("evaluation.fixture.read is required".to_string()));
    }
    let filter = kura_evaluation::ProductListFilter {
        tenant_id: tc.tenant_id.clone(),
        cursor: params.get("cursor").cloned().unwrap_or_default(),
        limit: query_int(&params, "limit"),
    };
    let items = state.store.lock().list_product_fixtures(&filter).map_err(ApiError::from_store)?;
    Ok(Json(EvaluationProductListResponse {
        tenant_id: tc.tenant_id.clone(),
        page: product_page(&params),
        items,
    }))
}

/// GET /v1/evaluation/product-fixtures/{fixture_id} (Go
/// handleEvaluationProductFixtureRoutes single-segment branch).
async fn get_product_fixture(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(fixture_id): Path<String>,
) -> Result<Json<kura_evaluation::ProductManagedFixture>, ApiError> {
    let tc = evaluation_product_tenant(tenant.as_ref().map(|e| &e.0))?;
    if !evaluation_product_permission(tc, Permission::EvaluationFixtureRead) {
        return Err(ApiError::Forbidden("evaluation.fixture.read is required".to_string()));
    }
    let item = state
        .store
        .lock()
        .get_product_fixture(&tc.tenant_id, &fixture_id)
        .map_err(ApiError::from_store)?
        .ok_or_else(|| ApiError::NotFound("product fixture not found".to_string()))?;
    Ok(Json(item))
}

/// GET /v1/evaluation/product-fixtures/{fixture_id}/revisions (Go
/// handleEvaluationProductFixtureRevisions GET branch).
async fn list_fixture_revisions(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(fixture_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<EvaluationProductListResponse<kura_evaluation::FixtureRevision>>, ApiError> {
    let tc = evaluation_product_tenant(tenant.as_ref().map(|e| &e.0))?;
    if !evaluation_product_permission(tc, Permission::EvaluationFixtureRead) {
        return Err(ApiError::Forbidden("evaluation.fixture.read is required".to_string()));
    }
    let items = state
        .store
        .lock()
        .list_fixture_revisions(&tc.tenant_id, &fixture_id, query_int(&params, "limit"))
        .map_err(ApiError::from_store)?;
    Ok(Json(EvaluationProductListResponse {
        tenant_id: tc.tenant_id.clone(),
        page: product_page(&params),
        items,
    }))
}

/// POST /v1/evaluation/product-fixtures/{fixture_id}/revisions — create a
/// fixture revision (201; Go handleEvaluationProductFixtureRevisions POST
/// branch). Requires evaluation.fixture.manage.
async fn create_fixture_revision(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(fixture_id): Path<String>,
    body: Bytes,
) -> Result<(StatusCode, AxumJson<ProductFixtureMutationResponse>), ApiError> {
    let tc = evaluation_product_tenant(tenant.as_ref().map(|e| &e.0))?;
    if !evaluation_product_permission(tc, Permission::EvaluationFixtureManage) {
        return Err(ApiError::Forbidden("evaluation.fixture.manage is required".to_string()));
    }
    let input: CreateFixtureRevisionRequest = decode_optional_json_body(&body)?;
    let fixture = state
        .store
        .lock()
        .get_product_fixture(&tc.tenant_id, &fixture_id)
        .map_err(ApiError::from_store)?
        .ok_or_else(|| ApiError::NotFound("product fixture not found".to_string()))?;
    let revisions = state
        .store
        .lock()
        .list_fixture_revisions(&tc.tenant_id, &fixture_id, 1)
        .map_err(ApiError::from_store)?;
    let next_revision_number = revisions.first().map(|r| r.revision_number + 1).unwrap_or(1);
    let (updated, revision) = kura_evaluation::create_product_fixture_revision(
        fixture,
        kura_evaluation::FixtureRevisionInput {
            revision_id: input.revision_id,
            fixture_payload: input.fixture_payload,
            content_summary: input.content_summary,
            change_summary: input.change_summary,
            source_evidence_refs: input.source_evidence_refs,
            redaction_status: kura_evaluation::RedactionStatus::Clean,
            created_by: tc.principal_id.clone(),
        },
        next_revision_number,
        Utc::now(),
    )
    .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    state
        .store
        .lock()
        .upsert_product_fixture(updated.clone())
        .map_err(ApiError::from_store)?;
    state
        .store
        .lock()
        .save_fixture_revision(revision.clone())
        .map_err(ApiError::from_store)?;
    Ok((StatusCode::CREATED, AxumJson(ProductFixtureMutationResponse {
        fixture: updated,
        revision: Some(revision),
    })))
}

/// POST /v1/evaluation/product-fixtures/{fixture_id}/review (Go
/// handleEvaluationProductFixtureReview). Requires evaluation.fixture.review.
async fn review_product_fixture_route(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(fixture_id): Path<String>,
    body: Bytes,
) -> Result<Json<ProductFixtureMutationResponse>, ApiError> {
    let tc = evaluation_product_tenant(tenant.as_ref().map(|e| &e.0))?;
    if !evaluation_product_permission(tc, Permission::EvaluationFixtureReview) {
        return Err(ApiError::Forbidden("evaluation.fixture.review is required".to_string()));
    }
    let input: ReviewProductFixtureRequest = decode_optional_json_body(&body)?;
    let fixture = state
        .store
        .lock()
        .get_product_fixture(&tc.tenant_id, &fixture_id)
        .map_err(ApiError::from_store)?
        .ok_or_else(|| ApiError::NotFound("product fixture not found".to_string()))?;
    let decision = match input.decision.as_str() {
        "approved" => kura_evaluation::FixtureReviewDecision::Approved,
        "rejected" => kura_evaluation::FixtureReviewDecision::Rejected,
        "needs_changes" => kura_evaluation::FixtureReviewDecision::NeedsChanges,
        _ => return Err(ApiError::BadRequest("invalid fixture review decision".to_string())),
    };
    let updated = kura_evaluation::review_product_fixture(
        fixture,
        &input.revision_id,
        decision,
        Utc::now(),
    )
    .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    state
        .store
        .lock()
        .upsert_product_fixture(updated.clone())
        .map_err(ApiError::from_store)?;
    Ok(Json(ProductFixtureMutationResponse { fixture: updated, revision: None }))
}

/// POST /v1/evaluation/product-fixtures/{fixture_id}/suppress (Go
/// handleEvaluationProductFixtureSuppress). Requires evaluation.fixture.suppress.
async fn suppress_product_fixture_route(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(fixture_id): Path<String>,
) -> Result<Json<ProductFixtureMutationResponse>, ApiError> {
    let tc = evaluation_product_tenant(tenant.as_ref().map(|e| &e.0))?;
    if !evaluation_product_permission(tc, Permission::EvaluationFixtureSuppress) {
        return Err(ApiError::Forbidden("evaluation.fixture.suppress is required".to_string()));
    }
    let fixture = state
        .store
        .lock()
        .get_product_fixture(&tc.tenant_id, &fixture_id)
        .map_err(ApiError::from_store)?
        .ok_or_else(|| ApiError::NotFound("product fixture not found".to_string()))?;
    let updated = kura_evaluation::suppress_product_fixture(fixture, Utc::now())
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    state
        .store
        .lock()
        .upsert_product_fixture(updated.clone())
        .map_err(ApiError::from_store)?;
    Ok(Json(ProductFixtureMutationResponse { fixture: updated, revision: None }))
}

/// POST /v1/evaluation/suppressions — create a suppression (201; Go
/// handleEvaluationProductSuppressions).
async fn create_suppression(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<(StatusCode, AxumJson<kura_evaluation::SuppressionRecord>), ApiError> {
    let tc = evaluation_product_tenant(tenant.as_ref().map(|e| &e.0))?;
    let input: CreateSuppressionRequest = decode_optional_json_body(&body)?;
    let now = Utc::now();
    let item = kura_evaluation::SuppressionRecord {
        suppression_id: if input.suppression_id.is_empty() {
            format!("suppression_{}", now.timestamp_nanos_opt().unwrap_or_default())
        } else {
            input.suppression_id
        },
        tenant_id: tc.tenant_id.clone(),
        target_kind: input.target_kind,
        target_id: input.target_id,
        target_source_ref: input.target_source_ref,
        reason_code: input.reason_code,
        reason: input.reason,
        created_by: if input.created_by.is_empty() {
            tc.principal_id.clone()
        } else {
            input.created_by
        },
        created_at: now,
        expires_at: input.expires_at,
        active: true,
    };
    state.store.lock().create_suppression(item.clone()).map_err(ApiError::BadRequest)?;
    Ok((StatusCode::CREATED, AxumJson(item)))
}

/// GET /v1/evaluation/campaigns (Go handleEvaluationProductCampaigns GET
/// branch). Requires evaluation.campaign.read.
async fn list_replay_campaigns(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<EvaluationProductListResponse<kura_evaluation::ReplayCampaign>>, ApiError> {
    let tc = evaluation_product_tenant(tenant.as_ref().map(|e| &e.0))?;
    if !evaluation_product_permission(tc, Permission::EvaluationCampaignRead) {
        return Err(ApiError::Forbidden("evaluation.campaign.read is required".to_string()));
    }
    let filter = kura_evaluation::ProductListFilter {
        tenant_id: tc.tenant_id.clone(),
        cursor: params.get("cursor").cloned().unwrap_or_default(),
        limit: query_int(&params, "limit"),
    };
    let items = state.store.lock().list_replay_campaigns(&filter).map_err(ApiError::from_store)?;
    Ok(Json(EvaluationProductListResponse {
        tenant_id: tc.tenant_id.clone(),
        page: product_page(&params),
        items,
    }))
}

/// POST /v1/evaluation/campaigns — create a replay campaign (201; Go
/// handleEvaluationProductCampaigns POST branch). Requires
/// evaluation.campaign.manage.
async fn create_replay_campaign_route(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<(StatusCode, AxumJson<kura_evaluation::ReplayCampaign>), ApiError> {
    let tc = evaluation_product_tenant(tenant.as_ref().map(|e| &e.0))?;
    if !evaluation_product_permission(tc, Permission::EvaluationCampaignManage) {
        return Err(ApiError::Forbidden("evaluation.campaign.manage is required".to_string()));
    }
    let input: CreateCampaignRequest = decode_optional_json_body(&body)?;
    let selections = campaign_source_selections(&state, tc, &input.source_selections)?;
    let (campaign, items) = kura_evaluation::create_replay_campaign(
        kura_evaluation::CreateCampaignInput {
            campaign_id: input.campaign_id,
            tenant_id: tc.tenant_id.clone(),
            display_name: input.display_name,
            scope_summary: input.scope_summary,
            started_by: tc.principal_id.clone(),
            idempotency_key: input.idempotency_key,
            source_selections: selections,
            start_immediately: input.start_immediately,
        },
        Utc::now(),
    )
    .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    state
        .store
        .lock()
        .save_replay_campaign(campaign.clone())
        .map_err(ApiError::from_store)?;
    for item in items {
        state.store.lock().save_campaign_item(item).map_err(ApiError::from_store)?;
    }
    Ok((StatusCode::CREATED, AxumJson(campaign)))
}

/// GET /v1/evaluation/campaigns/{campaign_id} (Go
/// handleEvaluationProductCampaignRoutes single-segment branch).
async fn get_replay_campaign(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(campaign_id): Path<String>,
) -> Result<Json<kura_evaluation::ReplayCampaign>, ApiError> {
    let tc = evaluation_product_tenant(tenant.as_ref().map(|e| &e.0))?;
    if !evaluation_product_permission(tc, Permission::EvaluationCampaignRead) {
        return Err(ApiError::Forbidden("evaluation.campaign.read is required".to_string()));
    }
    let item = state
        .store
        .lock()
        .get_replay_campaign(&tc.tenant_id, &campaign_id)
        .map_err(ApiError::from_store)?
        .ok_or_else(|| ApiError::NotFound("campaign not found".to_string()))?;
    Ok(Json(item))
}

/// GET /v1/evaluation/campaigns/{campaign_id}/items (Go
/// handleEvaluationProductCampaignRoutes items branch).
async fn list_campaign_items(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(campaign_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<EvaluationProductListResponse<kura_evaluation::CampaignItem>>, ApiError> {
    let tc = evaluation_product_tenant(tenant.as_ref().map(|e| &e.0))?;
    if !evaluation_product_permission(tc, Permission::EvaluationCampaignRead) {
        return Err(ApiError::Forbidden("evaluation.campaign.read is required".to_string()));
    }
    let filter = kura_evaluation::ProductListFilter {
        tenant_id: tc.tenant_id.clone(),
        cursor: params.get("cursor").cloned().unwrap_or_default(),
        limit: query_int(&params, "limit"),
    };
    let items = state
        .store
        .lock()
        .list_campaign_items(&filter, &campaign_id)
        .map_err(ApiError::from_store)?;
    Ok(Json(EvaluationProductListResponse {
        tenant_id: tc.tenant_id.clone(),
        page: product_page(&params),
        items,
    }))
}

/// GET /v1/evaluation/campaigns/{campaign_id}/attempt-groups (Go
/// handleEvaluationProductCampaignRoutes attempt-groups branch).
async fn list_campaign_attempt_groups(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(campaign_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<EvaluationProductListResponse<kura_evaluation::CampaignAttemptGroup>>, ApiError> {
    let tc = evaluation_product_tenant(tenant.as_ref().map(|e| &e.0))?;
    if !evaluation_product_permission(tc, Permission::EvaluationCampaignRead) {
        return Err(ApiError::Forbidden("evaluation.campaign.read is required".to_string()));
    }
    let filter = kura_evaluation::ProductListFilter {
        tenant_id: tc.tenant_id.clone(),
        cursor: params.get("cursor").cloned().unwrap_or_default(),
        limit: query_int(&params, "limit"),
    };
    let items = state
        .store
        .lock()
        .list_campaign_attempt_groups(&filter, &campaign_id)
        .map_err(ApiError::from_store)?;
    Ok(Json(EvaluationProductListResponse {
        tenant_id: tc.tenant_id.clone(),
        page: product_page(&params),
        items,
    }))
}

/// POST /v1/evaluation/campaigns/{campaign_id}/{start|complete|cancel|publish-results}
/// (Go handleEvaluationProductCampaignTransition). Requires
/// evaluation.campaign.manage.
async fn campaign_transition_route(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path((campaign_id, action)): Path<(String, String)>,
) -> Result<Json<kura_evaluation::ReplayCampaign>, ApiError> {
    let tc = evaluation_product_tenant(tenant.as_ref().map(|e| &e.0))?;
    if !evaluation_product_permission(tc, Permission::EvaluationCampaignManage) {
        return Err(ApiError::Forbidden("evaluation.campaign.manage is required".to_string()));
    }
    let campaign = state
        .store
        .lock()
        .get_replay_campaign(&tc.tenant_id, &campaign_id)
        .map_err(ApiError::from_store)?
        .ok_or_else(|| ApiError::NotFound("campaign not found".to_string()))?;
    let transition = match action.as_str() {
        "start" => kura_evaluation::CampaignTransition::Start,
        "complete" => kura_evaluation::CampaignTransition::Complete,
        "cancel" => kura_evaluation::CampaignTransition::Cancel,
        "publish-results" => kura_evaluation::CampaignTransition::Publish,
        _ => return Err(ApiError::NotFound("campaign route not found".to_string())),
    };
    let updated = kura_evaluation::transition_replay_campaign(campaign, transition, Utc::now())
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    state
        .store
        .lock()
        .save_replay_campaign(updated.clone())
        .map_err(ApiError::from_store)?;
    Ok(Json(updated))
}

/// GET /v1/evaluation/campaigns/{campaign_id}/tool-call-inspections (Go
/// handleEvaluationProductCampaignRoutes tool-call-inspections branch).
async fn list_campaign_tool_call_inspections(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(campaign_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<EvaluationProductListResponse<kura_evaluation::ToolCallInspection>>, ApiError> {
    let tc = evaluation_product_tenant(tenant.as_ref().map(|e| &e.0))?;
    if !evaluation_product_permission(tc, Permission::EvaluationInspectionRead) {
        return Err(ApiError::Forbidden("evaluation.inspection.read is required".to_string()));
    }
    let filter = kura_evaluation::ProductListFilter {
        tenant_id: tc.tenant_id.clone(),
        cursor: params.get("cursor").cloned().unwrap_or_default(),
        limit: query_int(&params, "limit"),
    };
    let items = state
        .store
        .lock()
        .list_tool_call_inspections(&filter, &campaign_id)
        .map_err(ApiError::from_store)?;
    Ok(Json(EvaluationProductListResponse {
        tenant_id: tc.tenant_id.clone(),
        page: product_page(&params),
        items,
    }))
}

/// GET /v1/evaluation/dashboard (Go handleEvaluationProductDashboard).
/// Requires evaluation.dashboard.read.
async fn list_dashboard_projections(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<EvaluationProductListResponse<kura_evaluation::DashboardProjection>>, ApiError> {
    let tc = evaluation_product_tenant(tenant.as_ref().map(|e| &e.0))?;
    if !evaluation_product_permission(tc, Permission::EvaluationDashboardRead) {
        return Err(ApiError::Forbidden("evaluation.dashboard.read is required".to_string()));
    }
    let filter = kura_evaluation::ProductListFilter {
        tenant_id: tc.tenant_id.clone(),
        cursor: params.get("cursor").cloned().unwrap_or_default(),
        limit: query_int(&params, "limit"),
    };
    let items = state.store.lock().list_dashboard_projections(&filter).map_err(ApiError::from_store)?;
    Ok(Json(EvaluationProductListResponse {
        tenant_id: tc.tenant_id.clone(),
        page: product_page(&params),
        items,
    }))
}

/// GET /v1/evaluation/tool-call-inspections/{inspection_id} (Go
/// handleEvaluationProductToolCallInspectionRoutes). Requires
/// evaluation.inspection.read.
async fn get_tool_call_inspection(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(inspection_id): Path<String>,
) -> Result<Json<kura_evaluation::ToolCallInspection>, ApiError> {
    let tc = evaluation_product_tenant(tenant.as_ref().map(|e| &e.0))?;
    if !evaluation_product_permission(tc, Permission::EvaluationInspectionRead) {
        return Err(ApiError::Forbidden("evaluation.inspection.read is required".to_string()));
    }
    let item = state
        .store
        .lock()
        .get_tool_call_inspection(&tc.tenant_id, &inspection_id)
        .map_err(ApiError::from_store)?
        .ok_or_else(|| ApiError::NotFound("tool-call inspection not found".to_string()))?;
    Ok(Json(item))
}

/// POST /v1/evaluation/retention/apply — apply product retention (Go
/// handleEvaluationProductRoutes retention/apply branch answers 501 "not
/// enabled"; the kura-store apply_retention DAO exists, so this wave
/// implements the real handler with a dry_run flag).
async fn apply_evaluation_retention(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<(StatusCode, AxumJson<serde_json::Value>), ApiError> {
    let tc = evaluation_product_tenant(tenant.as_ref().map(|e| &e.0))?;
    let input: ApplyRetentionRequest = decode_optional_json_body(&body)?;
    let filter = kura_evaluation::RetentionApplicationFilter {
        base: kura_evaluation::ProductListFilter {
            tenant_id: tc.tenant_id.clone(),
            cursor: String::new(),
            limit: 0,
        },
        resource_kinds: input.resource_kinds,
        dry_run: input.dry_run,
    };
    let application_ids = state.store.lock().apply_retention(&filter).map_err(ApiError::from_store)?;
    Ok((StatusCode::OK, AxumJson(serde_json::json!({
        "tenantId": tc.tenant_id,
        "dryRun": input.dry_run,
        "applicationIds": application_ids,
    }))))
}

/// Go campaignSourceSelectionsFromRequest: resolves each selection against
/// the store and snapshots the source state.
fn campaign_source_selections(
    state: &AppState,
    tc: &kura_identity::TenantContext,
    inputs: &[CampaignSourceSelectionRequest],
) -> Result<Vec<kura_evaluation::CampaignSourceSelection>, ApiError> {
    let mut selections = Vec::with_capacity(inputs.len());
    for input in inputs {
        let mut selection = kura_evaluation::CampaignSourceSelection {
            source_type: input.source_type.clone(),
            source_id: input.source_id.clone(),
            tenant_id: tc.tenant_id.clone(),
            source_snapshot: input.source_snapshot.clone(),
            selection_reason: input.selection_reason.clone(),
            ..Default::default()
        };
        match input.source_type {
            kura_evaluation::ProductResourceKind::ProductFixture => {
                let fixture = state
                    .store
                    .lock()
                    .get_product_fixture(&tc.tenant_id, &input.source_id)
                    .map_err(ApiError::from_store)?
                    .ok_or_else(|| {
                        ApiError::BadRequest(kura_evaluation::EvaluationError::CampaignSelectionInvalid.to_string())
                    })?;
                selection.suppression_state = fixture.suppression_state.clone();
                selection.retention_state = fixture.retention_state.clone();
                selection.review_state = fixture.review_state.clone();
                selection.source_snapshot = serde_json::json!({
                    "fixtureId": fixture.fixture_id,
                    "displayName": fixture.display_name,
                    "currentRevisionId": fixture.current_revision_id,
                    "reviewState": fixture.review_state,
                    "retentionState": fixture.retention_state,
                    "suppressionState": fixture.suppression_state,
                })
                .as_object()
                .cloned()
                .unwrap_or_default();
            }
            kura_evaluation::ProductResourceKind::DiscoveredCandidate => {
                let candidate = state
                    .store
                    .lock()
                    .get_discovered_candidate(&tc.tenant_id, &input.source_id)
                    .map_err(ApiError::from_store)?
                    .ok_or_else(|| {
                        ApiError::BadRequest(kura_evaluation::EvaluationError::CampaignSelectionInvalid.to_string())
                    })?;
                selection.suppression_state = candidate.suppression_state.clone();
                selection.retention_state = candidate.retention_state.clone();
                selection.source_snapshot = serde_json::json!({
                    "discoveredCandidateId": candidate.discovered_candidate_id,
                    "sourceKind": candidate.source_kind,
                    "sourceId": candidate.source_id,
                    "score": candidate.score,
                    "scoreBand": candidate.score_band,
                    "readinessStatus": candidate.readiness_status,
                    "retentionState": candidate.retention_state,
                    "suppressionState": candidate.suppression_state,
                })
                .as_object()
                .cloned()
                .unwrap_or_default();
            }
            _ => {
                if selection.retention_state.as_str().is_empty() {
                    selection.retention_state = kura_evaluation::RetentionState::Active;
                }
                if selection.suppression_state.as_str().is_empty() {
                    selection.suppression_state = kura_evaluation::SuppressionState::None;
                }
            }
        }
        selections.push(selection);
    }
    Ok(selections)
}

/// Go decodeOptionalJSON: empty body decodes to the zero value; malformed
/// JSON answers 400.
fn decode_optional_json_body<T: serde::de::DeserializeOwned>(body: &Bytes) -> Result<T, ApiError> {
    if body.is_empty() {
        return serde_json::from_slice(b"{}").map_err(|err| ApiError::BadRequest(err.to_string()));
    }
    serde_json::from_slice(body).map_err(|err| ApiError::BadRequest(err.to_string()))
}

/// Go createSuppressionRequest (the wire body; the store record itself has
/// no serde defaults, so the handler fills them like Go).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateSuppressionRequest {
    #[serde(default)]
    suppression_id: String,
    #[serde(default)]
    target_kind: kura_evaluation::ProductResourceKind,
    #[serde(default)]
    target_id: String,
    #[serde(default)]
    target_source_ref: String,
    #[serde(default)]
    reason_code: String,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    created_by: String,
    #[serde(default)]
    expires_at: Option<DateTime<Utc>>,
}

/// Request body for POST /v1/evaluation/retention/apply.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApplyRetentionRequest {
    #[serde(default)]
    resource_kinds: Vec<kura_evaluation::ProductResourceKind>,
    #[serde(default)]
    dry_run: bool,
}

pub fn router() -> Router<AppState> {
    Router::new()
        // Evaluation replay ledger.
        .route(
            "/v1/evaluation/replay-candidates",
            get(list_replay_candidates).post(create_replay_candidate),
        )
        .route(
            "/v1/evaluation/replay-candidates/{candidate_id}",
            get(get_replay_candidate),
        )
        .route(
            "/v1/evaluation/replay-candidates/{candidate_id}/attempts",
            post(replay_candidate_attempts),
        )
        .route(
            "/v1/evaluation/replay-candidates/{candidate_id}/live-validations",
            post(replay_candidate_live_validations),
        )
        .route("/v1/evaluation/replay-attempts", get(list_replay_attempts))
        .route(
            "/v1/evaluation/replay-attempts/{attempt_id}",
            get(get_replay_attempt),
        )
        .route(
            "/v1/evaluation/replay-attempts/{attempt_id}/compare",
            post(replay_attempt_compare),
        )
        .route("/v1/evaluation/comparisons", get(list_comparisons))
        .route(
            "/v1/evaluation/comparisons/{comparison_id}",
            get(get_comparison),
        )
        .route("/v1/evaluation/fixtures", get(list_fixtures))
        // Evaluation product family (Go evaluation_product.go).
        .route("/v1/evaluation/discovery-policies", get(list_discovery_policies))
        .route(
            "/v1/evaluation/discovery-policies/{policy_id}",
            get(get_discovery_policy).put(upsert_discovery_policy),
        )
        .route(
            "/v1/evaluation/discovery-runs",
            get(list_discovery_runs).post(start_discovery_run),
        )
        .route(
            "/v1/evaluation/discovery-runs/{discovery_run_id}",
            get(get_discovery_run),
        )
        .route(
            "/v1/evaluation/discovered-candidates",
            get(list_discovered_candidates),
        )
        .route(
            "/v1/evaluation/discovered-candidates/{discovered_candidate_id}",
            get(get_discovered_candidate),
        )
        .route(
            "/v1/evaluation/discovered-candidates/{discovered_candidate_id}/product-fixtures",
            post(materialize_product_fixture),
        )
        .route("/v1/evaluation/product-fixtures", get(list_product_fixtures))
        .route(
            "/v1/evaluation/product-fixtures/{fixture_id}",
            get(get_product_fixture),
        )
        .route(
            "/v1/evaluation/product-fixtures/{fixture_id}/revisions",
            get(list_fixture_revisions).post(create_fixture_revision),
        )
        .route(
            "/v1/evaluation/product-fixtures/{fixture_id}/review",
            post(review_product_fixture_route),
        )
        .route(
            "/v1/evaluation/product-fixtures/{fixture_id}/suppress",
            post(suppress_product_fixture_route),
        )
        .route("/v1/evaluation/suppressions", post(create_suppression))
        .route(
            "/v1/evaluation/campaigns",
            get(list_replay_campaigns).post(create_replay_campaign_route),
        )
        .route(
            "/v1/evaluation/campaigns/{campaign_id}",
            get(get_replay_campaign),
        )
        .route(
            "/v1/evaluation/campaigns/{campaign_id}/items",
            get(list_campaign_items),
        )
        .route(
            "/v1/evaluation/campaigns/{campaign_id}/attempt-groups",
            get(list_campaign_attempt_groups),
        )
        .route(
            "/v1/evaluation/campaigns/{campaign_id}/{action}",
            post(campaign_transition_route),
        )
        .route(
            "/v1/evaluation/campaigns/{campaign_id}/tool-call-inspections",
            get(list_campaign_tool_call_inspections),
        )
        .route("/v1/evaluation/dashboard", get(list_dashboard_projections))
        .route(
            "/v1/evaluation/tool-call-inspections/{inspection_id}",
            get(get_tool_call_inspection),
        )
        .route(
            "/v1/evaluation/retention/apply",
            post(apply_evaluation_retention),
        )

        // Live validation collection + items.
        .route(
            "/v1/live-validations",
            get(list_live_validations).post(start_live_validation),
        )
        .route(
            "/v1/live-validations/support-matrix",
            get(live_validation_support_matrix),
        )
        .route(
            "/v1/live-validations/kill-switches",
            get(list_kill_switches).post(set_kill_switch),
        )
        .route(
            "/v1/live-validations/discord-smoke",
            get(live_validation_discord_smoke),
        )
        .route(
            "/v1/live-validations/discord-conformance",
            get(live_validation_connector_conformance),
        )
        .route(
            "/v1/live-validations/telegram-smoke",
            get(live_validation_telegram_smoke),
        )
        .route(
            "/v1/live-validations/telegram-conformance",
            get(live_validation_connector_conformance),
        )
        .route(
            "/v1/live-validations/slack-smoke",
            get(live_validation_slack_smoke),
        )
        .route(
            "/v1/live-validations/slack-conformance",
            get(live_validation_connector_conformance),
        )
        .route(
            "/v1/live-validations/matrix-smoke",
            get(live_validation_matrix_smoke).post(record_matrix_smoke),
        )
        .route(
            "/v1/live-validations/matrix-conformance",
            get(live_validation_connector_conformance),
        )
        .route(
            "/v1/live-validations/{validation_id}",
            get(get_live_validation),
        )
        .route(
            "/v1/live-validations/{validation_id}/ledger",
            get(live_validation_ledger),
        )
        .route(
            "/v1/live-validations/{validation_id}/abort",
            post(live_validation_abort),
        )
        .route(
            "/v1/live-validations/{validation_id}/retention",
            get(live_validation_retention),
        )
        .route(
            "/v1/live-validations/{validation_id}/compare",
            post(live_validation_compare),
        )
        .route(
            "/v1/live-validations/{validation_id}/reconciliations/{ambiguous_commit_id}/resolve",
            post(live_validation_reconcile),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use axum::body::to_bytes;
    use axum::http::Request as HttpRequest;
    use kura_events::Bus;
    use kura_identity::{
        LifecycleStatus, Role, TenantContext as IdentityTenantContext, permissions_for_role,
    };
    use kura_store::SQLiteStore;
    use parking_lot::Mutex;
    use tower::ServiceExt;
    use uuid::Uuid;

    /// Fixed timestamp used by the live-validation crate tests: 2026-04-29T10:00:00Z.
    fn fixed_now() -> DateTime<Utc> {
        DateTime::<Utc>::from_timestamp_secs(1_777_456_800).expect("fixed clock")
    }

    fn test_config() -> kura_config::Config {
        kura_config::Config {
            project_root: String::new(),
            environment: kura_config::Environment::Test,
            bind_addr: "127.0.0.1:19192".to_string(),
            data_dir: "/tmp/kura-api-test".to_string(),
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

    fn fresh_store() -> Arc<Mutex<SQLiteStore>> {
        let dir = std::env::temp_dir().join(format!("kura-api-evaluation2-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        Arc::new(Mutex::new(
            SQLiteStore::new(dir.to_str().expect("path")).expect("store"),
        ))
    }

    struct Harness {
        state: AppState,
        store: Arc<Mutex<SQLiteStore>>,
        bus: Arc<Bus>,
    }

    fn harness(
        evaluation: Option<Arc<kura_evaluation::Manager>>,
        live_validation: Option<Arc<kura_livevalidation::Manager>>,
    ) -> Harness {
        let store = fresh_store();
        let bus = Arc::new(Bus::new());
        let mut state = AppState::new(test_config(), bus.clone(), Arc::clone(&store));
        state.evaluation = evaluation;
        state.live_validation = live_validation;
        Harness { state, store, bus }
    }

    /// Adapter mapping the evaluation manager's Store trait onto the
    /// SQLiteStore replay-ledger DAOs. The production adapter lives in the
    /// app-wiring layer; this one exists so handler tests can run.
    #[derive(Clone)]
    struct SqliteEvaluationStore {
        store: Arc<Mutex<SQLiteStore>>,
    }

    fn store_err(err: String) -> EvaluationError {
        EvaluationError::Store(err)
    }

    impl kura_evaluation::manager::Store for SqliteEvaluationStore {
        fn upsert_replay_candidate(&self, item: ReplayCandidate) -> Result<(), EvaluationError> {
            self.store.lock().upsert_replay_candidate(&item).map_err(store_err)
        }
        fn list_replay_candidates(
            &self,
            filter: &CandidateFilter,
        ) -> Result<Vec<ReplayCandidate>, EvaluationError> {
            self.store.lock().list_replay_candidates(filter).map_err(store_err)
        }
        fn get_replay_candidate(
            &self,
            environment_scope: &str,
            candidate_id: &str,
        ) -> Result<Option<ReplayCandidate>, EvaluationError> {
            self.store
                .lock()
                .get_replay_candidate(environment_scope, candidate_id)
                .map_err(store_err)
        }
        fn upsert_replay_attempt(&self, item: ReplayAttempt) -> Result<(), EvaluationError> {
            self.store.lock().upsert_replay_attempt(&item).map_err(store_err)
        }
        fn list_replay_attempts(
            &self,
            filter: &kura_evaluation::AttemptFilter,
        ) -> Result<Vec<ReplayAttempt>, EvaluationError> {
            self.store.lock().list_replay_attempts(filter).map_err(store_err)
        }
        fn get_replay_attempt(
            &self,
            environment_scope: &str,
            attempt_id: &str,
        ) -> Result<Option<ReplayAttempt>, EvaluationError> {
            self.store
                .lock()
                .get_replay_attempt(environment_scope, attempt_id)
                .map_err(store_err)
        }
        fn upsert_comparison_result(&self, item: ComparisonResult) -> Result<(), EvaluationError> {
            self.store.lock().upsert_comparison_result(&item).map_err(store_err)
        }
        fn list_comparison_results(
            &self,
            filter: &ComparisonFilter,
        ) -> Result<Vec<ComparisonResult>, EvaluationError> {
            self.store.lock().list_comparison_results(filter).map_err(store_err)
        }
        fn get_comparison_result(
            &self,
            environment_scope: &str,
            comparison_id: &str,
        ) -> Result<Option<ComparisonResult>, EvaluationError> {
            self.store
                .lock()
                .get_comparison_result(environment_scope, comparison_id)
                .map_err(store_err)
        }
        fn upsert_regression_fixture(&self, item: RegressionFixture) -> Result<(), EvaluationError> {
            self.store
                .lock()
                .upsert_regression_fixture(&item)
                .map_err(store_err)
        }
        fn list_regression_fixtures(
            &self,
            filter: &FixtureFilter,
        ) -> Result<Vec<RegressionFixture>, EvaluationError> {
            self.store.lock().list_regression_fixtures(filter).map_err(store_err)
        }
    }

    fn evaluation_manager(store: Arc<Mutex<SQLiteStore>>) -> Arc<kura_evaluation::Manager> {
        Arc::new(kura_evaluation::Manager::new(kura_evaluation::Dependencies {
            environment_scope: "test".to_string(),
            store: Some(Arc::new(SqliteEvaluationStore { store })),
            fixtures_dir: String::new(),
            runtime_recorder: None,
            billing: None,
            hosted_billing: false,
            clock: Some(Arc::new(fixed_now) as Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>),
        }))
    }

    fn live_validation_manager(hosted_billing: bool) -> Arc<kura_livevalidation::Manager> {
        Arc::new(kura_livevalidation::Manager::new(
            kura_livevalidation::Dependencies {
                environment_scope: "test".to_string(),
                // NOTE: the store stays None in these tests — kura-livevalidation
                // does not re-export LedgerOutcome, so its async Store trait is
                // not implementable outside the crate (reported).
                store: None,
                enabled: true,
                billing: None,
                hosted_billing,
                clock: Some(Arc::new(fixed_now) as kura_livevalidation::Clock),
                ledger_event_sink: None,
                candidate_tool_class_resolver: None,
            },
        ))
    }

    fn request(method: &str, uri: &str, body: Option<&str>) -> HttpRequest<axum::body::Body> {
        HttpRequest::builder()
            .method(method)
            .uri(uri)
            .body(match body {
                Some(body) => axum::body::Body::from(body.to_string()),
                None => axum::body::Body::empty(),
            })
            .expect("request")
    }

    async fn send(
        app: &axum::Router,
        req: HttpRequest<axum::body::Body>,
    ) -> (StatusCode, serde_json::Value) {
        let response = app.clone().oneshot(req).await.expect("oneshot");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.expect("body");
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    /// Runs the request with a resolved tenant context installed in the
    /// kura_identity tenantctx task-local (the live-validation manager reads it).
    async fn send_with_tenant(
        app: &axum::Router,
        req: HttpRequest<axum::body::Body>,
        tenant: IdentityTenantContext,
    ) -> (StatusCode, serde_json::Value) {
        let response = kura_identity::tenantctx::scope(tenant, app.clone().oneshot(req))
            .await
            .expect("oneshot");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.expect("body");
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    /// Attaches the TenantContext extension the smoke/conformance handlers read.
    fn with_tenant_extension(
        mut req: HttpRequest<axum::body::Body>,
        tenant: IdentityTenantContext,
    ) -> HttpRequest<axum::body::Body> {
        req.extensions_mut().insert(crate::middleware::TenantContext(tenant));
        req
    }

    fn operator_context() -> IdentityTenantContext {
        IdentityTenantContext {
            tenant_id: "ten_1".to_string(),
            principal_id: "prn_operator".to_string(),
            role: Some(Role::Operator),
            permissions: permissions_for_role(Role::Operator, LifecycleStatus::Active),
            ..IdentityTenantContext::default()
        }
    }

    fn admin_context() -> IdentityTenantContext {
        IdentityTenantContext {
            tenant_id: "ten_1".to_string(),
            principal_id: "prn_admin".to_string(),
            role: Some(Role::Admin),
            permissions: permissions_for_role(Role::Admin, LifecycleStatus::Active),
            ..IdentityTenantContext::default()
        }
    }

    fn viewer_context() -> IdentityTenantContext {
        IdentityTenantContext {
            tenant_id: "ten_1".to_string(),
            principal_id: "prn_viewer".to_string(),
            role: Some(Role::Viewer),
            permissions: permissions_for_role(Role::Viewer, LifecycleStatus::Active),
            ..IdentityTenantContext::default()
        }
    }

    /// Tenant context carrying the smoke-recorder permissions the Go tests use.
    fn smoke_tenant_context(tenant_id: &str, principal_id: &str) -> IdentityTenantContext {
        IdentityTenantContext {
            tenant_id: tenant_id.to_string(),
            principal_id: principal_id.to_string(),
            permissions: vec![
                kura_identity::Permission::LiveValidationExecute,
                kura_identity::Permission::ConnectorsManage,
                kura_identity::Permission::CredentialsInspect,
            ],
            ..IdentityTenantContext::default()
        }
    }

    fn seed_candidate(store: &Arc<Mutex<SQLiteStore>>, candidate: &ReplayCandidate) {
        store.lock().upsert_replay_candidate(candidate).expect("seed candidate");
    }

    fn curated_candidate(candidate_id: &str, tool_classes: Vec<String>) -> ReplayCandidate {
        ReplayCandidate {
            candidate_id: candidate_id.to_string(),
            candidate_kind: kura_evaluation::CandidateKind::CuratedWork,
            display_name: "candidate".to_string(),
            source_kind: kura_evaluation::SourceKind::Run,
            source_id: "run_1".to_string(),
            source_refs: vec![kura_evaluation::SourceRef {
                kind: kura_evaluation::SourceKind::Run,
                id: "run_1".to_string(),
                route: String::new(),
            }],
            tool_classes,
            environment_scope: "test".to_string(),
            readiness_status: kura_evaluation::ReadinessStatus::FullyReplayable,
            default_replay_mode: ReplayMode::NonLive,
            created_at: fixed_now(),
            updated_at: fixed_now(),
            ..ReplayCandidate::default()
        }
    }

    fn live_validation_start_body(validation_id: &str, tool_class: &str) -> String {
        serde_json::json!({
            "validationId": validation_id,
            "candidateId": "candidate_1",
            "candidateToolClasses": [tool_class],
            "requestedScope": {
                "scopeId": format!("scope_{validation_id}"),
                "includedToolClasses": [tool_class],
                "approvalMode": "scope_level",
                "declaredBy": "prn_operator",
                "declaredAt": "2026-04-29T10:00:00Z",
            }
        })
        .to_string()
    }

    // Port of Go TestEvaluationRoutesLaunchReplayAndCompare (without the
    // runtime-recorder run/workflow assertions — no recorder is wired here).
    #[tokio::test]
    async fn replay_candidates_crud_attempt_compare_and_events() {
        let store = fresh_store();
        seed_candidate(&store, &curated_candidate("candidate_curated", vec![]));
        store
            .lock()
            .upsert_regression_fixture(&RegressionFixture {
                fixture_id: "fixture_schedule_1".to_string(),
                display_name: "Schedule fixture".to_string(),
                domain_class: kura_evaluation::FixtureDomainClass::Schedule,
                source_refs: vec![],
                captured_evidence_refs: vec![],
                assumptions: vec![],
                limitations: vec![],
                expected_replay_mode: ReplayMode::NonLive,
                expected_comparison_summary: kura_evaluation::PlaneSummaries::default(),
                candidate_id: "candidate_fixture".to_string(),
                environment_scope: "test".to_string(),
                created_at: fixed_now(),
                updated_at: fixed_now(),
                ..RegressionFixture::default()
            })
            .expect("seed fixture");
        let h = harness(Some(evaluation_manager(Arc::clone(&store))), None);
        let app = crate::routes::router(h.state.clone());

        // GET list -> the seeded candidate, with the environment scope.
        let (status, json) = send(&app, request("GET", "/v1/evaluation/replay-candidates", None)).await;
        assert_eq!(status, StatusCode::OK, "body: {json}");
        assert_eq!(json["environmentScope"], "test");
        assert_eq!(json["items"].as_array().map(|a| a.len()).unwrap_or(0), 1);
        assert_eq!(json["items"][0]["candidateId"], "candidate_curated");

        // POST curated candidate -> 201.
        let (status, json) = send(
            &app,
            request(
                "POST",
                "/v1/evaluation/replay-candidates",
                Some(
                    r#"{"candidateId":"candidate_api_1","candidateKind":"curated_work","displayName":"Curated Run","sourceKind":"run","sourceId":"run_a","sourceRefs":[{"kind":"run","id":"run_a","route":"/v1/runs/run_a"}],"environmentScope":"test","readinessStatus":"partially_replayable","readinessReasons":["curated run has captured summaries"],"limitations":["evidence-only replay"],"defaultReplayMode":"non_live","expectedComparisonSummary":{"runtime":"runtime captured","policy":"policy captured","evidence":"evidence captured"}}"#,
                ),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "create body: {json}");
        assert_eq!(json["candidateId"], "candidate_api_1");
        assert_eq!(json["candidateKind"], "curated_work");
        assert_eq!(json["environmentScope"], "test");

        // POST missing source refs -> 400 (manager validation).
        let (status, json) = send(
            &app,
            request(
                "POST",
                "/v1/evaluation/replay-candidates",
                Some(r#"{"candidateId":"candidate_missing","candidateKind":"curated_work","displayName":"Missing Source"}"#),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "body: {json}");
        assert_eq!(json["code"], "bad_request");

        // POST fixture-kind candidate -> 400 (managed by repo fixtures).
        let (status, json) = send(
            &app,
            request(
                "POST",
                "/v1/evaluation/replay-candidates",
                Some(
                    r#"{"candidateId":"candidate_api_fixture","candidateKind":"fixture","displayName":"API Fixture","sourceKind":"fixture","sourceId":"fixture_api","sourceRefs":[{"kind":"fixture","id":"fixture_api"}],"environmentScope":"test","readinessStatus":"fully_replayable","readinessReasons":[],"limitations":[],"defaultReplayMode":"non_live"}"#,
                ),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "body: {json}");
        assert_eq!(json["message"], "fixture replay candidates are managed by repo fixtures");

        // GET fixtures -> the seeded fixture.
        let (status, json) = send(&app, request("GET", "/v1/evaluation/fixtures", None)).await;
        assert_eq!(status, StatusCode::OK, "body: {json}");
        assert_eq!(json["environmentScope"], "test");
        assert_eq!(json["items"].as_array().map(|a| a.len()).unwrap_or(0), 1);
        assert_eq!(json["items"][0]["fixtureId"], "fixture_schedule_1");

        // GET candidate detail + 404 for a missing one.
        let (status, json) = send(
            &app,
            request("GET", "/v1/evaluation/replay-candidates/candidate_curated", None),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {json}");
        assert_eq!(json["candidateId"], "candidate_curated");
        let (status, json) = send(
            &app,
            request("GET", "/v1/evaluation/replay-candidates/does-not-exist", None),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "body: {json}");
        assert_eq!(json["message"], "replay candidate not found");

        // POST attempt (empty body) -> 202 completed non-live.
        let (status, json) = send(
            &app,
            request("POST", "/v1/evaluation/replay-candidates/candidate_curated/attempts", None),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "attempt body: {json}");
        assert_eq!(json["mode"], "non_live");
        assert_eq!(json["status"], "completed");
        let attempt_id = json["attemptId"].as_str().expect("attemptId").to_string();

        // POST attempt with live-validation mode -> 400 bypass rejection.
        let (status, json) = send(
            &app,
            request(
                "POST",
                "/v1/evaluation/replay-candidates/candidate_curated/attempts",
                Some(r#"{"mode":"live_validation"}"#),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "body: {json}");
        assert_eq!(json["message"], "live validation attempts must use /v1/live-validations");

        // GET attempts list + 404 for a missing attempt.
        let (status, json) = send(&app, request("GET", "/v1/evaluation/replay-attempts", None)).await;
        assert_eq!(status, StatusCode::OK, "body: {json}");
        assert_eq!(json["items"].as_array().map(|a| a.len()).unwrap_or(0), 1);
        let (status, json) = send(
            &app,
            request("GET", "/v1/evaluation/replay-attempts/does-not-exist", None),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "body: {json}");
        assert_eq!(json["message"], "replay attempt not found");

        // POST compare -> 201 matched.
        let (status, json) = send(
            &app,
            request("POST", &format!("/v1/evaluation/replay-attempts/{attempt_id}/compare"), None),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "compare body: {json}");
        assert_eq!(json["terminalStatus"], "matched");
        let comparison_id = json["comparisonId"].as_str().expect("comparisonId").to_string();

        // GET comparisons list + detail.
        let (status, json) = send(&app, request("GET", "/v1/evaluation/comparisons", None)).await;
        assert_eq!(status, StatusCode::OK, "body: {json}");
        assert_eq!(json["environmentScope"], "test");
        assert_eq!(json["items"].as_array().map(|a| a.len()).unwrap_or(0), 1);
        let (status, json) = send(
            &app,
            request("GET", &format!("/v1/evaluation/comparisons/{comparison_id}"), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {json}");
        assert_eq!(json["comparisonId"], comparison_id);
        let (status, json) = send(
            &app,
            request("GET", "/v1/evaluation/comparisons/does-not-exist", None),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "body: {json}");
        assert_eq!(json["message"], "comparison not found");

        // The replay_started / replay_completed / comparison_completed events fired.
        let events = h
            .bus
            .list(&kura_events::Filter {
                category: "evaluation".to_string(),
                ..Default::default()
            });
        let names: Vec<String> = events.iter().map(|event| event.name.clone()).collect();
        for expected in [
            "evaluation.replay_started",
            "evaluation.replay_completed",
            "evaluation.comparison_completed",
        ] {
            assert!(
                names.iter().any(|name| name == expected),
                "expected {expected} in {names:?}"
            );
        }
    }

    #[tokio::test]
    async fn unconfigured_managers_return_500() {
        let h = harness(None, None);
        let app = crate::routes::router(h.state.clone());
        let (status, json) = send(&app, request("GET", "/v1/evaluation/replay-candidates", None)).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "body: {json}");
        assert_eq!(json["message"], "evaluation manager is not configured");
        let (status, json) = send(&app, request("GET", "/v1/live-validations", None)).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "body: {json}");
        assert_eq!(json["message"], "live validation manager is not configured");
    }

    // Port of Go TestLiveValidationRouteStartDenialsAndAwaitingApproval.
    #[tokio::test]
    async fn live_validation_start_denials_and_awaiting_approval() {
        let cases: Vec<(
            &str,
            IdentityTenantContext,
            Arc<kura_livevalidation::Manager>,
            String,
            StatusCode,
            Option<&str>,
            Option<&str>,
        )> = vec![
            (
                "permission",
                viewer_context(),
                live_validation_manager(false),
                live_validation_start_body("lv_permission", "daemon.inspection.read"),
                StatusCode::CONFLICT,
                Some("permission"),
                None,
            ),
            (
                "quota unavailable",
                operator_context(),
                live_validation_manager(true),
                live_validation_start_body("lv_quota", "daemon.inspection.read"),
                StatusCode::CONFLICT,
                Some("quota"),
                None,
            ),
            (
                "support matrix",
                operator_context(),
                live_validation_manager(false),
                live_validation_start_body("lv_support", "mcp.tool_call"),
                StatusCode::CONFLICT,
                Some("support_matrix"),
                None,
            ),
            (
                "awaiting approval",
                operator_context(),
                live_validation_manager(false),
                live_validation_start_body("lv_approval", "daemon.inspection.read"),
                StatusCode::ACCEPTED,
                None,
                Some("awaiting_approval"),
            ),
        ];
        for (name, tenant, manager, body, want_status, want_gate, want_state) in cases {
            let h = harness(None, Some(manager));
            let (status, json) = send_with_tenant(
                &crate::routes::router(h.state.clone()),
                request("POST", "/v1/live-validations", Some(&body)),
                tenant,
            )
            .await;
            assert_eq!(status, want_status, "{name} body: {json}");
            if let Some(gate) = want_gate {
                assert_eq!(json["denials"][0]["gate"], gate, "{name}");
            }
            if let Some(state) = want_state {
                assert_eq!(json["attempt"]["status"], state, "{name}");
            }
        }
    }

    #[tokio::test]
    async fn live_validation_support_matrix_route() {
        let h = harness(None, Some(live_validation_manager(false)));
        let (status, json) = send(
            &crate::routes::router(h.state.clone()),
            request("GET", "/v1/live-validations/support-matrix", None),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {json}");
        assert_eq!(json["version"], "v1");
        assert_eq!(json["environmentScope"], "test");
        let items = json["items"].as_array().expect("items");
        assert!(!items.is_empty());
        assert!(
            items.iter().any(|row| {
                row["toolClass"] == "mcp.tool_call" && row["safetyClass"] == "unsupported"
            }),
            "expected unsupported MCP row in {items:?}"
        );
    }

    // Port of Go TestLiveValidationKillSwitchSetListAndBlocksStart (the set +
    // list legs; the block-start leg needs a store-backed manager).
    #[tokio::test]
    async fn live_validation_kill_switches_set_and_list() {
        let h = harness(None, Some(live_validation_manager(false)));
        let app = crate::routes::router(h.state.clone());
        let (status, json) = send_with_tenant(
            &app,
            request(
                "POST",
                "/v1/live-validations/kill-switches",
                Some(r#"{"scope":"tenant","enabled":true,"reason":"containment"}"#),
            ),
            admin_context(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {json}");
        assert_eq!(json["enabled"], true);
        assert_eq!(json["tenantId"], "ten_1");
        assert_eq!(json["scope"], "tenant");
        let (status, json) = send_with_tenant(
            &app,
            request("GET", "/v1/live-validations/kill-switches", None),
            admin_context(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {json}");
        assert_eq!(json["tenantId"], "ten_1");
    }

    // Port of the reconcile legs of Go TestLiveValidationLedgerReconciliationRetentionAndComparisonRoutes.
    #[tokio::test]
    async fn live_validation_reconcile_requires_authority() {
        let h = harness(None, Some(live_validation_manager(false)));
        let app = crate::routes::router(h.state.clone());
        let body = r#"{"resolution":"confirmed_committed","reason":"provider checked"}"#;
        let (status, json) = send_with_tenant(
            &app,
            request(
                "POST",
                "/v1/live-validations/lv_1/reconciliations/amb_1/resolve",
                Some(body),
            ),
            viewer_context(),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "viewer body: {json}");
        assert_eq!(json["code"], "forbidden");
        let (status, json) = send_with_tenant(
            &app,
            request(
                "POST",
                "/v1/live-validations/lv_1/reconciliations/amb_1/resolve",
                Some(body),
            ),
            admin_context(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "admin body: {json}");
        assert_eq!(json["ambiguousCommitId"], "amb_1");
        assert_eq!(json["resolution"], "confirmed_committed");
        assert_eq!(json["resolvedBy"], "prn_admin");
    }

    #[tokio::test]
    async fn live_validation_list_carries_resolved_tenant() {
        let h = harness(None, Some(live_validation_manager(false)));
        let (status, json) = send_with_tenant(
            &crate::routes::router(h.state.clone()),
            request("GET", "/v1/live-validations", None),
            operator_context(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {json}");
        assert_eq!(json["tenantId"], "ten_1");
        assert_eq!(json["environmentScope"], "test");
        assert_eq!(json["items"].as_array().map(|a| a.len()).unwrap_or(0), 0);
    }

    // Port of Go TestReplayCandidateLiveValidationRouteHandsOffToLiveValidationManager.
    #[tokio::test]
    async fn replay_candidate_live_validation_hands_off() {
        let store = fresh_store();
        seed_candidate(
            &store,
            &curated_candidate("candidate_live", vec!["daemon.inspection.read".to_string()]),
        );
        let h = harness(
            Some(evaluation_manager(Arc::clone(&store))),
            Some(live_validation_manager(false)),
        );
        let body = serde_json::json!({
            "validationId": "lv_nested",
            "candidateId": "candidate_live",
            "candidateToolClasses": ["daemon.inspection.read"],
            "requestedScope": {
                "scopeId": "scope_lv_nested",
                "includedToolClasses": ["daemon.inspection.read"],
                "approvalMode": "scope_level",
                "declaredBy": "prn_operator",
                "declaredAt": "2026-04-29T09:59:00Z",
            },
            "freshApprovals": [{
                "approvalId": "approval_1",
                "validationId": "lv_nested",
                "approvalTarget": "scope",
                "toolClass": "daemon.inspection.read",
                "safetyClass": "read_only",
                "approvedScope": "scope_lv_nested",
                "status": "approved",
                "requestedBy": "prn_operator",
                "requestedAt": "2026-04-29T09:59:00Z",
                "resolvedAt": "2026-04-29T09:59:30Z",
            }],
        })
        .to_string();
        let (status, json) = send_with_tenant(
            &crate::routes::router(h.state.clone()),
            request(
                "POST",
                "/v1/evaluation/replay-candidates/candidate_live/live-validations",
                Some(&body),
            ),
            operator_context(),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "body: {json}");
        assert_eq!(json["attempt"]["candidateId"], "candidate_live");
        assert_eq!(json["attempt"]["status"], "running");
    }

    // Port of Go TestReplayCandidateLiveValidationRouteDerivesCandidateToolClasses.
    #[tokio::test]
    async fn replay_candidate_live_validation_derives_candidate_tool_classes() {
        let store = fresh_store();
        seed_candidate(
            &store,
            &curated_candidate(
                "candidate_mixed",
                vec![
                    "daemon.inspection.read".to_string(),
                    "mcp.tool_call".to_string(),
                ],
            ),
        );
        let h = harness(
            Some(evaluation_manager(Arc::clone(&store))),
            Some(live_validation_manager(false)),
        );
        // No candidateToolClasses in the request: the route derives them from
        // the candidate, and the unsupported mcp.tool_call class blocks.
        let body = serde_json::json!({
            "validationId": "lv_mixed",
            "candidateId": "candidate_mixed",
            "requestedScope": {
                "scopeId": "scope_lv_mixed",
                "includedToolClasses": ["daemon.inspection.read"],
                "approvalMode": "scope_level",
                "declaredBy": "prn_operator",
                "declaredAt": "2026-04-29T10:00:00Z",
            },
        })
        .to_string();
        let (status, json) = send_with_tenant(
            &crate::routes::router(h.state.clone()),
            request(
                "POST",
                "/v1/evaluation/replay-candidates/candidate_mixed/live-validations",
                Some(&body),
            ),
            operator_context(),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "body: {json}");
        assert_eq!(json["denials"][0]["gate"], "support_matrix");
        assert_eq!(json["denials"][0]["reference"], "mcp.tool_call");
    }

    #[tokio::test]
    async fn live_validation_smoke_requires_tenant_and_permission() {
        let h = harness(None, Some(live_validation_manager(false)));
        let app = crate::routes::router(h.state.clone());
        // No tenant context -> 403 missing_tenant.
        let (status, json) = send(
            &app,
            request("GET", "/v1/live-validations/discord-smoke?connectorId=discord-main", None),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "body: {json}");
        assert_eq!(json["reasonCode"], "credential_denied:missing_tenant");
        assert_eq!(json["error"], "credential_access_denied");
        // Viewer without credential-inspection authority -> 403 missing_permission.
        let (status, json) = send(
            &app,
            with_tenant_extension(
                request("GET", "/v1/live-validations/discord-smoke?connectorId=discord-main", None),
                viewer_context(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "body: {json}");
        assert_eq!(json["reasonCode"], "credential_denied:missing_permission");
        // Authorized but no evidence -> 404.
        let (status, json) = send(
            &app,
            with_tenant_extension(
                request("GET", "/v1/live-validations/discord-smoke?connectorId=discord-main", None),
                smoke_tenant_context("ten_discord", "prn_operator"),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "body: {json}");
    }

    #[tokio::test]
    async fn live_validation_matrix_smoke_records_structured_skip_evidence() {
        let h = harness(None, Some(live_validation_manager(false)));
        let app = crate::routes::router(h.state.clone());
        let tenant = smoke_tenant_context("ten_matrix", "prn_operator");
        // validatedAt must keep the 90-day retention window in the future
        // relative to the test clock.
        let post_body = r#"{"connectorId":"matrix-main","homeserverBindingId":"matrix_hs_1","status":"skipped","authorizationMode":"unavailable","owner":"operator","reason":"safe Matrix credentials unavailable","validatedAt":"2026-09-01T14:00:00Z","safeEvidence":{"policy":"structured_skip"}}"#;
        let (status, json) = send(
            &app,
            with_tenant_extension(request("POST", "/v1/live-validations/matrix-smoke", Some(post_body)), tenant.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "post body: {json}");
        assert_eq!(json["status"], "skipped");
        assert_eq!(json["authorizationMode"], "unavailable");
        assert_eq!(json["smokeEvidenceId"], "matrix_smoke_matrix-main");
        let (status, json) = send(
            &app,
            with_tenant_extension(
                request("GET", "/v1/live-validations/matrix-smoke?connectorId=matrix-main", None),
                tenant,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "get body: {json}");
        assert_eq!(json["status"], "skipped");
        assert_eq!(json["authorizationMode"], "unavailable");
        assert!(!json.to_string().contains("accessToken"));
    }

    #[tokio::test]
    async fn live_validation_slack_smoke_projects_tenant_safe_evidence() {
        let h = harness(None, Some(live_validation_manager(false)));
        let app = crate::routes::router(h.state.clone());
        let tenant = smoke_tenant_context("ten_slack", "prn_operator");
        // The POST recorder is not ported (kura-api has no kura-slack dep), so
        // seed the evidence through the store and verify the GET projection.
        {
            let store = h.store.lock();
            store
                .save_slack_smoke_evidence(&kura_store::SlackSmokeEvidenceRecord {
                    smoke_evidence_id: "slack_smoke_slack-main".to_string(),
                    tenant_id: "ten_slack".to_string(),
                    connector_id: "slack-main".to_string(),
                    workspace_binding_id: "workspace_binding_redacted".to_string(),
                    status: "passed".to_string(),
                    authorization_mode: "fake_oauth".to_string(),
                    owner: "operator".to_string(),
                    reason: "healthy".to_string(),
                    remaining_risk: String::new(),
                    validated_at: Utc::now() + chrono::Duration::days(30),
                    retention_expires_at: Utc::now() + chrono::Duration::days(120),
                    redaction_status: "redacted".to_string(),
                    safe_evidence: HashMap::from([("mode".to_string(), "fake".to_string())]),
                })
                .expect("seed slack smoke evidence");
        }
        let (status, json) = send(
            &app,
            with_tenant_extension(
                request("GET", "/v1/live-validations/slack-smoke?connectorId=slack-main", None),
                tenant,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {json}");
        assert_eq!(json["status"], "passed");
        assert_eq!(json["authorizationMode"], "fake_oauth");
        let raw = json.to_string();
        assert!(!raw.contains("xoxb-"), "leaked credential evidence: {raw}");
        assert!(!raw.contains("secret"), "leaked secret evidence: {raw}");
    }


    fn product_tenant_context(tenant_id: &str, permissions: Vec<Permission>) -> IdentityTenantContext {
        IdentityTenantContext {
            tenant_id: tenant_id.to_string(),
            principal_id: format!("prn_{tenant_id}"),
            permissions,
            ..Default::default()
        }
    }

    // Port of TestEvaluationProductRoutesListTenantScopedPoliciesWithoutManager
    // + TestEvaluationProductDiscoveryAPIRoutes.
    #[tokio::test]
    async fn evaluation_product_discovery_policies_and_runs() {
        let h = harness(None, None);
        let app = crate::routes::router(h.state.clone());
        let tenant = product_tenant_context("ten_api", Vec::new());
        let now = chrono::Utc::now();
        {
            let store = h.store.lock();
            store
                .upsert_discovery_policy(kura_evaluation::DiscoveryPolicy {
                    policy_id: "policy_api".to_string(),
                    tenant_id: "ten_api".to_string(),
                    enabled: true,
                    source_kinds: vec![kura_evaluation::SourceKind::Run],
                    window_start: now - chrono::Duration::hours(1),
                    window_end: now,
                    max_inspected_records: 10,
                    max_emitted_candidates: 2,
                    cost_budget: 5,
                    created_at: now,
                    updated_at: now,
                    ..Default::default()
                })
                .expect("upsert policy");
        }

        // List tenant-scoped policies (no permission gate in Go).
        let (status, json) = send(
            &app,
            with_tenant_extension(request("GET", "/v1/evaluation/discovery-policies", None), tenant.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "list policies: {json}");
        assert_eq!(json["tenantId"], "ten_api");
        assert_eq!(json["items"][0]["policyId"], "policy_api");

        // PUT a policy by id.
        let put_body = serde_json::json!({
            "enabled": true,
            "sourceKinds": ["run"],
            "windowStart": (now - chrono::Duration::hours(1)).to_rfc3339(),
            "windowEnd": now.to_rfc3339(),
            "maxInspectedRecords": 10,
            "maxEmittedCandidates": 2,
            "costBudget": 5,
        })
        .to_string();
        let (status, json) = send(
            &app,
            with_tenant_extension(
                request("PUT", "/v1/evaluation/discovery-policies/policy_api_1", Some(&put_body)),
                tenant.clone(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "put policy: {json}");
        assert_eq!(json["policyId"], "policy_api_1");

        // GET the policy.
        let (status, json) = send(
            &app,
            with_tenant_extension(
                request("GET", "/v1/evaluation/discovery-policies/policy_api_1", None),
                tenant.clone(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "get policy: {json}");
        assert_eq!(json["policyId"], "policy_api_1");

        // Start a discovery run referencing the policy (202).
        let (status, json) = send(
            &app,
            with_tenant_extension(
                request(
                    "POST",
                    "/v1/evaluation/discovery-runs",
                    Some(r#"{"policyId":"policy_api_1","idempotencyKey":"idem_api_1"}"#),
                ),
                tenant.clone(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "start run: {json}");
        assert_eq!(json["idempotencyKey"], "idem_api_1");
        let run_id = json["discoveryRunId"].as_str().expect("run id").to_string();

        // Seed a discovered candidate with evidence and fetch it.
        {
            let store = h.store.lock();
            store
                .save_discovered_candidate(
                    kura_evaluation::DiscoveredCandidate {
                        discovered_candidate_id: "candidate_api_1".to_string(),
                        tenant_id: "ten_api".to_string(),
                        discovery_run_id: run_id,
                        source_kind: kura_evaluation::SourceKind::Run,
                        source_id: "run_source_1".to_string(),
                        score: 0.9,
                        score_band: kura_evaluation::ScoreBand::High,
                        redaction_status: kura_evaluation::RedactionStatus::Redacted,
                        readiness_status: kura_evaluation::ReadinessStatus::FullyReplayable,
                        suppression_state: kura_evaluation::SuppressionState::None,
                        retention_state: kura_evaluation::RetentionState::Active,
                        created_at: now,
                        updated_at: now,
                        ..Default::default()
                    },
                    kura_evaluation::CandidateEvidence {
                        evidence_id: "evidence_api_1".to_string(),
                        tenant_id: "ten_api".to_string(),
                        discovered_candidate_id: "candidate_api_1".to_string(),
                        retention_state: kura_evaluation::RetentionState::Active,
                        created_at: now,
                        ..Default::default()
                    },
                )
                .expect("save candidate");
        }
        let (status, json) = send(
            &app,
            with_tenant_extension(
                request("GET", "/v1/evaluation/discovered-candidates/candidate_api_1", None),
                tenant.clone(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "get candidate: {json}");
        assert_eq!(json["discoveredCandidateId"], "candidate_api_1");

        // Create a suppression (201).
        let (status, json) = send(
            &app,
            with_tenant_extension(
                request(
                    "POST",
                    "/v1/evaluation/suppressions",
                    Some(r#"{"suppressionId":"suppression_api_1","targetKind":"discovered_candidate","targetId":"candidate_api_1","reasonCode":"operator_hidden","reason":"hidden in test"}"#),
                ),
                tenant.clone(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "suppression: {json}");
        assert_eq!(json["suppressionId"], "suppression_api_1");
        assert_eq!(json["active"], true);
    }

    // Port of TestEvaluationProductFixturePermissionDenialsAndLifecycleRoutes.
    #[tokio::test]
    async fn evaluation_product_fixture_permissions_and_lifecycle() {
        let h = harness(None, None);
        let app = crate::routes::router(h.state.clone());
        let now = chrono::Utc::now();
        let admin = product_tenant_context(
            "ten_fixture_api",
            vec![
                Permission::EvaluationFixtureRead,
                Permission::EvaluationFixtureManage,
                Permission::EvaluationFixtureReview,
                Permission::EvaluationFixtureSuppress,
            ],
        );
        let viewer = product_tenant_context("ten_fixture_api", vec![Permission::EvaluationFixtureRead]);
        {
            let store = h.store.lock();
            store
                .save_discovery_run(kura_evaluation::DiscoveryRun {
                    discovery_run_id: "discovery_run_fixture_api".to_string(),
                    tenant_id: "ten_fixture_api".to_string(),
                    status: kura_evaluation::ProductLifecycleStatus::Completed,
                    source_kinds: vec![kura_evaluation::SourceKind::Run],
                    window_start: now - chrono::Duration::hours(1),
                    window_end: now,
                    max_inspected_records: 10,
                    max_emitted_candidates: 2,
                    cost_budget: 5,
                    started_at: now,
                    updated_at: now,
                    ..Default::default()
                })
                .expect("save run");
            store
                .save_discovered_candidate(
                    kura_evaluation::DiscoveredCandidate {
                        discovered_candidate_id: "candidate_fixture_api".to_string(),
                        tenant_id: "ten_fixture_api".to_string(),
                        discovery_run_id: "discovery_run_fixture_api".to_string(),
                        source_kind: kura_evaluation::SourceKind::Run,
                        source_id: "run_fixture_api".to_string(),
                        score: 0.9,
                        score_band: kura_evaluation::ScoreBand::High,
                        redaction_status: kura_evaluation::RedactionStatus::Redacted,
                        readiness_status: kura_evaluation::ReadinessStatus::FullyReplayable,
                        suppression_state: kura_evaluation::SuppressionState::None,
                        retention_state: kura_evaluation::RetentionState::Active,
                        created_at: now,
                        updated_at: now,
                        ..Default::default()
                    },
                    kura_evaluation::CandidateEvidence {
                        evidence_id: "evidence_fixture_api".to_string(),
                        tenant_id: "ten_fixture_api".to_string(),
                        discovered_candidate_id: "candidate_fixture_api".to_string(),
                        redacted_payload: serde_json::json!({ "goal": "safe" }).as_object().cloned().unwrap_or_default(),
                        materialization_allowed: true,
                        retention_state: kura_evaluation::RetentionState::Active,
                        created_at: now,
                        ..Default::default()
                    },
                )
                .expect("save candidate");
        }

        // Viewer without fixture.manage is denied materialization (403).
        let (status, json) = send(
            &app,
            with_tenant_extension(
                request(
                    "POST",
                    "/v1/evaluation/discovered-candidates/candidate_fixture_api/product-fixtures",
                    Some(r#"{"displayName":"Denied Fixture","domainClass":"schedule","fixturePayload":{"goal":"safe"}}"#),
                ),
                viewer.clone(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "denied create: {json}");
        assert!(json.to_string().contains("evaluation.fixture.manage"));

        // Admin materializes the fixture (201).
        let (status, json) = send(
            &app,
            with_tenant_extension(
                request(
                    "POST",
                    "/v1/evaluation/discovered-candidates/candidate_fixture_api/product-fixtures",
                    Some(r#"{"fixtureId":"product_fixture_api","displayName":"Product Fixture API","domainClass":"schedule","fixturePayload":{"goal":"safe"},"changeSummary":"initial"}"#),
                ),
                admin.clone(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "create fixture: {json}");
        assert_eq!(json["fixture"]["fixtureId"], "product_fixture_api");
        let revision_id = json["revision"]["revisionId"].as_str().expect("revision id").to_string();

        // Viewer without fixture.manage cannot create a revision (403).
        let (status, json) = send(
            &app,
            with_tenant_extension(
                request(
                    "POST",
                    "/v1/evaluation/product-fixtures/product_fixture_api/revisions",
                    Some(r#"{"fixturePayload":{"goal":"viewer edit"},"changeSummary":"viewer edit"}"#),
                ),
                viewer.clone(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "denied revision: {json}");
        assert!(json.to_string().contains("evaluation.fixture.manage"));

        // Viewer without fixture.review is denied (403).
        let review_body = format!(r#"{{"revisionId":"{revision_id}","decision":"approved"}}"#);
        let (status, json) = send(
            &app,
            with_tenant_extension(
                request(
                    "POST",
                    "/v1/evaluation/product-fixtures/product_fixture_api/review",
                    Some(&review_body),
                ),
                viewer.clone(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "denied review: {json}");
        assert!(json.to_string().contains("evaluation.fixture.review"));

        // Admin approves the current revision (200).
        let (status, json) = send(
            &app,
            with_tenant_extension(
                request(
                    "POST",
                    "/v1/evaluation/product-fixtures/product_fixture_api/review",
                    Some(&review_body),
                ),
                admin.clone(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "review: {json}");
        assert_eq!(json["fixture"]["reviewState"], "approved");

        // Viewer without fixture.suppress is denied (403); admin suppresses.
        let (status, json) = send(
            &app,
            with_tenant_extension(
                request("POST", "/v1/evaluation/product-fixtures/product_fixture_api/suppress", None),
                viewer.clone(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "denied suppress: {json}");
        assert!(json.to_string().contains("evaluation.fixture.suppress"));
        let (status, json) = send(
            &app,
            with_tenant_extension(
                request("POST", "/v1/evaluation/product-fixtures/product_fixture_api/suppress", None),
                admin.clone(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "suppress: {json}");
        assert_eq!(json["fixture"]["suppressionState"], "suppressed");
    }

    // Retention/apply uses the store apply_retention DAO (dry-run default).
    #[tokio::test]
    async fn evaluation_product_retention_apply_records_applications() {
        let h = harness(None, None);
        let app = crate::routes::router(h.state.clone());
        let tenant = product_tenant_context("ten_api", Vec::new());
        let (status, json) = send(
            &app,
            with_tenant_extension(
                request("POST", "/v1/evaluation/retention/apply", Some(r#"{"dryRun":true}"#)),
                tenant.clone(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "retention: {json}");
        assert_eq!(json["tenantId"], "ten_api");
        assert_eq!(json["dryRun"], true);
        assert!(!json["applicationIds"].as_array().map(|v| v.is_empty()).unwrap_or(true));

        // Missing tenant answers the stable 400.
        let (status, json) = send(
            &app,
            request("POST", "/v1/evaluation/retention/apply", Some(r#"{"dryRun":true}"#)),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "no tenant: {json}");
    }

    // Campaign list requires evaluation.campaign.read; creation requires
    // evaluation.campaign.manage.
    #[tokio::test]
    async fn evaluation_product_campaign_permissions_and_creation() {
        let h = harness(None, None);
        let app = crate::routes::router(h.state.clone());
        let viewer = product_tenant_context("ten_api", Vec::new());
        let admin = product_tenant_context(
            "ten_api",
            vec![Permission::EvaluationCampaignRead, Permission::EvaluationCampaignManage],
        );

        // List denied without campaign.read.
        let (status, json) = send(
            &app,
            with_tenant_extension(request("GET", "/v1/evaluation/campaigns", None), viewer.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "denied list: {json}");
        assert!(json.to_string().contains("evaluation.campaign.read"));

        // Create denied without campaign.manage.
        let (status, json) = send(
            &app,
            with_tenant_extension(
                request("POST", "/v1/evaluation/campaigns", Some(r#"{"displayName":"Campaign One"}"#)),
                viewer.clone(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "denied create: {json}");
        assert!(json.to_string().contains("evaluation.campaign.manage"));

        // Admin creates a campaign with a fixture selection.
        {
            let store = h.store.lock();
            store
                .upsert_product_fixture(kura_evaluation::ProductManagedFixture {
                    fixture_id: "product_fixture_campaign".to_string(),
                    tenant_id: "ten_api".to_string(),
                    display_name: "Campaign Fixture".to_string(),
                    domain_class: kura_evaluation::FixtureDomainClass::Schedule,
                    source_kind: "run".to_string(),
                    current_revision_id: "revision_1".to_string(),
                    review_state: kura_evaluation::ProductLifecycleStatus::Approved,
                    suppression_state: kura_evaluation::SuppressionState::None,
                    retention_state: kura_evaluation::RetentionState::Active,
                    created_at: now_fixed(),
                    updated_at: now_fixed(),
                    ..Default::default()
                })
                .expect("upsert fixture");
        }
        let campaign_body = serde_json::json!({
            "displayName": "Campaign One",
            "sourceSelections": [
                { "sourceType": "product_fixture", "sourceId": "product_fixture_campaign" },
            ],
        })
        .to_string();
        let (status, json) = send(
            &app,
            with_tenant_extension(request("POST", "/v1/evaluation/campaigns", Some(&campaign_body)), admin.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "create campaign: {json}");
        assert_eq!(json["displayName"], "Campaign One");
        let campaign_id = json["campaignId"].as_str().expect("campaign id").to_string();

        // Campaign items were persisted.
        let (status, json) = send(
            &app,
            with_tenant_extension(
                request("GET", &format!("/v1/evaluation/campaigns/{campaign_id}/items"), None),
                admin.clone(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "campaign items: {json}");
        assert_eq!(json["items"].as_array().map(|v| v.len()), Some(1));
    }

    fn now_fixed() -> chrono::DateTime<chrono::Utc> {
        chrono::Utc::now()
    }
}
