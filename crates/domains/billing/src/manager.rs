//! Billing manager: reservation entry points, quota checks, and the
//! persistence abstraction (port of `manager.go`).

use std::pin::Pin;
use std::sync::Arc;

use crate::catalog::definition_for;
use crate::denial::DenialPayload;
use crate::denial::new_quota_exhausted_denial;
use crate::denial::new_quota_state_unavailable_denial;
use crate::error::BillingError;
use crate::error::Result;
use crate::lifecycle::ResolveInput;
use crate::projection::EffectiveQuota;
use crate::projection::project_quota;
use crate::types::AbuseRestrictionRecord;
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
use crate::types::UsageEventKind;
use crate::types::UsageReservation;
use chrono::DateTime;
use chrono::Utc;

/// Object-safe boxed future used by the repository trait.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Persistence abstraction for billing state.
///
/// Required methods mirror the Go `Repository` interface. Optional
/// capabilities (Go interface assertions: `ProjectionRepository`,
/// `PreviousPeriodRepository`, `AbuseRestrictionRepository`,
/// `DenialLookupRepository`, `EvidenceReferenceRepository`,
/// `AdminRepository`, `ReservationAdminRepository`, and the transactional
/// hooks) are defaulted methods: implementors override the ones they
/// support.
pub trait Repository: Send + Sync {
    fn active_plan(&self, tenant_id: &str) -> BoxFuture<'_, Result<Option<TenantPlan>>>;
    fn quota_override(
        &self,
        tenant_id: &str,
        category: &Category,
        at: DateTime<Utc>,
    ) -> BoxFuture<'_, Result<Option<QuotaOverride>>>;
    fn open_period(
        &self,
        tenant_id: &str,
        definition: &QuotaDefinition,
        at: DateTime<Utc>,
    ) -> BoxFuture<'_, Result<QuotaPeriod>>;
    fn usage_counter(
        &self,
        tenant_id: &str,
        category: &Category,
        quota_period_id: &str,
    ) -> BoxFuture<'_, Result<Option<UsageCounter>>>;
    fn save_usage_counter(&self, counter: UsageCounter) -> BoxFuture<'_, Result<()>>;
    fn reservation_by_operation(
        &self,
        tenant_id: &str,
        category: &Category,
        operation_key: &str,
    ) -> BoxFuture<'_, Result<Option<UsageReservation>>>;
    fn save_reservation(&self, reservation: UsageReservation) -> BoxFuture<'_, Result<()>>;
    fn append_usage_event(&self, event: UsageEvent) -> BoxFuture<'_, Result<()>>;
    fn append_quota_denial(&self, denial: QuotaDenial) -> BoxFuture<'_, Result<()>>;
    fn list_pending_reservations(&self) -> BoxFuture<'_, Result<Vec<UsageReservation>>>;

    // -- Optional projection capabilities -------------------------------

