//! Quota projections: effective limits, dashboards, near-limit classification,
//! and period arithmetic (port of `projection.go`).

use chrono::DateTime;
use chrono::Datelike;
use chrono::Duration;
use chrono::NaiveDate;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Map;
use serde_json::Value;

use crate::catalog::PERIOD_ANCHOR_UTC;
use crate::catalog::initial_definitions;
use crate::error::Result;
use crate::manager::Manager;
use crate::manager::development_plan;
use crate::types::AbuseRestrictionStatus;
use crate::types::AbuseRestrictionSummary;
use crate::types::Category;
use crate::types::EnforcementMode;
use crate::types::ManualAdjustment;
use crate::types::NearLimitReason;
use crate::types::PeriodKind;
use crate::types::PlanSummary;
use crate::types::QuotaDefinition;
use crate::types::QuotaDenial;
use crate::types::QuotaOverride;
use crate::types::QuotaOverrideSummary;
use crate::types::QuotaPeriod;
use crate::types::QuotaSection;
use crate::types::QuotaStatus;
use crate::types::QuotaStatusItem;
use crate::types::RecoveryAction;
use crate::types::TenantPlan;
use crate::types::TenantQuotaDashboard;
use crate::types::Unit;
use crate::types::UsageCounter;
use crate::types::UsagePeriodSummary;
use crate::types::go_zero_time;
use crate::types::is_false;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveQuota {
    pub tenant_id: String,
    pub plan_key: String,
    pub category: Category,
    pub unit: Unit,
    #[serde(default = "go_zero_time")]
    pub period_start: DateTime<Utc>,
    #[serde(default = "go_zero_time")]
    pub period_end: DateTime<Utc>,
    pub period_anchor: String,
    pub limit: i64,
    pub consumed_amount: i64,
    pub reserved_amount: i64,
    pub adjusted_amount: i64,
    pub carryover_applied: i64,
    pub remaining_amount: i64,
    pub enforcement_mode: EnforcementMode,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub denial_reason_code: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub over_limit: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    pub tenant_id: String,
    pub plan_key: String,
    pub enforcement_mode: EnforcementMode,
    pub quotas: Vec<EffectiveQuota>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub manual_adjustments: Vec<ManualAdjustment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub denials: Vec<QuotaDenial>,
}

impl Manager {
    /// Active plan for a tenant; hosted tenants fail closed when no plan is
    /// persisted, non-hosted tenants fall back to the unlimited development
    /// plan.
    pub async fn active_plan(&self, tenant_id: &str, hosted: bool) -> Result<TenantPlan> {
        let now = self.clock_now();
        let Some(repo) = self.repo() else {
            if hosted {
                return Err(crate::error::BillingError::QuotaStateUnavailable);
            }
            return Ok(development_plan(tenant_id, now));
        };
        if let Some(plan) = repo.active_plan(tenant_id).await? {
            return Ok(plan);
        }
        if hosted {
            return Err(crate::error::BillingError::QuotaStateUnavailable);
        }
        Ok(development_plan(tenant_id, now))
    }

    /// Effective quota projection for every catalog category.
    pub async fn usage_summary(&self, tenant_id: &str, hosted: bool) -> Result<UsageSummary> {
        let plan = self.active_plan(tenant_id, hosted).await?;
        let now = self.clock_now();
        let repo = self.repo();
        let mut summary = UsageSummary {
            tenant_id: tenant_id.to_string(),
            plan_key: plan.plan_key.clone(),
            enforcement_mode: plan.enforcement_mode.clone(),
            ..Default::default()
        };
        for definition in initial_definitions(now) {
            let mut period = synthetic_period(tenant_id, &definition, now);
            let mut counter = UsageCounter {
                tenant_id: tenant_id.to_string(),
                category: definition.category.clone(),
                quota_period_id: period.quota_period_id.clone(),
                updated_at: now,
                ..Default::default()
            };
            let mut override_ = None;
            if let Some(repo) = repo.as_ref() {
                period = repo.open_period(tenant_id, &definition, now).await?;
                if let Some(found) = repo
                    .usage_counter(tenant_id, &definition.category, &period.quota_period_id)
                    .await?
                {
                    counter = found;
                } else {
                    counter.quota_period_id = period.quota_period_id.clone();
                }
                override_ = repo
                    .quota_override(tenant_id, &definition.category, now)
                    .await?;
            }
            summary.quotas.push(project_quota(
                &plan,
                &definition,
                &period,
                &counter,
                override_.as_ref(),
            ));
        }
        if let Some(repo) = repo {
            summary.denials = repo.list_quota_denials(tenant_id, 100).await?;
            summary.manual_adjustments = repo.list_manual_adjustments(tenant_id, 100).await?;
        }
        Ok(summary)
    }

