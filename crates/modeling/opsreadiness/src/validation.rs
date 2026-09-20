//! Opsreadiness validation logic (port of validation.go, launch_gate.go, release_readiness.go,
//! backup_artifact.go, restore_validation.go, rollback_evidence.go, restart_recovery.go,
//! resource_observation.go, restore_credentials.go, soak_report.go, migration_evidence.go,
//! real_account_smoke.go, calendar_smoke.go, mail_smoke.go, and fixtures.go).

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::{
    BackupArtifact, FaultDrillResult, MigrationVerificationReport, RealAccountSmokeStatus,
    ReleaseReadinessEvidence, ResourceObservation, RestartEvent, RestoreVerificationResult,
    RollbackDecision, RunbookEvidence, SoakReport, TenantStateSummary, WorkloadCoverage,
};

pub const MAX_INSTALL_ELAPSED: Duration = Duration::from_secs(60 * 60);
pub const MAX_UPGRADE_ELAPSED: Duration = Duration::from_secs(90 * 60);
pub const MAX_RELEASE_REVIEW_ELAPSED: Duration = Duration::from_secs(30 * 60);
pub const MAX_RESTART_RECOVERY_ELAPSED: Duration = Duration::from_secs(5 * 60);
pub const MAX_QUEUE_BACKLOG_AGE: Duration = Duration::from_secs(30 * 60);
pub const MINIMUM_SOAK_DURATION: Duration = Duration::from_secs(24 * 3600);
pub const MINIMUM_RESTART_COUNT: usize = 3;
pub const MINIMUM_TENANT_COUNT: usize = 3;

pub const RAW_CREDENTIAL_MARKERS: &[&str] = &[
    "raw_secret",
    "access_token",
    "refresh_token",
    "oauth_code",
    "provider_token",
    "r37_fake_secret",
    "r37_fake_token",
    "r39_raw_secret",
    "do_not_leak",
];

pub const REQUIRED_LAUNCH_WORKLOADS: &[&str] = &[
    "activation",
    "setup",
    "channels",
    "sessions",
    "profile_binding",
    "routines",
    "webhooks",
    "quota_denial",
    "diagnostics",
    "evaluation",
    "live_validation",
    "support_bundle",
    "backup",
    "restore",
    "upgrade",
    "rollback",
];

