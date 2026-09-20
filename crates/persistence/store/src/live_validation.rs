//! SQLite CRUD for the live-validation side-effect ledger (migration
//! r40/r41 `live_validation_*` tables): attempts, declared scopes, fresh
//! approvals, ledger entries, kill switches, support-matrix snapshots,
//! ambiguous commits, reconciliation resolutions, comparisons, and retention
//! policies. Follows the evaluation.rs convention: `document_json` holds the
//! whole serialized domain value (the Go daemon reads these tables back via
//! `document_json` alone), and the denormalized columns drive filtering.

use rusqlite::{params, params_from_iter, types::Value};

use crate::SQLiteStore;
use crate::crud::{enum_str, now_rfc3339, null_string, opt_time_string};

fn bool_int(value: bool) -> i64 {
    if value { 1 } else { 0 }
}

impl SQLiteStore {
    pub fn upsert_live_validation_attempt(
        &self,
        item: &kura_livevalidation::Attempt,
    ) -> Result<(), String> {
        let document_json = serde_json::to_string(item)
            .map_err(|e| format!("marshal live validation attempt: {e}"))?;
        self.conn
            .execute(
                r#"INSERT INTO live_validation_attempts (
                    validation_id, tenant_id, candidate_id, source_attempt_id, environment_scope,
                    status, comparison_id, created_at, started_at, completed_at, updated_at,
                    document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                ON CONFLICT(validation_id) DO UPDATE SET
                    tenant_id = excluded.tenant_id,
                    candidate_id = excluded.candidate_id,
                    source_attempt_id = excluded.source_attempt_id,
                    environment_scope = excluded.environment_scope,
                    status = excluded.status,
                    comparison_id = excluded.comparison_id,
                    created_at = excluded.created_at,
                    started_at = excluded.started_at,
                    completed_at = excluded.completed_at,
                    updated_at = excluded.updated_at,
                    document_json = excluded.document_json"#,
                params![
                    item.validation_id,
                    item.tenant_id,
                    item.candidate_id,
                    null_string(&item.source_attempt_id),
                    item.environment_scope,
                    enum_str(&item.status),
                    null_string(&item.comparison_id),
                    now_rfc3339(&item.created_at),
                    opt_time_string(&item.started_at),
                    opt_time_string(&item.completed_at),
                    now_rfc3339(&item.updated_at),
                    document_json,
                ],
            )
            .map_err(|e| format!("upsert live validation attempt {}: {e}", item.validation_id))?;
        Ok(())
    }