    /// Full tenant quota dashboard with sections, overrides, restrictions,
    /// and the previous completed period.
    pub async fn quota_dashboard(
        &self,
        tenant_id: &str,
        hosted: bool,
    ) -> Result<TenantQuotaDashboard> {
        let plan = self.active_plan(tenant_id, hosted).await?;
        let now = self.clock_now();
        let repo = self.repo();
        let mut restrictions_by_category: std::collections::HashMap<
            Category,
            AbuseRestrictionSummary,
        > = std::collections::HashMap::new();
        if let Some(repo) = repo.as_ref() {
            for record in repo.list_abuse_restrictions(tenant_id, now).await? {
                restrictions_by_category.insert(record.affected_category.clone(), record.summary());
            }
        }
        let mut items = Vec::new();
        for definition in initial_definitions(now) {
            let mut period = synthetic_period(tenant_id, &definition, now);
            let mut counter = UsageCounter {
                tenant_id: tenant_id.to_string(),
                category: definition.category.clone(),
                quota_period_id: period.quota_period_id.clone(),
                updated_at: now,
                ..Default::default()
            };
            let mut previous = None;
            let mut override_ = None;
            if let Some(repo) = repo.as_ref() {
                period = repo.open_period(tenant_id, &definition, now).await?;
                if let Some(found) = repo
                    .usage_counter(tenant_id, &definition.category, &period.quota_period_id)
                    .await?
                {
                    counter = found;
                } else {
                    counter.quota_period_id = period.quota_period_id.clone();
                }
                if let Some((previous_period, previous_counter)) = repo
                    .previous_quota_period(tenant_id, &definition.category, period.period_start)
                    .await?
                {
                    let previous_quota = project_quota(
                        &plan,
                        &definition,
                        &previous_period,
                        &previous_counter,
                        None,
                    );
                    previous = Some(UsagePeriodSummary {
                        period_start: previous_quota.period_start,
                        period_end: previous_quota.period_end,
                        period_anchor: previous_quota.period_anchor,
                        consumed_amount: previous_quota.consumed_amount,
                        reserved_amount: previous_quota.reserved_amount,
                        adjusted_amount: previous_quota.adjusted_amount,
                        carryover_applied: previous_quota.carryover_applied,
                        remaining_amount: previous_quota.remaining_amount,
                        over_limit: previous_quota.over_limit,
                    });
                }
                override_ = repo
                    .quota_override(tenant_id, &definition.category, now)
                    .await?;
            }
            items.push(build_quota_status_item(
                &plan,
                &definition,
                &period,
                &counter,
                previous,
                override_.as_ref(),
                restrictions_by_category.get(&definition.category),
            ));
        }
        let plan_label = if plan.plan_key.is_empty() {
            plan.enforcement_mode.to_string()
        } else {
            plan.plan_key.clone()
        };
        Ok(TenantQuotaDashboard {
            tenant_id: tenant_id.to_string(),
            plan: PlanSummary {
                plan_key: plan.plan_key.clone(),
                enforcement_mode: plan.enforcement_mode.clone(),
                status: plan.status.clone(),
                effective_at: plan.effective_at,
                base_plan_label: plan_label,
                checkout_available: false,
            },
            sections: group_quota_status_items(items),
            generated_at: now,
            permission: Some(Map::from_iter([("allowed".to_string(), Value::from(true))])),
        })
    }
}

fn synthetic_period(
    tenant_id: &str,
    definition: &QuotaDefinition,
    now: DateTime<Utc>,
) -> QuotaPeriod {
    let (start, end) = period_for(&definition.period_kind, now);
    QuotaPeriod {
        quota_period_id: format!(
            "quota_period_{}_{}_{}",
            tenant_id,
            definition.category,
            start.format("%Y%m%d")
        ),
        tenant_id: tenant_id.to_string(),
        category: definition.category.clone(),
        period_kind: definition.period_kind.clone(),
        period_start: start,
        period_end: end,
        status: "open".to_string(),
        ..Default::default()
    }
}