pub const LAUNCH_GATE_STATEMENT: &str = "Context, knowledge, and memory work may begin only after non-knowledge parity release evidence passes or residual exceptions are explicitly accepted.";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkloadEvidence {
    pub name: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub owner: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

// serde(default): Go decodes a caller-supplied evidence index into the zero
// value, so absent fields must degrade to empty/false instead of rejecting
// the document at the API boundary.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LaunchGateEvidence {
    pub channels: Vec<RealAccountSmokeStatus>,
    pub provider_smoke: Vec<RealAccountSmokeStatus>,
    pub workloads: Vec<WorkloadEvidence>,
    pub soak_duration_met: bool,
    pub support_bundle_validated: bool,
    pub redaction_validated: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchDecision {
    pub result: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<String>,
    pub non_knowledge_parity_complete: bool,
    pub gate_statement: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CalendarSmokeInput {
    pub safe_credentials_available: bool,
    pub enabled: bool,
    pub create_update_cancel_exercised: bool,
    pub fake_backend_coverage_passing: bool,
    pub contains_raw_credential_material: bool,
    pub skip_reason: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MailSmokeInput {
    pub safe_credentials_available: bool,
    pub enabled: bool,
    pub send_reply_forward_exercised: bool,
    pub fake_backend_coverage_passing: bool,
    pub contains_raw_credential_material: bool,
    pub skip_reason: String,
}

const DEFAULT_CALENDAR_SKIP_REASON: &str =
    "safe Feishu/Lark calendar credentials unavailable in this environment";
const DEFAULT_MAIL_SKIP_REASON: &str =
    "safe Feishu/Lark mail credentials unavailable in this environment";

// ---- primitive validation helpers ----

pub fn require_non_empty(label: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        Err(format!("{label} is required"))
    } else {
        Ok(())
    }
}

pub fn require_items(label: &str, values: &[String]) -> Result<(), String> {
    if values.is_empty() {
        return Err(format!("{label} requires at least one item"));
    }
    for (i, value) in values.iter().enumerate() {
        if value.trim().is_empty() {
            return Err(format!("{label}[{i}] is empty"));
        }
    }
    Ok(())
}

pub fn require_elapsed_at_most(
    label: &str,
    elapsed: Duration,
    max: Duration,
) -> Result<(), String> {
    if elapsed <= Duration::ZERO {
        return Err(format!("{label} elapsed time is required"));
    }
    if elapsed > max {
        return Err(format!("{label} elapsed time {elapsed:?} exceeds {max:?}"));
    }
    Ok(())
}

pub fn contains_raw_credential_material(value: &str) -> bool {
    let lower = value.to_lowercase();
    RAW_CREDENTIAL_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
}

pub fn contains_any_raw_credential_material(values: &[String]) -> bool {
    values
        .iter()
        .any(|value| contains_raw_credential_material(value))
}

pub fn join_errors(results: Vec<Result<(), String>>) -> Result<(), String> {
    let errors: Vec<String> = results.into_iter().filter_map(|r| r.err()).collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

pub fn require_allowed(label: &str, value: &str, allowed: &[&str]) -> Result<(), String> {
    if allowed.iter().any(|item| item == &value) {
        Ok(())
    } else {
        Err(format!("{label} {value:?} is not one of {allowed:?}"))
    }
}

fn require_coverage(
    label: &str,
    coverage: &HashMap<String, bool>,
    required: &[&str],
) -> Result<(), String> {
    let missing: Vec<&str> = required
        .iter()
        .filter(|key| !coverage.get(**key).copied().unwrap_or(false))
        .copied()
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{label} missing required coverage: {}",
            missing.join(", ")
        ))
    }
}

fn require_no_raw_credential_slice(label: &str, values: &[String]) -> Result<(), String> {
    if contains_any_raw_credential_material(values) {
        Err(format!("{label} contains raw credential material"))
    } else {
        Ok(())
    }
}

// ---- domain validations ----

pub fn validate_release_readiness(evidence: &ReleaseReadinessEvidence) -> Result<(), String> {
    let mut results = vec![
        require_elapsed_at_most(
            "release readiness review",
            evidence.review_elapsed,
            MAX_RELEASE_REVIEW_ELAPSED,
        ),
        validate_real_account_smoke(&evidence.real_account_smoke),
    ];
    let required: Vec<(&str, bool)> = vec![
        ("install runbook", evidence.install_runbook_passed),
        ("upgrade runbook", evidence.upgrade_runbook_passed),
        ("backup artifact", evidence.backup_artifact_passed),
        ("restore verification", evidence.restore_verification_passed),
        (
            "migration verification",
            evidence.migration_verification_passed,
        ),
        ("rollback guidance", evidence.rollback_guidance_present),
        ("soak report", evidence.soak_report_passed),
        (
            "resource growth checks",
            evidence.resource_growth_checks_passed,
        ),
        ("credential redaction", evidence.credential_redaction_passed),
        (
            "fake-backend coverage",
            evidence.fake_backend_coverage_passed,
        ),
        (
            "Roadmap 40 rerun gate",
            evidence.roadmap40_rerun_gate_present,
        ),
        (
            "Roadmap 41 rerun gate",
            evidence.roadmap41_rerun_gate_present,
        ),
        (
            "Roadmap 42 diagnostics",
            evidence.roadmap42_diagnostics_present,
        ),
        (
            "Roadmap 42 smoke",
            evidence.roadmap42_smoke_evidence_present,
        ),
    ];
    for (label, ok) in required {
        if !ok {
            results.push(Err(format!("{label} is required for release readiness")));
        }
    }
    if evidence.decision != crate::RESULT_SHIP
        && evidence.decision != crate::RESULT_SHIP_WITH_RECORDED_SKIPS
    {
        results.push(Err(
            "release decision must be ship or ship_with_recorded_skips when evidence passes"
                .to_string(),
        ));
    }
    if evidence.roadmap42_smoke_evidence_present && evidence.diagnostic_smoke_reports.is_empty() {
        results.push(Err(
            "Roadmap 42 smoke evidence requires at least one diagnostic smoke report".to_string(),
        ));
    }
    join_errors(results)
}