    pub fn get_live_validation_attempt(
        &self,
        tenant_id: &str,
        validation_id: &str,
    ) -> Result<Option<kura_livevalidation::Attempt>, String> {
        let mut sql = String::from(
            "SELECT document_json FROM live_validation_attempts WHERE validation_id = ?",
        );
        let mut args: Vec<Value> = vec![Value::Text(validation_id.trim().to_string())];
        if !tenant_id.trim().is_empty() {
            sql.push_str(" AND tenant_id = ?");
            args.push(Value::Text(tenant_id.trim().to_string()));
        }
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("get live validation attempt {validation_id}: {e}"))?;
        let mut rows = stmt
            .query(params_from_iter(&args))
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let raw: String = row.get(0).map_err(|e| e.to_string())?;
        let item = serde_json::from_str(&raw)
            .map_err(|e| format!("decode live validation attempt {validation_id}: {e}"))?;
        Ok(Some(item))
    }

    pub fn list_live_validation_attempts(
        &self,
        filter: &kura_livevalidation::AttemptFilter,
    ) -> Result<Vec<kura_livevalidation::Attempt>, String> {
        let mut sql = String::from(
            r#"SELECT document_json
            FROM live_validation_attempts
            WHERE 1 = 1"#,
        );
        let mut args: Vec<Value> = Vec::new();
        if !filter.tenant_id.trim().is_empty() {
            sql.push_str(" AND tenant_id = ?");
            args.push(Value::Text(filter.tenant_id.trim().to_string()));
        }
        if !filter.environment_scope.trim().is_empty() {
            sql.push_str(" AND environment_scope = ?");
            args.push(Value::Text(filter.environment_scope.trim().to_string()));
        }
        if !filter.candidate_id.trim().is_empty() {
            sql.push_str(" AND candidate_id = ?");
            args.push(Value::Text(filter.candidate_id.trim().to_string()));
        }
        if !filter.status.is_empty() {
            sql.push_str(" AND status = ?");
            args.push(Value::Text(enum_str(&filter.status)));
        }
        sql.push_str(" ORDER BY updated_at DESC, validation_id DESC");
        if filter.limit > 0 {
            sql.push_str(" LIMIT ?");
            args.push(Value::Integer(filter.limit));
        }
        list_documents(&self.conn, &sql, &args, "live validation attempts")
    }

    pub fn upsert_live_validation_scope(
        &self,
        item: &kura_livevalidation::SideEffectScope,
        tenant_id: &str,
    ) -> Result<(), String> {
        let document_json = serde_json::to_string(item)
            .map_err(|e| format!("marshal live validation scope: {e}"))?;
        self.conn
            .execute(
                r#"INSERT INTO live_validation_scopes (
                    scope_id, validation_id, tenant_id, approval_mode, declared_at, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ON CONFLICT(scope_id) DO UPDATE SET
                    validation_id = excluded.validation_id,
                    tenant_id = excluded.tenant_id,
                    approval_mode = excluded.approval_mode,
                    declared_at = excluded.declared_at,
                    document_json = excluded.document_json"#,
                params![
                    item.scope_id,
                    item.validation_id,
                    tenant_id.trim(),
                    enum_str(&item.approval_mode),
                    now_rfc3339(&item.declared_at),
                    document_json,
                ],
            )
            .map_err(|e| format!("upsert live validation scope {}: {e}", item.scope_id))?;
        Ok(())
    }

    pub fn upsert_live_validation_approval(
        &self,
        item: &kura_livevalidation::FreshApproval,
    ) -> Result<(), String> {
        let document_json = serde_json::to_string(item)
            .map_err(|e| format!("marshal live validation approval: {e}"))?;
        self.conn
            .execute(
                r#"INSERT INTO live_validation_approvals (
                    approval_id, validation_id, tenant_id, approval_target, tool_class, action_ref,
                    status, requested_at, resolved_at, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                ON CONFLICT(approval_id) DO UPDATE SET
                    validation_id = excluded.validation_id,
                    tenant_id = excluded.tenant_id,
                    approval_target = excluded.approval_target,
                    tool_class = excluded.tool_class,
                    action_ref = excluded.action_ref,
                    status = excluded.status,
                    requested_at = excluded.requested_at,
                    resolved_at = excluded.resolved_at,
                    document_json = excluded.document_json"#,
                params![
                    item.approval_id,
                    item.validation_id,
                    item.tenant_id,
                    enum_str(&item.approval_target),
                    enum_str(&item.tool_class),
                    null_string(&item.action_ref),
                    enum_str(&item.status),
                    now_rfc3339(&item.requested_at),
                    opt_time_string(&item.resolved_at),
                    document_json,
                ],
            )
            .map_err(|e| format!("upsert live validation approval {}: {e}", item.approval_id))?;
        Ok(())
    }

    pub fn append_live_validation_ledger_entry(
        &self,
        item: &kura_livevalidation::SideEffectLedgerEntry,
    ) -> Result<(), String> {
        let document_json = serde_json::to_string(item)
            .map_err(|e| format!("marshal live validation ledger entry: {e}"))?;
        self.conn
            .execute(
                r#"INSERT INTO live_validation_ledger_entries (
                    ledger_entry_id, validation_id, tenant_id, candidate_id, tool_class, safety_class,
                    action_ref, outcome, attempted_at, completed_at, updated_at, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                ON CONFLICT(ledger_entry_id) DO UPDATE SET
                    validation_id = excluded.validation_id,
                    tenant_id = excluded.tenant_id,
                    candidate_id = excluded.candidate_id,
                    tool_class = excluded.tool_class,
                    safety_class = excluded.safety_class,
                    action_ref = excluded.action_ref,
                    outcome = excluded.outcome,
                    attempted_at = excluded.attempted_at,
                    completed_at = excluded.completed_at,
                    updated_at = excluded.updated_at,
                    document_json = excluded.document_json"#,
                params![
                    item.ledger_entry_id,
                    item.validation_id,
                    item.tenant_id,
                    item.candidate_id,
                    enum_str(&item.tool_class),
                    enum_str(&item.safety_class),
                    item.action_ref,
                    enum_str(&item.outcome),
                    opt_time_string(&item.attempted_at),
                    opt_time_string(&item.completed_at),
                    now_rfc3339(&item.updated_at),
                    document_json,
                ],
            )
            .map_err(|e| format!("append live validation ledger entry {}: {e}", item.ledger_entry_id))?;
        Ok(())
    }

    pub fn update_live_validation_ledger_outcome(
        &self,
        ledger_entry_id: &str,
        outcome: &kura_livevalidation::LedgerOutcome,
        reason_code: &str,
    ) -> Result<(), String> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT document_json FROM live_validation_ledger_entries WHERE ledger_entry_id = ?1",
                params![ledger_entry_id.trim()],
                |row| row.get(0),
            )
            .map_err(|e| format!("read live validation ledger entry {ledger_entry_id}: {e}"))?;
        let Some(raw) = raw else {
            return Ok(());
        };
        let mut item: kura_livevalidation::SideEffectLedgerEntry = serde_json::from_str(&raw)
            .map_err(|e| format!("decode live validation ledger entry {ledger_entry_id}: {e}"))?;
        item.outcome = outcome.clone();
        item.reason_code = reason_code.trim().to_string();
        item.updated_at = chrono::Utc::now();
        if kura_livevalidation::is_terminal_ledger_outcome(&item.outcome)
            && item.completed_at.is_none()
        {
            item.completed_at = Some(item.updated_at);
        }
        let document_json = serde_json::to_string(&item)
            .map_err(|e| format!("marshal live validation ledger entry {ledger_entry_id}: {e}"))?;
        self.conn
            .execute(
                "UPDATE live_validation_ledger_entries SET outcome = ?1, updated_at = ?2, document_json = ?3 WHERE ledger_entry_id = ?4",
                params![enum_str(&item.outcome), now_rfc3339(&item.updated_at), document_json, ledger_entry_id.trim()],
            )
            .map_err(|e| format!("update live validation ledger outcome {ledger_entry_id}: {e}"))?;
        Ok(())
    }

    pub fn list_live_validation_ledger_entries(
        &self,
        filter: &kura_livevalidation::LedgerFilter,
    ) -> Result<Vec<kura_livevalidation::SideEffectLedgerEntry>, String> {
        let mut sql = String::from(
            r#"SELECT document_json
            FROM live_validation_ledger_entries
            WHERE 1 = 1"#,
        );
        let mut args: Vec<Value> = Vec::new();
        if !filter.tenant_id.trim().is_empty() {
            sql.push_str(" AND tenant_id = ?");
            args.push(Value::Text(filter.tenant_id.trim().to_string()));
        }
        if !filter.validation_id.trim().is_empty() {
            sql.push_str(" AND validation_id = ?");
            args.push(Value::Text(filter.validation_id.trim().to_string()));
        }
        if !filter.candidate_id.trim().is_empty() {
            sql.push_str(" AND candidate_id = ?");
            args.push(Value::Text(filter.candidate_id.trim().to_string()));
        }
        if !filter.tool_class.is_empty() {
            sql.push_str(" AND tool_class = ?");
            args.push(Value::Text(enum_str(&filter.tool_class)));
        }
        if !filter.outcome.is_empty() {
            sql.push_str(" AND outcome = ?");
            args.push(Value::Text(enum_str(&filter.outcome)));
        }
        sql.push_str(" ORDER BY updated_at DESC, ledger_entry_id DESC");
        if filter.limit > 0 {
            sql.push_str(" LIMIT ?");
            args.push(Value::Integer(filter.limit));
        }
        list_documents(&self.conn, &sql, &args, "live validation ledger entries")
    }

    pub fn upsert_live_validation_kill_switch(
        &self,
        item: &kura_livevalidation::KillSwitch,
    ) -> Result<(), String> {
        let document_json = serde_json::to_string(item)
            .map_err(|e| format!("marshal live validation kill switch: {e}"))?;
        self.conn
            .execute(
                r#"INSERT INTO live_validation_kill_switches (
                    kill_switch_id, scope, tenant_id, enabled, changed_at, expires_at, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                ON CONFLICT(kill_switch_id) DO UPDATE SET
                    scope = excluded.scope,
                    tenant_id = excluded.tenant_id,
                    enabled = excluded.enabled,
                    changed_at = excluded.changed_at,
                    expires_at = excluded.expires_at,
                    document_json = excluded.document_json"#,
                params![
                    item.kill_switch_id,
                    enum_str(&item.scope),
                    null_string(&item.tenant_id),
                    bool_int(item.enabled),
                    now_rfc3339(&item.changed_at),
                    opt_time_string(&item.expires_at),
                    document_json,
                ],
            )
            .map_err(|e| {
                format!(
                    "upsert live validation kill switch {}: {e}",
                    item.kill_switch_id
                )
            })?;
        Ok(())
    }

    pub fn list_live_validation_kill_switches(
        &self,
        filter: &kura_livevalidation::KillSwitchFilter,
    ) -> Result<Vec<kura_livevalidation::KillSwitch>, String> {
        let mut sql = String::from(
            r#"SELECT document_json
            FROM live_validation_kill_switches
            WHERE 1 = 1"#,
        );
        let mut args: Vec<Value> = Vec::new();
        if !filter.scope.is_empty() {
            sql.push_str(" AND scope = ?");
            args.push(Value::Text(enum_str(&filter.scope)));
        }
        if !filter.tenant_id.trim().is_empty() {
            sql.push_str(" AND tenant_id = ?");
            args.push(Value::Text(filter.tenant_id.trim().to_string()));
        }
        if let Some(enabled) = filter.enabled {
            sql.push_str(" AND enabled = ?");
            args.push(Value::Integer(bool_int(enabled)));
        }
        sql.push_str(" ORDER BY changed_at DESC, kill_switch_id DESC");
        if filter.limit > 0 {
            sql.push_str(" LIMIT ?");
            args.push(Value::Integer(filter.limit));
        }
        list_documents(&self.conn, &sql, &args, "live validation kill switches")
    }

    pub fn upsert_live_validation_support_matrix_snapshot(
        &self,
        tenant_id: &str,
        snapshot_id: &str,
        rows: &[kura_livevalidation::MatrixRow],
    ) -> Result<(), String> {
        let document_json = serde_json::to_string(rows)
            .map_err(|e| format!("marshal live validation matrix snapshot: {e}"))?;
        let version = rows
            .first()
            .map(|row| row.version.clone())
            .unwrap_or_default();
        self.conn
            .execute(
                r#"INSERT INTO live_validation_support_matrix_snapshots (
                    snapshot_id, tenant_id, validation_id, version, created_at, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ON CONFLICT(snapshot_id) DO UPDATE SET
                    tenant_id = excluded.tenant_id,
                    validation_id = excluded.validation_id,
                    version = excluded.version,
                    created_at = excluded.created_at,
                    document_json = excluded.document_json"#,
                params![
                    snapshot_id.trim(),
                    tenant_id.trim(),
                    None::<String>,
                    version,
                    now_rfc3339(&chrono::Utc::now()),
                    document_json,
                ],
            )
            .map_err(|e| format!("upsert live validation matrix snapshot {snapshot_id}: {e}"))?;
        Ok(())
    }

    pub fn save_live_validation_ambiguous_commit(
        &self,
        item: &kura_livevalidation::AmbiguousCommit,
    ) -> Result<(), String> {
        let document_json = serde_json::to_string(item)
            .map_err(|e| format!("marshal live validation ambiguous commit: {e}"))?;
        self.conn
            .execute(
                r#"INSERT INTO live_validation_ambiguous_commits (
                    ambiguous_commit_id, ledger_entry_id, validation_id, tenant_id, cause,
                    automatic_retry_stopped, created_at, updated_at, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                ON CONFLICT(ambiguous_commit_id) DO UPDATE SET
                    ledger_entry_id = excluded.ledger_entry_id,
                    validation_id = excluded.validation_id,
                    tenant_id = excluded.tenant_id,
                    cause = excluded.cause,
                    automatic_retry_stopped = excluded.automatic_retry_stopped,
                    created_at = excluded.created_at,
                    updated_at = excluded.updated_at,
                    document_json = excluded.document_json"#,
                params![
                    item.ambiguous_commit_id,
                    item.ledger_entry_id,
                    item.validation_id,
                    item.tenant_id,
                    enum_str(&item.cause),
                    bool_int(item.automatic_retry_stopped),
                    now_rfc3339(&item.created_at),
                    now_rfc3339(&item.updated_at),
                    document_json,
                ],
            )
            .map_err(|e| {
                format!(
                    "save live validation ambiguous commit {}: {e}",
                    item.ambiguous_commit_id
                )
            })?;
        Ok(())
    }

    pub fn save_live_validation_reconciliation_resolution(
        &self,
        item: &kura_livevalidation::ReconciliationResolution,
    ) -> Result<(), String> {
        let document_json = serde_json::to_string(item)
            .map_err(|e| format!("marshal live validation reconciliation resolution: {e}"))?;
        self.conn
            .execute(
                r#"INSERT INTO live_validation_reconciliation_resolutions (
                    reconciliation_id, ambiguous_commit_id, tenant_id, resolved_by, resolution,
                    resolved_at, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                ON CONFLICT(reconciliation_id) DO UPDATE SET
                    ambiguous_commit_id = excluded.ambiguous_commit_id,
                    tenant_id = excluded.tenant_id,
                    resolved_by = excluded.resolved_by,
                    resolution = excluded.resolution,
                    resolved_at = excluded.resolved_at,
                    document_json = excluded.document_json"#,
                params![
                    item.reconciliation_id,
                    item.ambiguous_commit_id,
                    item.tenant_id,
                    item.resolved_by,
                    enum_str(&item.resolution),
                    now_rfc3339(&item.resolved_at),
                    document_json,
                ],
            )
            .map_err(|e| {
                format!(
                    "save live validation reconciliation {}: {e}",
                    item.reconciliation_id
                )
            })?;
        Ok(())
    }

    pub fn save_live_validation_comparison(
        &self,
        item: &kura_livevalidation::Comparison,
    ) -> Result<(), String> {
        let document_json = serde_json::to_string(item)
            .map_err(|e| format!("marshal live validation comparison: {e}"))?;
        self.conn
            .execute(
                r#"INSERT INTO live_validation_comparisons (
                    comparison_id, validation_id, tenant_id, candidate_id, terminal_status,
                    generated_at, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                ON CONFLICT(comparison_id) DO UPDATE SET
                    validation_id = excluded.validation_id,
                    tenant_id = excluded.tenant_id,
                    candidate_id = excluded.candidate_id,
                    terminal_status = excluded.terminal_status,
                    generated_at = excluded.generated_at,
                    document_json = excluded.document_json"#,
                params![
                    item.comparison_id,
                    item.validation_id,
                    "",
                    item.candidate_id,
                    enum_str(&item.terminal_status),
                    now_rfc3339(&item.generated_at),
                    document_json,
                ],
            )
            .map_err(|e| {
                format!(
                    "save live validation comparison {}: {e}",
                    item.comparison_id
                )
            })?;
        Ok(())
    }

    pub fn list_live_validation_comparisons(
        &self,
        filter: &kura_livevalidation::ComparisonFilter,
    ) -> Result<Vec<kura_livevalidation::Comparison>, String> {
        let mut sql = String::from(
            r#"SELECT document_json
            FROM live_validation_comparisons
            WHERE 1 = 1"#,
        );
        let mut args: Vec<Value> = Vec::new();
        if !filter.tenant_id.trim().is_empty() {
            sql.push_str(" AND tenant_id = ?");
            args.push(Value::Text(filter.tenant_id.trim().to_string()));
        }
        if !filter.validation_id.trim().is_empty() {
            sql.push_str(" AND validation_id = ?");
            args.push(Value::Text(filter.validation_id.trim().to_string()));
        }
        if !filter.candidate_id.trim().is_empty() {
            sql.push_str(" AND candidate_id = ?");
            args.push(Value::Text(filter.candidate_id.trim().to_string()));
        }
        if !filter.terminal_status.is_empty() {
            sql.push_str(" AND terminal_status = ?");
            args.push(Value::Text(enum_str(&filter.terminal_status)));
        }
        sql.push_str(" ORDER BY generated_at DESC, comparison_id DESC");
        if filter.limit > 0 {
            sql.push_str(" LIMIT ?");
            args.push(Value::Integer(filter.limit));
        }
        list_documents(&self.conn, &sql, &args, "live validation comparisons")
    }

    pub fn save_live_validation_retention_policy(
        &self,
        item: &kura_livevalidation::RetentionPolicy,
    ) -> Result<(), String> {
        let document_json = serde_json::to_string(item)
            .map_err(|e| format!("marshal live validation retention policy: {e}"))?;
        self.conn
            .execute(
                r#"INSERT INTO live_validation_retention_policies (
                    policy_id, tenant_id, applies_to, retention_mode, created_by_principal_id,
                    created_at, expires_at, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                ON CONFLICT(policy_id) DO UPDATE SET
                    tenant_id = excluded.tenant_id,
                    applies_to = excluded.applies_to,
                    retention_mode = excluded.retention_mode,
                    created_by_principal_id = excluded.created_by_principal_id,
                    created_at = excluded.created_at,
                    expires_at = excluded.expires_at,
                    document_json = excluded.document_json"#,
                params![
                    item.policy_id,
                    null_string(&item.tenant_id),
                    enum_str(&item.applies_to),
                    enum_str(&item.mode),
                    item.created_by_principal_id,
                    now_rfc3339(&item.created_at),
                    opt_time_string(&item.expires_at),
                    document_json,
                ],
            )
            .map_err(|e| {
                format!(
                    "save live validation retention policy {}: {e}",
                    item.policy_id
                )
            })?;
        Ok(())
    }
}

