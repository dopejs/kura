//! Evaluation product audit / discovery / fixture / campaign / dashboard /
//! tool-call-inspection events (port of `evaluation_product.go`).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::util::{is_go_zero_time, now_utc, payload};
use crate::{Event, Resource};
use kura_evaluation::{ProductLifecycleStatus, ProductResourceKind, RedactionStatus};

pub const EVALUATION_PRODUCT_AUDIT_RECORDED_NAME: &str = "evaluation.product_audit_recorded";
pub const EVALUATION_PRODUCT_REDACTION_FAILED_NAME: &str = "evaluation.product_redaction_failed";
pub const EVALUATION_PRODUCT_RETENTION_APPLIED_NAME: &str = "evaluation.product_retention_applied";
pub const EVALUATION_DISCOVERY_POLICY_CHANGED_NAME: &str = "evaluation.discovery_policy_changed";
pub const EVALUATION_DISCOVERY_STARTED_NAME: &str = "evaluation.discovery_started";
pub const EVALUATION_DISCOVERY_COMPLETED_NAME: &str = "evaluation.discovery_completed";
pub const EVALUATION_DISCOVERY_PARTIAL_NAME: &str = "evaluation.discovery_partial";
pub const EVALUATION_DISCOVERY_FAILED_NAME: &str = "evaluation.discovery_failed";
pub const EVALUATION_DISCOVERY_CANDIDATE_NAME: &str = "evaluation.discovery_candidate_suggested";
pub const EVALUATION_DISCOVERY_SUPPRESSED_NAME: &str = "evaluation.discovery_suppressed";
pub const EVALUATION_DISCOVERY_REDACTION_FAILED_NAME: &str =
    "evaluation.discovery_redaction_failed";
pub const EVALUATION_DISCOVERY_RETENTION_APPLIED_NAME: &str =
    "evaluation.discovery_retention_applied";
pub const EVALUATION_FIXTURE_CREATED_NAME: &str = "evaluation.fixture.created";
pub const EVALUATION_FIXTURE_REVISION_CREATED_NAME: &str = "evaluation.fixture.revision_created";
pub const EVALUATION_FIXTURE_REVIEWED_NAME: &str = "evaluation.fixture.reviewed";
pub const EVALUATION_FIXTURE_REDACTION_FAILED_NAME: &str = "evaluation.fixture.redaction_failed";
pub const EVALUATION_FIXTURE_SUPPRESSED_NAME: &str = "evaluation.fixture.suppressed";
pub const EVALUATION_FIXTURE_ARCHIVED_NAME: &str = "evaluation.fixture.archived";
pub const EVALUATION_FIXTURE_DELETED_NAME: &str = "evaluation.fixture.deleted";
pub const EVALUATION_FIXTURE_DENIED_NAME: &str = "evaluation.fixture.denied";
pub const EVALUATION_CAMPAIGN_CREATED_NAME: &str = "evaluation.campaign.created";
pub const EVALUATION_CAMPAIGN_STARTED_NAME: &str = "evaluation.campaign.started";
pub const EVALUATION_CAMPAIGN_CANCELLED_NAME: &str = "evaluation.campaign.cancelled";
pub const EVALUATION_CAMPAIGN_COMPLETED_NAME: &str = "evaluation.campaign.completed";
pub const EVALUATION_CAMPAIGN_FAILED_NAME: &str = "evaluation.campaign.failed";
pub const EVALUATION_CAMPAIGN_RESULTS_PUBLISHED_NAME: &str =
    "evaluation.campaign.results_published";
pub const EVALUATION_CAMPAIGN_REDACTION_FAILED_NAME: &str = "evaluation.campaign.redaction_failed";
pub const EVALUATION_DASHBOARD_PROJECTION_GENERATED_NAME: &str =
    "evaluation.dashboard.projection_generated";
pub const EVALUATION_TOOL_CALL_INSPECTION_GENERATED_NAME: &str =
    "evaluation.tool_call_inspection.generated";
pub const EVALUATION_TOOL_CALL_INSPECTION_REDACTION_FAILED_NAME: &str =
    "evaluation.tool_call_inspection.redaction_failed";
pub const EVALUATION_TOOL_CALL_INSPECTION_RETENTION_APPLIED_NAME: &str =
    "evaluation.tool_call_inspection.retention_applied";

