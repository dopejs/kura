//! SQLite CRUD for the integration diagnostics domain: diagnostic runs,
//! diagnostic results, and retention records. Ported from
//! `daemon/internal/store/integration_diagnostics.go` (SaveIntegrationDiagnosticRun,
//! SaveIntegrationDiagnosticResult, LatestIntegrationDiagnosticResults,
//! ListIntegrationDiagnosticRuns, GetIntegrationDiagnosticRun,
//! SaveDiagnosticRetentionRecord, ExpiredDiagnosticRetentionRecords,
//! ApplyExpiredDiagnosticRetentionRecords).
//!
//! The record types, enums, and pure freshness/retention helpers live in
//! `kura-integrations` (diagnostics.rs / diagnostics_runtime.rs), matching the Go
//! layout where the integrations package owns them and the store only persists
//! them. The tenant-binding resolution (ResolveActiveTenantBinding) is not
//! ported: tenant_id is written as-is (empty string when unbound), so the
//! tenant-scoped queries behave like the Go unbound-context paths.
//!
//! Filter enums use the same convention as the other ported domains: a filter
//! whose enum field holds the default variant (Go's zero value) is skipped.

use chrono::{DateTime, Utc};
use rusqlite::{Row, params, params_from_iter, types::Value};

use crate::SQLiteStore;
use crate::crud::{enum_str, now_rfc3339, null_string, opt_time_string};

use kura_integrations::{
    DiagnosticReasonCode, DiagnosticResult, DiagnosticResultFilter, DiagnosticRetentionRecord,
    DiagnosticRetentionState, DiagnosticRun, DiagnosticRunFilter, DiagnosticRunStatus,
    DiagnosticStatus, refresh_diagnostic_result_freshness,
};

/// A chrono-defaulted timestamp (UNIX epoch) stands in for Go's zero `time.Time`.
fn is_unset_time(dt: &DateTime<Utc>) -> bool {
    dt.timestamp() == 0 && dt.timestamp_subsec_nanos() == 0
}

/// Go `normalizeDiagnosticLimit`: default 50, capped at 200.
fn normalize_diagnostic_limit(limit: i64) -> i64 {
    if limit <= 0 {
        50
    } else if limit > 200 {
        200
    } else {
        limit
    }
}

/// Go `scanDiagnosticDocuments`: rows carry only `document_json`, which is
/// authoritative.
fn scan_document<T: serde::de::DeserializeOwned>(row: &Row) -> Result<T, String> {
    let raw: String = row.get(0).map_err(|e| e.to_string())?;
    serde_json::from_str(&raw).map_err(|e| format!("decode integration diagnostic document: {e}"))
}

impl SQLiteStore {
    /// Go `SaveIntegrationDiagnosticRun`.
    pub fn save_integration_diagnostic_run(&self, item: &DiagnosticRun) -> Result<(), String> {
        let document = serde_json::to_string(item)
            .map_err(|e| format!("marshal integration diagnostic run: {e}"))?;

        self.conn
            .execute(
                r#"INSERT INTO integration_diagnostic_runs (
                    diagnostic_run_id, tenant_id, integration_id, integration_account_id,
                    domain_kind, provider_kind, requested_by, trigger, status, started_at,
                    completed_at, failure_reason_code, redaction_status, retention_expires_at,
                    idempotency_key, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                ON CONFLICT(diagnostic_run_id) DO UPDATE SET
                    tenant_id = COALESCE(integration_diagnostic_runs.tenant_id, excluded.tenant_id),
                    status = excluded.status,
                    completed_at = excluded.completed_at,
                    failure_reason_code = excluded.failure_reason_code,
                    redaction_status = excluded.redaction_status,
                    retention_expires_at = excluded.retention_expires_at,
                    document_json = excluded.document_json"#,
                params![
                    item.diagnostic_run_id,
                    item.tenant_id,
                    item.integration_id,
                    null_string(&item.integration_account_id),
                    null_string(&item.domain_kind),
                    null_string(&item.provider_kind),
                    item.requested_by,
                    item.trigger,
                    enum_str(&item.status),
                    now_rfc3339(&item.started_at),
                    opt_time_string(&item.completed_at),
                    null_string(&item.failure_reason_code),
                    enum_str(&item.redaction_status),
                    now_rfc3339(&item.retention_expires_at),
                    null_string(&item.idempotency_key),
                    document,
                ],
            )
            .map_err(|e| {
                format!(
                    "save integration diagnostic run {}: {e}",
                    item.diagnostic_run_id
                )
            })?;
        Ok(())
    }