fn list_documents<T: serde::de::DeserializeOwned>(
    conn: &rusqlite::Connection,
    sql: &str,
    args: &[Value],
    what: &str,
) -> Result<Vec<T>, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| format!("list {what}: {e}"))?;
    let mut rows = stmt
        .query(params_from_iter(args))
        .map_err(|e| e.to_string())?;
    let mut items = Vec::new();
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let raw: String = row.get(0).map_err(|e| e.to_string())?;
        let item = serde_json::from_str(&raw).map_err(|e| format!("decode {what}: {e}"))?;
        items.push(item);
    }
    Ok(items)
}

// --- kura_livevalidation::Store trait impl (async wrapper over the DAOs) ---
//
// rusqlite's Connection is Send but not Sync, so SQLiteStore cannot be the
// trait's `Send + Sync` self type directly. The mutex is wrapped in the
// local `LiveValidationStoreHandle` newtype (same convention as
// SecretStoreHandle / ComputerUseStoreHandle).

/// Send + Sync handle over the SQLite store implementing
/// [`kura_livevalidation::Store`]. Construct from a fresh store and share as
/// `Arc<LiveValidationStoreHandle>` with the live-validation manager.
pub struct LiveValidationStoreHandle(pub parking_lot::Mutex<SQLiteStore>);