/// Go: `EvaluationProductAuditPayload`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluationProductAuditPayload {
    pub tenant_id: String,
    pub actor_id: String,
    pub action: String,
    pub target_kind: ProductResourceKind,
    pub target_id: String,
    pub outcome: String,
    pub reason_code: String,
    pub evidence_refs: Vec<String>,
    pub retention_app_id: String,
    pub occurred_at: DateTime<Utc>,
}

/// Go: `EvaluationProductAuditEvent`.
#[must_use]
pub fn evaluation_product_audit_event(name: &str, input: EvaluationProductAuditPayload) -> Event {
    let occurred_at = if is_go_zero_time(input.occurred_at) {
        now_utc()
    } else {
        input.occurred_at
    };
    let mut map = payload![
        "tenantId" => input.tenant_id,
        "actorId" => input.actor_id,
        "action" => input.action,
        "targetKind" => input.target_kind.as_str(),
        "outcome" => input.outcome,
        "createdAt" => occurred_at,
    ];
    if !input.target_id.is_empty() {
        map.insert("targetId".to_string(), serde_json::json!(input.target_id));
    }
    if !input.reason_code.is_empty() {
        map.insert(
            "reasonCode".to_string(),
            serde_json::json!(input.reason_code),
        );
    }
    if !input.evidence_refs.is_empty() {
        map.insert(
            "evidenceRefs".to_string(),
            serde_json::json!(input.evidence_refs),
        );
    }
    if !input.retention_app_id.is_empty() {
        map.insert(
            "retentionApplicationId".to_string(),
            serde_json::json!(input.retention_app_id),
        );
    }
    Event {
        tenant_id: input.tenant_id.clone(),
        category: "evaluation".to_string(),
        name: name.to_string(),
        occurred_at,
        resource: Resource {
            kind: input.target_kind.as_str().to_string(),
            id: input.target_id.clone(),
        },
        payload: map,
        ..Event::default()
    }
}

/// Go: `EvaluationDiscoveryPayload`. `redaction_status` is optional: the Go
/// builder omits the payload key when the status is the empty string, which
/// the closed Rust enum cannot represent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluationDiscoveryPayload {
    pub tenant_id: String,
    pub policy_id: String,
    pub discovery_run_id: String,
    pub discovered_candidate_id: String,
    pub suppression_id: String,
    pub status: ProductLifecycleStatus,
    pub reason_code: String,
    pub redaction_status: Option<RedactionStatus>,
    pub retention_application_id: String,
    pub occurred_at: DateTime<Utc>,
}

/// Go: `EvaluationDiscoveryEvent` — the resource resolves to the most specific
/// evidence present: candidate > suppression > discovery run > policy.
#[must_use]
pub fn evaluation_discovery_event(name: &str, input: EvaluationDiscoveryPayload) -> Event {
    let occurred_at = if is_go_zero_time(input.occurred_at) {
        now_utc()
    } else {
        input.occurred_at
    };
    let (resource_kind, resource_id) = if !input.discovered_candidate_id.is_empty() {
        (
            ProductResourceKind::DiscoveredCandidate.as_str(),
            input.discovered_candidate_id.clone(),
        )
    } else if !input.suppression_id.is_empty() {
        (
            ProductResourceKind::Suppression.as_str(),
            input.suppression_id.clone(),
        )
    } else if !input.policy_id.is_empty() && input.discovery_run_id.is_empty() {
        (
            ProductResourceKind::DiscoveryPolicy.as_str(),
            input.policy_id.clone(),
        )
    } else {
        (
            ProductResourceKind::DiscoveryRun.as_str(),
            input.discovery_run_id.clone(),
        )
    };
    let mut map = payload![
        "tenantId" => input.tenant_id,
        "status" => input.status.as_str(),
    ];
    if !input.policy_id.is_empty() {
        map.insert("policyId".to_string(), serde_json::json!(input.policy_id));
    }
    if !input.discovery_run_id.is_empty() {
        map.insert(
            "discoveryRunId".to_string(),
            serde_json::json!(input.discovery_run_id),
        );
    }
    if !input.discovered_candidate_id.is_empty() {
        map.insert(
            "discoveredCandidateId".to_string(),
            serde_json::json!(input.discovered_candidate_id),
        );
    }
    if !input.suppression_id.is_empty() {
        map.insert(
            "suppressionId".to_string(),
            serde_json::json!(input.suppression_id),
        );
    }
    if !input.reason_code.is_empty() {
        map.insert(
            "reasonCode".to_string(),
            serde_json::json!(input.reason_code),
        );
    }
    if let Some(status) = input.redaction_status {
        map.insert(
            "redactionStatus".to_string(),
            serde_json::json!(status.as_str()),
        );
    }
    if !input.retention_application_id.is_empty() {
        map.insert(
            "retentionApplicationId".to_string(),
            serde_json::json!(input.retention_application_id),
        );
    }
    Event {
        tenant_id: input.tenant_id.clone(),
        category: "evaluation".to_string(),
        name: name.to_string(),
        occurred_at,
        resource: Resource {
            kind: resource_kind.to_string(),
            id: resource_id,
        },
        payload: map,
        ..Event::default()
    }
}

