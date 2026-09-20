//! Port of the opsreadiness types (types.go + hosted_types.go): release/rollback/soak
//! evidence records and the hosted-deployment evidence model. The validation/smoke/hosted
//! logic is the next increment.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// Status / classification / result constants (types.go).
pub const STATUS_PASS: &str = "pass";
pub const STATUS_FAIL: &str = "fail";
pub const STATUS_SKIP: &str = "skip";

pub const TOPOLOGY_TENANT_SCOPED_SINGLE_NODE: &str = "tenant_scoped_single_node";
pub const ENVIRONMENT_TEST: &str = "test";

pub const CLASSIFICATION_RECOVERED: &str = "recovered";
pub const CLASSIFICATION_INTERRUPTED: &str = "interrupted";
pub const CLASSIFICATION_RETRIED: &str = "retried";
pub const CLASSIFICATION_RETRY_EXHAUSTED: &str = "retry_exhausted";
pub const CLASSIFICATION_OPERATOR_ACTION_NEEDED: &str = "operator_action_needed";

pub const RESULT_SHIP: &str = "ship";
pub const RESULT_NO_SHIP: &str = "no_ship";
pub const RESULT_SHIP_WITH_RECORDED_SKIPS: &str = "ship_with_recorded_skips";

pub const REQUIRED_WORKLOAD_AREAS: &[&str] = &[
    "runtime",
    "scheduler",
    "integrations",
    "delivery",
    "approvals",
    "quotas",
    "tenant_switching",
    "evaluation",
];

pub const REQUIRED_FAULT_TYPES: &[&str] = &[
    "transient_5xx",
    "rate_limit",
    "auth_expiry",
    "provider_unavailable",
    "slow_response",
    "malformed_response",
];

pub const REQUIRED_RESOURCE_CATEGORIES: &[&str] = &[
    "logs",
    "stored_data_size",
    "active_work_or_queue_backlog",
    "memory",
];