/// Effective quota for one category: limit after override, usage after
/// carryover, and over-limit detection for enforced plans.
#[must_use]
pub fn project_quota(
    plan: &TenantPlan,
    definition: &QuotaDefinition,
    period: &QuotaPeriod,
    counter: &UsageCounter,
    override_: Option<&QuotaOverride>,
) -> EffectiveQuota {
    let limit = override_
        .and_then(|item| item.limit)
        .unwrap_or(definition.default_limit);
    let mode = if plan.enforcement_mode.is_empty() {
        EnforcementMode::from(EnforcementMode::ENFORCED)
    } else {
        plan.enforcement_mode.clone()
    };
    let effective_usage =
        counter.committed_amount + counter.reserved_amount + counter.adjusted_amount
            - counter.carryover_amount;
    let mut remaining = limit - effective_usage;
    if mode == EnforcementMode::UNLIMITED {
        remaining = 0;
    }
    let mut out = EffectiveQuota {
        tenant_id: plan.tenant_id.clone(),
        plan_key: plan.plan_key.clone(),
        category: definition.category.clone(),
        unit: definition.unit.clone(),
        period_start: period.period_start,
        period_end: period.period_end,
        period_anchor: PERIOD_ANCHOR_UTC.to_string(),
        limit,
        consumed_amount: counter.committed_amount,
        reserved_amount: counter.reserved_amount,
        adjusted_amount: counter.adjusted_amount,
        carryover_applied: counter.carryover_amount,
        remaining_amount: remaining,
        enforcement_mode: mode,
        ..Default::default()
    };
    if out.enforcement_mode == EnforcementMode::ENFORCED && remaining < 0 {
        out.over_limit = true;
        out.denial_reason_code = definition.denial_reason_code.clone();
    }
    out
}

/// Dashboard status item: combines effective quota, near-limit
/// classification, override summary, and active abuse restriction.
#[must_use]
pub fn build_quota_status_item(
    plan: &TenantPlan,
    definition: &QuotaDefinition,
    period: &QuotaPeriod,
    counter: &UsageCounter,
    previous: Option<UsagePeriodSummary>,
    override_: Option<&QuotaOverride>,
    restriction: Option<&AbuseRestrictionSummary>,
) -> QuotaStatusItem {
    let quota = project_quota(plan, definition, period, counter, override_);
    let base_limit = definition.default_limit;
    let effective_limit = quota.limit;
    let typical_operation_amount = category_defined_typical_operation_amount(definition);
    let current = UsagePeriodSummary {
        period_start: quota.period_start,
        period_end: quota.period_end,
        period_anchor: quota.period_anchor.clone(),
        consumed_amount: quota.consumed_amount,
        reserved_amount: quota.reserved_amount,
        adjusted_amount: quota.adjusted_amount,
        carryover_applied: quota.carryover_applied,
        remaining_amount: quota.remaining_amount,
        over_limit: quota.over_limit,
    };
    let mut item = QuotaStatusItem {
        category: definition.category.clone(),
        unit: definition.unit.clone(),
        status: QuotaStatus::from(QuotaStatus::AVAILABLE),
        current_period: current,
        previous_period: previous,
        limit: effective_limit,
        remaining_amount: quota.remaining_amount,
        typical_operation_amount,
        base_limit,
        effective_limit,
        ..Default::default()
    };
    if let Some(override_) = override_ {
        item.override_ = Some(QuotaOverrideSummary {
            base_limit,
            effective_limit,
            reason: override_.reason.clone(),
            effective_at: override_.effective_at,
            expires_at: override_.expires_at,
        });
    }
    let mode = if plan.enforcement_mode.is_empty() {
        EnforcementMode::from(EnforcementMode::ENFORCED)
    } else {
        plan.enforcement_mode.clone()
    };
    if let Some(restriction) =
        restriction.filter(|item| item.status == AbuseRestrictionStatus::ACTIVE)
    {
        item.status = QuotaStatus::from(QuotaStatus::RESTRICTED);
        item.restriction = Some(restriction.clone());
    } else if mode == EnforcementMode::UNLIMITED {
        item.status = QuotaStatus::from(QuotaStatus::UNLIMITED);
    } else if mode == EnforcementMode::NOT_MEASURABLE {
        item.status = QuotaStatus::from(QuotaStatus::NOT_MEASURABLE);
    } else if effective_limit <= 0 && definition.unit != Unit::BYTES {
        item.status = QuotaStatus::from(QuotaStatus::EXHAUSTED);
    } else if quota.remaining_amount <= 0 {
        item.status = QuotaStatus::from(QuotaStatus::EXHAUSTED);
    } else if is_quota_near_limit(&quota, typical_operation_amount) {
        item.status = QuotaStatus::from(QuotaStatus::NEAR_LIMIT);
        item.near_limit = true;
        item.near_limit_reason = near_limit_reason_for_quota(&quota, typical_operation_amount);
    }
    item.recovery_actions =
        recovery_actions_for_quota_status(&item.status, &item.near_limit_reason);
    item
}

