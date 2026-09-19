//! SQLite CRUD for consumer policy records. Ported from `daemon/internal/store/store.go`
//! (UpsertConsumerPolicyRecord, ListConsumerPolicyRecords). The tenant column is written as
//! NULL until the tenancy package is ported; `document_json` holds the whole document.

use chrono::{DateTime, Utc};
use rusqlite::{Row, params};

use crate::SQLiteStore;
use crate::crud::{now_rfc3339, null_string, opt_time_string, parse_opt_rfc3339, parse_rfc3339};

/// A consumer-policy evaluation ledger row. `document` is the JSON-serialized policy record.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConsumerPolicyRecordRecord {
    pub policy_record_id: String,
    pub consumer_kind: String,
    pub consumer_id: String,
    pub operation_kind: String,
    pub declaration_id: String,
    pub status: String,
    pub decision: String,
    pub approval_status: String,
    pub secret_resolution: String,
    pub requested_by: String,
    pub sandbox_execution_id: String,
    pub tool_call_id: String,
    pub provider_operation_id: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub document: String,
}

fn scan_consumer_policy_record(row: &Row) -> Result<ConsumerPolicyRecordRecord, String> {
    let policy_record_id: String = row.get(0).map_err(|e| e.to_string())?;
    let consumer_kind: String = row.get(1).map_err(|e| e.to_string())?;
    let consumer_id: String = row.get(2).map_err(|e| e.to_string())?;
    let operation_kind: String = row.get(3).map_err(|e| e.to_string())?;
    let declaration_id: Option<String> = row.get(4).map_err(|e| e.to_string())?;
    let status: String = row.get(5).map_err(|e| e.to_string())?;
    let decision: String = row.get(6).map_err(|e| e.to_string())?;
    let approval_status: String = row.get(7).map_err(|e| e.to_string())?;
    let secret_resolution: String = row.get(8).map_err(|e| e.to_string())?;
    let requested_by: Option<String> = row.get(9).map_err(|e| e.to_string())?;
    let sandbox_execution_id: Option<String> = row.get(10).map_err(|e| e.to_string())?;
    let tool_call_id: Option<String> = row.get(11).map_err(|e| e.to_string())?;
    let provider_operation_id: Option<String> = row.get(12).map_err(|e| e.to_string())?;
    let started_at: String = row.get(13).map_err(|e| e.to_string())?;
    let completed_at: Option<String> = row.get(14).map_err(|e| e.to_string())?;
    let document: String = row.get(15).map_err(|e| e.to_string())?;

    Ok(ConsumerPolicyRecordRecord {
        policy_record_id,
        consumer_kind,
        consumer_id,
        operation_kind,
        declaration_id: declaration_id.unwrap_or_default(),
        status,
        decision,
        approval_status,
        secret_resolution,
        requested_by: requested_by.unwrap_or_default(),
        sandbox_execution_id: sandbox_execution_id.unwrap_or_default(),
        tool_call_id: tool_call_id.unwrap_or_default(),
        provider_operation_id: provider_operation_id.unwrap_or_default(),
        started_at: parse_rfc3339(&started_at)?,
        completed_at: parse_opt_rfc3339(completed_at)?,
        document,
    })
}

impl SQLiteStore {
    pub fn upsert_consumer_policy_record(
        &self,
        record: &ConsumerPolicyRecordRecord,
    ) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO consumer_policy_records (
                    policy_record_id, consumer_kind, consumer_id, operation_kind, declaration_id,
                    status, decision, approval_status, secret_resolution, requested_by,
                    sandbox_execution_id, tool_call_id, provider_operation_id, started_at,
                    completed_at, document_json, tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
                ON CONFLICT(policy_record_id) DO UPDATE SET
                    consumer_kind = excluded.consumer_kind,
                    consumer_id = excluded.consumer_id,
                    operation_kind = excluded.operation_kind,
                    declaration_id = excluded.declaration_id,
                    status = excluded.status,
                    decision = excluded.decision,
                    approval_status = excluded.approval_status,
                    secret_resolution = excluded.secret_resolution,
                    requested_by = excluded.requested_by,
                    sandbox_execution_id = excluded.sandbox_execution_id,
                    tool_call_id = excluded.tool_call_id,
                    provider_operation_id = excluded.provider_operation_id,
                    started_at = excluded.started_at,
                    completed_at = excluded.completed_at,
                    document_json = excluded.document_json,
                    tenant_id = COALESCE(consumer_policy_records.tenant_id, excluded.tenant_id)"#,
                params![
                    record.policy_record_id,
                    record.consumer_kind,
                    record.consumer_id,
                    record.operation_kind,
                    null_string(record.declaration_id.trim()),
                    record.status,
                    record.decision,
                    record.approval_status,
                    record.secret_resolution,
                    null_string(record.requested_by.trim()),
                    null_string(record.sandbox_execution_id.trim()),
                    null_string(record.tool_call_id.trim()),
                    null_string(record.provider_operation_id.trim()),
                    now_rfc3339(&record.started_at),
                    opt_time_string(&record.completed_at),
                    record.document,
                    None::<String>,
                ],
            )
            .map_err(|e| format!("upsert consumer policy record {}: {e}", record.policy_record_id))?;
        Ok(())
    }

    pub fn list_consumer_policy_records(&self) -> Result<Vec<ConsumerPolicyRecordRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT policy_record_id, consumer_kind, consumer_id, operation_kind, declaration_id, status, decision, approval_status, secret_resolution, requested_by, sandbox_execution_id, tool_call_id, provider_operation_id, started_at, completed_at, document_json
                FROM consumer_policy_records
                ORDER BY started_at ASC, policy_record_id ASC"#,
            )
            .map_err(|e| format!("list consumer policy records: {e}"))?;
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_consumer_policy_record(row)?);
        }
        Ok(items)
    }
}
