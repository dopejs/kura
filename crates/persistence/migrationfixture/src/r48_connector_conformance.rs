//! Roadmap 48 connector-conformance fixture (port of
//! daemon/internal/store/migrationfixture/r48_connector_conformance.go): two
//! tenants × every connector-conformance table (migration v43).

use std::collections::HashMap;

use rusqlite::params;

use kura_store::SQLiteStore;

use crate::FIXTURE_TIMESTAMP;
use crate::seeds::exec_insert;

/// Table names expected from the Roadmap 48 storage migration (migration v43).
pub static R48_CONNECTOR_CONFORMANCE_TABLE_NAMES: [&str; 4] = [
    "connector_conformance_results",
    "connector_diagnostic_states",
    "connector_diagnostic_redaction_failures",
    "connector_delivery_boundaries",
];

/// Expected per-table row counts after seeding (Go R48ConnectorConformanceFixture).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct R48ConnectorConformanceFixture {
    pub tenant_ids: Vec<String>,
    pub expected_row_count: HashMap<String, i64>,
}

#[must_use]
pub fn build_r48_connector_conformance_fixture() -> R48ConnectorConformanceFixture {
    let mut counts = HashMap::new();
    for table in R48_CONNECTOR_CONFORMANCE_TABLE_NAMES {
        counts.insert(table.to_string(), 2);
    }
    R48ConnectorConformanceFixture {
        tenant_ids: vec![
            "ten_connector_alpha".to_string(),
            "ten_connector_beta".to_string(),
        ],
        expected_row_count: counts,
    }
}

/// Seeds two tenants × every r48 table. Requires the store at head schema (v43+).
pub fn seed_r48_connector_conformance_rows(
    store: &SQLiteStore,
) -> Result<R48ConnectorConformanceFixture, String> {
    let fixture = build_r48_connector_conformance_fixture();
    let conn = crate::open_fixture_connection(store.db_path())?;
    let ts = FIXTURE_TIMESTAMP;

    for (index, tenant_id) in fixture.tenant_ids.iter().enumerate() {
        let suffix = (index + 1).to_string();
        let connector_id = format!("r48_connector_{suffix}");
        let result_id = format!("r48_conformance_result_{suffix}");
        let diagnostic_id = format!("r48_diagnostic_state_{suffix}");
        let failure_id = format!("r48_redaction_failure_{suffix}");
        let boundary_id = format!("r48_delivery_boundary_{suffix}");

        exec_insert(
            &conn,
            "INSERT INTO connector_conformance_results (conformance_result_id, tenant_id, connector_kind, connector_id, scenario_id, area, result, reason_code, redaction_status, evidence_timestamp, retention_expires_at, document_json) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                result_id,
                tenant_id,
                "discord",
                connector_id,
                "discord.direct.pass",
                "direct_message",
                "pass",
                None::<String>,
                "redacted",
                ts,
                ts,
                "{\"result\":\"pass\"}"
            ],
        )?;
        exec_insert(
            &conn,
            "INSERT INTO connector_diagnostic_states (diagnostic_state_id, tenant_id, connector_id, connector_account_id, status, reason_code, remediation_owner, user_visible_severity, retry_safety, evidence_timestamp, stale_after, freshness_state, redaction_status, retention_expires_at, redaction_failure_id, document_json) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                diagnostic_id,
                tenant_id,
                connector_id,
                format!("acct_{suffix}"),
                "healthy",
                "healthy",
                "none_required",
                "info",
                "no_action_needed",
                ts,
                ts,
                "fresh",
                "redacted",
                ts,
                None::<String>,
                "{\"reasonCode\":\"healthy\"}"
            ],
        )?;
        exec_insert(
            &conn,
            "INSERT INTO connector_diagnostic_redaction_failures (redaction_failure_id, tenant_id, connector_id, diagnostic_state_id, reason_code, occurred_at, retention_expires_at, document_json) VALUES (?,?,?,?,?,?,?,?)",
            params![
                failure_id,
                tenant_id,
                connector_id,
                diagnostic_id,
                "redaction_failed_closed",
                ts,
                ts,
                "{\"redactionStatus\":\"suppressed\"}"
            ],
        )?;
        exec_insert(
            &conn,
            "INSERT INTO connector_delivery_boundaries (boundary_id, tenant_id, connector_id, foreground_reply_outcome_id, background_delivery_id, transport_kind, separation_status, created_at, document_json) VALUES (?,?,?,?,?,?,?,?,?)",
            params![
                boundary_id,
                tenant_id,
                connector_id,
                format!("foreground_{suffix}"),
                format!("background_{suffix}"),
                "discord",
                "separate_truths",
                ts,
                "{\"separationStatus\":\"separate_truths\"}"
            ],
        )?;
    }
    Ok(fixture)
}

/// Counts rows per r48 table (Go CountR48ConnectorConformanceRows).
pub fn count_r48_connector_conformance_rows(
    store: &SQLiteStore,
) -> Result<HashMap<String, i64>, String> {
    let conn = crate::open_fixture_connection(store.db_path())?;
    let mut counts = HashMap::new();
    for table in R48_CONNECTOR_CONFORMANCE_TABLE_NAMES {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .map_err(|e| format!("count {table}: {e}"))?;
        counts.insert(table.to_string(), count);
    }
    Ok(counts)
}
