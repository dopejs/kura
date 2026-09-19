//! SQLite CRUD for delivery records (targets, preferences, outcomes, attempts, summary windows).
//! Ported from `daemon/internal/store/store.go` (UpsertDeliveryTarget, ListDeliveryTargets,
//! GetDeliveryTarget, UpsertDeliveryPreference, ListDeliveryPreferences, GetDeliveryPreference,
//! UpsertDeliveryOutcome, ListDeliveryOutcomes, GetDeliveryOutcome, UpsertDeliveryAttempt,
//! ListDeliveryAttempts, UpsertDeliverySummaryWindow, ListDeliverySummaryWindows,
//! GetDeliverySummaryWindow). The tenant column is written as NULL until the tenancy package is
//! ported.

use chrono::{DateTime, Utc};
use rusqlite::{Row, params, types::Value};

use crate::SQLiteStore;
use crate::crud::{now_rfc3339, null_string, opt_time_string, parse_opt_rfc3339, parse_rfc3339};

/// A delivery target row. `document` is the JSON-serialized target document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeliveryTargetRecord {
    pub target_id: String,
    pub environment_scope: String,
    pub target_kind: String,
    pub status: String,
    pub updated_at: DateTime<Utc>,
    pub document: String,
}

/// A delivery preference row. `document` is the JSON-serialized preference document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeliveryPreferenceRecord {
    pub preference_id: String,
    pub environment_scope: String,
    pub scope_kind: String,
    pub integration_id: String,
    pub active: bool,
    pub updated_at: DateTime<Utc>,
    pub document: String,
}

/// A delivery outcome row. `document` is the JSON-serialized outcome document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeliveryOutcomeRecord {
    pub delivery_id: String,
    pub environment_scope: String,
    pub source_kind: String,
    pub source_id: String,
    pub run_id: String,
    pub workflow_id: String,
    pub schedule_id: String,
    pub integration_id: String,
    pub status: String,
    pub chosen_target_id: String,
    pub preference_id: String,
    pub summary_window_id: String,
    pub updated_at: DateTime<Utc>,
    pub document: String,
}

/// Optional equality filters for listing delivery outcomes (empty values are ignored).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeliveryOutcomeFilter {
    pub source_kind: String,
    pub source_id: String,
    pub run_id: String,
    pub workflow_id: String,
    pub schedule_id: String,
    pub integration_id: String,
    pub status: String,
    pub target_id: String,
}

/// A delivery attempt row. `document` is the JSON-serialized attempt document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeliveryAttemptRecord {
    pub attempt_id: String,
    pub delivery_id: String,
    pub attempt_number: i64,
    pub target_id: String,
    pub status: String,
    pub next_retry_at: Option<DateTime<Utc>>,
    pub document: String,
}

/// A delivery summary window row. `document` is the JSON-serialized window document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeliverySummaryWindowRecord {
    pub summary_window_id: String,
    pub environment_scope: String,
    pub target_id: String,
    pub preference_id: String,
    pub status: String,
    pub window_ends_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub document: String,
}

fn scan_delivery_target(row: &Row) -> Result<DeliveryTargetRecord, String> {
    let target_id: String = row.get(0).map_err(|e| e.to_string())?;
    let environment_scope: String = row.get(1).map_err(|e| e.to_string())?;
    let target_kind: String = row.get(2).map_err(|e| e.to_string())?;
    let status: String = row.get(3).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(4).map_err(|e| e.to_string())?;
    let document: String = row.get(5).map_err(|e| e.to_string())?;

    Ok(DeliveryTargetRecord {
        target_id,
        environment_scope,
        target_kind,
        status,
        updated_at: parse_rfc3339(&updated_at)?,
        document,
    })
}

fn scan_delivery_preference(row: &Row) -> Result<DeliveryPreferenceRecord, String> {
    let preference_id: String = row.get(0).map_err(|e| e.to_string())?;
    let environment_scope: String = row.get(1).map_err(|e| e.to_string())?;
    let scope_kind: String = row.get(2).map_err(|e| e.to_string())?;
    let integration_id: Option<String> = row.get(3).map_err(|e| e.to_string())?;
    let active: bool = row.get(4).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(5).map_err(|e| e.to_string())?;
    let document: String = row.get(6).map_err(|e| e.to_string())?;

    Ok(DeliveryPreferenceRecord {
        preference_id,
        environment_scope,
        scope_kind,
        integration_id: integration_id.unwrap_or_default(),
        active,
        updated_at: parse_rfc3339(&updated_at)?,
        document,
    })
}

