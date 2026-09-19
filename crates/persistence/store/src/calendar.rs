//! SQLite CRUD for calendar accounts, operations, and artifacts. Ported from
//! `daemon/internal/store/store.go` (UpsertCalendarAccount, ListCalendarAccounts,
//! UpsertCalendarOperation, ListCalendarOperations, GetCalendarOperationByID,
//! UpsertCalendarArtifact, ListCalendarArtifacts). The tenant column is written as NULL
//! until the tenancy package is ported; `document_json` holds the whole document, matching Go.

use rusqlite::{Row, params, params_from_iter};

use crate::SQLiteStore;
use crate::crud::{enum_str, now_rfc3339, null_string, parse_rfc3339};

/// Mirrors Go's `CalendarOperationFilter`: non-empty trimmed fields are ANDed into the query.
#[derive(Debug, Clone, Default)]
pub struct CalendarOperationFilter {
    pub integration_id: String,
    pub run_id: String,
    pub workflow_id: String,
    pub schedule_id: String,
    pub delivery_id: String,
    pub operation_class: String,
    pub status: String,
    pub external_event_id: String,
}

fn scan_calendar_account(row: &Row) -> Result<kura_calendar::AccountProjection, String> {
    let updated_at: String = row.get(6).map_err(|e| e.to_string())?;
    parse_rfc3339(&updated_at)?;
    let document_json: String = row.get(7).map_err(|e| e.to_string())?;
    crate::crud::decode_json_field(&document_json)
        .map_err(|e| format!("decode calendar account: {e}"))
}

fn scan_calendar_operation(row: &Row) -> Result<kura_calendar::Operation, String> {
    let updated_at: String = row.get(11).map_err(|e| e.to_string())?;
    parse_rfc3339(&updated_at)?;
    let document_json: String = row.get(12).map_err(|e| e.to_string())?;
    crate::crud::decode_json_field(&document_json)
        .map_err(|e| format!("decode calendar operation: {e}"))
}

fn scan_calendar_artifact(row: &Row) -> Result<kura_calendar::Artifact, String> {
    let created_at: String = row.get(6).map_err(|e| e.to_string())?;
    parse_rfc3339(&created_at)?;
    let document_json: String = row.get(7).map_err(|e| e.to_string())?;
    crate::crud::decode_json_field(&document_json)
        .map_err(|e| format!("decode calendar artifact: {e}"))
}

impl SQLiteStore {
    pub fn upsert_calendar_account(
        &self,
        item: &kura_calendar::AccountProjection,
    ) -> Result<(), String> {
        let document_json = serde_json::to_string(item)
            .map_err(|e| format!("marshal calendar account {}: {e}", item.calendar_account_id))?;

        self.conn
            .execute(
                r#"INSERT INTO calendar_accounts (
                    calendar_account_id,
                    integration_id,
                    environment_scope,
                    account_key,
                    readiness_status,
                    canonical_default,
                    updated_at,
                    document_json,
                    tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                ON CONFLICT(calendar_account_id) DO UPDATE SET
                    integration_id = excluded.integration_id,
                    environment_scope = excluded.environment_scope,
                    account_key = excluded.account_key,
                    readiness_status = excluded.readiness_status,
                    canonical_default = excluded.canonical_default,
                    updated_at = excluded.updated_at,
                    document_json = excluded.document_json,
                    tenant_id = COALESCE(calendar_accounts.tenant_id, excluded.tenant_id)"#,
                params![
                    item.calendar_account_id,
                    item.integration_id,
                    item.environment_scope,
                    null_string(&item.account_key),
                    item.readiness_status,
                    item.canonical_default,
                    now_rfc3339(&item.updated_at),
                    document_json,
                    None::<String>,
                ],
            )
            .map_err(|e| format!("upsert calendar account {}: {e}", item.calendar_account_id))?;
        Ok(())
    }