mod hosted;
mod validation;
pub use hosted::*;
pub use validation::*;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunbookEvidence {
    pub name: String,
    pub steps: Vec<String>,
    pub elapsed: Duration,
    pub max_elapsed: Duration,
    pub health_checks: Vec<String>,
    pub diagnostics: Vec<String>,
    pub failure_modes: Vec<String>,
    pub rollback_or_cleanup: Vec<String>,
    pub out_of_scope: Vec<String>,
    pub test_environment: bool,
    pub production_opt_in: bool,
    pub used_production_data: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MigrationVerificationReport {
    pub source_version: String,
    pub target_version: String,
    pub preflight_checks: Vec<String>,
    pub postflight_checks: Vec<String>,
    pub migration_progress: String,
    pub tenant_integrity_summary: String,
    pub quota_accounting_summary: String,
    pub credential_remediation: String,
    pub rollback_path: String,
    pub result: String,
    pub operator_diagnostics: Vec<String>,
    pub persisted_state_reversible: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RollbackDecision {
    pub in_place_safe: bool,
    pub persisted_state_reversible: bool,
    pub backup_verified: bool,
    pub selected_path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TenantStateSummary {
    pub tenant_id: String,
    pub credential_refs: Vec<String>,
    pub quota_state: String,
    pub work_state: String,
    pub reconnect_required: bool,
    pub operator_action_needed: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupArtifact {
    pub artifact_id: String,
    pub created_at: DateTime<Utc>,
    pub source_version: String,
    pub source_environment: String,
    pub included_material: Vec<String>,
    pub excluded_material: Vec<String>,
    pub tenant_count: i64,
    pub tenant_state_summary: Vec<TenantStateSummary>,
    pub integrity_checks: Vec<String>,
    pub compatibility_notes: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RestoreVerificationResult {
    pub backup_artifact_id: String,
    pub restore_environment: String,
    pub tenant_record_checks_passed: i64,
    pub tenant_record_checks_total: i64,
    pub cross_tenant_leakage_observed: bool,
    pub raw_credential_material_found: bool,
    pub secret_reference_checks: Vec<String>,
    pub quota_state_checks: Vec<String>,
    pub work_state_checks: Vec<String>,
    pub credential_remediation_states: Vec<String>,
    pub invalid_backup_failed_clearly: bool,
    pub partial_restore_reported_passed: bool,
    pub result: String,
}

pub type WorkloadCoverage = HashMap<String, bool>;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RestartEvent {
    pub restart_id: String,
    pub unfinished_work: String,
    pub classification: String,
    pub recovery_time: Duration,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FaultDrillResult {
    pub fault_type: String,
    pub domain: String,
    pub observed_classification: String,
    pub retry_exhausted: bool,
    pub operator_action_needed: bool,
    pub contains_raw_credential_material: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResourceObservation {
    pub category: String,
    pub available: bool,
    pub monotonic_growth: bool,
    pub queue_backlog_age: Duration,
    pub operator_visibility: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SoakReport {
    pub report_id: String,
    pub branch_or_version: String,
    pub environment: String,
    pub data_directory: String,
    pub baseline_topology: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
    pub duration: Duration,
    pub temporary_shorter_duration: bool,
    pub temporary_duration_reason: String,
    pub follow_up_full_rerun: bool,
    pub tenant_set_summary: Vec<TenantStateSummary>,
    pub workload_coverage: WorkloadCoverage,
    pub restart_events: Vec<RestartEvent>,
    pub fault_drill_results: Vec<FaultDrillResult>,
    pub resource_observations: Vec<ResourceObservation>,
    pub cross_tenant_leakage: bool,
    pub unclassified_failures: Vec<String>,
    pub retry_exhaustion_summary: Vec<FaultDrillResult>,
    pub final_result: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RealAccountSmokeStatus {
    pub domain: String,
    pub safe_credentials_available: bool,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub result: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub skip_reason: String,
    pub fake_backend_coverage_passing: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub contains_raw_credential_material: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReleaseReadinessEvidence {
    pub install_runbook_passed: bool,
    pub upgrade_runbook_passed: bool,
    pub backup_artifact_passed: bool,
    pub restore_verification_passed: bool,
    pub migration_verification_passed: bool,
    pub rollback_guidance_present: bool,
    pub soak_report_passed: bool,
    pub resource_growth_checks_passed: bool,
    pub credential_redaction_passed: bool,
    pub fake_backend_coverage_passed: bool,
    pub roadmap40_rerun_gate_present: bool,
    pub roadmap41_rerun_gate_present: bool,
    pub roadmap42_diagnostics_present: bool,
    pub roadmap42_smoke_evidence_present: bool,
    pub review_elapsed: Duration,
    pub real_account_smoke: Vec<RealAccountSmokeStatus>,
    pub diagnostic_smoke_reports: Vec<SmokeMatrixReport>,
    pub decision: String,
}

// ---- hosted deployment model (hosted_types.go) ----

pub const HOSTED_HOST_CLASS_STABLE_TEST_HOST: &str = "stable_test_host";
pub const HOSTED_HOST_CLASS_VPS: &str = "vps";
pub const HOSTED_HOST_CLASS_DEVELOPER_LAPTOP: &str = "developer_laptop";
pub const HOSTED_HOST_CLASS_UNSUPPORTED: &str = "unsupported";

pub const HOSTED_LIVE_CONNECTORS_DISABLED: &str = "disabled";
pub const HOSTED_LIVE_CONNECTORS_LIVE: &str = "live";

pub const HOSTED_SUPERVISOR_MODE_REPO_FOREGROUND: &str = "repo_foreground";

pub const HOSTED_RUN_STATUS_PROVISIONING: &str = "provisioning";
pub const HOSTED_RUN_STATUS_RUNNING: &str = "running";
pub const HOSTED_RUN_STATUS_STOPPED: &str = "stopped";
pub const HOSTED_RUN_STATUS_FAILED: &str = "failed";
pub const HOSTED_RUN_STATUS_COMPLETED: &str = "completed";
pub const HOSTED_RUN_STATUS_EXPIRED: &str = "expired";

pub const HOSTED_EVENT_START: &str = "start";
pub const HOSTED_EVENT_STOP: &str = "stop";
pub const HOSTED_EVENT_RESTART: &str = "restart";
pub const HOSTED_EVENT_STATUS: &str = "status";
pub const HOSTED_EVENT_HEALTH_CHECK: &str = "health_check";
pub const HOSTED_EVENT_CRASH_DETECTED: &str = "crash_detected";
pub const HOSTED_EVENT_REBOOT_RECOVERY: &str = "reboot_recovery";
pub const HOSTED_EVENT_MANUAL_STOP: &str = "manual_stop";
pub const HOSTED_EVENT_FAILED_RESTART: &str = "failed_restart";
pub const HOSTED_EVENT_REPEATED_CRASH: &str = "repeated_crash";

pub const HOSTED_RESULT_PASSED: &str = "passed";
pub const HOSTED_RESULT_FAILED: &str = "failed";
pub const HOSTED_RESULT_BLOCKED: &str = "blocked";
pub const HOSTED_RESULT_UNSUPPORTED: &str = "unsupported";
pub const HOSTED_RESULT_OPERATOR_ACTION_NEEDED: &str = "operator_action_needed";

pub const HOSTED_REDACTION_PASSED: &str = "passed";
pub const HOSTED_REDACTION_FAILED: &str = "failed";

pub const FAILURE_OWNER_DAEMON: &str = "daemon";
pub const FAILURE_OWNER_HOST: &str = "host";
pub const FAILURE_OWNER_NETWORK: &str = "network";
pub const FAILURE_OWNER_PROVIDER: &str = "provider";
pub const FAILURE_OWNER_CREDENTIAL: &str = "credential";
pub const FAILURE_OWNER_QUOTA: &str = "quota";
pub const FAILURE_OWNER_OPERATOR_ACTION: &str = "operator_action";
pub const FAILURE_OWNER_UNSUPPORTED_OBSERVATION: &str = "unsupported_observation";
pub const FAILURE_OWNER_UNKNOWN: &str = "unknown";

pub const HOSTED_UPGRADE_PHASE_PREFLIGHT: &str = "preflight";
pub const HOSTED_UPGRADE_PHASE_POSTFLIGHT: &str = "postflight";

pub const HOSTED_ROLLBACK_IN_PLACE: &str = "in_place_rollback";
pub const HOSTED_ROLLBACK_RESTORE_FROM_BACKUP_REQUIRED: &str = "restore_from_backup_required";
pub const HOSTED_ROLLBACK_NO_ROLLBACK_NEEDED: &str = "no_rollback_needed";
pub const HOSTED_ROLLBACK_BLOCKED: &str = "blocked";

pub const REQUIRED_HOSTED_EVIDENCE_TYPES: &[&str] = &[
    "deployment_manifest",
    "configuration_profile",
    "health_checks",
    "logs",
    "soak_report",
    "backup_evidence",
    "restore_evidence",
    "upgrade_preflight",
    "upgrade_postflight",
    "rollback_decision",
    "integration_diagnostics",
    "resource_observations",
    "redaction_check",
    "retention_metadata",
];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct HostedOperationalProfile {
    pub profile_id: String,
    pub profile_name: String,
    pub environment: String,
    pub host_class: String,
    pub data_directory: String,
    pub log_directory: String,
    pub artifact_directory: String,
    pub backup_directory: String,
    pub report_directory: String,
    pub temporary_directory: String,
    pub live_connector_mode: String,
    pub retention_days: i64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct HostedRun {
    pub run_id: String,
    pub profile_id: String,
    pub commit_or_version: String,
    pub host: String,
    pub operator: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
    pub supervisor_mode: String,
    pub status: String,
    pub artifact_root: String,
    pub retention_expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostedDeploymentManifest {
    pub manifest_id: String,
    pub run_id: String,
    pub profile_id: String,
    pub commit_or_version: String,
    pub branch: String,
    pub host: String,
    pub operator: String,
    pub started_at: DateTime<Utc>,
    pub configuration_profile: String,
    pub data_directory: String,
    pub artifact_directory: String,
    pub supervisor_mode: String,
    pub daemon_address: String,
    pub live_connector_mode: String,
    pub redaction_status: String,
    pub retention_expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostedSupervisorEvent {
    pub event_id: String,
    pub run_id: String,
    pub event_type: String,
    pub requested_by: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
    pub daemon_health: String,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub recovery_seconds: i64,
    pub result: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub failure_owner: String,
    pub evidence_path: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostedEvidenceLink {
    pub evidence_type: String,
    pub path: String,
    pub run_id: String,
    pub profile_id: String,
    pub commit_or_version: String,
    pub status: String,
    pub generated_at: DateTime<Utc>,
    pub retention_expires_at: DateTime<Utc>,
    pub redaction_status: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unsupported_fields: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocking_findings: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HostedReleaseEvidenceIndex {
    pub release_index_id: String,
    pub run_id: String,
    pub profile_id: String,
    pub commit_or_version: String,
    pub generated_at: DateTime<Utc>,
    pub review_target: String,
    pub retention_expires_at: DateTime<Utc>,
    pub decision: String,
    pub review_elapsed: Duration,
    pub authorized_retention_policy: String,
    pub evidence_links: Vec<HostedEvidenceLink>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HostedBackupEvidence {
    pub backup_id: String,
    pub run_id: String,
    pub source_profile_id: String,
    pub source_commit_or_version: String,
    pub artifact_path: String,
    pub checksum: String,
    pub tenant_summary: Vec<TenantStateSummary>,
    pub included_material: Vec<String>,
    pub excluded_material: Vec<String>,
    pub compatibility_notes: Vec<String>,
    pub redaction_status: String,
    pub generated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HostedRestoreRehearsalResult {
    pub restore_result_id: String,
    pub run_id: String,
    pub backup_id: String,
    pub target_profile_id: String,
    pub target_data_directory: String,
    pub target_is_alternate: bool,
    pub tenant_count: i64,
    pub tenant_states: Vec<TenantStateSummary>,
    pub tenant_state_result: String,
    pub migration_state_result: String,
    pub credential_remediation_result: String,
    pub quota_state_result: String,
    pub daemon_health_result: String,
    pub cross_tenant_leakage: bool,
    pub raw_credential_scan_result: String,
    pub result: String,
    pub generated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HostedRollbackDecisionRecord {
    pub rollback_decision_id: String,
    pub run_id: String,
    pub trigger: String,
    pub decision: String,
    pub rationale: String,
    pub required_backup_id: String,
    pub supporting_evidence_links: Vec<String>,
    pub operator: String,
    pub decided_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HostedUpgradeEvidence {
    pub upgrade_evidence_id: String,
    pub run_id: String,
    pub phase: String,
    pub deployment_identity: String,
    pub profile_identity: String,
    pub data_location: String,
    pub artifact_location: String,
    pub required_backup_state: String,
    pub daemon_health: String,
    pub configuration_readiness: String,
    pub tenant_data_verification: String,
    pub migration_state: String,
    pub credential_remediation_state: String,
    pub quota_state: String,
    pub operational_diagnostics: String,
    pub rollback_guidance: String,
    pub failure_owner: String,
    pub blocking_findings: Vec<String>,
    pub generated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostedObservation {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub value: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub unsupported: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HostedObservationReport {
    pub observation_report_id: String,
    pub run_id: String,
    pub sample_window: String,
    pub daemon_health: String,
    pub database_size: HostedObservation,
    pub log_size: HostedObservation,
    pub memory: HostedObservation,
    pub goroutines: HostedObservation,
    pub file_descriptors: HostedObservation,
    pub queue_or_backlog: HostedObservation,
    pub connector_health: HostedObservation,
    pub mcp_health: HostedObservation,
    pub integration_diagnostic_state: HostedObservation,
    pub unsupported_fields: Vec<String>,
    pub monotonic_resource_growth: bool,
    pub failure_owner: String,
    pub blocking_findings: Vec<String>,
    pub generated_at: DateTime<Utc>,
}

#[must_use]
fn is_zero_i64(v: &i64) -> bool {
    *v == 0
}

#[must_use]
fn is_false(v: &bool) -> bool {
    !*v
}
