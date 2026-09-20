//! SQLite CRUD for the Telegram channel-connector setup + evidence records.
//! Ported from `daemon/internal/store/telegram_setup.go` (SaveTelegramHostedSetup,
//! GetTelegramHostedSetup, SaveTelegramAllowment, ListTelegramAllowments,
//! SaveTelegramSmokeEvidence, LatestTelegramSmokeEvidence,
//! SaveTelegramUpdateEvidence, ListTelegramUpdateEvidence). Record types are
//! store-local; the tenant-binding resolution is not ported.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use rusqlite::{Row, params};
use serde::{Deserialize, Serialize};

use crate::SQLiteStore;
use crate::crud::{now_rfc3339, null_string, opt_time_string};

/// Go `TelegramHostedSetupRecord` (stored in `telegram_hosted_setups`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramHostedSetupRecord {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub connector_id: String,
    pub connector_kind: String,
    pub display_name: String,
    pub status: String,
    pub terminal_state: String,
    pub hosted_ready: bool,
    pub credential_state: String,
    pub allowment_state: String,
    pub group_behavior: String,
    pub delivery_eligible: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
    pub redaction_status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validated_at: Option<DateTime<Utc>>,
    pub retention_expires_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_binding: Option<ConnectorAccountBindingSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowments: Vec<TelegramAllowmentRecord>,
}

/// Go `ConnectorAccountBindingSummary`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorAccountBindingSummary {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub connector_id: String,
    pub connector_account_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub display_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider_account_hint: String,
    pub redaction_status: String,
    pub updated_at: DateTime<Utc>,
}

/// Go `TelegramAllowmentRecord` (stored in `telegram_allowments`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramAllowmentRecord {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub connector_id: String,
    pub allowment_id: String,
    #[serde(
        rename = "telegramScopeType",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub scope_type: String,
    #[serde(
        rename = "telegramScopeId",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub scope_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider_label: String,
    pub enabled: bool,
    pub group_gate: String,
    pub validation_state: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
    pub validated_at: DateTime<Utc>,
    pub redaction_status: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub safe_evidence: HashMap<String, String>,
}

/// Go `TelegramSmokeEvidenceRecord` (stored in `telegram_smoke_evidence`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramSmokeEvidenceRecord {
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

/// Go `TelegramUpdateEvidenceRecord` (stored in `telegram_update_evidence`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramUpdateEvidenceRecord {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub connector_id: String,
    #[serde(
        rename = "telegramChatId",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub chat_id: String,
    #[serde(
        rename = "telegramMessageId",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub message_id: String,
    #[serde(
        rename = "telegramUpdateId",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub update_id: String,
    pub route_outcome: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
    pub received_at: DateTime<Utc>,
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

fn scan_hosted_setup(row: &Row) -> Result<TelegramHostedSetupRecord, String> {
    let raw: String = row.get(0).map_err(|e| e.to_string())?;
    decode_document(&raw, "telegram hosted setup")
}

fn scan_allowment(row: &Row) -> Result<TelegramAllowmentRecord, String> {
    let raw: String = row.get(0).map_err(|e| e.to_string())?;
    decode_document(&raw, "telegram allowment")
}

fn scan_smoke(row: &Row) -> Result<TelegramSmokeEvidenceRecord, String> {
    let raw: String = row.get(0).map_err(|e| e.to_string())?;
    decode_document(&raw, "telegram smoke evidence")
}

fn scan_update_evidence(row: &Row) -> Result<TelegramUpdateEvidenceRecord, String> {
    let raw: String = row.get(0).map_err(|e| e.to_string())?;
    decode_document(&raw, "telegram update evidence")
}

/// Go `normalizeTelegramHostedSetupRecord`.
fn normalize_hosted_setup(mut record: TelegramHostedSetupRecord) -> TelegramHostedSetupRecord {
    let now = Utc::now();
    record.connector_kind = coalesce(&record.connector_kind, "telegram");
    record.status = coalesce(&record.status, "degraded");
    record.terminal_state = coalesce(&record.terminal_state, "action-required");
    record.credential_state = coalesce(&record.credential_state, "missing");
    record.allowment_state = coalesce(&record.allowment_state, "none");
    record.group_behavior = coalesce(&record.group_behavior, "disabled");
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
    if let Some(binding) = record.account_binding.as_mut() {
        binding.tenant_id = coalesce(&binding.tenant_id, &record.tenant_id);
        binding.connector_id = coalesce(&binding.connector_id, &record.connector_id);
        binding.redaction_status = coalesce(&binding.redaction_status, "redacted");
        if is_unset_time(&binding.updated_at) {
            binding.updated_at = record.updated_at;
        }
    }
    record.hosted_ready = record.terminal_state == "ready";
    record
}

