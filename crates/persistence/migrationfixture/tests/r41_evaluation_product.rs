//! r41 evaluation-product fixture: two tenants × every Roadmap 41 table.

mod common;

use common::{open_conn, temp_dir};
use kura_migrationfixture::{
    apply_head_migrations, build_pre_tenant_v21_fixture, count_r41_evaluation_product_rows,
    seed_r41_evaluation_product_rows,
};

fn head_store() -> (kura_store::SQLiteStore, String) {
    let dir = temp_dir("r41");
    let store = build_pre_tenant_v21_fixture(&dir).unwrap();
    apply_head_migrations(&store).unwrap();
    (store, dir)
}

#[test]
fn r41_seeds_two_tenants_per_table() {
    let (store, _dir) = head_store();
    let fixture = seed_r41_evaluation_product_rows(&store).unwrap();
    assert_eq!(fixture.tenant_ids, vec!["ten_eval_alpha", "ten_eval_beta"]);

    let counts = count_r41_evaluation_product_rows(&store).unwrap();
    for (table, expected) in &fixture.expected_row_count {
        assert_eq!(counts.get(table), Some(expected), "table {table}");
    }
}

#[test]
fn r41_seeded_ids_and_documents_load_back() {
    let (store, _dir) = head_store();
    seed_r41_evaluation_product_rows(&store).unwrap();
    let conn = open_conn(store.db_path());

    // Policy + run + candidate chain for the alpha tenant.
    let policy: (String, i64, String, String) = conn
        .query_row(
            "SELECT tenant_id, enabled, created_by, document_json FROM evaluation_discovery_policies WHERE policy_id = 'r41_policy_1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(policy.0, "ten_eval_alpha");
    assert_eq!(policy.1, 1);
    assert_eq!(policy.2, "prn_1");
    assert_eq!(policy.3, "{\"redactionStatus\":\"clean\"}");

    let candidate: (String, String, f64, String, String, String) = conn
        .query_row(
            "SELECT tenant_id, discovery_run_id, score, score_band, suppression_state, retention_state FROM evaluation_discovered_candidates WHERE discovered_candidate_id = 'r41_discovered_candidate_1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
        )
        .unwrap();
    assert_eq!(candidate.0, "ten_eval_alpha");
    assert_eq!(candidate.1, "r41_discovery_run_1");
    assert!((candidate.2 - 0.92).abs() < 1e-9);
    assert_eq!(candidate.3, "high");
    assert_eq!(candidate.4, "none");
    assert_eq!(candidate.5, "active");

    // Beta tenant: suppression active=1, retention dry_run=1 (i % 2).
    let beta_suppression: i64 = conn
        .query_row(
            "SELECT active FROM evaluation_suppressions WHERE suppression_id = 'r41_suppression_2'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(beta_suppression, 1);
    let beta_dry_run: i64 = conn
        .query_row(
            "SELECT dry_run FROM evaluation_retention_applications WHERE application_id = 'r41_retention_2'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(beta_dry_run, 1);

    // Campaign attempt group carries the drift count per tenant index.
    let beta_drift: i64 = conn
        .query_row(
            "SELECT drift_count FROM evaluation_campaign_attempt_groups WHERE attempt_group_id = 'r41_attempt_group_2'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(beta_drift, 1);

    // Revisions are immutable snapshots with redacted payloads.
    let revision: String = conn
        .query_row(
            "SELECT document_json FROM evaluation_fixture_revisions WHERE revision_id = 'r41_revision_1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(revision, "{\"payload\":{\"secret\":\"[REDACTED]\"}}");
}
