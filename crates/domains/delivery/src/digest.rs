//! Port of `daemon/internal/delivery/digest.go`: summary-window batching for routine-success
//! results, window emission, and startup restore. Go runs `scheduleWindow` in a goroutine; the
//! Rust port spawns a detached std thread, mirroring `scheduleRetry`.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;

use crate::manager::{
    DeliveryError, ManagerInner, new_delivery_id, new_summary_window_id, payload_map,
};
use crate::{
    DeliveryMode, DeliveryOutcome, DeliveryPreference, DeliveryTarget, OutcomeStatus, ResultClass,
    SummaryWindow, SummaryWindowStatus,
};

impl ManagerInner {
    /// Port of `queueDigestOutcome`: attach the outcome to the open summary window for the
    /// preference/target pair and schedule the window emission.
    pub(crate) fn queue_digest_outcome(
        self: &Arc<Self>,
        mut outcome: DeliveryOutcome,
        pref: &DeliveryPreference,
        target: &DeliveryTarget,
    ) -> Result<DeliveryOutcome, DeliveryError> {
        let window = self.find_or_create_open_window(pref, target)?;
        let now = Utc::now();
        outcome.status = OutcomeStatus::Queued;
        outcome.summary_window_id = window.summary_window_id.clone();
        outcome.updated_at = now;
        self.store_outcome(&outcome)?;
        self.schedule_window(&window.summary_window_id, window.window_ends_at);
        self.attach_attempts(outcome)
    }

    /// Port of `findOrCreateOpenWindow`: reuse the still-open window for the same
    /// preference/target pair (incrementing its result count), otherwise create one with a
    /// default 15-minute window.
    pub(crate) fn find_or_create_open_window(
        &self,
        pref: &DeliveryPreference,
        target: &DeliveryTarget,
    ) -> Result<SummaryWindow, DeliveryError> {
        let items = self.list_summary_windows()?;
        let now = Utc::now();
        for item in items {
            if item.preference_id == pref.preference_id
                && item.target_id == target.target_id
                && item.status == SummaryWindowStatus::Open
                && item.window_ends_at > now
            {
                let mut item = item;
                item.result_count += 1;
                item.updated_at = now;
                self.store_window(&item)?;
                return Ok(item);
            }
        }
        let mut window_minutes = pref
            .summary_policy
            .as_ref()
            .map(|p| p.window_minutes)
            .unwrap_or(0);
        if window_minutes <= 0 {
            window_minutes = 15;
        }
        let window = SummaryWindow {
            summary_window_id: new_summary_window_id(),
            environment_scope: self.environment_scope.clone(),
            target_id: target.target_id.clone(),
            preference_id: pref.preference_id.clone(),
            status: SummaryWindowStatus::Open,
            window_started_at: now,
            window_ends_at: now + chrono::Duration::minutes(window_minutes),
            result_count: 1,
            created_at: now,
            updated_at: now,
            ..SummaryWindow::default()
        };
        self.store_window(&window)?;
        Ok(window)
    }

    /// Port of `scheduleWindow`: dedupes on `windowScheduled`, then spawns a detached thread
    /// that sleeps until `when` and emits the window.
    pub(crate) fn schedule_window(
        self: &Arc<Self>,
        summary_window_id: &str,
        when: chrono::DateTime<Utc>,
    ) {
        if summary_window_id.trim().is_empty() {
            return;
        }
        {
            let mut schedules = self.schedules.lock();
            if schedules.window_scheduled.contains_key(summary_window_id) {
                return;
            }
            schedules
                .window_scheduled
                .insert(summary_window_id.to_string(), ());
        }
        let inner = Arc::clone(self);
        let summary_window_id = summary_window_id.to_string();
        std::thread::spawn(move || {
            let delay = (when - Utc::now()).to_std().unwrap_or(Duration::ZERO);
            if delay > Duration::ZERO {
                std::thread::sleep(delay);
            }
            let _ = inner.emit_window(&summary_window_id);
        });
    }

    /// Port of `clearWindowSchedule`. Public on [`crate::Manager`] for tests that advance the
    /// window clock manually.
    pub(crate) fn clear_window_schedule(&self, summary_window_id: &str) {
        self.schedules
            .lock()
            .window_scheduled
            .remove(summary_window_id);
    }

    /// Port of `emitWindow`: emits the batched digest delivery once the window has elapsed.
    /// A still-open window reschedules itself; an empty window is cancelled; windows already
    /// delivered/failed/cancelled are no-ops.
    pub(crate) fn emit_window(
        self: &Arc<Self>,
        summary_window_id: &str,
    ) -> Result<(), DeliveryError> {
        let inner_result = self.emit_window_inner(summary_window_id);
        self.clear_window_schedule(summary_window_id);
        inner_result
    }

