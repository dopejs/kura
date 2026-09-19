//! r50 telegram channel-connector fixture: two tenants × every Roadmap 50
//! table, with document_json matching what the Go Save* accessors store.

mod common;

use common::{open_conn, temp_dir};
use kura_migrationfixture::{
    apply_head_migrations, build_pre_tenant_v21_fixture, count_r50_telegram_channel_connector_rows,
    seed_r50_telegram_channel_connector_rows,
};

fn head_store() -> (kura_store::SQLiteStore, String) {
    let dir = temp_dir("r50");
    let store = build_pre_tenant_v21_fixture(&dir).unwrap();
    apply_head_migrations(&store).unwrap();
    (store, dir)
}

#[test]
fn r50_seeds_two_tenants_per_table() {
    let (store, _dir) = head_store();
    let fixture = seed_r50_telegram_channel_connector_rows(&store).unwrap();
    assert_eq!(
        fixture.tenant_ids,
        vec!["ten_telegram_alpha", "ten_telegram_beta"]
    );

    let counts = count_r50_telegram_channel_connector_rows(&store).unwrap();
    for (table, expected) in &fixture.expected_row_count {
        assert_eq!(counts.get(table), Some(expected), "table {table}");
    }
}

#[test]
fn r50_hosted_setup_row_matches_go_document_shape() {
    let (store, _dir) = head_store();
    seed_r50_telegram_channel_connector_rows(&store).unwrap();
    let conn = open_conn(store.db_path());

    let row: (String, String, i64, String, String, String) = conn
        .query_row(
            "SELECT terminal_state, allowment_state, delivery_eligible, reason_code, redaction_status, document_json FROM telegram_hosted_setups WHERE tenant_id = 'ten_telegram_alpha' AND connector_id = 'telegram-r50-1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
        )
        .unwrap();
    assert_eq!(row.0, "action-required");
    assert_eq!(row.1, "none");
    assert_eq!(row.2, 0);
    assert_eq!(row.3, "telegram_allowment_missing");
    assert_eq!(row.4, "redacted");

    let doc: serde_json::Value = serde_json::from_str(&row.5).unwrap();
    assert_eq!(doc["tenantId"], "ten_telegram_alpha");
    assert_eq!(doc["connectorId"], "telegram-r50-1");
    assert_eq!(doc["connectorKind"], "telegram");
    assert_eq!(doc["displayName"], "Telegram R50");
    assert_eq!(doc["status"], "degraded");
    assert_eq!(doc["terminalState"], "action-required");
    assert_eq!(doc["hostedReady"], false);
    assert_eq!(doc["credentialState"], "valid");
    assert_eq!(doc["allowmentState"], "none");
    assert_eq!(doc["groupBehavior"], "disabled");
    assert_eq!(doc["deliveryEligible"], false);
    assert_eq!(doc["reasonCode"], "telegram_allowment_missing");
    assert_eq!(doc["redactionStatus"], "redacted");
}

#[test]
fn r50_allowment_smoke_update_rows_load_back() {
    let (store, _dir) = head_store();
    seed_r50_telegram_channel_connector_rows(&store).unwrap();
    let conn = open_conn(store.db_path());

    // Allowment: telegram-scope json names and safe evidence.
    let allowment: (String, String, i64, String, String, String) = conn
        .query_row(
            "SELECT scope_type, scope_id, enabled, group_gate, validation_state, document_json FROM telegram_allowments WHERE tenant_id = 'ten_telegram_beta' AND connector_id = 'telegram-r50-2'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
        )
        .unwrap();
    assert_eq!(allowment.0, "direct_chat");
    assert_eq!(allowment.1, "chat_2");
    assert_eq!(allowment.2, 1);
    assert_eq!(allowment.3, "not_applicable");
    assert_eq!(allowment.4, "valid");

    let doc: serde_json::Value = serde_json::from_str(&allowment.5).unwrap();
    assert_eq!(doc["tenantId"], "ten_telegram_beta");
    assert_eq!(doc["connectorId"], "telegram-r50-2");
    assert_eq!(doc["allowmentId"], "allow_2");
    assert_eq!(doc["telegramScopeType"], "direct_chat");
    assert_eq!(doc["telegramScopeId"], "chat_2");
    assert_eq!(doc["enabled"], true);
    assert_eq!(doc["safeEvidence"]["scope"], "direct_chat");

    // Smoke evidence.
    let smoke: (String, String, String, String) = conn
        .query_row(
            "SELECT status, credential_mode, owner, reason FROM telegram_smoke_evidence WHERE smoke_evidence_id = 'telegram_smoke_1'",
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

    // Update evidence: composite identity + route outcome.
    let update: (String, String, String, String, String, String) = conn
        .query_row(
            "SELECT chat_id, message_id, update_id, route_outcome, reason_code, document_json FROM telegram_update_evidence WHERE tenant_id = 'ten_telegram_alpha' AND connector_id = 'telegram-r50-1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
        )
        .unwrap();
    assert_eq!(update.0, "chat_1");
    assert_eq!(update.1, "message_1");
    assert_eq!(update.2, "update_1");
    assert_eq!(update.3, "accepted");
    assert_eq!(update.4, "accepted");
    let udoc: serde_json::Value = serde_json::from_str(&update.5).unwrap();
    assert_eq!(udoc["telegramChatId"], "chat_1");
    assert_eq!(
        udoc["safeEvidence"]["identityRule"],
        "telegram_chat_message_id"
    );
}