fn scan_delivery_outcome(row: &Row) -> Result<DeliveryOutcomeRecord, String> {
    let delivery_id: String = row.get(0).map_err(|e| e.to_string())?;
    let environment_scope: String = row.get(1).map_err(|e| e.to_string())?;
    let source_kind: String = row.get(2).map_err(|e| e.to_string())?;
    let source_id: String = row.get(3).map_err(|e| e.to_string())?;
    let run_id: Option<String> = row.get(4).map_err(|e| e.to_string())?;
    let workflow_id: Option<String> = row.get(5).map_err(|e| e.to_string())?;
    let schedule_id: Option<String> = row.get(6).map_err(|e| e.to_string())?;
    let integration_id: Option<String> = row.get(7).map_err(|e| e.to_string())?;
    let status: String = row.get(8).map_err(|e| e.to_string())?;
    let chosen_target_id: Option<String> = row.get(9).map_err(|e| e.to_string())?;
    let preference_id: Option<String> = row.get(10).map_err(|e| e.to_string())?;
    let summary_window_id: Option<String> = row.get(11).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(12).map_err(|e| e.to_string())?;
    let document: String = row.get(13).map_err(|e| e.to_string())?;

    Ok(DeliveryOutcomeRecord {
        delivery_id,
        environment_scope,
        source_kind,
        source_id,
        run_id: run_id.unwrap_or_default(),
        workflow_id: workflow_id.unwrap_or_default(),
        schedule_id: schedule_id.unwrap_or_default(),
        integration_id: integration_id.unwrap_or_default(),
        status,
        chosen_target_id: chosen_target_id.unwrap_or_default(),
        preference_id: preference_id.unwrap_or_default(),
        summary_window_id: summary_window_id.unwrap_or_default(),
        updated_at: parse_rfc3339(&updated_at)?,
        document,
    })
}

fn scan_delivery_attempt(row: &Row) -> Result<DeliveryAttemptRecord, String> {
    let attempt_id: String = row.get(0).map_err(|e| e.to_string())?;
    let delivery_id: String = row.get(1).map_err(|e| e.to_string())?;
    let attempt_number: i64 = row.get(2).map_err(|e| e.to_string())?;
    let target_id: String = row.get(3).map_err(|e| e.to_string())?;
    let status: String = row.get(4).map_err(|e| e.to_string())?;
    let next_retry_at: Option<String> = row.get(5).map_err(|e| e.to_string())?;
    let document: String = row.get(6).map_err(|e| e.to_string())?;

    Ok(DeliveryAttemptRecord {
        attempt_id,
        delivery_id,
        attempt_number,
        target_id,
        status,
        next_retry_at: parse_opt_rfc3339(next_retry_at)?,
        document,
    })
}

fn scan_delivery_summary_window(row: &Row) -> Result<DeliverySummaryWindowRecord, String> {
    let summary_window_id: String = row.get(0).map_err(|e| e.to_string())?;
    let environment_scope: String = row.get(1).map_err(|e| e.to_string())?;
    let target_id: String = row.get(2).map_err(|e| e.to_string())?;
    let preference_id: String = row.get(3).map_err(|e| e.to_string())?;
    let status: String = row.get(4).map_err(|e| e.to_string())?;
    let window_ends_at: String = row.get(5).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(6).map_err(|e| e.to_string())?;
    let document: String = row.get(7).map_err(|e| e.to_string())?;

    Ok(DeliverySummaryWindowRecord {
        summary_window_id,
        environment_scope,
        target_id,
        preference_id,
        status,
        window_ends_at: parse_rfc3339(&window_ends_at)?,
        updated_at: parse_rfc3339(&updated_at)?,
        document,
    })
}

/// Appends `AND <column> = ?<n>` for non-empty filter values, mirroring the Go filter builder.
fn push_outcome_filter(
    sql: &mut String,
    args: &mut Vec<Value>,
    index: &mut usize,
    column: &str,
    value: &str,
) {
    let trimmed = value.trim();
    if !trimmed.is_empty() {
        *index += 1;
        sql.push_str(&format!(" AND {column} = ?{index}"));
        args.push(Value::from(trimmed.to_string()));
    }
}