pub fn validate_backup_artifact(artifact: &BackupArtifact) -> Result<(), String> {
    let mut results = vec![
        require_non_empty("artifact id", &artifact.artifact_id),
        require_non_empty("source version", &artifact.source_version),
        require_non_empty("source environment", &artifact.source_environment),
        require_items("included material", &artifact.included_material),
        require_items("excluded material", &artifact.excluded_material),
        require_items("integrity checks", &artifact.integrity_checks),
        validate_representative_tenants(artifact.tenant_count, &artifact.tenant_state_summary),
        require_no_raw_credential_slice("included material", &artifact.included_material),
        require_no_raw_credential_slice("integrity checks", &artifact.integrity_checks),
        require_credential_exclusions(&artifact.excluded_material),
    ];
    for (i, tenant) in artifact.tenant_state_summary.iter().enumerate() {
        results.push(validate_tenant_state_summary(
            &format!("tenant state summary[{i}]"),
            tenant,
        ));
    }
    join_errors(results)
}

pub fn validate_representative_tenants(
    count: i64,
    tenants: &[TenantStateSummary],
) -> Result<(), String> {
    if count < MINIMUM_TENANT_COUNT as i64 || tenants.len() < MINIMUM_TENANT_COUNT {
        return Err(format!(
            "representative backup requires at least {MINIMUM_TENANT_COUNT} tenants"
        ));
    }
    let mut seen: HashSet<&str> = HashSet::new();
    for tenant in tenants {
        if !seen.insert(tenant.tenant_id.as_str()) {
            return Err(format!("duplicate tenant {:?}", tenant.tenant_id));
        }
    }
    Ok(())
}

pub fn validate_tenant_state_summary(
    label: &str,
    tenant: &TenantStateSummary,
) -> Result<(), String> {
    join_errors(vec![
        require_non_empty(&format!("{label}.tenant id"), &tenant.tenant_id),
        require_items(&format!("{label}.credential refs"), &tenant.credential_refs),
        require_non_empty(&format!("{label}.quota state"), &tenant.quota_state),
        require_non_empty(&format!("{label}.work state"), &tenant.work_state),
        require_no_raw_credential_slice(
            &format!("{label}.credential refs"),
            &tenant.credential_refs,
        ),
    ])
}

fn require_credential_exclusions(values: &[String]) -> Result<(), String> {
    let required = [
        "raw secret",
        "access token",
        "refresh token",
        "oauth",
        "provider token",
        "derived credential",
    ];
    let joined = values
        .join(
            "
",
        )
        .to_lowercase();
    let missing: Vec<&str> = required
        .iter()
        .filter(|item| !joined.contains(**item))
        .copied()
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "excluded material missing credential exclusions: {}",
            missing.join(", ")
        ))
    }
}

