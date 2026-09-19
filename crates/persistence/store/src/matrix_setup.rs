//! SQLite CRUD for the Matrix channel-connector setup + evidence records.
//! Ported from `daemon/internal/store/matrix.go` (SaveMatrixHostedSetup,
//! GetMatrixHostedSetup, SaveMatrixRoutePolicy, GetMatrixRoutePolicy,
//! SaveMatrixSmokeEvidence, LatestMatrixSmokeEvidence, SaveMatrixEventEvidence,
//! ListMatrixEventEvidence). Record types are store-local; the tenant-binding
//! resolution is not ported.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use rusqlite::{Row, params};
use serde::{Deserialize, Serialize};

use crate::SQLiteStore;
use crate::crud::{now_rfc3339, null_string, opt_time_string};

/// Go `MatrixHostedSetupRecord` (stored in `matrix_hosted_setups`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatrixHostedSetupRecord {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub connector_id: String,
    pub connector_kind: String,
    pub display_name: String,
    pub status: String,
    pub terminal_state: String,
    pub bot_credential_state: String,
    pub homeserver_state: String,
    pub route_policy_state: String,
    pub delivery_eligible: bool,
    pub homeserver_binding_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
    pub redaction_status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validated_at: Option<DateTime<Utc>>,
    pub retention_expires_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homeserver_binding: Option<MatrixHomeserverBindingRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_policy: Option<MatrixRoutePolicyRecord>,
}

/// Go `MatrixHomeserverBindingRecord`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatrixHomeserverBindingRecord {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub homeserver_binding_id: String,
    pub homeserver_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub homeserver_name: String,
    pub bot_user_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bot_device_id: String,
    pub authorization_state: String,
    pub homeserver_capability_state: String,
    pub validated_at: DateTime<Utc>,
    pub redaction_status: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub safe_evidence: HashMap<String, String>,
}

/// Go `MatrixRoutePolicyRecord` (stored in `matrix_route_policies`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatrixRoutePolicyRecord {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub connector_id: String,
    pub homeserver_binding_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected_rooms: Vec<MatrixConversationRouteRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_direct_users: Vec<String>,
    pub room_invocation_gate: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub configured_commands: Vec<String>,
    pub encrypted_room_policy: String,
    pub validation_state: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
    pub validated_at: DateTime<Utc>,
    pub redaction_status: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub safe_evidence: HashMap<String, String>,
}

/// Go `MatrixConversationRouteRecord`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatrixConversationRouteRecord {
    pub conversation_id: String,
    pub conversation_type: String,
    pub room_selection_state: String,
    pub validation_state: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub redaction_status: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub safe_evidence: HashMap<String, String>,
}

/// Go `MatrixSmokeEvidenceRecord` (stored in `matrix_smoke_evidence`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatrixSmokeEvidenceRecord {
    pub smoke_evidence_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub connector_id: String,
    pub homeserver_binding_id: String,
    pub status: String,
    pub authorization_mode: String,
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

/// Go `MatrixEventEvidenceRecord` (stored in `matrix_event_evidence`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatrixEventEvidenceRecord {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub connector_id: String,
    pub homeserver_id: String,
    pub conversation_id: String,
    pub matrix_event_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sync_batch_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub transaction_id: String,
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

fn scan_hosted_setup(row: &Row) -> Result<MatrixHostedSetupRecord, String> {
    let raw: String = row.get(0).map_err(|e| e.to_string())?;
    decode_document(&raw, "matrix hosted setup")
}

fn scan_route_policy(row: &Row) -> Result<MatrixRoutePolicyRecord, String> {
    let raw: String = row.get(0).map_err(|e| e.to_string())?;
    decode_document(&raw, "matrix route policy")
}

fn scan_smoke(row: &Row) -> Result<MatrixSmokeEvidenceRecord, String> {
    let raw: String = row.get(0).map_err(|e| e.to_string())?;
    decode_document(&raw, "matrix smoke evidence")
}