    /// Go `SaveIntegrationDiagnosticResult`.
    pub fn save_integration_diagnostic_result(
        &self,
        item: &DiagnosticResult,
    ) -> Result<(), String> {
        let document = serde_json::to_string(item)
            .map_err(|e| format!("marshal integration diagnostic result: {e}"))?;

        self.conn
            .execute(
                r#"INSERT INTO integration_diagnostic_results (
                    diagnostic_result_id, tenant_id, integration_id, integration_account_id,
                    domain_kind, provider_kind, capability, status, reason_code,
                    remediation_owner, retry_safety, checked_at, stale_after, freshness_state,
                    run_id, redaction_status, retention_expires_at, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
                ON CONFLICT(diagnostic_result_id) DO UPDATE SET
                    tenant_id = COALESCE(integration_diagnostic_results.tenant_id, excluded.tenant_id),
                    status = excluded.status,
                    reason_code = excluded.reason_code,
                    remediation_owner = excluded.remediation_owner,
                    retry_safety = excluded.retry_safety,
                    checked_at = excluded.checked_at,
                    stale_after = excluded.stale_after,
                    freshness_state = excluded.freshness_state,
                    redaction_status = excluded.redaction_status,
                    retention_expires_at = excluded.retention_expires_at,
                    document_json = excluded.document_json"#,
                params![
                    item.diagnostic_result_id,
                    item.tenant_id,
                    item.integration_id,
                    null_string(&item.integration_account_id),
                    item.domain_kind,
                    item.provider_kind,
                    item.capability,
                    enum_str(&item.status),
                    enum_str(&item.reason_code),
                    enum_str(&item.remediation_owner),
                    enum_str(&item.retry_safety),
                    now_rfc3339(&item.checked_at),
                    now_rfc3339(&item.stale_after),
                    enum_str(&item.freshness_state),
                    null_string(&item.run_id),
                    enum_str(&item.redaction_status),
                    now_rfc3339(&item.retention_expires_at),
                    document,
                ],
            )
            .map_err(|e| format!("save integration diagnostic result {}: {e}", item.diagnostic_result_id))?;
        Ok(())
    }