pub fn validate_restore_result(result: &RestoreVerificationResult) -> Result<(), String> {
    let mut results = vec![
        require_non_empty("backup artifact id", &result.backup_artifact_id),
        require_non_empty("restore environment", &result.restore_environment),
        require_items("secret reference checks", &result.secret_reference_checks),
        require_items("quota state checks", &result.quota_state_checks),
        require_items("work state checks", &result.work_state_checks),
        require_items(
            "credential remediation states",
            &result.credential_remediation_states,
        ),
        require_non_empty("restore result", &result.result),
    ];
    if result.tenant_record_checks_total <= 0
        || result.tenant_record_checks_passed != result.tenant_record_checks_total
    {
        results.push(Err("100% of tenant record checks must pass".to_string()));
    }
    if result.cross_tenant_leakage_observed {
        results.push(Err("cross-tenant leakage observed".to_string()));
    }
    if result.raw_credential_material_found {
        results.push(Err("raw credential material found".to_string()));
    }
    if result.partial_restore_reported_passed {
        results.push(Err("partial restore reported as passed".to_string()));
    }
    if !result.invalid_backup_failed_clearly {
        results.push(Err("invalid backup behavior is not proven".to_string()));
    }
    join_errors(results)
}

pub fn validate_rollback_decision(decision: &RollbackDecision) -> Result<(), String> {
    require_non_empty("rollback path", &decision.selected_path)?;
    if !decision.persisted_state_reversible && decision.selected_path != "restore_from_backup" {
        return Err("irreversible persisted state must use restore_from_backup".to_string());
    }
    if decision.selected_path == "restore_from_backup" && !decision.backup_verified {
        return Err("restore_from_backup requires a verified backup".to_string());
    }
    require_non_empty("rollback reason", &decision.reason)
}

pub fn validate_restart_recovery(events: &[RestartEvent]) -> Result<(), String> {
    if events.len() < MINIMUM_RESTART_COUNT {
        return Err(format!(
            "soak requires at least {MINIMUM_RESTART_COUNT} daemon restarts"
        ));
    }
    for event in events {
        require_allowed(
            "restart classification",
            &event.classification,
            &[
                crate::CLASSIFICATION_RECOVERED,
                crate::CLASSIFICATION_INTERRUPTED,
                crate::CLASSIFICATION_RETRIED,
                crate::CLASSIFICATION_OPERATOR_ACTION_NEEDED,
            ],
        )?;
        if event.recovery_time > MAX_RESTART_RECOVERY_ELAPSED {
            return Err(format!(
                "restart {} recovery {:?} exceeds {:?}",
                event.restart_id, event.recovery_time, MAX_RESTART_RECOVERY_ELAPSED
            ));
        }
    }
    Ok(())
}

pub fn validate_resource_observations(observations: &[ResourceObservation]) -> Result<(), String> {
    let mut coverage: HashMap<String, bool> = HashMap::new();
    for observation in observations {
        if observation.available {
            coverage.insert(observation.category.clone(), true);
        }
        if observation.monotonic_growth {
            return Err(format!(
                "resource category {} grew monotonically",
                observation.category
            ));
        }
        if observation.category == "active_work_or_queue_backlog"
            && observation.queue_backlog_age > MAX_QUEUE_BACKLOG_AGE
        {
            return Err(format!(
                "queue backlog persisted for {:?}",
                observation.queue_backlog_age
            ));
        }
    }
    require_coverage(
        "resource observations",
        &coverage,
        crate::REQUIRED_RESOURCE_CATEGORIES,
    )
}

pub fn validate_credential_remediation(states: &[String]) -> Result<(), String> {
    require_items("credential remediation states", states)?;
    for state in states {
        if state != "reconnect_required"
            && state != "revalidation_required"
            && state != "blocked_until_reconnected"
        {
            return Err(format!(
                "credential remediation state {state:?} does not block credential-bearing use"
            ));
        }
    }
    Ok(())
}

pub fn validate_soak_workload(coverage: &WorkloadCoverage) -> Result<(), String> {
    require_coverage("soak workload", coverage, crate::REQUIRED_WORKLOAD_AREAS)
}

