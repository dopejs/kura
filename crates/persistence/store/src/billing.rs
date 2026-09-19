//! SQLite CRUD for the billing/quota plane plus the `kura_billing::Repository`
//! trait implementation. Ported from `daemon/internal/store/billing.go`
//! (ActivePlan, SavePlan, QuotaOverride, SaveQuotaOverride, OpenPeriod,
//! UsageCounter, PreviousQuotaPeriod, SaveUsageCounter, ReservationByOperation,
//! ReservationByID, SaveReservation, AppendUsageEvent, ListUsageEvidenceRefs,
//! AppendQuotaDenial, SaveManualAdjustment, ListPendingReservations,
//! ListQuotaDenials, QuotaDenialByID, SaveAbuseRestriction,
//! ListAbuseRestrictions, ListManualAdjustments).
//!
//! The transactional hooks on `kura_billing::Repository` (`reserve_usage`,
//! `resolve_usage`, `reserve_all_usage`) are left at their defaulted
//! `Ok(None)` implementations, so the billing manager runs its step-by-step
//! logic against the atomic DAOs below — the same behavior the Go daemon gets
//! for non-transactional repositories.
//!
//! Persistence matches the Go implementation: each row carries an explicit
//! column set for filtering/ordering plus (where applicable) a full
//! document_json snapshot, and reads decode only the explicit columns.

use chrono::{DateTime, Utc};
use rusqlite::{Row, params};

use crate::SQLiteStore;
use crate::crud::{now_rfc3339, null_string, opt_time_string, parse_opt_rfc3339, parse_rfc3339};

/// Result alias for the billing repository trait surface (kura_billing does not
/// re-export its error Result alias).
type BillingResult<T> = std::result::Result<T, kura_billing::BillingError>;

/// Go-style random id: `<prefix>_<16 hex chars>` (8 random bytes).
fn new_billing_id(prefix: &str) -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("{}_{}", prefix, &hex[..16])
}

fn is_go_zero_time(dt: &DateTime<Utc>) -> bool {
    dt == &kura_billing::go_zero_time()
}

fn billing_document_json(
    value: &Option<serde_json::Map<String, serde_json::Value>>,
) -> Result<Option<String>, String> {
    match value {
        None => Ok(None),
        Some(v) => serde_json::to_string(v)
            .map(Some)
            .map_err(|e| format!("marshal billing document: {e}")),
    }
}

fn decode_billing_document(
    raw: Option<String>,
) -> Result<Option<serde_json::Map<String, serde_json::Value>>, String> {
    match raw {
        None => Ok(None),
        Some(s) if s.trim().is_empty() || s.trim() == "{}" => Ok(None),
        Some(s) => serde_json::from_str(&s)
            .map(Some)
            .map_err(|e| format!("decode billing document: {e}")),
    }
}

fn scan_tenant_plan(row: &Row) -> Result<kura_billing::TenantPlan, String> {
    let plan_id: String = row.get(0).map_err(|e| e.to_string())?;
    let tenant_id: String = row.get(1).map_err(|e| e.to_string())?;
    let plan_key: String = row.get(2).map_err(|e| e.to_string())?;
    let status: String = row.get(3).map_err(|e| e.to_string())?;
    let enforcement_mode: String = row.get(4).map_err(|e| e.to_string())?;
    let effective_at: String = row.get(5).map_err(|e| e.to_string())?;
    let superseded_at: Option<String> = row.get(6).map_err(|e| e.to_string())?;
    let assigned_by: Option<String> = row.get(7).map_err(|e| e.to_string())?;
    let reason: Option<String> = row.get(8).map_err(|e| e.to_string())?;
    let document_json: Option<String> = row.get(9).map_err(|e| e.to_string())?;
    Ok(kura_billing::TenantPlan {
        plan_id,
        tenant_id,
        plan_key,
        status: kura_billing::PlanStatus::from(status),
        enforcement_mode: kura_billing::EnforcementMode::from(enforcement_mode),
        effective_at: parse_rfc3339(&effective_at)?,
        superseded_at: parse_opt_rfc3339(superseded_at)?,
        assigned_by_principal_id: assigned_by.unwrap_or_default(),
        assignment_reason: reason.unwrap_or_default(),
        document: decode_billing_document(document_json)?,
    })
}

fn scan_quota_override(row: &Row) -> Result<kura_billing::QuotaOverride, String> {
    let quota_override_id: String = row.get(0).map_err(|e| e.to_string())?;
    let tenant_id: String = row.get(1).map_err(|e| e.to_string())?;
    let category: String = row.get(2).map_err(|e| e.to_string())?;
    let limit_amount: Option<i64> = row.get(3).map_err(|e| e.to_string())?;
    let carryover_enabled: Option<i64> = row.get(4).map_err(|e| e.to_string())?;
    let carryover_max: Option<i64> = row.get(5).map_err(|e| e.to_string())?;
    let effective_at: String = row.get(6).map_err(|e| e.to_string())?;
    let expires_at: Option<String> = row.get(7).map_err(|e| e.to_string())?;
    let reason: String = row.get(8).map_err(|e| e.to_string())?;
    let created_by: Option<String> = row.get(9).map_err(|e| e.to_string())?;
    Ok(kura_billing::QuotaOverride {
        quota_override_id,
        tenant_id,
        category: kura_billing::Category::from(category),
        limit: limit_amount,
        carryover_enabled: carryover_enabled.map(|v| v != 0),
        carryover_max,
        effective_at: parse_rfc3339(&effective_at)?,
        expires_at: parse_opt_rfc3339(expires_at)?,
        reason,
        created_by_principal_id: created_by.unwrap_or_default(),
    })
}