impl LiveValidationStoreHandle {
    pub fn new(store: SQLiteStore) -> Self {
        Self(parking_lot::Mutex::new(store))
    }
}

impl kura_livevalidation::Store for LiveValidationStoreHandle {
    fn upsert_attempt(
        &self,
        item: kura_livevalidation::Attempt,
    ) -> kura_livevalidation::BoxFuture<'_, Result<(), kura_livevalidation::LiveValidationError>>
    {
        Box::pin(async move {
            let store = self.0.lock();
            store
                .upsert_live_validation_attempt(&item)
                .map_err(kura_livevalidation::LiveValidationError::Store)
        })
    }

    fn get_attempt(
        &self,
        tenant_id: &str,
        validation_id: &str,
    ) -> kura_livevalidation::BoxFuture<
        '_,
        Result<Option<kura_livevalidation::Attempt>, kura_livevalidation::LiveValidationError>,
    > {
        let tenant_id = tenant_id.to_string();
        let validation_id = validation_id.to_string();
        Box::pin(async move {
            let store = self.0.lock();
            store
                .get_live_validation_attempt(&tenant_id, &validation_id)
                .map_err(kura_livevalidation::LiveValidationError::Store)
        })
    }

    fn list_attempts(
        &self,
        filter: kura_livevalidation::AttemptFilter,
    ) -> kura_livevalidation::BoxFuture<
        '_,
        Result<Vec<kura_livevalidation::Attempt>, kura_livevalidation::LiveValidationError>,
    > {
        Box::pin(async move {
            let store = self.0.lock();
            store
                .list_live_validation_attempts(&filter)
                .map_err(kura_livevalidation::LiveValidationError::Store)
        })
    }

