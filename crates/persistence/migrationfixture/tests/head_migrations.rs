//! The head re-check over the seeded fixture must be loss-less: every seeded
//! row survives an idempotent migrate-to-head pass on the baseline schema.

mod common;

use common::{open_conn, temp_dir};
use kura_migrationfixture::{
    apply_head_migrations, build_pre_tenant_v21_fixture, count_seeded_rows,
};

#[test]
fn head_migrations_are_loss_less_and_preserve_rows() {
    let dir = temp_dir("head_migrations");
    let store = build_pre_tenant_v21_fixture(&dir).unwrap();
    assert_eq!(
        store.schema_version().unwrap(),
        kura_store::CURRENT_SCHEMA_VERSION
    );

    let before = count_seeded_rows(&store).unwrap();
    apply_head_migrations(&store).unwrap();
    assert_eq!(
        store.schema_version().unwrap(),
        kura_store::CURRENT_SCHEMA_VERSION
    );

    // Pre/post counts are equal for every seeded table.
    let after = count_seeded_rows(&store).unwrap();
    assert_eq!(before, after);

    // Spot-check a few rows still load back with their exact payloads.
    let conn = open_conn(store.db_path());
    let run_status: String = conn
        .query_row(
            "SELECT status FROM runs WHERE run_id = 'run_seed'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(run_status, "queued");
    let snapshot: String = conn
        .query_row(
            "SELECT snapshot_json FROM checkpoints WHERE checkpoint_id = 'chk_seed'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(snapshot.contains("\"runId\":\"run_seed\""));
    let event_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
        .unwrap();
    assert_eq!(event_count, 5);
}

#[test]
fn fixture_builder_runs_pre_tenant_then_head_then_roadmap_seeds() {
    let dir = temp_dir("fixture_builder");
    let output = kura_migrationfixture::FixtureBuilder::new()
        .build(&dir)
        .unwrap();
    assert_eq!(
        output.store.schema_version().unwrap(),
        kura_store::CURRENT_SCHEMA_VERSION
    );
    // Pre-tenant tables carry their seeds.
    assert_eq!(output.counts.get("sessions"), Some(&1));
    assert_eq!(output.counts.get("events"), Some(&5));
    // r37 fixture metadata.
    let r37 = output.r37.expect("r37 fixture present by default");
    assert_eq!(r37.provider_id, "r37_legacy_provider");
    assert_eq!(r37.conflict_ref, "R37_CONFLICT_TOKEN");

    // Roadmap tables are populated by the builder.
    let conn = open_conn(output.store.db_path());
    let discord: i64 = common::count(&conn, "discord_hosted_setups");
    assert_eq!(discord, 2);
    let slack: i64 = common::count(&conn, "slack_event_evidence");
    assert_eq!(slack, 2);
    let r41: i64 = common::count(&conn, "evaluation_discovery_policies");
    assert_eq!(r41, 2);
    let r42: i64 = common::count(&conn, "integration_diagnostic_runs");
    assert_eq!(r42, 2);
    let r48: i64 = common::count(&conn, "connector_conformance_results");
    assert_eq!(r48, 2);
}
