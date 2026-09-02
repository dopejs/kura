//! Tenant-aware low-level primitives for the runtime spine and per-domain tables.
//!
//! Ported from the Go store package tenancy files:
//! - daemon/internal/store/runtime_tenancy.go (lookup / bind / delete / RAW reads)
//! - daemon/internal/store/runtime_tenant_safe.go (atomic tenant-aware upserts + events)
//! - daemon/internal/store/default_tenant.go (default-personal-tenant resolver)
//! - daemon/internal/store/events_tenancy.go, approvals_tenancy.go,
//!   schedules_tenancy.go, workflows_tenancy.go (per-domain RAW reads)
//!
//! Roadmap 35 (T028 Pass A) semantics:
//! - Reads return only rows whose tenant_id equals the caller's tenant. Rows still
//!   NULL pre-backfill are NOT returned (fail-closed).
//! - The *ForTenantSafe upserts bind tenant_id in the same INSERT and gate the
//!   ON-CONFLICT branch with "WHERE table.tenant_id IS NULL OR table.tenant_id =
//!   excluded.tenant_id". When the WHERE is false, zero rows are touched and the
//!   helper returns the ERR_CROSS_TENANT_ROW sentinel without mutating state.
//! - By-id lookups whose target row exists in another tenant return
//!   ERR_CROSS_TENANT_ROW; the tenancy crate maps that to not-found plus an audit
//!   denial so the row's existence is never leaked (FR-006).
//!
//! context.Context is not threaded through: everything is synchronous and the
//! acting tenant is passed explicitly as the tenant_id argument (callers resolve
//! it via the tenancy layer's require).

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};

use chrono::{DateTime, Utc};
use rusqlite::{params, params_from_iter, types::Value, Row};

use crate::crud::{
    decode_map, decode_opt_json, decode_vec, enum_str, marshal_json, marshal_map, marshal_vec,
    now_rfc3339, null_string, opt_time_string, parse_enum, parse_opt_rfc3339, parse_rfc3339,
};
use crate::SQLiteStore;

// ---------------------------------------------------------------------------
// Sentinels
// ---------------------------------------------------------------------------

impl SQLiteStore {
    /// Sentinel error string returned by the tenant-aware primitives when a row exists
    /// but its tenant_id does not match the caller's tenant. The tenancy layer maps
    /// this to a 404 + audit emission so the resource's existence is not leaked across
    /// tenants (FR-006).
    pub const ERR_CROSS_TENANT_ROW: &'static str = "row belongs to a different tenant";

    /// Sentinel error string returned by resolve_default_personal_tenant_id when no
    /// personal tenant has been bootstrapped yet (pre-bootstrap boot path).
    pub const ERR_DEFAULT_PERSONAL_TENANT_UNAVAILABLE: &'static str =
        "store: default personal tenant not bootstrapped yet";

    /// Reports whether err is the cross-tenant sentinel (the only error value the
    /// tenant-aware primitives produce for a cross-tenant condition).
    #[must_use]
    pub fn is_cross_tenant_row(err: &str) -> bool {
        err == Self::ERR_CROSS_TENANT_ROW
    }

    /// Reports whether err is the default-personal-tenant-unavailable sentinel.
    #[must_use]
    pub fn is_default_personal_tenant_unavailable(err: &str) -> bool {
        err == Self::ERR_DEFAULT_PERSONAL_TENANT_UNAVAILABLE
    }
}

// ---------------------------------------------------------------------------
// Row-level tenant primitives (Go: runtime_tenancy.go)
// ---------------------------------------------------------------------------

impl SQLiteStore {
    /// Returns the tenant_id of a single row keyed by primary-key column:
    /// Ok(Some(id)) when the row exists, Ok(None) when it does not.
    ///
    /// table and pk_column are not bind-parameter eligible in SQLite, so callers MUST
    /// pass trusted compile-time constants. The tenancy layer owns the small set of
    /// allowed (table, pk_column) pairs.
    pub fn lookup_row_tenant(&self, table: &str, pk_column: &str, pk: &str) -> Result<Option<String>, String> {
        let sql = format!("SELECT tenant_id FROM {table} WHERE {pk_column} = ?1");
        match self.conn.query_row(&sql, params![pk], |row| row.get(0)) {
            Ok(tenant) => Ok(tenant),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(format!("lookup tenant for {table}.{pk_column}={pk}: {e}")),
        }
    }

    /// Sets tenant_id on a single row keyed by primary key, refusing to overwrite a
    /// previously-bound non-matching tenant. Returns ERR_CROSS_TENANT_ROW when the
    /// existing tenant_id is non-NULL and differs from tenant_id. Idempotent for the
    /// matching-tenant case and a no-op when the row is absent.
    pub fn bind_row_tenant(
        &self,
        table: &str,
        pk_column: &str,
        pk: &str,
        tenant_id: &str,
    ) -> Result<(), String> {
        if tenant_id.is_empty() {
            return Err("BindRowTenant: empty tenantID".to_string());
        }
        let sql = format!("UPDATE {table} SET tenant_id = ?1 WHERE {pk_column} = ?2 AND (tenant_id IS NULL OR tenant_id = ?1)");
        let affected = self
            .conn
            .execute(&sql, params![tenant_id, pk])
            .map_err(|e| format!("bind tenant for {table}.{pk_column}={pk}: {e}"))?;
        if affected == 0 {
            // Either the row does not exist (nothing to bind) or the row is owned by
            // another tenant. Disambiguate so callers can audit.
            if let Some(existing) = self.lookup_row_tenant(table, pk_column, pk)? {
                if !existing.is_empty() && existing != tenant_id {
                    return Err(Self::ERR_CROSS_TENANT_ROW.to_string());
                }
            }
        }
        Ok(())
    }

    /// Removes a row only if its tenant_id matches the caller's tenant. Returns
    /// Ok(true) on a successful delete, Ok(false) when no row exists, and
    /// ERR_CROSS_TENANT_ROW when the row is owned by a different tenant.
    pub fn delete_row_for_tenant(
        &self,
        table: &str,
        pk_column: &str,
        pk: &str,
        tenant_id: &str,
    ) -> Result<bool, String> {
        if tenant_id.is_empty() {
            return Err("DeleteRowForTenant: empty tenantID".to_string());
        }
        let existing = self.lookup_row_tenant(table, pk_column, pk)?;
        match existing {
            None => return Ok(false),
            Some(owner) if !owner.is_empty() && owner != tenant_id => {
                return Err(Self::ERR_CROSS_TENANT_ROW.to_string());
            }
            _ => {}
        }
        let sql = format!("DELETE FROM {table} WHERE {pk_column} = ?1 AND (tenant_id IS NULL OR tenant_id = ?2)");
        let affected = self
            .conn
            .execute(&sql, params![pk, tenant_id])
            .map_err(|e| format!("delete {table}.{pk_column}={pk} for tenant: {e}"))?;
        Ok(affected > 0)
    }

