//! SQLite CRUD for the Slack channel-connector setup + evidence records.
//! Ported from `daemon/internal/store/slack_setup.go` (SaveSlackHostedSetup,
//! GetSlackHostedSetup, SaveSlackRoutePolicy, GetSlackRoutePolicy,
//! SaveSlackSmokeEvidence, LatestSlackSmokeEvidence, SaveSlackEventEvidence,
//! ListSlackEventEvidence). Record types are store-local; the tenant-binding
//! resolution is not ported.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use rusqlite::{Row, params};
use serde::{Deserialize, Serialize};

use crate::SQLiteStore;
use crate::crud::{now_rfc3339, null_string, opt_time_string};

/// Go `SlackHostedSetupRecord` (stored in `slack_hosted_setups`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlackHostedSetupRecord {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub connector_id: String,
    pub connector_kind: String,
    pub display_name: String,
    pub status: String,
    pub terminal_state: String,
    pub oauth_state: String,
    pub route_policy_state: String,
    pub delivery_eligible: bool,
    pub workspace_binding_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
    pub redaction_status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validated_at: Option<DateTime<Utc>>,
    pub retention_expires_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_binding: Option<SlackWorkspaceBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_policy: Option<SlackRoutePolicyRecord>,
}

/// Go `SlackWorkspaceBinding`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlackWorkspaceBinding {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub connector_id: String,
    pub workspace_binding_id: String,
    pub workspace_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workspace_label: String,
    pub installation_id: String,
    pub oauth_grant_state: String,
    pub required_scope_state: String,
    pub validated_at: DateTime<Utc>,
    pub redaction_status: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub safe_evidence: HashMap<String, String>,
}

/// Go `SlackRoutePolicyRecord` (stored in `slack_route_policies`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlackRoutePolicyRecord {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub connector_id: String,
    pub workspace_binding_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected_channels: Vec<SlackConversationRouteRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_dm_users: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_dm_user_groups: Vec<String>,
    pub mention_gate: String,
    pub thread_reply_mode: String,
    pub validation_state: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
    pub validated_at: DateTime<Utc>,
    pub redaction_status: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub safe_evidence: HashMap<String, String>,
}

/// Go `SlackConversationRouteRecord`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlackConversationRouteRecord {
    pub conversation_id: String,
    pub conversation_type: String,
    pub selected_channel_state: String,
    pub validation_state: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub redaction_status: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub safe_evidence: HashMap<String, String>,
}

/// Go `SlackSmokeEvidenceRecord` (stored in `slack_smoke_evidence`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlackSmokeEvidenceRecord {
    pub smoke_evidence_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub connector_id: String,
    pub workspace_binding_id: String,
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

/// Go `SlackEventEvidenceRecord` (stored in `slack_event_evidence`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlackEventEvidenceRecord {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub connector_id: String,
    pub workspace_id: String,
    pub conversation_id: String,
    pub message_id: String,
    pub event_id: String,
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

fn scan_hosted_setup(row: &Row) -> Result<SlackHostedSetupRecord, String> {
    let raw: String = row.get(0).map_err(|e| e.to_string())?;
    decode_document(&raw, "slack hosted setup")
}

fn scan_route_policy(row: &Row) -> Result<SlackRoutePolicyRecord, String> {
    let raw: String = row.get(0).map_err(|e| e.to_string())?;
    decode_document(&raw, "slack route policy")
}

fn scan_smoke(row: &Row) -> Result<SlackSmokeEvidenceRecord, String> {
    let raw: String = row.get(0).map_err(|e| e.to_string())?;
    decode_document(&raw, "slack smoke evidence")
}

fn scan_event_evidence(row: &Row) -> Result<SlackEventEvidenceRecord, String> {
    let raw: String = row.get(0).map_err(|e| e.to_string())?;
    decode_document(&raw, "slack event evidence")
}

/// Go `normalizeSlackHostedSetupRecord`.
fn normalize_hosted_setup(mut record: SlackHostedSetupRecord) -> SlackHostedSetupRecord {
    let now = Utc::now();
    record.connector_kind = coalesce(&record.connector_kind, "slack");
    record.status = coalesce(&record.status, "degraded");
    record.terminal_state = coalesce(&record.terminal_state, "action-required");
    record.oauth_state = coalesce(&record.oauth_state, "grant_missing");
    record.route_policy_state = coalesce(&record.route_policy_state, "none");
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
    record
}