fn scan_quota_period(row: &Row) -> Result<kura_billing::QuotaPeriod, String> {
    let quota_period_id: String = row.get(0).map_err(|e| e.to_string())?;
    let tenant_id: String = row.get(1).map_err(|e| e.to_string())?;
    let category: String = row.get(2).map_err(|e| e.to_string())?;
    let period_kind: String = row.get(3).map_err(|e| e.to_string())?;
    let period_start: String = row.get(4).map_err(|e| e.to_string())?;
    let period_end: String = row.get(5).map_err(|e| e.to_string())?;
    let carryover_from: Option<String> = row.get(6).map_err(|e| e.to_string())?;
    let status: String = row.get(7).map_err(|e| e.to_string())?;
    Ok(kura_billing::QuotaPeriod {
        quota_period_id,
        tenant_id,
        category: kura_billing::Category::from(category),
        period_kind: kura_billing::PeriodKind::from(period_kind),
        period_start: parse_rfc3339(&period_start)?,
        period_end: parse_rfc3339(&period_end)?,
        carryover_from_period_id: carryover_from.unwrap_or_default(),
        status,
    })
}

fn scan_usage_counter(row: &Row) -> Result<kura_billing::UsageCounter, String> {
    let usage_counter_id: String = row.get(0).map_err(|e| e.to_string())?;
    let tenant_id: String = row.get(1).map_err(|e| e.to_string())?;
    let category: String = row.get(2).map_err(|e| e.to_string())?;
    let quota_period_id: String = row.get(3).map_err(|e| e.to_string())?;
    let committed_amount: i64 = row.get(4).map_err(|e| e.to_string())?;
    let reserved_amount: i64 = row.get(5).map_err(|e| e.to_string())?;
    let adjusted_amount: i64 = row.get(6).map_err(|e| e.to_string())?;
    let carryover_amount: i64 = row.get(7).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(8).map_err(|e| e.to_string())?;
    Ok(kura_billing::UsageCounter {
        usage_counter_id,
        tenant_id,
        category: kura_billing::Category::from(category),
        quota_period_id,
        committed_amount,
        reserved_amount,
        adjusted_amount,
        carryover_amount,
        updated_at: parse_rfc3339(&updated_at)?,
    })
}

fn scan_usage_reservation(row: &Row) -> Result<kura_billing::UsageReservation, String> {
    let reservation_id: String = row.get(0).map_err(|e| e.to_string())?;
    let tenant_id: String = row.get(1).map_err(|e| e.to_string())?;
    let category: String = row.get(2).map_err(|e| e.to_string())?;
    let quota_period_id: String = row.get(3).map_err(|e| e.to_string())?;
    let operation_key: String = row.get(4).map_err(|e| e.to_string())?;
    let amount_reserved: i64 = row.get(5).map_err(|e| e.to_string())?;
    let amount_committed: i64 = row.get(6).map_err(|e| e.to_string())?;
    let amount_refunded: i64 = row.get(7).map_err(|e| e.to_string())?;
    let status: String = row.get(8).map_err(|e| e.to_string())?;
    let reservation_point: Option<String> = row.get(9).map_err(|e| e.to_string())?;
    let commit_point: Option<String> = row.get(10).map_err(|e| e.to_string())?;
    let refund_point: Option<String> = row.get(11).map_err(|e| e.to_string())?;
    let created_at: String = row.get(12).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(13).map_err(|e| e.to_string())?;
    let expires_at: Option<String> = row.get(14).map_err(|e| e.to_string())?;
    let recovery_reason: Option<String> = row.get(15).map_err(|e| e.to_string())?;
    Ok(kura_billing::UsageReservation {
        reservation_id,
        tenant_id,
        category: kura_billing::Category::from(category),
        quota_period_id,
        operation_key,
        amount_reserved,
        amount_committed,
        amount_refunded,
        status: kura_billing::ReservationStatus::from(status),
        reservation_point: reservation_point.unwrap_or_default(),
        commit_point: commit_point.unwrap_or_default(),
        refund_point: refund_point.unwrap_or_default(),
        created_at: parse_rfc3339(&created_at)?,
        updated_at: parse_rfc3339(&updated_at)?,
        expires_at: parse_opt_rfc3339(expires_at)?,
        recovery_reason: recovery_reason.unwrap_or_default(),
    })
}

fn scan_quota_denial(row: &Row) -> Result<kura_billing::QuotaDenial, String> {
    let denial_id: String = row.get(0).map_err(|e| e.to_string())?;
    let tenant_id: String = row.get(1).map_err(|e| e.to_string())?;
    let category: Option<String> = row.get(2).map_err(|e| e.to_string())?;
    let quota_period_id: Option<String> = row.get(3).map_err(|e| e.to_string())?;
    let operation_key: String = row.get(4).map_err(|e| e.to_string())?;
    let reason_code: String = row.get(5).map_err(|e| e.to_string())?;
    let requested_amount: i64 = row.get(6).map_err(|e| e.to_string())?;
    let remaining_amount: i64 = row.get(7).map_err(|e| e.to_string())?;
    let guarded_entry_point: String = row.get(8).map_err(|e| e.to_string())?;
    let created_at: String = row.get(9).map_err(|e| e.to_string())?;
    Ok(kura_billing::QuotaDenial {
        denial_id,
        tenant_id,
        category: kura_billing::Category::from(category.unwrap_or_default()),
        quota_period_id: quota_period_id.unwrap_or_default(),
        operation_key,
        reason_code,
        requested_amount,
        remaining_amount,
        guarded_entry_point,
        created_at: parse_rfc3339(&created_at)?,
    })
}

fn scan_manual_adjustment(row: &Row) -> Result<kura_billing::ManualAdjustment, String> {
    let adjustment_id: String = row.get(0).map_err(|e| e.to_string())?;
    let tenant_id: String = row.get(1).map_err(|e| e.to_string())?;
    let category: String = row.get(2).map_err(|e| e.to_string())?;
    let quota_period_id: String = row.get(3).map_err(|e| e.to_string())?;
    let amount_delta: i64 = row.get(4).map_err(|e| e.to_string())?;
    let reason: String = row.get(5).map_err(|e| e.to_string())?;
    let created_by: Option<String> = row.get(6).map_err(|e| e.to_string())?;
    let created_at: String = row.get(7).map_err(|e| e.to_string())?;
    Ok(kura_billing::ManualAdjustment {
        adjustment_id,
        tenant_id,
        category: kura_billing::Category::from(category),
        quota_period_id,
        amount_delta,
        reason,
        created_by_principal_id: created_by.unwrap_or_default(),
        created_at: parse_rfc3339(&created_at)?,
    })
}