/// Amount of usage a typical operation consumes, used for
/// below-one-operation near-limit detection.
#[must_use]
pub fn category_defined_typical_operation_amount(definition: &QuotaDefinition) -> i64 {
    if definition.unit == Unit::BYTES {
        let document = definition.document.as_ref();
        let estimate = int64_from_document(document, "artifactWriteReservationEstimateBytes");
        if estimate > 0 {
            return estimate;
        }
        let typical = int64_from_document(document, "typicalOperationAmount");
        if typical > 0 {
            return typical;
        }
        return 1;
    }
    1
}

fn int64_from_document(document: Option<&Map<String, Value>>, key: &str) -> i64 {
    match document.and_then(|document| document.get(key)) {
        Some(Value::Number(number)) => number
            .as_i64()
            .or_else(|| number.as_u64().and_then(|value| i64::try_from(value).ok()))
            .or_else(|| number.as_f64().map(|value| value as i64))
            .unwrap_or(0),
        _ => 0,
    }
}

#[must_use]
pub fn is_quota_near_limit(quota: &EffectiveQuota, typical_operation_amount: i64) -> bool {
    near_limit_reason_for_quota(quota, typical_operation_amount) != NearLimitReason::NONE
}

/// Near-limit classification: >=80% of the limit consumed, or less than one
/// typical operation remaining.
#[must_use]
pub fn near_limit_reason_for_quota(
    quota: &EffectiveQuota,
    typical_operation_amount: i64,
) -> NearLimitReason {
    if quota.enforcement_mode != EnforcementMode::ENFORCED
        || quota.limit <= 0
        || quota.remaining_amount <= 0
    {
        return NearLimitReason::from(NearLimitReason::NONE);
    }
    let used = quota.consumed_amount + quota.reserved_amount + quota.adjusted_amount
        - quota.carryover_applied;
    if used * 100 >= quota.limit * 80 {
        return NearLimitReason::from(NearLimitReason::PERCENT_THRESHOLD);
    }
    if typical_operation_amount > 0 && quota.remaining_amount < typical_operation_amount {
        return NearLimitReason::from(NearLimitReason::BELOW_ONE_TYPICAL_OPERATION);
    }
    NearLimitReason::from(NearLimitReason::NONE)
}

/// Recovery actions surfaced for a quota status.
#[must_use]
pub fn recovery_actions_for_quota_status(
    status: &QuotaStatus,
    reason: &NearLimitReason,
) -> Vec<RecoveryAction> {
    match status.as_str() {
        QuotaStatus::RESTRICTED => vec![RecoveryAction::from(RecoveryAction::CONTACT_SUPPORT)],
        QuotaStatus::UNAVAILABLE => vec![
            RecoveryAction::from(RecoveryAction::OPERATOR_RESOLUTION_REQUIRED),
            RecoveryAction::from(RecoveryAction::RETRY_LATER),
        ],
        QuotaStatus::EXHAUSTED => vec![
            RecoveryAction::from(RecoveryAction::WAIT),
            RecoveryAction::from(RecoveryAction::REDUCE_SCOPE),
            RecoveryAction::from(RecoveryAction::REQUEST_OVERRIDE),
        ],
        QuotaStatus::NEAR_LIMIT => {
            if *reason == NearLimitReason::BELOW_ONE_TYPICAL_OPERATION {
                vec![
                    RecoveryAction::from(RecoveryAction::REDUCE_SCOPE),
                    RecoveryAction::from(RecoveryAction::WAIT),
                ]
            } else {
                vec![
                    RecoveryAction::from(RecoveryAction::WAIT),
                    RecoveryAction::from(RecoveryAction::REDUCE_SCOPE),
                ]
            }
        }
        _ => Vec::new(),
    }
}

/// Groups status items into dashboard sections in a stable order.
#[must_use]
pub fn group_quota_status_items(items: Vec<QuotaStatusItem>) -> Vec<QuotaSection> {
    let mut grouped: std::collections::HashMap<&'static str, QuotaSection> =
        std::collections::HashMap::new();
    let order = [
        "launches",
        "runtime",
        "integrations",
        "storage",
        "evaluations",
    ];
    for item in items {
        let (key, label) = quota_section_for_category(&item.category);
        grouped
            .entry(key)
            .or_insert_with(|| QuotaSection {
                section_key: key.to_string(),
                label: label.to_string(),
                items: Vec::new(),
            })
            .items
            .push(item);
    }
    let mut sections = Vec::with_capacity(grouped.len());
    for key in order {
        if let Some(section) = grouped.remove(key) {
            sections.push(section);
        }
    }
    let mut extra: Vec<&'static str> = grouped.keys().copied().collect();
    extra.sort_unstable();
    for key in extra {
        if let Some(section) = grouped.remove(key) {
            sections.push(section);
        }
    }
    sections
}