/// Go `normalizeSlackRoutePolicyRecord`.
fn normalize_route_policy(mut record: SlackRoutePolicyRecord) -> SlackRoutePolicyRecord {
    record.mention_gate = coalesce(&record.mention_gate, "agent_mention_required");
    record.thread_reply_mode =
        coalesce(&record.thread_reply_mode, "channel_mentions_thread_rooted");
    record.validation_state = coalesce(&record.validation_state, "blocked");
    record.redaction_status = coalesce(&record.redaction_status, "redacted");
    if is_unset_time(&record.validated_at) {
        record.validated_at = Utc::now();
    }
    record
}

/// Go `normalizeSlackSmokeEvidenceRecord`.
fn normalize_smoke(mut record: SlackSmokeEvidenceRecord) -> SlackSmokeEvidenceRecord {
    let now = Utc::now();
    record.status = coalesce(&record.status, "skipped");
    record.authorization_mode = coalesce(&record.authorization_mode, "unavailable");
    record.owner = coalesce(&record.owner, "operator");
    record.reason = coalesce(&record.reason, "safe_slack_authorization_unavailable");
    record.redaction_status = coalesce(&record.redaction_status, "redacted");
    if record.smoke_evidence_id.trim().is_empty() {
        record.smoke_evidence_id = new_store_id("slack_smoke");
    }
    if is_unset_time(&record.validated_at) {
        record.validated_at = now;
    }
    if is_unset_time(&record.retention_expires_at) {
        record.retention_expires_at = record.validated_at + Duration::days(90);
    }
    record
}