fn scan_abuse_restriction(row: &Row) -> Result<kura_billing::AbuseRestrictionRecord, String> {
    let restriction_id: String = row.get(0).map_err(|e| e.to_string())?;
    let tenant_id: String = row.get(1).map_err(|e| e.to_string())?;
    let status: String = row.get(2).map_err(|e| e.to_string())?;
    let affected_category: String = row.get(3).map_err(|e| e.to_string())?;
    let recovery_action: String = row.get(4).map_err(|e| e.to_string())?;
    let visible_reason_code: String = row.get(5).map_err(|e| e.to_string())?;
    let source_audit_ref: Option<String> = row.get(6).map_err(|e| e.to_string())?;
    let support_contact_allowed: i64 = row.get(7).map_err(|e| e.to_string())?;
    let started_at: String = row.get(8).map_err(|e| e.to_string())?;
    let expires_at: Option<String> = row.get(9).map_err(|e| e.to_string())?;
    let document_json: Option<String> = row.get(10).map_err(|e| e.to_string())?;
    Ok(kura_billing::AbuseRestrictionRecord {
        restriction_id,
        tenant_id,
        status: kura_billing::AbuseRestrictionStatus::from(status),
        affected_category: kura_billing::Category::from(affected_category),
        recovery_action: kura_billing::RecoveryAction::from(recovery_action),
        visible_reason_code,
        source_audit_ref: source_audit_ref.unwrap_or_default(),
        support_contact_allowed: support_contact_allowed != 0,
        started_at: parse_rfc3339(&started_at)?,
        expires_at: parse_opt_rfc3339(expires_at)?,
        document: decode_billing_document(document_json)?,
    })
}

const TENANT_PLAN_COLUMNS: &str = "plan_id, tenant_id, plan_key, status, enforcement_mode, effective_at, superseded_at, assigned_by_principal_id, assignment_reason, document_json";
const QUOTA_OVERRIDE_COLUMNS: &str = "quota_override_id, tenant_id, category, limit_amount, carryover_enabled, carryover_max, effective_at, expires_at, reason, created_by_principal_id";
const QUOTA_PERIOD_COLUMNS: &str = "quota_period_id, tenant_id, category, period_kind, period_start, period_end, carryover_from_period_id, status";
const USAGE_COUNTER_COLUMNS: &str = "usage_counter_id, tenant_id, category, quota_period_id, committed_amount, reserved_amount, adjusted_amount, carryover_amount, updated_at";
const USAGE_RESERVATION_COLUMNS: &str = "reservation_id, tenant_id, category, quota_period_id, operation_key, amount_reserved, amount_committed, amount_refunded, status, reservation_point, commit_point, refund_point, created_at, updated_at, expires_at, recovery_reason";
const QUOTA_DENIAL_COLUMNS: &str = "denial_id, tenant_id, category, quota_period_id, operation_key, reason_code, requested_amount, remaining_amount, guarded_entry_point, created_at";
const MANUAL_ADJUSTMENT_COLUMNS: &str = "adjustment_id, tenant_id, category, quota_period_id, amount_delta, reason, created_by_principal_id, created_at";
const ABUSE_RESTRICTION_COLUMNS: &str = "restriction_id, tenant_id, status, affected_category, recovery_action, visible_reason_code, source_audit_ref, support_contact_allowed, started_at, expires_at, document_json";

