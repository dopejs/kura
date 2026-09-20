//! Integration diagnostic run / state-change / smoke / redaction / retention
//! events (port of `integration_diagnostics.go`).

use crate::util::{is_go_zero_time, now_utc, payload};
use crate::{Event, Resource};
use kura_integrations::{
    DiagnosticResult, DiagnosticRetentionRecord, DiagnosticRun, DiagnosticStatus,
};
use kura_opsreadiness::SmokeMatrixReport;

pub const INTEGRATION_DIAGNOSTIC_RUN_STARTED_NAME: &str = "integration_diagnostic.run_started";
pub const INTEGRATION_DIAGNOSTIC_RUN_COMPLETED_NAME: &str = "integration_diagnostic.run_completed";
pub const INTEGRATION_DIAGNOSTIC_STATE_CHANGED_NAME: &str = "integration_diagnostic.state_changed";
pub const INTEGRATION_DIAGNOSTIC_REDACTION_FAILED_NAME: &str =
    "integration_diagnostic.redaction_failed_closed";
pub const INTEGRATION_DIAGNOSTIC_SMOKE_COMPLETED_NAME: &str =
    "integration_diagnostic.smoke_completed";
pub const INTEGRATION_DIAGNOSTIC_RETENTION_APPLIED_NAME: &str =
    "integration_diagnostic.retention_applied";

/// Go: `IntegrationDiagnosticRunEvent` — a completed run reports its
/// completion time.
#[must_use]
pub fn integration_diagnostic_run_event(name: &str, run: DiagnosticRun) -> Event {
    let occurred_at = run.completed_at.unwrap_or(run.started_at);
    let occurred_at = if is_go_zero_time(occurred_at) {
        now_utc()
    } else {
        occurred_at
    };
    Event {
        tenant_id: run.tenant_id.clone(),
        category: "integration".to_string(),
        name: name.to_string(),
        occurred_at,
        resource: Resource {
            kind: "integration_diagnostic_run".to_string(),
            id: run.diagnostic_run_id.clone(),
        },
        payload: payload![
            "tenantId" => run.tenant_id,
            "diagnosticRunId" => run.diagnostic_run_id,
            "integrationId" => run.integration_id,
            "requestedBy" => run.requested_by,
            "status" => run.status.as_str(),
            "redactionStatus" => run.redaction_status.as_str(),
        ],
        ..Event::default()
    }
}

/// Go: `IntegrationDiagnosticStateChangedEvent`.
#[must_use]
pub fn integration_diagnostic_state_changed_event(
    result: DiagnosticResult,
    previous: DiagnosticStatus,
) -> Event {
    let occurred_at = if is_go_zero_time(result.checked_at) {
        now_utc()
    } else {
        result.checked_at
    };
    Event {
        tenant_id: result.tenant_id.clone(),
        category: "integration".to_string(),
        name: INTEGRATION_DIAGNOSTIC_STATE_CHANGED_NAME.to_string(),
        occurred_at,
        resource: Resource {
            kind: "integration_diagnostic_result".to_string(),
            id: result.diagnostic_result_id.clone(),
        },
        payload: payload![
            "tenantId" => result.tenant_id,
            "diagnosticResultId" => result.diagnostic_result_id,
            "integrationId" => result.integration_id,
            "previousStatus" => previous.as_str(),
            "status" => result.status.as_str(),
            "reasonCode" => result.reason_code.as_str(),
            "remediationOwner" => result.remediation_owner.as_str(),
        ],
        ..Event::default()
    }
}

/// Go: `IntegrationDiagnosticSmokeCompletedEvent`.
#[must_use]
pub fn integration_diagnostic_smoke_completed_event(report: SmokeMatrixReport) -> Event {
    let occurred_at = report.completed_at.unwrap_or_else(now_utc);
    Event {
        tenant_id: report.tenant_id.clone(),
        category: "integration".to_string(),
        name: INTEGRATION_DIAGNOSTIC_SMOKE_COMPLETED_NAME.to_string(),
        occurred_at,
        resource: Resource {
            kind: "integration_diagnostic_smoke_report".to_string(),
            id: report.smoke_report_id.clone(),
        },
        payload: payload![
            "tenantId" => report.tenant_id,
            "smokeReportId" => report.smoke_report_id,
            "status" => report.status.as_str(),
            "domainSummary" => report.domain_summary,
            "artifactRefs" => report.artifact_refs,
        ],
        ..Event::default()
    }
}

/// Go: `IntegrationDiagnosticRedactionFailedEvent` — fails closed.
#[must_use]
pub fn integration_diagnostic_redaction_failed_event(result: DiagnosticResult) -> Event {
    let occurred_at = if is_go_zero_time(result.checked_at) {
        now_utc()
    } else {
        result.checked_at
    };
    Event {
        tenant_id: result.tenant_id.clone(),
        category: "integration".to_string(),
        name: INTEGRATION_DIAGNOSTIC_REDACTION_FAILED_NAME.to_string(),
        occurred_at,
        resource: Resource {
            kind: "integration_diagnostic_result".to_string(),
            id: result.diagnostic_result_id.clone(),
        },
        payload: payload![
            "tenantId" => result.tenant_id,
            "targetKind" => "diagnostic_result",
            "targetId" => result.diagnostic_result_id,
            "diagnosticResultId" => result.diagnostic_result_id,
            "integrationId" => result.integration_id,
            "reasonCode" => result.reason_code.as_str(),
            "redactionStatus" => result.redaction_status.as_str(),
        ],
        ..Event::default()
    }
}

/// Go: `IntegrationDiagnosticRetentionAppliedEvent`.
#[must_use]
pub fn integration_diagnostic_retention_applied_event(record: DiagnosticRetentionRecord) -> Event {
    let occurred_at = record.applied_at.unwrap_or(record.updated_at);
    let occurred_at = if is_go_zero_time(occurred_at) {
        now_utc()
    } else {
        occurred_at
    };
    Event {
        tenant_id: record.tenant_id.clone(),
        category: "integration".to_string(),
        name: INTEGRATION_DIAGNOSTIC_RETENTION_APPLIED_NAME.to_string(),
        occurred_at,
        resource: Resource {
            kind: "integration_diagnostic_retention".to_string(),
            id: record.retention_record_id.clone(),
        },
        payload: payload![
            "tenantId" => record.tenant_id,
            "targetKind" => record.target_kind,
            "targetId" => record.target_id,
            "retentionState" => record.retention_state.as_str(),
            "effectiveExpiresAt" => record.effective_expires_at,
        ],
        ..Event::default()
    }
}