fn scan_event_evidence(row: &Row) -> Result<MatrixEventEvidenceRecord, String> {
    let raw: String = row.get(0).map_err(|e| e.to_string())?;
    decode_document(&raw, "matrix event evidence")
}

/// Go `normalizeMatrixHostedSetupRecord`.
fn normalize_hosted_setup(mut record: MatrixHostedSetupRecord) -> MatrixHostedSetupRecord {
    let now = Utc::now();
    record.connector_kind = coalesce(&record.connector_kind, "matrix");
    record.status = coalesce(&record.status, "degraded");
    record.terminal_state = coalesce(&record.terminal_state, "action-required");
    record.bot_credential_state = coalesce(&record.bot_credential_state, "unknown");
    record.homeserver_state = coalesce(&record.homeserver_state, "unknown");
    record.route_policy_state = coalesce(&record.route_policy_state, "none");
    record.redaction_status = coalesce(&record.redaction_status, "redacted");
    if record.homeserver_binding_id.trim().is_empty() && !record.connector_id.trim().is_empty() {
        record.homeserver_binding_id = format!("matrix_homeserver_{}", record.connector_id);
    }
    if is_unset_time(&record.created_at) {
        record.created_at = now;
    }
    if is_unset_time(&record.updated_at) {
        record.updated_at = now;
    }
    if is_unset_time(&record.retention_expires_at) {
        record.retention_expires_at = now + Duration::days(90);
    }
    record
}

/// Go `normalizeMatrixRoutePolicyRecord`.
fn normalize_route_policy(mut record: MatrixRoutePolicyRecord) -> MatrixRoutePolicyRecord {
    let now = Utc::now();
    record.room_invocation_gate = coalesce(
        &record.room_invocation_gate,
        "bot_mention_or_command_required",
    );
    record.encrypted_room_policy = coalesce(&record.encrypted_room_policy, "unsupported");
    record.validation_state = coalesce(&record.validation_state, "valid");
    record.redaction_status = coalesce(&record.redaction_status, "redacted");
    if is_unset_time(&record.validated_at) {
        record.validated_at = now;
    }
    for room in record.selected_rooms.iter_mut() {
        room.conversation_type = coalesce(&room.conversation_type, "room");
        room.room_selection_state = coalesce(&room.room_selection_state, "selected");
        room.validation_state = coalesce(&room.validation_state, "valid");
        room.redaction_status = coalesce(&room.redaction_status, "redacted");
    }
    record
}

/// Go `normalizeMatrixSmokeEvidenceRecord`.
fn normalize_smoke(mut record: MatrixSmokeEvidenceRecord) -> MatrixSmokeEvidenceRecord {
    let now = Utc::now();
    record.status = coalesce(&record.status, "skipped");
    record.authorization_mode = coalesce(&record.authorization_mode, "unavailable");
    record.owner = coalesce(&record.owner, "operator");
    record.reason = coalesce(&record.reason, "safe_matrix_authorization_unavailable");
    record.redaction_status = coalesce(&record.redaction_status, "redacted");
    if record.smoke_evidence_id.trim().is_empty() {
        record.smoke_evidence_id = new_store_id("matrix_smoke");
    }
    if is_unset_time(&record.validated_at) {
        record.validated_at = now;
    }
    if is_unset_time(&record.retention_expires_at) {
        record.retention_expires_at = record.validated_at + Duration::days(90);
    }
    record
}