pub fn validate_soak_report(report: &SoakReport) -> Result<(), String> {
    let mut results = vec![
        require_non_empty("report id", &report.report_id),
        require_non_empty("branch or version", &report.branch_or_version),
        require_non_empty("environment", &report.environment),
        require_non_empty("data directory", &report.data_directory),
        require_non_empty("baseline topology", &report.baseline_topology),
        validate_soak_workload(&report.workload_coverage),
        validate_restart_recovery(&report.restart_events),
        validate_fault_drills(&report.fault_drill_results),
        validate_resource_observations(&report.resource_observations),
        validate_representative_tenants(
            report.tenant_set_summary.len() as i64,
            &report.tenant_set_summary,
        ),
    ];
    if report.baseline_topology != crate::TOPOLOGY_TENANT_SCOPED_SINGLE_NODE {
        results.push(Err(format!(
            "baseline topology must be {}",
            crate::TOPOLOGY_TENANT_SCOPED_SINGLE_NODE
        )));
    }
    if report.environment != crate::ENVIRONMENT_TEST {
        results.push(Err(format!(
            "default soak environment must be {}",
            crate::ENVIRONMENT_TEST
        )));
    }
    if report.duration < MINIMUM_SOAK_DURATION {
        if !report.temporary_shorter_duration
            || report.temporary_duration_reason.is_empty()
            || !report.follow_up_full_rerun
        {
            results.push(Err(format!("soak duration {:?} is shorter than {:?} without temporary threshold rationale and full rerun requirement", report.duration, MINIMUM_SOAK_DURATION)));
        }
    }
    if report.cross_tenant_leakage {
        results.push(Err("cross-tenant leakage observed".to_string()));
    }
    if !report.unclassified_failures.is_empty() {
        results.push(Err(format!(
            "unclassified failures observed: {:?}",
            report.unclassified_failures
        )));
    }
    if report.final_result != crate::STATUS_PASS {
        results.push(Err("final result must be pass".to_string()));
    }
    join_errors(results)
}

pub fn validate_fault_drills(results: &[FaultDrillResult]) -> Result<(), String> {
    let mut coverage: HashMap<String, bool> = HashMap::new();
    for result in results {
        coverage.insert(result.fault_type.clone(), true);
        require_allowed(
            "fault classification",
            &result.observed_classification,
            &[
                crate::CLASSIFICATION_RECOVERED,
                crate::CLASSIFICATION_RETRY_EXHAUSTED,
                crate::CLASSIFICATION_OPERATOR_ACTION_NEEDED,
            ],
        )?;
        if result.retry_exhausted && !result.operator_action_needed {
            return Err(format!(
                "retry exhaustion for {} lacks operator-action-needed state",
                result.fault_type
            ));
        }
        if result.contains_raw_credential_material {
            return Err(format!(
                "fault drill {} exposed raw credential material",
                result.fault_type
            ));
        }
    }
    require_coverage("fault drills", &coverage, crate::REQUIRED_FAULT_TYPES)
}

pub fn validate_real_account_smoke(statuses: &[RealAccountSmokeStatus]) -> Result<(), String> {
    if statuses.is_empty() {
        return Err("real-account smoke status is required for each supported domain".to_string());
    }
    for status in statuses {
        require_non_empty("real-account smoke domain", &status.domain)?;
        if status.contains_raw_credential_material {
            return Err(format!(
                "real-account smoke for {} exposed raw credential material",
                status.domain
            ));
        }
        if !status.fake_backend_coverage_passing {
            return Err(format!(
                "fake-backend coverage must pass for {}",
                status.domain
            ));
        }
        if status.safe_credentials_available && !status.enabled {
            return Err(format!(
                "safe credentials available for {} but smoke is not enabled",
                status.domain
            ));
        }
        if status.safe_credentials_available
            && status.enabled
            && status.result != crate::STATUS_PASS
        {
            return Err(format!(
                "real-account smoke for {} must pass when safe credentials are used",
                status.domain
            ));
        }
        if !status.safe_credentials_available && status.skip_reason.is_empty() {
            return Err(format!(
                "missing safe credentials for {} require explicit skip reason",
                status.domain
            ));
        }
    }
    Ok(())
}