/// Go: `EvaluationFixturePayload`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluationFixturePayload {
    pub tenant_id: String,
    pub actor_id: String,
    pub fixture_id: String,
    pub revision_id: String,
    pub source_candidate_id: String,
    pub source_evidence_refs: Vec<String>,
    pub review_state: Option<ProductLifecycleStatus>,
    pub redaction_status: Option<RedactionStatus>,
    pub outcome: String,
    pub reason_code: String,
    pub occurred_at: DateTime<Utc>,
}

/// Go: `EvaluationFixtureEvent`.
#[must_use]
pub fn evaluation_fixture_event(name: &str, input: EvaluationFixturePayload) -> Event {
    let occurred_at = if is_go_zero_time(input.occurred_at) {
        now_utc()
    } else {
        input.occurred_at
    };
    let mut map = payload![
        "tenantId" => input.tenant_id,
        "fixtureId" => input.fixture_id,
        "outcome" => input.outcome,
    ];
    if !input.actor_id.is_empty() {
        map.insert("actorId".to_string(), serde_json::json!(input.actor_id));
    }
    if !input.revision_id.is_empty() {
        map.insert(
            "revisionId".to_string(),
            serde_json::json!(input.revision_id),
        );
    }
    if !input.source_candidate_id.is_empty() {
        map.insert(
            "sourceCandidateId".to_string(),
            serde_json::json!(input.source_candidate_id),
        );
    }
    if !input.source_evidence_refs.is_empty() {
        map.insert(
            "sourceEvidenceRefs".to_string(),
            serde_json::json!(input.source_evidence_refs),
        );
    }
    if let Some(state) = input.review_state {
        map.insert("reviewState".to_string(), serde_json::json!(state.as_str()));
    }
    if let Some(status) = input.redaction_status {
        map.insert(
            "redactionStatus".to_string(),
            serde_json::json!(status.as_str()),
        );
    }
    if !input.reason_code.is_empty() {
        map.insert(
            "reasonCode".to_string(),
            serde_json::json!(input.reason_code),
        );
    }
    Event {
        tenant_id: input.tenant_id.clone(),
        category: "evaluation".to_string(),
        name: name.to_string(),
        occurred_at,
        resource: Resource {
            kind: ProductResourceKind::ProductFixture.as_str().to_string(),
            id: input.fixture_id.clone(),
        },
        payload: map,
        ..Event::default()
    }
}

/// Go: `EvaluationCampaignPayload`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluationCampaignPayload {
    pub tenant_id: String,
    pub actor_id: String,
    pub campaign_id: String,
    pub campaign_item_id: String,
    pub attempt_group_id: String,
    pub status: ProductLifecycleStatus,
    pub outcome: String,
    pub reason_code: String,
    pub redaction_status: Option<RedactionStatus>,
    pub occurred_at: DateTime<Utc>,
}

/// Go: `EvaluationCampaignEvent`.
#[must_use]
pub fn evaluation_campaign_event(name: &str, input: EvaluationCampaignPayload) -> Event {
    let occurred_at = if is_go_zero_time(input.occurred_at) {
        now_utc()
    } else {
        input.occurred_at
    };
    let mut map = payload![
        "tenantId" => input.tenant_id,
        "campaignId" => input.campaign_id,
        "status" => input.status.as_str(),
    ];
    if !input.actor_id.is_empty() {
        map.insert("actorId".to_string(), serde_json::json!(input.actor_id));
    }
    if !input.campaign_item_id.is_empty() {
        map.insert(
            "campaignItemId".to_string(),
            serde_json::json!(input.campaign_item_id),
        );
    }
    if !input.attempt_group_id.is_empty() {
        map.insert(
            "attemptGroupId".to_string(),
            serde_json::json!(input.attempt_group_id),
        );
    }
    if !input.outcome.is_empty() {
        map.insert("outcome".to_string(), serde_json::json!(input.outcome));
    }
    if !input.reason_code.is_empty() {
        map.insert(
            "reasonCode".to_string(),
            serde_json::json!(input.reason_code),
        );
    }
    if let Some(status) = input.redaction_status {
        map.insert(
            "redactionStatus".to_string(),
            serde_json::json!(status.as_str()),
        );
    }
    Event {
        tenant_id: input.tenant_id.clone(),
        category: "evaluation".to_string(),
        name: name.to_string(),
        occurred_at,
        resource: Resource {
            kind: ProductResourceKind::Campaign.as_str().to_string(),
            id: input.campaign_id.clone(),
        },
        payload: map,
        ..Event::default()
    }
}

