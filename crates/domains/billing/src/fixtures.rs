//! In-memory fixture repository shared by the crate's tests (port of
//! `test_fixtures_test.go` / `operation_fixtures_test.go`).

use std::collections::HashMap;

use chrono::DateTime;
use chrono::Utc;
use parking_lot::Mutex;

use crate::error::Result;
use crate::manager::BoxFuture;
use crate::manager::Repository;
use crate::manager::development_plan;
use crate::projection::period_for;
use crate::types::AbuseRestrictionRecord;
use crate::types::AbuseRestrictionStatus;
use crate::types::Category;
use crate::types::EnforcementMode;
use crate::types::ManualAdjustment;
use crate::types::PlanStatus;
use crate::types::QuotaDefinition;
use crate::types::QuotaDenial;
use crate::types::QuotaOverride;
use crate::types::QuotaPeriod;
use crate::types::ReservationStatus;
use crate::types::TenantPlan;
use crate::types::UsageCounter;
use crate::types::UsageEvent;
use crate::types::UsageReservation;

pub(crate) const TEN_FINITE: &str = "ten_finite";
pub(crate) const TEN_OTHER: &str = "ten_other";
pub(crate) const RUN_ID: &str = "run_fixture";
pub(crate) const STEP_ID: &str = "step_fixture";
pub(crate) const CLIENT: &str = "client_fixture";

pub(crate) fn fixed_now() -> DateTime<Utc> {
    crate::projection::utc_ymd_hms(2026, 4, 28, 12, 0, 0)
}

#[derive(Default)]
struct FixtureState {
    plans: HashMap<String, TenantPlan>,
    overrides: HashMap<String, QuotaOverride>,
    periods: HashMap<String, QuotaPeriod>,
    counters: HashMap<String, UsageCounter>,
    reservations: HashMap<String, UsageReservation>,
    events: Vec<UsageEvent>,
    denials: Vec<QuotaDenial>,
    adjustments: Vec<ManualAdjustment>,
    restrictions: Vec<AbuseRestrictionRecord>,
}

pub(crate) struct FixtureRepo {
    state: Mutex<FixtureState>,
}

fn counter_key(tenant_id: &str, category: &Category, period_id: &str) -> String {
    format!("{tenant_id}:{category}:{period_id}")
}

impl FixtureRepo {
    pub(crate) fn new(now: DateTime<Utc>) -> Self {
        let repo = Self {
            state: Mutex::new(FixtureState::default()),
        };
        {
            let mut state = repo.state.lock();
            state.plans.insert(
                "ten_finite".to_string(),
                TenantPlan {
                    plan_id: "plan_finite".to_string(),
                    tenant_id: "ten_finite".to_string(),
                    plan_key: "hosted-finite".to_string(),
                    status: PlanStatus::from(PlanStatus::ACTIVE),
                    enforcement_mode: EnforcementMode::from(EnforcementMode::ENFORCED),
                    effective_at: now,
                    ..Default::default()
                },
            );
            state.plans.insert(
                "ten_unlimited".to_string(),
                TenantPlan {
                    plan_id: "plan_unlimited".to_string(),
                    tenant_id: "ten_unlimited".to_string(),
                    plan_key: "unlimited".to_string(),
                    status: PlanStatus::from(PlanStatus::ACTIVE),
                    enforcement_mode: EnforcementMode::from(EnforcementMode::UNLIMITED),
                    effective_at: now,
                    ..Default::default()
                },
            );
            state
                .plans
                .insert("ten_dev".to_string(), development_plan("ten_dev", now));
        }
        repo
    }

