//! Roadmap 49 discord production-hardening fixture (port of
//! daemon/internal/store/migrationfixture/r49_discord_hardening.go).
//!
//! The Go fixture seeds via the store accessors SaveDiscordHostedSetup /
//! SaveDiscordDestinationValidation, which are not yet ported to kura-store.
//! The tables exist (migration v45) and the rows below replicate those
//! accessors' exact column writes and document_json payloads.

use std::collections::HashMap;

use rusqlite::params;

use kura_store::SQLiteStore;

use crate::FIXTURE_TIMESTAMP;
use crate::records::{discord_destination_validation_document, discord_hosted_setup_document};
use crate::seeds::exec_insert;

/// Table names expected from the Roadmap 49 storage migration (migration v45).
pub static R49_DISCORD_HARDENING_TABLE_NAMES: [&str; 3] = [
    "discord_hosted_setups",
    "discord_destination_validations",
    "discord_smoke_evidence",
];

/// Expected per-table row counts after seeding (Go R49DiscordHardeningFixture).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct R49DiscordHardeningFixture {
    pub tenant_ids: Vec<String>,
    pub expected_row_count: HashMap<String, i64>,
}

#[must_use]
pub fn build_r49_discord_hardening_fixture() -> R49DiscordHardeningFixture {
    let counts = HashMap::from([
        ("discord_hosted_setups".to_string(), 2),
        ("discord_destination_validations".to_string(), 2),
        ("discord_smoke_evidence".to_string(), 2),
    ]);
    R49DiscordHardeningFixture {
        tenant_ids: vec![
            "ten_discord_alpha".to_string(),
            "ten_discord_beta".to_string(),
        ],
        expected_row_count: counts,
    }
}

/// Seeds two tenants × every r49 table. Requires the store at head schema (v45+).
pub fn seed_r49_discord_hardening_rows(
    store: &SQLiteStore,
) -> Result<R49DiscordHardeningFixture, String> {
    let fixture = build_r49_discord_hardening_fixture();
    let conn = crate::open_fixture_connection(store.db_path())?;
    let ts = FIXTURE_TIMESTAMP;

    for (index, tenant_id) in fixture.tenant_ids.iter().enumerate() {
        let suffix = (index + 1).to_string();
        let connector_id = format!("discord-r49-{suffix}");

        let hosted_document = discord_hosted_setup_document(
            tenant_id,
            &connector_id,
            "discord",
            "Discord R49",
            "degraded",
            "degraded_needs_repair",
            "valid",
            true,
            true,
            "gateway",
            "destination_validation_failed",
            "redacted",
            ts,
            ts,
            ts,
            ts,
        )?;
        exec_insert(
            &conn,
            "INSERT INTO discord_hosted_setups (tenant_id, connector_id, connector_kind, display_name, status, readiness_state, credential_state, respond_in_dm, require_mention, delivery_mode, reason_code, redaction_status, created_at, updated_at, validated_at, retention_expires_at, document_json) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                tenant_id,
                connector_id,
                "discord",
                "Discord R49",
                "degraded",
                "degraded_needs_repair",
                "valid",
                1i64,
                1i64,
                "gateway",
                "destination_validation_failed",
                "redacted",
                ts,
                ts,
                ts,
                ts,
                hosted_document
            ],
        )?;

        let destination_document = discord_destination_validation_document(
            tenant_id,
            &connector_id,
            &format!("channel_{suffix}"),
            "channel",
            true,
            "missing_permission",
            "permission_missing",
            ts,
            "redacted",
            &[("permission", "send_messages")],
        )?;
        exec_insert(
            &conn,
            "INSERT INTO discord_destination_validations (tenant_id, connector_id, destination_id, destination_type, provider_label, selected, validation_state, reason_code, validated_at, redaction_status, document_json) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            params![
                tenant_id,
                connector_id,
                format!("channel_{suffix}"),
                "channel",
                None::<String>,
                1i64,
                "missing_permission",
                "permission_missing",
                ts,
                "redacted",
                destination_document
            ],
        )?;

        exec_insert(
            &conn,
            "INSERT INTO discord_smoke_evidence (smoke_evidence_id, tenant_id, connector_id, status, credential_mode, owner, reason, remaining_risk, validated_at, retention_expires_at, redaction_status, document_json) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                format!("discord_smoke_{suffix}"),
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
                "{\"status\":\"skipped\"}"
            ],
        )?;
    }
    Ok(fixture)
}

/// Counts rows per r49 table (Go CountR49DiscordHardeningRows).
pub fn count_r49_discord_hardening_rows(
    store: &SQLiteStore,
) -> Result<HashMap<String, i64>, String> {
    let conn = crate::open_fixture_connection(store.db_path())?;
    let mut counts = HashMap::new();
    for table in R49_DISCORD_HARDENING_TABLE_NAMES {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .map_err(|e| format!("count {table}: {e}"))?;
        counts.insert(table.to_string(), count);
    }
    Ok(counts)
}