    /// Scans the runs table without a tenant filter. Test-only: production callers MUST
    /// use the tenancy layer's list_runs_for_tenant so cross-tenant rows do not leak
    /// out of the read path.
    pub fn list_runs_all_tenants_for_test(&self) -> Result<Vec<kura_runtime::Run>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT run_id, session_id, schedule_id, schedule_attempt_id, reminder_id,
                    reminder_occurrence_id, entrypoint, status, goal, created_at, updated_at
                FROM runs
                ORDER BY created_at ASC, run_id ASC"#,
            )
            .map_err(|e| format!("ListRunsAllTenantsForTest: {e}"))?;
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_run(row)?);
        }
        Ok(items)
    }

    /// Reports whether a run with the given id exists AND belongs to tenant_id.
    /// Returns Ok(true) when the row is present and owned by the caller's tenant;
    /// Ok(false) when no such row exists OR the row is owned by a different tenant.
    /// The two cases are indistinguishable by design (FR-006).
    pub fn run_exists_for_tenant(&self, run_id: &str, tenant_id: &str) -> Result<bool, String> {
        let n: i64 = match self.conn.query_row(
            "SELECT 1 FROM runs WHERE run_id = ?1 AND tenant_id = ?2 LIMIT 1",
            params![run_id, tenant_id],
            |row| row.get(0),
        ) {
            Ok(n) => n,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(false),
            Err(e) => return Err(format!("run exists for tenant {run_id}/{tenant_id}: {e}")),
        };
        Ok(n == 1)
    }

    /// Returns all runs whose tenant_id matches tenant_id in ascending creation order.
    /// Pass A semantics: pre-backfill rows whose tenant_id is still NULL are NOT
    /// returned (fail-closed).
    pub fn list_runs_for_tenant_raw(&self, tenant_id: &str) -> Result<Vec<kura_runtime::Run>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT run_id, session_id, schedule_id, schedule_attempt_id, reminder_id,
                    reminder_occurrence_id, entrypoint, status, goal, created_at, updated_at
                FROM runs
                WHERE tenant_id = ?1
                ORDER BY created_at ASC, run_id ASC"#,
            )
            .map_err(|e| format!("list runs for tenant: {e}"))?;
        let mut rows = stmt.query(params![tenant_id]).map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_run(row)?);
        }
        Ok(items)
    }

    /// Returns a single run by id IF its tenant_id matches tenant_id.
    /// Ok(Some(run)) on match, Ok(None) on a missing row, and ERR_CROSS_TENANT_ROW on
    /// mismatch. A NULL-tenant row (pre-backfill) is returned as-is, mirroring the Go
    /// helper.
    pub fn get_run_for_tenant_raw(
        &self,
        run_id: &str,
        tenant_id: &str,
    ) -> Result<Option<kura_runtime::Run>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT run_id, session_id, schedule_id, schedule_attempt_id, reminder_id,
                    reminder_occurrence_id, entrypoint, status, goal, created_at, updated_at,
                    tenant_id
                FROM runs
                WHERE run_id = ?1"#,
            )
            .map_err(|e| format!("get run for tenant: {e}"))?;
        let mut rows = stmt.query(params![run_id]).map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let row_tenant: Option<String> = row.get(11).map_err(|e| e.to_string())?;
        if let Some(owner) = row_tenant {
            if !owner.is_empty() && owner != tenant_id {
                return Err(Self::ERR_CROSS_TENANT_ROW.to_string());
            }
        }
        scan_run_tenant(row).map(Some)
    }

    /// Mirrors list_sessions but filtered by tenant. NULL-tenant rows are excluded.
    pub fn list_sessions_for_tenant_raw(&self, tenant_id: &str) -> Result<Vec<kura_router::Session>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT session_id, kind, status, channel, account_id, peer_id, thread_id,
                    routing_key, generation, created_at, updated_at, last_active_at, last_reset_at
                FROM sessions
                WHERE tenant_id = ?1
                ORDER BY created_at ASC, session_id ASC"#,
            )
            .map_err(|e| format!("list sessions for tenant: {e}"))?;
        let mut rows = stmt.query(params![tenant_id]).map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_session(row)?);
        }
        Ok(items)
    }

    /// Mirrors list_steps but filtered by tenant (and run). NULL-tenant rows excluded.
    pub fn list_steps_for_tenant_raw(&self, tenant_id: &str, run_id: &str) -> Result<Vec<kura_runtime::Step>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT step_id, run_id, workflow_id, workflow_step_id, attempt, title, kind,
                    status, input_json, output_json, created_at, updated_at
                FROM steps
                WHERE tenant_id = ?1 AND run_id = ?2
                ORDER BY created_at ASC, step_id ASC"#,
            )
            .map_err(|e| format!("list steps for tenant: {e}"))?;
        let mut rows = stmt.query(params![tenant_id, run_id]).map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_step(row)?);
        }
        Ok(items)
    }

    /// Mirrors list_tool_calls but filtered by tenant (and run + step). NULL-tenant rows
    /// excluded.
    pub fn list_tool_calls_for_tenant_raw(
        &self,
        tenant_id: &str,
        run_id: &str,
        step_id: &str,
    ) -> Result<Vec<kura_runtime::ToolCall>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT tool_call_id, run_id, step_id, workflow_id, workflow_step_id, attempt,
                    computer_use_session_id, computer_use_action_id, invocation_kind, capability_id,
                    skill_id, mcp_server_id, mcp_server_name, mcp_tool_name, mcp_transport_kind,
                    mcp_session_id, authorization_result, tool_name, status, sandbox_execution_id,
                    failure_class, input_json, output_json, sandbox_json,
                    integration_bindings_json, error_text, created_at, updated_at
                FROM tool_calls
                WHERE tenant_id = ?1 AND run_id = ?2 AND step_id = ?3
                ORDER BY created_at ASC, tool_call_id ASC"#,
            )
            .map_err(|e| format!("list tool_calls for tenant: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id, run_id, step_id])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_tool_call(row)?);
        }
        Ok(items)
    }

    /// Mirrors list_llm_dispatches but filtered by tenant (newest first).
    pub fn list_llm_dispatches_for_tenant_raw(&self, tenant_id: &str) -> Result<Vec<kura_llm::Dispatch>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT dispatch_id, provider, model, messages_json, stream, status, output_text,
                    finish_reason, usage_json, error_code, error_text, timeout_ms, max_retries,
                    attempt_count, created_at, updated_at, started_at, completed_at, tools_json, tool_calls_json
                FROM llm_dispatches
                WHERE tenant_id = ?1
                ORDER BY created_at DESC, dispatch_id DESC"#,
            )
            .map_err(|e| format!("list llm_dispatches for tenant: {e}"))?;
        let mut rows = stmt.query(params![tenant_id]).map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_llm_dispatch(row)?);
        }
        Ok(items)
    }

    /// Mirrors get_llm_dispatch but enforces tenant ownership. Returns
    /// ERR_CROSS_TENANT_ROW when the dispatch exists in a different tenant.
    pub fn get_llm_dispatch_for_tenant_raw(
        &self,
        dispatch_id: &str,
        tenant_id: &str,
    ) -> Result<Option<kura_llm::Dispatch>, String> {
        match self.lookup_row_tenant("llm_dispatches", "dispatch_id", dispatch_id)? {
            Some(owner) if !owner.is_empty() && owner != tenant_id => {
                return Err(Self::ERR_CROSS_TENANT_ROW.to_string());
            }
            None => return Ok(None),
            _ => {}
        }
        self.get_llm_dispatch(dispatch_id)
    }

    /// Sets tenant_id on the row keyed by the composite PK
    /// (server_id, tool_name, runtime_surface). Mirrors bind_row_tenant for the only
    /// single-row helper that does not have a scalar primary key.
    pub fn bind_mcp_tool_exposure_rule_tenant(
        &self,
        server_id: &str,
        tool_name: &str,
        runtime_surface: &str,
        tenant_id: &str,
    ) -> Result<(), String> {
        if tenant_id.is_empty() {
            return Err("BindMCPToolExposureRuleTenant: empty tenantID".to_string());
        }
        let affected = self
            .conn
            .execute(
                r#"UPDATE mcp_tool_exposure_rules
                SET tenant_id = ?1
                WHERE server_id = ?2 AND tool_name = ?3 AND runtime_surface = ?4
                  AND (tenant_id IS NULL OR tenant_id = ?1)"#,
                params![tenant_id, server_id, tool_name, runtime_surface],
            )
            .map_err(|e| format!("bind tenant for mcp_tool_exposure_rules: {e}"))?;
        if affected == 0 {
            // Determine whether the row exists with a different tenant id (cross-tenant
            // write attempt) versus simply being absent.
            match self.conn.query_row(
                r#"SELECT tenant_id FROM mcp_tool_exposure_rules
                WHERE server_id = ?1 AND tool_name = ?2 AND runtime_surface = ?3"#,
                params![server_id, tool_name, runtime_surface],
                |row| row.get::<_, Option<String>>(0),
            ) {
                Ok(Some(existing)) if !existing.is_empty() && existing != tenant_id => {
                    return Err(Self::ERR_CROSS_TENANT_ROW.to_string());
                }
                Ok(_) => {}
                Err(rusqlite::Error::QueryReturnedNoRows) => {}
                Err(e) => {
                    return Err(format!("lookup tenant for mcp_tool_exposure_rules: {e}"));
                }
            }
        }
        Ok(())
    }

    /// Sets tenant_id on every checkpoint row whose run_id matches run_id and whose
    /// tenant_id is currently NULL or already equal to tenant_id. Used by the
    /// save_checkpoint_for_tenant_safe path because checkpoint_id is generated
    /// server-side and not exposed.
    pub fn bind_run_checkpoints_tenant(&self, run_id: &str, tenant_id: &str) -> Result<(), String> {
        if tenant_id.is_empty() {
            return Err("BindRunCheckpointsTenant: empty tenantID".to_string());
        }
        self.conn
            .execute(
                r#"UPDATE checkpoints
                SET tenant_id = ?1
                WHERE run_id = ?2 AND (tenant_id IS NULL OR tenant_id = ?1)"#,
                params![tenant_id, run_id],
            )
            .map_err(|e| format!("bind run checkpoints tenant for run {run_id}: {e}"))?;
        Ok(())
    }

    /// Mirrors list_latest_checkpoints but returns only checkpoints whose tenant_id
    /// matches the caller. Pass A: pre-backfill rows (tenant_id IS NULL) are excluded,
    /// matching the fail-closed semantics of the other tenant-aware reads.
    pub fn list_latest_checkpoints_for_tenant_raw(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<kura_runtime::RunCheckpoint>, String> {
        let all = self.list_latest_checkpoints()?;
        if all.is_empty() {
            return Ok(all);
        }
        // Pull every {run_id, captured_at} that already has a tenant binding in this
        // tenant; filter the latest-checkpoint projection against that set.
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT run_id, captured_at
                FROM checkpoints
                WHERE tenant_id = ?1"#,
            )
            .map_err(|e| format!("list checkpoints for tenant: {e}"))?;
        let mut rows = stmt.query(params![tenant_id]).map_err(|e| e.to_string())?;
        let mut owned: HashSet<(String, String)> = HashSet::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let run_id: String = row.get(0).map_err(|e| e.to_string())?;
            let captured_at: String = row.get(1).map_err(|e| e.to_string())?;
            owned.insert((run_id, captured_at));
        }
        let mut items = Vec::with_capacity(all.len());
        for cp in all {
            let key = (cp.run.run_id.clone(), now_rfc3339(&cp.captured_at));
            if owned.contains(&key) {
                items.push(cp);
            }
        }
        Ok(items)
    }
}