impl SQLiteStore {
    pub fn billing_active_plan(
        &self,
        tenant_id: &str,
    ) -> Result<Option<kura_billing::TenantPlan>, String> {
        let at = Utc::now();
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {TENANT_PLAN_COLUMNS}
                 FROM billing_tenant_plans
                 WHERE tenant_id = ?1 AND status = ?2 AND effective_at <= ?3
                 ORDER BY effective_at DESC, plan_id DESC
                 LIMIT 1"
            ))
            .map_err(|e| format!("billing active plan: {e}"))?;
        let mut rows = stmt
            .query(params![
                tenant_id.trim(),
                kura_billing::PlanStatus::ACTIVE,
                now_rfc3339(&at),
            ])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_tenant_plan(row).map(Some)
    }

    pub fn billing_save_plan(&self, mut plan: kura_billing::TenantPlan) -> Result<(), String> {
        if plan.plan_id.is_empty() {
            plan.plan_id = new_billing_id("plan");
        }
        if plan.status.is_empty() {
            plan.status = kura_billing::PlanStatus::from(kura_billing::PlanStatus::ACTIVE);
        }
        if plan.enforcement_mode.is_empty() {
            plan.enforcement_mode =
                kura_billing::EnforcementMode::from(kura_billing::EnforcementMode::ENFORCED);
        }
        if is_go_zero_time(&plan.effective_at) {
            plan.effective_at = Utc::now();
        }
        let document_json = billing_document_json(&plan.document)?;
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin billing plan save: {e}"))?;
        if plan.status.as_str() == kura_billing::PlanStatus::ACTIVE {
            tx.execute(
                "UPDATE billing_tenant_plans
                 SET status = ?1, superseded_at = ?2
                 WHERE tenant_id = ?3 AND status = ?4 AND plan_id <> ?5",
                params![
                    kura_billing::PlanStatus::SUPERSEDED,
                    now_rfc3339(&plan.effective_at),
                    plan.tenant_id,
                    kura_billing::PlanStatus::ACTIVE,
                    plan.plan_id,
                ],
            )
            .map_err(|e| format!("supersede billing tenant plans: {e}"))?;
        }
        tx.execute(
            "INSERT INTO billing_tenant_plans (
                plan_id, tenant_id, plan_key, status, enforcement_mode, effective_at, superseded_at,
                assigned_by_principal_id, assignment_reason, document_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(plan_id) DO UPDATE SET
                tenant_id = excluded.tenant_id,
                plan_key = excluded.plan_key,
                status = excluded.status,
                enforcement_mode = excluded.enforcement_mode,
                effective_at = excluded.effective_at,
                superseded_at = excluded.superseded_at,
                assigned_by_principal_id = excluded.assigned_by_principal_id,
                assignment_reason = excluded.assignment_reason,
                document_json = excluded.document_json",
            params![
                plan.plan_id,
                plan.tenant_id,
                plan.plan_key,
                plan.status.as_str(),
                plan.enforcement_mode.as_str(),
                now_rfc3339(&plan.effective_at),
                opt_time_string(&plan.superseded_at),
                null_string(&plan.assigned_by_principal_id),
                null_string(&plan.assignment_reason),
                document_json,
            ],
        )
        .map_err(|e| format!("save billing tenant plan: {e}"))?;
        tx.commit()
            .map_err(|e| format!("commit billing plan save: {e}"))
    }

    pub fn billing_quota_override(
        &self,
        tenant_id: &str,
        category: &kura_billing::Category,
        at: &DateTime<Utc>,
    ) -> Result<Option<kura_billing::QuotaOverride>, String> {
        let at = if is_go_zero_time(at) { Utc::now() } else { *at };
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {QUOTA_OVERRIDE_COLUMNS}
                 FROM billing_quota_overrides
                 WHERE tenant_id = ?1 AND category = ?2 AND effective_at <= ?3 AND (expires_at IS NULL OR expires_at > ?4)
                 ORDER BY effective_at DESC, quota_override_id DESC
                 LIMIT 1"
            ))
            .map_err(|e| format!("billing quota override: {e}"))?;
        let mut rows = stmt
            .query(params![
                tenant_id.trim(),
                category.as_str(),
                now_rfc3339(&at),
                now_rfc3339(&at),
            ])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_quota_override(row).map(Some)
    }

    pub fn billing_save_quota_override(
        &self,
        mut override_: kura_billing::QuotaOverride,
    ) -> Result<(), String> {
        if override_.quota_override_id.is_empty() {
            override_.quota_override_id = new_billing_id("quota_override");
        }
        if is_go_zero_time(&override_.effective_at) {
            override_.effective_at = Utc::now();
        }
        self.conn
            .execute(
                "INSERT INTO billing_quota_overrides (
                    quota_override_id, tenant_id, category, limit_amount, carryover_enabled, carryover_max,
                    effective_at, expires_at, reason, created_by_principal_id
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(quota_override_id) DO UPDATE SET
                    tenant_id = excluded.tenant_id,
                    category = excluded.category,
                    limit_amount = excluded.limit_amount,
                    carryover_enabled = excluded.carryover_enabled,
                    carryover_max = excluded.carryover_max,
                    effective_at = excluded.effective_at,
                    expires_at = excluded.expires_at,
                    reason = excluded.reason,
                    created_by_principal_id = excluded.created_by_principal_id",
                params![
                    override_.quota_override_id,
                    override_.tenant_id,
                    override_.category.as_str(),
                    override_.limit,
                    override_.carryover_enabled.map(|v| if v { 1 } else { 0 }),
                    override_.carryover_max,
                    now_rfc3339(&override_.effective_at),
                    opt_time_string(&override_.expires_at),
                    override_.reason,
                    null_string(&override_.created_by_principal_id),
                ],
            )
            .map_err(|e| format!("save billing quota override: {e}"))?;
        Ok(())
    }

    pub fn billing_open_period(
        &self,
        tenant_id: &str,
        definition: &kura_billing::QuotaDefinition,
        at: &DateTime<Utc>,
    ) -> Result<kura_billing::QuotaPeriod, String> {
        let (start, end) = kura_billing::period_for(&definition.period_kind, *at);
        let period_id = format!(
            "quota_period_{}_{}_{}",
            tenant_id.trim(),
            definition.category.as_str(),
            start.format("%Y%m%d")
        );
        self.conn
            .execute(
                "INSERT INTO billing_quota_periods (
                    quota_period_id, tenant_id, category, period_kind, period_start, period_end,
                    carryover_from_period_id, status
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7)
                 ON CONFLICT(tenant_id, category, period_start) DO NOTHING",
                params![
                    period_id,
                    tenant_id.trim(),
                    definition.category.as_str(),
                    definition.period_kind.as_str(),
                    now_rfc3339(&start),
                    now_rfc3339(&end),
                    "open",
                ],
            )
            .map_err(|e| format!("open billing quota period: {e}"))?;
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {QUOTA_PERIOD_COLUMNS}
                 FROM billing_quota_periods
                 WHERE tenant_id = ?1 AND category = ?2 AND period_start = ?3"
            ))
            .map_err(|e| format!("open billing quota period: {e}"))?;
        let mut rows = stmt
            .query(params![
                tenant_id.trim(),
                definition.category.as_str(),
                now_rfc3339(&start),
            ])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Err("open billing quota period: no row".to_string());
        };
        scan_quota_period(row)
    }

    pub fn billing_usage_counter(
        &self,
        tenant_id: &str,
        category: &kura_billing::Category,
        quota_period_id: &str,
    ) -> Result<Option<kura_billing::UsageCounter>, String> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {USAGE_COUNTER_COLUMNS}
                 FROM billing_usage_counters
                 WHERE tenant_id = ?1 AND category = ?2 AND quota_period_id = ?3"
            ))
            .map_err(|e| format!("billing usage counter: {e}"))?;
        let mut rows = stmt
            .query(params![
                tenant_id.trim(),
                category.as_str(),
                quota_period_id.trim()
            ])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_usage_counter(row).map(Some)
    }

    pub fn billing_previous_quota_period(
        &self,
        tenant_id: &str,
        category: &kura_billing::Category,
        before: &DateTime<Utc>,
    ) -> Result<Option<(kura_billing::QuotaPeriod, kura_billing::UsageCounter)>, String> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {QUOTA_PERIOD_COLUMNS}
                 FROM billing_quota_periods
                 WHERE tenant_id = ?1 AND category = ?2 AND period_end = ?3 AND status = ?4
                 ORDER BY period_end DESC, quota_period_id DESC
                 LIMIT 1"
            ))
            .map_err(|e| format!("billing previous quota period: {e}"))?;
        let mut rows = stmt
            .query(params![
                tenant_id.trim(),
                category.as_str(),
                now_rfc3339(before),
                "closed",
            ])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let period = scan_quota_period(row)?;
        let counter =
            match self.billing_usage_counter(tenant_id, category, &period.quota_period_id)? {
                Some(counter) => counter,
                None => kura_billing::UsageCounter {
                    tenant_id: tenant_id.trim().to_string(),
                    category: category.clone(),
                    quota_period_id: period.quota_period_id.clone(),
                    updated_at: *before,
                    ..Default::default()
                },
            };
        Ok(Some((period, counter)))
    }

    pub fn billing_save_usage_counter(
        &self,
        mut counter: kura_billing::UsageCounter,
    ) -> Result<(), String> {
        if counter.usage_counter_id.is_empty() {
            counter.usage_counter_id = new_billing_id("usage_counter");
        }
        if is_go_zero_time(&counter.updated_at) {
            counter.updated_at = Utc::now();
        }
        self.conn
            .execute(
                "INSERT INTO billing_usage_counters (
                    usage_counter_id, tenant_id, category, quota_period_id, committed_amount, reserved_amount,
                    adjusted_amount, carryover_amount, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(tenant_id, category, quota_period_id) DO UPDATE SET
                    committed_amount = excluded.committed_amount,
                    reserved_amount = excluded.reserved_amount,
                    adjusted_amount = excluded.adjusted_amount,
                    carryover_amount = excluded.carryover_amount,
                    updated_at = excluded.updated_at",
                params![
                    counter.usage_counter_id,
                    counter.tenant_id,
                    counter.category.as_str(),
                    counter.quota_period_id,
                    counter.committed_amount,
                    counter.reserved_amount,
                    counter.adjusted_amount,
                    counter.carryover_amount,
                    now_rfc3339(&counter.updated_at),
                ],
            )
            .map_err(|e| format!("save billing usage counter: {e}"))?;
        Ok(())
    }

    pub fn billing_reservation_by_operation(
        &self,
        tenant_id: &str,
        category: &kura_billing::Category,
        operation_key: &str,
    ) -> Result<Option<kura_billing::UsageReservation>, String> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {USAGE_RESERVATION_COLUMNS}
                 FROM billing_usage_reservations
                 WHERE tenant_id = ?1 AND category = ?2 AND operation_key = ?3"
            ))
            .map_err(|e| format!("billing reservation by operation: {e}"))?;
        let mut rows = stmt
            .query(params![
                tenant_id.trim(),
                category.as_str(),
                operation_key.trim()
            ])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_usage_reservation(row).map(Some)
    }

    pub fn billing_reservation_by_id(
        &self,
        tenant_id: &str,
        reservation_id: &str,
    ) -> Result<Option<kura_billing::UsageReservation>, String> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {USAGE_RESERVATION_COLUMNS}
                 FROM billing_usage_reservations
                 WHERE tenant_id = ?1 AND reservation_id = ?2"
            ))
            .map_err(|e| format!("billing reservation by id: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id.trim(), reservation_id.trim()])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_usage_reservation(row).map(Some)
    }

    pub fn billing_save_reservation(
        &self,
        mut reservation: kura_billing::UsageReservation,
    ) -> Result<(), String> {
        if reservation.reservation_id.is_empty() {
            reservation.reservation_id = new_billing_id("reservation");
        }
        let now = Utc::now();
        if is_go_zero_time(&reservation.created_at) {
            reservation.created_at = now;
        }
        if is_go_zero_time(&reservation.updated_at) {
            reservation.updated_at = now;
        }
        self.conn
            .execute(
                "INSERT INTO billing_usage_reservations (
                    reservation_id, tenant_id, category, quota_period_id, operation_key, amount_reserved,
                    amount_committed, amount_refunded, status, reservation_point, commit_point, refund_point,
                    created_at, updated_at, expires_at, recovery_reason
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                 ON CONFLICT(tenant_id, category, operation_key) DO UPDATE SET
                    quota_period_id = excluded.quota_period_id,
                    amount_reserved = excluded.amount_reserved,
                    amount_committed = excluded.amount_committed,
                    amount_refunded = excluded.amount_refunded,
                    status = excluded.status,
                    reservation_point = excluded.reservation_point,
                    commit_point = excluded.commit_point,
                    refund_point = excluded.refund_point,
                    updated_at = excluded.updated_at,
                    expires_at = excluded.expires_at,
                    recovery_reason = excluded.recovery_reason",
                params![
                    reservation.reservation_id,
                    reservation.tenant_id,
                    reservation.category.as_str(),
                    reservation.quota_period_id,
                    reservation.operation_key,
                    reservation.amount_reserved,
                    reservation.amount_committed,
                    reservation.amount_refunded,
                    reservation.status.as_str(),
                    null_string(&reservation.reservation_point),
                    null_string(&reservation.commit_point),
                    null_string(&reservation.refund_point),
                    now_rfc3339(&reservation.created_at),
                    now_rfc3339(&reservation.updated_at),
                    opt_time_string(&reservation.expires_at),
                    null_string(&reservation.recovery_reason),
                ],
            )
            .map_err(|e| format!("save billing usage reservation: {e}"))?;
        Ok(())
    }

    pub fn billing_append_usage_event(
        &self,
        mut event: kura_billing::UsageEvent,
    ) -> Result<(), String> {
        if event.usage_event_id.is_empty() {
            event.usage_event_id = new_billing_id("usage_event");
        }
        if is_go_zero_time(&event.created_at) {
            event.created_at = Utc::now();
        }
        let document_json = billing_document_json(&event.document)?;
        self.conn
            .execute(
                "INSERT OR IGNORE INTO billing_usage_events (
                    usage_event_id, tenant_id, category, quota_period_id, operation_key, event_kind,
                    amount, reason_code, reason, actor_principal_id, outcome, created_at, document_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    event.usage_event_id,
                    event.tenant_id,
                    null_string(event.category.as_str()),
                    null_string(&event.quota_period_id),
                    null_string(&event.operation_key),
                    event.event_kind.as_str(),
                    event.amount,
                    event.reason_code,
                    null_string(&event.reason),
                    null_string(&event.actor_principal_id),
                    event.outcome,
                    now_rfc3339(&event.created_at),
                    document_json,
                ],
            )
            .map_err(|e| format!("append billing usage event: {e}"))?;
        Ok(())
    }

    pub fn billing_list_usage_evidence_refs(
        &self,
        tenant_id: &str,
        operation_key: &str,
        limit: usize,
    ) -> Result<Vec<String>, String> {
        let limit = if limit == 0 { 100 } else { limit as i64 };
        let mut stmt = self
            .conn
            .prepare(
                "SELECT usage_event_id
                 FROM billing_usage_events
                 WHERE tenant_id = ?1 AND operation_key = ?2
                 ORDER BY created_at ASC, usage_event_id ASC
                 LIMIT ?3",
            )
            .map_err(|e| format!("list billing usage evidence refs: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id.trim(), operation_key.trim(), limit])
            .map_err(|e| e.to_string())?;
        let mut refs = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let usage_event_id: String = row.get(0).map_err(|e| e.to_string())?;
            refs.push(format!("billing_usage_event:{usage_event_id}"));
        }
        Ok(refs)
    }

    pub fn billing_append_quota_denial(
        &self,
        mut denial: kura_billing::QuotaDenial,
    ) -> Result<(), String> {
        if denial.denial_id.is_empty() {
            denial.denial_id = new_billing_id("denial");
        }
        if is_go_zero_time(&denial.created_at) {
            denial.created_at = Utc::now();
        }
        self.conn
            .execute(
                "INSERT OR IGNORE INTO billing_quota_denials (
                    denial_id, tenant_id, category, quota_period_id, operation_key, reason_code,
                    requested_amount, remaining_amount, guarded_entry_point, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    denial.denial_id,
                    denial.tenant_id,
                    null_string(denial.category.as_str()),
                    null_string(&denial.quota_period_id),
                    denial.operation_key,
                    denial.reason_code,
                    denial.requested_amount,
                    denial.remaining_amount,
                    denial.guarded_entry_point,
                    now_rfc3339(&denial.created_at),
                ],
            )
            .map_err(|e| format!("append billing quota denial: {e}"))?;
        Ok(())
    }

    pub fn billing_save_manual_adjustment(
        &self,
        mut adjustment: kura_billing::ManualAdjustment,
    ) -> Result<(), String> {
        if adjustment.adjustment_id.is_empty() {
            adjustment.adjustment_id = new_billing_id("manual_adjustment");
        }
        if is_go_zero_time(&adjustment.created_at) {
            adjustment.created_at = Utc::now();
        }
        self.conn
            .execute(
                "INSERT INTO billing_manual_adjustments (
                    adjustment_id, tenant_id, category, quota_period_id, amount_delta,
                    reason, created_by_principal_id, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(adjustment_id) DO UPDATE SET
                    tenant_id = excluded.tenant_id,
                    category = excluded.category,
                    quota_period_id = excluded.quota_period_id,
                    amount_delta = excluded.amount_delta,
                    reason = excluded.reason,
                    created_by_principal_id = excluded.created_by_principal_id,
                    created_at = excluded.created_at",
                params![
                    adjustment.adjustment_id,
                    adjustment.tenant_id,
                    adjustment.category.as_str(),
                    adjustment.quota_period_id,
                    adjustment.amount_delta,
                    adjustment.reason,
                    null_string(&adjustment.created_by_principal_id),
                    now_rfc3339(&adjustment.created_at),
                ],
            )
            .map_err(|e| format!("save billing manual adjustment: {e}"))?;
        Ok(())
    }

    pub fn billing_list_pending_reservations(
        &self,
    ) -> Result<Vec<kura_billing::UsageReservation>, String> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {USAGE_RESERVATION_COLUMNS}
                 FROM billing_usage_reservations
                 WHERE status = ?1
                 ORDER BY updated_at ASC, reservation_id ASC"
            ))
            .map_err(|e| format!("list pending billing reservations: {e}"))?;
        let mut rows = stmt
            .query(params![kura_billing::ReservationStatus::RESERVED])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_usage_reservation(row)?);
        }
        Ok(items)
    }

    pub fn billing_list_quota_denials(
        &self,
        tenant_id: &str,
        limit: usize,
    ) -> Result<Vec<kura_billing::QuotaDenial>, String> {
        let limit = if limit == 0 { 100 } else { limit as i64 };
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {QUOTA_DENIAL_COLUMNS}
                 FROM billing_quota_denials
                 WHERE tenant_id = ?1
                 ORDER BY created_at DESC, denial_id DESC
                 LIMIT ?2"
            ))
            .map_err(|e| format!("list billing quota denials: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id.trim(), limit])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_quota_denial(row)?);
        }
        Ok(items)
    }

    pub fn billing_quota_denial_by_id(
        &self,
        tenant_id: &str,
        denial_id: &str,
    ) -> Result<Option<kura_billing::QuotaDenial>, String> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {QUOTA_DENIAL_COLUMNS}
                 FROM billing_quota_denials
                 WHERE tenant_id = ?1 AND denial_id = ?2
                 LIMIT 1"
            ))
            .map_err(|e| format!("billing quota denial by id: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id.trim(), denial_id.trim()])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_quota_denial(row).map(Some)
    }

    pub fn billing_save_abuse_restriction(
        &self,
        mut record: kura_billing::AbuseRestrictionRecord,
    ) -> Result<(), String> {
        if record.restriction_id.is_empty() {
            record.restriction_id = new_billing_id("abuse_restriction");
        }
        if record.status.is_empty() {
            record.status = kura_billing::AbuseRestrictionStatus::from(
                kura_billing::AbuseRestrictionStatus::ACTIVE,
            );
        }
        if record.recovery_action.is_empty() {
            record.recovery_action =
                kura_billing::RecoveryAction::from(kura_billing::RecoveryAction::CONTACT_SUPPORT);
        }
        if is_go_zero_time(&record.started_at) {
            record.started_at = Utc::now();
        }
        // The schema column is NOT NULL; Go writes NULL for a nil document, so
        // default an empty object to stay within the constraint.
        let document_json = match billing_document_json(&record.document)? {
            Some(json) => json,
            None => "{}".to_string(),
        };
        self.conn
            .execute(
                "INSERT INTO billing_abuse_restrictions (
                    restriction_id, tenant_id, status, affected_category, recovery_action, visible_reason_code,
                    source_audit_ref, support_contact_allowed, started_at, expires_at, document_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(restriction_id) DO UPDATE SET
                    tenant_id = excluded.tenant_id,
                    status = excluded.status,
                    affected_category = excluded.affected_category,
                    recovery_action = excluded.recovery_action,
                    visible_reason_code = excluded.visible_reason_code,
                    source_audit_ref = excluded.source_audit_ref,
                    support_contact_allowed = excluded.support_contact_allowed,
                    started_at = excluded.started_at,
                    expires_at = excluded.expires_at,
                    document_json = excluded.document_json",
                params![
                    record.restriction_id,
                    record.tenant_id,
                    record.status.as_str(),
                    record.affected_category.as_str(),
                    record.recovery_action.as_str(),
                    record.visible_reason_code,
                    null_string(&record.source_audit_ref),
                    if record.support_contact_allowed { 1 } else { 0 },
                    now_rfc3339(&record.started_at),
                    opt_time_string(&record.expires_at),
                    document_json,
                ],
            )
            .map_err(|e| format!("save billing abuse restriction: {e}"))?;
        Ok(())
    }

    pub fn billing_list_abuse_restrictions(
        &self,
        tenant_id: &str,
        at: &DateTime<Utc>,
    ) -> Result<Vec<kura_billing::AbuseRestrictionRecord>, String> {
        let at = if is_go_zero_time(at) { Utc::now() } else { *at };
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {ABUSE_RESTRICTION_COLUMNS}
                 FROM billing_abuse_restrictions
                 WHERE tenant_id = ?1 AND status = ?2 AND started_at <= ?3 AND (expires_at IS NULL OR expires_at > ?4)
                 ORDER BY started_at DESC, restriction_id DESC"
            ))
            .map_err(|e| format!("list billing abuse restrictions: {e}"))?;
        let mut rows = stmt
            .query(params![
                tenant_id.trim(),
                kura_billing::AbuseRestrictionStatus::ACTIVE,
                now_rfc3339(&at),
                now_rfc3339(&at),
            ])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_abuse_restriction(row)?);
        }
        Ok(items)
    }

    pub fn billing_list_manual_adjustments(
        &self,
        tenant_id: &str,
        limit: usize,
    ) -> Result<Vec<kura_billing::ManualAdjustment>, String> {
        let limit = if limit == 0 { 100 } else { limit as i64 };
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {MANUAL_ADJUSTMENT_COLUMNS}
                 FROM billing_manual_adjustments
                 WHERE tenant_id = ?1
                 ORDER BY created_at DESC, adjustment_id DESC
                 LIMIT ?2"
            ))
            .map_err(|e| format!("list billing manual adjustments: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id.trim(), limit])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_manual_adjustment(row)?);
        }
        Ok(items)
    }
}