pub fn validate_migration_evidence(report: &MigrationVerificationReport) -> Result<(), String> {
    join_errors(vec![
        require_non_empty("source version", &report.source_version),
        require_non_empty("target version", &report.target_version),
        require_items("preflight checks", &report.preflight_checks),
        require_items("postflight checks", &report.postflight_checks),
        require_non_empty("migration progress", &report.migration_progress),
        require_non_empty("tenant integrity summary", &report.tenant_integrity_summary),
        require_non_empty("quota accounting summary", &report.quota_accounting_summary),
        require_non_empty("credential remediation", &report.credential_remediation),
        require_non_empty("rollback path", &report.rollback_path),
        require_non_empty("result", &report.result),
        require_items("operator diagnostics", &report.operator_diagnostics),
    ])
}

pub fn validate_upgrade_runbook(evidence: &RunbookEvidence) -> Result<(), String> {
    join_errors(vec![
        require_non_empty("runbook name", &evidence.name),
        require_items("upgrade steps", &evidence.steps),
        require_elapsed_at_most("upgrade", evidence.elapsed, MAX_UPGRADE_ELAPSED),
        require_items("health checks", &evidence.health_checks),
        require_items("diagnostics", &evidence.diagnostics),
        require_items("failure modes", &evidence.failure_modes),
        require_items("rollback decision points", &evidence.rollback_or_cleanup),
        require_no_production_data("upgrade runbook", evidence),
    ])
}

pub fn validate_install_runbook(evidence: &RunbookEvidence) -> Result<(), String> {
    join_errors(vec![
        require_non_empty("runbook name", &evidence.name),
        require_items("install steps", &evidence.steps),
        require_elapsed_at_most("install", evidence.elapsed, MAX_INSTALL_ELAPSED),
        require_items("health checks", &evidence.health_checks),
        require_items("diagnostics", &evidence.diagnostics),
        require_items("failure modes", &evidence.failure_modes),
        require_items("cleanup", &evidence.rollback_or_cleanup),
        require_no_production_data("install runbook", evidence),
    ])
}

fn require_no_production_data(label: &str, evidence: &RunbookEvidence) -> Result<(), String> {
    if evidence.used_production_data && !evidence.production_opt_in {
        Err(format!(
            "{label} used production data without explicit opt-in"
        ))
    } else {
        Ok(())
    }
}

