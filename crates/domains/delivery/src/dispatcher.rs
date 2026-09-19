//! Port of `daemon/internal/delivery/dispatcher.go`: immediate dispatch, attempt ledger,
//! retry scheduling, and failure projection. Go runs `scheduleRetry` in a goroutine; the Rust
//! port spawns a detached std thread that sleeps until the retry time and resumes the
//! outcome. The `retryScheduled` map is a dedupe guard exactly like the Go one.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::manager::{DeliveryError, ManagerInner, new_attempt_id, non_empty};
use crate::{
    AttemptStatus, DeliveryAttempt, DeliveryOutcome, DeliveryTarget, OutcomeStatus, TargetStatus,
};

impl ManagerInner {
    /// Port of `dispatchImmediate`.
    pub(crate) fn dispatch_immediate(
        self: &Arc<Self>,
        outcome: DeliveryOutcome,
        target: &DeliveryTarget,
    ) -> Result<DeliveryOutcome, DeliveryError> {
        let attempt_number = self.next_attempt_number(&outcome.delivery_id);
        self.dispatch_attempt(outcome, target, attempt_number)
    }

    /// Port of `dispatchAttempt`: records the outcome as dispatching, gates on target status /
    /// connector enablement / adapter availability, runs one adapter send, and advances the
    /// attempt + outcome statuses.
    pub(crate) fn dispatch_attempt(
        self: &Arc<Self>,
        mut outcome: DeliveryOutcome,
        target: &DeliveryTarget,
        attempt_number: i64,
    ) -> Result<DeliveryOutcome, DeliveryError> {
        let now = Utc::now();
        outcome.status = OutcomeStatus::Dispatching;
        outcome.updated_at = now;
        self.store_outcome(&outcome)?;

        if target.status != TargetStatus::Active {
            return self.fail_outcome_without_retry(
                outcome,
                &target.target_id,
                attempt_number,
                "target_unavailable",
                &format!("target {} is {}", target.target_id, target.status),
            );
        }
        if self.connector_delivery_disabled(target)? {
            let connector_id = target
                .connector_binding
                .as_ref()
                .map(|b| b.connector_id.clone())
                .unwrap_or_default();
            let failed = self.fail_outcome_without_retry(
                outcome,
                &target.target_id,
                attempt_number,
                "connector_disabled",
                &format!("connector {connector_id} is disabled"),
            )?;
            self.record_channel_background_delivery_outcome(&failed, target, "connector_disabled")?;
            return Ok(failed);
        }

        let adapter = self.adapter_for(target.target_kind);
        let Some(adapter) = adapter else {
            return self.fail_outcome_without_retry(
                outcome,
                &target.target_id,
                attempt_number,
                "adapter_unavailable",
                &format!("no adapter registered for {}", target.target_id),
            );
        };

        let mut attempt = DeliveryAttempt {
            attempt_id: new_attempt_id(),
            delivery_id: outcome.delivery_id.clone(),
            attempt_number,
            target_id: target.target_id.clone(),
            transport_kind: target.target_kind.as_str().to_string(),
            status: AttemptStatus::Running,
            started_at: now,
            ..DeliveryAttempt::default()
        };
        self.store_attempt(&attempt)?;

        let send_result = adapter.send(target.clone(), outcome.clone());
        let completed_at = Utc::now();
        match send_result {
            Err(send_err) => {
                attempt.completed_at = Some(completed_at);
                // Go records the caller-side attempt's failure class (empty at this point:
                // handleAttemptFailure mutates its own copy), so capture it before the move.
                let failure_class = attempt.failure_class.clone();
                let failed = self.handle_attempt_failure(outcome, attempt, &send_err)?;
                self.record_channel_background_delivery_outcome(&failed, target, &failure_class)?;
                Ok(failed)
            }
            Ok(result) => {
                attempt.transport_kind = non_empty(&result.transport_kind, &attempt.transport_kind);
                attempt.transport_receipt_summary = result.receipt_summary.clone();
                attempt.connector_message_delivery_id =
                    result.connector_message_delivery_id.clone();
                attempt.completed_at = Some(completed_at);
                attempt.status = AttemptStatus::Delivered;
                self.store_attempt(&attempt)?;
                self.publish_attempt_recorded(&outcome, &attempt)?;

                outcome.status = OutcomeStatus::Delivered;
                outcome.updated_at = completed_at;
                outcome.finalized_at = Some(completed_at);
                self.store_outcome(&outcome)?;
                self.publish_outcome_status_changed(&outcome)?;
                self.record_channel_background_delivery_outcome(&outcome, target, "delivered")?;
                self.clear_retry_schedule(&outcome.delivery_id);
                self.attach_attempts(outcome)
            }
        }
    }