fn quota_section_for_category(category: &Category) -> (&'static str, &'static str) {
    match category.as_str() {
        Category::RUN_LAUNCHES | Category::WORKFLOW_LAUNCHES => ("launches", "Launches"),
        Category::RUNTIME_TOOL_CALLS | Category::LIVE_VALIDATION_ATTEMPTS => ("runtime", "Runtime"),
        Category::INTEGRATION_OPERATIONS => ("integrations", "Integrations"),
        Category::ARTIFACT_STORAGE_BYTES => ("storage", "Artifact Storage"),
        Category::REPLAY_EVALUATION_ATTEMPTS => ("evaluations", "Evaluations"),
        _ => ("other", "Other"),
    }
}

/// UTC period boundaries for a period kind containing `now`.
#[must_use]
pub fn period_for(kind: &PeriodKind, now: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
    let now = now;
    match kind.as_str() {
        PeriodKind::DAILY => {
            let date = now.date_naive();
            let start = utc_midnight(date);
            (start, start + Duration::days(1))
        }
        PeriodKind::MONTHLY => {
            let date = now.date_naive();
            let start = utc_midnight(date.with_day(1).unwrap_or(date));
            let (year, month) = (start.year(), start.month());
            let (next_year, next_month) = if month == 12 {
                (year + 1, 1)
            } else {
                (year, month + 1)
            };
            let end = NaiveDate::from_ymd_opt(next_year, next_month, 1)
                .map(utc_midnight)
                .unwrap_or(start);
            (start, end)
        }
        _ => (
            utc_ymd_hms(1970, 1, 1, 0, 0, 0),
            utc_ymd_hms(9999, 12, 31, 23, 59, 59),
        ),
    }
}

pub(crate) fn utc_midnight(date: NaiveDate) -> DateTime<Utc> {
    DateTime::from_naive_utc_and_offset(
        date.and_hms_opt(0, 0, 0)
            .unwrap_or_else(|| DateTime::UNIX_EPOCH.naive_utc()),
        Utc,
    )
}