/// Go `normalizeMatrixEventEvidenceRecord`.
fn normalize_event_evidence(mut record: MatrixEventEvidenceRecord) -> MatrixEventEvidenceRecord {
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
    /// Go `SaveMatrixHostedSetup`.
    pub fn save_matrix_hosted_setup(&self, record: &MatrixHostedSetupRecord) -> Result<(), String> {
        let record = normalize_hosted_setup(record.clone());
        let document = serde_json::to_string(&record)
            .map_err(|e| format!("marshal matrix hosted setup: {e}"))?;

        self.conn
            .execute(
                r#"INSERT INTO matrix_hosted_setups (
                    tenant_id, connector_id, connector_kind, display_name, status, terminal_state,
                    bot_credential_state, homeserver_state, route_policy_state, delivery_eligible,
                    homeserver_binding_id, reason_code, redaction_status, created_at, updated_at,
                    validated_at, retention_expires_at, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
                ON CONFLICT(tenant_id, connector_id) DO UPDATE SET
                    connector_kind = excluded.connector_kind,
                    display_name = excluded.display_name,
                    status = excluded.status,
                    terminal_state = excluded.terminal_state,
                    bot_credential_state = excluded.bot_credential_state,
                    homeserver_state = excluded.homeserver_state,
                    route_policy_state = excluded.route_policy_state,
                    delivery_eligible = excluded.delivery_eligible,
                    homeserver_binding_id = excluded.homeserver_binding_id,
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
                    record.bot_credential_state,
                    record.homeserver_state,
                    record.route_policy_state,
                    i64::from(record.delivery_eligible),
                    record.homeserver_binding_id,
                    null_string(&record.reason_code),
                    record.redaction_status,
                    now_rfc3339(&record.created_at),
                    now_rfc3339(&record.updated_at),
                    opt_time_string(&record.validated_at),
                    now_rfc3339(&record.retention_expires_at),
                    document,
                ],
            )
            .map_err(|e| format!("save matrix hosted setup {}: {e}", record.connector_id))?;
        Ok(())
    }

    /// Go `GetMatrixHostedSetup` — the route policy is re-read from its own table.
    pub fn get_matrix_hosted_setup(
        &self,
        tenant_id: &str,
        connector_id: &str,
    ) -> Result<Option<MatrixHostedSetupRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT document_json FROM matrix_hosted_setups WHERE tenant_id = ?1 AND connector_id = ?2",
            )
            .map_err(|e| format!("get matrix hosted setup {connector_id}: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id.trim(), connector_id.trim()])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let mut record = scan_hosted_setup(row)?;
        record.route_policy = self.get_matrix_route_policy(tenant_id, connector_id)?;
        Ok(Some(record))
    }

    /// Go `SaveMatrixRoutePolicy`.
    pub fn save_matrix_route_policy(&self, record: &MatrixRoutePolicyRecord) -> Result<(), String> {
        let record = normalize_route_policy(record.clone());
        let document = serde_json::to_string(&record)
            .map_err(|e| format!("marshal matrix route policy: {e}"))?;

        self.conn
            .execute(
                r#"INSERT INTO matrix_route_policies (
                    tenant_id, connector_id, homeserver_binding_id, validation_state, reason_code,
                    validated_at, redaction_status, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                ON CONFLICT(tenant_id, connector_id) DO UPDATE SET
                    homeserver_binding_id = excluded.homeserver_binding_id,
                    validation_state = excluded.validation_state,
                    reason_code = excluded.reason_code,
                    validated_at = excluded.validated_at,
                    redaction_status = excluded.redaction_status,
                    document_json = excluded.document_json"#,
                params![
                    record.tenant_id,
                    record.connector_id,
                    record.homeserver_binding_id,
                    record.validation_state,
                    null_string(&record.reason_code),
                    now_rfc3339(&record.validated_at),
                    record.redaction_status,
                    document,
                ],
            )
            .map_err(|e| format!("save matrix route policy {}: {e}", record.connector_id))?;
        Ok(())
    }

    /// Go `GetMatrixRoutePolicy`.
    pub fn get_matrix_route_policy(
        &self,
        tenant_id: &str,
        connector_id: &str,
    ) -> Result<Option<MatrixRoutePolicyRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT document_json FROM matrix_route_policies WHERE tenant_id = ?1 AND connector_id = ?2",
            )
            .map_err(|e| format!("get matrix route policy {connector_id}: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id.trim(), connector_id.trim()])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_route_policy(row).map(Some)
    }

    /// Go `SaveMatrixSmokeEvidence`.
    pub fn save_matrix_smoke_evidence(
        &self,
        record: &MatrixSmokeEvidenceRecord,
    ) -> Result<(), String> {
        let record = normalize_smoke(record.clone());
        let document = serde_json::to_string(&record)
            .map_err(|e| format!("marshal matrix smoke evidence: {e}"))?;

        self.conn
            .execute(
                r#"INSERT INTO matrix_smoke_evidence (
                    smoke_evidence_id, tenant_id, connector_id, homeserver_binding_id, status,
                    authorization_mode, owner, reason, remaining_risk, validated_at,
                    retention_expires_at, redaction_status, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                ON CONFLICT(smoke_evidence_id) DO UPDATE SET
                    status = excluded.status,
                    authorization_mode = excluded.authorization_mode,
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
                    record.homeserver_binding_id,
                    record.status,
                    record.authorization_mode,
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
                    "save matrix smoke evidence {}: {e}",
                    record.smoke_evidence_id
                )
            })?;
        Ok(())
    }

    /// Go `LatestMatrixSmokeEvidence`.
    pub fn latest_matrix_smoke_evidence(
        &self,
        tenant_id: &str,
        connector_id: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<MatrixSmokeEvidenceRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT document_json
                FROM matrix_smoke_evidence
                WHERE tenant_id = ?1 AND connector_id = ?2 AND retention_expires_at > ?3
                ORDER BY validated_at DESC, smoke_evidence_id DESC
                LIMIT 1"#,
            )
            .map_err(|e| format!("latest matrix smoke evidence {connector_id}: {e}"))?;
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

    /// Go `SaveMatrixEventEvidence`.
    pub fn save_matrix_event_evidence(
        &self,
        record: &MatrixEventEvidenceRecord,
    ) -> Result<(), String> {
        let record = normalize_event_evidence(record.clone());
        let document = serde_json::to_string(&record)
            .map_err(|e| format!("marshal matrix event evidence: {e}"))?;

        self.conn
            .execute(
                r#"INSERT INTO matrix_event_evidence (
                    tenant_id, connector_id, homeserver_id, conversation_id, matrix_event_id,
                    sync_batch_id, transaction_id, route_outcome, reason_code, received_at,
                    retention_expires_at, redaction_status, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                ON CONFLICT(tenant_id, connector_id, homeserver_id, conversation_id, matrix_event_id) DO UPDATE SET
                    sync_batch_id = excluded.sync_batch_id,
                    transaction_id = excluded.transaction_id,
                    route_outcome = excluded.route_outcome,
                    reason_code = excluded.reason_code,
                    received_at = excluded.received_at,
                    retention_expires_at = excluded.retention_expires_at,
                    redaction_status = excluded.redaction_status,
                    document_json = excluded.document_json"#,
                params![
                    record.tenant_id,
                    record.connector_id,
                    record.homeserver_id,
                    record.conversation_id,
                    record.matrix_event_id,
                    null_string(&record.sync_batch_id),
                    null_string(&record.transaction_id),
                    record.route_outcome,
                    null_string(&record.reason_code),
                    now_rfc3339(&record.received_at),
                    now_rfc3339(&record.retention_expires_at),
                    record.redaction_status,
                    document,
                ],
            )
            .map_err(|e| format!("save matrix event evidence {}: {e}", record.matrix_event_id))?;
        Ok(())
    }

    /// Go `ListMatrixEventEvidence` — unexpired event evidence, newest first.
    pub fn list_matrix_event_evidence(
        &self,
        tenant_id: &str,
        connector_id: &str,
        now: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<MatrixEventEvidenceRecord>, String> {
        let limit = if limit <= 0 || limit > 100 {
            100
        } else {
            limit
        };
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT document_json
                FROM matrix_event_evidence
                WHERE tenant_id = ?1 AND connector_id = ?2 AND retention_expires_at > ?3
                ORDER BY received_at DESC, matrix_event_id DESC
                LIMIT ?4"#,
            )
            .map_err(|e| format!("list matrix event evidence: {e}"))?;
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
            items.push(scan_event_evidence(row)?);
        }
        Ok(items)
    }
}