    fn upsert_scope(
        &self,
        item: kura_livevalidation::SideEffectScope,
        tenant_id: &str,
    ) -> kura_livevalidation::BoxFuture<'_, Result<(), kura_livevalidation::LiveValidationError>>
    {
        let tenant_id = tenant_id.to_string();
        Box::pin(async move {
            let store = self.0.lock();
            store
                .upsert_live_validation_scope(&item, &tenant_id)
                .map_err(kura_livevalidation::LiveValidationError::Store)
        })
    }

    fn upsert_approval(
        &self,
        item: kura_livevalidation::FreshApproval,
    ) -> kura_livevalidation::BoxFuture<'_, Result<(), kura_livevalidation::LiveValidationError>>
    {
        Box::pin(async move {
            let store = self.0.lock();
            store
                .upsert_live_validation_approval(&item)
                .map_err(kura_livevalidation::LiveValidationError::Store)
        })
    }

    fn append_ledger_entry(
        &self,
        item: kura_livevalidation::SideEffectLedgerEntry,
    ) -> kura_livevalidation::BoxFuture<'_, Result<(), kura_livevalidation::LiveValidationError>>
    {
        Box::pin(async move {
            let store = self.0.lock();
            store
                .append_live_validation_ledger_entry(&item)
                .map_err(kura_livevalidation::LiveValidationError::Store)
        })
    }