// ---------------------------------------------------------------------------
// Atomic tenant-aware upserts (Go: runtime_tenant_safe.go)
// ---------------------------------------------------------------------------

impl SQLiteStore {
    /// Persists a run row binding tenant_id in the same INSERT. Returns
    /// ERR_CROSS_TENANT_ROW if the row exists and is owned by a different tenant —
    /// the existing row is preserved.
    pub fn upsert_run_for_tenant_safe(&self, run: &kura_runtime::Run, tenant_id: &str) -> Result<(), String> {
        if tenant_id.is_empty() {
            return Err("UpsertRunForTenantSafe: empty tenantID".to_string());
        }
        let affected = self
            .conn
            .execute(
                r#"INSERT INTO runs (
                    run_id, session_id, schedule_id, schedule_attempt_id, reminder_id,
                    reminder_occurrence_id, entrypoint, status, goal, created_at, updated_at,
                    tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                ON CONFLICT(run_id) DO UPDATE SET
                    session_id = excluded.session_id,
                    schedule_id = excluded.schedule_id,
                    schedule_attempt_id = excluded.schedule_attempt_id,
                    reminder_id = excluded.reminder_id,
                    reminder_occurrence_id = excluded.reminder_occurrence_id,
                    entrypoint = excluded.entrypoint,
                    status = excluded.status,
                    goal = excluded.goal,
                    created_at = excluded.created_at,
                    updated_at = excluded.updated_at,
                    tenant_id = excluded.tenant_id
                WHERE runs.tenant_id IS NULL OR runs.tenant_id = excluded.tenant_id"#,
                params![
                    run.run_id,
                    null_string(&run.session_id),
                    null_string(&run.schedule_id),
                    null_string(&run.schedule_attempt_id),
                    null_string(&run.reminder_id),
                    null_string(&run.reminder_occurrence_id),
                    run.entrypoint,
                    run.status.as_str(),
                    run.goal,
                    now_rfc3339(&run.created_at),
                    now_rfc3339(&run.updated_at),
                    tenant_id,
                ],
            )
            .map_err(|e| format!("upsert run for tenant: {e}"))?;
        if affected == 0 {
            // Either the row exists in another tenant (ON CONFLICT WHERE blocked the
            // update), or the engine made no change. Distinguish with a follow-up lookup
            // so we never silently drop a write.
            if let Some(owner) = self.lookup_row_tenant("runs", "run_id", &run.run_id)? {
                if !owner.is_empty() && owner != tenant_id {
                    return Err(Self::ERR_CROSS_TENANT_ROW.to_string());
                }
            }
        }
        Ok(())
    }

    /// Persists a session row binding tenant_id in the same statement.
    pub fn upsert_session_for_tenant_safe(&self, session: &kura_router::Session, tenant_id: &str) -> Result<(), String> {
        if tenant_id.is_empty() {
            return Err("UpsertSessionForTenantSafe: empty tenantID".to_string());
        }
        let affected = self
            .conn
            .execute(
                r#"INSERT INTO sessions (
                    session_id, kind, status, channel, account_id, peer_id, thread_id,
                    routing_key, generation, created_at, updated_at, last_active_at,
                    last_reset_at, tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                ON CONFLICT(session_id) DO UPDATE SET
                    kind = excluded.kind,
                    status = excluded.status,
                    channel = excluded.channel,
                    account_id = excluded.account_id,
                    peer_id = excluded.peer_id,
                    thread_id = excluded.thread_id,
                    routing_key = excluded.routing_key,
                    generation = excluded.generation,
                    created_at = excluded.created_at,
                    updated_at = excluded.updated_at,
                    last_active_at = excluded.last_active_at,
                    last_reset_at = excluded.last_reset_at,
                    tenant_id = excluded.tenant_id
                WHERE sessions.tenant_id IS NULL OR sessions.tenant_id = excluded.tenant_id"#,
                params![
                    session.session_id,
                    enum_str(&session.kind),
                    enum_str(&session.status),
                    session.channel,
                    null_string(&session.account_id),
                    session.peer_id,
                    null_string(&session.thread_id),
                    session.routing_key,
                    session.generation,
                    now_rfc3339(&session.created_at),
                    now_rfc3339(&session.updated_at),
                    now_rfc3339(&session.last_active_at),
                    opt_time_string(&session.last_reset_at),
                    tenant_id,
                ],
            )
            .map_err(|e| format!("upsert session for tenant: {e}"))?;
        if affected == 0 {
            if let Some(owner) = self.lookup_row_tenant("sessions", "session_id", &session.session_id)? {
                if !owner.is_empty() && owner != tenant_id {
                    return Err(Self::ERR_CROSS_TENANT_ROW.to_string());
                }
            }
        }
        Ok(())
    }