    fn list_quota_denials(
        &self,
        tenant_id: &str,
        limit: usize,
    ) -> BoxFuture<'_, Result<Vec<QuotaDenial>>> {
        let _ = (tenant_id, limit);
        Box::pin(async { Ok(Vec::new()) })
    }

    fn list_manual_adjustments(
        &self,
        tenant_id: &str,
        limit: usize,
    ) -> BoxFuture<'_, Result<Vec<ManualAdjustment>>> {
        let _ = (tenant_id, limit);
        Box::pin(async { Ok(Vec::new()) })
    }

    fn previous_quota_period(
        &self,
        tenant_id: &str,
        category: &Category,
        before: DateTime<Utc>,
    ) -> BoxFuture<'_, Result<Option<(QuotaPeriod, UsageCounter)>>> {
        let _ = (tenant_id, category, before);
        Box::pin(async { Ok(None) })
    }

    fn list_abuse_restrictions(
        &self,
        tenant_id: &str,
        at: DateTime<Utc>,
    ) -> BoxFuture<'_, Result<Vec<AbuseRestrictionRecord>>> {
        let _ = (tenant_id, at);
        Box::pin(async { Ok(Vec::new()) })
    }

    fn quota_denial_by_id(
        &self,
        tenant_id: &str,
        denial_id: &str,
    ) -> BoxFuture<'_, Result<Option<QuotaDenial>>> {
        let _ = (tenant_id, denial_id);
        Box::pin(async { Ok(None) })
    }

    fn list_usage_evidence_refs(
        &self,
        tenant_id: &str,
        operation_key: &str,
        limit: usize,
    ) -> BoxFuture<'_, Result<Vec<String>>> {
        let _ = (tenant_id, operation_key, limit);
        Box::pin(async { Ok(Vec::new()) })
    }

    // -- Optional admin capabilities -------------------------------------

    fn save_plan(&self, plan: TenantPlan) -> BoxFuture<'_, Result<()>> {
        let _ = plan;
        Box::pin(async { Err(BillingError::NotSupported("admin plan assignment")) })
    }

    fn save_quota_override(&self, override_: QuotaOverride) -> BoxFuture<'_, Result<()>> {
        let _ = override_;
        Box::pin(async { Err(BillingError::NotSupported("quota overrides")) })
    }

    fn save_manual_adjustment(&self, adjustment: ManualAdjustment) -> BoxFuture<'_, Result<()>> {
        let _ = adjustment;
        Box::pin(async { Err(BillingError::NotSupported("manual adjustments")) })
    }

    fn reservation_by_id(
        &self,
        tenant_id: &str,
        reservation_id: &str,
    ) -> BoxFuture<'_, Result<Option<UsageReservation>>> {
        let _ = (tenant_id, reservation_id);
        Box::pin(async { Err(BillingError::NotSupported("reservation resolution")) })
    }

    // -- Optional transactional hooks ------------------------------------
    //
    // A repository capable of running the accounting mutation in one durable
    // transaction overrides these; returning `Ok(None)` falls back to the
    // manager's step-by-step logic.

    fn reserve_usage(
        &self,
        input: ReserveInput,
        now: DateTime<Utc>,
    ) -> BoxFuture<'_, Result<Option<ReserveResult>>> {
        let _ = (input, now);
        Box::pin(async { Ok(None) })
    }

    fn resolve_usage(
        &self,
        input: ResolveInput,
        status: ReservationStatus,
        event_kind: UsageEventKind,
        now: DateTime<Utc>,
    ) -> BoxFuture<'_, Result<Option<UsageReservation>>> {
        let _ = (input, status, event_kind, now);
        Box::pin(async { Ok(None) })
    }

    fn reserve_all_usage(
        &self,
        inputs: Vec<ReserveInput>,
        now: DateTime<Utc>,
    ) -> BoxFuture<'_, Result<Option<ReserveAllResult>>> {
        let _ = (inputs, now);
        Box::pin(async { Ok(None) })
    }
}

/// Billing accounting engine. Holds an optional repository and a clock;
/// serializes non-transactional mutations through an async lock.
pub struct Manager {
    repo: Option<Arc<dyn Repository>>,
    clock: Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>,
    mu: tokio::sync::Mutex<()>,
}

impl Manager {
    pub fn new(repo: Arc<dyn Repository>) -> Self {
        Self::with_clock(repo, Utc::now)
    }

    pub fn with_clock(
        repo: Arc<dyn Repository>,
        now: impl Fn() -> DateTime<Utc> + Send + Sync + 'static,
    ) -> Self {
        Self {
            repo: Some(repo),
            clock: Arc::new(now),
            mu: tokio::sync::Mutex::new(()),
        }
    }

    /// Manager without persistence (Go: `Manager{repo: nil}`); hosted
    /// operations fail closed, non-hosted ones use the development plan.
    pub fn without_repo() -> Self {
        Self {
            repo: None,
            clock: Arc::new(Utc::now),
            mu: tokio::sync::Mutex::new(()),
        }
    }

    pub(crate) fn repo(&self) -> Option<Arc<dyn Repository>> {
        self.repo.clone()
    }

    pub(crate) fn clock_now(&self) -> DateTime<Utc> {
        (self.clock)()
    }

