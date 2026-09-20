//! Port of `daemon/internal/evaluation/types.go` (the replay ledger model) and
//! `daemon/internal/evaluation/product_types.go` (the tenant-scoped evaluation
//! product model: discovery policies/runs, discovered candidates, candidate
//! evidence, suppressions, product-managed fixtures, replay campaigns, campaign
//! attempt groups, dashboard projections, and tool-call inspections). Wire
//! layout mirrors the Go JSON tags exactly: camelCase fields, snake_case enum
//! values, and `omitempty` mapped to `#[serde(default, skip_serializing_if =
//! ...)]`.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

string_enum!(CandidateKind {
    CuratedWork => "curated_work",
    Fixture => "fixture",
});

string_enum!(SourceKind {
    Run => "run",
    Workflow => "workflow",
    Schedule => "schedule",
    Integration => "integration",
    ComputerUse => "computer_use",
    Fixture => "fixture",
    // Open-string values the Go package uses without named constants:
    // `SourceKind("tool_call")`, `SourceKind("live_validation_ledger")`, and the
    // repo fixture manifests' `SourceKind("fixture_evidence")` captured-evidence
    // refs.
    FixtureEvidence => "fixture_evidence",
    ToolCall => "tool_call",
    LiveValidationLedger => "live_validation_ledger",
});

string_enum!(ReadinessStatus {
    FullyReplayable => "fully_replayable",
    PartiallyReplayable => "partially_replayable",
    Blocked => "blocked",
    Unreplayable => "unreplayable",
});

string_enum!(ReplayMode {
    NonLive => "non_live",
    LiveValidation => "live_validation",
});

string_enum!(ReplayAttemptStatus {
    Queued => "queued",
    Running => "running",
    Completed => "completed",
    Blocked => "blocked",
    Unreplayable => "unreplayable",
    Failed => "failed",
    Cancelled => "cancelled",
});

string_enum!(ApprovalHandling {
    Blocked => "blocked",
    EvidenceOnly => "evidence_only",
    FreshApprovalRequired => "fresh_approval_required",
});

string_enum!(SideEffectHandling {
    Blocked => "blocked",
    EvidenceOnly => "evidence_only",
    Live => "live",
});

string_enum!(ComparisonTerminalStatus {
    Matched => "matched",
    Drifted => "drifted",
    Blocked => "blocked",
    Unreplayable => "unreplayable",
});

string_enum!(DriftPlane {
    Runtime => "runtime",
    Policy => "policy",
    Integration => "integration",
    Delivery => "delivery",
    Evidence => "evidence",
    Unknown => "unknown",
    Mixed => "mixed",
});

string_enum!(FixtureDomainClass {
    Schedule => "schedule",
    Integration => "integration",
    ComputerUse => "computer_use",
});

// --- Product-model enums (product_types.go) ------------------------------

string_enum!(ProductResourceKind {
    DiscoveryPolicy => "discovery_policy",
    DiscoveryRun => "discovery_run",
    DiscoveredCandidate => "discovered_candidate",
    CandidateEvidence => "candidate_evidence",
    Suppression => "suppression",
    ProductFixture => "product_fixture",
    FixtureRevision => "fixture_revision",
    Campaign => "campaign",
    CampaignItem => "campaign_item",
    CampaignAttemptGroup => "campaign_attempt_group",
    DashboardProjection => "dashboard_projection",
    ToolCallInspection => "tool_call_inspection",
    RetentionApplication => "retention_application",
});

string_enum!(ProductLifecycleStatus {
    Queued => "queued",
    Running => "running",
    Completed => "completed",
    Partial => "partial",
    Failed => "failed",
    Cancelled => "cancelled",
    Draft => "draft",
    InReview => "in_review",
    Approved => "approved",
    Rejected => "rejected",
    Published => "published",
    Archived => "archived",
    Deleted => "deleted",
    Expired => "expired",
    Suppressed => "suppressed",
});

string_enum!(RetentionState {
    Active => "active",
    Expired => "expired",
    Deleted => "deleted",
    Tombstone => "tombstone",
});

string_enum!(SuppressionState {
    None => "none",
    Suppressed => "suppressed",
    Expired => "expired",
    Revoked => "revoked",
});

// Evaluation redaction status. Distinct from `kura_connectors::RedactionStatus`
// (which carries `redacted`/`suppressed`/`redaction_failed` wire values for
// connector conformance evidence); the Go evaluation package defines its own
// `RedactionStatus` with `clean`/`redacted`/`failed`.
string_enum!(RedactionStatus {
    Clean => "clean",
    Redacted => "redacted",
    Failed => "failed",
});