    /// Port of `handleAttemptFailure`: retryable below `maxAttempts` (outcome queued with a
    /// scheduled retry), terminal failure at/above it (outcome failed with a diagnostic).
    pub(crate) fn handle_attempt_failure(
        self: &Arc<Self>,
        outcome: DeliveryOutcome,
        mut attempt: DeliveryAttempt,
        send_err: &str,
    ) -> Result<DeliveryOutcome, DeliveryError> {
        let completed_at = Utc::now();
        attempt.failure_class = "transport_failed".to_string();
        attempt.failure_reason = send_err.to_string();
        let max_attempts = self.config.lock().max_attempts;
        if attempt.attempt_number < max_attempts {
            let next_retry_at = completed_at + self.retry_delay_for_attempt(attempt.attempt_number);
            attempt.status = AttemptStatus::RetryableFailure;
            attempt.next_retry_at = Some(next_retry_at);
            self.store_attempt(&attempt)?;
            self.publish_attempt_recorded(&outcome, &attempt)?;
            let mut outcome = outcome;
            outcome.status = OutcomeStatus::Queued;
            outcome.updated_at = completed_at;
            self.store_outcome(&outcome)?;
            self.publish_outcome_status_changed(&outcome)?;
            self.schedule_retry(&outcome.delivery_id, next_retry_at);
            self.attach_attempts(outcome)
        } else {
            attempt.status = AttemptStatus::TerminalFailure;
            attempt.failure_class = "retry_exhausted".to_string();
            attempt.next_retry_at = None;
            self.store_attempt(&attempt)?;
            self.publish_attempt_recorded(&outcome, &attempt)?;
            let mut outcome = outcome;
            outcome.status = OutcomeStatus::Failed;
            outcome.updated_at = completed_at;
            outcome.finalized_at = Some(completed_at);
            outcome.diagnostic_failure = Some(delivery_diagnostic_failure(
                &outcome,
                &attempt.failure_class,
                &attempt.failure_reason,
                true,
                completed_at,
            ));
            if outcome.payload_preview.trim().is_empty() {
                outcome.payload_preview = send_err.to_string();
            }
            self.store_outcome(&outcome)?;
            self.publish_outcome_status_changed(&outcome)?;
            self.clear_retry_schedule(&outcome.delivery_id);
            self.attach_attempts(outcome)
        }
    }

    /// Port of `failOutcomeWithoutRetry`: immediate terminal failure with a diagnostic
    /// projection (no retry is scheduled).
    pub(crate) fn fail_outcome_without_retry(
        &self,
        mut outcome: DeliveryOutcome,
        target_id: &str,
        attempt_number: i64,
        failure_class: &str,
        failure_reason: &str,
    ) -> Result<DeliveryOutcome, DeliveryError> {
        let now = Utc::now();
        let attempt = DeliveryAttempt {
            attempt_id: new_attempt_id(),
            delivery_id: outcome.delivery_id.clone(),
            attempt_number,
            target_id: target_id.to_string(),
            // Go: nonEmpty(string(outcome.Mode), "unknown"); the Rust enum is never empty.
            transport_kind: outcome.mode.as_str().to_string(),
            status: AttemptStatus::TerminalFailure,
            failure_class: failure_class.to_string(),
            failure_reason: failure_reason.to_string(),
            started_at: now,
            completed_at: Some(now),
            ..DeliveryAttempt::default()
        };
        self.store_attempt(&attempt)?;
        self.publish_attempt_recorded(&outcome, &attempt)?;
        outcome.status = OutcomeStatus::Failed;
        outcome.updated_at = now;
        outcome.finalized_at = Some(now);
        outcome.diagnostic_failure = Some(delivery_diagnostic_failure(
            &outcome,
            failure_class,
            failure_reason,
            false,
            now,
        ));
        if outcome.payload_preview.trim().is_empty() {
            outcome.payload_preview = failure_reason.to_string();
        }
        self.store_outcome(&outcome)?;
        self.publish_outcome_status_changed(&outcome)?;
        self.clear_retry_schedule(&outcome.delivery_id);
        self.attach_attempts(outcome)
    }