impl SQLiteStore {
    pub fn upsert_delivery_target(&self, record: &DeliveryTargetRecord) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO delivery_targets (
                    target_id, environment_scope, target_kind, status, updated_at, document_json,
                    tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                ON CONFLICT(target_id) DO UPDATE SET
                    environment_scope = excluded.environment_scope,
                    target_kind = excluded.target_kind,
                    status = excluded.status,
                    updated_at = excluded.updated_at,
                    document_json = excluded.document_json,
                    tenant_id = COALESCE(delivery_targets.tenant_id, excluded.tenant_id)"#,
                params![
                    record.target_id,
                    record.environment_scope,
                    record.target_kind,
                    record.status,
                    now_rfc3339(&record.updated_at),
                    record.document,
                    None::<String>,
                ],
            )
            .map_err(|e| format!("upsert delivery target {}: {e}", record.target_id))?;
        Ok(())
    }

    pub fn list_delivery_targets(
        &self,
        environment_scope: &str,
    ) -> Result<Vec<DeliveryTargetRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT target_id, environment_scope, target_kind, status, updated_at, document_json
                FROM delivery_targets
                WHERE environment_scope = ?1
                ORDER BY updated_at DESC, target_id DESC"#,
            )
            .map_err(|e| format!("list delivery targets: {e}"))?;
        let mut rows = stmt
            .query(params![environment_scope])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_delivery_target(row)?);
        }
        Ok(items)
    }

    pub fn get_delivery_target(
        &self,
        environment_scope: &str,
        target_id: &str,
    ) -> Result<Option<DeliveryTargetRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT target_id, environment_scope, target_kind, status, updated_at, document_json
                FROM delivery_targets
                WHERE environment_scope = ?1 AND target_id = ?2"#,
            )
            .map_err(|e| format!("get delivery target {target_id}: {e}"))?;
        let mut rows = stmt
            .query(params![environment_scope, target_id])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_delivery_target(row).map(Some)
    }

    pub fn upsert_delivery_preference(
        &self,
        record: &DeliveryPreferenceRecord,
    ) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO delivery_preferences (
                    preference_id, environment_scope, scope_kind, integration_id, active,
                    updated_at, document_json, tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                ON CONFLICT(preference_id) DO UPDATE SET
                    environment_scope = excluded.environment_scope,
                    scope_kind = excluded.scope_kind,
                    integration_id = excluded.integration_id,
                    active = excluded.active,
                    updated_at = excluded.updated_at,
                    document_json = excluded.document_json,
                    tenant_id = COALESCE(delivery_preferences.tenant_id, excluded.tenant_id)"#,
                params![
                    record.preference_id,
                    record.environment_scope,
                    record.scope_kind,
                    null_string(&record.integration_id),
                    record.active,
                    now_rfc3339(&record.updated_at),
                    record.document,
                    None::<String>,
                ],
            )
            .map_err(|e| format!("upsert delivery preference {}: {e}", record.preference_id))?;
        Ok(())
    }

    pub fn list_delivery_preferences(
        &self,
        environment_scope: &str,
    ) -> Result<Vec<DeliveryPreferenceRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT preference_id, environment_scope, scope_kind, integration_id, active,
                    updated_at, document_json
                FROM delivery_preferences
                WHERE environment_scope = ?1
                ORDER BY updated_at DESC, preference_id DESC"#,
            )
            .map_err(|e| format!("list delivery preferences: {e}"))?;
        let mut rows = stmt
            .query(params![environment_scope])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_delivery_preference(row)?);
        }
        Ok(items)
    }

    pub fn get_delivery_preference(
        &self,
        environment_scope: &str,
        preference_id: &str,
    ) -> Result<Option<DeliveryPreferenceRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT preference_id, environment_scope, scope_kind, integration_id, active,
                    updated_at, document_json
                FROM delivery_preferences
                WHERE environment_scope = ?1 AND preference_id = ?2"#,
            )
            .map_err(|e| format!("get delivery preference {preference_id}: {e}"))?;
        let mut rows = stmt
            .query(params![environment_scope, preference_id])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_delivery_preference(row).map(Some)
    }

    pub fn upsert_delivery_outcome(&self, record: &DeliveryOutcomeRecord) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO delivery_outcomes (
                    delivery_id, environment_scope, source_kind, source_id, run_id, workflow_id,
                    schedule_id, integration_id, status, chosen_target_id, preference_id,
                    summary_window_id, updated_at, document_json, tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
                ON CONFLICT(delivery_id) DO UPDATE SET
                    environment_scope = excluded.environment_scope,
                    source_kind = excluded.source_kind,
                    source_id = excluded.source_id,
                    run_id = excluded.run_id,
                    workflow_id = excluded.workflow_id,
                    schedule_id = excluded.schedule_id,
                    integration_id = excluded.integration_id,
                    status = excluded.status,
                    chosen_target_id = excluded.chosen_target_id,
                    preference_id = excluded.preference_id,
                    summary_window_id = excluded.summary_window_id,
                    updated_at = excluded.updated_at,
                    document_json = excluded.document_json,
                    tenant_id = COALESCE(delivery_outcomes.tenant_id, excluded.tenant_id)"#,
                params![
                    record.delivery_id,
                    record.environment_scope,
                    record.source_kind,
                    record.source_id,
                    null_string(&record.run_id),
                    null_string(&record.workflow_id),
                    null_string(&record.schedule_id),
                    null_string(&record.integration_id),
                    record.status,
                    null_string(&record.chosen_target_id),
                    null_string(&record.preference_id),
                    null_string(&record.summary_window_id),
                    now_rfc3339(&record.updated_at),
                    record.document,
                    None::<String>,
                ],
            )
            .map_err(|e| format!("upsert delivery outcome {}: {e}", record.delivery_id))?;
        Ok(())
    }

    pub fn list_delivery_outcomes(
        &self,
        environment_scope: &str,
        filter: &DeliveryOutcomeFilter,
    ) -> Result<Vec<DeliveryOutcomeRecord>, String> {
        let mut sql = String::from(
            r#"SELECT delivery_id, environment_scope, source_kind, source_id, run_id, workflow_id,
                schedule_id, integration_id, status, chosen_target_id, preference_id,
                summary_window_id, updated_at, document_json
            FROM delivery_outcomes
            WHERE environment_scope = ?1"#,
        );
        let mut args: Vec<Value> = vec![Value::from(environment_scope.to_string())];
        let mut index = 1usize;
        push_outcome_filter(
            &mut sql,
            &mut args,
            &mut index,
            "source_kind",
            &filter.source_kind,
        );
        push_outcome_filter(
            &mut sql,
            &mut args,
            &mut index,
            "source_id",
            &filter.source_id,
        );
        push_outcome_filter(&mut sql, &mut args, &mut index, "run_id", &filter.run_id);
        push_outcome_filter(
            &mut sql,
            &mut args,
            &mut index,
            "workflow_id",
            &filter.workflow_id,
        );
        push_outcome_filter(
            &mut sql,
            &mut args,
            &mut index,
            "schedule_id",
            &filter.schedule_id,
        );
        push_outcome_filter(
            &mut sql,
            &mut args,
            &mut index,
            "integration_id",
            &filter.integration_id,
        );
        push_outcome_filter(&mut sql, &mut args, &mut index, "status", &filter.status);
        push_outcome_filter(
            &mut sql,
            &mut args,
            &mut index,
            "chosen_target_id",
            &filter.target_id,
        );
        sql.push_str(" ORDER BY updated_at DESC, delivery_id DESC");

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("list delivery outcomes: {e}"))?;
        let mut rows = stmt
            .query(rusqlite::params_from_iter(args))
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_delivery_outcome(row)?);
        }
        Ok(items)
    }

    pub fn get_delivery_outcome(
        &self,
        environment_scope: &str,
        delivery_id: &str,
    ) -> Result<Option<DeliveryOutcomeRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT delivery_id, environment_scope, source_kind, source_id, run_id, workflow_id,
                    schedule_id, integration_id, status, chosen_target_id, preference_id,
                    summary_window_id, updated_at, document_json
                FROM delivery_outcomes
                WHERE environment_scope = ?1 AND delivery_id = ?2"#,
            )
            .map_err(|e| format!("get delivery outcome {delivery_id}: {e}"))?;
        let mut rows = stmt
            .query(params![environment_scope, delivery_id])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_delivery_outcome(row).map(Some)
    }

    pub fn upsert_delivery_attempt(&self, record: &DeliveryAttemptRecord) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO delivery_attempts (
                    attempt_id, delivery_id, attempt_number, target_id, status, next_retry_at,
                    document_json, tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                ON CONFLICT(attempt_id) DO UPDATE SET
                    delivery_id = excluded.delivery_id,
                    attempt_number = excluded.attempt_number,
                    target_id = excluded.target_id,
                    status = excluded.status,
                    next_retry_at = excluded.next_retry_at,
                    document_json = excluded.document_json,
                    tenant_id = COALESCE(delivery_attempts.tenant_id, excluded.tenant_id)"#,
                params![
                    record.attempt_id,
                    record.delivery_id,
                    record.attempt_number,
                    record.target_id,
                    record.status,
                    opt_time_string(&record.next_retry_at),
                    record.document,
                    None::<String>,
                ],
            )
            .map_err(|e| format!("upsert delivery attempt {}: {e}", record.attempt_id))?;
        Ok(())
    }

    pub fn list_delivery_attempts(
        &self,
        delivery_id: &str,
    ) -> Result<Vec<DeliveryAttemptRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT attempt_id, delivery_id, attempt_number, target_id, status,
                    next_retry_at, document_json
                FROM delivery_attempts
                WHERE delivery_id = ?1
                ORDER BY attempt_number ASC, attempt_id ASC"#,
            )
            .map_err(|e| format!("list delivery attempts: {e}"))?;
        let mut rows = stmt
            .query(params![delivery_id])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_delivery_attempt(row)?);
        }
        Ok(items)
    }

    pub fn upsert_delivery_summary_window(
        &self,
        record: &DeliverySummaryWindowRecord,
    ) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO delivery_summary_windows (
                    summary_window_id, environment_scope, target_id, preference_id, status,
                    window_ends_at, updated_at, document_json, tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                ON CONFLICT(summary_window_id) DO UPDATE SET
                    environment_scope = excluded.environment_scope,
                    target_id = excluded.target_id,
                    preference_id = excluded.preference_id,
                    status = excluded.status,
                    window_ends_at = excluded.window_ends_at,
                    updated_at = excluded.updated_at,
                    document_json = excluded.document_json,
                    tenant_id = COALESCE(delivery_summary_windows.tenant_id, excluded.tenant_id)"#,
                params![
                    record.summary_window_id,
                    record.environment_scope,
                    record.target_id,
                    record.preference_id,
                    record.status,
                    now_rfc3339(&record.window_ends_at),
                    now_rfc3339(&record.updated_at),
                    record.document,
                    None::<String>,
                ],
            )
            .map_err(|e| {
                format!(
                    "upsert delivery summary window {}: {e}",
                    record.summary_window_id
                )
            })?;
        Ok(())
    }

    pub fn list_delivery_summary_windows(
        &self,
        environment_scope: &str,
    ) -> Result<Vec<DeliverySummaryWindowRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT summary_window_id, environment_scope, target_id, preference_id, status,
                    window_ends_at, updated_at, document_json
                FROM delivery_summary_windows
                WHERE environment_scope = ?1
                ORDER BY updated_at DESC, summary_window_id DESC"#,
            )
            .map_err(|e| format!("list delivery summary windows: {e}"))?;
        let mut rows = stmt
            .query(params![environment_scope])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_delivery_summary_window(row)?);
        }
        Ok(items)
    }

    pub fn get_delivery_summary_window(
        &self,
        environment_scope: &str,
        summary_window_id: &str,
    ) -> Result<Option<DeliverySummaryWindowRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT summary_window_id, environment_scope, target_id, preference_id, status,
                    window_ends_at, updated_at, document_json
                FROM delivery_summary_windows
                WHERE environment_scope = ?1 AND summary_window_id = ?2"#,
            )
            .map_err(|e| format!("get delivery summary window {summary_window_id}: {e}"))?;
        let mut rows = stmt
            .query(params![environment_scope, summary_window_id])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_delivery_summary_window(row).map(Some)
    }
}
