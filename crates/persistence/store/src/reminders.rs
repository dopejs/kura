//! SQLite CRUD for reminders, reminder occurrences, and reminder actions. Ported from
//! `daemon/internal/store/store.go` (UpsertReminder, ListReminders, GetReminder,
//! UpsertReminderOccurrence, ListReminderOccurrences, GetReminderOccurrence,
//! AppendReminderAction, ListReminderActions). The tenant column is written as NULL until
//! the tenancy package is ported; `document_json` holds the whole document, matching Go.

use chrono::{DateTime, Utc};
use rusqlite::{Row, params, params_from_iter};

use crate::SQLiteStore;
use crate::crud::{now_rfc3339, null_string, opt_time_string, parse_opt_rfc3339, parse_rfc3339};

/// Mirrors Go's `ReminderOccurrenceFilter`: non-empty trimmed fields are ANDed into the query.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReminderOccurrenceFilter {
    pub reminder_id: String,
    pub state: String,
    pub run_id: String,
    pub workflow_id: String,
    pub delivery_id: String,
    pub scheduled_before: Option<DateTime<Utc>>,
    pub scheduled_after: Option<DateTime<Utc>>,
}

/// A reminder ledger row. `document` is the JSON-serialized reminder document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReminderRecord {
    pub reminder_id: String,
    pub environment_scope: String,
    pub tenant_id: String,
    pub behavior_mode: String,
    pub current_state: String,
    pub next_due_at: Option<DateTime<Utc>>,
    pub active_occurrence_id: String,
    pub updated_at: DateTime<Utc>,
    pub document: String,
}

/// A reminder occurrence ledger row. `document` is the JSON-serialized occurrence document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReminderOccurrenceRecord {
    pub occurrence_id: String,
    pub reminder_id: String,
    pub environment_scope: String,
    pub state: String,
    pub scheduled_for: DateTime<Utc>,
    pub run_id: String,
    pub workflow_id: String,
    pub latest_delivery_id: String,
    pub latest_delivery_status: String,
    pub updated_at: DateTime<Utc>,
    pub document: String,
}

/// A reminder action ledger row. `document` is the JSON-serialized action document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReminderActionRecord {
    pub action_id: String,
    pub reminder_id: String,
    pub occurrence_id: String,
    pub action_kind: String,
    pub new_state: String,
    pub run_id: String,
    pub workflow_id: String,
    pub delivery_id: String,
    pub created_at: DateTime<Utc>,
    pub document: String,
}

fn scan_reminder(row: &Row) -> Result<ReminderRecord, String> {
    let reminder_id: String = row.get(0).map_err(|e| e.to_string())?;
    let environment_scope: String = row.get(1).map_err(|e| e.to_string())?;
    let tenant_id: Option<String> = row.get(2).map_err(|e| e.to_string())?;
    let behavior_mode: String = row.get(3).map_err(|e| e.to_string())?;
    let current_state: String = row.get(4).map_err(|e| e.to_string())?;
    let next_due_at: Option<String> = row.get(5).map_err(|e| e.to_string())?;
    let active_occurrence_id: Option<String> = row.get(6).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(7).map_err(|e| e.to_string())?;
    let document: String = row.get(8).map_err(|e| e.to_string())?;

    Ok(ReminderRecord {
        reminder_id,
        environment_scope,
        tenant_id: tenant_id.unwrap_or_default(),
        behavior_mode,
        current_state,
        next_due_at: parse_opt_rfc3339(next_due_at)?,
        active_occurrence_id: active_occurrence_id.unwrap_or_default(),
        updated_at: parse_rfc3339(&updated_at)?,
        document,
    })
}

fn scan_reminder_occurrence(row: &Row) -> Result<ReminderOccurrenceRecord, String> {
    let occurrence_id: String = row.get(0).map_err(|e| e.to_string())?;
    let reminder_id: String = row.get(1).map_err(|e| e.to_string())?;
    let environment_scope: String = row.get(2).map_err(|e| e.to_string())?;
    let state: String = row.get(3).map_err(|e| e.to_string())?;
    let scheduled_for: String = row.get(4).map_err(|e| e.to_string())?;
    let run_id: Option<String> = row.get(5).map_err(|e| e.to_string())?;
    let workflow_id: Option<String> = row.get(6).map_err(|e| e.to_string())?;
    let latest_delivery_id: Option<String> = row.get(7).map_err(|e| e.to_string())?;
    let latest_delivery_status: Option<String> = row.get(8).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(9).map_err(|e| e.to_string())?;
    let document: String = row.get(10).map_err(|e| e.to_string())?;

    Ok(ReminderOccurrenceRecord {
        occurrence_id,
        reminder_id,
        environment_scope,
        state,
        scheduled_for: parse_rfc3339(&scheduled_for)?,
        run_id: run_id.unwrap_or_default(),
        workflow_id: workflow_id.unwrap_or_default(),
        latest_delivery_id: latest_delivery_id.unwrap_or_default(),
        latest_delivery_status: latest_delivery_status.unwrap_or_default(),
        updated_at: parse_rfc3339(&updated_at)?,
        document,
    })
}

