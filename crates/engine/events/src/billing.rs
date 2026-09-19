//! Billing usage, quota-denial, and reservation-recovery events (port of
//! `billing.go`).

use crate::util::{is_go_zero_time, now_utc, payload};
use crate::{Event, Resource};
use kura_billing::{QuotaDenial, RecoveryDecision, UsageEvent};

pub const BILLING_USAGE_RESERVED_NAME: &str = "billing.usage_reserved";
pub const BILLING_USAGE_COMMITTED_NAME: &str = "billing.usage_committed";
pub const BILLING_USAGE_REFUNDED_NAME: &str = "billing.usage_refunded";
pub const BILLING_QUOTA_DENIED_NAME: &str = "billing.quota_denied";
pub const BILLING_MANUAL_ADJUSTMENT_CREATED_NAME: &str = "billing.manual_adjustment_created";
pub const BILLING_RESERVATION_RECOVERY_DECIDED_NAME: &str = "billing.reservation_recovery_decided";

/// Go: `BillingUsageEvent` — projects a metered usage event onto the bus.
#[must_use]
pub fn billing_usage_event(name: &str, event: UsageEvent) -> Event {
    let occurred_at = if is_go_zero_time(event.created_at) {
        now_utc()
    } else {
        event.created_at
    };
    Event {
        tenant_id: event.tenant_id.clone(),
        category: "billing".to_string(),
        name: name.to_string(),
        occurred_at,
        resource: Resource {
            kind: "billing_usage_event".to_string(),
            id: event.usage_event_id.clone(),
        },
        payload: payload![
            "tenantId" => event.tenant_id,
            "category" => event.category.as_str(),
            "quotaPeriodId" => event.quota_period_id,
            "operationKey" => event.operation_key,
            "amount" => event.amount,
            "reasonCode" => event.reason_code,
            "outcome" => event.outcome,
        ],
        ..Event::default()
    }
}

/// Go: `BillingQuotaDeniedEvent`.
#[must_use]
pub fn billing_quota_denied_event(denial: QuotaDenial) -> Event {
    let occurred_at = if is_go_zero_time(denial.created_at) {
        now_utc()
    } else {
        denial.created_at
    };
    Event {
        tenant_id: denial.tenant_id.clone(),
        category: "billing".to_string(),
        name: BILLING_QUOTA_DENIED_NAME.to_string(),
        occurred_at,
        resource: Resource {
            kind: "billing_denial".to_string(),
            id: denial.denial_id.clone(),
        },
        payload: payload![
            "tenantId" => denial.tenant_id,
            "category" => denial.category.as_str(),
            "quotaPeriodId" => denial.quota_period_id,
            "operationKey" => denial.operation_key,
            "reasonCode" => denial.reason_code,
            "requestedAmount" => denial.requested_amount,
            "remainingAmount" => denial.remaining_amount,
        ],
        ..Event::default()
    }
}

/// Go: `BillingRecoveryDecisionEvent` — the decision outcome and reason
/// fall back to the reservation's recorded values.
#[must_use]
pub fn billing_recovery_decision_event(decision: RecoveryDecision) -> Event {
    let reservation = &decision.reservation;
    let occurred_at = if is_go_zero_time(reservation.updated_at) {
        now_utc()
    } else {
        reservation.updated_at
    };
    let reason = if decision.reason.is_empty() {
        reservation.recovery_reason.clone()
    } else {
        decision.reason.clone()
    };
    let outcome = if decision.outcome.is_empty() {
        reservation.status.clone()
    } else {
        decision.outcome.clone()
    };
    Event {
        tenant_id: reservation.tenant_id.clone(),
        category: "billing".to_string(),
        name: BILLING_RESERVATION_RECOVERY_DECIDED_NAME.to_string(),
        occurred_at,
        resource: Resource {
            kind: "billing_reservation".to_string(),
            id: reservation.reservation_id.clone(),
        },
        payload: payload![
            "tenantId" => reservation.tenant_id,
            "category" => reservation.category.as_str(),
            "quotaPeriodId" => reservation.quota_period_id,
            "operationKey" => reservation.operation_key,
            "outcome" => outcome.as_str(),
            "reason" => reason,
        ],
        ..Event::default()
    }
}
