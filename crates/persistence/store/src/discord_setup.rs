//! SQLite CRUD for the Discord channel-connector setup + evidence records.
//! Ported from `daemon/internal/store/discord_setup.go` (SaveDiscordHostedSetup,
//! GetDiscordHostedSetup, SaveDiscordDestinationValidation,
//! ListDiscordDestinationValidations, SaveDiscordSmokeEvidence,
//! LatestDiscordSmokeEvidence). Record types are store-local; the tenant-binding
//! resolution in the Go normalizers is not ported (tenant_id is written as-is).

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use rusqlite::{Row, params};
use serde::{Deserialize, Serialize};

use crate::SQLiteStore;
use crate::crud::{now_rfc3339, null_string, opt_time_string};

/// Go `DiscordHostedSetupRecord` (the record stored in `discord_hosted_setups`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscordHostedSetupRecord {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub connector_id: String,
    pub connector_kind: String,
    pub display_name: String,
    pub status: String,
    pub readiness_state: String,
    pub hosted_ready: bool,
    pub credential_state: String,
    pub respond_in_dm: bool,
    pub require_mention: bool,
    pub delivery_mode: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
    pub redaction_status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validated_at: Option<DateTime<Utc>>,
    pub retention_expires_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub destinations: Vec<DiscordDestinationValidationRecord>,
}

/// Go `DiscordDestinationValidationRecord`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscordDestinationValidationRecord {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub connector_id: String,
    pub destination_id: String,
    pub destination_type: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider_label: String,
    pub selected: bool,
    pub validation_state: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
    pub validated_at: DateTime<Utc>,
    pub redaction_status: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub safe_evidence: HashMap<String, String>,
}

/// Go `DiscordSmokeEvidenceRecord`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscordSmokeEvidenceRecord {
    pub smoke_evidence_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub connector_id: String,
    pub status: String,
    pub credential_mode: String,
    pub owner: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub remaining_risk: String,
    pub validated_at: DateTime<Utc>,
    pub retention_expires_at: DateTime<Utc>,
    pub redaction_status: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub safe_evidence: HashMap<String, String>,
}

fn new_store_id(prefix: &str) -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("{prefix}_{}", &hex[..16])
}

fn is_unset_time(dt: &DateTime<Utc>) -> bool {
    dt.timestamp() == 0 && dt.timestamp_subsec_nanos() == 0
}

fn coalesce(value: &str, default: &str) -> String {
    if value.trim().is_empty() {
        default.to_string()
    } else {
        value.to_string()
    }
}

fn decode_document<T: serde::de::DeserializeOwned>(raw: &str, what: &str) -> Result<T, String> {
    serde_json::from_str(raw).map_err(|e| format!("decode {what}: {e}"))
}

fn scan_hosted_setup(row: &Row) -> Result<DiscordHostedSetupRecord, String> {
    let raw: String = row.get(0).map_err(|e| e.to_string())?;
    decode_document(&raw, "discord hosted setup")
}

fn scan_destination(row: &Row) -> Result<DiscordDestinationValidationRecord, String> {
    let raw: String = row.get(0).map_err(|e| e.to_string())?;
    decode_document(&raw, "discord destination validation")
}

fn scan_smoke(row: &Row) -> Result<DiscordSmokeEvidenceRecord, String> {
    let raw: String = row.get(0).map_err(|e| e.to_string())?;
    decode_document(&raw, "discord smoke evidence")
}

/// Go `normalizeDiscordHostedSetupRecord`.
fn normalize_hosted_setup(mut record: DiscordHostedSetupRecord) -> DiscordHostedSetupRecord {
    let now = Utc::now();
    record.connector_kind = coalesce(&record.connector_kind, "discord");
    record.status = coalesce(&record.status, "degraded");
    record.readiness_state = coalesce(&record.readiness_state, "degraded_needs_repair");
    record.credential_state = coalesce(&record.credential_state, "missing");
    record.delivery_mode = coalesce(&record.delivery_mode, "gateway");
    record.redaction_status = coalesce(&record.redaction_status, "redacted");
    if is_unset_time(&record.created_at) {
        record.created_at = now;
    }
    if is_unset_time(&record.updated_at) {
        record.updated_at = now;
    }
    if is_unset_time(&record.retention_expires_at) {
        record.retention_expires_at = record.updated_at + Duration::days(90);
    }
    record.hosted_ready = record.readiness_state == "hosted_ready";
    record
}