    fn update_ledger_entry_outcome(
        &self,
        ledger_entry_id: &str,
        outcome: &kura_livevalidation::LedgerOutcome,
        reason_code: &str,
    ) -> kura_livevalidation::BoxFuture<'_, Result<(), kura_livevalidation::LiveValidationError>>
    {
        let ledger_entry_id = ledger_entry_id.to_string();
        let outcome = outcome.clone();
        let reason_code = reason_code.to_string();
        Box::pin(async move {
            let store = self.0.lock();
            store
                .update_live_validation_ledger_outcome(&ledger_entry_id, &outcome, &reason_code)
                .map_err(kura_livevalidation::LiveValidationError::Store)
        })
    }

    fn list_ledger_entries(
        &self,
        filter: kura_livevalidation::LedgerFilter,
    ) -> kura_livevalidation::BoxFuture<
        '_,
        Result<
            Vec<kura_livevalidation::SideEffectLedgerEntry>,
            kura_livevalidation::LiveValidationError,
        >,
    > {
        Box::pin(async move {
            let store = self.0.lock();
            store
                .list_live_validation_ledger_entries(&filter)
                .map_err(kura_livevalidation::LiveValidationError::Store)
        })
    }

    fn upsert_kill_switch(
        &self,
        item: kura_livevalidation::KillSwitch,
    ) -> kura_livevalidation::BoxFuture<'_, Result<(), kura_livevalidation::LiveValidationError>>
    {
        Box::pin(async move {
            let store = self.0.lock();
            store
                .upsert_live_validation_kill_switch(&item)
                .map_err(kura_livevalidation::LiveValidationError::Store)
        })
    }