    /// Go `LatestIntegrationDiagnosticResults` — newest unexpired results for the
    /// tenant, optionally filtered, with freshness refreshed against `now`.
    pub fn latest_integration_diagnostic_results(
        &self,
        filter: &DiagnosticResultFilter,
        now: DateTime<Utc>,
    ) -> Result<Vec<DiagnosticResult>, String> {
        let now = if is_unset_time(&now) { Utc::now() } else { now };
        let mut sql = String::from(
            "SELECT document_json FROM integration_diagnostic_results WHERE tenant_id = ?",
        );
        let mut args: Vec<Value> = vec![Value::Text(filter.tenant_id.trim().to_string())];
        if !filter.integration_id.trim().is_empty() {
            sql.push_str(" AND integration_id = ?");
            args.push(Value::Text(filter.integration_id.trim().to_string()));
        }
        if !filter.domain_kind.trim().is_empty() {
            sql.push_str(" AND domain_kind = ?");
            args.push(Value::Text(filter.domain_kind.trim().to_string()));
        }
        if !filter.provider_kind.trim().is_empty() {
            sql.push_str(" AND provider_kind = ?");
            args.push(Value::Text(filter.provider_kind.trim().to_string()));
        }
        if filter.status != DiagnosticStatus::default() {
            sql.push_str(" AND status = ?");
            args.push(Value::Text(enum_str(&filter.status)));
        }
        if filter.reason_code != DiagnosticReasonCode::default() {
            sql.push_str(" AND reason_code = ?");
            args.push(Value::Text(enum_str(&filter.reason_code)));
        }
        if !filter.include_expired {
            sql.push_str(" AND retention_expires_at > ?");
            args.push(Value::Text(now_rfc3339(&now)));
        }
        sql.push_str(" ORDER BY checked_at DESC, diagnostic_result_id DESC LIMIT ?");
        args.push(Value::Integer(normalize_diagnostic_limit(filter.limit)));

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("list integration diagnostic results: {e}"))?;
        let mut rows = stmt
            .query(params_from_iter(&args))
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let mut item: DiagnosticResult = scan_document(row)?;
            item = refresh_diagnostic_result_freshness(item, now);
            items.push(item);
        }
        Ok(items)
    }

    /// Go `ListIntegrationDiagnosticRuns` — newest runs for the tenant, optionally
    /// filtered by integration, domain, provider, status, and failure reason code.
    pub fn list_integration_diagnostic_runs(
        &self,
        filter: &DiagnosticRunFilter,
        now: DateTime<Utc>,
    ) -> Result<Vec<DiagnosticRun>, String> {
        let now = if is_unset_time(&now) { Utc::now() } else { now };
        let mut sql = String::from(
            "SELECT document_json FROM integration_diagnostic_runs WHERE tenant_id = ?",
        );
        let mut args: Vec<Value> = vec![Value::Text(filter.tenant_id.trim().to_string())];
        if !filter.integration_id.trim().is_empty() {
            sql.push_str(" AND integration_id = ?");
            args.push(Value::Text(filter.integration_id.trim().to_string()));
        }
        if !filter.domain_kind.trim().is_empty() {
            sql.push_str(" AND domain_kind = ?");
            args.push(Value::Text(filter.domain_kind.trim().to_string()));
        }
        if !filter.provider_kind.trim().is_empty() {
            sql.push_str(" AND provider_kind = ?");
            args.push(Value::Text(filter.provider_kind.trim().to_string()));
        }
        if filter.status != DiagnosticRunStatus::default() {
            sql.push_str(" AND status = ?");
            args.push(Value::Text(enum_str(&filter.status)));
        }
        if filter.reason_code != DiagnosticReasonCode::default() {
            sql.push_str(" AND failure_reason_code = ?");
            args.push(Value::Text(enum_str(&filter.reason_code)));
        }
        if !filter.include_expired {
            sql.push_str(" AND retention_expires_at > ?");
            args.push(Value::Text(now_rfc3339(&now)));
        }
        sql.push_str(" ORDER BY started_at DESC, diagnostic_run_id DESC LIMIT ?");
        args.push(Value::Integer(normalize_diagnostic_limit(filter.limit)));

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("list integration diagnostic runs: {e}"))?;
        let mut rows = stmt
            .query(params_from_iter(&args))
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_document(row)?);
        }
        Ok(items)
    }

    /// Go `GetIntegrationDiagnosticRun` — one run scoped to the tenant; expired
    /// runs are hidden unless `include_expired` is set.
    pub fn get_integration_diagnostic_run(
        &self,
        tenant_id: &str,
        run_id: &str,
        include_expired: bool,
        now: DateTime<Utc>,
    ) -> Result<Option<DiagnosticRun>, String> {
        let now = if is_unset_time(&now) { Utc::now() } else { now };
        let mut sql = String::from(
            "SELECT document_json FROM integration_diagnostic_runs WHERE tenant_id = ? AND diagnostic_run_id = ?",
        );
        let mut args: Vec<Value> = vec![
            Value::Text(tenant_id.trim().to_string()),
            Value::Text(run_id.trim().to_string()),
        ];
        if !include_expired {
            sql.push_str(" AND retention_expires_at > ?");
            args.push(Value::Text(now_rfc3339(&now)));
        }

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("get integration diagnostic run {run_id}: {e}"))?;
        let mut rows = stmt
            .query(params_from_iter(&args))
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_document(row).map(Some)
    }

    /// Go `SaveDiagnosticRetentionRecord`.
    pub fn save_diagnostic_retention_record(
        &self,
        record: &DiagnosticRetentionRecord,
    ) -> Result<(), String> {
        let document = serde_json::to_string(record)
            .map_err(|e| format!("marshal diagnostic retention record: {e}"))?;

        self.conn
            .execute(
                r#"INSERT INTO integration_diagnostic_retention (
                    retention_record_id, tenant_id, target_kind, target_id, policy_ref,
                    default_expires_at, effective_expires_at, retention_state, applied_at,
                    created_at, updated_at, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                ON CONFLICT(retention_record_id) DO UPDATE SET
                    effective_expires_at = excluded.effective_expires_at,
                    retention_state = excluded.retention_state,
                    applied_at = excluded.applied_at,
                    updated_at = excluded.updated_at,
                    document_json = excluded.document_json"#,
                params![
                    record.retention_record_id,
                    record.tenant_id,
                    record.target_kind,
                    record.target_id,
                    null_string(&record.policy_ref),
                    now_rfc3339(&record.default_expires_at),
                    now_rfc3339(&record.effective_expires_at),
                    record.retention_state.as_str(),
                    opt_time_string(&record.applied_at),
                    now_rfc3339(&record.created_at),
                    now_rfc3339(&record.updated_at),
                    document,
                ],
            )
            .map_err(|e| {
                format!(
                    "save diagnostic retention record {}: {e}",
                    record.retention_record_id
                )
            })?;
        Ok(())
    }

    /// Go `ExpiredDiagnosticRetentionRecords` — active records whose effective
    /// expiry has passed, most recently expired first.
    pub fn expired_diagnostic_retention_records(
        &self,
        tenant_id: &str,
        now: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<DiagnosticRetentionRecord>, String> {
        let now = if is_unset_time(&now) { Utc::now() } else { now };
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT document_json FROM integration_diagnostic_retention
                WHERE tenant_id = ?1 AND effective_expires_at <= ?2 AND retention_state = ?3
                ORDER BY effective_expires_at DESC, retention_record_id DESC
                LIMIT ?4"#,
            )
            .map_err(|e| format!("list expired diagnostic retention records: {e}"))?;
        let mut rows = stmt
            .query(params![
                tenant_id.trim(),
                now_rfc3339(&now),
                DiagnosticRetentionState::Active.as_str(),
                normalize_diagnostic_limit(limit),
            ])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_document(row)?);
        }
        Ok(items)
    }

    /// Go `ApplyExpiredDiagnosticRetentionRecords` — flips each expired active
    /// record to `Expired` (applied_at = now, updated_at = now) and re-saves it.
    pub fn apply_expired_diagnostic_retention_records(
        &self,
        tenant_id: &str,
        now: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<DiagnosticRetentionRecord>, String> {
        let now = if is_unset_time(&now) { Utc::now() } else { now };
        let mut items = self.expired_diagnostic_retention_records(tenant_id, now, limit)?;
        for item in &mut items {
            item.retention_state = DiagnosticRetentionState::Expired;
            item.applied_at = Some(now);
            item.updated_at = now;
            self.save_diagnostic_retention_record(item)?;
        }
        Ok(items)
    }
}