    /// Persists an llm dispatch row binding tenant_id in the same statement.
    pub fn upsert_llm_dispatch_for_tenant_safe(&self, dispatch: &kura_llm::Dispatch, tenant_id: &str) -> Result<(), String> {
        if tenant_id.is_empty() {
            return Err("UpsertLLMDispatchForTenantSafe: empty tenantID".to_string());
        }
        let messages_json = serde_json::to_string(&dispatch.messages)
            .map_err(|e| format!("marshal llm dispatch messages: {e}"))?;
        let usage_json = serde_json::to_string(&dispatch.usage)
            .map_err(|e| format!("marshal llm dispatch usage: {e}"))?;
        let affected = self
            .conn
            .execute(
                r#"INSERT INTO llm_dispatches (
                    dispatch_id, provider, model, messages_json, stream, status, output_text,
                    finish_reason, usage_json, error_code, error_text, timeout_ms, max_retries,
                    attempt_count, created_at, updated_at, started_at, completed_at, tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)
                ON CONFLICT(dispatch_id) DO UPDATE SET
                    provider = excluded.provider,
                    model = excluded.model,
                    messages_json = excluded.messages_json,
                    stream = excluded.stream,
                    status = excluded.status,
                    output_text = excluded.output_text,
                    finish_reason = excluded.finish_reason,
                    usage_json = excluded.usage_json,
                    error_code = excluded.error_code,
                    error_text = excluded.error_text,
                    timeout_ms = excluded.timeout_ms,
                    max_retries = excluded.max_retries,
                    attempt_count = excluded.attempt_count,
                    created_at = excluded.created_at,
                    updated_at = excluded.updated_at,
                    started_at = excluded.started_at,
                    completed_at = excluded.completed_at,
                    tenant_id = excluded.tenant_id
                WHERE llm_dispatches.tenant_id IS NULL OR llm_dispatches.tenant_id = excluded.tenant_id"#,
                params![
                    dispatch.dispatch_id,
                    dispatch.provider,
                    dispatch.model,
                    messages_json,
                    dispatch.stream,
                    enum_str(&dispatch.status),
                    dispatch.output,
                    null_string(&dispatch.finish_reason),
                    usage_json,
                    null_string(&dispatch.error_code),
                    null_string(&dispatch.error),
                    dispatch.timeout_ms,
                    dispatch.max_retries,
                    dispatch.attempt_count,
                    now_rfc3339(&dispatch.created_at),
                    now_rfc3339(&dispatch.updated_at),
                    opt_time_string(&dispatch.started_at),
                    opt_time_string(&dispatch.completed_at),
                    tenant_id,
                ],
            )
            .map_err(|e| format!("upsert llm dispatch for tenant: {e}"))?;
        if affected == 0 {
            if let Some(owner) = self.lookup_row_tenant("llm_dispatches", "dispatch_id", &dispatch.dispatch_id)? {
                if !owner.is_empty() && owner != tenant_id {
                    return Err(Self::ERR_CROSS_TENANT_ROW.to_string());
                }
            }
        }
        Ok(())
    }

    /// Persists a step row binding tenant_id in the same statement. Cross-tenant
    /// collisions are atomically refused — tenant A's row is never modified by tenant
    /// B's write attempt.
    pub fn upsert_step_for_tenant_safe(&self, step: &kura_runtime::Step, tenant_id: &str) -> Result<(), String> {
        if tenant_id.is_empty() {
            return Err("UpsertStepForTenantSafe: empty tenantID".to_string());
        }
        let input_json = marshal_json(&step.input)?;
        let output_json = marshal_json(&step.output)?;
        let affected = self
            .conn
            .execute(
                r#"INSERT INTO steps (
                    step_id, run_id, workflow_id, workflow_step_id, attempt, title, kind,
                    status, input_json, output_json, created_at, updated_at, tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                ON CONFLICT(step_id) DO UPDATE SET
                    run_id = excluded.run_id,
                    workflow_id = excluded.workflow_id,
                    workflow_step_id = excluded.workflow_step_id,
                    attempt = excluded.attempt,
                    title = excluded.title,
                    kind = excluded.kind,
                    status = excluded.status,
                    input_json = excluded.input_json,
                    output_json = excluded.output_json,
                    created_at = excluded.created_at,
                    updated_at = excluded.updated_at,
                    tenant_id = excluded.tenant_id
                WHERE steps.tenant_id IS NULL OR steps.tenant_id = excluded.tenant_id"#,
                params![
                    step.step_id,
                    step.run_id,
                    null_string(&step.workflow_id),
                    null_string(&step.workflow_step_id),
                    step.attempt,
                    step.title,
                    step.kind,
                    step.status.as_str(),
                    input_json,
                    output_json,
                    now_rfc3339(&step.created_at),
                    now_rfc3339(&step.updated_at),
                    tenant_id,
                ],
            )
            .map_err(|e| format!("upsert step for tenant: {e}"))?;
        if affected == 0 {
            if let Some(owner) = self.lookup_row_tenant("steps", "step_id", &step.step_id)? {
                if !owner.is_empty() && owner != tenant_id {
                    return Err(Self::ERR_CROSS_TENANT_ROW.to_string());
                }
            }
        }
        Ok(())
    }

    /// Persists a tool_call row binding tenant_id in the same statement.
    pub fn upsert_tool_call_for_tenant_safe(&self, tool_call: &kura_runtime::ToolCall, tenant_id: &str) -> Result<(), String> {
        if tenant_id.is_empty() {
            return Err("UpsertToolCallForTenantSafe: empty tenantID".to_string());
        }
        let input_json = marshal_json(&tool_call.input)?;
        let output_json = marshal_json(&tool_call.output)?;
        let sandbox_json = marshal_map(&tool_call.sandbox)?;
        let integration_bindings_json = marshal_vec(&tool_call.integration_bindings)?;
        let affected = self
            .conn
            .execute(
                r#"INSERT INTO tool_calls (
                    tool_call_id, run_id, step_id, workflow_id, workflow_step_id, attempt,
                    computer_use_session_id, computer_use_action_id, invocation_kind, capability_id,
                    skill_id, mcp_server_id, mcp_server_name, mcp_tool_name, mcp_transport_kind,
                    mcp_session_id, authorization_result, tool_name, status, sandbox_execution_id,
                    failure_class, input_json, output_json, sandbox_json,
                    integration_bindings_json, error_text, created_at, updated_at, tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29)
                ON CONFLICT(tool_call_id) DO UPDATE SET
                    run_id = excluded.run_id,
                    step_id = excluded.step_id,
                    workflow_id = excluded.workflow_id,
                    workflow_step_id = excluded.workflow_step_id,
                    attempt = excluded.attempt,
                    computer_use_session_id = excluded.computer_use_session_id,
                    computer_use_action_id = excluded.computer_use_action_id,
                    invocation_kind = excluded.invocation_kind,
                    capability_id = excluded.capability_id,
                    skill_id = excluded.skill_id,
                    mcp_server_id = excluded.mcp_server_id,
                    mcp_server_name = excluded.mcp_server_name,
                    mcp_tool_name = excluded.mcp_tool_name,
                    mcp_transport_kind = excluded.mcp_transport_kind,
                    mcp_session_id = excluded.mcp_session_id,
                    authorization_result = excluded.authorization_result,
                    tool_name = excluded.tool_name,
                    status = excluded.status,
                    sandbox_execution_id = excluded.sandbox_execution_id,
                    failure_class = excluded.failure_class,
                    input_json = excluded.input_json,
                    output_json = excluded.output_json,
                    sandbox_json = excluded.sandbox_json,
                    integration_bindings_json = excluded.integration_bindings_json,
                    error_text = excluded.error_text,
                    created_at = excluded.created_at,
                    updated_at = excluded.updated_at,
                    tenant_id = excluded.tenant_id
                WHERE tool_calls.tenant_id IS NULL OR tool_calls.tenant_id = excluded.tenant_id"#,
                params![
                    tool_call.tool_call_id,
                    tool_call.run_id,
                    tool_call.step_id,
                    null_string(&tool_call.workflow_id),
                    null_string(&tool_call.workflow_step_id),
                    tool_call.attempt,
                    null_string(&tool_call.computer_use_session_id),
                    null_string(&tool_call.computer_use_action_id),
                    null_string(&tool_call.invocation_kind),
                    tool_call.capability_id,
                    null_string(&tool_call.skill_id),
                    null_string(&tool_call.mcp_server_id),
                    null_string(&tool_call.mcp_server_name),
                    null_string(&tool_call.mcp_tool_name),
                    null_string(&tool_call.mcp_transport_kind),
                    null_string(&tool_call.mcp_session_id),
                    null_string(&tool_call.authorization_result),
                    tool_call.tool_name,
                    tool_call.status.as_str(),
                    null_string(&tool_call.sandbox_execution_id),
                    null_string(&tool_call.failure_class),
                    input_json,
                    output_json,
                    sandbox_json,
                    integration_bindings_json,
                    null_string(&tool_call.error),
                    now_rfc3339(&tool_call.created_at),
                    now_rfc3339(&tool_call.updated_at),
                    tenant_id,
                ],
            )
            .map_err(|e| format!("upsert tool_call for tenant: {e}"))?;
        if affected == 0 {
            if let Some(owner) = self.lookup_row_tenant("tool_calls", "tool_call_id", &tool_call.tool_call_id)? {
                if !owner.is_empty() && owner != tenant_id {
                    return Err(Self::ERR_CROSS_TENANT_ROW.to_string());
                }
            }
        }
        Ok(())
    }

    /// Persists a checkpoint row and binds tenant_id to it. Because checkpoint_id is
    /// generated server-side and not surfaced to callers, the bind step uses
    /// bind_run_checkpoints_tenant on the parent run.
    ///
    /// This is the store-side equivalent of the Go tenancy Runtime.SaveCheckpointForTenant
    /// (Require + SaveCheckpoint + BindRunCheckpointsTenant).
    pub fn save_checkpoint_for_tenant_safe(
        &self,
        checkpoint: &kura_runtime::RunCheckpoint,
        tenant_id: &str,
    ) -> Result<(), String> {
        if tenant_id.is_empty() {
            return Err("SaveCheckpointForTenantSafe: empty tenantID".to_string());
        }
        self.save_checkpoint(checkpoint)?;
        self.bind_run_checkpoints_tenant(&checkpoint.run.run_id, tenant_id)
    }

    /// Appends an event row with tenant_id pre-bound in a single INSERT. Unlike the
    /// events-table updates (events are append-only) this is a plain INSERT plus
    /// tenant_id assignment, returning the persisted event with its sequence.
    ///
    /// The events table is mixed (tenant-owned + global categories share it). Callers
    /// MUST refuse to call this for global categories — the tenancy layer's
    /// is_global_category gate enforces it.
    pub fn append_event_for_tenant_safe(
        &self,
        input: &AppendEventInput,
        tenant_id: &str,
    ) -> Result<AppendEventResult, String> {
        if tenant_id.is_empty() {
            return Err("AppendEventForTenantSafe: empty tenantID".to_string());
        }
        self.conn
            .execute(
                r#"INSERT INTO events (
                    event_id, environment_scope, category, name, occurred_at, session_id, run_id,
                    workflow_id, workflow_step_id, schedule_id, schedule_attempt_id, step_id,
                    connector_id, capability_id, resource_kind, resource_id, payload_json,
                    tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
                ON CONFLICT(event_id) DO NOTHING"#,
                params![
                    input.event_id,
                    input.environment_scope,
                    input.category,
                    input.name,
                    now_rfc3339(&input.occurred_at),
                    null_string(&input.session_id),
                    null_string(&input.run_id),
                    null_string(&input.workflow_id),
                    null_string(&input.workflow_step_id),
                    null_string(&input.schedule_id),
                    null_string(&input.schedule_attempt_id),
                    null_string(&input.step_id),
                    null_string(&input.connector_id),
                    null_string(&input.capability_id),
                    input.resource_kind,
                    input.resource_id,
                    input.payload_json,
                    tenant_id,
                ],
            )
            .map_err(|e| format!("append event for tenant: {e}"))?;
        let sequence: i64 = self
            .conn
            .query_row(
                "SELECT rowid FROM events WHERE event_id = ?1",
                params![input.event_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("load event sequence for tenant: {e}"))?;
        Ok(AppendEventResult { sequence, tenant_id: tenant_id.to_string() })
    }
}

// ---------------------------------------------------------------------------
// Per-domain tenant-aware RAW reads
// ---------------------------------------------------------------------------