    fn list_kill_switches(
        &self,
        filter: kura_livevalidation::KillSwitchFilter,
    ) -> kura_livevalidation::BoxFuture<
        '_,
        Result<Vec<kura_livevalidation::KillSwitch>, kura_livevalidation::LiveValidationError>,
    > {
        Box::pin(async move {
            let store = self.0.lock();
            store
                .list_live_validation_kill_switches(&filter)
                .map_err(kura_livevalidation::LiveValidationError::Store)
        })
    }

    fn upsert_support_matrix_snapshot(
        &self,
        tenant_id: &str,
        snapshot_id: &str,
        rows: Vec<kura_livevalidation::MatrixRow>,
    ) -> kura_livevalidation::BoxFuture<'_, Result<(), kura_livevalidation::LiveValidationError>>
    {
        let tenant_id = tenant_id.to_string();
        let snapshot_id = snapshot_id.to_string();
        Box::pin(async move {
            let store = self.0.lock();
            store
                .upsert_live_validation_support_matrix_snapshot(&tenant_id, &snapshot_id, &rows)
                .map_err(kura_livevalidation::LiveValidationError::Store)
        })
    }

    fn save_ambiguous_commit(
        &self,
        item: kura_livevalidation::AmbiguousCommit,
    ) -> kura_livevalidation::BoxFuture<'_, Result<(), kura_livevalidation::LiveValidationError>>
    {
        Box::pin(async move {
            let store = self.0.lock();
            store
                .save_live_validation_ambiguous_commit(&item)
                .map_err(kura_livevalidation::LiveValidationError::Store)
        })
    }

