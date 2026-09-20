//! r42 integration-diagnostics fixture: two tenants × every Roadmap 42 table.

mod common;

use common::{open_conn, temp_dir};
use kura_migrationfixture::{
    apply_head_migrations, build_pre_tenant_v21_fixture, count_r42_integration_diagnostic_rows,
    seed_r42_integration_diagnostic_rows,
};

fn head_store() -> (kura_store::SQLiteStore, String) {
    let dir = temp_dir("r42");
    let store = build_pre_tenant_v21_fixture(&dir).unwrap();
    apply_head_migrations(&store).unwrap();
    (store, dir)
}

#[test]
fn r42_seeds_two_tenants_per_table() {
    let (store, _dir) = head_store();
    let fixture = seed_r42_integration_diagnostic_rows(&store).unwrap();
    assert_eq!(fixture.tenant_ids, vec!["ten_diag_alpha", "ten_diag_beta"]);

    let counts = count_r42_integration_diagnostic_rows(&store).unwrap();
    for (table, expected) in &fixture.expected_row_count {
        assert_eq!(counts.get(table), Some(expected), "table {table}");
    }
}

#[test]
fn r42_seeded_ids_and_documents_load_back() {
    let (store, _dir) = head_store();
    seed_r42_integration_diagnostic_rows(&store).unwrap();
    let conn = open_conn(store.db_path());

    let run: (String, String, String, String, String) = conn
        .query_row(
            "SELECT tenant_id, integration_id, provider_kind, trigger, status FROM integration_diagnostic_runs WHERE diagnostic_run_id = 'r42_diag_run_1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .unwrap();
    assert_eq!(run.0, "ten_diag_alpha");
    assert_eq!(run.1, "r42_integration_1");
    assert_eq!(run.2, "feishu_lark");
    assert_eq!(run.3, "operator_inspection");
    assert_eq!(run.4, "completed");

    // Diagnostic result links to its run and keeps freshness state.
    let result: (String, String, String, String) = conn
        .query_row(
            "SELECT run_id, freshness_state, redaction_status, document_json FROM integration_diagnostic_results WHERE diagnostic_result_id = 'r42_diag_result_1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(result.0, "r42_diag_run_1");
    assert_eq!(result.1, "fresh");
    assert_eq!(result.2, "redacted");
    assert_eq!(result.3, "{\"reasonCode\":\"healthy\"}");

    // Retention record targets the diagnostic run.
    let retention: (String, String, String) = conn
        .query_row(
            "SELECT target_kind, target_id, retention_state FROM integration_diagnostic_retention WHERE retention_record_id = 'r42_retention_1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        retention,
        (
            "diagnostic_run".to_string(),
            "r42_diag_run_1".to_string(),
            "active".to_string()
        )
    );

    // Probe outcome links to the smoke report.
    let probe: (String, String, String) = conn
        .query_row(
            "SELECT smoke_report_id, result, reason_code FROM integration_smoke_probe_outcomes WHERE probe_outcome_id = 'r42_probe_1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        probe,
        (
            "r42_smoke_1".to_string(),
            "passed".to_string(),
            "healthy".to_string()
        )
    );
}