pub fn validate_launch_gate(evidence: &LaunchGateEvidence) -> LaunchDecision {
    let mut reasons: Vec<String> = Vec::new();

    let mut by_name: HashMap<String, &WorkloadEvidence> = HashMap::new();
    for workload in &evidence.workloads {
        by_name.insert(workload.name.trim().to_string(), workload);
    }
    for &required in REQUIRED_LAUNCH_WORKLOADS {
        match by_name.get(required) {
            None => reasons.push(format!("missing required workload evidence: {required}")),
            Some(w) if w.status == crate::STATUS_FAIL => {
                reasons.push(format!("required workload failed: {required}"))
            }
            Some(w) if w.status == crate::STATUS_SKIP && w.reason.trim().is_empty() => reasons
                .push(format!(
                    "skipped workload requires an accepted reason: {required}"
                )),
            Some(w) if w.status != crate::STATUS_PASS && w.status != crate::STATUS_SKIP => reasons
                .push(format!(
                    "workload has invalid status {:?}: {required}",
                    w.status
                )),
            _ => {}
        }
    }

    if evidence.channels.len() < 3 {
        reasons.push(format!(
            "launch gate requires at least 3 channel entries, got {}",
            evidence.channels.len()
        ));
    }
    if validate_real_account_smoke(&evidence.channels).is_err() && !evidence.channels.is_empty() {
        reasons.push(format!(
            "channel smoke invalid: {}",
            validate_real_account_smoke(&evidence.channels).unwrap_err()
        ));
    }

    let mut provider_domains: HashSet<String> = HashSet::new();
    for p in &evidence.provider_smoke {
        provider_domains.insert(p.domain.trim().to_string());
    }
    for domain in ["calendar", "mail"] {
        if !provider_domains.contains(domain) {
            reasons.push(format!("missing {domain} provider smoke entry"));
        }
    }
    if validate_real_account_smoke(&evidence.provider_smoke).is_err()
        && !evidence.provider_smoke.is_empty()
    {
        reasons.push(format!(
            "provider smoke invalid: {}",
            validate_real_account_smoke(&evidence.provider_smoke).unwrap_err()
        ));
    }

    if !evidence.soak_duration_met {
        reasons.push("full-duration hosted soak not met".to_string());
    }
    if !evidence.support_bundle_validated {
        reasons.push("support bundle generation/validation not exercised during soak".to_string());
    }
    if !evidence.redaction_validated {
        reasons.push("redaction validation not exercised during soak".to_string());
    }

    reasons.sort();
    let mut decision = LaunchDecision {
        result: crate::RESULT_SHIP.to_string(),
        non_knowledge_parity_complete: true,
        gate_statement: LAUNCH_GATE_STATEMENT.to_string(),
        ..LaunchDecision::default()
    };
    if !reasons.is_empty() {
        decision.result = crate::RESULT_NO_SHIP.to_string();
        decision.reasons = reasons;
        decision.non_knowledge_parity_complete = false;
    }
    decision
}

pub fn calendar_real_account_smoke(input: &CalendarSmokeInput) -> RealAccountSmokeStatus {
    let mut status = RealAccountSmokeStatus {
        domain: "calendar".to_string(),
        safe_credentials_available: input.safe_credentials_available,
        enabled: input.enabled,
        fake_backend_coverage_passing: input.fake_backend_coverage_passing,
        contains_raw_credential_material: input.contains_raw_credential_material,
        ..RealAccountSmokeStatus::default()
    };
    if input.safe_credentials_available && input.enabled && input.create_update_cancel_exercised {
        status.result = crate::STATUS_PASS.to_string();
    } else if input.safe_credentials_available && input.enabled {
        status.result = crate::STATUS_FAIL.to_string();
    } else {
        status.result = crate::STATUS_SKIP.to_string();
        status.skip_reason = input.skip_reason.trim().to_string();
        if status.skip_reason.is_empty() {
            status.skip_reason = DEFAULT_CALENDAR_SKIP_REASON.to_string();
        }
    }
    status
}

pub fn mail_real_account_smoke(input: &MailSmokeInput) -> RealAccountSmokeStatus {
    let mut status = RealAccountSmokeStatus {
        domain: "mail".to_string(),
        safe_credentials_available: input.safe_credentials_available,
        enabled: input.enabled,
        fake_backend_coverage_passing: input.fake_backend_coverage_passing,
        contains_raw_credential_material: input.contains_raw_credential_material,
        ..RealAccountSmokeStatus::default()
    };
    if input.safe_credentials_available && input.enabled && input.send_reply_forward_exercised {
        status.result = crate::STATUS_PASS.to_string();
    } else if input.safe_credentials_available && input.enabled {
        status.result = crate::STATUS_FAIL.to_string();
    } else {
        status.result = crate::STATUS_SKIP.to_string();
        status.skip_reason = input.skip_reason.trim().to_string();
        if status.skip_reason.is_empty() {
            status.skip_reason = DEFAULT_MAIL_SKIP_REASON.to_string();
        }
    }
    status
}

pub fn load_json_fixture<T: DeserializeOwned>(path: &str) -> Result<T, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("read fixture {path}: {e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("decode fixture {path}: {e}"))
}

pub fn load_hosted_evidence_fixture<T: DeserializeOwned>(path: &str) -> Result<T, String> {
    load_json_fixture(path)
}
