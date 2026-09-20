//! Roadmap 50 telegram channel-connector fixture (port of
//! daemon/internal/store/migrationfixture/r50_telegram_channel_connector.go).
//!
//! The Go fixture seeds via the store accessors SaveTelegramHostedSetup /
//! SaveTelegramAllowment / SaveTelegramSmokeEvidence / SaveTelegramUpdateEvidence,
//! which are not yet ported to kura-store. The tables exist (migration v46) and
//! the rows below replicate those accessors' exact column writes and
//! document_json payloads.

use std::collections::HashMap;

use rusqlite::params;

use kura_store::SQLiteStore;

use crate::FIXTURE_TIMESTAMP;
use crate::records::{
    telegram_allowment_document, telegram_hosted_setup_document, telegram_smoke_evidence_document,
    telegram_update_evidence_document,
};
use crate::seeds::exec_insert;

/// Table names expected from the Roadmap 50 storage migration (migration v46).
pub static R50_TELEGRAM_CHANNEL_CONNECTOR_TABLE_NAMES: [&str; 4] = [
    "telegram_hosted_setups",
    "telegram_allowments",
    "telegram_smoke_evidence",
    "telegram_update_evidence",
];

/// Expected per-table row counts after seeding (Go R50TelegramChannelConnectorFixture).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct R50TelegramChannelConnectorFixture {
    pub tenant_ids: Vec<String>,
    pub expected_row_count: HashMap<String, i64>,
}

#[must_use]
pub fn build_r50_telegram_channel_connector_fixture() -> R50TelegramChannelConnectorFixture {
    let counts = HashMap::from([
        ("telegram_hosted_setups".to_string(), 2),
        ("telegram_allowments".to_string(), 2),
        ("telegram_smoke_evidence".to_string(), 2),
        ("telegram_update_evidence".to_string(), 2),
    ]);
    R50TelegramChannelConnectorFixture {
        tenant_ids: vec![
            "ten_telegram_alpha".to_string(),
            "ten_telegram_beta".to_string(),
        ],
        expected_row_count: counts,
    }
}

/// Seeds two tenants × every r50 table. Requires the store at head schema (v46+).
pub fn seed_r50_telegram_channel_connector_rows(
    store: &SQLiteStore,
) -> Result<R50TelegramChannelConnectorFixture, String> {
    let fixture = build_r50_telegram_channel_connector_fixture();
    let conn = crate::open_fixture_connection(store.db_path())?;
    let ts = FIXTURE_TIMESTAMP;

    for (index, tenant_id) in fixture.tenant_ids.iter().enumerate() {
        let suffix = (index + 1).to_string();
        let connector_id = format!("telegram-r50-{suffix}");

        let hosted_document = telegram_hosted_setup_document(
            tenant_id,
            &connector_id,
            "telegram",
            "Telegram R50",
            "degraded",
            "action-required",
            "valid",
            "none",
            "disabled",
            "telegram_allowment_missing",
            "redacted",
            ts,
            ts,
            ts,
            ts,
        )?;
        exec_insert(
            &conn,
            "INSERT INTO telegram_hosted_setups (tenant_id, connector_id, connector_kind, display_name, status, terminal_state, credential_state, allowment_state, group_behavior, delivery_eligible, reason_code, redaction_status, created_at, updated_at, validated_at, retention_expires_at, document_json) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                tenant_id,
                connector_id,
                "telegram",
                "Telegram R50",
                "degraded",
                "action-required",
                "valid",
                "none",
                "disabled",
                0i64,
                "telegram_allowment_missing",
                "redacted",
                ts,
                ts,
                ts,
                ts,
                hosted_document
            ],
        )?;

        let allowment_document = telegram_allowment_document(
            tenant_id,
            &connector_id,
            &format!("allow_{suffix}"),
            "direct_chat",
            &format!("chat_{suffix}"),
            true,
            "not_applicable",
            "valid",
            "healthy",
            ts,
            "redacted",
            &[("scope", "direct_chat")],
        )?;
        exec_insert(
            &conn,
            "INSERT INTO telegram_allowments (tenant_id, connector_id, allowment_id, scope_type, scope_id, provider_label, enabled, group_gate, validation_state, reason_code, validated_at, redaction_status, document_json) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                tenant_id,
                connector_id,
                format!("allow_{suffix}"),
                "direct_chat",
                format!("chat_{suffix}"),
                None::<String>,
                1i64,
                "not_applicable",
                "valid",
                "healthy",
                ts,
                "redacted",
                allowment_document
            ],
        )?;

        let smoke_document = telegram_smoke_evidence_document(
            &format!("telegram_smoke_{suffix}"),
            tenant_id,
            &connector_id,
            "skipped",
            "unavailable",
            "operator",
            "safe_credentials_unavailable",
            "live smoke skipped",
            ts,
            ts,
            "redacted",
            &[("policy", "structured_skip")],
        )?;
        exec_insert(
            &conn,
            "INSERT INTO telegram_smoke_evidence (smoke_evidence_id, tenant_id, connector_id, status, credential_mode, owner, reason, remaining_risk, validated_at, retention_expires_at, redaction_status, document_json) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                format!("telegram_smoke_{suffix}"),
                tenant_id,
                connector_id,
                "skipped",
                "unavailable",
                "operator",
                "safe_credentials_unavailable",
                "live smoke skipped",
                ts,
                ts,
                "redacted",
                smoke_document
            ],
        )?;

        let update_document = telegram_update_evidence_document(
            tenant_id,
            &connector_id,
            &format!("chat_{suffix}"),
            &format!("message_{suffix}"),
            &format!("update_{suffix}"),
            "accepted",
            "accepted",
            ts,
            ts,
            "redacted",
            &[("identityRule", "telegram_chat_message_id")],
        )?;
        exec_insert(
            &conn,
            "INSERT INTO telegram_update_evidence (tenant_id, connector_id, chat_id, message_id, update_id, route_outcome, reason_code, received_at, retention_expires_at, redaction_status, document_json) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            params![
                tenant_id,
                connector_id,
                format!("chat_{suffix}"),
                format!("message_{suffix}"),
                format!("update_{suffix}"),
                "accepted",
                "accepted",
                ts,
                ts,
                "redacted",
                update_document
            ],
        )?;
    }
    Ok(fixture)
}

/// Counts rows per r50 table (Go CountR50TelegramChannelConnectorRows).
pub fn count_r50_telegram_channel_connector_rows(
    store: &SQLiteStore,
) -> Result<HashMap<String, i64>, String> {
    let conn = crate::open_fixture_connection(store.db_path())?;
    let mut counts = HashMap::new();
    for table in R50_TELEGRAM_CHANNEL_CONNECTOR_TABLE_NAMES {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .map_err(|e| format!("count {table}: {e}"))?;
        counts.insert(table.to_string(), count);
    }
    Ok(counts)
}
