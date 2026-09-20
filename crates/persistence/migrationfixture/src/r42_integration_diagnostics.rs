//! Roadmap 42 integration-diagnostics fixture (port of
//! daemon/internal/store/migrationfixture/r42_integration_diagnostics.go): two
//! tenants × every integration-diagnostics table (migration v39).

use std::collections::HashMap;

use rusqlite::params;

use kura_store::SQLiteStore;

use crate::FIXTURE_TIMESTAMP;
use crate::seeds::exec_insert;

/// Table names expected from the Roadmap 42 storage migration (migration v39).
pub static R42_INTEGRATION_DIAGNOSTIC_TABLE_NAMES: [&str; 6] = [
    "integration_diagnostic_runs",
    "integration_diagnostic_results",
    "integration_provider_classifications",
    "integration_smoke_reports",
    "integration_smoke_probe_outcomes",
    "integration_diagnostic_retention",
];

/// Expected per-table row counts after seeding (Go R42IntegrationDiagnosticFixture).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct R42IntegrationDiagnosticFixture {
    pub tenant_ids: Vec<String>,
    pub expected_row_count: HashMap<String, i64>,
}

#[must_use]
pub fn build_r42_integration_diagnostic_fixture() -> R42IntegrationDiagnosticFixture {
    let mut counts = HashMap::new();
    for table in R42_INTEGRATION_DIAGNOSTIC_TABLE_NAMES {
        counts.insert(table.to_string(), 2);
    }
    R42IntegrationDiagnosticFixture {
        tenant_ids: vec!["ten_diag_alpha".to_string(), "ten_diag_beta".to_string()],
        expected_row_count: counts,
    }
}

/// Seeds two tenants × every r42 table. Requires the store at head schema (v39+).
pub fn seed_r42_integration_diagnostic_rows(
    store: &SQLiteStore,
) -> Result<R42IntegrationDiagnosticFixture, String> {
    let fixture = build_r42_integration_diagnostic_fixture();
    let conn = crate::open_fixture_connection(store.db_path())?;
    let ts = FIXTURE_TIMESTAMP;

    for (index, tenant_id) in fixture.tenant_ids.iter().enumerate() {
        let suffix = (index + 1).to_string();
        let run_id = format!("r42_diag_run_{suffix}");
        let result_id = format!("r42_diag_result_{suffix}");
        let classification_id = format!("r42_classification_{suffix}");
        let smoke_id = format!("r42_smoke_{suffix}");
        let outcome_id = format!("r42_probe_{suffix}");
        let retention_id = format!("r42_retention_{suffix}");
        let integration_id = format!("r42_integration_{suffix}");

        exec_insert(
            &conn,
            "INSERT INTO integration_diagnostic_runs (diagnostic_run_id, tenant_id, integration_id, integration_account_id, domain_kind, provider_kind, requested_by, trigger, status, started_at, completed_at, failure_reason_code, redaction_status, retention_expires_at, idempotency_key, document_json) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                run_id,
                tenant_id,
                integration_id,
                format!("acct_{suffix}"),
                "calendar",
                "feishu_lark",
                format!("operator_{suffix}"),
                "operator_inspection",
                "completed",
                ts,
                None::<String>,
                None::<String>,
                "redacted",
                ts,
                format!("idem_{suffix}"),
                "{\"status\":\"completed\"}"
            ],
        )?;
        exec_insert(
            &conn,
            "INSERT INTO integration_diagnostic_results (diagnostic_result_id, tenant_id, integration_id, integration_account_id, domain_kind, provider_kind, capability, status, reason_code, remediation_owner, retry_safety, checked_at, stale_after, freshness_state, run_id, redaction_status, retention_expires_at, document_json) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                result_id,
                tenant_id,
                integration_id,
                format!("acct_{suffix}"),
                "calendar",
                "feishu_lark",
                "calendar.read",
                "healthy",
                "healthy",
                "none_required",
                "no_action_needed",
                ts,
                ts,
                "fresh",
                run_id,
                "redacted",
                ts,
                "{\"reasonCode\":\"healthy\"}"
            ],
        )?;
        exec_insert(
            &conn,
            "INSERT INTO integration_provider_classifications (classification_id, tenant_id, provider_kind, domain_kind, integration_id, operation_class, reason_code, retry_safety, remediation_owner, redaction_status, created_at, document_json) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                classification_id,
                tenant_id,
                "feishu_lark",
                "calendar",
                integration_id,
                "calendar.read",
                "healthy",
                "no_action_needed",
                "none_required",
                "redacted",
                ts,
                "{\"reasonCode\":\"healthy\"}"
            ],
        )?;
        exec_insert(
            &conn,
            "INSERT INTO integration_smoke_reports (smoke_report_id, tenant_id, report_kind, requested_by, status, started_at, completed_at, published_at, retention_expires_at, document_json) VALUES (?,?,?,?,?,?,?,?,?,?)",
            params![
                smoke_id,
                tenant_id,
                "diagnostic",
                format!("operator_{suffix}"),
                "completed",
                ts,
                None::<String>,
                None::<String>,
                ts,
                "{\"status\":\"completed\"}"
            ],
        )?;
        exec_insert(
            &conn,
            "INSERT INTO integration_smoke_probe_outcomes (probe_outcome_id, tenant_id, smoke_report_id, integration_id, integration_account_id, domain_kind, provider_kind, probe_action, result, reason_code, retry_safety, checked_at, redaction_status, retention_expires_at, document_json) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                outcome_id,
                tenant_id,
                smoke_id,
                integration_id,
                format!("acct_{suffix}"),
                "calendar",
                "feishu_lark",
                "calendar.read",
                "passed",
                "healthy",
                "no_action_needed",
                ts,
                "redacted",
                ts,
                "{\"result\":\"passed\"}"
            ],
        )?;
        exec_insert(
            &conn,
            "INSERT INTO integration_diagnostic_retention (retention_record_id, tenant_id, target_kind, target_id, policy_ref, default_expires_at, effective_expires_at, retention_state, applied_at, created_at, updated_at, document_json) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                retention_id,
                tenant_id,
                "diagnostic_run",
                run_id,
                None::<String>,
                ts,
                ts,
                "active",
                None::<String>,
                ts,
                ts,
                "{\"retentionState\":\"active\"}"
            ],
        )?;
    }
    Ok(fixture)
}

/// Counts rows per r42 table (Go CountR42IntegrationDiagnosticRows).
pub fn count_r42_integration_diagnostic_rows(
    store: &SQLiteStore,
) -> Result<HashMap<String, i64>, String> {
    let conn = crate::open_fixture_connection(store.db_path())?;
    let mut counts = HashMap::new();
    for table in R42_INTEGRATION_DIAGNOSTIC_TABLE_NAMES {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .map_err(|e| format!("count {table}: {e}"))?;
        counts.insert(table.to_string(), count);
    }
    Ok(counts)
}