    fn save_reconciliation_resolution(
        &self,
        item: kura_livevalidation::ReconciliationResolution,
    ) -> kura_livevalidation::BoxFuture<'_, Result<(), kura_livevalidation::LiveValidationError>>
    {
        Box::pin(async move {
            let store = self.0.lock();
            store
                .save_live_validation_reconciliation_resolution(&item)
                .map_err(kura_livevalidation::LiveValidationError::Store)
        })
    }

    fn save_comparison(
        &self,
        item: kura_livevalidation::Comparison,
    ) -> kura_livevalidation::BoxFuture<'_, Result<(), kura_livevalidation::LiveValidationError>>
    {
        Box::pin(async move {
            let store = self.0.lock();
            store
                .save_live_validation_comparison(&item)
                .map_err(kura_livevalidation::LiveValidationError::Store)
        })
    }

    fn list_comparisons(
        &self,
        filter: kura_livevalidation::ComparisonFilter,
    ) -> kura_livevalidation::BoxFuture<
        '_,
        Result<Vec<kura_livevalidation::Comparison>, kura_livevalidation::LiveValidationError>,
    > {
        Box::pin(async move {
            let store = self.0.lock();
            store
                .list_live_validation_comparisons(&filter)
                .map_err(kura_livevalidation::LiveValidationError::Store)
        })
    }

    fn save_retention_policy(
        &self,
        item: kura_livevalidation::RetentionPolicy,
    ) -> kura_livevalidation::BoxFuture<'_, Result<(), kura_livevalidation::LiveValidationError>>
    {
        Box::pin(async move {
            let store = self.0.lock();
            store
                .save_live_validation_retention_policy(&item)
                .map_err(kura_livevalidation::LiveValidationError::Store)
        })
    }
}