/// Go `normalizeDiscordDestinationValidationRecord`.
fn normalize_destination(
    mut record: DiscordDestinationValidationRecord,
) -> DiscordDestinationValidationRecord {
    record.validation_state = coalesce(&record.validation_state, "invalid");
    record.redaction_status = coalesce(&record.redaction_status, "redacted");
    if is_unset_time(&record.validated_at) {
        record.validated_at = Utc::now();
    }
    record
}

/// Go `normalizeDiscordSmokeEvidenceRecord`.
fn normalize_smoke(mut record: DiscordSmokeEvidenceRecord) -> DiscordSmokeEvidenceRecord {
    let now = Utc::now();
    record.status = coalesce(&record.status, "skipped");
    record.credential_mode = coalesce(&record.credential_mode, "unavailable");
    record.owner = coalesce(&record.owner, "operator");
    record.reason = coalesce(&record.reason, "safe_credentials_unavailable");
    record.redaction_status = coalesce(&record.redaction_status, "redacted");
    if record.smoke_evidence_id.trim().is_empty() {
        record.smoke_evidence_id = new_store_id("discord_smoke");
    }
    if is_unset_time(&record.validated_at) {
        record.validated_at = now;
    }
    if is_unset_time(&record.retention_expires_at) {
        record.retention_expires_at = record.validated_at + Duration::days(90);
    }
    record
}

impl SQLiteStore {
    /// Go `SaveDiscordHostedSetup`.
    pub fn save_discord_hosted_setup(
        &self,
        record: &DiscordHostedSetupRecord,
    ) -> Result<(), String> {
        let record = normalize_hosted_setup(record.clone());
        let document = serde_json::to_string(&record)
            .map_err(|e| format!("marshal discord hosted setup: {e}"))?;

        self.conn
            .execute(
                r#"INSERT INTO discord_hosted_setups (
                    tenant_id, connector_id, connector_kind, display_name, status, readiness_state,
                    credential_state, respond_in_dm, require_mention, delivery_mode, reason_code,
                    redaction_status, created_at, updated_at, validated_at, retention_expires_at,
                    document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
                ON CONFLICT(tenant_id, connector_id) DO UPDATE SET
                    connector_kind = excluded.connector_kind,
                    display_name = excluded.display_name,
                    status = excluded.status,
                    readiness_state = excluded.readiness_state,
                    credential_state = excluded.credential_state,
                    respond_in_dm = excluded.respond_in_dm,
                    require_mention = excluded.require_mention,
                    delivery_mode = excluded.delivery_mode,
                    reason_code = excluded.reason_code,
                    redaction_status = excluded.redaction_status,
                    updated_at = excluded.updated_at,
                    validated_at = excluded.validated_at,
                    retention_expires_at = excluded.retention_expires_at,
                    document_json = excluded.document_json"#,
                params![
                    record.tenant_id,
                    record.connector_id,
                    record.connector_kind,
                    record.display_name,
                    record.status,
                    record.readiness_state,
                    record.credential_state,
                    i64::from(record.respond_in_dm),
                    i64::from(record.require_mention),
                    record.delivery_mode,
                    null_string(&record.reason_code),
                    record.redaction_status,
                    now_rfc3339(&record.created_at),
                    now_rfc3339(&record.updated_at),
                    opt_time_string(&record.validated_at),
                    now_rfc3339(&record.retention_expires_at),
                    document,
                ],
            )
            .map_err(|e| format!("save discord hosted setup {}: {e}", record.connector_id))?;
        Ok(())
    }