fn scan_reminder_action(row: &Row) -> Result<ReminderActionRecord, String> {
    let action_id: String = row.get(0).map_err(|e| e.to_string())?;
    let reminder_id: String = row.get(1).map_err(|e| e.to_string())?;
    let occurrence_id: Option<String> = row.get(2).map_err(|e| e.to_string())?;
    let action_kind: String = row.get(3).map_err(|e| e.to_string())?;
    let new_state: Option<String> = row.get(4).map_err(|e| e.to_string())?;
    let run_id: Option<String> = row.get(5).map_err(|e| e.to_string())?;
    let workflow_id: Option<String> = row.get(6).map_err(|e| e.to_string())?;
    let delivery_id: Option<String> = row.get(7).map_err(|e| e.to_string())?;
    let created_at: String = row.get(8).map_err(|e| e.to_string())?;
    let document: String = row.get(9).map_err(|e| e.to_string())?;

    Ok(ReminderActionRecord {
        action_id,
        reminder_id,
        occurrence_id: occurrence_id.unwrap_or_default(),
        action_kind,
        new_state: new_state.unwrap_or_default(),
        run_id: run_id.unwrap_or_default(),
        workflow_id: workflow_id.unwrap_or_default(),
        delivery_id: delivery_id.unwrap_or_default(),
        created_at: parse_rfc3339(&created_at)?,
        document,
    })
}