    pub(crate) fn emit_window_inner(
        self: &Arc<Self>,
        summary_window_id: &str,
    ) -> Result<(), DeliveryError> {
        let (mut window, ok) = self.get_summary_window(summary_window_id)?;
        if !ok {
            return Ok(());
        }
        let now = Utc::now();
        if window.status == SummaryWindowStatus::Open && window.window_ends_at > now {
            self.schedule_window(&window.summary_window_id, window.window_ends_at);
            return Ok(());
        }
        if window.result_count <= 0 {
            window.status = SummaryWindowStatus::Cancelled;
            window.updated_at = now;
            return self.store_window(&window);
        }
        if window.status != SummaryWindowStatus::Open
            && window.status != SummaryWindowStatus::Ready
            && window.status != SummaryWindowStatus::Dispatching
        {
            return Ok(());
        }

        window.status = SummaryWindowStatus::Dispatching;
        window.updated_at = now;
        self.store_window(&window)?;

        let (target, ok) = self.get_target(&window.target_id)?;
        if !ok {
            window.status = SummaryWindowStatus::Failed;
            window.updated_at = Utc::now();
            return self.store_window(&window);
        }
        let outcome = DeliveryOutcome {
            delivery_id: new_delivery_id(),
            environment_scope: self.environment_scope.clone(),
            source_kind: "summary_window".to_string(),
            source_id: window.summary_window_id.clone(),
            result_class: ResultClass::RoutineSuccess,
            mode: DeliveryMode::Immediate,
            status: OutcomeStatus::Pending,
            chosen_target_id: target.target_id.clone(),
            preference_id: window.preference_id.clone(),
            summary_window_id: window.summary_window_id.clone(),
            payload_preview: format!("digest summary with {} routed results", window.result_count),
            created_at: now,
            updated_at: now,
            ..DeliveryOutcome::default()
        };
        self.store_outcome(&outcome)?;
        self.publish_outcome_created(&outcome)?;
        let outcome = self.dispatch_attempt(outcome, &target, 1)?;
        window.emitted_delivery_id = outcome.delivery_id.clone();
        window.updated_at = Utc::now();
        window.status = if outcome.status == OutcomeStatus::Delivered {
            SummaryWindowStatus::Delivered
        } else {
            SummaryWindowStatus::Failed
        };
        self.store_window(&window)?;
        self.publish_event(
            "delivery.summary_emitted",
            kura_events::Resource {
                kind: "delivery_summary_window".to_string(),
                id: window.summary_window_id.clone(),
            },
            payload_map(serde_json::json!({
                "summaryWindowId": window.summary_window_id,
                "resultCount": window.result_count,
                "emittedDeliveryId": window.emitted_delivery_id,
            })),
        )?;
        Ok(())
    }

    /// Port of `Restore`: resume queued/dispatching outcomes (except digest outcomes already
    /// attached to a window) and re-arm open/ready/dispatching summary windows.
    pub fn restore(self: &Arc<Self>) -> Result<(), DeliveryError> {
        let outcomes = self.list_outcomes(&crate::OutcomeFilter::default())?;
        for outcome in outcomes {
            if outcome.status != OutcomeStatus::Queued
                && outcome.status != OutcomeStatus::Dispatching
            {
                continue;
            }
            let mut next_run_at = Utc::now();
            if let Some(last) = outcome.attempts.last() {
                if let Some(next_retry_at) = last.next_retry_at {
                    next_run_at = next_retry_at;
                }
            }
            if outcome.mode == DeliveryMode::Digest && !outcome.summary_window_id.trim().is_empty()
            {
                continue;
            }
            self.schedule_retry(&outcome.delivery_id, next_run_at);
        }

        let windows = self.list_summary_windows()?;
        for window in windows {
            match window.status {
                SummaryWindowStatus::Open => {
                    self.schedule_window(&window.summary_window_id, window.window_ends_at);
                }
                SummaryWindowStatus::Ready | SummaryWindowStatus::Dispatching => {
                    self.schedule_window(&window.summary_window_id, Utc::now());
                }
                _ => {}
            }
        }
        Ok(())
    }
}

impl crate::Manager {
    /// Port of `Restore` (see digest.go): resumes queued outcomes and re-arms summary
    /// windows after a restart.
    pub fn restore(&self) -> Result<(), crate::manager::DeliveryError> {
        self.inner.restore()
    }

    /// Port of `emitWindow`. Public so tests can advance the window clock manually; the
    /// detached schedule threads call the same logic.
    pub fn emit_window(
        &self,
        summary_window_id: &str,
    ) -> Result<(), crate::manager::DeliveryError> {
        self.inner.emit_window(summary_window_id)
    }

    /// Port of `clearWindowSchedule`. Public for tests that re-arm a window after changing
    /// its deadline.
    pub fn clear_window_schedule(&self, summary_window_id: &str) {
        self.inner.clear_window_schedule(summary_window_id);
    }
}