    pub(crate) async fn mu_lock(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.mu.lock().await
    }

    /// Reserve quota for one operation. Idempotent per operation key.
    ///
    /// Go returns `(ReserveResult, error)` with both populated on quota
    /// denials; here denial-class failures are reported on
    /// [`ReserveResult::failure`] while `Err` is reserved for repository
    /// failures.
    pub async fn reserve(&self, input: ReserveInput) -> Result<ReserveResult> {
        let Some(repo) = self.repo() else {
            if input.hosted {
                let denial =
                    new_quota_state_unavailable_denial(&input.tenant_id, &input.operation_key);
                return Ok(ReserveResult {
                    allowed: false,
                    denial: Some(denial),
                    failure: Some(BillingError::QuotaStateUnavailable),
                    ..Default::default()
                });
            }
            return Ok(ReserveResult {
                allowed: true,
                ..Default::default()
            });
        };
        let _guard = self.mu.lock().await;
        let now = self.clock_now();
        if let Some(result) = repo.reserve_usage(input.clone(), now).await? {
            return Ok(result);
        }
        let amount = if input.amount <= 0 { 1 } else { input.amount };
        let Some(definition) = definition_for(&input.category) else {
            return Err(BillingError::UnknownCategory(input.category.to_string()));
        };
        let plan = match repo.active_plan(&input.tenant_id).await? {
            Some(plan) => plan,
            None => {
                if input.hosted {
                    let denial =
                        new_quota_state_unavailable_denial(&input.tenant_id, &input.operation_key);
                    return Ok(ReserveResult {
                        allowed: false,
                        denial: Some(denial),
                        failure: Some(BillingError::QuotaStateUnavailable),
                        ..Default::default()
                    });
                }
                development_plan(&input.tenant_id, now)
            }
        };
        if plan.enforcement_mode == EnforcementMode::UNLIMITED {
            return Ok(ReserveResult {
                allowed: true,
                ..Default::default()
            });
        }
        let period = match repo.open_period(&input.tenant_id, &definition, now).await {
            Ok(period) => period,
            Err(err) => {
                if input.hosted {
                    let denial =
                        new_quota_state_unavailable_denial(&input.tenant_id, &input.operation_key);
                    return Ok(ReserveResult {
                        allowed: false,
                        denial: Some(denial),
                        failure: Some(BillingError::QuotaStateUnavailable),
                        ..Default::default()
                    });
                }
                return Err(err);
            }
        };
        if let Some(existing) = repo
            .reservation_by_operation(&input.tenant_id, &input.category, &input.operation_key)
            .await?
        {
            if existing.status == ReservationStatus::OPERATOR_ACTION_NEEDED {
                let denial =
                    new_quota_state_unavailable_denial(&input.tenant_id, &input.operation_key);
                return Ok(ReserveResult {
                    allowed: false,
                    reservation: Some(existing),
                    denial: Some(denial),
                    failure: Some(BillingError::OperatorActionRequired),
                    ..Default::default()
                });
            }
            if existing.status == ReservationStatus::DENIED {
                let payload = new_quota_exhausted_denial(
                    &input.tenant_id,
                    &input.category,
                    &input.operation_key,
                    amount,
                    0,
                    &period,
                );
                return Ok(ReserveResult {
                    allowed: false,
                    reservation: Some(existing),
                    denial: Some(payload),
                    failure: Some(BillingError::QuotaDenied),
                    ..Default::default()
                });
            }
            return Ok(ReserveResult {
                allowed: true,
                reservation: Some(existing),
                ..Default::default()
            });
        }
        let counter = repo
            .usage_counter(&input.tenant_id, &input.category, &period.quota_period_id)
            .await?
            .unwrap_or_else(|| UsageCounter {
                usage_counter_id: format!(
                    "usage_counter_{}_{}_{}",
                    input.tenant_id, input.category, period.quota_period_id
                ),
                tenant_id: input.tenant_id.clone(),
                category: input.category.clone(),
                quota_period_id: period.quota_period_id.clone(),
                updated_at: now,
                ..Default::default()
            });
        let override_ = repo
            .quota_override(&input.tenant_id, &input.category, now)
            .await?;
        let quota = project_quota(&plan, &definition, &period, &counter, override_.as_ref());
        if quota.remaining_amount < amount {
            let payload = new_quota_exhausted_denial(
                &input.tenant_id,
                &input.category,
                &input.operation_key,
                amount,
                quota.remaining_amount,
                &period,
            );
            let reservation = UsageReservation {
                reservation_id: format!("reservation_{}", input.operation_key),
                tenant_id: input.tenant_id.clone(),
                category: input.category.clone(),
                quota_period_id: period.quota_period_id.clone(),
                operation_key: input.operation_key.clone(),
                amount_reserved: amount,
                status: ReservationStatus::from(ReservationStatus::DENIED),
                reservation_point: input.reservation_point.clone(),
                created_at: now,
                updated_at: now,
                ..Default::default()
            };
            repo.save_reservation(reservation.clone()).await?;
            let denial = QuotaDenial {
                denial_id: format!("denial_{}", input.operation_key),
                tenant_id: input.tenant_id.clone(),
                category: input.category.clone(),
                quota_period_id: period.quota_period_id.clone(),
                operation_key: input.operation_key.clone(),
                reason_code: payload.reason_code.clone(),
                requested_amount: amount,
                remaining_amount: quota.remaining_amount,
                guarded_entry_point: input.guarded_entry_point.clone(),
                created_at: now,
            };
            repo.append_quota_denial(denial.clone()).await?;
            // Evidence append is best-effort on the denial path (Go: `_ =`).
            let _ = repo
                .append_usage_event(UsageEvent {
                    usage_event_id: format!("usage_event_denial_{}", input.operation_key),
                    tenant_id: input.tenant_id.clone(),
                    category: input.category.clone(),
                    quota_period_id: period.quota_period_id.clone(),
                    operation_key: input.operation_key.clone(),
                    event_kind: UsageEventKind::from(UsageEventKind::DENIAL),
                    amount,
                    reason_code: denial.reason_code.clone(),
                    outcome: "denied".to_string(),
                    created_at: now,
                    ..Default::default()
                })
                .await;
            return Ok(ReserveResult {
                allowed: false,
                reservation: Some(reservation),
                denial: Some(payload),
                quota: Some(quota),
                failure: Some(BillingError::QuotaDenied),
            });
        }
        let reservation = UsageReservation {
            reservation_id: format!("reservation_{}", input.operation_key),
            tenant_id: input.tenant_id.clone(),
            category: input.category.clone(),
            quota_period_id: period.quota_period_id.clone(),
            operation_key: input.operation_key.clone(),
            amount_reserved: amount,
            status: ReservationStatus::from(ReservationStatus::RESERVED),
            reservation_point: input.reservation_point.clone(),
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        let mut counter = counter;
        counter.reserved_amount += amount;
        counter.updated_at = now;
        repo.save_usage_counter(counter).await?;
        repo.save_reservation(reservation.clone()).await?;
        repo.append_usage_event(UsageEvent {
            usage_event_id: format!("usage_event_reserved_{}", input.operation_key),
            tenant_id: input.tenant_id.clone(),
            category: input.category.clone(),
            quota_period_id: period.quota_period_id.clone(),
            operation_key: input.operation_key.clone(),
            event_kind: UsageEventKind::from(UsageEventKind::RESERVATION),
            amount,
            reason_code: "usage_reserved".to_string(),
            actor_principal_id: input.actor_principal_id.clone(),
            outcome: "reserved".to_string(),
            created_at: now,
            ..Default::default()
        })
        .await?;
        Ok(ReserveResult {
            allowed: true,
            reservation: Some(reservation),
            quota: Some(quota),
            ..Default::default()
        })
    }

    /// Reserve several categories atomically: on any denial the earlier
    /// reservations are released. The failing result's denial/error is
    /// surfaced on [`ReserveAllResult`].
    pub async fn reserve_all(&self, inputs: Vec<ReserveInput>) -> Result<ReserveAllResult> {
        if let Some(repo) = self.repo() {
            let now = self.clock_now();
            if let Some(result) = repo.reserve_all_usage(inputs.clone(), now).await? {
                return Ok(result);
            }
        }
        let mut results = Vec::with_capacity(inputs.len());
        for input in inputs {
            let result = match self.reserve(input).await {
                Ok(result) => result,
                Err(err) => {
                    results.push(ReserveResult::default());
                    self.release_prior_reservations(&results).await;
                    return Ok(ReserveAllResult {
                        allowed: false,
                        results,
                        failure: Some(err),
                        ..Default::default()
                    });
                }
            };
            let failed = result.failure.is_some() || !result.allowed;
            results.push(result);
            if !failed {
                continue;
            }
            self.release_prior_reservations(&results).await;
            let last = results.last().cloned().unwrap_or_default();
            return Ok(ReserveAllResult {
                allowed: false,
                results,
                denial: last.denial,
                failure: last.failure,
            });
        }
        Ok(ReserveAllResult {
            allowed: true,
            results,
            ..Default::default()
        })
    }

    async fn release_prior_reservations(&self, results: &[ReserveResult]) {
        let Some(last) = results.len().checked_sub(1) else {
            return;
        };
        for prior in &results[..last] {
            let Some(reservation) = &prior.reservation else {
                continue;
            };
            let _ = self
                .release(ResolveInput {
                    tenant_id: reservation.tenant_id.clone(),
                    category: reservation.category.clone(),
                    operation_key: reservation.operation_key.clone(),
                    amount: reservation.amount_reserved,
                    reason_code: "billing.multi_category_reservation_released".to_string(),
                    reason: "multi-category reservation denied".to_string(),
                    ..Default::default()
                })
                .await;
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ReserveInput {
    pub tenant_id: String,
    pub category: Category,
    pub amount: i64,
    pub operation_key: String,
    pub reservation_point: String,
    pub guarded_entry_point: String,
    pub actor_principal_id: String,
    pub hosted: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ReserveResult {
    pub allowed: bool,
    pub reservation: Option<UsageReservation>,
    pub denial: Option<DenialPayload>,
    pub quota: Option<EffectiveQuota>,
    /// Denial-class failure (Go: non-nil `err` return): `QuotaDenied`,
    /// `QuotaStateUnavailable`, or `OperatorActionRequired`.
    pub failure: Option<BillingError>,
}

#[derive(Debug, Clone, Default)]
pub struct ReserveAllResult {
    pub allowed: bool,
    pub results: Vec<ReserveResult>,
    pub denial: Option<DenialPayload>,
    /// Failure of the first denied/failed category (Go: non-nil `err`).
    pub failure: Option<BillingError>,
}

/// Unlimited development plan used for non-hosted tenants without a
/// persisted plan.
#[must_use]
pub fn development_plan(tenant_id: &str, now: DateTime<Utc>) -> TenantPlan {
    TenantPlan {
        plan_id: format!("plan_{tenant_id}_development"),
        tenant_id: tenant_id.to_string(),
        plan_key: "development".to_string(),
        status: PlanStatus::from(PlanStatus::ACTIVE),
        enforcement_mode: EnforcementMode::from(EnforcementMode::UNLIMITED),
        effective_at: now,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::catalog::definition_for;
    use crate::fixtures::CLIENT;
    use crate::fixtures::FixtureRepo;
    use crate::fixtures::RUN_ID;
    use crate::fixtures::STEP_ID;
    use crate::fixtures::TEN_FINITE;
    use crate::fixtures::fixed_now;
    use crate::operation_key::integration_operation_key;
    use crate::operation_key::run_operation_key;
    use crate::operation_key::tool_call_operation_key;

    fn manager_and_repo() -> (Arc<FixtureRepo>, Manager) {
        let now = fixed_now();
        let repo = Arc::new(FixtureRepo::new(now));
        let manager = Manager::with_clock(repo.clone(), move || now);
        (repo, manager)
    }

    #[tokio::test]
    async fn reserve_commit_replay_is_idempotent() {
        let (_repo, manager) = manager_and_repo();
        let input = ReserveInput {
            tenant_id: TEN_FINITE.to_string(),
            category: Category::from(Category::RUN_LAUNCHES),
            amount: 1,
            operation_key: run_operation_key(TEN_FINITE, CLIENT, RUN_ID),
            reservation_point: "test".to_string(),
            guarded_entry_point: "POST /v1/runs".to_string(),
            hosted: true,
            ..Default::default()
        };
        let first = manager.reserve(input.clone()).await.unwrap();
        assert!(first.allowed, "{first:?}");
        let second = manager.reserve(input.clone()).await.unwrap();
        assert!(second.allowed, "{second:?}");
        assert_eq!(
            second.reservation.as_ref().unwrap().reservation_id,
            first.reservation.as_ref().unwrap().reservation_id
        );
        let resolve = ResolveInput {
            tenant_id: TEN_FINITE.to_string(),
            category: Category::from(Category::RUN_LAUNCHES),
            operation_key: input.operation_key.clone(),
            ..Default::default()
        };
        let committed = manager.commit(resolve.clone()).await.unwrap();
        assert_eq!(committed.status, ReservationStatus::COMMITTED);
        let committed_again = manager.commit(resolve).await.unwrap();
        assert_eq!(committed_again.amount_committed, committed.amount_committed);
    }

    #[tokio::test]
    async fn lifecycle_replay_is_idempotent() {
        let now = fixed_now();
        for status in [
            ReservationStatus::COMMITTED,
            ReservationStatus::REFUNDED,
            ReservationStatus::RELEASED,
        ] {
            let repo = Arc::new(FixtureRepo::new(now));
            let manager = Manager::with_clock(repo.clone(), move || now);
            let key = run_operation_key(TEN_FINITE, CLIENT, &format!("run_{status}"));
            let result = manager
                .reserve(ReserveInput {
                    tenant_id: TEN_FINITE.to_string(),
                    category: Category::from(Category::RUN_LAUNCHES),
                    amount: 1,
                    operation_key: key.clone(),
                    hosted: true,
                    ..Default::default()
                })
                .await
                .unwrap();
            assert!(result.allowed, "{status}: {result:?}");
            let input = ResolveInput {
                tenant_id: TEN_FINITE.to_string(),
                category: Category::from(Category::RUN_LAUNCHES),
                operation_key: key.clone(),
                ..Default::default()
            };
            let first = manager.resolve_to(input.clone(), status).await.unwrap();
            assert_eq!(first.status, status, "{status}");
            let second = manager.resolve_to(input, status).await.unwrap();
            assert_eq!(second.status, status, "{status} replay");

            let definition = definition_for(&Category::from(Category::RUN_LAUNCHES)).unwrap();
            let period = repo.open_period_sync(TEN_FINITE, &definition, now);
            let counter = repo
                .counter(
                    TEN_FINITE,
                    &Category::from(Category::RUN_LAUNCHES),
                    &period.quota_period_id,
                )
                .expect("counter");
            assert_eq!(
                counter.reserved_amount, 0,
                "{status}: reserved changed on replay"
            );
            if status == ReservationStatus::COMMITTED {
                assert_eq!(
                    counter.committed_amount, 1,
                    "{status}: commit double-counted"
                );
            } else {
                assert_eq!(
                    counter.committed_amount, 0,
                    "{status}: non-commit lifecycle committed usage"
                );
            }
        }
    }

    #[tokio::test]
    async fn denies_when_quota_exhausted() {
        let now = fixed_now();
        let (repo, manager) = manager_and_repo();
        let definition = definition_for(&Category::from(Category::RUN_LAUNCHES)).unwrap();
        let period = repo.open_period_sync(TEN_FINITE, &definition, now);
        repo.save_counter(UsageCounter {
            usage_counter_id: "counter_1".to_string(),
            tenant_id: TEN_FINITE.to_string(),
            category: Category::from(Category::RUN_LAUNCHES),
            quota_period_id: period.quota_period_id.clone(),
            committed_amount: definition.default_limit,
            updated_at: now,
            ..Default::default()
        });
        let result = manager
            .reserve(ReserveInput {
                tenant_id: TEN_FINITE.to_string(),
                category: Category::from(Category::RUN_LAUNCHES),
                amount: 1,
                operation_key: "op_over".to_string(),
                hosted: true,
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(
            matches!(result.failure, Some(BillingError::QuotaDenied)),
            "expected stable quota denial: {result:?}"
        );
        let denial = result.denial.as_ref().expect("denial payload");
        assert_eq!(denial.reason_code, "quota_denied:run_launches_exhausted");
        assert_eq!(repo.denial_count(), 1);
    }

    #[tokio::test]
    async fn denied_reservation_replay_is_idempotent() {
        let now = fixed_now();
        let (repo, manager) = manager_and_repo();
        let definition = definition_for(&Category::from(Category::RUN_LAUNCHES)).unwrap();
        let period = repo.open_period_sync(TEN_FINITE, &definition, now);
        repo.save_counter(UsageCounter {
            usage_counter_id: "counter_1".to_string(),
            tenant_id: TEN_FINITE.to_string(),
            category: Category::from(Category::RUN_LAUNCHES),
            quota_period_id: period.quota_period_id.clone(),
            committed_amount: definition.default_limit,
            updated_at: now,
            ..Default::default()
        });
        let input = ReserveInput {
            tenant_id: TEN_FINITE.to_string(),
            category: Category::from(Category::RUN_LAUNCHES),
            amount: 1,
            operation_key: "op_denied_replay".to_string(),
            hosted: true,
            ..Default::default()
        };
        let first = manager.reserve(input.clone()).await.unwrap();
        assert!(
            matches!(first.failure, Some(BillingError::QuotaDenied)),
            "{first:?}"
        );
        assert_eq!(
            first.reservation.as_ref().unwrap().status,
            ReservationStatus::DENIED
        );
        let second = manager.reserve(input).await.unwrap();
        assert!(
            matches!(second.failure, Some(BillingError::QuotaDenied)),
            "{second:?}"
        );
        assert_eq!(
            second.reservation.as_ref().unwrap().reservation_id,
            first.reservation.as_ref().unwrap().reservation_id
        );
        assert_eq!(
            repo.denial_count(),
            1,
            "expected one recorded denial after replay"
        );
    }

    #[tokio::test]
    async fn reserve_all_rolls_back_partial_reservations_on_denied_category() {
        let now = fixed_now();
        let (repo, manager) = manager_and_repo();
        let tool_definition =
            definition_for(&Category::from(Category::RUNTIME_TOOL_CALLS)).unwrap();
        let tool_period = repo.open_period_sync(TEN_FINITE, &tool_definition, now);
        repo.save_counter(UsageCounter {
            usage_counter_id: "counter_tool_exhausted".to_string(),
            tenant_id: TEN_FINITE.to_string(),
            category: Category::from(Category::RUNTIME_TOOL_CALLS),
            quota_period_id: tool_period.quota_period_id.clone(),
            committed_amount: tool_definition.default_limit,
            updated_at: now,
            ..Default::default()
        });

        let integration_key = integration_operation_key(TEN_FINITE, "mail", "op_1", "");
        let result = manager
            .reserve_all(vec![
                ReserveInput {
                    tenant_id: TEN_FINITE.to_string(),
                    category: Category::from(Category::INTEGRATION_OPERATIONS),
                    amount: 1,
                    operation_key: integration_key.clone(),
                    reservation_point: "integration preflight".to_string(),
                    guarded_entry_point: "mail operation".to_string(),
                    hosted: true,
                    ..Default::default()
                },
                ReserveInput {
                    tenant_id: TEN_FINITE.to_string(),
                    category: Category::from(Category::RUNTIME_TOOL_CALLS),
                    amount: 1,
                    operation_key: tool_call_operation_key(
                        TEN_FINITE, RUN_ID, STEP_ID, "tool_1", "",
                    ),
                    reservation_point: "tool call creation".to_string(),
                    guarded_entry_point: "tool call".to_string(),
                    hosted: true,
                    ..Default::default()
                },
            ])
            .await
            .unwrap();
        assert!(
            matches!(result.failure, Some(BillingError::QuotaDenied)),
            "expected multi-category denial: {result:?}"
        );
        assert!(!result.allowed);
        assert!(result.denial.is_some());

        let integration_definition =
            definition_for(&Category::from(Category::INTEGRATION_OPERATIONS)).unwrap();
        let integration_period = repo.open_period_sync(TEN_FINITE, &integration_definition, now);
        let counter = repo
            .counter(
                TEN_FINITE,
                &Category::from(Category::INTEGRATION_OPERATIONS),
                &integration_period.quota_period_id,
            )
            .expect("counter");
        assert_eq!(
            counter.reserved_amount, 0,
            "expected integration reservation rollback"
        );
        let reservation = repo
            .reservation(
                TEN_FINITE,
                &Category::from(Category::INTEGRATION_OPERATIONS),
                &integration_key,
            )
            .expect("reservation");
        assert_eq!(
            reservation.status,
            ReservationStatus::RELEASED,
            "expected rolled-back reservation to be released"
        );
    }

    #[tokio::test]
    async fn lowered_quota_denies_new_work_immediately() {
        let now = fixed_now();
        let (repo, manager) = manager_and_repo();
        let definition = definition_for(&Category::from(Category::RUN_LAUNCHES)).unwrap();
        let period = repo.open_period_sync(TEN_FINITE, &definition, now);
        repo.save_counter(UsageCounter {
            usage_counter_id: "counter_1".to_string(),
            tenant_id: TEN_FINITE.to_string(),
            category: Category::from(Category::RUN_LAUNCHES),
            quota_period_id: period.quota_period_id.clone(),
            committed_amount: 1,
            updated_at: now,
            ..Default::default()
        });
        manager
            .apply_quota_override(crate::types::QuotaOverride {
                quota_override_id: "override_lowered".to_string(),
                tenant_id: TEN_FINITE.to_string(),
                category: Category::from(Category::RUN_LAUNCHES),
                limit: Some(0),
                reason: "downgrade".to_string(),
                created_by_principal_id: "admin".to_string(),
                ..Default::default()
            })
            .await
            .unwrap();
        let result = manager
            .reserve(ReserveInput {
                tenant_id: TEN_FINITE.to_string(),
                category: Category::from(Category::RUN_LAUNCHES),
                amount: 1,
                operation_key: "op_after_lowered_quota".to_string(),
                hosted: true,
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(
            matches!(result.failure, Some(BillingError::QuotaDenied)),
            "expected lowered quota denial: {result:?}"
        );
        assert!(result.denial.is_some());
        assert!(result.quota.as_ref().unwrap().over_limit);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_last_unit_reservation() {
        let now = fixed_now();
        let repo = Arc::new(FixtureRepo::new(now));
        let manager = Arc::new(Manager::with_clock(repo, move || now));
        let mut handles = Vec::new();
        for i in 0..4_u8 {
            let manager = manager.clone();
            handles.push(tokio::spawn(async move {
                let suffix = char::from(b'a' + i);
                manager
                    .reserve(ReserveInput {
                        tenant_id: TEN_FINITE.to_string(),
                        category: Category::from(Category::RUN_LAUNCHES),
                        amount: 1,
                        operation_key: run_operation_key(
                            TEN_FINITE,
                            "",
                            &format!("run_last_{suffix}"),
                        ),
                        hosted: true,
                        ..Default::default()
                    })
                    .await
            }));
        }
        let mut allowed = 0;
        let mut denied = 0;
        for handle in handles {
            match handle.await.unwrap() {
                Ok(result) if result.allowed => allowed += 1,
                Ok(result) if matches!(result.failure, Some(BillingError::QuotaDenied)) => {
                    denied += 1
                }
                other => panic!("unexpected reserve outcome: {other:?}"),
            }
        }
        assert_eq!((allowed, denied), (1, 3));
    }
}