    pub fn list_calendar_accounts(
        &self,
        environment_scope: &str,
    ) -> Result<Vec<kura_calendar::AccountProjection>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT calendar_account_id, integration_id, environment_scope, account_key, readiness_status, canonical_default, updated_at, document_json
                FROM calendar_accounts
                WHERE environment_scope = ?1
                ORDER BY updated_at ASC, calendar_account_id ASC"#,
            )
            .map_err(|e| format!("list calendar accounts for {environment_scope}: {e}"))?;
        let mut rows = stmt
            .query(params![environment_scope.trim()])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_calendar_account(row)?);
        }
        Ok(items)
    }

    pub fn upsert_calendar_operation(&self, item: &kura_calendar::Operation) -> Result<(), String> {
        let document_json = serde_json::to_string(item)
            .map_err(|e| format!("marshal calendar operation {}: {e}", item.operation_id))?;

        self.conn
            .execute(
                r#"INSERT INTO calendar_operations (
                    operation_id,
                    integration_id,
                    calendar_account_id,
                    environment_scope,
                    operation_class,
                    status,
                    external_event_id,
                    run_id,
                    workflow_id,
                    schedule_id,
                    delivery_id,
                    updated_at,
                    document_json,
                    tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                ON CONFLICT(operation_id) DO UPDATE SET
                    integration_id = excluded.integration_id,
                    calendar_account_id = excluded.calendar_account_id,
                    environment_scope = excluded.environment_scope,
                    operation_class = excluded.operation_class,
                    status = excluded.status,
                    external_event_id = excluded.external_event_id,
                    run_id = excluded.run_id,
                    workflow_id = excluded.workflow_id,
                    schedule_id = excluded.schedule_id,
                    delivery_id = excluded.delivery_id,
                    updated_at = excluded.updated_at,
                    document_json = excluded.document_json,
                    tenant_id = COALESCE(calendar_operations.tenant_id, excluded.tenant_id)"#,
                params![
                    item.operation_id,
                    item.integration_id,
                    item.calendar_account_id,
                    item.environment_scope,
                    enum_str(&item.operation_class),
                    enum_str(&item.status),
                    null_string(&item.external_event_id),
                    null_string(&item.run_id),
                    null_string(&item.workflow_id),
                    null_string(&item.schedule_id),
                    null_string(&item.delivery_id),
                    now_rfc3339(&item.updated_at),
                    document_json,
                    None::<String>,
                ],
            )
            .map_err(|e| format!("upsert calendar operation {}: {e}", item.operation_id))?;
        Ok(())
    }

    pub fn list_calendar_operations(
        &self,
        environment_scope: &str,
        filter: &CalendarOperationFilter,
    ) -> Result<Vec<kura_calendar::Operation>, String> {
        let mut sql = String::from(
            r#"SELECT operation_id, integration_id, calendar_account_id, environment_scope, operation_class, status, external_event_id, run_id, workflow_id, schedule_id, delivery_id, updated_at, document_json
            FROM calendar_operations
            WHERE environment_scope = ?1"#,
        );
        let mut args: Vec<String> = vec![environment_scope.trim().to_string()];
        let mut idx = 2;
        if !filter.integration_id.trim().is_empty() {
            sql.push_str(&format!(" AND integration_id = ?{idx}"));
            args.push(filter.integration_id.trim().to_string());
            idx += 1;
        }
        if !filter.run_id.trim().is_empty() {
            sql.push_str(&format!(" AND run_id = ?{idx}"));
            args.push(filter.run_id.trim().to_string());
            idx += 1;
        }
        if !filter.workflow_id.trim().is_empty() {
            sql.push_str(&format!(" AND workflow_id = ?{idx}"));
            args.push(filter.workflow_id.trim().to_string());
            idx += 1;
        }
        if !filter.schedule_id.trim().is_empty() {
            sql.push_str(&format!(" AND schedule_id = ?{idx}"));
            args.push(filter.schedule_id.trim().to_string());
            idx += 1;
        }
        if !filter.delivery_id.trim().is_empty() {
            sql.push_str(&format!(" AND delivery_id = ?{idx}"));
            args.push(filter.delivery_id.trim().to_string());
            idx += 1;
        }
        if !filter.operation_class.trim().is_empty() {
            sql.push_str(&format!(" AND operation_class = ?{idx}"));
            args.push(filter.operation_class.trim().to_string());
            idx += 1;
        }
        if !filter.status.trim().is_empty() {
            sql.push_str(&format!(" AND status = ?{idx}"));
            args.push(filter.status.trim().to_string());
            idx += 1;
        }
        if !filter.external_event_id.trim().is_empty() {
            sql.push_str(&format!(" AND external_event_id = ?{idx}"));
            args.push(filter.external_event_id.trim().to_string());
        }
        sql.push_str(" ORDER BY updated_at DESC, operation_id DESC");
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("list calendar operations for {environment_scope}: {e}"))?;
        let mut rows = stmt
            .query(params_from_iter(args.iter()))
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_calendar_operation(row)?);
        }
        Ok(items)
    }

    pub fn get_calendar_operation_by_id(
        &self,
        environment_scope: &str,
        operation_id: &str,
    ) -> Result<Option<kura_calendar::Operation>, String> {
        let wanted = operation_id.trim();
        let items =
            self.list_calendar_operations(environment_scope, &CalendarOperationFilter::default())?;
        Ok(items.into_iter().find(|item| item.operation_id == wanted))
    }

    pub fn upsert_calendar_artifact(&self, item: &kura_calendar::Artifact) -> Result<(), String> {
        let document_json = serde_json::to_string(item)
            .map_err(|e| format!("marshal calendar artifact {}: {e}", item.artifact_id))?;

        self.conn
            .execute(
                r#"INSERT INTO calendar_artifacts (
                    artifact_id,
                    operation_id,
                    integration_id,
                    environment_scope,
                    kind,
                    external_event_id,
                    created_at,
                    document_json,
                    tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                ON CONFLICT(artifact_id) DO UPDATE SET
                    operation_id = excluded.operation_id,
                    integration_id = excluded.integration_id,
                    environment_scope = excluded.environment_scope,
                    kind = excluded.kind,
                    external_event_id = excluded.external_event_id,
                    created_at = excluded.created_at,
                    document_json = excluded.document_json,
                    tenant_id = COALESCE(calendar_artifacts.tenant_id, excluded.tenant_id)"#,
                params![
                    item.artifact_id,
                    item.operation_id,
                    item.integration_id,
                    item.environment_scope,
                    enum_str(&item.kind),
                    null_string(&item.external_event_id),
                    now_rfc3339(&item.created_at),
                    document_json,
                    None::<String>,
                ],
            )
            .map_err(|e| format!("upsert calendar artifact {}: {e}", item.artifact_id))?;
        Ok(())
    }

    pub fn list_calendar_artifacts(
        &self,
        environment_scope: &str,
        operation_id: &str,
    ) -> Result<Vec<kura_calendar::Artifact>, String> {
        let mut sql = String::from(
            r#"SELECT artifact_id, operation_id, integration_id, environment_scope, kind, external_event_id, created_at, document_json
            FROM calendar_artifacts
            WHERE environment_scope = ?1"#,
        );
        let mut args: Vec<String> = vec![environment_scope.trim().to_string()];
        if !operation_id.trim().is_empty() {
            args.push(operation_id.trim().to_string());
            sql.push_str(" AND operation_id = ?2");
        }
        sql.push_str(" ORDER BY created_at ASC, artifact_id ASC");
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("list calendar artifacts for {environment_scope}: {e}"))?;
        let mut rows = stmt
            .query(params_from_iter(args.iter()))
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_calendar_artifact(row)?);
        }
        Ok(items)
    }
}
