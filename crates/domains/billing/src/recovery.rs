//! Crash/restart recovery for pending reservations (port of `recovery.go`).

use crate::error::Result;
use crate::lifecycle::ResolveInput;
use crate::manager::Manager;
use crate::types::ReservationStatus;
use crate::types::UsageReservation;

#[derive(Debug, Clone)]
pub struct RecoveryDecision {
    pub reservation: UsageReservation,
    pub outcome: ReservationStatus,
    pub reason: String,
}

impl Manager {
    /// Resolve every pending reservation with the caller's decision;
    /// ambiguous outcomes default to operator action needed.
    pub async fn recover_pending_reservations<F>(
        &self,
        decide: Option<F>,
    ) -> Result<Vec<RecoveryDecision>>
    where
        F: Fn(&UsageReservation) -> RecoveryDecision,
    {
        let Some(repo) = self.repo() else {
            return Ok(Vec::new());
        };
        let items = repo.list_pending_reservations().await?;
        let mut out = Vec::with_capacity(items.len());
        for reservation in items {
            let decision = match &decide {
                Some(decide) => {
                    let mut decision = decide(&reservation);
                    if decision.reservation.reservation_id.is_empty() {
                        decision.reservation = reservation.clone();
                    }
                    if decision.outcome.is_empty() {
                        decision.outcome =
                            ReservationStatus::from(ReservationStatus::OPERATOR_ACTION_NEEDED);
                    }
                    decision
                }
                None => RecoveryDecision {
                    reservation: reservation.clone(),
                    outcome: ReservationStatus::from(ReservationStatus::OPERATOR_ACTION_NEEDED),
                    reason: "restart outcome could not be proven".to_string(),
                },
            };
            let input = ResolveInput {
                tenant_id: reservation.tenant_id.clone(),
                category: reservation.category.clone(),
                operation_key: reservation.operation_key.clone(),
                amount: reservation.amount_reserved,
                reason_code: "billing.reservation_recovery_decided".to_string(),
                reason: decision.reason.clone(),
                ..Default::default()
            };
            match decision.outcome.as_str() {
                ReservationStatus::COMMITTED => {
                    self.commit(input).await?;
                }
                ReservationStatus::RELEASED => {
                    self.release(input).await?;
                }
                ReservationStatus::REFUNDED => {
                    self.refund(input).await?;
                }
                _ => {
                    self.mark_operator_action_needed(input).await?;
                }
            }
            out.push(decision);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::catalog::definition_for;
    use crate::error::BillingError;
    use crate::fixtures::FixtureRepo;
    use crate::fixtures::TEN_FINITE;
    use crate::fixtures::fixed_now;
    use crate::manager::ReserveInput;
    use crate::types::Category;
    use crate::types::UsageCounter;

    fn seed_pending(
        repo: &FixtureRepo,
        now: chrono::DateTime<chrono::Utc>,
        category: &'static str,
        keys: &[&str],
    ) -> crate::types::QuotaPeriod {
        let category = Category::from(category);
        let definition = definition_for(&category).unwrap();
        let period = repo.open_period_sync(TEN_FINITE, &definition, now);
        repo.save_counter(UsageCounter {
            usage_counter_id: "counter_recovery".to_string(),
            tenant_id: TEN_FINITE.to_string(),
            category: category.clone(),
            quota_period_id: period.quota_period_id.clone(),
            reserved_amount: keys.len() as i64,
            updated_at: now,
            ..Default::default()
        });
        for key in keys {
            repo.save_reservation_sync(UsageReservation {
                reservation_id: format!("reservation_{key}"),
                tenant_id: TEN_FINITE.to_string(),
                category: category.clone(),
                quota_period_id: period.quota_period_id.clone(),
                operation_key: (*key).to_string(),
                amount_reserved: 1,
                status: ReservationStatus::from(ReservationStatus::RESERVED),
                created_at: now,
                updated_at: now,
                ..Default::default()
            });
        }
        period
    }

    #[tokio::test]
    async fn recover_pending_reservations_applies_decisions() {
        let now = fixed_now();
        let repo = Arc::new(FixtureRepo::new(now));
        let manager = Manager::with_clock(repo.clone(), move || now);
        let period = seed_pending(
            &repo,
            now,
            Category::INTEGRATION_OPERATIONS,
            &["op_commit", "op_release", "op_refund", "op_operator"],
        );

        let decisions = manager
            .recover_pending_reservations(Some(|reservation: &UsageReservation| {
                let (outcome, reason) = match reservation.operation_key.as_str() {
                    "op_commit" => (ReservationStatus::COMMITTED, "commit proven"),
                    "op_release" => (ReservationStatus::RELEASED, "not started"),
                    "op_refund" => (ReservationStatus::REFUNDED, "canceled"),
                    _ => (ReservationStatus::OPERATOR_ACTION_NEEDED, "ambiguous"),
                };
                RecoveryDecision {
                    reservation: reservation.clone(),
                    outcome: ReservationStatus::from(outcome),
                    reason: reason.to_string(),
                }
            }))
            .await
            .unwrap();
        assert_eq!(decisions.len(), 4);

        let counter = repo
            .counter(
                TEN_FINITE,
                &Category::from(Category::INTEGRATION_OPERATIONS),
                &period.quota_period_id,
            )
            .expect("counter");
        assert_eq!(
            counter.committed_amount, 1,
            "unexpected recovered counter: {counter:?}"
        );
        assert_eq!(
            counter.reserved_amount, 1,
            "unexpected recovered counter: {counter:?}"
        );

        let want: [(&str, &str); 4] = [
            ("op_commit", ReservationStatus::COMMITTED),
            ("op_release", ReservationStatus::RELEASED),
            ("op_refund", ReservationStatus::REFUNDED),
            ("op_operator", ReservationStatus::OPERATOR_ACTION_NEEDED),
        ];
        for (key, status) in want {
            let reservation = repo
                .reservation(
                    TEN_FINITE,
                    &Category::from(Category::INTEGRATION_OPERATIONS),
                    key,
                )
                .expect("reservation");
            assert_eq!(reservation.status, status, "{key}");
        }
    }

    #[tokio::test]
    async fn recover_pending_reservations_defaults_ambiguous_to_operator_action_needed() {
        let now = fixed_now();
        let repo = Arc::new(FixtureRepo::new(now));
        let manager = Manager::with_clock(repo.clone(), move || now);
        seed_pending(&repo, now, Category::RUN_LAUNCHES, &["op_ambiguous"]);

        manager
            .recover_pending_reservations(None::<fn(&UsageReservation) -> RecoveryDecision>)
            .await
            .unwrap();
        let reservation = repo
            .reservation(
                TEN_FINITE,
                &Category::from(Category::RUN_LAUNCHES),
                "op_ambiguous",
            )
            .expect("reservation");
        assert_eq!(
            reservation.status,
            ReservationStatus::OPERATOR_ACTION_NEEDED
        );
        assert!(!reservation.recovery_reason.is_empty());

        let result = manager
            .reserve(ReserveInput {
                tenant_id: TEN_FINITE.to_string(),
                category: Category::from(Category::RUN_LAUNCHES),
                operation_key: "op_ambiguous".to_string(),
                hosted: true,
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(
            matches!(result.failure, Some(BillingError::OperatorActionRequired)),
            "expected duplicate work denial until operator resolution: {result:?}"
        );
        assert!(result.denial.is_some());
        assert_eq!(
            result.reservation.as_ref().unwrap().status,
            ReservationStatus::OPERATOR_ACTION_NEEDED
        );
    }
}
