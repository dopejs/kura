//! SQLite CRUD for the store-local record types (sandbox executions, consumer-policy records,
//! secret-scope bindings, MCP servers/tools, reminders, delivery, schedules). These records pair
//! explicit query columns with a `document_json` column holding the full serialized domain value.
//! Ported from `daemon/internal/store/store.go`.

use chrono::{DateTime, Utc};
use rusqlite::{Row, params};

use crate::SQLiteStore;
use crate::crud::{now_rfc3339, null_string, opt_time_string, parse_opt_rfc3339, parse_rfc3339};

/// A sandbox execution ledger row. `document` is the JSON-serialized execution document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SandboxExecutionRecord {
    pub execution_id: String,
    pub profile_id: String,
    pub backend_kind: String,
    pub status: String,
    pub approval_id: String,
    pub requested_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub document: String,
}

fn scan_sandbox_execution(row: &Row) -> Result<SandboxExecutionRecord, String> {
    let execution_id: String = row.get(0).map_err(|e| e.to_string())?;
    let profile_id: String = row.get(1).map_err(|e| e.to_string())?;
    let backend_kind: String = row.get(2).map_err(|e| e.to_string())?;
    let status: String = row.get(3).map_err(|e| e.to_string())?;
    let approval_id: Option<String> = row.get(4).map_err(|e| e.to_string())?;
    let requested_at: String = row.get(5).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(6).map_err(|e| e.to_string())?;
    let started_at: Option<String> = row.get(7).map_err(|e| e.to_string())?;
    let completed_at: Option<String> = row.get(8).map_err(|e| e.to_string())?;
    let document: String = row.get(9).map_err(|e| e.to_string())?;

    Ok(SandboxExecutionRecord {
        execution_id,
        profile_id,
        backend_kind,
        status,
        approval_id: approval_id.unwrap_or_default(),
        requested_at: parse_rfc3339(&requested_at)?,
        updated_at: parse_rfc3339(&updated_at)?,
        started_at: parse_opt_rfc3339(started_at)?,
        completed_at: parse_opt_rfc3339(completed_at)?,
        document,
    })
}

impl SQLiteStore {
    pub fn upsert_sandbox_execution(&self, record: &SandboxExecutionRecord) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO sandbox_executions (
                    execution_id, profile_id, backend_kind, status, approval_id, requested_at,
                    updated_at, started_at, completed_at, document_json, tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                ON CONFLICT(execution_id) DO UPDATE SET
                    profile_id = excluded.profile_id,
                    backend_kind = excluded.backend_kind,
                    status = excluded.status,
                    approval_id = excluded.approval_id,
                    requested_at = excluded.requested_at,
                    updated_at = excluded.updated_at,
                    started_at = excluded.started_at,
                    completed_at = excluded.completed_at,
                    document_json = excluded.document_json,
                    tenant_id = COALESCE(sandbox_executions.tenant_id, excluded.tenant_id)"#,
                params![
                    record.execution_id,
                    record.profile_id,
                    record.backend_kind,
                    record.status,
                    null_string(record.approval_id.trim()),
                    now_rfc3339(&record.requested_at),
                    now_rfc3339(&record.updated_at),
                    opt_time_string(&record.started_at),
                    opt_time_string(&record.completed_at),
                    record.document,
                    None::<String>,
                ],
            )
            .map_err(|e| format!("upsert sandbox execution {}: {e}", record.execution_id))?;
        Ok(())
    }

    /// Execution ids owned by `tenant_id`. Pre-backfill rows (NULL tenant)
    /// are excluded, matching the `kura-tenancy` read convention; the by-id
    /// guard still admits them.
    pub fn list_sandbox_execution_ids_for_tenant(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<String>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT execution_id FROM sandbox_executions WHERE tenant_id = ?1 ORDER BY execution_id")
            .map_err(|e| format!("list sandbox execution ids for tenant: {e}"))?;
        let mut rows = stmt.query(params![tenant_id]).map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(row.get::<_, String>(0).map_err(|e| e.to_string())?);
        }
        Ok(items)
    }

    pub fn list_sandbox_executions(&self) -> Result<Vec<SandboxExecutionRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT execution_id, profile_id, backend_kind, status, approval_id, requested_at,
                    updated_at, started_at, completed_at, document_json
                FROM sandbox_executions
                ORDER BY requested_at ASC, execution_id ASC"#,
            )
            .map_err(|e| format!("list sandbox executions: {e}"))?;
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_sandbox_execution(row)?);
        }
        Ok(items)
    }
}
