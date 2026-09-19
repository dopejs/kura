//! Reservation lifecycle transitions: commit, refund, release, and
//! operator-action marking (port of `lifecycle.go`).

use crate::error::BillingError;
use crate::error::Result;
use crate::manager::Manager;
use crate::types::Category;
use crate::types::ReservationStatus;
use crate::types::UsageEvent;
use crate::types::UsageEventKind;
use crate::types::UsageReservation;

#[derive(Debug, Clone, Default)]
pub struct ResolveInput {
    pub tenant_id: String,
    pub category: Category,
    pub operation_key: String,
    pub amount: i64,
    pub reason_code: String,
    pub reason: String,
    pub actor_principal_id: String,
}

impl Manager {
    pub async fn commit(&self, input: ResolveInput) -> Result<UsageReservation> {
        self.resolve(input, ReservationStatus::COMMITTED, UsageEventKind::COMMIT)
            .await
    }

    pub async fn refund(&self, input: ResolveInput) -> Result<UsageReservation> {
        self.resolve(input, ReservationStatus::REFUNDED, UsageEventKind::REFUND)
            .await
    }

    pub async fn release(&self, input: ResolveInput) -> Result<UsageReservation> {
        self.resolve(input, ReservationStatus::RELEASED, UsageEventKind::RELEASE)
            .await
    }

    pub async fn mark_operator_action_needed(
        &self,
        input: ResolveInput,
    ) -> Result<UsageReservation> {
        if input.reason.is_empty() {
            return Err(BillingError::ReasonRequired);
        }
        self.resolve(
            input,
            ReservationStatus::OPERATOR_ACTION_NEEDED,
            UsageEventKind::RECOVERY_DECISION,
        )
        .await
    }

    /// Test helper resolving to an arbitrary status with the event kind
    /// implied by it (Go: the parameterized `resolve` call).
    #[cfg(test)]
    pub(crate) async fn resolve_to(
        &self,
        input: ResolveInput,
        status: &'static str,
    ) -> Result<UsageReservation> {
        let event_kind = match status {
            ReservationStatus::COMMITTED => UsageEventKind::COMMIT,
            ReservationStatus::REFUNDED => UsageEventKind::REFUND,
            ReservationStatus::RELEASED => UsageEventKind::RELEASE,
            _ => UsageEventKind::RECOVERY_DECISION,
        };
        self.resolve(input, status, event_kind).await
    }

    /// Shared lifecycle transition. Idempotent: replaying the same target
    /// status returns the stored reservation unchanged.
    async fn resolve(
        &self,
        input: ResolveInput,
        status: &'static str,
        event_kind: &'static str,
    ) -> Result<UsageReservation> {
        let Some(repo) = self.repo() else {
            return Ok(UsageReservation::default());
        };
        if let Some(reservation) = repo
            .resolve_usage(
                input.clone(),
                ReservationStatus::from(status),
                UsageEventKind::from(event_kind),
                self.clock_now(),
            )
            .await?
        {
            return Ok(reservation);
        }
        let _guard = self.mu_lock().await;
        let now = self.clock_now();
        let Some(mut reservation) = repo
            .reservation_by_operation(&input.tenant_id, &input.category, &input.operation_key)
            .await?
        else {
            return Err(BillingError::ReservationNotFound(
                input.operation_key.clone(),
            ));
        };
        if reservation.status == status {
            return Ok(reservation);
        }
        let Some(mut counter) = repo
            .usage_counter(
                &input.tenant_id,
                &input.category,
                &reservation.quota_period_id,
            )
            .await?
        else {
            return Err(BillingError::CounterNotFound(
                reservation.reservation_id.clone(),
            ));
        };
        let amount = if input.amount <= 0 {
            reservation.amount_reserved
        } else {
            input.amount
        };
        match status {
            ReservationStatus::COMMITTED => {
                let delta = amount - reservation.amount_committed;
                let reserved_release = counter
                    .reserved_amount
                    .min(reservation.amount_reserved - reservation.amount_refunded);
                counter.reserved_amount -= reserved_release;
                counter.committed_amount += delta;
                reservation.amount_committed = amount;
                if amount < reservation.amount_reserved {
                    reservation.amount_refunded += reservation.amount_reserved - amount;
                }
            }
            ReservationStatus::REFUNDED | ReservationStatus::RELEASED => {
                let refund = counter
                    .reserved_amount
                    .min(reservation.amount_reserved - reservation.amount_refunded);
                counter.reserved_amount -= refund;
                reservation.amount_refunded += refund;
            }
            ReservationStatus::OPERATOR_ACTION_NEEDED => {
                reservation.recovery_reason = input.reason.clone();
            }
            _ => {}
        }
        counter.updated_at = now;
        reservation.status = ReservationStatus::from(status);
        reservation.updated_at = now;
        repo.save_usage_counter(counter).await?;
        repo.save_reservation(reservation.clone()).await?;
        let reason_code = if input.reason_code.is_empty() {
            event_kind.to_string()
        } else {
            input.reason_code.clone()
        };
        repo.append_usage_event(UsageEvent {
            usage_event_id: format!("usage_event_{event_kind}_{}", input.operation_key),
            tenant_id: input.tenant_id.clone(),
            category: input.category.clone(),
            quota_period_id: reservation.quota_period_id.clone(),
            operation_key: input.operation_key.clone(),
            event_kind: UsageEventKind::from(event_kind),
            amount,
            reason_code,
            reason: input.reason.clone(),
            actor_principal_id: input.actor_principal_id.clone(),
            outcome: status.to_string(),
            created_at: now,
            ..Default::default()
        })
        .await?;
        Ok(reservation)
    }
}
