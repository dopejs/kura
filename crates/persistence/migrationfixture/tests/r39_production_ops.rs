//! r39 production-ops fixture: standalone SQLite file built, copied, and
//! validated (integrity + tenant state + no raw credential material).

mod common;

use common::{count, open_conn, temp_dir};
use kura_migrationfixture::{
    build_r39_production_ops_fixture, build_r39_production_ops_sqlite_fixture,
    copy_r39_production_ops_sqlite_fixture, validate_r39_production_ops_sqlite_restore,
};

#[test]
fn r39_fixture_builds_with_expected_shape() {
    let dir = temp_dir("r39_build");
    let src = format!("{dir}/source.sqlite");
    let fixture = build_r39_production_ops_sqlite_fixture(&src).unwrap();
    assert_eq!(fixture.tenants.len(), 3);
    assert_eq!(fixture.expected_record_checks, 12);

    let conn = open_conn(&src);
    assert_eq!(count(&conn, "r39_tenants"), 3);
    assert_eq!(count(&conn, "r39_secret_refs"), 4); // alpha 2 + beta 1 + gamma 1
    assert_eq!(count(&conn, "r39_work_items"), 3);

    let gamma: (String, String, i64, i64) = conn
        .query_row(
            "SELECT quota_state, work_state, reconnect_required, operator_action_needed FROM r39_tenants WHERE tenant_id = 'ten_ops_gamma'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        gamma,
        (
            "usage_95_of_100".to_string(),
            "retry_exhausted_operator_action_needed".to_string(),
            1,
            1
        )
    );

    // No raw credential material anywhere in the source fixture.
    let raw: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM r39_secret_refs WHERE secret_ref LIKE '%do_not_leak%' OR secret_ref LIKE '%access_token%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(raw, 0);
}

#[test]
fn r39_restore_roundtrip_passes_validation() {
    let dir = temp_dir("r39_restore");
    let src = format!("{dir}/source.sqlite");
    let fixture = build_r39_production_ops_sqlite_fixture(&src).unwrap();

    let dest = format!("{dir}/restored.sqlite");
    copy_r39_production_ops_sqlite_fixture(&src, &dest).unwrap();

    // Validation re-opens the restored file: integrity_check, tenant states,
    // secret refs, and the no-raw-material guarantee.
    validate_r39_production_ops_sqlite_restore(&dest, &fixture).unwrap();
}

#[test]
fn r39_validation_rejects_tenant_state_drift() {
    let dir = temp_dir("r39_drift");
    let src = format!("{dir}/source.sqlite");
    let fixture = build_r39_production_ops_sqlite_fixture(&src).unwrap();

    let dest = format!("{dir}/restored.sqlite");
    copy_r39_production_ops_sqlite_fixture(&src, &dest).unwrap();

    // Corrupt one tenant's quota state after the copy.
    let conn = open_conn(&dest);
    conn.execute(
        "UPDATE r39_tenants SET quota_state = 'usage_99_of_100' WHERE tenant_id = 'ten_ops_alpha'",
        [],
    )
    .unwrap();
    drop(conn);

    let result = validate_r39_production_ops_sqlite_restore(&dest, &fixture);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("state mismatch"));
}

#[test]
fn r39_validation_rejects_raw_credential_rows() {
    let dir = temp_dir("r39_raw");
    let src = format!("{dir}/source.sqlite");
    let fixture = build_r39_production_ops_sqlite_fixture(&src).unwrap();

    let dest = format!("{dir}/restored.sqlite");
    copy_r39_production_ops_sqlite_fixture(&src, &dest).unwrap();

    let conn = open_conn(&dest);
    conn.execute(
        "INSERT INTO r39_secret_refs (tenant_id, secret_ref, reconnect_required) VALUES ('ten_ops_alpha', 'raw_secret_leaked', 0)",
        [],
    )
    .unwrap();
    drop(conn);

    let result = validate_r39_production_ops_sqlite_restore(&dest, &fixture);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("raw credential"));
}

#[test]
fn r39_static_fixture_matches_go_ids() {
    let fixture = build_r39_production_ops_fixture();
    let ids: Vec<&str> = fixture
        .tenants
        .iter()
        .map(|tenant| tenant.tenant_id.as_str())
        .collect();
    assert_eq!(ids, vec!["ten_ops_alpha", "ten_ops_beta", "ten_ops_gamma"]);
    assert_eq!(
        fixture.tenants[0].credential_refs,
        vec!["secretref_calendar_alpha", "secretref_provider_alpha"]
    );
    assert!(fixture.tenants[2].reconnect_required);
    assert!(fixture.tenants[2].operator_action_needed);
}