pub(crate) fn utc_ymd_hms(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
) -> DateTime<Utc> {
    DateTime::from_naive_utc_and_offset(
        NaiveDate::from_ymd_opt(year, month, day)
            .and_then(|date| date.and_hms_opt(hour, minute, second))
            .unwrap_or_else(|| DateTime::UNIX_EPOCH.naive_utc()),
        Utc,
    )
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use serde_json::json;

    use super::*;
    use crate::catalog::definition_for;
    use crate::catalog::required_categories;
    use crate::fixtures::TEN_FINITE;
    use crate::manager::BoxFuture;
    use crate::manager::Repository;
    use crate::types::PlanStatus;
    use crate::types::UsageEvent;
    use crate::types::UsageReservation;

    fn finite_plan() -> TenantPlan {
        TenantPlan {
            tenant_id: TEN_FINITE.to_string(),
            plan_key: "finite".to_string(),
            enforcement_mode: EnforcementMode::from(EnforcementMode::ENFORCED),
            ..Default::default()
        }
    }

    fn current_period(definition: &QuotaDefinition, now: DateTime<Utc>) -> QuotaPeriod {
        let (start, end) = period_for(&definition.period_kind, now);
        QuotaPeriod {
            tenant_id: TEN_FINITE.to_string(),
            category: definition.category.clone(),
            period_start: start,
            period_end: end,
            ..Default::default()
        }
    }

    #[test]
    fn period_for_uses_utc_boundaries() {
        let now = utc_ymd_hms(2026, 4, 28, 23, 30, 0);
        let (start, end) = period_for(&PeriodKind::from(PeriodKind::DAILY), now);
        assert_eq!(start, utc_ymd_hms(2026, 4, 28, 0, 0, 0));
        assert_eq!(end - start, Duration::hours(24));
    }

    #[test]
    fn project_quota_shows_unlimited_and_over_limit_states() {
        let now = utc_ymd_hms(2026, 4, 28, 12, 0, 0);
        let definition = definition_for(&Category::from(Category::RUN_LAUNCHES)).unwrap();
        let period = current_period(&definition, now);
        let counter = UsageCounter {
            tenant_id: TEN_FINITE.to_string(),
            category: definition.category.clone(),
            committed_amount: 11,
            ..Default::default()
        };
        let override_ = QuotaOverride {
            tenant_id: TEN_FINITE.to_string(),
            category: definition.category.clone(),
            limit: Some(10),
            ..Default::default()
        };
        let quota = project_quota(
            &finite_plan(),
            &definition,
            &period,
            &counter,
            Some(&override_),
        );
        assert!(
            quota.over_limit,
            "expected over-limit finite projection: {quota:?}"
        );
        assert_eq!(quota.remaining_amount, -1);
        assert!(!quota.denial_reason_code.is_empty());

        let unlimited = project_quota(
            &development_plan("ten_dev", now),
            &definition,
            &period,
            &counter,
            None,
        );
        assert_eq!(unlimited.enforcement_mode, EnforcementMode::UNLIMITED);
        assert!(!unlimited.over_limit);
    }

    #[test]
    fn project_quota_accounts_for_carryover() {
        let now = utc_ymd_hms(2026, 4, 28, 12, 0, 0);
        let definition = definition_for(&Category::from(Category::RUN_LAUNCHES)).unwrap();
        let period = current_period(&definition, now);
        let counter = UsageCounter {
            tenant_id: TEN_FINITE.to_string(),
            category: definition.category.clone(),
            committed_amount: 5,
            reserved_amount: 2,
            adjusted_amount: -1,
            carryover_amount: 3,
            ..Default::default()
        };
        let override_ = QuotaOverride {
            tenant_id: TEN_FINITE.to_string(),
            category: definition.category.clone(),
            limit: Some(10),
            ..Default::default()
        };
        let quota = project_quota(
            &finite_plan(),
            &definition,
            &period,
            &counter,
            Some(&override_),
        );
        assert_eq!(quota.carryover_applied, 3);
        assert_eq!(quota.remaining_amount, 7);
    }

    #[test]
    fn build_quota_status_item_classifies_near_limit_for_percent_and_typical_operation() {
        let now = utc_ymd_hms(2026, 5, 7, 10, 0, 0);
        let definition = definition_for(&Category::from(Category::RUN_LAUNCHES)).unwrap();
        let period = current_period(&definition, now);
        let override_ = QuotaOverride {
            tenant_id: TEN_FINITE.to_string(),
            category: definition.category.clone(),
            limit: Some(10),
            ..Default::default()
        };
        let item = build_quota_status_item(
            &finite_plan(),
            &definition,
            &period,
            &UsageCounter {
                tenant_id: TEN_FINITE.to_string(),
                category: definition.category.clone(),
                committed_amount: 8,
                ..Default::default()
            },
            None,
            Some(&override_),
            None,
        );
        assert_eq!(
            item.status,
            QuotaStatus::NEAR_LIMIT,
            "expected percent near-limit: {item:?}"
        );
        assert!(item.near_limit);
        assert_eq!(item.near_limit_reason, NearLimitReason::PERCENT_THRESHOLD);
        assert_eq!(item.typical_operation_amount, 1);

        let byte_definition = QuotaDefinition {
            category: Category::from(Category::ARTIFACT_STORAGE_BYTES),
            unit: Unit::from(Unit::BYTES),
            period_kind: PeriodKind::from(PeriodKind::MONTHLY),
            default_limit: 10_000,
            denial_reason_code: "quota_denied:artifact_storage_bytes_exhausted".to_string(),
            document: Some(Map::from_iter([(
                "artifactWriteReservationEstimateBytes".to_string(),
                json!(4096_i64),
            )])),
            ..Default::default()
        };
        let byte_period = current_period(&byte_definition, now);
        let byte_item = build_quota_status_item(
            &finite_plan(),
            &byte_definition,
            &byte_period,
            &UsageCounter {
                tenant_id: TEN_FINITE.to_string(),
                category: byte_definition.category.clone(),
                committed_amount: 6_000,
                ..Default::default()
            },
            None,
            None,
            None,
        );
        assert_eq!(byte_item.status, QuotaStatus::NEAR_LIMIT, "{byte_item:?}");
        assert_eq!(
            byte_item.near_limit_reason,
            NearLimitReason::BELOW_ONE_TYPICAL_OPERATION
        );
        assert_eq!(byte_item.typical_operation_amount, 4096);
    }

    #[test]
    fn group_quota_status_items_includes_all_required_categories() {
        let items: Vec<QuotaStatusItem> = required_categories()
            .into_iter()
            .map(|category| QuotaStatusItem {
                category,
                ..Default::default()
            })
            .collect();
        let sections = group_quota_status_items(items);
        let mut seen = std::collections::HashSet::new();
        for section in &sections {
            assert!(!section.section_key.is_empty() && !section.label.is_empty());
            for item in &section.items {
                assert!(
                    seen.insert(item.category.clone()),
                    "category {} appeared in multiple sections",
                    item.category
                );
            }
        }
        for category in required_categories() {
            assert!(seen.contains(&category), "missing category {category}");
        }
    }

    #[test]
    fn build_quota_status_item_shows_override_and_restriction_separately() {
        let now = utc_ymd_hms(2026, 5, 7, 10, 0, 0);
        let definition = definition_for(&Category::from(Category::RUN_LAUNCHES)).unwrap();
        let (start, end) = period_for(&definition.period_kind, now);
        let restriction = AbuseRestrictionSummary {
            restriction_id: "restriction_1".to_string(),
            status: AbuseRestrictionStatus::from(AbuseRestrictionStatus::ACTIVE),
            affected_category: Category::from(Category::RUN_LAUNCHES),
            recovery_action: RecoveryAction::from(RecoveryAction::CONTACT_SUPPORT),
            visible_reason_code: "abuse_restriction:temporary".to_string(),
            source_audit_ref: "audit_1".to_string(),
            support_contact_allowed: true,
            ..Default::default()
        };
        let override_ = QuotaOverride {
            tenant_id: TEN_FINITE.to_string(),
            category: definition.category.clone(),
            limit: Some(3),
            reason: "support override".to_string(),
            effective_at: now,
            ..Default::default()
        };
        let item = build_quota_status_item(
            &finite_plan(),
            &definition,
            &QuotaPeriod {
                tenant_id: TEN_FINITE.to_string(),
                category: definition.category.clone(),
                period_start: start,
                period_end: end,
                ..Default::default()
            },
            &UsageCounter {
                tenant_id: TEN_FINITE.to_string(),
                category: definition.category.clone(),
                committed_amount: 1,
                ..Default::default()
            },
            None,
            Some(&override_),
            Some(&restriction),
        );
        let override_summary = item.override_.as_ref().expect("override summary");
        assert_eq!(override_summary.base_limit, definition.default_limit);
        assert_eq!(override_summary.effective_limit, 3);
        assert!(item.restriction.is_some());
        assert_eq!(item.status, QuotaStatus::RESTRICTED);
    }

    #[test]
    fn recovery_actions_for_exhausted_quota_include_override_request() {
        let actions = recovery_actions_for_quota_status(
            &QuotaStatus::from(QuotaStatus::EXHAUSTED),
            &NearLimitReason::from(NearLimitReason::NONE),
        );
        assert!(
            actions
                .iter()
                .any(|action| *action == RecoveryAction::REQUEST_OVERRIDE),
            "expected request_override in {actions:?}"
        );
    }

    struct ProjectionTestRepo {
        plan: TenantPlan,
        periods: HashMap<Category, QuotaPeriod>,
        counters: HashMap<String, UsageCounter>,
        previous_period: QuotaPeriod,
        previous_counter: UsageCounter,
    }

    impl Repository for ProjectionTestRepo {
        fn active_plan(&self, _tenant_id: &str) -> BoxFuture<'_, Result<Option<TenantPlan>>> {
            let plan = self.plan.clone();
            Box::pin(async move { Ok(Some(plan)) })
        }

        fn quota_override(
            &self,
            _tenant_id: &str,
            _category: &Category,
            _at: DateTime<Utc>,
        ) -> BoxFuture<'_, Result<Option<QuotaOverride>>> {
            Box::pin(async { Ok(None) })
        }

        fn open_period(
            &self,
            tenant_id: &str,
            definition: &QuotaDefinition,
            at: DateTime<Utc>,
        ) -> BoxFuture<'_, Result<QuotaPeriod>> {
            let period = if let Some(period) = self.periods.get(&definition.category) {
                period.clone()
            } else {
                let (start, end) = period_for(&definition.period_kind, at);
                QuotaPeriod {
                    quota_period_id: format!("period_{}", definition.category),
                    tenant_id: tenant_id.to_string(),
                    category: definition.category.clone(),
                    period_kind: definition.period_kind.clone(),
                    period_start: start,
                    period_end: end,
                    status: "open".to_string(),
                    ..Default::default()
                }
            };
            Box::pin(async move { Ok(period) })
        }

        fn usage_counter(
            &self,
            tenant_id: &str,
            category: &Category,
            quota_period_id: &str,
        ) -> BoxFuture<'_, Result<Option<UsageCounter>>> {
            let counter = self.counters.get(quota_period_id).cloned();
            let (tenant_id, category, quota_period_id) = (
                tenant_id.to_string(),
                category.clone(),
                quota_period_id.to_string(),
            );
            Box::pin(async move {
                if counter.is_some() {
                    return Ok(counter);
                }
                let _ = UsageCounter {
                    tenant_id,
                    category,
                    quota_period_id,
                    ..Default::default()
                };
                Ok(None)
            })
        }

        fn save_usage_counter(&self, _counter: UsageCounter) -> BoxFuture<'_, Result<()>> {
            Box::pin(async { Ok(()) })
        }

        fn reservation_by_operation(
            &self,
            _tenant_id: &str,
            _category: &Category,
            _operation_key: &str,
        ) -> BoxFuture<'_, Result<Option<UsageReservation>>> {
            Box::pin(async { Ok(None) })
        }

        fn save_reservation(&self, _reservation: UsageReservation) -> BoxFuture<'_, Result<()>> {
            Box::pin(async { Ok(()) })
        }

        fn append_usage_event(&self, _event: UsageEvent) -> BoxFuture<'_, Result<()>> {
            Box::pin(async { Ok(()) })
        }

        fn append_quota_denial(&self, _denial: QuotaDenial) -> BoxFuture<'_, Result<()>> {
            Box::pin(async { Ok(()) })
        }

        fn list_pending_reservations(&self) -> BoxFuture<'_, Result<Vec<UsageReservation>>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn previous_quota_period(
            &self,
            _tenant_id: &str,
            category: &Category,
            _before: DateTime<Utc>,
        ) -> BoxFuture<'_, Result<Option<(QuotaPeriod, UsageCounter)>>> {
            let out = if *category == self.previous_period.category {
                Some((self.previous_period.clone(), self.previous_counter.clone()))
            } else {
                None
            };
            Box::pin(async move { Ok(out) })
        }
    }

    #[tokio::test]
    async fn quota_dashboard_projects_current_and_immediately_previous_completed_period() {
        let now = utc_ymd_hms(2026, 5, 7, 10, 0, 0);
        let definition = definition_for(&Category::from(Category::RUN_LAUNCHES)).unwrap();
        let (current_start, current_end) = period_for(&definition.period_kind, now);
        let previous_start = current_start - Duration::days(30);
        let previous_end = current_start;
        let repo = ProjectionTestRepo {
            plan: TenantPlan {
                tenant_id: TEN_FINITE.to_string(),
                plan_key: "finite".to_string(),
                status: PlanStatus::from(PlanStatus::ACTIVE),
                enforcement_mode: EnforcementMode::from(EnforcementMode::ENFORCED),
                effective_at: now - Duration::hours(1),
                ..Default::default()
            },
            periods: HashMap::from([(
                Category::from(Category::RUN_LAUNCHES),
                QuotaPeriod {
                    quota_period_id: "period_current".to_string(),
                    tenant_id: TEN_FINITE.to_string(),
                    category: Category::from(Category::RUN_LAUNCHES),
                    period_kind: definition.period_kind.clone(),
                    period_start: current_start,
                    period_end: current_end,
                    status: "open".to_string(),
                    ..Default::default()
                },
            )]),
            counters: HashMap::from([(
                "period_current".to_string(),
                UsageCounter {
                    tenant_id: TEN_FINITE.to_string(),
                    category: Category::from(Category::RUN_LAUNCHES),
                    quota_period_id: "period_current".to_string(),
                    committed_amount: 4,
                    ..Default::default()
                },
            )]),
            previous_period: QuotaPeriod {
                quota_period_id: "period_previous".to_string(),
                tenant_id: TEN_FINITE.to_string(),
                category: Category::from(Category::RUN_LAUNCHES),
                period_kind: definition.period_kind.clone(),
                period_start: previous_start,
                period_end: previous_end,
                status: "closed".to_string(),
                ..Default::default()
            },
            previous_counter: UsageCounter {
                tenant_id: TEN_FINITE.to_string(),
                category: Category::from(Category::RUN_LAUNCHES),
                quota_period_id: "period_previous".to_string(),
                committed_amount: 7,
                ..Default::default()
            },
        };
        let manager = Manager::with_clock(Arc::new(repo), move || now);
        let dashboard = manager.quota_dashboard(TEN_FINITE, true).await.unwrap();
        for section in &dashboard.sections {
            for item in &section.items {
                if item.category != Category::RUN_LAUNCHES {
                    continue;
                }
                assert_eq!(item.current_period.consumed_amount, 4);
                let previous = item.previous_period.as_ref().expect("previous period");
                assert_eq!(previous.consumed_amount, 7);
                assert_eq!(previous.period_end, previous_end);
                return;
            }
        }
        panic!("run launch dashboard item not found");
    }
}
