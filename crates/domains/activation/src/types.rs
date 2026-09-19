//! Core domain types for activation state, readiness, and quota baselines.
//!
//! Port of `daemon/internal/activation/types.go`. JSON field names match the
//! Go tags (camelCase) so persisted and wire representations stay compatible.

use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Map;
use serde_json::Value;

define_string_enum!(
    /// Activation lifecycle status.
    Status {
        NOT_STARTED => "not_started",
        IN_PROGRESS => "in_progress",
        BLOCKED => "blocked",
        ACTIVE => "active",
        FIRST_ACTION_COMPLETED => "first_action_completed"
    }
);

/// Activation step identifiers (Go `Step*` constants).
pub const STEP_RESOLVE_PERSONAL_TENANT: &str = "resolve_personal_tenant";
pub const STEP_TENANT_RESOLVED: &str = "tenant_resolved";
pub const STEP_QUOTA_BASELINE: &str = "quota_baseline";
pub const STEP_QUOTA_BASELINE_READY: &str = "quota_baseline_ready";
pub const STEP_TEST_CHAT: &str = "test_chat";
pub const STEP_TEST_CHAT_COMPLETED: &str = "test_chat_completed";
pub const STEP_COMPLETED: &str = "completed";

define_string_enum!(
    /// Stable machine-readable reason for a denial, blocker, or failure.
    ReasonCode {
        PRINCIPAL_DISABLED => "activation_denied:principal_disabled",
        PRINCIPAL_DENIED => "activation_denied:principal_denied",
        TENANT_ACCESS_REVOKED => "activation_denied:tenant_access_revoked",
        QUOTA_BASELINE_UNAVAILABLE => "activation_blocked:quota_baseline_unavailable",
        ENVIRONMENT_UNAVAILABLE => "activation_blocked:environment_unavailable",
        TEST_CHAT_UNAVAILABLE => "activation_blocked:test_chat_unavailable",
        TENANT_RESOLUTION_FAILED => "activation_failed:tenant_resolution",
        TEST_CHAT_FAILED => "activation_failed:test_chat",
        AUDIT_WRITE_FAILED => "activation_failed:audit_write",
        PERSISTENCE_FAILED => "activation_failed:persistence",
        UNEXPECTED_FAILED => "activation_failed:unexpected"
    }
);

define_string_enum!(
    /// Category of a readiness checklist item.
    ReadinessKind {
        TENANT_ACCESS => "tenant_access",
        ENVIRONMENT => "environment",
        QUOTA_BASELINE => "quota_baseline",
        TEST_CHAT => "test_chat"
    }
);

define_string_enum!(
    /// Health of one readiness checklist item.
    ReadinessStatus {
        READY => "ready",
        BLOCKED => "blocked",
        DEGRADED => "degraded",
        MISSING_CONFIGURATION => "missing_configuration",
        OPTIONAL => "optional"
    }
);

define_string_enum!(
    /// Who is expected to remediate a blocker or failure.
    RemediationOwner {
        PRODUCT_USER => "product_user",
        OPERATOR => "operator",
        TENANT_ADMIN => "tenant_admin",
        SYSTEM => "system",
        NONE_REQUIRED => "none_required"
    }
);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadinessItem {
    pub item_id: String,
    pub item_kind: ReadinessKind,
    pub status: ReadinessStatus,
    #[serde(default, skip_serializing_if = "ReasonCode::is_empty")]
    pub reason_code: ReasonCode,
    pub display_name: String,
    pub required_for_activation: bool,
    pub retryable: bool,
    pub remediation_owner: RemediationOwner,
    pub updated_at: DateTime<Utc>,
}