    pub(crate) fn open_period_sync(
        &self,
        tenant_id: &str,
        definition: &QuotaDefinition,
        at: DateTime<Utc>,
    ) -> QuotaPeriod {
        let (start, end) = period_for(&definition.period_kind, at);
        let key = format!(
            "{}:{}:{}",
            tenant_id,
            definition.category,
            start.to_rfc3339()
        );
        let mut state = self.state.lock();
        state
            .periods
            .entry(key.clone())
            .or_insert_with(|| QuotaPeriod {
                quota_period_id: format!("period_{key}"),
                tenant_id: tenant_id.to_string(),
                category: definition.category.clone(),
                period_kind: definition.period_kind.clone(),
                period_start: start,
                period_end: end,
                status: "open".to_string(),
                ..Default::default()
            })
            .clone()
    }

    pub(crate) fn save_counter(&self, counter: UsageCounter) {
        self.state.lock().counters.insert(
            counter_key(
                &counter.tenant_id,
                &counter.category,
                &counter.quota_period_id,
            ),
            counter,
        );
    }

    pub(crate) fn save_reservation_sync(&self, reservation: UsageReservation) {
        self.state.lock().reservations.insert(
            counter_key(
                &reservation.tenant_id,
                &reservation.category,
                &reservation.operation_key,
            ),
            reservation,
        );
    }

    pub(crate) fn counter(
        &self,
        tenant_id: &str,
        category: &Category,
        period_id: &str,
    ) -> Option<UsageCounter> {
        self.state
            .lock()
            .counters
            .get(&counter_key(tenant_id, category, period_id))
            .cloned()
    }

    pub(crate) fn reservation(
        &self,
        tenant_id: &str,
        category: &Category,
        operation_key: &str,
    ) -> Option<UsageReservation> {
        self.state
            .lock()
            .reservations
            .get(&counter_key(tenant_id, category, operation_key))
            .cloned()
    }

    pub(crate) fn denial_count(&self) -> usize {
        self.state.lock().denials.len()
    }

    pub(crate) fn adjustment_count(&self) -> usize {
        self.state.lock().adjustments.len()
    }

    pub(crate) fn events(&self) -> Vec<UsageEvent> {
        self.state.lock().events.clone()
    }

    pub(crate) fn push_denial(&self, denial: QuotaDenial) {
        self.state.lock().denials.push(denial);
    }

    pub(crate) fn push_restriction(&self, restriction: AbuseRestrictionRecord) {
        self.state.lock().restrictions.push(restriction);
    }

    pub(crate) fn push_event(&self, event: UsageEvent) {
        self.state.lock().events.push(event);
    }

    pub(crate) fn set_override(&self, override_: QuotaOverride) {
        self.state.lock().overrides.insert(
            format!("{}:{}", override_.tenant_id, override_.category),
            override_,
        );
    }
}

