//! SQLite CRUD for schedule records (schedules, schedule targets, dispatch attempts). Ported from
//! `daemon/internal/store/store.go` (UpsertSchedule, GetSchedule, ListSchedules,
//! UpsertScheduleTarget, GetScheduleTarget, UpsertScheduleDispatchAttempt,
//! ListScheduleDispatchAttempts). The tenant column is written as NULL until the tenancy package
//! is ported.

use chrono::{DateTime, Utc};
use rusqlite::{Row, params};

use crate::SQLiteStore;
use crate::crud::{now_rfc3339, null_string, opt_time_string, parse_opt_rfc3339, parse_rfc3339};

/// A schedule ledger row. `document` is the JSON-serialized schedule document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScheduleRecord {
    pub schedule_id: String,
    pub environment_scope: String,
    pub tenant_id: String,
    pub kind: String,
    pub status: String,
    pub target_ref_id: String,
    pub timezone: String,
    pub next_due_at: Option<DateTime<Utc>>,
    pub last_attempt_at: Option<DateTime<Utc>>,
    pub last_outcome: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub paused_at: Option<DateTime<Utc>>,
    pub cancelled_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub document: String,
}

/// A schedule target binding row. `document` is the JSON-serialized target document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScheduleTargetRecord {
    pub target_ref_id: String,
    pub schedule_id: String,
    pub target_kind: String,
    pub revision: i64,
    pub active: bool,
    pub updated_at: DateTime<Utc>,
    pub document: String,
}

/// A schedule dispatch attempt row. `document` is the JSON-serialized attempt document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScheduleDispatchAttemptRecord {
    pub attempt_id: String,
    pub schedule_id: String,
    pub due_at: DateTime<Utc>,
    pub trigger_source: String,
    pub dispatch_status: String,
    pub failure_class: String,
    pub failure_reason: String,
    pub retry_count: i64,
    pub retry_budget: i64,
    pub next_retry_at: Option<DateTime<Utc>>,
    pub resolved_target_revision: i64,
    pub run_id: String,
    pub workflow_id: String,
    pub downstream_status: String,
    pub skipped_reason: String,
    pub missed_count: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub document: String,
}

fn scan_schedule(row: &Row) -> Result<ScheduleRecord, String> {
    let schedule_id: String = row.get(0).map_err(|e| e.to_string())?;
    let environment_scope: String = row.get(1).map_err(|e| e.to_string())?;
    let tenant_id: Option<String> = row.get(2).map_err(|e| e.to_string())?;
    let kind: String = row.get(3).map_err(|e| e.to_string())?;
    let status: String = row.get(4).map_err(|e| e.to_string())?;
    let target_ref_id: String = row.get(5).map_err(|e| e.to_string())?;
    let timezone: Option<String> = row.get(6).map_err(|e| e.to_string())?;
    let next_due_at: Option<String> = row.get(7).map_err(|e| e.to_string())?;
    let last_attempt_at: Option<String> = row.get(8).map_err(|e| e.to_string())?;
    let last_outcome: Option<String> = row.get(9).map_err(|e| e.to_string())?;
    let created_at: String = row.get(10).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(11).map_err(|e| e.to_string())?;
    let paused_at: Option<String> = row.get(12).map_err(|e| e.to_string())?;
    let cancelled_at: Option<String> = row.get(13).map_err(|e| e.to_string())?;
    let completed_at: Option<String> = row.get(14).map_err(|e| e.to_string())?;
    let document: String = row.get(15).map_err(|e| e.to_string())?;

    Ok(ScheduleRecord {
        schedule_id,
        environment_scope,
        tenant_id: tenant_id.unwrap_or_default(),
        kind,
        status,
        target_ref_id,
        timezone: timezone.unwrap_or_default(),
        next_due_at: parse_opt_rfc3339(next_due_at)?,
        last_attempt_at: parse_opt_rfc3339(last_attempt_at)?,
        last_outcome: last_outcome.unwrap_or_default(),
        created_at: parse_rfc3339(&created_at)?,
        updated_at: parse_rfc3339(&updated_at)?,
        paused_at: parse_opt_rfc3339(paused_at)?,
        cancelled_at: parse_opt_rfc3339(cancelled_at)?,
        completed_at: parse_opt_rfc3339(completed_at)?,
        document,
    })
}

fn scan_schedule_target(row: &Row) -> Result<ScheduleTargetRecord, String> {
    let target_ref_id: String = row.get(0).map_err(|e| e.to_string())?;
    let schedule_id: String = row.get(1).map_err(|e| e.to_string())?;
    let target_kind: String = row.get(2).map_err(|e| e.to_string())?;
    let revision: i64 = row.get(3).map_err(|e| e.to_string())?;
    let active: bool = row.get(4).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(5).map_err(|e| e.to_string())?;
    let document: String = row.get(6).map_err(|e| e.to_string())?;

    Ok(ScheduleTargetRecord {
        target_ref_id,
        schedule_id,
        target_kind,
        revision,
        active,
        updated_at: parse_rfc3339(&updated_at)?,
        document,
    })
}

