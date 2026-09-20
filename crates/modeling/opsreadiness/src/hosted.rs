//! Opsreadiness hosted-deployment evidence validation and integration diagnostic smoke
//! (port of hosted_*.go and integration_diagnostic_smoke*.go).

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use kura_integrations::DiagnosticReasonCode;
use serde::{Deserialize, Serialize};

use crate::{
    HostedBackupEvidence, HostedDeploymentManifest, HostedEvidenceLink, HostedObservation,
    HostedObservationReport, HostedOperationalProfile, HostedReleaseEvidenceIndex,
    HostedRestoreRehearsalResult, HostedRollbackDecisionRecord, HostedRun, HostedSupervisorEvent,
    HostedUpgradeEvidence, MAX_INSTALL_ELAPSED, MAX_RELEASE_REVIEW_ELAPSED,
    MAX_RESTART_RECOVERY_ELAPSED, MINIMUM_TENANT_COUNT, join_errors, require_allowed,
    require_elapsed_at_most, require_items, require_non_empty, validate_representative_tenants,
    validate_tenant_state_summary,
};

macro_rules! string_enum {
    ($name:ident { $first:ident => $first_s:literal $(, $v:ident => $s:literal)* $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
        pub enum $name {
            #[default]
            #[serde(rename = $first_s)]
            $first,
            $(#[serde(rename = $s)] $v),*
        }
        impl $name {
            #[must_use]
            pub fn as_str(self) -> &'static str {
                match self {
                    $name::$first => $first_s,
                    $( $name::$v => $s ),*
                }
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

string_enum!(SmokeReportStatus {
    Draft => "draft",
    Running => "running",
    Completed => "completed",
    Blocked => "blocked",
    Failed => "failed",
    Published => "published",
});

string_enum!(SmokeProbeResult {
    Passed => "passed",
    Failed => "failed",
    Blocked => "blocked",
    Skipped => "skipped",
});

string_enum!(SmokeBlockedReason {
    MissingSafeCredentials => "missing_safe_credentials",
    UnsafeSideEffectScope => "unsafe_side_effect_scope",
    TenantApprovalUnavailable => "tenant_approval_unavailable",
    ProviderOutage => "provider_outage",
    UnsupportedDomain => "unsupported_domain",
    OperatorDeferred => "operator_deferred",
    MissingTenantAdminApproval => "missing_tenant_admin_approval",
    MissingOperatorApproval => "missing_operator_approval",
    RedactionFailedClosed => "redaction_failed_closed",
});

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmokeMatrixReport {
    pub smoke_report_id: String,
    pub tenant_id: String,
    pub report_kind: String,
    pub requested_by: String,
    pub status: SmokeReportStatus,
    pub domain_summary: HashMap<String, String>,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<DateTime<Utc>>,
    pub artifact_refs: Vec<String>,
    pub retention_expires_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub probe_outcomes: Vec<SmokeProbeOutcome>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmokeProbeOutcome {
    pub probe_outcome_id: String,
    pub tenant_id: String,
    pub smoke_report_id: String,
    pub integration_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub integration_account_id: String,
    pub domain_kind: String,
    pub provider_kind: String,
    pub probe_action: String,
    pub result: SmokeProbeResult,
    pub reason_code: String,
    pub remediation_hint: String,
    pub retry_safety: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub blocked_or_skipped_reason: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approval_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_refs: Vec<String>,
    pub checked_at: DateTime<Utc>,
    pub redaction_status: String,
    pub retention_expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default)]
pub struct SmokeProbeInput {
    pub tenant_id: String,
    pub integration_id: String,
    pub integration_account_id: String,
    pub domain_kind: String,
    pub provider_kind: String,
    pub probe_action: String,
    pub requested_by: String,
    pub safe_credentials_available: bool,
    pub tenant_approval_available: bool,
    pub provider_available: bool,
    pub supported: bool,
    pub read_only_or_reversible: bool,
    pub tenant_admin_approved: bool,
    pub operator_approved: bool,
    pub operator_deferred: bool,
    pub reason_code: DiagnosticReasonCode,
    pub artifact_refs: Vec<String>,
    pub checked_at: DateTime<Utc>,
}

pub const HOSTED_RAW_CREDENTIAL_MARKERS: &[&str] = &[
    "raw_secret",
    "access_token",
    "refresh_token",
    "oauth_code",
    "provider_token",
    "authorization:",
    "bearer ",
    "client_secret",
    "api_key=",
    "password=",
    "do_not_leak",
];

// ---- hosted identity / run helpers ----

pub fn generate_hosted_run_id(
    profile_id: &str,
    started_at: DateTime<Utc>,
) -> Result<String, String> {
    if profile_id.trim().is_empty() {
        return Err("profile id is required".to_string());
    }
    if started_at == DateTime::<Utc>::default() {
        return Err("started at is required".to_string());
    }
    Ok(format!(
        "{}_{}",
        sanitize_hosted_identity(profile_id),
        started_at.format("%Y%m%dT%H%M%SZ")
    ))
}

fn sanitize_hosted_identity(value: &str) -> String {
    value
        .trim()
        .to_lowercase()
        .replace(' ', "_")
        .replace('/', "_")
        .replace(char::from(92), "_")
        .replace(':', "_")
}

fn require_hosted_run_identity(
    run: &HostedRun,
    run_id: &str,
    profile_id: &str,
    commit_or_version: &str,
) -> Result<(), String> {
    let mut results = Vec::new();
    if run_id != run.run_id {
        results.push(Err(format!(
            "evidence identity run {:?} does not match {:?}",
            run_id, run.run_id
        )));
    }
    if !profile_id.is_empty() && profile_id != run.profile_id {
        results.push(Err(format!(
            "evidence identity profile {:?} does not match {:?}",
            profile_id, run.profile_id
        )));
    }
    if !commit_or_version.is_empty() && commit_or_version != run.commit_or_version {
        results.push(Err(format!(
            "evidence identity commit {:?} does not match {:?}",
            commit_or_version, run.commit_or_version
        )));
    }
    join_errors(results)
}

fn require_generated_at(label: &str, value: DateTime<Utc>) -> Result<(), String> {
    if value == DateTime::<Utc>::default() {
        Err(format!("{label} generated timestamp is required"))
    } else {
        Ok(())
    }
}

fn require_status_pass(label: &str, value: &str) -> Result<(), String> {
    if value != crate::STATUS_PASS && value != crate::HOSTED_RESULT_PASSED {
        Err(format!("{label} must pass, got {value:?}"))
    } else {
        Ok(())
    }
}

fn validate_hosted_retention(
    label: &str,
    expires_at: DateTime<Utc>,
    authorized_policy: &str,
) -> Result<(), String> {
    if expires_at == DateTime::<Utc>::default() {
        return Err(format!("{label} retention expiry is required"));
    }
    if expires_at <= Utc::now() && authorized_policy.trim().is_empty() {
        return Err(format!("{label} evidence expired for normal inspection"));
    }
    Ok(())
}

pub fn validate_hosted_failure_owner(owner: &str) -> Result<(), String> {
    require_allowed(
        "failure owner",
        owner,
        &[
            crate::FAILURE_OWNER_DAEMON,
            crate::FAILURE_OWNER_HOST,
            crate::FAILURE_OWNER_NETWORK,
            crate::FAILURE_OWNER_PROVIDER,
            crate::FAILURE_OWNER_CREDENTIAL,
            crate::FAILURE_OWNER_QUOTA,
            crate::FAILURE_OWNER_OPERATOR_ACTION,
            crate::FAILURE_OWNER_UNSUPPORTED_OBSERVATION,
            crate::FAILURE_OWNER_UNKNOWN,
        ],
    )
}

pub fn validate_hosted_redaction<T: Serialize>(label: &str, payload: &T) -> Result<(), String> {
    let raw = serde_json::to_string(payload)
        .map_err(|e| format!("{label} redaction payload cannot be encoded: {e}"))?;
    let body = raw.to_lowercase();
    for marker in HOSTED_RAW_CREDENTIAL_MARKERS {
        if body.contains(&marker.to_lowercase()) {
            return Err(format!("{label} contains raw credential material"));
        }
    }
    Ok(())
}

// ---- hosted profile / run validations ----

pub fn validate_hosted_profile(profile: &HostedOperationalProfile) -> Result<(), String> {
    let mut results = vec![
        require_non_empty("profile id", &profile.profile_id),
        require_non_empty("profile name", &profile.profile_name),
        require_allowed(
            "environment",
            &profile.environment,
            &[crate::ENVIRONMENT_TEST],
        ),
        require_allowed(
            "host class",
            &profile.host_class,
            &[
                crate::HOSTED_HOST_CLASS_STABLE_TEST_HOST,
                crate::HOSTED_HOST_CLASS_VPS,
                crate::HOSTED_HOST_CLASS_DEVELOPER_LAPTOP,
                crate::HOSTED_HOST_CLASS_UNSUPPORTED,
            ],
        ),
        require_non_empty("data directory", &profile.data_directory),
        require_non_empty("log directory", &profile.log_directory),
        require_non_empty("artifact directory", &profile.artifact_directory),
        require_non_empty("backup directory", &profile.backup_directory),
        require_non_empty("report directory", &profile.report_directory),
        require_non_empty("temporary directory", &profile.temporary_directory),
        require_allowed(
            "live connector mode",
            &profile.live_connector_mode,
            &[
                crate::HOSTED_LIVE_CONNECTORS_DISABLED,
                crate::HOSTED_LIVE_CONNECTORS_LIVE,
            ],
        ),
    ];
    if profile.data_directory.trim() == "~/.kura" {
        results.push(Err("hosted profile refuses production data directory without an explicit production recovery opt-in".to_string()));
    }
    if profile.live_connector_mode == crate::HOSTED_LIVE_CONNECTORS_LIVE {
        results.push(Err("live connector mode requires explicit operator opt-in outside default hosted validation".to_string()));
    }
    if profile.retention_days < 90 {
        results.push(Err("retention days must be at least 90".to_string()));
    }
    join_errors(results)
}

pub fn validate_hosted_stable_host(profile: &HostedOperationalProfile) -> Result<(), String> {
    match profile.host_class.as_str() {
        crate::HOSTED_HOST_CLASS_STABLE_TEST_HOST | crate::HOSTED_HOST_CLASS_VPS => Ok(()),
        crate::HOSTED_HOST_CLASS_DEVELOPER_LAPTOP => Err(
            "developer laptop cannot satisfy hosted release-readiness stable-host evidence"
                .to_string(),
        ),
        _ => Err(format!(
            "unsupported host class {:?} cannot satisfy hosted release-readiness evidence",
            profile.host_class
        )),
    }
}

pub fn validate_hosted_run(
    profile: &HostedOperationalProfile,
    run: &HostedRun,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let mut results = vec![
        require_non_empty("run id", &run.run_id),
        require_non_empty("run profile id", &run.profile_id),
        require_non_empty("commit or version", &run.commit_or_version),
        require_non_empty("host", &run.host),
        require_non_empty("operator", &run.operator),
        require_non_empty("artifact root", &run.artifact_root),
        require_allowed(
            "supervisor mode",
            &run.supervisor_mode,
            &[crate::HOSTED_SUPERVISOR_MODE_REPO_FOREGROUND],
        ),
        require_allowed(
            "run status",
            &run.status,
            &[
                crate::HOSTED_RUN_STATUS_PROVISIONING,
                crate::HOSTED_RUN_STATUS_RUNNING,
                crate::HOSTED_RUN_STATUS_STOPPED,
                crate::HOSTED_RUN_STATUS_FAILED,
                crate::HOSTED_RUN_STATUS_COMPLETED,
                crate::HOSTED_RUN_STATUS_EXPIRED,
            ],
        ),
        validate_hosted_retention("run", run.retention_expires_at, ""),
    ];
    if !profile.profile_id.is_empty() && run.profile_id != profile.profile_id {
        results.push(Err(format!(
            "run profile identity {:?} does not match hosted profile {:?}",
            run.profile_id, profile.profile_id
        )));
    }
    if run.started_at == DateTime::<Utc>::default() {
        results.push(Err("run started at is required".to_string()));
    }
    if now != DateTime::<Utc>::default()
        && run.retention_expires_at != DateTime::<Utc>::default()
        && run.retention_expires_at <= now
    {
        results.push(Err(format!(
            "run evidence expired at {}",
            run.retention_expires_at
        )));
    }
    join_errors(results)
}

pub fn validate_hosted_provisioning_elapsed(elapsed: std::time::Duration) -> Result<(), String> {
    require_elapsed_at_most("hosted profile provisioning", elapsed, MAX_INSTALL_ELAPSED)
}

// ---- hosted evidence validations ----

pub fn validate_hosted_deployment_manifest(
    run: &HostedRun,
    manifest: &HostedDeploymentManifest,
) -> Result<(), String> {
    let mut results = vec![
        require_non_empty("manifest id", &manifest.manifest_id),
        require_non_empty("commit or version", &manifest.commit_or_version),
        require_hosted_run_identity(
            run,
            &manifest.run_id,
            &manifest.profile_id,
            &manifest.commit_or_version,
        ),
        require_non_empty("branch", &manifest.branch),
        require_non_empty("host", &manifest.host),
        require_non_empty("operator", &manifest.operator),
        require_non_empty("configuration profile", &manifest.configuration_profile),
        require_non_empty("data directory", &manifest.data_directory),
        require_non_empty("artifact directory", &manifest.artifact_directory),
        require_allowed(
            "supervisor mode",
            &manifest.supervisor_mode,
            &[crate::HOSTED_SUPERVISOR_MODE_REPO_FOREGROUND],
        ),
        require_non_empty("daemon address", &manifest.daemon_address),
        require_allowed(
            "live connector mode",
            &manifest.live_connector_mode,
            &[crate::HOSTED_LIVE_CONNECTORS_DISABLED],
        ),
        require_allowed(
            "redaction status",
            &manifest.redaction_status,
            &[crate::HOSTED_REDACTION_PASSED],
        ),
        validate_hosted_retention("manifest", manifest.retention_expires_at, ""),
        validate_hosted_redaction("manifest", manifest),
    ];
    if manifest.started_at == DateTime::<Utc>::default() {
        results.push(Err("manifest started at is required".to_string()));
    }
    join_errors(results)
}

pub fn validate_hosted_supervisor_event(
    run: &HostedRun,
    event: &HostedSupervisorEvent,
) -> Result<(), String> {
    let mut results = vec![
        require_non_empty("event id", &event.event_id),
        require_hosted_run_identity(run, &event.run_id, "", ""),
        require_allowed(
            "event type",
            &event.event_type,
            &[
                crate::HOSTED_EVENT_START,
                crate::HOSTED_EVENT_STOP,
                crate::HOSTED_EVENT_RESTART,
                crate::HOSTED_EVENT_STATUS,
                crate::HOSTED_EVENT_HEALTH_CHECK,
                crate::HOSTED_EVENT_CRASH_DETECTED,
                crate::HOSTED_EVENT_REBOOT_RECOVERY,
                crate::HOSTED_EVENT_MANUAL_STOP,
                crate::HOSTED_EVENT_FAILED_RESTART,
                crate::HOSTED_EVENT_REPEATED_CRASH,
            ],
        ),
        require_non_empty("daemon health", &event.daemon_health),
        require_allowed(
            "result",
            &event.result,
            &[
                crate::HOSTED_RESULT_PASSED,
                crate::HOSTED_RESULT_FAILED,
                crate::HOSTED_RESULT_BLOCKED,
                crate::HOSTED_RESULT_UNSUPPORTED,
                crate::HOSTED_RESULT_OPERATOR_ACTION_NEEDED,
            ],
        ),
        require_non_empty("evidence path", &event.evidence_path),
        validate_hosted_redaction("supervisor event", event),
    ];
    if event.started_at == DateTime::<Utc>::default() {
        results.push(Err("event started at is required".to_string()));
    }
    if event.completed_at == DateTime::<Utc>::default() {
        results.push(Err("event completed at is required".to_string()));
    }
    if is_operator_initiated_supervisor_event(&event.event_type) {
        results.push(require_non_empty("requested by", &event.requested_by));
    }
    if event.result != crate::HOSTED_RESULT_PASSED {
        results.push(validate_hosted_failure_owner(&event.failure_owner));
    }
    match event.event_type.as_str() {
        crate::HOSTED_EVENT_CRASH_DETECTED | crate::HOSTED_EVENT_REBOOT_RECOVERY => {
            if event.recovery_seconds <= 0 {
                results.push(Err(format!(
                    "{} recovery seconds are required",
                    event.event_type
                )));
            }
            if std::time::Duration::from_secs(event.recovery_seconds as u64)
                > MAX_RESTART_RECOVERY_ELAPSED
            {
                results.push(Err(format!(
                    "{} recovery exceeds 5 minutes",
                    event.event_type
                )));
            }
        }
        crate::HOSTED_EVENT_MANUAL_STOP => {
            if event.recovery_seconds != 0 {
                results.push(Err(
                    "manual stop must not be classified as crash recovery".to_string()
                ));
            }
        }
        crate::HOSTED_EVENT_REPEATED_CRASH => {
            if event.result == crate::HOSTED_RESULT_PASSED {
                results.push(Err(
                    "repeated crash must surface failed restart or operator action needed"
                        .to_string(),
                ));
            }
        }
        crate::HOSTED_EVENT_FAILED_RESTART => {
            if event.result == crate::HOSTED_RESULT_PASSED {
                results.push(Err("failed restart cannot pass".to_string()));
            }
        }
        _ => {}
    }
    join_errors(results)
}

fn is_operator_initiated_supervisor_event(event_type: &str) -> bool {
    matches!(
        event_type,
        crate::HOSTED_EVENT_START
            | crate::HOSTED_EVENT_STOP
            | crate::HOSTED_EVENT_RESTART
            | crate::HOSTED_EVENT_STATUS
            | crate::HOSTED_EVENT_HEALTH_CHECK
            | crate::HOSTED_EVENT_MANUAL_STOP
    )
}

pub fn validate_hosted_backup_evidence(
    run: &HostedRun,
    backup: &HostedBackupEvidence,
) -> Result<(), String> {
    let mut results = vec![
        require_non_empty("backup id", &backup.backup_id),
        require_hosted_run_identity(
            run,
            &backup.run_id,
            &backup.source_profile_id,
            &backup.source_commit_or_version,
        ),
        require_non_empty("artifact path", &backup.artifact_path),
        require_non_empty("checksum", &backup.checksum),
        require_items("included material", &backup.included_material),
        require_items("excluded material", &backup.excluded_material),
        require_items("compatibility notes", &backup.compatibility_notes),
        validate_representative_tenants(backup.tenant_summary.len() as i64, &backup.tenant_summary),
        require_credential_exclusions(&backup.excluded_material),
        require_allowed(
            "redaction status",
            &backup.redaction_status,
            &[crate::HOSTED_REDACTION_PASSED],
        ),
        require_generated_at("backup", backup.generated_at),
        validate_hosted_redaction("backup", backup),
    ];
    for tenant in &backup.tenant_summary {
        results.push(validate_tenant_state_summary("tenant summary", tenant));
    }
    join_errors(results)
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

pub fn validate_hosted_restore_rehearsal(
    run: &HostedRun,
    result: &HostedRestoreRehearsalResult,
) -> Result<(), String> {
    let mut results = vec![
        require_non_empty("restore result id", &result.restore_result_id),
        require_hosted_run_identity(run, &result.run_id, "", ""),
        require_non_empty("backup id", &result.backup_id),
        require_non_empty("target profile id", &result.target_profile_id),
        require_non_empty("target data directory", &result.target_data_directory),
        validate_representative_tenants(result.tenant_count, &result.tenant_states),
        require_status_pass("tenant state result", &result.tenant_state_result),
        require_status_pass("migration state result", &result.migration_state_result),
        require_status_pass(
            "credential remediation result",
            &result.credential_remediation_result,
        ),
        require_status_pass("quota state result", &result.quota_state_result),
        require_status_pass("daemon health result", &result.daemon_health_result),
        require_status_pass(
            "raw credential scan result",
            &result.raw_credential_scan_result,
        ),
        require_status_pass("restore result", &result.result),
        require_generated_at("restore", result.generated_at),
        validate_hosted_redaction("restore", result),
    ];
    if !result.target_is_alternate {
        results.push(Err(
            "restore rehearsal must use an alternate target".to_string()
        ));
    }
    if result.tenant_count < MINIMUM_TENANT_COUNT as i64 {
        results.push(Err(
            "restore rehearsal must cover at least 3 tenants".to_string()
        ));
    }
    if result.cross_tenant_leakage {
        results.push(Err(
            "restore rehearsal observed cross-tenant leakage".to_string()
        ));
    }
    for tenant in &result.tenant_states {
        results.push(validate_tenant_state_summary("restore tenant", tenant));
    }
    join_errors(results)
}

pub fn validate_hosted_rollback_decision(
    run: &HostedRun,
    decision: &HostedRollbackDecisionRecord,
) -> Result<(), String> {
    let mut results = vec![
        require_non_empty("rollback decision id", &decision.rollback_decision_id),
        require_hosted_run_identity(run, &decision.run_id, "", ""),
        require_non_empty("trigger", &decision.trigger),
        require_allowed(
            "rollback decision",
            &decision.decision,
            &[
                crate::HOSTED_ROLLBACK_IN_PLACE,
                crate::HOSTED_ROLLBACK_RESTORE_FROM_BACKUP_REQUIRED,
                crate::HOSTED_ROLLBACK_NO_ROLLBACK_NEEDED,
                crate::HOSTED_ROLLBACK_BLOCKED,
            ],
        ),
        require_non_empty("rationale", &decision.rationale),
        require_items(
            "supporting evidence links",
            &decision.supporting_evidence_links,
        ),
        require_non_empty("operator", &decision.operator),
        validate_hosted_redaction("rollback decision", decision),
    ];
    if decision.decision == crate::HOSTED_ROLLBACK_RESTORE_FROM_BACKUP_REQUIRED {
        results.push(require_non_empty(
            "required backup id",
            &decision.required_backup_id,
        ));
    }
    if decision.decided_at == DateTime::<Utc>::default() {
        results.push(require_generated_at(
            "rollback decision",
            decision.decided_at,
        ));
    }
    join_errors(results)
}

pub fn validate_hosted_upgrade_evidence(
    run: &HostedRun,
    evidence: &HostedUpgradeEvidence,
) -> Result<(), String> {
    let mut results = vec![
        require_non_empty("upgrade evidence id", &evidence.upgrade_evidence_id),
        require_hosted_run_identity(run, &evidence.run_id, &evidence.profile_identity, ""),
        require_allowed(
            "upgrade phase",
            &evidence.phase,
            &[
                crate::HOSTED_UPGRADE_PHASE_PREFLIGHT,
                crate::HOSTED_UPGRADE_PHASE_POSTFLIGHT,
            ],
        ),
        require_generated_at("upgrade", evidence.generated_at),
        validate_hosted_redaction("upgrade", evidence),
    ];
    if !evidence.blocking_findings.is_empty() {
        results.push(Err("upgrade evidence has blocking findings".to_string()));
        if !evidence.failure_owner.is_empty() {
            results.push(validate_hosted_failure_owner(&evidence.failure_owner));
        }
    }
    match evidence.phase.as_str() {
        crate::HOSTED_UPGRADE_PHASE_PREFLIGHT => {
            results.push(require_non_empty(
                "deployment identity",
                &evidence.deployment_identity,
            ));
            results.push(require_non_empty(
                "profile identity",
                &evidence.profile_identity,
            ));
            results.push(require_non_empty("data location", &evidence.data_location));
            results.push(require_non_empty(
                "artifact location",
                &evidence.artifact_location,
            ));
            results.push(require_status_pass(
                "required backup state",
                &evidence.required_backup_state,
            ));
            results.push(require_status_pass(
                "daemon health",
                &evidence.daemon_health,
            ));
            results.push(require_status_pass(
                "configuration readiness",
                &evidence.configuration_readiness,
            ));
        }
        crate::HOSTED_UPGRADE_PHASE_POSTFLIGHT => {
            results.push(require_status_pass(
                "daemon health",
                &evidence.daemon_health,
            ));
            results.push(require_status_pass(
                "tenant data verification",
                &evidence.tenant_data_verification,
            ));
            results.push(require_status_pass(
                "migration state",
                &evidence.migration_state,
            ));
            results.push(require_status_pass(
                "credential remediation state",
                &evidence.credential_remediation_state,
            ));
            results.push(require_status_pass("quota state", &evidence.quota_state));
            results.push(require_status_pass(
                "operational diagnostics",
                &evidence.operational_diagnostics,
            ));
            results.push(require_non_empty(
                "rollback guidance",
                &evidence.rollback_guidance,
            ));
        }
        _ => {}
    }
    join_errors(results)
}

pub fn validate_hosted_observation_report(
    run: &HostedRun,
    report: &HostedObservationReport,
) -> Result<(), String> {
    let mut results = vec![
        require_non_empty("observation report id", &report.observation_report_id),
        require_hosted_run_identity(run, &report.run_id, "", ""),
        require_non_empty("sample window", &report.sample_window),
        require_non_empty("daemon health", &report.daemon_health),
        require_generated_at("observation report", report.generated_at),
        validate_hosted_redaction("observation report", report),
        validate_hosted_observation(
            "databaseSize",
            &report.database_size,
            &report.unsupported_fields,
        ),
        validate_hosted_observation("logSize", &report.log_size, &report.unsupported_fields),
        validate_hosted_observation("memory", &report.memory, &report.unsupported_fields),
        validate_hosted_observation("goroutines", &report.goroutines, &report.unsupported_fields),
        validate_hosted_observation(
            "fileDescriptors",
            &report.file_descriptors,
            &report.unsupported_fields,
        ),
        validate_hosted_observation(
            "queueOrBacklog",
            &report.queue_or_backlog,
            &report.unsupported_fields,
        ),
        validate_hosted_observation(
            "connectorHealth",
            &report.connector_health,
            &report.unsupported_fields,
        ),
        validate_hosted_observation("mcpHealth", &report.mcp_health, &report.unsupported_fields),
        validate_hosted_observation(
            "integrationDiagnosticState",
            &report.integration_diagnostic_state,
            &report.unsupported_fields,
        ),
    ];
    if report.daemon_health != crate::STATUS_PASS {
        results.push(Err(
            "observation report has blocking daemon health finding".to_string()
        ));
    }
    if report.monotonic_resource_growth {
        results.push(Err(
            "observation report has blocking resource growth finding".to_string(),
        ));
    }
    if report
        .queue_or_backlog
        .value
        .to_lowercase()
        .contains("backlog")
    {
        results.push(Err(
            "observation report has blocking backlog finding".to_string()
        ));
    }
    if !report.blocking_findings.is_empty()
        || report.daemon_health != crate::STATUS_PASS
        || report.monotonic_resource_growth
    {
        results.push(validate_hosted_failure_owner(&report.failure_owner));
    }
    join_errors(results)
}

fn validate_hosted_observation(
    label: &str,
    observation: &HostedObservation,
    unsupported_fields: &[String],
) -> Result<(), String> {
    if !observation.value.trim().is_empty() {
        return Ok(());
    }
    if observation.unsupported && unsupported_fields.iter().any(|f| f == label) {
        return Ok(());
    }
    Err(format!(
        "{label} is required or must be listed as unsupported"
    ))
}

pub fn validate_hosted_release_evidence_index(
    index: &HostedReleaseEvidenceIndex,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let mut results = vec![
        require_non_empty("release index id", &index.release_index_id),
        require_non_empty("run id", &index.run_id),
        require_non_empty("profile id", &index.profile_id),
        require_non_empty("commit or version", &index.commit_or_version),
        require_non_empty("review target", &index.review_target),
        require_elapsed_at_most(
            "release evidence review",
            index.review_elapsed,
            MAX_RELEASE_REVIEW_ELAPSED,
        ),
        require_allowed(
            "release decision",
            &index.decision,
            &[
                crate::RESULT_SHIP,
                crate::RESULT_NO_SHIP,
                crate::RESULT_SHIP_WITH_RECORDED_SKIPS,
            ],
        ),
        validate_hosted_retention(
            "release index",
            index.retention_expires_at,
            &index.authorized_retention_policy,
        ),
        validate_hosted_redaction("release index", index),
    ];
    if index.review_elapsed > MAX_RELEASE_REVIEW_ELAPSED {
        results.push(Err(
            "release evidence review must complete in 30 minutes or less".to_string(),
        ));
    }
    if index.generated_at == DateTime::<Utc>::default() {
        results.push(Err("release index generated at is required".to_string()));
    }
    if now != DateTime::<Utc>::default()
        && index.retention_expires_at < now
        && index.authorized_retention_policy.is_empty()
    {
        results.push(Err("release index evidence expired".to_string()));
    }
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (i, link) in index.evidence_links.iter().enumerate() {
        results.push(validate_hosted_evidence_link(index, i, link));
        seen.insert(link.evidence_type.clone());
    }
    for evidence_type in crate::REQUIRED_HOSTED_EVIDENCE_TYPES {
        if !seen.contains(*evidence_type) {
            results.push(Err(format!(
                "missing required evidence link {evidence_type}"
            )));
        }
    }
    for link in &index.evidence_links {
        if link.status != crate::STATUS_PASS && link.status != crate::HOSTED_RESULT_PASSED {
            results.push(Err(format!(
                "release evidence {} failed",
                link.evidence_type
            )));
        }
    }
    join_errors(results)
}

fn validate_hosted_evidence_link(
    index: &HostedReleaseEvidenceIndex,
    i: usize,
    link: &HostedEvidenceLink,
) -> Result<(), String> {
    let label = format!("evidence link[{i}]");
    let mut results = vec![
        require_non_empty(&format!("{label}.evidence type"), &link.evidence_type),
        require_non_empty(&format!("{label}.path"), &link.path),
        require_non_empty(&format!("{label}.status"), &link.status),
        validate_hosted_retention(
            &label,
            link.retention_expires_at,
            &index.authorized_retention_policy,
        ),
    ];
    if link.run_id != index.run_id
        || link.profile_id != index.profile_id
        || link.commit_or_version != index.commit_or_version
    {
        results.push(Err(format!(
            "{label} identity does not match release index"
        )));
    }
    if link.generated_at == DateTime::<Utc>::default() {
        results.push(Err(format!("{label} generated at is required")));
    }
    if !link.redaction_status.is_empty() && link.redaction_status != crate::HOSTED_REDACTION_PASSED
    {
        results.push(Err(format!("{label} redaction failed")));
    }
    if !link.blocking_findings.is_empty() {
        results.push(Err(format!("{label} has blocking findings")));
    }
    join_errors(results)
}

// ---- integration diagnostic smoke ----

pub fn build_integration_diagnostic_smoke_report(
    report_id: &str,
    requested_by: &str,
    probes: &[SmokeProbeInput],
    started_at: DateTime<Utc>,
) -> SmokeMatrixReport {
    let started_at = if started_at == DateTime::<Utc>::default() {
        Utc::now()
    } else {
        started_at
    };
    let completed_at = started_at;
    let mut report = SmokeMatrixReport {
        smoke_report_id: report_id.to_string(),
        report_kind: "diagnostic".to_string(),
        requested_by: requested_by.to_string(),
        status: SmokeReportStatus::Completed,
        domain_summary: HashMap::new(),
        started_at,
        completed_at: Some(completed_at),
        artifact_refs: Vec::new(),
        retention_expires_at: kura_integrations::diagnostic_retention_expiry(started_at),
        probe_outcomes: Vec::new(),
        ..SmokeMatrixReport::default()
    };
    for (index, probe) in probes.iter().enumerate() {
        if report.tenant_id.is_empty() {
            report.tenant_id = probe.tenant_id.clone();
        }
        let outcome = build_smoke_probe_outcome(report_id, index, probe, started_at);
        report.domain_summary.insert(
            outcome.domain_kind.clone(),
            outcome.result.as_str().to_string(),
        );
        report
            .artifact_refs
            .extend(outcome.artifact_refs.iter().cloned());
        if outcome.result == SmokeProbeResult::Failed {
            report.status = SmokeReportStatus::Failed;
        }
        if outcome.result == SmokeProbeResult::Blocked
            && report.status == SmokeReportStatus::Completed
        {
            report.status = SmokeReportStatus::Blocked;
        }
        report.probe_outcomes.push(outcome);
    }
    report
}

pub fn build_smoke_probe_outcome(
    report_id: &str,
    index: usize,
    probe: &SmokeProbeInput,
    fallback_time: DateTime<Utc>,
) -> SmokeProbeOutcome {
    let mut checked_at = probe.checked_at;
    if checked_at == DateTime::<Utc>::default() {
        checked_at = fallback_time;
    }
    let mut result = SmokeProbeResult::Passed;
    let mut blocked_reason = String::new();
    let mut reason_code = probe.reason_code;
    if probe.operator_deferred {
        result = SmokeProbeResult::Skipped;
        blocked_reason = SmokeBlockedReason::OperatorDeferred.as_str().to_string();
        reason_code = DiagnosticReasonCode::OperatorActionNeeded;
    } else if !probe.supported {
        result = SmokeProbeResult::Skipped;
        blocked_reason = SmokeBlockedReason::UnsupportedDomain.as_str().to_string();
        reason_code = DiagnosticReasonCode::UnsupportedDiagnostic;
    } else if !probe.safe_credentials_available {
        result = SmokeProbeResult::Blocked;
        blocked_reason = SmokeBlockedReason::MissingSafeCredentials
            .as_str()
            .to_string();
        reason_code = DiagnosticReasonCode::TokenMissing;
    } else if !probe.tenant_approval_available {
        result = SmokeProbeResult::Blocked;
        blocked_reason = SmokeBlockedReason::TenantApprovalUnavailable
            .as_str()
            .to_string();
        reason_code = DiagnosticReasonCode::TenantApprovalPending;
    } else if !probe.provider_available {
        result = SmokeProbeResult::Blocked;
        blocked_reason = SmokeBlockedReason::ProviderOutage.as_str().to_string();
        reason_code = DiagnosticReasonCode::ProviderUnavailable;
    } else if !probe.read_only_or_reversible && !probe.tenant_admin_approved {
        result = SmokeProbeResult::Blocked;
        blocked_reason = SmokeBlockedReason::MissingTenantAdminApproval
            .as_str()
            .to_string();
        reason_code = DiagnosticReasonCode::UnsafeToRetry;
    } else if !probe.read_only_or_reversible && !probe.operator_approved {
        result = SmokeProbeResult::Blocked;
        blocked_reason = SmokeBlockedReason::MissingOperatorApproval
            .as_str()
            .to_string();
        reason_code = DiagnosticReasonCode::UnsafeToRetry;
    } else if reason_code != DiagnosticReasonCode::Healthy {
        result = SmokeProbeResult::Failed;
    }
    let (_, _owner, retry_safety) = kura_integrations::diagnostic_defaults(reason_code);
    SmokeProbeOutcome {
        probe_outcome_id: diagnostic_smoke_outcome_id(report_id, index),
        tenant_id: probe.tenant_id.clone(),
        smoke_report_id: report_id.to_string(),
        integration_id: probe.integration_id.clone(),
        integration_account_id: probe.integration_account_id.clone(),
        domain_kind: probe.domain_kind.clone(),
        provider_kind: probe.provider_kind.clone(),
        probe_action: probe.probe_action.clone(),
        result,
        reason_code: reason_code.as_str().to_string(),
        remediation_hint: kura_integrations::diagnostic_remediation_hint(reason_code),
        retry_safety: retry_safety.as_str().to_string(),
        blocked_or_skipped_reason: blocked_reason,
        artifact_refs: probe.artifact_refs.clone(),
        checked_at,
        redaction_status: kura_integrations::RedactionStatus::Redacted
            .as_str()
            .to_string(),
        retention_expires_at: kura_integrations::diagnostic_retention_expiry(checked_at),
        ..SmokeProbeOutcome::default()
    }
}

fn diagnostic_smoke_outcome_id(report_id: &str, index: usize) -> String {
    format!("{report_id}_probe_{}", index + 1)
}
