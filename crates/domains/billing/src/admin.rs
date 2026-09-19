//! Operator/admin mutations: plan assignment, quota overrides, manual
//! adjustments, and reservation resolution (port of `admin.go`).

use crate::catalog::definition_for;
use crate::error::BillingError;
use crate::error::Result;
use crate::lifecycle::ResolveInput;
use crate::manager::Manager;
use crate::types::EnforcementMode;
use crate::types::ManualAdjustment;
use crate::types::PlanStatus;
use crate::types::QuotaOverride;
use crate::types::ReservationStatus;
use crate::types::TenantPlan;
use crate::types::UsageEvent;
use crate::types::UsageEventKind;
use crate::types::UsageReservation;
use crate::types::go_zero_time;

impl Manager {
    /// Assign a plan to a tenant, recording the change as a usage event.
    pub async fn assign_plan(
        &self,
        mut plan: TenantPlan,
        actor_principal_id: &str,
        reason: &str,
    ) -> Result<()> {
        if reason.trim().is_empty() {
            return Err(BillingError::ReasonRequired);
        }
        let repo = self
            .repo()
            .ok_or(BillingError::NotSupported("admin plan assignment"))?;
        if plan.assignment_reason.is_empty() {
            plan.assignment_reason = reason.to_string();
        }
        if plan.assigned_by_principal_id.is_empty() {
            plan.assigned_by_principal_id = actor_principal_id.to_string();
        }
        if plan.status.is_empty() {
            plan.status = PlanStatus::from(PlanStatus::ACTIVE);
        }
        if plan.enforcement_mode.is_empty() {
            plan.enforcement_mode = EnforcementMode::from(EnforcementMode::ENFORCED);
        }
        if plan.effective_at == go_zero_time() {
            plan.effective_at = self.clock_now();
        }
        repo.save_plan(plan.clone()).await?;
        repo.append_usage_event(UsageEvent {
            usage_event_id: format!(
                "usage_event_plan_changed_{}_{}",
                plan.tenant_id, plan.plan_id
            ),
            tenant_id: plan.tenant_id.clone(),
            event_kind: UsageEventKind::from(UsageEventKind::PLAN_CHANGED),
            reason_code: "billing.plan_changed".to_string(),
            reason: reason.to_string(),
            actor_principal_id: actor_principal_id.to_string(),
            outcome: "succeeded".to_string(),
            created_at: self.clock_now(),
            ..Default::default()
        })
        .await
    }

    /// Apply a quota override for a tenant/category.
    pub async fn apply_quota_override(&self, mut override_: QuotaOverride) -> Result<()> {
        if override_.reason.trim().is_empty() {
            return Err(BillingError::ReasonRequired);
        }
        let repo = self
            .repo()
            .ok_or(BillingError::NotSupported("quota overrides"))?;
        if override_.effective_at == go_zero_time() {
            override_.effective_at = self.clock_now();
        }
        repo.save_quota_override(override_.clone()).await?;
        repo.append_usage_event(UsageEvent {
            usage_event_id: format!(
                "usage_event_quota_override_{}_{}",
                override_.tenant_id, override_.category
            ),
            tenant_id: override_.tenant_id.clone(),
            category: override_.category.clone(),
            event_kind: UsageEventKind::from(UsageEventKind::QUOTA_OVERRIDE),
            reason_code: "billing.quota_override_changed".to_string(),
            reason: override_.reason.clone(),
            actor_principal_id: override_.created_by_principal_id.clone(),
            outcome: "succeeded".to_string(),
            created_at: self.clock_now(),
            ..Default::default()
        })
        .await
    }

    /// Apply a manual usage adjustment; rejects adjustments that would make
    /// effective usage negative.
    pub async fn apply_manual_adjustment(&self, adjustment: ManualAdjustment) -> Result<()> {
        if adjustment.reason.trim().is_empty() {
            return Err(BillingError::ReasonRequired);
        }
        let repo = self
            .repo()
            .ok_or(BillingError::NotSupported("manual adjustments"))?;
        if definition_for(&adjustment.category).is_none() {
            return Err(BillingError::UnknownCategory(
                adjustment.category.to_string(),
            ));
        }
        let counter = repo
            .usage_counter(
                &adjustment.tenant_id,
                &adjustment.category,
                &adjustment.quota_period_id,
            )
            .await?;
        if let Some(counter) = &counter {
            if counter.committed_amount
                + counter.reserved_amount
                + counter.adjusted_amount
                + adjustment.amount_delta
                < 0
            {
                return Err(BillingError::NegativeEffectiveUsage);
            }
        }
        repo.save_manual_adjustment(adjustment.clone()).await?;
        if let Some(mut counter) = counter {
            counter.adjusted_amount += adjustment.amount_delta;
            counter.updated_at = self.clock_now();
            repo.save_usage_counter(counter).await?;
        }
        repo.append_usage_event(UsageEvent {
            usage_event_id: format!(
                "usage_event_adjustment_{}_{}",
                adjustment.tenant_id, adjustment.adjustment_id
            ),
            tenant_id: adjustment.tenant_id.clone(),
            category: adjustment.category.clone(),
            quota_period_id: adjustment.quota_period_id.clone(),
            event_kind: UsageEventKind::from(UsageEventKind::MANUAL_ADJUSTMENT),
            amount: adjustment.amount_delta,
            reason_code: "billing.manual_adjustment_created".to_string(),
            reason: adjustment.reason.clone(),
            actor_principal_id: adjustment.created_by_principal_id.clone(),
            outcome: "succeeded".to_string(),
            created_at: self.clock_now(),
            ..Default::default()
        })
        .await
    }