fn scan_schedule_dispatch_attempt(row: &Row) -> Result<ScheduleDispatchAttemptRecord, String> {
    let attempt_id: String = row.get(0).map_err(|e| e.to_string())?;
    let schedule_id: String = row.get(1).map_err(|e| e.to_string())?;
    let due_at: String = row.get(2).map_err(|e| e.to_string())?;
    let trigger_source: String = row.get(3).map_err(|e| e.to_string())?;
    let dispatch_status: String = row.get(4).map_err(|e| e.to_string())?;
    let failure_class: Option<String> = row.get(5).map_err(|e| e.to_string())?;
    let failure_reason: Option<String> = row.get(6).map_err(|e| e.to_string())?;
    let retry_count: i64 = row.get(7).map_err(|e| e.to_string())?;
    let retry_budget: i64 = row.get(8).map_err(|e| e.to_string())?;
    let next_retry_at: Option<String> = row.get(9).map_err(|e| e.to_string())?;
    let resolved_target_revision: i64 = row.get(10).map_err(|e| e.to_string())?;
    let run_id: Option<String> = row.get(11).map_err(|e| e.to_string())?;
    let workflow_id: Option<String> = row.get(12).map_err(|e| e.to_string())?;
    let downstream_status: String = row.get(13).map_err(|e| e.to_string())?;
    let skipped_reason: Option<String> = row.get(14).map_err(|e| e.to_string())?;
    let missed_count: i64 = row.get(15).map_err(|e| e.to_string())?;
    let created_at: String = row.get(16).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(17).map_err(|e| e.to_string())?;
    let document: String = row.get(18).map_err(|e| e.to_string())?;

    Ok(ScheduleDispatchAttemptRecord {
        attempt_id,
        schedule_id,
        due_at: parse_rfc3339(&due_at)?,
        trigger_source,
        dispatch_status,
        failure_class: failure_class.unwrap_or_default(),
        failure_reason: failure_reason.unwrap_or_default(),
        retry_count,
        retry_budget,
        next_retry_at: parse_opt_rfc3339(next_retry_at)?,
        resolved_target_revision,
        run_id: run_id.unwrap_or_default(),
        workflow_id: workflow_id.unwrap_or_default(),
        downstream_status,
        skipped_reason: skipped_reason.unwrap_or_default(),
        missed_count,
        created_at: parse_rfc3339(&created_at)?,
        updated_at: parse_rfc3339(&updated_at)?,
        document,
    })
}