impl SQLiteStore {
    /// Appends a tenant-owned event with tenant_id bound atomically in the same
    /// INSERT. Returns the persisted event including the assigned sequence. Refuses
    /// (returns ERR_CROSS_TENANT_ROW) when an event row with the same event_id
    /// already exists owned by a different tenant, or is a global-category row
    /// (NULL tenant_id) that must not be claimed. Pre-backfill tenant-owned rows
    /// (NULL tenant_id, non-global category) are claimed atomically.
    pub fn append_event_for_tenant_raw(
        &self,
        event: &kura_events::Event,
        tenant_id: &str,
    ) -> Result<kura_events::Event, String> {
        if tenant_id.is_empty() {
            return Err("AppendEventForTenantRaw: empty tenantID".to_string());
        }
        if event.event_id.trim().is_empty() {
            return Err("AppendEventForTenantRaw: empty event_id (caller must pre-fill via ensureEventDefaults)".to_string());
        }
        let payload_json =
            serde_json::to_string(&event.payload).map_err(|e| format!("marshal event payload: {e}"))?;
        let affected = self
            .conn
            .execute(
                r#"INSERT INTO events (
                    event_id, environment_scope, category, name, occurred_at, session_id, run_id,
                    workflow_id, workflow_step_id, schedule_id, schedule_attempt_id, step_id,
                    connector_id, capability_id, resource_kind, resource_id, payload_json,
                    tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
                ON CONFLICT(event_id) DO NOTHING"#,
                params![
                    event.event_id,
                    event.environment_scope,
                    event.category,
                    event.name,
                    now_rfc3339(&event.occurred_at),
                    null_string(&event.scope.session_id),
                    null_string(&event.scope.run_id),
                    null_string(&event.scope.workflow_id),
                    null_string(&event.scope.workflow_step_id),
                    null_string(&event.scope.schedule_id),
                    null_string(&event.scope.schedule_attempt_id),
                    null_string(&event.scope.step_id),
                    null_string(&event.scope.connector_id),
                    null_string(&event.scope.capability_id),
                    event.resource.kind,
                    event.resource.id,
                    payload_json,
                    tenant_id,
                ],
            )
            .map_err(|e| format!("append event for tenant: {e}"))?;
        if affected == 0 {
            // A row with the same event_id already exists. Disambiguate so callers can
            // audit cross-tenant collisions AND so we refuse to claim NULL-tenant rows
            // that belong to a global category.
            let (existing_tenant, existing_category): (Option<String>, String) = self
                .conn
                .query_row(
                    "SELECT tenant_id, category FROM events WHERE event_id = ?1",
                    params![event.event_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(|e| format!("append event lookup existing: {e}"))?;
            if let Some(existing) = existing_tenant {
                if !existing.is_empty() && existing != tenant_id {
                    return Err(Self::ERR_CROSS_TENANT_ROW.to_string());
                }
            } else {
                // NULL tenant_id row. Global categories are never claimed; non-global
                // rows are pre-backfill compatibility and are claimed atomically.
                if kura_events::is_global_category(&existing_category) {
                    return Err(Self::ERR_CROSS_TENANT_ROW.to_string());
                }
                self.conn
                    .execute(
                        "UPDATE events SET tenant_id = ?1 WHERE event_id = ?2 AND tenant_id IS NULL",
                        params![tenant_id, event.event_id],
                    )
                    .map_err(|e| format!("claim tenant on existing event: {e}"))?;
            }
        }
        let sequence: i64 = self
            .conn
            .query_row(
                "SELECT rowid FROM events WHERE event_id = ?1",
                params![event.event_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("load event sequence {}: {e}", event.event_id))?;
        let mut out = event.clone();
        out.sequence = sequence;
        out.tenant_id = tenant_id.to_string();
        Ok(out)
    }

    /// Returns events whose tenant_id matches tenant_id. Reads tenant_id directly from
    /// the events table; global rows (NULL tenant_id) are excluded by the WHERE
    /// clause.
    pub fn list_events_for_tenant_raw(
        &self,
        tenant_id: &str,
        filter: &kura_events::Filter,
    ) -> Result<Vec<kura_events::Event>, String> {
        let mut sql = String::from(
            r#"SELECT rowid, event_id, environment_scope, category, name, occurred_at, session_id,
                run_id, workflow_id, workflow_step_id, schedule_id, schedule_attempt_id, step_id,
                connector_id, capability_id, resource_kind, resource_id, payload_json, tenant_id
            FROM events
            WHERE tenant_id = ?1"#,
        );
        let mut args: Vec<Value> = vec![Value::Text(tenant_id.to_string())];
        if !filter.environment_scope.trim().is_empty() {
            sql.push_str(" AND environment_scope = ?");
            args.push(Value::Text(filter.environment_scope.trim().to_string()));
        }
        if !filter.category.trim().is_empty() {
            sql.push_str(" AND category = ?");
            args.push(Value::Text(filter.category.trim().to_string()));
        }
        if !filter.run_id.trim().is_empty() {
            sql.push_str(" AND run_id = ?");
            args.push(Value::Text(filter.run_id.trim().to_string()));
        }
        if !filter.session_id.trim().is_empty() {
            sql.push_str(" AND session_id = ?");
            args.push(Value::Text(filter.session_id.trim().to_string()));
        }
        if !filter.schedule_id.trim().is_empty() {
            sql.push_str(" AND schedule_id = ?");
            args.push(Value::Text(filter.schedule_id.trim().to_string()));
        }
        if !filter.schedule_attempt_id.trim().is_empty() {
            sql.push_str(" AND schedule_attempt_id = ?");
            args.push(Value::Text(filter.schedule_attempt_id.trim().to_string()));
        }
        if !filter.resource_kind.trim().is_empty() {
            sql.push_str(" AND resource_kind = ?");
            args.push(Value::Text(filter.resource_kind.trim().to_string()));
        }
        if filter.cursor > 0 {
            sql.push_str(" AND rowid > ?");
            args.push(Value::Integer(filter.cursor));
        }
        sql.push_str(" ORDER BY rowid ASC");

        let mut stmt = self.conn.prepare(&sql).map_err(|e| format!("list events for tenant: {e}"))?;
        let mut rows = stmt.query(params_from_iter(args.iter())).map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_event_with_tenant(row)?);
        }
        Ok(items)
    }

    /// Mirrors list_approvals but filtered by tenant.
    pub fn list_approvals_for_tenant_raw(&self, tenant_id: &str) -> Result<Vec<kura_policy::Approval>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT approval_id, action, resource_kind, resource_id, reason, requested_by,
                    status, created_at, updated_at, resolved_at, resolution, comment,
                    integration_bindings_json
                FROM approvals
                WHERE tenant_id = ?1
                ORDER BY created_at ASC, approval_id ASC"#,
            )
            .map_err(|e| format!("list approvals for tenant: {e}"))?;
        let mut rows = stmt.query(params![tenant_id]).map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_approval(row)?);
        }
        Ok(items)
    }

    /// Mirrors list_decisions but filtered by tenant.
    pub fn list_decisions_for_tenant_raw(&self, tenant_id: &str) -> Result<Vec<kura_policy::Decision>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT decision_id, action, resource_kind, resource_id, outcome, reason,
                    approval_id, created_at
                FROM decisions
                WHERE tenant_id = ?1
                ORDER BY created_at ASC, decision_id ASC"#,
            )
            .map_err(|e| format!("list decisions for tenant: {e}"))?;
        let mut rows = stmt.query(params![tenant_id]).map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_decision(row)?);
        }
        Ok(items)
    }

    /// Mirrors list_schedules but filtered by tenant (and environment scope).
    pub fn list_schedules_for_tenant_raw(
        &self,
        tenant_id: &str,
        environment_scope: &str,
    ) -> Result<Vec<crate::schedule::ScheduleRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT schedule_id, environment_scope, tenant_id, kind, status, target_ref_id,
                    timezone, next_due_at, last_attempt_at, last_outcome, created_at, updated_at,
                    paused_at, cancelled_at, completed_at, document_json
                FROM schedules
                WHERE tenant_id = ?1 AND environment_scope = ?2
                ORDER BY created_at ASC, schedule_id ASC"#,
            )
            .map_err(|e| format!("list schedules for tenant: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id, environment_scope.trim()])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_schedule(row)?);
        }
        Ok(items)
    }

    /// Mirrors list_workflows but filtered by tenant (and environment scope + run).
    /// Each row is decoded via the same document_json path as get_workflow so child
    /// steps/dependencies/handoffs are loaded consistently.
    pub fn list_workflows_for_tenant_raw(
        &self,
        tenant_id: &str,
        environment_scope: &str,
        run_id: &str,
    ) -> Result<Vec<kura_orchestration::Workflow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT workflow_id
                FROM workflows
                WHERE tenant_id = ?1 AND environment_scope = ?2 AND run_id = ?3
                ORDER BY created_at ASC, workflow_id ASC"#,
            )
            .map_err(|e| format!("list workflows for tenant: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id, environment_scope.trim(), run_id.trim()])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let workflow_id: String = row.get(0).map_err(|e| e.to_string())?;
            if let Some(workflow) = self.get_workflow(environment_scope, run_id, &workflow_id)? {
                items.push(workflow);
            }
        }
        Ok(items)
    }

    /// Returns the tenant_id of an mcp_tools row keyed by the composite PK
    /// (server_id, tool_name). Ok(None) when the row is absent.
    pub fn mcp_tool_tenant_id(&self, server_id: &str, tool_name: &str) -> Result<Option<String>, String> {
        match self.conn.query_row(
            r#"SELECT tenant_id FROM mcp_tools
            WHERE server_id = ?1 AND tool_name = ?2"#,
            params![server_id, tool_name],
            |row| row.get::<_, Option<String>>(0),
        ) {
            Ok(tenant) => Ok(tenant),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(format!("lookup tenant for mcp_tools: {e}")),
        }
    }

    /// Sets tenant_id on an mcp_tools row keyed by (server_id, tool_name), refusing
    /// to overwrite a previously-bound non-matching tenant. Returns
    /// ERR_CROSS_TENANT_ROW on mismatch; idempotent for the matching case.
    pub fn bind_mcp_tool_tenant(
        &self,
        server_id: &str,
        tool_name: &str,
        tenant_id: &str,
    ) -> Result<(), String> {
        if tenant_id.is_empty() {
            return Err("bindMCPToolTenant: empty tenantID".to_string());
        }
        let affected = self
            .conn
            .execute(
                r#"UPDATE mcp_tools
                SET tenant_id = ?1
                WHERE server_id = ?2 AND tool_name = ?3 AND (tenant_id IS NULL OR tenant_id = ?1)"#,
                params![tenant_id, server_id, tool_name],
            )
            .map_err(|e| format!("bind tenant for mcp_tools: {e}"))?;
        if affected != 0 {
            return Ok(());
        }
        match self.mcp_tool_tenant_id(server_id, tool_name)? {
            Some(existing) if !existing.is_empty() && existing != tenant_id => {
                Err(Self::ERR_CROSS_TENANT_ROW.to_string())
            }
            _ => Ok(()),
        }
    }
}