// --- kura_billing::Repository trait impl (Send + Sync handle over the DAOs) ---
//
// rusqlite's Connection is Send but not Sync, so SQLiteStore cannot be the
// trait's `Send + Sync` self type directly. Following the kura_secrets::Store
// precedent (see secrets.rs), the mutex is wrapped in the local
// `BillingRepositoryHandle` newtype. Each method locks, runs the sync DAO, and
// maps failures to `BillingError::Repository`.

/// Send + Sync handle over the SQLite store implementing
/// `kura_billing::Repository`. Construct from a fresh store and share as
/// `Arc<BillingRepositoryHandle>` with the billing manager.
pub struct BillingRepositoryHandle(pub parking_lot::Mutex<SQLiteStore>);

impl BillingRepositoryHandle {
    pub fn new(store: SQLiteStore) -> Self {
        Self(parking_lot::Mutex::new(store))
    }
}

impl kura_billing::Repository for BillingRepositoryHandle {
    fn active_plan<'a>(
        &'a self,
        tenant_id: &str,
    ) -> kura_billing::BoxFuture<'a, BillingResult<Option<kura_billing::TenantPlan>>> {
        let tenant_id = tenant_id.to_string();
        Box::pin(async move {
            self.0
                .lock()
                .billing_active_plan(&tenant_id)
                .map_err(kura_billing::BillingError::Repository)
        })
    }

    fn quota_override<'a>(
        &'a self,
        tenant_id: &str,
        category: &kura_billing::Category,
        at: DateTime<Utc>,
    ) -> kura_billing::BoxFuture<'a, BillingResult<Option<kura_billing::QuotaOverride>>> {
        let tenant_id = tenant_id.to_string();
        let category = category.clone();
        Box::pin(async move {
            self.0
                .lock()
                .billing_quota_override(&tenant_id, &category, &at)
                .map_err(kura_billing::BillingError::Repository)
        })
    }

    fn open_period<'a>(
        &'a self,
        tenant_id: &str,
        definition: &kura_billing::QuotaDefinition,
        at: DateTime<Utc>,
    ) -> kura_billing::BoxFuture<'a, BillingResult<kura_billing::QuotaPeriod>> {
        let tenant_id = tenant_id.to_string();
        let definition = definition.clone();
        Box::pin(async move {
            self.0
                .lock()
                .billing_open_period(&tenant_id, &definition, &at)
                .map_err(kura_billing::BillingError::Repository)
        })
    }

    fn usage_counter<'a>(
        &'a self,
        tenant_id: &str,
        category: &kura_billing::Category,
        quota_period_id: &str,
    ) -> kura_billing::BoxFuture<'a, BillingResult<Option<kura_billing::UsageCounter>>> {
        let tenant_id = tenant_id.to_string();
        let category = category.clone();
        let quota_period_id = quota_period_id.to_string();
        Box::pin(async move {
            self.0
                .lock()
                .billing_usage_counter(&tenant_id, &category, &quota_period_id)
                .map_err(kura_billing::BillingError::Repository)
        })
    }

    fn save_usage_counter<'a>(
        &'a self,
        counter: kura_billing::UsageCounter,
    ) -> kura_billing::BoxFuture<'a, BillingResult<()>> {
        Box::pin(async move {
            self.0
                .lock()
                .billing_save_usage_counter(counter)
                .map_err(kura_billing::BillingError::Repository)
        })
    }

    fn reservation_by_operation<'a>(
        &'a self,
        tenant_id: &str,
        category: &kura_billing::Category,
        operation_key: &str,
    ) -> kura_billing::BoxFuture<'a, BillingResult<Option<kura_billing::UsageReservation>>> {
        let tenant_id = tenant_id.to_string();
        let category = category.clone();
        let operation_key = operation_key.to_string();
        Box::pin(async move {
            self.0
                .lock()
                .billing_reservation_by_operation(&tenant_id, &category, &operation_key)
                .map_err(kura_billing::BillingError::Repository)
        })
    }

    fn save_reservation<'a>(
        &'a self,
        reservation: kura_billing::UsageReservation,
    ) -> kura_billing::BoxFuture<'a, BillingResult<()>> {
        Box::pin(async move {
            self.0
                .lock()
                .billing_save_reservation(reservation)
                .map_err(kura_billing::BillingError::Repository)
        })
    }

    fn append_usage_event<'a>(
        &'a self,
        event: kura_billing::UsageEvent,
    ) -> kura_billing::BoxFuture<'a, BillingResult<()>> {
        Box::pin(async move {
            self.0
                .lock()
                .billing_append_usage_event(event)
                .map_err(kura_billing::BillingError::Repository)
        })
    }

    fn append_quota_denial<'a>(
        &'a self,
        denial: kura_billing::QuotaDenial,
    ) -> kura_billing::BoxFuture<'a, BillingResult<()>> {
        Box::pin(async move {
            self.0
                .lock()
                .billing_append_quota_denial(denial)
                .map_err(kura_billing::BillingError::Repository)
        })
    }

    fn list_pending_reservations<'a>(
        &'a self,
    ) -> kura_billing::BoxFuture<'a, BillingResult<Vec<kura_billing::UsageReservation>>> {
        Box::pin(async move {
            self.0
                .lock()
                .billing_list_pending_reservations()
                .map_err(kura_billing::BillingError::Repository)
        })
    }

    fn save_plan<'a>(
        &'a self,
        plan: kura_billing::TenantPlan,
    ) -> kura_billing::BoxFuture<'a, BillingResult<()>> {
        Box::pin(async move {
            self.0
                .lock()
                .billing_save_plan(plan)
                .map_err(kura_billing::BillingError::Repository)
        })
    }

    fn save_quota_override<'a>(
        &'a self,
        override_: kura_billing::QuotaOverride,
    ) -> kura_billing::BoxFuture<'a, BillingResult<()>> {
        Box::pin(async move {
            self.0
                .lock()
                .billing_save_quota_override(override_)
                .map_err(kura_billing::BillingError::Repository)
        })
    }

    fn save_manual_adjustment<'a>(
        &'a self,
        adjustment: kura_billing::ManualAdjustment,
    ) -> kura_billing::BoxFuture<'a, BillingResult<()>> {
        Box::pin(async move {
            self.0
                .lock()
                .billing_save_manual_adjustment(adjustment)
                .map_err(kura_billing::BillingError::Repository)
        })
    }

    fn reservation_by_id<'a>(
        &'a self,
        tenant_id: &str,
        reservation_id: &str,
    ) -> kura_billing::BoxFuture<'a, BillingResult<Option<kura_billing::UsageReservation>>> {
        let tenant_id = tenant_id.to_string();
        let reservation_id = reservation_id.to_string();
        Box::pin(async move {
            self.0
                .lock()
                .billing_reservation_by_id(&tenant_id, &reservation_id)
                .map_err(kura_billing::BillingError::Repository)
        })
    }

    fn list_quota_denials<'a>(
        &'a self,
        tenant_id: &str,
        limit: usize,
    ) -> kura_billing::BoxFuture<'a, BillingResult<Vec<kura_billing::QuotaDenial>>> {
        let tenant_id = tenant_id.to_string();
        Box::pin(async move {
            self.0
                .lock()
                .billing_list_quota_denials(&tenant_id, limit)
                .map_err(kura_billing::BillingError::Repository)
        })
    }

    fn list_manual_adjustments<'a>(
        &'a self,
        tenant_id: &str,
        limit: usize,
    ) -> kura_billing::BoxFuture<'a, BillingResult<Vec<kura_billing::ManualAdjustment>>> {
        let tenant_id = tenant_id.to_string();
        Box::pin(async move {
            self.0
                .lock()
                .billing_list_manual_adjustments(&tenant_id, limit)
                .map_err(kura_billing::BillingError::Repository)
        })
    }

    fn previous_quota_period<'a>(
        &'a self,
        tenant_id: &str,
        category: &kura_billing::Category,
        before: DateTime<Utc>,
    ) -> kura_billing::BoxFuture<
        'a,
        BillingResult<Option<(kura_billing::QuotaPeriod, kura_billing::UsageCounter)>>,
    > {
        let tenant_id = tenant_id.to_string();
        let category = category.clone();
        Box::pin(async move {
            self.0
                .lock()
                .billing_previous_quota_period(&tenant_id, &category, &before)
                .map_err(kura_billing::BillingError::Repository)
        })
    }

    fn list_abuse_restrictions<'a>(
        &'a self,
        tenant_id: &str,
        at: DateTime<Utc>,
    ) -> kura_billing::BoxFuture<'a, BillingResult<Vec<kura_billing::AbuseRestrictionRecord>>> {
        let tenant_id = tenant_id.to_string();
        Box::pin(async move {
            self.0
                .lock()
                .billing_list_abuse_restrictions(&tenant_id, &at)
                .map_err(kura_billing::BillingError::Repository)
        })
    }

    fn quota_denial_by_id<'a>(
        &'a self,
        tenant_id: &str,
        denial_id: &str,
    ) -> kura_billing::BoxFuture<'a, BillingResult<Option<kura_billing::QuotaDenial>>> {
        let tenant_id = tenant_id.to_string();
        let denial_id = denial_id.to_string();
        Box::pin(async move {
            self.0
                .lock()
                .billing_quota_denial_by_id(&tenant_id, &denial_id)
                .map_err(kura_billing::BillingError::Repository)
        })
    }

    fn list_usage_evidence_refs<'a>(
        &'a self,
        tenant_id: &str,
        operation_key: &str,
        limit: usize,
    ) -> kura_billing::BoxFuture<'a, BillingResult<Vec<String>>> {
        let tenant_id = tenant_id.to_string();
        let operation_key = operation_key.to_string();
        Box::pin(async move {
            self.0
                .lock()
                .billing_list_usage_evidence_refs(&tenant_id, &operation_key, limit)
                .map_err(kura_billing::BillingError::Repository)
        })
    }
}