/// Go `normalizeSlackEventEvidenceRecord`.
fn normalize_event_evidence(mut record: SlackEventEvidenceRecord) -> SlackEventEvidenceRecord {
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
    /// Go `SaveSlackHostedSetup`.
    pub fn save_slack_hosted_setup(&self, record: &SlackHostedSetupRecord) -> Result<(), String> {
        let record = normalize_hosted_setup(record.clone());
        let document = serde_json::to_string(&record)
            .map_err(|e| format!("marshal slack hosted setup: {e}"))?;

        self.conn
            .execute(
                r#"INSERT INTO slack_hosted_setups (
                    tenant_id, connector_id, connector_kind, display_name, status, terminal_state,
                    oauth_state, route_policy_state, delivery_eligible, workspace_binding_id,
                    reason_code, redaction_status, created_at, updated_at, validated_at,
                    retention_expires_at, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
                ON CONFLICT(tenant_id, connector_id) DO UPDATE SET
                    connector_kind = excluded.connector_kind,
                    display_name = excluded.display_name,
                    status = excluded.status,
                    terminal_state = excluded.terminal_state,
                    oauth_state = excluded.oauth_state,
                    route_policy_state = excluded.route_policy_state,
                    delivery_eligible = excluded.delivery_eligible,
                    workspace_binding_id = excluded.workspace_binding_id,
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
                    record.oauth_state,
                    record.route_policy_state,
                    i64::from(record.delivery_eligible),
                    record.workspace_binding_id,
                    null_string(&record.reason_code),
                    record.redaction_status,
                    now_rfc3339(&record.created_at),
                    now_rfc3339(&record.updated_at),
                    opt_time_string(&record.validated_at),
                    now_rfc3339(&record.retention_expires_at),
                    document,
                ],
            )
            .map_err(|e| format!("save slack hosted setup {}: {e}", record.connector_id))?;
        Ok(())
    }

    /// Go `GetSlackHostedSetup` — the route policy is re-read from its own table.
    pub fn get_slack_hosted_setup(
        &self,
        tenant_id: &str,
        connector_id: &str,
    ) -> Result<Option<SlackHostedSetupRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT document_json FROM slack_hosted_setups WHERE tenant_id = ?1 AND connector_id = ?2",
            )
            .map_err(|e| format!("get slack hosted setup {connector_id}: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id.trim(), connector_id.trim()])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let mut record = scan_hosted_setup(row)?;
        record.route_policy = self.get_slack_route_policy(tenant_id, connector_id)?;
        Ok(Some(record))
    }

    /// Go `SaveSlackRoutePolicy`.
    pub fn save_slack_route_policy(&self, record: &SlackRoutePolicyRecord) -> Result<(), String> {
        let record = normalize_route_policy(record.clone());
        let document = serde_json::to_string(&record)
            .map_err(|e| format!("marshal slack route policy: {e}"))?;

        self.conn
            .execute(
                r#"INSERT INTO slack_route_policies (
                    tenant_id, connector_id, workspace_binding_id, validation_state, reason_code,
                    validated_at, redaction_status, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                ON CONFLICT(tenant_id, connector_id) DO UPDATE SET
                    workspace_binding_id = excluded.workspace_binding_id,
                    validation_state = excluded.validation_state,
                    reason_code = excluded.reason_code,
                    validated_at = excluded.validated_at,
                    redaction_status = excluded.redaction_status,
                    document_json = excluded.document_json"#,
                params![
                    record.tenant_id,
                    record.connector_id,
                    record.workspace_binding_id,
                    record.validation_state,
                    null_string(&record.reason_code),
                    now_rfc3339(&record.validated_at),
                    record.redaction_status,
                    document,
                ],
            )
            .map_err(|e| format!("save slack route policy {}: {e}", record.connector_id))?;
        Ok(())
    }

    /// Go `GetSlackRoutePolicy`.
    pub fn get_slack_route_policy(
        &self,
        tenant_id: &str,
        connector_id: &str,
    ) -> Result<Option<SlackRoutePolicyRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT document_json FROM slack_route_policies WHERE tenant_id = ?1 AND connector_id = ?2",
            )
            .map_err(|e| format!("get slack route policy {connector_id}: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id.trim(), connector_id.trim()])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_route_policy(row).map(Some)
    }

    /// Go `SaveSlackSmokeEvidence`.
    pub fn save_slack_smoke_evidence(
        &self,
        record: &SlackSmokeEvidenceRecord,
    ) -> Result<(), String> {
        let record = normalize_smoke(record.clone());
        let document = serde_json::to_string(&record)
            .map_err(|e| format!("marshal slack smoke evidence: {e}"))?;

        self.conn
            .execute(
                r#"INSERT INTO slack_smoke_evidence (
                    smoke_evidence_id, tenant_id, connector_id, workspace_binding_id, status,
                    authorization_mode, owner, reason, remaining_risk, validated_at,
                    retention_expires_at, redaction_status, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                ON CONFLICT(smoke_evidence_id) DO UPDATE SET
                    tenant_id = excluded.tenant_id,
                    connector_id = excluded.connector_id,
                    workspace_binding_id = excluded.workspace_binding_id,
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
                    record.workspace_binding_id,
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
                    "save slack smoke evidence {}: {e}",
                    record.smoke_evidence_id
                )
            })?;
        Ok(())
    }

    /// Go `LatestSlackSmokeEvidence`.
    pub fn latest_slack_smoke_evidence(
        &self,
        tenant_id: &str,
        connector_id: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<SlackSmokeEvidenceRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT document_json
                FROM slack_smoke_evidence
                WHERE tenant_id = ?1 AND connector_id = ?2 AND retention_expires_at > ?3
                ORDER BY validated_at DESC, smoke_evidence_id DESC
                LIMIT 1"#,
            )
            .map_err(|e| format!("latest slack smoke evidence {connector_id}: {e}"))?;
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

    /// Go `SaveSlackEventEvidence`.
    pub fn save_slack_event_evidence(
        &self,
        record: &SlackEventEvidenceRecord,
    ) -> Result<(), String> {
        let record = normalize_event_evidence(record.clone());
        let document = serde_json::to_string(&record)
            .map_err(|e| format!("marshal slack event evidence: {e}"))?;

        self.conn
            .execute(
                r#"INSERT INTO slack_event_evidence (
                    tenant_id, connector_id, workspace_id, conversation_id, message_id, event_id,
                    route_outcome, reason_code, received_at, retention_expires_at,
                    redaction_status, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                ON CONFLICT(tenant_id, connector_id, workspace_id, conversation_id, message_id, event_id) DO UPDATE SET
                    route_outcome = excluded.route_outcome,
                    reason_code = excluded.reason_code,
                    received_at = excluded.received_at,
                    retention_expires_at = excluded.retention_expires_at,
                    redaction_status = excluded.redaction_status,
                    document_json = excluded.document_json"#,
                params![
                    record.tenant_id,
                    record.connector_id,
                    record.workspace_id,
                    record.conversation_id,
                    record.message_id,
                    record.event_id,
                    record.route_outcome,
                    null_string(&record.reason_code),
                    now_rfc3339(&record.received_at),
                    now_rfc3339(&record.retention_expires_at),
                    record.redaction_status,
                    document,
                ],
            )
            .map_err(|e| format!("save slack event evidence {}: {e}", record.event_id))?;
        Ok(())
    }

    /// Go `ListSlackEventEvidence` — unexpired event evidence, newest first.
    pub fn list_slack_event_evidence(
        &self,
        tenant_id: &str,
        connector_id: &str,
        now: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<SlackEventEvidenceRecord>, String> {
        let limit = if limit <= 0 || limit > 100 {
            100
        } else {
            limit
        };
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT document_json
                FROM slack_event_evidence
                WHERE tenant_id = ?1 AND connector_id = ?2 AND retention_expires_at > ?3
                ORDER BY received_at DESC, event_id DESC
                LIMIT ?4"#,
            )
            .map_err(|e| format!("list slack event evidence: {e}"))?;
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
