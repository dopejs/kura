//! r49 discord production-hardening fixture: two tenants × every Roadmap 49
//! table, with document_json matching what the Go Save* accessors store.

mod common;

use common::{open_conn, temp_dir};
use kura_migrationfixture::{
    apply_head_migrations, build_pre_tenant_v21_fixture, count_r49_discord_hardening_rows,
    seed_r49_discord_hardening_rows,
};

fn head_store() -> (kura_store::SQLiteStore, String) {
    let dir = temp_dir("r49");
    let store = build_pre_tenant_v21_fixture(&dir).unwrap();
    apply_head_migrations(&store).unwrap();
    (store, dir)
}

#[test]
fn r49_seeds_two_tenants_per_table() {
    let (store, _dir) = head_store();
    let fixture = seed_r49_discord_hardening_rows(&store).unwrap();
    assert_eq!(
        fixture.tenant_ids,
        vec!["ten_discord_alpha", "ten_discord_beta"]
    );

    let counts = count_r49_discord_hardening_rows(&store).unwrap();
    for (table, expected) in &fixture.expected_row_count {
        assert_eq!(counts.get(table), Some(expected), "table {table}");
    }
}

#[test]
fn r49_hosted_setup_row_matches_go_document_shape() {
    let (store, _dir) = head_store();
    seed_r49_discord_hardening_rows(&store).unwrap();
    let conn = open_conn(store.db_path());

    let row: (String, i64, i64, String, String, String, String) = conn
        .query_row(
            "SELECT readiness_state, respond_in_dm, require_mention, delivery_mode, reason_code, redaction_status, document_json FROM discord_hosted_setups WHERE tenant_id = 'ten_discord_alpha' AND connector_id = 'discord-r49-1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?)),
        )
        .unwrap();
    assert_eq!(row.0, "degraded_needs_repair");
    assert_eq!(row.1, 1);
    assert_eq!(row.2, 1);
    assert_eq!(row.3, "gateway");
    assert_eq!(row.4, "destination_validation_failed");
    assert_eq!(row.5, "redacted");

    // document_json mirrors the Go DiscordHostedSetupRecord marshal exactly.
    let doc: serde_json::Value = serde_json::from_str(&row.6).unwrap();
    assert_eq!(doc["tenantId"], "ten_discord_alpha");
    assert_eq!(doc["connectorId"], "discord-r49-1");
    assert_eq!(doc["connectorKind"], "discord");
    assert_eq!(doc["displayName"], "Discord R49");
    assert_eq!(doc["status"], "degraded");
    assert_eq!(doc["readinessState"], "degraded_needs_repair");
    assert_eq!(doc["hostedReady"], false);
    assert_eq!(doc["credentialState"], "valid");
    assert_eq!(doc["respondInDM"], true);
    assert_eq!(doc["requireMention"], true);
    assert_eq!(doc["deliveryMode"], "gateway");
    assert_eq!(doc["reasonCode"], "destination_validation_failed");
    assert_eq!(doc["redactionStatus"], "redacted");
    assert_eq!(doc["createdAt"], "2025-01-01T00:00:00Z");
    assert_eq!(doc["updatedAt"], "2025-01-01T00:00:00Z");
    assert_eq!(doc["validatedAt"], "2025-01-01T00:00:00Z");
    assert_eq!(doc["retentionExpiresAt"], "2025-01-01T00:00:00Z");
    // destinations omitempty: empty in the fixture, so absent from the document.
    assert!(doc.get("destinations").is_none());
}

#[test]
fn r49_destination_validation_and_smoke_rows_load_back() {
    let (store, _dir) = head_store();
    seed_r49_discord_hardening_rows(&store).unwrap();
    let conn = open_conn(store.db_path());

    let destination: (String, i64, String, String, String) = conn
        .query_row(
            "SELECT destination_type, selected, validation_state, reason_code, document_json FROM discord_destination_validations WHERE tenant_id = 'ten_discord_beta' AND connector_id = 'discord-r49-2'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .unwrap();
    assert_eq!(destination.0, "channel");
    assert_eq!(destination.1, 1);
    assert_eq!(destination.2, "missing_permission");
    assert_eq!(destination.3, "permission_missing");

    let doc: serde_json::Value = serde_json::from_str(&destination.4).unwrap();
    assert_eq!(doc["tenantId"], "ten_discord_beta");
    assert_eq!(doc["connectorId"], "discord-r49-2");
    assert_eq!(doc["destinationId"], "channel_2");
    assert_eq!(doc["selected"], true);
    assert_eq!(doc["safeEvidence"]["permission"], "send_messages");
    // providerLabel omitempty: empty -> absent.
    assert!(doc.get("providerLabel").is_none());

    let smoke: (String, String, String, String) = conn
        .query_row(
            "SELECT status, credential_mode, owner, reason FROM discord_smoke_evidence WHERE smoke_evidence_id = 'discord_smoke_1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        smoke,
        (
            "skipped".to_string(),
            "unavailable".to_string(),
            "operator".to_string(),
            "safe_credentials_unavailable".to_string()
        )
    );
}