define_string_enum!(
    /// Availability of the projected quota baseline.
    QuotaBaselineStatus {
        AVAILABLE => "available",
        UNAVAILABLE => "unavailable"
    }
);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaProjection {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub category: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub unit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining: Option<i64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub period: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaBaseline {
    pub tenant_id: String,
    pub plan_key: String,
    pub enforcement_mode: String,
    pub status: QuotaBaselineStatus,
    pub quotas: Vec<QuotaProjection>,
    pub projected_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub projection_source: String,
    #[serde(default, skip_serializing_if = "ReasonCode::is_empty")]
    pub reason_code: ReasonCode,
}

/// Action kind of the default activation first action (test chat).
pub const FIRST_ACTION_TEST_CHAT: &str = "test_chat";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FirstAction {
    pub action_id: String,
    pub action_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub display_name: String,
    pub recommended: bool,
    pub available: bool,
    pub blocking_item_ids: Vec<String>,
    pub invoke_route: String,
    pub result_route: String,
}

/// The default first action offered once activation is otherwise ready.
#[must_use]
pub fn default_test_chat_first_action(
    available: bool,
    blocking_item_ids: Vec<String>,
) -> FirstAction {
    FirstAction {
        action_id: FIRST_ACTION_TEST_CHAT.to_string(),
        action_kind: FIRST_ACTION_TEST_CHAT.to_string(),
        display_name: "Test chat".to_string(),
        recommended: true,
        available,
        blocking_item_ids,
        invoke_route: "/v1/activation/test-chat".to_string(),
        result_route: "/v1/activation".to_string(),
    }
}

define_string_enum!(
    /// Outcome of an activation test chat run.
    TestChatStatus {
        COMPLETED => "completed",
        FAILED => "failed",
        CANCELLED => "cancelled"
    }
);

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestChatMetadata {
    pub activation_id: String,
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dispatch_id: String,
    pub status: TestChatStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub usage: Map<String, Value>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub finish_reason: String,
    #[serde(default, skip_serializing_if = "ReasonCode::is_empty")]
    pub reason_code: ReasonCode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
}

define_string_enum!(
    /// Pipeline stage at which activation failed or blocked.
    FailureStage {
        TENANT_RESOLUTION => "tenant_resolution",
        ELIGIBILITY => "eligibility",
        QUOTA_BASELINE => "quota_baseline",
        AUTHORIZATION => "authorization",
        TEST_CHAT => "test_chat",
        AUDIT => "audit",
        PERSISTENCE => "persistence",
        UNEXPECTED => "unexpected"
    }
);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FailureReason {
    pub reason_code: ReasonCode,
    pub stage: FailureStage,
    pub retryable: bool,
    pub remediation_owner: RemediationOwner,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditMetadata {
    pub activation_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub principal_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub token_id: String,
    #[serde(default, skip_serializing_if = "FailureStage::is_empty")]
    pub stage: FailureStage,
    #[serde(default, skip_serializing_if = "Status::is_empty")]
    pub from_status: Status,
    #[serde(default, skip_serializing_if = "Status::is_empty")]
    pub to_status: Status,
    #[serde(default, skip_serializing_if = "ReasonCode::is_empty")]
    pub reason_code: ReasonCode,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "RemediationOwner::is_empty")]
    pub remediation_owner: RemediationOwner,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_chat: Option<TestChatMetadata>,
    pub transitioned_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub environment_scope: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub completed_step_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub readiness_item_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub quota_baseline_status: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub activation_id: String,
    pub principal_id: String,
    pub tenant_id: String,
    pub environment_scope: String,
    pub status: Status,
    pub current_step_id: String,
    pub completed_step_ids: Vec<String>,
    pub blocking_reason_codes: Vec<ReasonCode>,
    pub readiness_items: Vec<ReadinessItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_baseline: Option<QuotaBaseline>,
    pub first_action: FirstAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_chat: Option<TestChatMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<FailureReason>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_action_completed_at: Option<DateTime<Utc>>,
    pub last_evaluated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_transition_audit_event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub activation_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub principal_id: String,
    pub status: Status,
    pub stage: FailureStage,
    pub reason_code: ReasonCode,
    pub retryable: bool,
    pub remediation_owner: RemediationOwner,
    pub last_transition_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub readiness_item_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub quota_baseline_status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_chat: Option<TestChatMetadata>,
}