    /// Port of `nextAttemptNumber`.
    pub(crate) fn next_attempt_number(&self, delivery_id: &str) -> i64 {
        let attempts = match self.store.lock().list_delivery_attempts(delivery_id) {
            Ok(items) => items,
            Err(_) => return 1,
        };
        let mut max_attempt = 0;
        for item in &attempts {
            if item.attempt_number > max_attempt {
                max_attempt = item.attempt_number;
            }
        }
        max_attempt + 1
    }

    /// Port of `retryDelayForAttempt`: exponential backoff capped at `maxRetryDelay`.
    pub(crate) fn retry_delay_for_attempt(&self, attempt_number: i64) -> Duration {
        let config = self.config.lock();
        let mut delay = config.base_retry_delay;
        if delay <= Duration::ZERO {
            delay = Duration::from_secs(5);
        }
        if attempt_number > 1 {
            for _ in 1..attempt_number {
                delay = delay.saturating_mul(2);
                if config.max_retry_delay > Duration::ZERO && delay >= config.max_retry_delay {
                    return config.max_retry_delay;
                }
            }
        }
        if config.max_retry_delay > Duration::ZERO && delay > config.max_retry_delay {
            return config.max_retry_delay;
        }
        delay
    }

    /// Port of `scheduleRetry`: dedupes on `retryScheduled`, then spawns a detached thread
    /// that sleeps until `when` and resumes the outcome.
    pub(crate) fn schedule_retry(self: &Arc<Self>, delivery_id: &str, when: DateTime<Utc>) {
        if delivery_id.trim().is_empty() {
            return;
        }
        {
            let mut schedules = self.schedules.lock();
            if schedules.retry_scheduled.contains_key(delivery_id) {
                return;
            }
            schedules
                .retry_scheduled
                .insert(delivery_id.to_string(), ());
        }
        let inner = Arc::clone(self);
        let delivery_id = delivery_id.to_string();
        std::thread::spawn(move || {
            let delay = (when - Utc::now()).to_std().unwrap_or(Duration::ZERO);
            if delay > Duration::ZERO {
                std::thread::sleep(delay);
            }
            let _ = inner.resume_outcome(&delivery_id);
        });
    }

    /// Port of `clearRetrySchedule`.
    pub(crate) fn clear_retry_schedule(&self, delivery_id: &str) {
        self.schedules.lock().retry_scheduled.remove(delivery_id);
    }

    /// Port of `resumeOutcome`: clears the schedule, then dispatches another attempt if the
    /// outcome is still queued/dispatching and the chosen target still exists.
    pub(crate) fn resume_outcome(self: &Arc<Self>, delivery_id: &str) -> Result<(), DeliveryError> {
        let inner_result = self.resume_outcome_inner(delivery_id);
        self.clear_retry_schedule(delivery_id);
        inner_result
    }

    pub(crate) fn resume_outcome_inner(
        self: &Arc<Self>,
        delivery_id: &str,
    ) -> Result<(), DeliveryError> {
        let (outcome, ok) = self.get_outcome(delivery_id)?;
        if !ok {
            return Ok(());
        }
        if outcome.status != OutcomeStatus::Queued && outcome.status != OutcomeStatus::Dispatching {
            return Ok(());
        }
        let chosen_target_id = outcome.chosen_target_id.clone();
        let (target, ok) = self.get_target(&chosen_target_id)?;
        if !ok {
            let next = self.next_attempt_number(&outcome.delivery_id);
            self.fail_outcome_without_retry(
                outcome,
                &chosen_target_id,
                next,
                "target_missing",
                "configured target is missing",
            )?;
            return Ok(());
        }
        let next = self.next_attempt_number(&outcome.delivery_id);
        self.dispatch_attempt(outcome, &target, next)?;
        Ok(())
    }
}

/// Port of `deliveryDiagnosticFailure`: project the operation failure through the
/// integrations classifier (domain "delivery").
fn delivery_diagnostic_failure(
    outcome: &DeliveryOutcome,
    failure_class: &str,
    failure_reason: &str,
    side_effecting: bool,
    checked_at: DateTime<Utc>,
) -> kura_integrations::DiagnosticFailureProjection {
    kura_integrations::diagnostic_failure_for_operation_failure(
        "delivery",
        "",
        &outcome.integration_id,
        outcome.mode.as_str(),
        failure_class,
        failure_reason,
        side_effecting,
        checked_at,
    )
}