// ---------------------------------------------------------------------------
// Default-personal-tenant resolver (Go: default_tenant.go)
// ---------------------------------------------------------------------------

/// Per-database default-tenant cache entry. The cache is process-global keyed by
/// database path because the Go store carries it on the store instance and this
/// crate's SQLiteStore struct cannot be extended from the tenancy module.
#[derive(Debug, Clone, Default)]
struct DefaultTenantCacheEntry {
    id: String,
    seeded: bool,
}

static DEFAULT_TENANT_CACHE: LazyLock<Mutex<HashMap<String, DefaultTenantCacheEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Paths that already emitted the cold-path warning (warn-once semantics per db).
static COLD_WARNED_PATHS: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

impl SQLiteStore {
    /// Returns the bootstrapped personal tenant id, caching after the first read.
    /// Returns ERR_DEFAULT_PERSONAL_TENANT_UNAVAILABLE only when no personal tenant
    /// has been bootstrapped (pre-bootstrap boot path).
    pub fn resolve_default_personal_tenant_id(&self) -> Result<String, String> {
        let key = self.db_path();
        {
            let cache = DEFAULT_TENANT_CACHE.lock().unwrap();
            if let Some(entry) = cache.get(key) {
                if !entry.id.is_empty() {
                    return Ok(entry.id.clone());
                }
            }
        }
        let id: Option<String> = match self.conn.query_row(
            r#"SELECT tenant_id FROM tenants
            WHERE tenant_kind = 'personal'
            ORDER BY created_at ASC LIMIT 1"#,
            [],
            |row| row.get(0),
        ) {
            Ok(id) => id,
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                return Err(Self::ERR_DEFAULT_PERSONAL_TENANT_UNAVAILABLE.to_string());
            }
            Err(e) => return Err(format!("resolve default personal tenant: {e}")),
        };
        match id {
            Some(id) if !id.trim().is_empty() => {
                let id = id.trim().to_string();
                DEFAULT_TENANT_CACHE.lock().unwrap().insert(
                    key.to_string(),
                    DefaultTenantCacheEntry { id: id.clone(), seeded: true },
                );
                Ok(id)
            }
            _ => Err(Self::ERR_DEFAULT_PERSONAL_TENANT_UNAVAILABLE.to_string()),
        }
    }

    /// Reads the personal tenant id and primes the in-memory cache so that
    /// resolve_default_tenant_binding can answer purely from memory afterwards.
    /// Called from the app boot path immediately after bootstrap completes. Safe to
    /// call repeatedly; a no-op once the cache is populated.
    pub fn seed_default_tenant_cache(&self) -> Result<(), String> {
        let key = self.db_path();
        {
            let cache = DEFAULT_TENANT_CACHE.lock().unwrap();
            if let Some(entry) = cache.get(key) {
                if !entry.id.is_empty() {
                    return Ok(());
                }
            }
        }
        match self.resolve_default_personal_tenant_id() {
            Err(e) if Self::is_default_personal_tenant_unavailable(&e) => {
                // "No personal tenant yet" is a legitimate pre-bootstrap state — record
                // that we did look so the fail-closed branch is not tripped next call.
                DEFAULT_TENANT_CACHE.lock().unwrap().insert(
                    key.to_string(),
                    DefaultTenantCacheEntry { id: String::new(), seeded: true },
                );
                Ok(())
            }
            other => other.map(|_| ()),
        }
    }

    /// Returns the resolved default-personal tenant id, or None when no personal
    /// tenant has been bootstrapped yet. Fail-closed: when the cache was never
    /// seeded, the cold path refuses to bind and logs a one-time warning
    /// (post-enforcement the schema's NOT NULL/CHECK constraints surface the
    /// missed-seeding bug loudly).
    pub fn resolve_default_tenant_binding(&self) -> Option<String> {
        let key = self.db_path();
        let (id, seeded) = {
            let cache = DEFAULT_TENANT_CACHE.lock().unwrap();
            match cache.get(key) {
                Some(entry) => (entry.id.clone(), entry.seeded),
                None => (String::new(), false),
            }
        };
        if !id.is_empty() {
            return Some(id);
        }
        if seeded {
            // Seeding ran but found no personal tenant (pre-bootstrap). The caller's
            // INSERT writes tenant_id=NULL, which enforcement rejects loudly.
            return None;
        }
        let mut warned = COLD_WARNED_PATHS.lock().unwrap();
        if warned.insert(key.to_string()) {
            eprintln!(
                "store: ResolveDefaultTenantBinding called before SeedDefaultTenantCache; \
                 writes will return NULL tenant_id and may be rejected by the T077a/b \
                 NOT NULL/CHECK enforcement. App boot must call SeedDefaultTenantCache \
                 after BootstrapLocal."
            );
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Scanners (self-contained; the sibling modules keep theirs private)
// ---------------------------------------------------------------------------

fn scan_run(row: &Row) -> Result<kura_runtime::Run, String> {
    let run_id: String = row.get(0).map_err(|e| e.to_string())?;
    let session_id: Option<String> = row.get(1).map_err(|e| e.to_string())?;
    let schedule_id: Option<String> = row.get(2).map_err(|e| e.to_string())?;
    let schedule_attempt_id: Option<String> = row.get(3).map_err(|e| e.to_string())?;
    let reminder_id: Option<String> = row.get(4).map_err(|e| e.to_string())?;
    let reminder_occurrence_id: Option<String> = row.get(5).map_err(|e| e.to_string())?;
    let entrypoint: String = row.get(6).map_err(|e| e.to_string())?;
    let status: String = row.get(7).map_err(|e| e.to_string())?;
    let goal: String = row.get(8).map_err(|e| e.to_string())?;
    let created_at: String = row.get(9).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(10).map_err(|e| e.to_string())?;

    Ok(kura_runtime::Run {
        run_id,
        session_id: session_id.unwrap_or_default(),
        schedule_id: schedule_id.unwrap_or_default(),
        schedule_attempt_id: schedule_attempt_id.unwrap_or_default(),
        reminder_id: reminder_id.unwrap_or_default(),
        reminder_occurrence_id: reminder_occurrence_id.unwrap_or_default(),
        entrypoint,
        status: parse_enum(&status)?,
        goal,
        created_at: parse_rfc3339(&created_at)?,
        updated_at: parse_rfc3339(&updated_at)?,
        ..kura_runtime::Run::default()
    })
}

/// The get_run_for_tenant_raw scan: run columns plus tenant_id at index 11.
fn scan_run_tenant(row: &Row) -> Result<kura_runtime::Run, String> {
    let run_id: String = row.get(0).map_err(|e| e.to_string())?;
    let session_id: Option<String> = row.get(1).map_err(|e| e.to_string())?;
    let schedule_id: Option<String> = row.get(2).map_err(|e| e.to_string())?;
    let schedule_attempt_id: Option<String> = row.get(3).map_err(|e| e.to_string())?;
    let reminder_id: Option<String> = row.get(4).map_err(|e| e.to_string())?;
    let reminder_occurrence_id: Option<String> = row.get(5).map_err(|e| e.to_string())?;
    let entrypoint: String = row.get(6).map_err(|e| e.to_string())?;
    let status: String = row.get(7).map_err(|e| e.to_string())?;
    let goal: String = row.get(8).map_err(|e| e.to_string())?;
    let created_at: String = row.get(9).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(10).map_err(|e| e.to_string())?;

    Ok(kura_runtime::Run {
        run_id,
        session_id: session_id.unwrap_or_default(),
        schedule_id: schedule_id.unwrap_or_default(),
        schedule_attempt_id: schedule_attempt_id.unwrap_or_default(),
        reminder_id: reminder_id.unwrap_or_default(),
        reminder_occurrence_id: reminder_occurrence_id.unwrap_or_default(),
        entrypoint,
        status: parse_enum(&status)?,
        goal,
        created_at: parse_rfc3339(&created_at)?,
        updated_at: parse_rfc3339(&updated_at)?,
        ..kura_runtime::Run::default()
    })
}

fn scan_session(row: &Row) -> Result<kura_router::Session, String> {
    let session_id: String = row.get(0).map_err(|e| e.to_string())?;
    let kind: String = row.get(1).map_err(|e| e.to_string())?;
    let status: String = row.get(2).map_err(|e| e.to_string())?;
    let channel: String = row.get(3).map_err(|e| e.to_string())?;
    let account_id: Option<String> = row.get(4).map_err(|e| e.to_string())?;
    let peer_id: String = row.get(5).map_err(|e| e.to_string())?;
    let thread_id: Option<String> = row.get(6).map_err(|e| e.to_string())?;
    let routing_key: String = row.get(7).map_err(|e| e.to_string())?;
    let generation: i64 = row.get(8).map_err(|e| e.to_string())?;
    let created_at: String = row.get(9).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(10).map_err(|e| e.to_string())?;
    let last_active_at: String = row.get(11).map_err(|e| e.to_string())?;
    let last_reset_at: Option<String> = row.get(12).map_err(|e| e.to_string())?;

    Ok(kura_router::Session {
        session_id,
        kind: parse_enum(&kind)?,
        status: parse_enum(&status)?,
        channel,
        account_id: account_id.unwrap_or_default(),
        peer_id,
        thread_id: thread_id.unwrap_or_default(),
        routing_key,
        generation,
        created_at: parse_rfc3339(&created_at)?,
        updated_at: parse_rfc3339(&updated_at)?,
        last_active_at: parse_rfc3339(&last_active_at)?,
        last_reset_at: parse_opt_rfc3339(last_reset_at)?,
        active_profile_projection: None,
    })
}

fn scan_step(row: &Row) -> Result<kura_runtime::Step, String> {
    let step_id: String = row.get(0).map_err(|e| e.to_string())?;
    let run_id: String = row.get(1).map_err(|e| e.to_string())?;
    let workflow_id: Option<String> = row.get(2).map_err(|e| e.to_string())?;
    let workflow_step_id: Option<String> = row.get(3).map_err(|e| e.to_string())?;
    let attempt: i64 = row.get(4).map_err(|e| e.to_string())?;
    let title: String = row.get(5).map_err(|e| e.to_string())?;
    let kind: String = row.get(6).map_err(|e| e.to_string())?;
    let status: String = row.get(7).map_err(|e| e.to_string())?;
    let input_json: Option<String> = row.get(8).map_err(|e| e.to_string())?;
    let output_json: Option<String> = row.get(9).map_err(|e| e.to_string())?;
    let created_at: String = row.get(10).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(11).map_err(|e| e.to_string())?;

    Ok(kura_runtime::Step {
        step_id,
        run_id,
        workflow_id: workflow_id.unwrap_or_default(),
        workflow_step_id: workflow_step_id.unwrap_or_default(),
        attempt,
        title,
        kind,
        status: parse_enum(&status)?,
        created_at: parse_rfc3339(&created_at)?,
        updated_at: parse_rfc3339(&updated_at)?,
        input: decode_opt_json(&input_json)?,
        output: decode_opt_json(&output_json)?,
    })
}

fn scan_tool_call(row: &Row) -> Result<kura_runtime::ToolCall, String> {
    let tool_call_id: String = row.get(0).map_err(|e| e.to_string())?;
    let run_id: String = row.get(1).map_err(|e| e.to_string())?;
    let step_id: String = row.get(2).map_err(|e| e.to_string())?;
    let workflow_id: Option<String> = row.get(3).map_err(|e| e.to_string())?;
    let workflow_step_id: Option<String> = row.get(4).map_err(|e| e.to_string())?;
    let attempt: i64 = row.get(5).map_err(|e| e.to_string())?;
    let computer_use_session_id: Option<String> = row.get(6).map_err(|e| e.to_string())?;
    let computer_use_action_id: Option<String> = row.get(7).map_err(|e| e.to_string())?;
    let invocation_kind: Option<String> = row.get(8).map_err(|e| e.to_string())?;
    let capability_id: String = row.get(9).map_err(|e| e.to_string())?;
    let skill_id: Option<String> = row.get(10).map_err(|e| e.to_string())?;
    let mcp_server_id: Option<String> = row.get(11).map_err(|e| e.to_string())?;
    let mcp_server_name: Option<String> = row.get(12).map_err(|e| e.to_string())?;
    let mcp_tool_name: Option<String> = row.get(13).map_err(|e| e.to_string())?;
    let mcp_transport_kind: Option<String> = row.get(14).map_err(|e| e.to_string())?;
    let mcp_session_id: Option<String> = row.get(15).map_err(|e| e.to_string())?;
    let authorization_result: Option<String> = row.get(16).map_err(|e| e.to_string())?;
    let tool_name: String = row.get(17).map_err(|e| e.to_string())?;
    let status: String = row.get(18).map_err(|e| e.to_string())?;
    let sandbox_execution_id: Option<String> = row.get(19).map_err(|e| e.to_string())?;
    let failure_class: Option<String> = row.get(20).map_err(|e| e.to_string())?;
    let input_json: Option<String> = row.get(21).map_err(|e| e.to_string())?;
    let output_json: Option<String> = row.get(22).map_err(|e| e.to_string())?;
    let sandbox_json: Option<String> = row.get(23).map_err(|e| e.to_string())?;
    let integration_bindings_json: Option<String> = row.get(24).map_err(|e| e.to_string())?;
    let error_text: Option<String> = row.get(25).map_err(|e| e.to_string())?;
    let created_at: String = row.get(26).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(27).map_err(|e| e.to_string())?;

    Ok(kura_runtime::ToolCall {
        tool_call_id,
        run_id,
        step_id,
        workflow_id: workflow_id.unwrap_or_default(),
        workflow_step_id: workflow_step_id.unwrap_or_default(),
        attempt,
        computer_use_session_id: computer_use_session_id.unwrap_or_default(),
        computer_use_action_id: computer_use_action_id.unwrap_or_default(),
        invocation_kind: invocation_kind.unwrap_or_default(),
        capability_id,
        skill_id: skill_id.unwrap_or_default(),
        mcp_server_id: mcp_server_id.unwrap_or_default(),
        mcp_server_name: mcp_server_name.unwrap_or_default(),
        mcp_tool_name: mcp_tool_name.unwrap_or_default(),
        mcp_transport_kind: mcp_transport_kind.unwrap_or_default(),
        mcp_session_id: mcp_session_id.unwrap_or_default(),
        authorization_result: authorization_result.unwrap_or_default(),
        tool_name,
        status: parse_enum(&status)?,
        sandbox_execution_id: sandbox_execution_id.unwrap_or_default(),
        failure_class: failure_class.unwrap_or_default(),
        created_at: parse_rfc3339(&created_at)?,
        updated_at: parse_rfc3339(&updated_at)?,
        input: decode_opt_json(&input_json)?,
        output: decode_opt_json(&output_json)?,
        sandbox: decode_map(&sandbox_json)?,
        integration_bindings: decode_vec(&integration_bindings_json)?,
        error: error_text.unwrap_or_default(),
        ..kura_runtime::ToolCall::default()
    })
}

fn scan_llm_dispatch(row: &Row) -> Result<kura_llm::Dispatch, String> {
    let dispatch_id: String = row.get(0).map_err(|e| e.to_string())?;
    let provider: String = row.get(1).map_err(|e| e.to_string())?;
    let model: String = row.get(2).map_err(|e| e.to_string())?;
    let messages_raw: String = row.get(3).map_err(|e| e.to_string())?;
    let stream: bool = row.get(4).map_err(|e| e.to_string())?;
    let status: String = row.get(5).map_err(|e| e.to_string())?;
    let output: String = row.get(6).map_err(|e| e.to_string())?;
    let finish_reason: Option<String> = row.get(7).map_err(|e| e.to_string())?;
    let usage_raw: String = row.get(8).map_err(|e| e.to_string())?;
    let error_code: Option<String> = row.get(9).map_err(|e| e.to_string())?;
    let error_text: Option<String> = row.get(10).map_err(|e| e.to_string())?;
    let timeout_ms: i64 = row.get(11).map_err(|e| e.to_string())?;
    let max_retries: i64 = row.get(12).map_err(|e| e.to_string())?;
    let attempt_count: i64 = row.get(13).map_err(|e| e.to_string())?;
    let created_at: String = row.get(14).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(15).map_err(|e| e.to_string())?;
    let started_at: Option<String> = row.get(16).map_err(|e| e.to_string())?;
    let completed_at: Option<String> = row.get(17).map_err(|e| e.to_string())?;
    let tools_raw: Option<String> = row.get(18).map_err(|e| e.to_string())?;
    let tool_calls_raw: Option<String> = row.get(19).map_err(|e| e.to_string())?;

    let status: kura_llm::DispatchStatus = parse_enum(&status)?;
    let messages: Vec<kura_llm::Message> =
        crate::crud::decode_json_field(&messages_raw).map_err(|e| format!("decode llm dispatch messages: {e}"))?;
    // Null for every dispatch written before these columns existed, and for
    // any plain chat request since.
    let tools: Vec<kura_llm::ToolSpec> = match tools_raw {
        Some(raw) if !raw.is_empty() => crate::crud::decode_json_field(&raw)
            .map_err(|e| format!("decode llm dispatch tools: {e}"))?,
        _ => Vec::new(),
    };
    let tool_calls: Vec<kura_llm::ToolCall> = match tool_calls_raw {
        Some(raw) if !raw.is_empty() => crate::crud::decode_json_field(&raw)
            .map_err(|e| format!("decode llm dispatch tool calls: {e}"))?,
        _ => Vec::new(),
    };
    let usage: kura_llm::Usage =
        crate::crud::decode_json_field(&usage_raw).map_err(|e| format!("decode llm dispatch usage: {e}"))?;
    let partial = status == kura_llm::DispatchStatus::PartialFailed;

    Ok(kura_llm::Dispatch {
        dispatch_id,
        provider,
        model,
        messages,
        tools,
        tool_calls,
        stream,
        status,
        output,
        finish_reason: finish_reason.unwrap_or_default(),
        usage,
        error_code: error_code.unwrap_or_default(),
        error: error_text.unwrap_or_default(),
        timeout_ms,
        partial,
        max_retries,
        attempt_count,
        created_at: parse_rfc3339(&created_at)?,
        updated_at: parse_rfc3339(&updated_at)?,
        started_at: parse_opt_rfc3339(started_at)?,
        completed_at: parse_opt_rfc3339(completed_at)?,
    })
}

fn scan_approval(row: &Row) -> Result<kura_policy::Approval, String> {
    let approval_id: String = row.get(0).map_err(|e| e.to_string())?;
    let action: String = row.get(1).map_err(|e| e.to_string())?;
    let resource_kind: Option<String> = row.get(2).map_err(|e| e.to_string())?;
    let resource_id: Option<String> = row.get(3).map_err(|e| e.to_string())?;
    let reason: String = row.get(4).map_err(|e| e.to_string())?;
    let requested_by: Option<String> = row.get(5).map_err(|e| e.to_string())?;
    let status: String = row.get(6).map_err(|e| e.to_string())?;
    let created_at: String = row.get(7).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(8).map_err(|e| e.to_string())?;
    let resolved_at: Option<String> = row.get(9).map_err(|e| e.to_string())?;
    let resolution: Option<String> = row.get(10).map_err(|e| e.to_string())?;
    let comment: Option<String> = row.get(11).map_err(|e| e.to_string())?;
    let integration_bindings_json: Option<String> = row.get(12).map_err(|e| e.to_string())?;

    Ok(kura_policy::Approval {
        approval_id,
        action,
        resource_kind: resource_kind.unwrap_or_default(),
        resource_id: resource_id.unwrap_or_default(),
        reason,
        requested_by: requested_by.unwrap_or_default(),
        status: parse_enum(&status)?,
        created_at: parse_rfc3339(&created_at)?,
        updated_at: parse_rfc3339(&updated_at)?,
        resolved_at: parse_opt_rfc3339(resolved_at)?,
        resolution: resolution.unwrap_or_default(),
        comment: comment.unwrap_or_default(),
        sandbox: None,
        integration_bindings: decode_vec(&integration_bindings_json)?,
    })
}

fn scan_decision(row: &Row) -> Result<kura_policy::Decision, String> {
    let decision_id: String = row.get(0).map_err(|e| e.to_string())?;
    let action: String = row.get(1).map_err(|e| e.to_string())?;
    let resource_kind: Option<String> = row.get(2).map_err(|e| e.to_string())?;
    let resource_id: Option<String> = row.get(3).map_err(|e| e.to_string())?;
    let outcome: String = row.get(4).map_err(|e| e.to_string())?;
    let reason: String = row.get(5).map_err(|e| e.to_string())?;
    let approval_id: Option<String> = row.get(6).map_err(|e| e.to_string())?;
    let created_at: String = row.get(7).map_err(|e| e.to_string())?;

    Ok(kura_policy::Decision {
        decision_id,
        action,
        resource_kind: resource_kind.unwrap_or_default(),
        resource_id: resource_id.unwrap_or_default(),
        outcome: parse_enum(&outcome)?,
        reason,
        approval_id: approval_id.unwrap_or_default(),
        created_at: parse_rfc3339(&created_at)?,
        sandbox: None,
    })
}

fn scan_schedule(row: &Row) -> Result<crate::schedule::ScheduleRecord, String> {
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

    Ok(crate::schedule::ScheduleRecord {
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

/// Reads the same columns as the sibling events scanner plus the tenant_id column
/// (index 18) and surfaces it on the event.
fn scan_event_with_tenant(row: &Row) -> Result<kura_events::Event, String> {
    let sequence: i64 = row.get(0).map_err(|e| e.to_string())?;
    let event_id: String = row.get(1).map_err(|e| e.to_string())?;
    let environment_scope: Option<String> = row.get(2).map_err(|e| e.to_string())?;
    let category: String = row.get(3).map_err(|e| e.to_string())?;
    let name: String = row.get(4).map_err(|e| e.to_string())?;
    let occurred_at: String = row.get(5).map_err(|e| e.to_string())?;
    let session_id: Option<String> = row.get(6).map_err(|e| e.to_string())?;
    let run_id: Option<String> = row.get(7).map_err(|e| e.to_string())?;
    let workflow_id: Option<String> = row.get(8).map_err(|e| e.to_string())?;
    let workflow_step_id: Option<String> = row.get(9).map_err(|e| e.to_string())?;
    let schedule_id: Option<String> = row.get(10).map_err(|e| e.to_string())?;
    let schedule_attempt_id: Option<String> = row.get(11).map_err(|e| e.to_string())?;
    let step_id: Option<String> = row.get(12).map_err(|e| e.to_string())?;
    let connector_id: Option<String> = row.get(13).map_err(|e| e.to_string())?;
    let capability_id: Option<String> = row.get(14).map_err(|e| e.to_string())?;
    let resource_kind: String = row.get(15).map_err(|e| e.to_string())?;
    let resource_id: String = row.get(16).map_err(|e| e.to_string())?;
    let payload_json: Option<String> = row.get(17).map_err(|e| e.to_string())?;
    let tenant_id: Option<String> = row.get(18).map_err(|e| e.to_string())?;

    Ok(kura_events::Event {
        event_id,
        sequence,
        environment_scope: environment_scope.unwrap_or_default(),
        tenant_id: tenant_id.unwrap_or_default(),
        category,
        name,
        occurred_at: parse_rfc3339(&occurred_at)?,
        scope: kura_events::Scope {
            session_id: session_id.unwrap_or_default(),
            run_id: run_id.unwrap_or_default(),
            workflow_id: workflow_id.unwrap_or_default(),
            workflow_step_id: workflow_step_id.unwrap_or_default(),
            schedule_id: schedule_id.unwrap_or_default(),
            schedule_attempt_id: schedule_attempt_id.unwrap_or_default(),
            step_id: step_id.unwrap_or_default(),
            connector_id: connector_id.unwrap_or_default(),
            capability_id: capability_id.unwrap_or_default(),
            ..kura_events::Scope::default()
        },
        resource: kura_events::Resource { kind: resource_kind, id: resource_id },
        payload: decode_map(&payload_json)?,
    })
}

// ---------------------------------------------------------------------------
// Store-internal event input/result (Go: sqliteAppendEventInput/Result)
// ---------------------------------------------------------------------------

/// Collects the columns the events INSERT needs, keeping append_event_for_tenant_safe
/// independent of the higher-level kura_events::Event type for testability.
#[derive(Debug, Clone, Default)]
pub struct AppendEventInput {
    pub event_id: String,
    pub environment_scope: String,
    pub category: String,
    pub name: String,
    pub occurred_at: DateTime<Utc>,
    pub session_id: String,
    pub run_id: String,
    pub workflow_id: String,
    pub workflow_step_id: String,
    pub schedule_id: String,
    pub schedule_attempt_id: String,
    pub step_id: String,
    pub connector_id: String,
    pub capability_id: String,
    pub resource_kind: String,
    pub resource_id: String,
    pub payload_json: String,
}

/// The result of append_event_for_tenant_safe: the assigned rowid sequence plus the
/// bound tenant id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppendEventResult {
    pub sequence: i64,
    pub tenant_id: String,
}

/// A JSON column that stays null when there is nothing to record.
///
/// Every dispatch predating these columns has neither, and a plain chat
/// request still has neither -- writing `[]` for all of them would make an
/// empty list indistinguishable from a row written before the column existed.
fn null_json<T: serde::Serialize>(values: &[T], label: &str) -> Result<Option<String>, String> {
    if values.is_empty() {
        return Ok(None);
    }
    serde_json::to_string(values)
        .map(Some)
        .map_err(|e| format!("marshal llm dispatch {label}: {e}"))
}