/// Go: `EvaluationDashboardPayload`. Window bounds are omitted when unset;
/// `generated_at` falls back to the event time.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluationDashboardPayload {
    pub tenant_id: String,
    pub projection_id: String,
    pub window_start: DateTime<Utc>,
    pub window_end: DateTime<Utc>,
    pub generated_at: DateTime<Utc>,
    pub outcome: String,
    pub occurred_at: DateTime<Utc>,
}

/// Go: `EvaluationDashboardEvent`.
#[must_use]
pub fn evaluation_dashboard_event(name: &str, input: EvaluationDashboardPayload) -> Event {
    let occurred_at = if is_go_zero_time(input.occurred_at) {
        now_utc()
    } else {
        input.occurred_at
    };
    let generated_at = if is_go_zero_time(input.generated_at) {
        occurred_at
    } else {
        input.generated_at
    };
    let mut map = payload![
        "tenantId" => input.tenant_id,
        "projectionId" => input.projection_id,
        "generatedAt" => generated_at,
    ];
    if !is_go_zero_time(input.window_start) {
        map.insert(
            "windowStart".to_string(),
            serde_json::json!(input.window_start),
        );
    }
    if !is_go_zero_time(input.window_end) {
        map.insert("windowEnd".to_string(), serde_json::json!(input.window_end));
    }
    if !input.outcome.is_empty() {
        map.insert("outcome".to_string(), serde_json::json!(input.outcome));
    }
    Event {
        tenant_id: input.tenant_id.clone(),
        category: "evaluation".to_string(),
        name: name.to_string(),
        occurred_at,
        resource: Resource {
            kind: ProductResourceKind::DashboardProjection
                .as_str()
                .to_string(),
            id: input.projection_id.clone(),
        },
        payload: map,
        ..Event::default()
    }
}

/// Go: `EvaluationToolCallInspectionPayload`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluationToolCallInspectionPayload {
    pub tenant_id: String,
    pub inspection_id: String,
    pub campaign_id: String,
    pub campaign_item_id: String,
    pub classification: String,
    pub redaction_status: Option<RedactionStatus>,
    pub retention_application_id: String,
    pub outcome: String,
    pub reason_code: String,
    pub occurred_at: DateTime<Utc>,
}

/// Go: `EvaluationToolCallInspectionEvent`.
#[must_use]
pub fn evaluation_tool_call_inspection_event(
    name: &str,
    input: EvaluationToolCallInspectionPayload,
) -> Event {
    let occurred_at = if is_go_zero_time(input.occurred_at) {
        now_utc()
    } else {
        input.occurred_at
    };
    let mut map = payload![
        "tenantId" => input.tenant_id,
        "inspectionId" => input.inspection_id,
        "campaignId" => input.campaign_id,
        "classification" => input.classification,
    ];
    if !input.campaign_item_id.is_empty() {
        map.insert(
            "campaignItemId".to_string(),
            serde_json::json!(input.campaign_item_id),
        );
    }
    if let Some(status) = input.redaction_status {
        map.insert(
            "redactionStatus".to_string(),
            serde_json::json!(status.as_str()),
        );
    }
    if !input.retention_application_id.is_empty() {
        map.insert(
            "retentionApplicationId".to_string(),
            serde_json::json!(input.retention_application_id),
        );
    }
    if !input.outcome.is_empty() {
        map.insert("outcome".to_string(), serde_json::json!(input.outcome));
    }
    if !input.reason_code.is_empty() {
        map.insert(
            "reasonCode".to_string(),
            serde_json::json!(input.reason_code),
        );
    }
    Event {
        tenant_id: input.tenant_id.clone(),
        category: "evaluation".to_string(),
        name: name.to_string(),
        occurred_at,
        resource: Resource {
            kind: ProductResourceKind::ToolCallInspection.as_str().to_string(),
            id: input.inspection_id.clone(),
        },
        payload: map,
        ..Event::default()
    }
}