impl Repository for FixtureRepo {
    fn active_plan(&self, tenant_id: &str) -> BoxFuture<'_, Result<Option<TenantPlan>>> {
        let plan = self.state.lock().plans.get(tenant_id).cloned();
        Box::pin(async move { Ok(plan) })
    }

    fn quota_override(
        &self,
        tenant_id: &str,
        category: &Category,
        _at: DateTime<Utc>,
    ) -> BoxFuture<'_, Result<Option<QuotaOverride>>> {
        let override_ = self
            .state
            .lock()
            .overrides
            .get(&format!("{tenant_id}:{category}"))
            .cloned();
        Box::pin(async move { Ok(override_) })
    }

    fn open_period(
        &self,
        tenant_id: &str,
        definition: &QuotaDefinition,
        at: DateTime<Utc>,
    ) -> BoxFuture<'_, Result<QuotaPeriod>> {
        let period = self.open_period_sync(tenant_id, definition, at);
        Box::pin(async move { Ok(period) })
    }

    fn usage_counter(
        &self,
        tenant_id: &str,
        category: &Category,
        quota_period_id: &str,
    ) -> BoxFuture<'_, Result<Option<UsageCounter>>> {
        let counter = self.counter(tenant_id, category, quota_period_id);
        Box::pin(async move { Ok(counter) })
    }

    fn save_usage_counter(&self, counter: UsageCounter) -> BoxFuture<'_, Result<()>> {
        self.save_counter(counter);
        Box::pin(async { Ok(()) })
    }

    fn reservation_by_operation(
        &self,
        tenant_id: &str,
        category: &Category,
        operation_key: &str,
    ) -> BoxFuture<'_, Result<Option<UsageReservation>>> {
        let reservation = self.reservation(tenant_id, category, operation_key);
        Box::pin(async move { Ok(reservation) })
    }

    fn reservation_by_id(
        &self,
        tenant_id: &str,
        reservation_id: &str,
    ) -> BoxFuture<'_, Result<Option<UsageReservation>>> {
        let reservation = self
            .state
            .lock()
            .reservations
            .values()
            .find(|item| item.tenant_id == tenant_id && item.reservation_id == reservation_id)
            .cloned();
        Box::pin(async move { Ok(reservation) })
    }

    fn save_reservation(&self, reservation: UsageReservation) -> BoxFuture<'_, Result<()>> {
        self.save_reservation_sync(reservation);
        Box::pin(async { Ok(()) })
    }

    fn append_usage_event(&self, event: UsageEvent) -> BoxFuture<'_, Result<()>> {
        self.push_event(event);
        Box::pin(async { Ok(()) })
    }

    fn append_quota_denial(&self, denial: QuotaDenial) -> BoxFuture<'_, Result<()>> {
        self.push_denial(denial);
        Box::pin(async { Ok(()) })
    }

    fn quota_denial_by_id(
        &self,
        tenant_id: &str,
        denial_id: &str,
    ) -> BoxFuture<'_, Result<Option<QuotaDenial>>> {
        let denial = self
            .state
            .lock()
            .denials
            .iter()
            .find(|item| item.tenant_id == tenant_id && item.denial_id == denial_id)
            .cloned();
        Box::pin(async move { Ok(denial) })
    }

    fn list_abuse_restrictions(
        &self,
        tenant_id: &str,
        at: DateTime<Utc>,
    ) -> BoxFuture<'_, Result<Vec<AbuseRestrictionRecord>>> {
        let restrictions: Vec<AbuseRestrictionRecord> = self
            .state
            .lock()
            .restrictions
            .iter()
            .filter(|item| {
                item.tenant_id == tenant_id
                    && item.status == AbuseRestrictionStatus::ACTIVE
                    && item.started_at <= at
                    && item.expires_at.is_none_or(|expires| expires > at)
            })
            .cloned()
            .collect();
        Box::pin(async move { Ok(restrictions) })
    }

    fn list_usage_evidence_refs(
        &self,
        tenant_id: &str,
        operation_key: &str,
        _limit: usize,
    ) -> BoxFuture<'_, Result<Vec<String>>> {
        let refs: Vec<String> = self
            .state
            .lock()
            .events
            .iter()
            .filter(|item| item.tenant_id == tenant_id && item.operation_key == operation_key)
            .map(|item| format!("usage_event:{}", item.usage_event_id))
            .collect();
        Box::pin(async move { Ok(refs) })
    }

    fn list_pending_reservations(&self) -> BoxFuture<'_, Result<Vec<UsageReservation>>> {
        let pending: Vec<UsageReservation> = self
            .state
            .lock()
            .reservations
            .values()
            .filter(|item| item.status == ReservationStatus::RESERVED)
            .cloned()
            .collect();
        Box::pin(async move { Ok(pending) })
    }

    fn save_plan(&self, plan: TenantPlan) -> BoxFuture<'_, Result<()>> {
        self.state.lock().plans.insert(plan.tenant_id.clone(), plan);
        Box::pin(async { Ok(()) })
    }

    fn save_quota_override(&self, override_: QuotaOverride) -> BoxFuture<'_, Result<()>> {
        self.set_override(override_);
        Box::pin(async { Ok(()) })
    }

    fn save_manual_adjustment(&self, adjustment: ManualAdjustment) -> BoxFuture<'_, Result<()>> {
        self.state.lock().adjustments.push(adjustment);
        Box::pin(async { Ok(()) })
    }
}