/// Go `normalizeTelegramAllowmentRecord`.
fn normalize_allowment(mut record: TelegramAllowmentRecord) -> TelegramAllowmentRecord {
    record.scope_type = coalesce(&record.scope_type, "direct_chat");
    record.group_gate = coalesce(&record.group_gate, "not_applicable");
    record.validation_state = coalesce(&record.validation_state, "invalid");
    record.redaction_status = coalesce(&record.redaction_status, "redacted");
    if record.allowment_id.trim().is_empty() {
        record.allowment_id = new_store_id("telegram_allowment");
    }
    if is_unset_time(&record.validated_at) {
        record.validated_at = Utc::now();
    }
    record
}

/// Go `normalizeTelegramSmokeEvidenceRecord`.
fn normalize_smoke(mut record: TelegramSmokeEvidenceRecord) -> TelegramSmokeEvidenceRecord {
    let now = Utc::now();
    record.status = coalesce(&record.status, "skipped");
    record.credential_mode = coalesce(&record.credential_mode, "unavailable");
    record.owner = coalesce(&record.owner, "operator");
    record.reason = coalesce(&record.reason, "safe_credentials_unavailable");
    record.redaction_status = coalesce(&record.redaction_status, "redacted");
    if record.smoke_evidence_id.trim().is_empty() {
        record.smoke_evidence_id = new_store_id("telegram_smoke");
    }
    if is_unset_time(&record.validated_at) {
        record.validated_at = now;
    }
    if is_unset_time(&record.retention_expires_at) {
        record.retention_expires_at = record.validated_at + Duration::days(90);
    }
    record
}

/// Go `normalizeTelegramUpdateEvidenceRecord`.
fn normalize_update_evidence(
    mut record: TelegramUpdateEvidenceRecord,
) -> TelegramUpdateEvidenceRecord {
    let now = Utc::now();
    record.route_outcome = coalesce(&record.route_outcome, "accepted");
    record.redaction_status = coalesce(&record.redaction_status, "redacted");
    if is_unset_time(&record.received_at) {
        record.received_at = now;
    }
    if is_unset_time(&record.retention_expires_at) {
        record.retention_expires_at = record.received_at + Duration::days(90);
    }
    record
}

