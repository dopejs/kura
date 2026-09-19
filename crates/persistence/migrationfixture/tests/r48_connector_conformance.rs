//! r48 connector-conformance fixture: two tenants × every Roadmap 48 table.

mod common;

use common::{open_conn, temp_dir};
use kura_migrationfixture::{
    apply_head_migrations, build_pre_tenant_v21_fixture, count_r48_connector_conformance_rows,
    seed_r48_connector_conformance_rows,
};

fn head_store() -> (kura_store::SQLiteStore, String) {
    let dir = temp_dir("r48");
    let store = build_pre_tenant_v21_fixture(&dir).unwrap();
    apply_head_migrations(&store).unwrap();
    (store, dir)
}

#[test]
fn r48_seeds_two_tenants_per_table() {
    let (store, _dir) = head_store();
    let fixture = seed_r48_connector_conformance_rows(&store).unwrap();
    assert_eq!(
        fixture.tenant_ids,
        vec!["ten_connector_alpha", "ten_connector_beta"]
    );

    let counts = count_r48_connector_conformance_rows(&store).unwrap();
    for (table, expected) in &fixture.expected_row_count {
        assert_eq!(counts.get(table), Some(expected), "table {table}");
    }
}

#[test]
fn r48_seeded_ids_and_documents_load_back() {
    let (store, _dir) = head_store();
    seed_r48_connector_conformance_rows(&store).unwrap();
    let conn = open_conn(store.db_path());

    let conformance: (String, String, String, String, String) = conn
        .query_row(
            "SELECT tenant_id, connector_id, scenario_id, area, result FROM connector_conformance_results WHERE conformance_result_id = 'r48_conformance_result_1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .unwrap();
    assert_eq!(conformance.0, "ten_connector_alpha");
    assert_eq!(conformance.1, "r48_connector_1");
    assert_eq!(conformance.2, "discord.direct.pass");
    assert_eq!(conformance.3, "direct_message");
    assert_eq!(conformance.4, "pass");

    // Diagnostic state + redaction failure linkage.
    let diagnostic: (String, String, String, String, String) = conn
        .query_row(
            "SELECT tenant_id, connector_id, reason_code, freshness_state, redaction_status FROM connector_diagnostic_states WHERE diagnostic_state_id = 'r48_diagnostic_state_1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .unwrap();
    assert_eq!(diagnostic.0, "ten_connector_alpha");
    assert_eq!(diagnostic.1, "r48_connector_1");
    assert_eq!(diagnostic.2, "healthy");
    assert_eq!(diagnostic.3, "fresh");
    assert_eq!(diagnostic.4, "redacted");

    let failure: (String, String) = conn
        .query_row(
            "SELECT connector_id, reason_code FROM connector_diagnostic_redaction_failures WHERE redaction_failure_id = 'r48_redaction_failure_1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        failure,
        (
            "r48_connector_1".to_string(),
            "redaction_failed_closed".to_string()
        )
    );

    let boundary: (String, String, String, String) = conn
        .query_row(
            "SELECT connector_id, foreground_reply_outcome_id, background_delivery_id, separation_status FROM connector_delivery_boundaries WHERE boundary_id = 'r48_delivery_boundary_1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        boundary,
        (
            "r48_connector_1".to_string(),
            "foreground_1".to_string(),
            "background_1".to_string(),
            "separate_truths".to_string()
        )
    );
}