    /// Go `GetDiscordHostedSetup` — the destinations are re-read from their own
    /// table (authoritative, like Go).
    pub fn get_discord_hosted_setup(
        &self,
        tenant_id: &str,
        connector_id: &str,
    ) -> Result<Option<DiscordHostedSetupRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT document_json FROM discord_hosted_setups WHERE tenant_id = ?1 AND connector_id = ?2",
            )
            .map_err(|e| format!("get discord hosted setup {connector_id}: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id.trim(), connector_id.trim()])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let mut record = scan_hosted_setup(row)?;
        record.destinations = self.list_discord_destination_validations(tenant_id, connector_id)?;
        Ok(Some(record))
    }

    /// Go `SaveDiscordDestinationValidation`.
    pub fn save_discord_destination_validation(
        &self,
        record: &DiscordDestinationValidationRecord,
    ) -> Result<(), String> {
        let record = normalize_destination(record.clone());
        let document = serde_json::to_string(&record)
            .map_err(|e| format!("marshal discord destination validation: {e}"))?;

        self.conn
            .execute(
                r#"INSERT INTO discord_destination_validations (
                    tenant_id, connector_id, destination_id, destination_type, provider_label,
                    selected, validation_state, reason_code, validated_at, redaction_status,
                    document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                ON CONFLICT(tenant_id, connector_id, destination_type, destination_id) DO UPDATE SET
                    provider_label = excluded.provider_label,
                    selected = excluded.selected,
                    validation_state = excluded.validation_state,
                    reason_code = excluded.reason_code,
                    validated_at = excluded.validated_at,
                    redaction_status = excluded.redaction_status,
                    document_json = excluded.document_json"#,
                params![
                    record.tenant_id,
                    record.connector_id,
                    record.destination_id,
                    record.destination_type,
                    null_string(&record.provider_label),
                    i64::from(record.selected),
                    record.validation_state,
                    null_string(&record.reason_code),
                    now_rfc3339(&record.validated_at),
                    record.redaction_status,
                    document,
                ],
            )
            .map_err(|e| {
                format!(
                    "save discord destination validation {}: {e}",
                    record.destination_id
                )
            })?;
        Ok(())
    }

    /// Go `ListDiscordDestinationValidations`.
    pub fn list_discord_destination_validations(
        &self,
        tenant_id: &str,
        connector_id: &str,
    ) -> Result<Vec<DiscordDestinationValidationRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT document_json
                FROM discord_destination_validations
                WHERE tenant_id = ?1 AND connector_id = ?2
                ORDER BY destination_type ASC, destination_id ASC"#,
            )
            .map_err(|e| format!("list discord destination validations: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id.trim(), connector_id.trim()])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_destination(row)?);
        }
        Ok(items)
    }

    /// Go `SaveDiscordSmokeEvidence`.
    pub fn save_discord_smoke_evidence(
        &self,
        record: &DiscordSmokeEvidenceRecord,
    ) -> Result<(), String> {
        let record = normalize_smoke(record.clone());
        let document = serde_json::to_string(&record)
            .map_err(|e| format!("marshal discord smoke evidence: {e}"))?;

        self.conn
            .execute(
                r#"INSERT INTO discord_smoke_evidence (
                    smoke_evidence_id, tenant_id, connector_id, status, credential_mode, owner,
                    reason, remaining_risk, validated_at, retention_expires_at, redaction_status,
                    document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                ON CONFLICT(smoke_evidence_id) DO UPDATE SET
                    tenant_id = excluded.tenant_id,
                    connector_id = excluded.connector_id,
                    status = excluded.status,
                    credential_mode = excluded.credential_mode,
                    owner = excluded.owner,
                    reason = excluded.reason,
                    remaining_risk = excluded.remaining_risk,
                    validated_at = excluded.validated_at,
                    retention_expires_at = excluded.retention_expires_at,
                    redaction_status = excluded.redaction_status,
                    document_json = excluded.document_json"#,
                params![
                    record.smoke_evidence_id,
                    record.tenant_id,
                    record.connector_id,
                    record.status,
                    record.credential_mode,
                    record.owner,
                    record.reason,
                    null_string(&record.remaining_risk),
                    now_rfc3339(&record.validated_at),
                    now_rfc3339(&record.retention_expires_at),
                    record.redaction_status,
                    document,
                ],
            )
            .map_err(|e| {
                format!(
                    "save discord smoke evidence {}: {e}",
                    record.smoke_evidence_id
                )
            })?;
        Ok(())
    }

    /// Go `LatestDiscordSmokeEvidence` — the newest unexpired evidence row.
    pub fn latest_discord_smoke_evidence(
        &self,
        tenant_id: &str,
        connector_id: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<DiscordSmokeEvidenceRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT document_json
                FROM discord_smoke_evidence
                WHERE tenant_id = ?1 AND connector_id = ?2 AND retention_expires_at > ?3
                ORDER BY validated_at DESC, smoke_evidence_id DESC
                LIMIT 1"#,
            )
            .map_err(|e| format!("latest discord smoke evidence {connector_id}: {e}"))?;
        let mut rows = stmt
            .query(params![
                tenant_id.trim(),
                connector_id.trim(),
                now_rfc3339(&now)
            ])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_smoke(row).map(Some)
    }
}