impl SQLiteStore {
    pub fn upsert_reminder(&self, record: &ReminderRecord) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO reminders (
                    reminder_id, environment_scope, behavior_mode, current_state, next_due_at,
                    active_occurrence_id, updated_at, document_json, tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                ON CONFLICT(reminder_id) DO UPDATE SET
                    environment_scope = excluded.environment_scope,
                    behavior_mode = excluded.behavior_mode,
                    current_state = excluded.current_state,
                    next_due_at = excluded.next_due_at,
                    active_occurrence_id = excluded.active_occurrence_id,
                    updated_at = excluded.updated_at,
                    document_json = excluded.document_json,
                    tenant_id = COALESCE(reminders.tenant_id, excluded.tenant_id)"#,
                params![
                    record.reminder_id,
                    record.environment_scope,
                    record.behavior_mode,
                    record.current_state,
                    opt_time_string(&record.next_due_at),
                    null_string(record.active_occurrence_id.trim()),
                    now_rfc3339(&record.updated_at),
                    record.document,
                    None::<String>,
                ],
            )
            .map_err(|e| format!("upsert reminder {}: {e}", record.reminder_id))?;
        Ok(())
    }

    pub fn list_reminders(&self, environment_scope: &str) -> Result<Vec<ReminderRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT reminder_id, environment_scope, tenant_id, behavior_mode, current_state, next_due_at, active_occurrence_id, updated_at, document_json
                FROM reminders
                WHERE environment_scope = ?1
                ORDER BY updated_at DESC, reminder_id DESC"#,
            )
            .map_err(|e| format!("list reminders for {environment_scope}: {e}"))?;
        let mut rows = stmt
            .query(params![environment_scope.trim()])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_reminder(row)?);
        }
        Ok(items)
    }

    pub fn get_reminder(
        &self,
        environment_scope: &str,
        reminder_id: &str,
    ) -> Result<Option<ReminderRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT reminder_id, environment_scope, tenant_id, behavior_mode, current_state, next_due_at, active_occurrence_id, updated_at, document_json
                FROM reminders
                WHERE environment_scope = ?1 AND reminder_id = ?2"#,
            )
            .map_err(|e| format!("get reminder {reminder_id}: {e}"))?;
        let mut rows = stmt
            .query(params![environment_scope.trim(), reminder_id.trim()])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_reminder(row).map(Some)
    }

    pub fn upsert_reminder_occurrence(
        &self,
        record: &ReminderOccurrenceRecord,
    ) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO reminder_occurrences (
                    occurrence_id, reminder_id, environment_scope, state, scheduled_for, run_id,
                    workflow_id, latest_delivery_id, latest_delivery_status, updated_at,
                    document_json, tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                ON CONFLICT(occurrence_id) DO UPDATE SET
                    reminder_id = excluded.reminder_id,
                    environment_scope = excluded.environment_scope,
                    state = excluded.state,
                    scheduled_for = excluded.scheduled_for,
                    run_id = excluded.run_id,
                    workflow_id = excluded.workflow_id,
                    latest_delivery_id = excluded.latest_delivery_id,
                    latest_delivery_status = excluded.latest_delivery_status,
                    updated_at = excluded.updated_at,
                    document_json = excluded.document_json,
                    tenant_id = COALESCE(reminder_occurrences.tenant_id, excluded.tenant_id)"#,
                params![
                    record.occurrence_id,
                    record.reminder_id,
                    record.environment_scope,
                    record.state,
                    now_rfc3339(&record.scheduled_for),
                    null_string(record.run_id.trim()),
                    null_string(record.workflow_id.trim()),
                    null_string(record.latest_delivery_id.trim()),
                    null_string(record.latest_delivery_status.trim()),
                    now_rfc3339(&record.updated_at),
                    record.document,
                    None::<String>,
                ],
            )
            .map_err(|e| format!("upsert reminder occurrence {}: {e}", record.occurrence_id))?;
        Ok(())
    }

    pub fn list_reminder_occurrences(
        &self,
        environment_scope: &str,
        filter: &ReminderOccurrenceFilter,
    ) -> Result<Vec<ReminderOccurrenceRecord>, String> {
        let mut sql = String::from(
            r#"SELECT occurrence_id, reminder_id, environment_scope, state, scheduled_for, run_id, workflow_id, latest_delivery_id, latest_delivery_status, updated_at, document_json
            FROM reminder_occurrences
            WHERE environment_scope = ?"#,
        );
        let mut args: Vec<String> = vec![environment_scope.trim().to_string()];
        if !filter.reminder_id.trim().is_empty() {
            sql.push_str(" AND reminder_id = ?");
            args.push(filter.reminder_id.trim().to_string());
        }
        if !filter.state.trim().is_empty() {
            sql.push_str(" AND state = ?");
            args.push(filter.state.trim().to_string());
        }
        if !filter.run_id.trim().is_empty() {
            sql.push_str(" AND run_id = ?");
            args.push(filter.run_id.trim().to_string());
        }
        if !filter.workflow_id.trim().is_empty() {
            sql.push_str(" AND workflow_id = ?");
            args.push(filter.workflow_id.trim().to_string());
        }
        if !filter.delivery_id.trim().is_empty() {
            sql.push_str(" AND latest_delivery_id = ?");
            args.push(filter.delivery_id.trim().to_string());
        }
        if let Some(before) = &filter.scheduled_before {
            sql.push_str(" AND scheduled_for <= ?");
            args.push(now_rfc3339(before));
        }
        if let Some(after) = &filter.scheduled_after {
            sql.push_str(" AND scheduled_for >= ?");
            args.push(now_rfc3339(after));
        }
        sql.push_str(" ORDER BY scheduled_for DESC, occurrence_id DESC");
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("list reminder occurrences for {environment_scope}: {e}"))?;
        let mut rows = stmt
            .query(params_from_iter(args.iter()))
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_reminder_occurrence(row)?);
        }
        Ok(items)
    }

    pub fn get_reminder_occurrence(
        &self,
        environment_scope: &str,
        occurrence_id: &str,
    ) -> Result<Option<ReminderOccurrenceRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT occurrence_id, reminder_id, environment_scope, state, scheduled_for, run_id, workflow_id, latest_delivery_id, latest_delivery_status, updated_at, document_json
                FROM reminder_occurrences
                WHERE environment_scope = ?1 AND occurrence_id = ?2"#,
            )
            .map_err(|e| format!("get reminder occurrence {occurrence_id}: {e}"))?;
        let mut rows = stmt
            .query(params![environment_scope.trim(), occurrence_id.trim()])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_reminder_occurrence(row).map(Some)
    }

    pub fn append_reminder_action(&self, record: &ReminderActionRecord) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO reminder_actions (
                    action_id, reminder_id, occurrence_id, action_kind, new_state, run_id,
                    workflow_id, delivery_id, created_at, document_json, tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"#,
                params![
                    record.action_id,
                    record.reminder_id,
                    null_string(record.occurrence_id.trim()),
                    record.action_kind,
                    null_string(record.new_state.trim()),
                    null_string(record.run_id.trim()),
                    null_string(record.workflow_id.trim()),
                    null_string(record.delivery_id.trim()),
                    now_rfc3339(&record.created_at),
                    record.document,
                    None::<String>,
                ],
            )
            .map_err(|e| format!("append reminder action {}: {e}", record.action_id))?;
        Ok(())
    }

    pub fn list_reminder_actions(
        &self,
        environment_scope: &str,
        reminder_id: &str,
    ) -> Result<Vec<ReminderActionRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT a.action_id, a.reminder_id, a.occurrence_id, a.action_kind, a.new_state, a.run_id, a.workflow_id, a.delivery_id, a.created_at, a.document_json
                FROM reminder_actions a
                INNER JOIN reminders r ON r.reminder_id = a.reminder_id
                WHERE a.reminder_id = ?1 AND r.environment_scope = ?2
                ORDER BY a.created_at DESC, a.action_id DESC"#,
            )
            .map_err(|e| format!("list reminder actions for {reminder_id}: {e}"))?;
        let mut rows = stmt
            .query(params![reminder_id.trim(), environment_scope.trim()])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_reminder_action(row)?);
        }
        Ok(items)
    }
}