string_enum!(ScoreBand {
    High => "high",
    Medium => "medium",
    Low => "low",
});

// --- Replay ledger model (types.go) --------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRef {
    pub kind: SourceKind,
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub route: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SafetyScope {
    pub mode: ReplayMode,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaneSummaries {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub runtime: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub policy: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub integration: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub delivery: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub evidence: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayCandidate {
    pub candidate_id: String,
    pub candidate_kind: CandidateKind,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub source_kind: SourceKind,
    pub source_id: String,
    pub source_refs: Vec<SourceRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_classes: Vec<String>,
    pub environment_scope: String,
    pub readiness_status: ReadinessStatus,
    pub readiness_reasons: Vec<String>,
    pub limitations: Vec<String>,
    pub default_replay_mode: ReplayMode,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fixture_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub latest_attempt_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub latest_comparison_id: String,
    // Go json tag is "expectedComparisonSummary" (not the camelCase of the field).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "expectedComparisonSummary"
    )]
    pub expected_comparison: Option<PlaneSummaries>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub captured_evidence_refs: Vec<SourceRef>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayAttempt {
    pub attempt_id: String,
    pub candidate_id: String,
    pub source_refs: Vec<SourceRef>,
    pub environment_scope: String,
    pub mode: ReplayMode,
    pub status: ReplayAttemptStatus,
    pub safety_scope: SafetyScope,
    pub approval_handling: ApprovalHandling,
    pub side_effect_handling: SideEffectHandling,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub launched_by: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub change_window_label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub baseline_attempt_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub result_run_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub result_workflow_id: String,
    pub evidence_refs: Vec<SourceRef>,
    pub blocked_reasons: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub runtime_summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub policy_summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub integration_summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub delivery_summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub evidence_summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComparisonResult {
    pub comparison_id: String,
    pub candidate_id: String,
    pub baseline_ref: String,
    pub attempt_id: String,
    pub environment_scope: String,
    pub terminal_status: ComparisonTerminalStatus,
    pub runtime_summary: String,
    pub policy_summary: String,
    pub integration_summary: String,
    pub delivery_summary: String,
    pub evidence_summary: String,
    pub confidence: String,
    pub limitations: Vec<String>,
    pub drift_findings: Vec<DriftFinding>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub change_window_label: String,
    pub generated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriftFinding {
    pub finding_id: String,
    pub comparison_id: String,
    pub plane: DriftPlane,
    pub severity: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub baseline_value: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub replay_value: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_refs: Vec<SourceRef>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub recommended_action: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegressionFixture {
    pub fixture_id: String,
    pub display_name: String,
    pub domain_class: FixtureDomainClass,
    #[serde(default)]
    pub manifest_path: String,
    pub source_refs: Vec<SourceRef>,
    pub captured_evidence_refs: Vec<SourceRef>,
    pub assumptions: Vec<String>,
    pub limitations: Vec<String>,
    pub expected_replay_mode: ReplayMode,
    pub expected_comparison_summary: PlaneSummaries,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub candidate_id: String,
    #[serde(default)]
    pub environment_scope: String,
    #[serde(default = "crate::types::go_zero_time_default")]
    pub created_at: DateTime<Utc>,
    #[serde(default = "crate::types::go_zero_time_default")]
    pub updated_at: DateTime<Utc>,
}

#[must_use]
fn go_zero_time_default() -> chrono::DateTime<Utc> {
    crate::util::go_zero_time()
}

// Plain query structs: no serde, used as API query parameters.

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CandidateFilter {
    pub environment_scope: String,
    pub candidate_kind: CandidateKind,
    pub source_kind: SourceKind,
    pub readiness_status: ReadinessStatus,
    pub limit: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AttemptFilter {
    pub environment_scope: String,
    pub candidate_id: String,
    pub status: ReplayAttemptStatus,
    pub limit: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComparisonFilter {
    pub environment_scope: String,
    pub candidate_id: String,
    pub attempt_id: String,
    pub terminal_status: ComparisonTerminalStatus,
    pub limit: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FixtureFilter {
    pub environment_scope: String,
    pub domain_class: FixtureDomainClass,
    pub limit: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateReplayAttemptInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<ReplayMode>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub change_window_label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub baseline_attempt_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safety_scope: Option<SafetyScope>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub launched_by: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateComparisonInput {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub baseline_attempt_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub baseline_ref: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub change_window_label: String,
}

// --- Product model (product_types.go) -------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryPolicy {
    pub policy_id: String,
    pub tenant_id: String,
    pub enabled: bool,
    pub source_kinds: Vec<SourceKind>,
    pub window_start: DateTime<Utc>,
    pub window_end: DateTime<Utc>,
    pub max_inspected_records: i64,
    pub max_emitted_candidates: i64,
    pub cost_budget: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sensitive_field_rules: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub retention_policy_ref: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryRun {
    pub discovery_run_id: String,
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub policy_id: String,
    pub status: ProductLifecycleStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cursor: String,
    pub source_kinds: Vec<SourceKind>,
    pub window_start: DateTime<Utc>,
    pub window_end: DateTime<Utc>,
    pub max_inspected_records: i64,
    pub max_emitted_candidates: i64,
    pub cost_budget: i64,
    pub inspected_records: i64,
    pub emitted_candidates: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub partial_reason: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub started_by: String,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredCandidate {
    pub discovered_candidate_id: String,
    pub tenant_id: String,
    pub discovery_run_id: String,
    pub source_kind: SourceKind,
    pub source_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_refs: Vec<SourceRef>,
    pub score: f64,
    pub score_band: ScoreBand,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub explanation_fields: serde_json::Map<String, serde_json::Value>,
    pub redaction_status: RedactionStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub evidence_ref: String,
    pub readiness_status: ReadinessStatus,
    pub suppression_state: SuppressionState,
    pub retention_state: RetentionState,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateEvidence {
    pub evidence_id: String,
    pub tenant_id: String,
    pub discovered_candidate_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_refs: Vec<SourceRef>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub redacted_payload: serde_json::Map<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redaction_rules_applied: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sensitive_fields_excluded: Vec<String>,
    pub materialization_allowed: bool,
    pub retention_state: RetentionState,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuppressionRecord {
    pub suppression_id: String,
    pub tenant_id: String,
    pub target_kind: ProductResourceKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub target_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub target_source_ref: String,
    pub reason_code: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    pub active: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductManagedFixture {
    pub fixture_id: String,
    pub tenant_id: String,
    pub display_name: String,
    pub domain_class: FixtureDomainClass,
    pub source_kind: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_refs: Vec<SourceRef>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_candidate_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub current_revision_id: String,
    pub review_state: ProductLifecycleStatus,
    pub suppression_state: SuppressionState,
    pub retention_state: RetentionState,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FixtureRevision {
    pub revision_id: String,
    pub fixture_id: String,
    pub tenant_id: String,
    pub revision_number: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content_summary: String,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub fixture_payload: serde_json::Map<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub change_summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_evidence_refs: Vec<String>,
    pub redaction_status: RedactionStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub created_by: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayCampaign {
    pub campaign_id: String,
    pub tenant_id: String,
    pub display_name: String,
    pub status: ProductLifecycleStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub scope_summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub started_by: String,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<DateTime<Utc>>,
    pub retention_state: RetentionState,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CampaignItem {
    pub campaign_item_id: String,
    pub campaign_id: String,
    pub tenant_id: String,
    pub source_type: ProductResourceKind,
    pub source_id: String,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub source_snapshot: serde_json::Map<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub selection_reason: String,
    pub suppression_checked_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CampaignAttemptGroup {
    pub attempt_group_id: String,
    pub campaign_id: String,
    pub campaign_item_id: String,
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replay_attempt_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comparison_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub live_validation_ids: Vec<String>,
    pub status: ProductLifecycleStatus,
    pub drift_count: i64,
    pub failure_count: i64,
    pub unsupported_count: i64,
    pub operator_action_needed_count: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardProjection {
    pub projection_id: String,
    pub tenant_id: String,
    pub window_start: DateTime<Utc>,
    pub window_end: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub campaign_status_counts: HashMap<String, i64>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub drift_summary: HashMap<String, i64>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub failure_summary: HashMap<String, i64>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub unsupported_summary: HashMap<String, i64>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub operator_action_needed_summary: HashMap<String, i64>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub live_validation_summary: HashMap<String, i64>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub candidate_summary: HashMap<String, i64>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub fixture_summary: HashMap<String, i64>,
    pub generated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cursor: String,
    pub retention_state: RetentionState,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallInspection {
    pub inspection_id: String,
    pub tenant_id: String,
    pub campaign_id: String,
    pub campaign_item_id: String,
    pub tool_call_ref: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub original_evidence_ref: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub non_live_replay_evidence_ref: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub live_validation_ledger_refs: Vec<String>,
    pub classification: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub diff_summary: String,
    pub redaction_status: RedactionStatus,
    pub retention_state: RetentionState,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductPage {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cursor: String,
    pub limit: i64,
}