impl SQLiteStore {
    /// Go `SaveTelegramHostedSetup`.
    pub fn save_telegram_hosted_setup(
        &self,
        record: &TelegramHostedSetupRecord,
    ) -> Result<(), String> {
        let record = normalize_hosted_setup(record.clone());
        let document = serde_json::to_string(&record)
            .map_err(|e| format!("marshal telegram hosted setup: {e}"))?;

        self.conn
            .execute(
                r#"INSERT INTO telegram_hosted_setups (
                    tenant_id, connector_id, connector_kind, display_name, status, terminal_state,
                    credential_state, allowment_state, group_behavior, delivery_eligible, reason_code,
                    redaction_status, created_at, updated_at, validated_at, retention_expires_at,
                    document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
                ON CONFLICT(tenant_id, connector_id) DO UPDATE SET
                    connector_kind = excluded.connector_kind,
                    display_name = excluded.display_name,
                    status = excluded.status,
                    terminal_state = excluded.terminal_state,
                    credential_state = excluded.credential_state,
                    allowment_state = excluded.allowment_state,
                    group_behavior = excluded.group_behavior,
                    delivery_eligible = excluded.delivery_eligible,
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
                    record.terminal_state,
                    record.credential_state,
                    record.allowment_state,
                    record.group_behavior,
                    i64::from(record.delivery_eligible),
                    null_string(&record.reason_code),
                    record.redaction_status,
                    now_rfc3339(&record.created_at),
                    now_rfc3339(&record.updated_at),
                    opt_time_string(&record.validated_at),
                    now_rfc3339(&record.retention_expires_at),
                    document,
                ],
            )
            .map_err(|e| format!("save telegram hosted setup {}: {e}", record.connector_id))?;
        Ok(())
    }

    /// Go `GetTelegramHostedSetup` — the allowments are re-read from their own
    /// table.
    pub fn get_telegram_hosted_setup(
        &self,
        tenant_id: &str,
        connector_id: &str,
    ) -> Result<Option<TelegramHostedSetupRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT document_json FROM telegram_hosted_setups WHERE tenant_id = ?1 AND connector_id = ?2",
            )
            .map_err(|e| format!("get telegram hosted setup {connector_id}: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id.trim(), connector_id.trim()])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let mut record = scan_hosted_setup(row)?;
        record.allowments = self.list_telegram_allowments(tenant_id, connector_id)?;
        Ok(Some(record))
    }

    /// Go `SaveTelegramAllowment`.
    pub fn save_telegram_allowment(&self, record: &TelegramAllowmentRecord) -> Result<(), String> {
        let record = normalize_allowment(record.clone());
        let document = serde_json::to_string(&record)
            .map_err(|e| format!("marshal telegram allowment: {e}"))?;

        self.conn
            .execute(
                r#"INSERT INTO telegram_allowments (
                    tenant_id, connector_id, allowment_id, scope_type, scope_id, provider_label,
                    enabled, group_gate, validation_state, reason_code, validated_at, redaction_status,
                    document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                ON CONFLICT(tenant_id, connector_id, allowment_id) DO UPDATE SET
                    scope_type = excluded.scope_type,
                    scope_id = excluded.scope_id,
                    provider_label = excluded.provider_label,
                    enabled = excluded.enabled,
                    group_gate = excluded.group_gate,
                    validation_state = excluded.validation_state,
                    reason_code = excluded.reason_code,
                    validated_at = excluded.validated_at,
                    redaction_status = excluded.redaction_status,
                    document_json = excluded.document_json"#,
                params![
                    record.tenant_id,
                    record.connector_id,
                    record.allowment_id,
                    record.scope_type,
                    record.scope_id,
                    null_string(&record.provider_label),
                    i64::from(record.enabled),
                    record.group_gate,
                    record.validation_state,
                    null_string(&record.reason_code),
                    now_rfc3339(&record.validated_at),
                    record.redaction_status,
                    document,
                ],
            )
            .map_err(|e| format!("save telegram allowment {}: {e}", record.allowment_id))?;
        Ok(())
    }

    /// Go `ListTelegramAllowments`.
    pub fn list_telegram_allowments(
        &self,
        tenant_id: &str,
        connector_id: &str,
    ) -> Result<Vec<TelegramAllowmentRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT document_json
                FROM telegram_allowments
                WHERE tenant_id = ?1 AND connector_id = ?2
                ORDER BY scope_type ASC, allowment_id ASC"#,
            )
            .map_err(|e| format!("list telegram allowments: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id.trim(), connector_id.trim()])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_allowment(row)?);
        }
        Ok(items)
    }

    /// Go `SaveTelegramSmokeEvidence`.
    pub fn save_telegram_smoke_evidence(
        &self,
        record: &TelegramSmokeEvidenceRecord,
    ) -> Result<(), String> {
        let record = normalize_smoke(record.clone());
        let document = serde_json::to_string(&record)
            .map_err(|e| format!("marshal telegram smoke evidence: {e}"))?;

        self.conn
            .execute(
                r#"INSERT INTO telegram_smoke_evidence (
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
                    "save telegram smoke evidence {}: {e}",
                    record.smoke_evidence_id
                )
            })?;
        Ok(())
    }

    /// Go `LatestTelegramSmokeEvidence`.
    pub fn latest_telegram_smoke_evidence(
        &self,
        tenant_id: &str,
        connector_id: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<TelegramSmokeEvidenceRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT document_json
                FROM telegram_smoke_evidence
                WHERE tenant_id = ?1 AND connector_id = ?2 AND retention_expires_at > ?3
                ORDER BY validated_at DESC, smoke_evidence_id DESC
                LIMIT 1"#,
            )
            .map_err(|e| format!("latest telegram smoke evidence {connector_id}: {e}"))?;
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

    /// Go `SaveTelegramUpdateEvidence`.
    pub fn save_telegram_update_evidence(
        &self,
        record: &TelegramUpdateEvidenceRecord,
    ) -> Result<(), String> {
        let record = normalize_update_evidence(record.clone());
        let document = serde_json::to_string(&record)
            .map_err(|e| format!("marshal telegram update evidence: {e}"))?;

        self.conn
            .execute(
                r#"INSERT INTO telegram_update_evidence (
                    tenant_id, connector_id, chat_id, message_id, update_id, route_outcome,
                    reason_code, received_at, retention_expires_at, redaction_status, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                ON CONFLICT(tenant_id, connector_id, chat_id, message_id, update_id) DO UPDATE SET
                    route_outcome = excluded.route_outcome,
                    reason_code = excluded.reason_code,
                    received_at = excluded.received_at,
                    retention_expires_at = excluded.retention_expires_at,
                    redaction_status = excluded.redaction_status,
                    document_json = excluded.document_json"#,
                params![
                    record.tenant_id,
                    record.connector_id,
                    record.chat_id,
                    record.message_id,
                    record.update_id,
                    record.route_outcome,
                    null_string(&record.reason_code),
                    now_rfc3339(&record.received_at),
                    now_rfc3339(&record.retention_expires_at),
                    record.redaction_status,
                    document,
                ],
            )
            .map_err(|e| {
                format!(
                    "save telegram update evidence {}/{}/{}: {e}",
                    record.chat_id, record.message_id, record.update_id
                )
            })?;
        Ok(())
    }

    /// Go `ListTelegramUpdateEvidence` — unexpired update evidence, newest first.
    pub fn list_telegram_update_evidence(
        &self,
        tenant_id: &str,
        connector_id: &str,
        now: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<TelegramUpdateEvidenceRecord>, String> {
        let limit = if limit <= 0 || limit > 100 {
            100
        } else {
            limit
        };
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT document_json
                FROM telegram_update_evidence
                WHERE tenant_id = ?1 AND connector_id = ?2 AND retention_expires_at > ?3
                ORDER BY received_at DESC, update_id DESC
                LIMIT ?4"#,
            )
            .map_err(|e| format!("list telegram update evidence: {e}"))?;
        let mut rows = stmt
            .query(params![
                tenant_id.trim(),
                connector_id.trim(),
                now_rfc3339(&now),
                limit
            ])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_update_evidence(row)?);
        }
        Ok(items)
    }
}