    /// Resolve a reservation by ID to a terminal lifecycle outcome.
    pub async fn resolve_reservation(
        &self,
        input: ResolveReservationInput,
    ) -> Result<UsageReservation> {
        if input.reason.trim().is_empty() {
            return Err(BillingError::ReasonRequired);
        }
        let repo = self
            .repo()
            .ok_or(BillingError::NotSupported("reservation resolution"))?;
        let Some(reservation) = repo
            .reservation_by_id(&input.tenant_id, &input.reservation_id)
            .await?
        else {
            return Err(BillingError::ReservationIdNotFound(
                input.reservation_id.clone(),
            ));
        };
        let resolve_input = ResolveInput {
            tenant_id: input.tenant_id.clone(),
            category: reservation.category.clone(),
            operation_key: reservation.operation_key.clone(),
            amount: input.amount,
            reason_code: "billing.reservation_resolved".to_string(),
            reason: input.reason.clone(),
            actor_principal_id: input.actor_principal_id.clone(),
            ..Default::default()
        };
        match input.outcome.as_str() {
            ReservationStatus::COMMITTED => self.commit(resolve_input).await,
            ReservationStatus::REFUNDED => self.refund(resolve_input).await,
            ReservationStatus::RELEASED => self.release(resolve_input).await,
            ReservationStatus::OPERATOR_ACTION_NEEDED => {
                self.mark_operator_action_needed(resolve_input).await
            }
            other => Err(BillingError::UnsupportedResolutionOutcome(
                other.to_string(),
            )),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ResolveReservationInput {
    pub tenant_id: String,
    pub reservation_id: String,
    pub outcome: ReservationStatus,
    pub amount: i64,
    pub reason: String,
    pub actor_principal_id: String,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::catalog::definition_for;
    use crate::fixtures::FixtureRepo;
    use crate::fixtures::TEN_FINITE;
    use crate::fixtures::fixed_now;
    use crate::types::Category;
    use crate::types::UsageCounter;

    fn manager_and_repo() -> (Arc<FixtureRepo>, Manager) {
        let now = fixed_now();
        let repo = Arc::new(FixtureRepo::new(now));
        let manager = Manager::with_clock(repo.clone(), move || now);
        (repo, manager)
    }

    fn seed_counter(repo: &FixtureRepo) -> crate::types::QuotaPeriod {
        let now = fixed_now();
        let definition = definition_for(&Category::from(Category::RUN_LAUNCHES)).unwrap();
        let period = repo.open_period_sync(TEN_FINITE, &definition, now);
        repo.save_counter(UsageCounter {
            usage_counter_id: "counter_adjustment".to_string(),
            tenant_id: TEN_FINITE.to_string(),
            category: Category::from(Category::RUN_LAUNCHES),
            quota_period_id: period.quota_period_id.clone(),
            committed_amount: 2,
            updated_at: now,
            ..Default::default()
        });
        period
    }

    #[tokio::test]
    async fn apply_manual_adjustment_validation() {
        let (repo, manager) = manager_and_repo();
        let period = seed_counter(&repo);

        let err = manager
            .apply_manual_adjustment(ManualAdjustment {
                adjustment_id: "adjustment_without_reason".to_string(),
                tenant_id: TEN_FINITE.to_string(),
                category: Category::from(Category::RUN_LAUNCHES),
                quota_period_id: period.quota_period_id.clone(),
                amount_delta: 1,
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert!(matches!(err, BillingError::ReasonRequired), "{err}");

        let err = manager
            .apply_manual_adjustment(ManualAdjustment {
                adjustment_id: "adjustment_negative_effective_usage".to_string(),
                tenant_id: TEN_FINITE.to_string(),
                category: Category::from(Category::RUN_LAUNCHES),
                quota_period_id: period.quota_period_id.clone(),
                amount_delta: -3,
                reason: "operator correction".to_string(),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert!(matches!(err, BillingError::NegativeEffectiveUsage), "{err}");

        let err = manager
            .apply_manual_adjustment(ManualAdjustment {
                adjustment_id: "adjustment_unknown_category".to_string(),
                tenant_id: TEN_FINITE.to_string(),
                category: Category::from("unknown"),
                quota_period_id: period.quota_period_id.clone(),
                amount_delta: 1,
                reason: "operator correction".to_string(),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert!(matches!(err, BillingError::UnknownCategory(_)), "{err}");
    }

    #[tokio::test]
    async fn apply_manual_adjustment_updates_counter_and_evidence() {
        let (repo, manager) = manager_and_repo();
        let period = seed_counter(&repo);

        manager
            .apply_manual_adjustment(ManualAdjustment {
                adjustment_id: "adjustment_valid".to_string(),
                tenant_id: TEN_FINITE.to_string(),
                category: Category::from(Category::RUN_LAUNCHES),
                quota_period_id: period.quota_period_id.clone(),
                amount_delta: -1,
                reason: "operator correction".to_string(),
                created_by_principal_id: "admin".to_string(),
                ..Default::default()
            })
            .await
            .unwrap();
        let counter = repo
            .counter(
                TEN_FINITE,
                &Category::from(Category::RUN_LAUNCHES),
                &period.quota_period_id,
            )
            .expect("counter");
        assert_eq!(counter.adjusted_amount, -1);
        assert_eq!(repo.adjustment_count(), 1);
        let events = repo.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_kind, UsageEventKind::MANUAL_ADJUSTMENT);
        assert!(!events[0].reason.is_empty());
    }
}