impl SQLiteStore {
    pub fn upsert_schedule(&self, record: &ScheduleRecord) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO schedules (
                    schedule_id, environment_scope, kind, status, target_ref_id, timezone,
                    next_due_at, last_attempt_at, last_outcome, created_at, updated_at,
                    paused_at, cancelled_at, completed_at, document_json, tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                ON CONFLICT(schedule_id) DO UPDATE SET
                    environment_scope = excluded.environment_scope,
                    kind = excluded.kind,
                    status = excluded.status,
                    target_ref_id = excluded.target_ref_id,
                    timezone = excluded.timezone,
                    next_due_at = excluded.next_due_at,
                    last_attempt_at = excluded.last_attempt_at,
                    last_outcome = excluded.last_outcome,
                    created_at = excluded.created_at,
                    updated_at = excluded.updated_at,
                    paused_at = excluded.paused_at,
                    cancelled_at = excluded.cancelled_at,
                    completed_at = excluded.completed_at,
                    document_json = excluded.document_json,
                    tenant_id = COALESCE(schedules.tenant_id, excluded.tenant_id)"#,
                params![
                    record.schedule_id,
                    record.environment_scope,
                    record.kind,
                    record.status,
                    record.target_ref_id,
                    null_string(&record.timezone),
                    opt_time_string(&record.next_due_at),
                    opt_time_string(&record.last_attempt_at),
                    null_string(&record.last_outcome),
                    now_rfc3339(&record.created_at),
                    now_rfc3339(&record.updated_at),
                    opt_time_string(&record.paused_at),
                    opt_time_string(&record.cancelled_at),
                    opt_time_string(&record.completed_at),
                    record.document,
                    None::<String>,
                ],
            )
            .map_err(|e| format!("upsert schedule {}: {e}", record.schedule_id))?;
        Ok(())
    }

    pub fn get_schedule(
        &self,
        environment_scope: &str,
        schedule_id: &str,
    ) -> Result<Option<ScheduleRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT schedule_id, environment_scope, tenant_id, kind, status, target_ref_id,
                    timezone, next_due_at, last_attempt_at, last_outcome, created_at, updated_at,
                    paused_at, cancelled_at, completed_at, document_json
                FROM schedules
                WHERE environment_scope = ?1 AND schedule_id = ?2"#,
            )
            .map_err(|e| format!("get schedule {schedule_id}: {e}"))?;
        let mut rows = stmt
            .query(params![environment_scope, schedule_id])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_schedule(row).map(Some)
    }

    pub fn list_schedules(&self, environment_scope: &str) -> Result<Vec<ScheduleRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT schedule_id, environment_scope, tenant_id, kind, status, target_ref_id,
                    timezone, next_due_at, last_attempt_at, last_outcome, created_at, updated_at,
                    paused_at, cancelled_at, completed_at, document_json
                FROM schedules
                WHERE environment_scope = ?1
                ORDER BY created_at ASC, schedule_id ASC"#,
            )
            .map_err(|e| format!("list schedules: {e}"))?;
        let mut rows = stmt
            .query(params![environment_scope])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_schedule(row)?);
        }
        Ok(items)
    }

    pub fn upsert_schedule_target(&self, record: &ScheduleTargetRecord) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO schedule_targets (
                    target_ref_id, schedule_id, target_kind, revision, active, updated_at,
                    document_json, tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                ON CONFLICT(target_ref_id) DO UPDATE SET
                    schedule_id = excluded.schedule_id,
                    target_kind = excluded.target_kind,
                    revision = excluded.revision,
                    active = excluded.active,
                    updated_at = excluded.updated_at,
                    document_json = excluded.document_json,
                    tenant_id = COALESCE(schedule_targets.tenant_id, excluded.tenant_id)"#,
                params![
                    record.target_ref_id,
                    record.schedule_id,
                    record.target_kind,
                    record.revision,
                    record.active,
                    now_rfc3339(&record.updated_at),
                    record.document,
                    None::<String>,
                ],
            )
            .map_err(|e| format!("upsert schedule target {}: {e}", record.target_ref_id))?;
        Ok(())
    }

    pub fn get_schedule_target(
        &self,
        schedule_id: &str,
        target_ref_id: &str,
    ) -> Result<Option<ScheduleTargetRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT target_ref_id, schedule_id, target_kind, revision, active, updated_at,
                    document_json
                FROM schedule_targets
                WHERE schedule_id = ?1 AND target_ref_id = ?2"#,
            )
            .map_err(|e| format!("get schedule target {target_ref_id}: {e}"))?;
        let mut rows = stmt
            .query(params![schedule_id, target_ref_id])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_schedule_target(row).map(Some)
    }

    pub fn upsert_schedule_dispatch_attempt(
        &self,
        record: &ScheduleDispatchAttemptRecord,
    ) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO schedule_dispatch_attempts (
                    attempt_id, schedule_id, due_at, trigger_source, dispatch_status,
                    failure_class, failure_reason, retry_count, retry_budget, next_retry_at,
                    resolved_target_revision, run_id, workflow_id, downstream_status,
                    skipped_reason, missed_count, created_at, updated_at, document_json,
                    tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)
                ON CONFLICT(attempt_id) DO UPDATE SET
                    schedule_id = excluded.schedule_id,
                    due_at = excluded.due_at,
                    trigger_source = excluded.trigger_source,
                    dispatch_status = excluded.dispatch_status,
                    failure_class = excluded.failure_class,
                    failure_reason = excluded.failure_reason,
                    retry_count = excluded.retry_count,
                    retry_budget = excluded.retry_budget,
                    next_retry_at = excluded.next_retry_at,
                    resolved_target_revision = excluded.resolved_target_revision,
                    run_id = excluded.run_id,
                    workflow_id = excluded.workflow_id,
                    downstream_status = excluded.downstream_status,
                    skipped_reason = excluded.skipped_reason,
                    missed_count = excluded.missed_count,
                    created_at = excluded.created_at,
                    updated_at = excluded.updated_at,
                    document_json = excluded.document_json,
                    tenant_id = COALESCE(schedule_dispatch_attempts.tenant_id, excluded.tenant_id)"#,
                params![
                    record.attempt_id,
                    record.schedule_id,
                    now_rfc3339(&record.due_at),
                    record.trigger_source,
                    record.dispatch_status,
                    null_string(&record.failure_class),
                    null_string(&record.failure_reason),
                    record.retry_count,
                    record.retry_budget,
                    opt_time_string(&record.next_retry_at),
                    record.resolved_target_revision,
                    null_string(&record.run_id),
                    null_string(&record.workflow_id),
                    record.downstream_status,
                    null_string(&record.skipped_reason),
                    record.missed_count,
                    now_rfc3339(&record.created_at),
                    now_rfc3339(&record.updated_at),
                    record.document,
                    None::<String>,
                ],
            )
            .map_err(|e| format!("upsert schedule attempt {}: {e}", record.attempt_id))?;
        Ok(())
    }

    pub fn list_schedule_dispatch_attempts(
        &self,
        schedule_id: &str,
    ) -> Result<Vec<ScheduleDispatchAttemptRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT attempt_id, schedule_id, due_at, trigger_source, dispatch_status,
                    failure_class, failure_reason, retry_count, retry_budget, next_retry_at,
                    resolved_target_revision, run_id, workflow_id, downstream_status,
                    skipped_reason, missed_count, created_at, updated_at, document_json
                FROM schedule_dispatch_attempts
                WHERE schedule_id = ?1
                ORDER BY due_at DESC, created_at DESC, attempt_id DESC"#,
            )
            .map_err(|e| format!("list schedule attempts {schedule_id}: {e}"))?;
        let mut rows = stmt
            .query(params![schedule_id])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_schedule_dispatch_attempt(row)?);
        }
        Ok(items)
    }
}
