//! Port of daemon/internal/reminders/manager.go: the reminders manager.
//!
//! Sync port (like kura-delivery / kura-scheduler): context.Context is dropped, and the
//! catch-up + tick loop runs in a detached std thread when `start` is called. The store
//! is held behind Arc<parking_lot::Mutex<SQLiteStore>> so the manager (and any axum
//! AppState field) is Send + Sync; the workflow launcher seam carries Send + Sync
//! supertraits for the same reason.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use chrono::{DateTime, Utc};
use kura_delivery::{Manager as DeliveryManager, OutcomeInput, ResultClass};
use kura_events::{Bus, Event, Resource, Scope};
use kura_identity::TenantContext;
use kura_identity::tenantctx;
use kura_livevalidation::{FakeOutcome, FakeOutcomeResult, SafetyClass, fake_outcome_result_for};
use kura_scheduler::{Trigger, TriggerKind, next_due_after};
use kura_store::SQLiteStore;
use kura_store::reminders::{
    ReminderActionRecord, ReminderOccurrenceFilter, ReminderOccurrenceRecord, ReminderRecord,
};
use parking_lot::Mutex;
use serde_json::{Map, json};
use uuid::Uuid;

use crate::follow_up::refresh_follow_up_link;
use crate::types::{
    ActionKind, ActionRecord, ActorKind, BehaviorMode, CreateInput, Occurrence, OccurrenceFilter,
    Reminder, ReminderError, State, TransitionInput, WorkflowLaunchResult, WorkflowLauncher,
    is_unresolved_state,
};

/// Go Clock interface: injectable now source.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

/// Go realClock: time.Now().UTC().
#[derive(Debug, Clone, Copy, Default)]
pub struct RealClock;

impl Clock for RealClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// Go Dependencies.
pub struct Dependencies {
    pub environment_scope: String,
    pub store: Arc<Mutex<SQLiteStore>>,
    pub event_bus: Option<Bus>,
    pub delivery: Option<DeliveryManager>,
    pub workflow_launcher: Option<Arc<dyn WorkflowLauncher>>,
    pub clock: Option<Arc<dyn Clock>>,
    pub tick_interval: Duration,
}

/// Shared manager state (port of Go's Manager struct fields).
pub(crate) struct ManagerInner {
    env: String,
    store: Arc<Mutex<SQLiteStore>>,
    event_bus: Option<Bus>,
    delivery: Option<DeliveryManager>,
    workflow: Option<Arc<dyn WorkflowLauncher>>,
    clock: Arc<dyn Clock>,
    tick_interval: Duration,
    overdue_after: chrono::Duration,
    started: AtomicBool,
    stopped: AtomicBool,
    handle: Mutex<Option<JoinHandle<()>>>,
}

/// The reminders manager (port of Go Manager). Cloneable handle over shared inner state;
/// all operations are synchronous.
#[derive(Clone)]
pub struct Manager {
    inner: Arc<ManagerInner>,
}

impl Manager {
    /// Go NewManager: applies the clock (default RealClock) and tick-interval (default
    /// 500ms) defaults exactly like the Go constructor; overdueAfter mirrors tickInterval.
    #[must_use]
    pub fn new(deps: Dependencies) -> Self {
        let clock = deps.clock.unwrap_or_else(|| Arc::new(RealClock));
        let tick_interval = if deps.tick_interval <= Duration::ZERO {
            Duration::from_millis(500)
        } else {
            deps.tick_interval
        };
        let overdue_after = chrono::Duration::from_std(tick_interval)
            .unwrap_or_else(|_| chrono::Duration::milliseconds(500));
        Manager {
            inner: Arc::new(ManagerInner {
                env: deps.environment_scope.trim().to_string(),
                store: deps.store,
                event_bus: deps.event_bus,
                delivery: deps.delivery,
                workflow: deps.workflow_launcher,
                clock,
                tick_interval,
                overdue_after,
                started: AtomicBool::new(false),
                stopped: AtomicBool::new(false),
                handle: Mutex::new(None),
            }),
        }
    }

    /// Go RunLiveValidationOutcome: the fake-outcome verdict for an idempotent mutation
    /// (reminder lifecycle transitions are safe to retry).
    #[must_use]
    pub fn run_live_validation_outcome(&self, outcome: &FakeOutcome) -> FakeOutcomeResult {
        fake_outcome_result_for(
            outcome,
            &SafetyClass::from(SafetyClass::IDEMPOTENT_MUTATION),
        )
    }

    /// Go Start: launches the catch-up + tick loop on a detached std thread. A no-op once
    /// already started.
    pub fn start(&self) -> Result<(), ReminderError> {
        if self.inner.started.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        self.inner.stopped.store(false, Ordering::SeqCst);
        let manager = self.clone();
        let handle = std::thread::spawn(move || {
            let _ = manager.catch_up();
            while !manager.inner.stopped.load(Ordering::SeqCst) {
                let _ = manager.tick();
                std::thread::sleep(manager.inner.tick_interval);
            }
        });
        *self.inner.handle.lock() = Some(handle);
        Ok(())
    }

    /// Go Close: stops the tick loop and joins the background thread.
    pub fn close(&self) {
        self.inner.stopped.store(true, Ordering::SeqCst);
        if let Some(handle) = self.inner.handle.lock().take() {
            let _ = handle.join();
        }
    }

    /// Go CatchUp.
    pub fn catch_up(&self) -> Result<(), ReminderError> {
        self.inner.tick()
    }

    /// Go Tick: processes every reminder against the current clock time.
    pub fn tick(&self) -> Result<(), ReminderError> {
        self.inner.tick()
    }

    /// Go List: all reminders with refreshed projections.
    pub fn list(&self) -> Result<Vec<Reminder>, ReminderError> {
        self.inner.list()
    }

    /// Go Get.
    pub fn get(&self, reminder_id: &str) -> Result<(Reminder, bool), ReminderError> {
        self.inner.get(reminder_id)
    }

    /// Go ListOccurrences.
    pub fn list_occurrences(
        &self,
        filter: &OccurrenceFilter,
    ) -> Result<Vec<Occurrence>, ReminderError> {
        self.inner.list_occurrences(filter)
    }

    /// Go GetOccurrence.
    pub fn get_occurrence(&self, occurrence_id: &str) -> Result<(Occurrence, bool), ReminderError> {
        self.inner.get_occurrence(occurrence_id)
    }

    /// Go ListActions.
    pub fn list_actions(&self, reminder_id: &str) -> Result<Vec<ActionRecord>, ReminderError> {
        self.inner.list_actions(reminder_id)
    }

    /// Go Create.
    pub fn create(&self, input: &CreateInput) -> Result<Reminder, ReminderError> {
        self.inner.create(input)
    }

    /// Go Acknowledge.
    pub fn acknowledge(
        &self,
        reminder_id: &str,
        input: &TransitionInput,
    ) -> Result<(Reminder, Occurrence, ActionRecord), ReminderError> {
        self.inner.transition_occurrence(
            reminder_id,
            input,
            ActionKind::Acknowledged,
            State::Acknowledged,
        )
    }

    /// Go Complete.
    pub fn complete(
        &self,
        reminder_id: &str,
        input: &TransitionInput,
    ) -> Result<(Reminder, Occurrence, ActionRecord), ReminderError> {
        self.inner.transition_occurrence(
            reminder_id,
            input,
            ActionKind::Completed,
            State::Completed,
        )
    }

    /// Go Dismiss.
    pub fn dismiss(
        &self,
        reminder_id: &str,
        input: &TransitionInput,
    ) -> Result<(Reminder, Occurrence, ActionRecord), ReminderError> {
        self.inner.transition_occurrence(
            reminder_id,
            input,
            ActionKind::Dismissed,
            State::Dismissed,
        )
    }

    /// Go Cancel.
    pub fn cancel(
        &self,
        reminder_id: &str,
        input: &TransitionInput,
    ) -> Result<(Reminder, Occurrence, ActionRecord), ReminderError> {
        self.inner.cancel(reminder_id, input)
    }

    /// Go Snooze.
    pub fn snooze(
        &self,
        reminder_id: &str,
        input: &TransitionInput,
    ) -> Result<(Reminder, Occurrence, ActionRecord), ReminderError> {
        self.inner.snooze(reminder_id, input)
    }

    /// Go Reschedule.
    pub fn reschedule(
        &self,
        reminder_id: &str,
        input: &TransitionInput,
    ) -> Result<(Reminder, Occurrence, ActionRecord), ReminderError> {
        self.inner.reschedule(reminder_id, input)
    }
}

impl ManagerInner {
    pub(crate) fn tick(&self) -> Result<(), ReminderError> {
        let items = self.list_reminder_docs()?;
        let now = self.clock.now();
        for item in &items {
            self.process_reminder(item, now)?;
        }
        Ok(())
    }

    pub(crate) fn list(&self) -> Result<Vec<Reminder>, ReminderError> {
        let items = self.list_reminder_docs()?;
        let mut out = Vec::with_capacity(items.len());
        for item in &items {
            out.push(self.refresh_reminder_projection(item)?);
        }
        Ok(out)
    }

    pub(crate) fn get(&self, reminder_id: &str) -> Result<(Reminder, bool), ReminderError> {
        let (item, ok) = self.get_reminder_doc(reminder_id)?;
        if !ok {
            return Ok((Reminder::default(), false));
        }
        Ok((self.refresh_reminder_projection(&item)?, true))
    }

    pub(crate) fn list_occurrences(
        &self,
        filter: &OccurrenceFilter,
    ) -> Result<Vec<Occurrence>, ReminderError> {
        self.list_occurrence_docs(filter)
    }

    pub(crate) fn get_occurrence(
        &self,
        occurrence_id: &str,
    ) -> Result<(Occurrence, bool), ReminderError> {
        self.get_occurrence_doc(occurrence_id)
    }

    pub(crate) fn list_actions(
        &self,
        reminder_id: &str,
    ) -> Result<Vec<ActionRecord>, ReminderError> {
        self.list_action_docs(reminder_id)
    }

    pub(crate) fn create(&self, input: &CreateInput) -> Result<Reminder, ReminderError> {
        if input.title.trim().is_empty() {
            return Err(ReminderError::TitleRequired);
        }
        if input.behavior_mode == BehaviorMode::LaunchWorkflow
            && input.workflow_launch_config.is_none()
        {
            return Err(ReminderError::WorkflowConfigRequired);
        }
        let now = self.clock.now();
        let next_due_at = next_due_after(&input.trigger, now - chrono::Duration::seconds(1))
            .map_err(|e| ReminderError::Other(e.to_string()))?;
        let reminder = Reminder {
            reminder_id: new_reminder_id(),
            environment_scope: self.env.clone(),
            title: input.title.trim().to_string(),
            details: input.details.trim().to_string(),
            behavior_mode: input.behavior_mode,
            trigger: input.trigger.clone(),
            current_state: State::Pending,
            next_due_at,
            workflow_launch_config: clone_workflow_launch_config(&input.workflow_launch_config),
            follow_up_link: crate::follow_up::clone_follow_up_link(&input.follow_up_link),
            created_at: now,
            updated_at: now,
            ..Reminder::default()
        };
        self.put_reminder(&reminder)?;
        self.append_action(
            &reminder,
            "",
            ActionKind::Created,
            ActorKind::User,
            "",
            State::Pending.as_str(),
            "",
            "",
            "",
            "",
            now,
        )?;
        self.publish_reminder_event("reminder.created", &reminder, None, None)?;
        self.refresh_reminder_projection(&reminder)
    }

    pub(crate) fn transition_occurrence(
        &self,
        reminder_id: &str,
        input: &TransitionInput,
        action_kind: ActionKind,
        target: State,
    ) -> Result<(Reminder, Occurrence, ActionRecord), ReminderError> {
        let (mut reminder, mut occurrence) =
            self.get_actionable_occurrence(reminder_id, &input.occurrence_id)?;
        let now = self.clock.now();
        let prev = occurrence.state;
        occurrence.state = target;
        occurrence.updated_at = now;
        match target {
            State::Acknowledged => occurrence.acknowledged_at = Some(now),
            State::Completed => occurrence.completed_at = Some(now),
            State::Dismissed => occurrence.dismissed_at = Some(now),
            _ => {}
        }
        self.put_occurrence(&occurrence)?;
        reminder.current_state = target;
        reminder.active_occurrence_id = occurrence.occurrence_id.clone();
        reminder.updated_at = now;
        self.put_reminder(&reminder)?;
        let action = self.append_action(
            &reminder,
            &occurrence.occurrence_id,
            action_kind,
            input.actor_kind,
            prev.as_str(),
            target.as_str(),
            &input.reason,
            &occurrence.run_id,
            &occurrence.workflow_id,
            "",
            now,
        )?;
        self.publish_reminder_event(
            "reminder.occurrence_transitioned",
            &reminder,
            Some(&occurrence),
            Some(&action),
        )?;
        Ok((reminder, occurrence, action))
    }

    pub(crate) fn cancel(
        &self,
        reminder_id: &str,
        input: &TransitionInput,
    ) -> Result<(Reminder, Occurrence, ActionRecord), ReminderError> {
        let (reminder, ok) = self.get(reminder_id)?;
        if !ok {
            return Err(ReminderError::ReminderNotFound);
        }
        let now = self.clock.now();
        let mut reminder = reminder;
        reminder.current_state = State::Cancelled;
        reminder.next_due_at = None;
        reminder.active_occurrence_id = String::new();
        reminder.cancelled_at = Some(now);
        reminder.updated_at = now;
        self.put_reminder(&reminder)?;
        let mut occurrence = Occurrence::default();
        if !input.occurrence_id.trim().is_empty() {
            if let (occ, true) = self.get_occurrence_doc(&input.occurrence_id)? {
                occurrence = occ;
                occurrence.state = State::Cancelled;
                occurrence.cancelled_at = Some(now);
                occurrence.updated_at = now;
                self.put_occurrence(&occurrence)?;
            }
        }
        let action = self.append_action(
            &reminder,
            &occurrence.occurrence_id,
            ActionKind::Cancelled,
            input.actor_kind,
            occurrence.state.as_str(),
            State::Cancelled.as_str(),
            &input.reason,
            &occurrence.run_id,
            &occurrence.workflow_id,
            "",
            now,
        )?;
        self.publish_reminder_event(
            "reminder.updated",
            &reminder,
            Some(&occurrence),
            Some(&action),
        )?;
        Ok((reminder, occurrence, action))
    }

    pub(crate) fn snooze(
        &self,
        reminder_id: &str,
        input: &TransitionInput,
    ) -> Result<(Reminder, Occurrence, ActionRecord), ReminderError> {
        let Some(snoozed_until) = input.snoozed_until else {
            return Err(ReminderError::SnoozeRequired);
        };
        let (mut reminder, mut occurrence) =
            self.get_actionable_occurrence(reminder_id, &input.occurrence_id)?;
        let now = self.clock.now();
        occurrence.state = State::Snoozed;
        occurrence.snoozed_until = Some(snoozed_until);
        occurrence.updated_at = now;
        self.put_occurrence(&occurrence)?;
        reminder.current_state = State::Snoozed;
        reminder.next_due_at = Some(snoozed_until.with_timezone(&Utc));
        reminder.active_occurrence_id = occurrence.occurrence_id.clone();
        reminder.updated_at = now;
        if reminder.trigger.kind == TriggerKind::Once {
            reminder.trigger.fire_at = Some(snoozed_until.with_timezone(&Utc));
        }
        self.put_reminder(&reminder)?;
        let action = self.append_action(
            &reminder,
            &occurrence.occurrence_id,
            ActionKind::Snoozed,
            input.actor_kind,
            State::Due.as_str(),
            State::Snoozed.as_str(),
            &input.reason,
            &occurrence.run_id,
            &occurrence.workflow_id,
            "",
            now,
        )?;
        self.publish_reminder_event(
            "reminder.occurrence_transitioned",
            &reminder,
            Some(&occurrence),
            Some(&action),
        )?;
        Ok((reminder, occurrence, action))
    }

    pub(crate) fn reschedule(
        &self,
        reminder_id: &str,
        input: &TransitionInput,
    ) -> Result<(Reminder, Occurrence, ActionRecord), ReminderError> {
        let (reminder, ok) = self.get(reminder_id)?;
        if !ok {
            return Err(ReminderError::ReminderNotFound);
        }
        let Some(trigger) = &input.trigger else {
            return Err(ReminderError::InvalidTrigger);
        };
        let now = self.clock.now();
        let mut reminder = reminder;
        reminder.trigger = trigger.clone();
        let next_due_at = next_due_after(&reminder.trigger, now - chrono::Duration::seconds(1))
            .map_err(|e| ReminderError::Other(e.to_string()))?;
        reminder.next_due_at = next_due_at;
        reminder.current_state = State::Pending;
        reminder.active_occurrence_id = String::new();
        reminder.updated_at = now;
        self.put_reminder(&reminder)?;
        let occurrence = if input.occurrence_id.trim().is_empty() {
            Occurrence::default()
        } else {
            self.get_occurrence_doc(&input.occurrence_id)
                .map(|(o, _)| o)
                .unwrap_or_default()
        };
        let action = self.append_action(
            &reminder,
            &occurrence.occurrence_id,
            ActionKind::Rescheduled,
            input.actor_kind,
            occurrence.state.as_str(),
            State::Pending.as_str(),
            &input.reason,
            &occurrence.run_id,
            &occurrence.workflow_id,
            "",
            now,
        )?;
        self.publish_reminder_event(
            "reminder.updated",
            &reminder,
            Some(&occurrence),
            Some(&action),
        )?;
        Ok((reminder, occurrence, action))
    }

    fn process_reminder(
        &self,
        reminder: &Reminder,
        now: DateTime<Utc>,
    ) -> Result<(), ReminderError> {
        let reminder = self.refresh_reminder_projection(reminder)?;
        if reminder.current_state == State::Cancelled {
            return Ok(());
        }
        if !reminder.active_occurrence_id.is_empty() {
            if let (occurrence, true) = self.get_occurrence_doc(&reminder.active_occurrence_id)? {
                match occurrence.state {
                    State::Snoozed => {
                        if let Some(until) = occurrence.snoozed_until {
                            if until <= now {
                                return self.make_occurrence_due(&reminder, &occurrence, now);
                            }
                        }
                    }
                    State::Due => {
                        if now.signed_duration_since(occurrence.scheduled_for) >= self.overdue_after
                        {
                            return self.mark_occurrence_overdue(&reminder, &occurrence, now);
                        }
                    }
                    State::Overdue => {}
                    _ => {}
                }
            }
        }
        if let Some(next_due_at) = reminder.next_due_at {
            if next_due_at <= now {
                return self.create_due_occurrence(&reminder, now);
            }
        }
        Ok(())
    }

    fn create_due_occurrence(
        &self,
        reminder: &Reminder,
        now: DateTime<Utc>,
    ) -> Result<(), ReminderError> {
        let (previous, ok) = self.current_occurrence(reminder)?;
        if ok && is_unresolved_state(previous.state) {
            self.mark_occurrence_missed(reminder, &previous, now)?;
        }
        let scheduled_for = reminder.next_due_at.unwrap_or(now);
        let occurrence = Occurrence {
            occurrence_id: new_occurrence_id(),
            reminder_id: reminder.reminder_id.clone(),
            environment_scope: reminder.environment_scope.clone(),
            state: State::Due,
            scheduled_for,
            became_due_at: Some(now),
            created_at: now,
            updated_at: now,
            ..Occurrence::default()
        };
        self.put_occurrence(&occurrence)?;
        let mut updated = reminder.clone();
        updated.active_occurrence_id = occurrence.occurrence_id.clone();
        updated.current_state = State::Due;
        updated.updated_at = now;
        let next_due_at = next_reminder_due_after(&reminder.trigger, scheduled_for)?;
        updated.next_due_at = next_due_at;
        updated.trigger.next_due_at = next_due_at;
        self.put_reminder(&updated)?;
        let action = self.append_action(
            &updated,
            &occurrence.occurrence_id,
            ActionKind::Due,
            ActorKind::System,
            "",
            State::Due.as_str(),
            "",
            "",
            "",
            "",
            now,
        )?;
        self.publish_reminder_event(
            "reminder.occurrence_created",
            &updated,
            Some(&occurrence),
            Some(&action),
        )?;
        self.handle_due_occurrence(&updated, &occurrence, now)
    }

    fn handle_due_occurrence(
        &self,
        reminder: &Reminder,
        occurrence: &Occurrence,
        now: DateTime<Utc>,
    ) -> Result<(), ReminderError> {
        self.with_reminder_tenant_context(reminder, || {
            self.handle_due_occurrence_inner(reminder, occurrence, now)
        })
    }

    fn handle_due_occurrence_inner(
        &self,
        reminder: &Reminder,
        occurrence: &Occurrence,
        now: DateTime<Utc>,
    ) -> Result<(), ReminderError> {
        match reminder.behavior_mode {
            BehaviorMode::NotifyOnly => self.emit_reminder_delivery(reminder, occurrence, now),
            BehaviorMode::LaunchWorkflow => {
                if reminder.workflow_launch_config.is_none() || self.workflow.is_none() {
                    return self.record_workflow_launch_failure(
                        reminder,
                        occurrence,
                        now,
                        "workflow launcher is not configured",
                    );
                }
                let cfg = reminder
                    .workflow_launch_config
                    .as_ref()
                    .expect("checked above");
                let launcher = self.workflow.as_ref().expect("checked above");
                match launcher.launch_reminder_workflow(
                    cfg,
                    &reminder.reminder_id,
                    &occurrence.occurrence_id,
                ) {
                    Ok(result) => {
                        self.apply_workflow_launch_success(reminder, occurrence, now, &result)
                    }
                    Err(err) => {
                        self.record_workflow_launch_failure(reminder, occurrence, now, &err)
                    }
                }
            }
        }
    }

    fn apply_workflow_launch_success(
        &self,
        reminder: &Reminder,
        occurrence: &Occurrence,
        now: DateTime<Utc>,
        result: &WorkflowLaunchResult,
    ) -> Result<(), ReminderError> {
        let mut occ = occurrence.clone();
        occ.run_id = result.run_id.clone();
        occ.workflow_id = result.workflow_id.clone();
        occ.state = State::Acknowledged;
        occ.acknowledged_at = Some(now);
        occ.updated_at = now;
        self.put_occurrence(&occ)?;
        let mut rem = reminder.clone();
        rem.current_state = State::Acknowledged;
        rem.updated_at = now;
        self.put_reminder(&rem)?;
        let action = self.append_action(
            &rem,
            &occ.occurrence_id,
            ActionKind::WorkflowStarted,
            ActorKind::System,
            State::Due.as_str(),
            State::Acknowledged.as_str(),
            "",
            &result.run_id,
            &result.workflow_id,
            "",
            now,
        )?;
        self.publish_reminder_event(
            "reminder.workflow_launch_started",
            &rem,
            Some(&occ),
            Some(&action),
        )
    }

    fn with_reminder_tenant_context<R>(&self, reminder: &Reminder, run: impl FnOnce() -> R) -> R {
        if tenantctx::from_context().is_some() {
            return run();
        }
        let tenant_id = reminder.tenant_id.trim();
        if tenant_id.is_empty() {
            return run();
        }
        let tc = TenantContext {
            tenant_id: tenant_id.to_string(),
            principal_id: "system:reminder".to_string(),
            ..TenantContext::default()
        };
        tenantctx::with_context(tc, run)
    }

    fn emit_reminder_delivery(
        &self,
        reminder: &Reminder,
        occurrence: &Occurrence,
        now: DateTime<Utc>,
    ) -> Result<(), ReminderError> {
        let Some(delivery) = &self.delivery else {
            return Ok(());
        };
        let outcome = delivery
            .emit_outcome(OutcomeInput {
                source_kind: "reminder_occurrence".to_string(),
                source_id: occurrence.occurrence_id.clone(),
                result_class: ResultClass::RoutineSuccess,
                payload_preview: reminder.title.clone(),
                ..OutcomeInput::default()
            })
            .map_err(|e| ReminderError::Other(format!("emit reminder delivery: {e}")))?;
        let mut occ = occurrence.clone();
        occ.latest_delivery_id = outcome.delivery_id.clone();
        occ.latest_delivery_status = outcome.status.as_str().to_string();
        occ.latest_delivery_target_id = outcome.chosen_target_id.clone();
        occ.updated_at = now;
        self.put_occurrence(&occ)?;
        let action = self.append_action(
            reminder,
            &occ.occurrence_id,
            ActionKind::DeliveryLinked,
            ActorKind::System,
            occ.state.as_str(),
            occ.state.as_str(),
            "",
            &occ.run_id,
            &occ.workflow_id,
            &outcome.delivery_id,
            now,
        )?;
        self.publish_reminder_event(
            "reminder.delivery_linked",
            reminder,
            Some(&occ),
            Some(&action),
        )
    }

    fn mark_occurrence_overdue(
        &self,
        reminder: &Reminder,
        occurrence: &Occurrence,
        now: DateTime<Utc>,
    ) -> Result<(), ReminderError> {
        if occurrence.state != State::Due {
            return Ok(());
        }
        let mut occ = occurrence.clone();
        occ.state = State::Overdue;
        occ.overdue_at = Some(now);
        occ.updated_at = now;
        self.put_occurrence(&occ)?;
        let mut rem = reminder.clone();
        rem.current_state = State::Overdue;
        rem.updated_at = now;
        self.put_reminder(&rem)?;
        let action = self.append_action(
            &rem,
            &occ.occurrence_id,
            ActionKind::Overdue,
            ActorKind::System,
            State::Due.as_str(),
            State::Overdue.as_str(),
            "",
            &occ.run_id,
            &occ.workflow_id,
            "",
            now,
        )?;
        self.publish_reminder_event(
            "reminder.occurrence_transitioned",
            &rem,
            Some(&occ),
            Some(&action),
        )
    }

    fn mark_occurrence_missed(
        &self,
        reminder: &Reminder,
        occurrence: &Occurrence,
        now: DateTime<Utc>,
    ) -> Result<(), ReminderError> {
        let mut occ = occurrence.clone();
        occ.state = State::Missed;
        occ.missed_at = Some(now);
        occ.updated_at = now;
        self.put_occurrence(&occ)?;
        let action = self.append_action(
            reminder,
            &occ.occurrence_id,
            ActionKind::Missed,
            ActorKind::System,
            occ.state.as_str(),
            State::Missed.as_str(),
            "",
            &occ.run_id,
            &occ.workflow_id,
            "",
            now,
        )?;
        self.publish_reminder_event(
            "reminder.occurrence_transitioned",
            reminder,
            Some(&occ),
            Some(&action),
        )
    }

    fn make_occurrence_due(
        &self,
        reminder: &Reminder,
        occurrence: &Occurrence,
        now: DateTime<Utc>,
    ) -> Result<(), ReminderError> {
        let mut occ = occurrence.clone();
        occ.state = State::Due;
        occ.became_due_at = Some(now);
        occ.updated_at = now;
        self.put_occurrence(&occ)?;
        let mut rem = reminder.clone();
        rem.current_state = State::Due;
        rem.updated_at = now;
        self.put_reminder(&rem)?;
        let action = self.append_action(
            &rem,
            &occ.occurrence_id,
            ActionKind::Due,
            ActorKind::System,
            State::Snoozed.as_str(),
            State::Due.as_str(),
            "",
            &occ.run_id,
            &occ.workflow_id,
            "",
            now,
        )?;
        self.publish_reminder_event(
            "reminder.occurrence_transitioned",
            &rem,
            Some(&occ),
            Some(&action),
        )?;
        self.handle_due_occurrence(&rem, &occ, now)
    }

    fn record_workflow_launch_failure(
        &self,
        reminder: &Reminder,
        occurrence: &Occurrence,
        now: DateTime<Utc>,
        reason: &str,
    ) -> Result<(), ReminderError> {
        let action = self.append_action(
            reminder,
            &occurrence.occurrence_id,
            ActionKind::WorkflowStartFailed,
            ActorKind::System,
            occurrence.state.as_str(),
            occurrence.state.as_str(),
            reason,
            &occurrence.run_id,
            &occurrence.workflow_id,
            "",
            now,
        )?;
        self.publish_reminder_event(
            "reminder.workflow_launch_failed",
            reminder,
            Some(occurrence),
            Some(&action),
        )
    }

    fn get_actionable_occurrence(
        &self,
        reminder_id: &str,
        occurrence_id: &str,
    ) -> Result<(Reminder, Occurrence), ReminderError> {
        let (reminder, ok) = self.get(reminder_id)?;
        if !ok {
            return Err(ReminderError::ReminderNotFound);
        }
        let target_id = {
            let trimmed = occurrence_id.trim();
            if trimmed.is_empty() {
                reminder.active_occurrence_id.clone()
            } else {
                trimmed.to_string()
            }
        };
        if target_id.is_empty() {
            return Err(ReminderError::OccurrenceNotFound);
        }
        let (occurrence, ok) = self.get_occurrence_doc(&target_id)?;
        if !ok || occurrence.reminder_id != reminder_id {
            return Err(ReminderError::OccurrenceNotFound);
        }
        if !matches!(
            occurrence.state,
            State::Due | State::Overdue | State::Snoozed
        ) {
            return Err(ReminderError::InvalidState);
        }
        Ok((reminder, occurrence))
    }

    fn current_occurrence(&self, reminder: &Reminder) -> Result<(Occurrence, bool), ReminderError> {
        if reminder.active_occurrence_id.trim().is_empty() {
            return Ok((Occurrence::default(), false));
        }
        self.get_occurrence_doc(&reminder.active_occurrence_id)
    }

    fn refresh_reminder_projection(&self, reminder: &Reminder) -> Result<Reminder, ReminderError> {
        let link = refresh_follow_up_link(&self.store, &self.env, &reminder.follow_up_link)?;
        let mut reminder = reminder.clone();
        reminder.follow_up_link = link;
        if !reminder.active_occurrence_id.is_empty() {
            if let (occurrence, true) = self.get_occurrence_doc(&reminder.active_occurrence_id)? {
                reminder.current_state = occurrence.state;
                return Ok(reminder);
            }
        }
        if reminder.cancelled_at.is_some() {
            reminder.current_state = State::Cancelled;
            return Ok(reminder);
        }
        if reminder.next_due_at.is_some() {
            reminder.current_state = State::Pending;
            return Ok(reminder);
        }
        let items = self.list_occurrence_docs(&OccurrenceFilter {
            reminder_id: reminder.reminder_id.clone(),
            ..OccurrenceFilter::default()
        })?;
        if let Some(first) = items.first() {
            reminder.current_state = first.state;
        }
        Ok(reminder)
    }

    #[allow(clippy::too_many_arguments)]
    fn append_action(
        &self,
        reminder: &Reminder,
        occurrence_id: &str,
        kind: ActionKind,
        actor: ActorKind,
        previous_state: &str,
        new_state: &str,
        reason: &str,
        run_id: &str,
        workflow_id: &str,
        delivery_id: &str,
        created_at: DateTime<Utc>,
    ) -> Result<ActionRecord, ReminderError> {
        let item = ActionRecord {
            action_id: new_action_id(),
            reminder_id: reminder.reminder_id.clone(),
            occurrence_id: occurrence_id.to_string(),
            action_kind: kind,
            actor_kind: actor,
            previous_state: previous_state.to_string(),
            new_state: new_state.to_string(),
            reason: reason.trim().to_string(),
            run_id: run_id.trim().to_string(),
            workflow_id: workflow_id.trim().to_string(),
            delivery_id: delivery_id.trim().to_string(),
            created_at,
        };
        self.put_action(&item)?;
        Ok(item)
    }

    fn publish_reminder_event(
        &self,
        name: &str,
        reminder: &Reminder,
        occurrence: Option<&Occurrence>,
        action: Option<&ActionRecord>,
    ) -> Result<(), ReminderError> {
        let Some(bus) = &self.event_bus else {
            return Ok(());
        };
        let mut payload = Map::new();
        payload.insert("reminderId".to_string(), json!(reminder.reminder_id));
        payload.insert(
            "behaviorMode".to_string(),
            json!(reminder.behavior_mode.as_str()),
        );
        payload.insert("nextDueAt".to_string(), json!(reminder.next_due_at));
        payload.insert(
            "currentState".to_string(),
            json!(reminder.current_state.as_str()),
        );
        payload.insert(
            "activeOccurrenceId".to_string(),
            json!(reminder.active_occurrence_id),
        );
        let mut scope = Scope::default();
        if let Some(occurrence) = occurrence {
            payload.insert("occurrenceId".to_string(), json!(occurrence.occurrence_id));
            payload.insert("state".to_string(), json!(occurrence.state.as_str()));
            payload.insert("scheduledFor".to_string(), json!(occurrence.scheduled_for));
            scope.run_id = occurrence.run_id.clone();
            scope.workflow_id = occurrence.workflow_id.clone();
        }
        if let Some(action) = action {
            payload.insert("actionKind".to_string(), json!(action.action_kind.as_str()));
            payload.insert("reason".to_string(), json!(action.reason));
            if !action.delivery_id.is_empty() {
                payload.insert("deliveryId".to_string(), json!(action.delivery_id));
            }
        }
        let mut event = Event {
            event_id: format!("evt_{}", random_hex(8)),
            category: "reminder".to_string(),
            name: name.to_string(),
            occurred_at: self.clock.now(),
            scope,
            resource: Resource {
                kind: "reminder".to_string(),
                id: reminder.reminder_id.clone(),
            },
            payload,
            ..Event::default()
        };
        event.environment_scope = self.env.clone();
        let event = self
            .store
            .lock()
            .append_event(&event)
            .map_err(ReminderError::Store)?;
        bus.publish(event);
        Ok(())
    }

    fn list_reminder_docs(&self) -> Result<Vec<Reminder>, ReminderError> {
        let records = self
            .store
            .lock()
            .list_reminders(&self.env)
            .map_err(ReminderError::Store)?;
        let mut items = Vec::with_capacity(records.len());
        for record in &records {
            items.push(decode_reminder_record(record)?);
        }
        Ok(items)
    }

    fn get_reminder_doc(&self, reminder_id: &str) -> Result<(Reminder, bool), ReminderError> {
        let record = self
            .store
            .lock()
            .get_reminder(&self.env, reminder_id)
            .map_err(ReminderError::Store)?;
        match record {
            Some(record) => Ok((decode_reminder_record(&record)?, true)),
            None => Ok((Reminder::default(), false)),
        }
    }

    fn put_reminder(&self, reminder: &Reminder) -> Result<(), ReminderError> {
        let record = encode_reminder_record(reminder)?;
        self.store
            .lock()
            .upsert_reminder(&record)
            .map_err(ReminderError::Store)
    }

    fn list_occurrence_docs(
        &self,
        filter: &OccurrenceFilter,
    ) -> Result<Vec<Occurrence>, ReminderError> {
        let records = self
            .store
            .lock()
            .list_reminder_occurrences(
                &self.env,
                &ReminderOccurrenceFilter {
                    reminder_id: filter.reminder_id.clone(),
                    state: filter
                        .state
                        .map(|s| s.as_str().to_string())
                        .unwrap_or_default(),
                    run_id: filter.run_id.clone(),
                    workflow_id: filter.workflow_id.clone(),
                    delivery_id: filter.delivery_id.clone(),
                    scheduled_before: filter.scheduled_before,
                    scheduled_after: filter.scheduled_after,
                },
            )
            .map_err(ReminderError::Store)?;
        let mut items = Vec::with_capacity(records.len());
        for record in &records {
            items.push(decode_occurrence_record(record)?);
        }
        Ok(items)
    }

    fn get_occurrence_doc(&self, occurrence_id: &str) -> Result<(Occurrence, bool), ReminderError> {
        let record = self
            .store
            .lock()
            .get_reminder_occurrence(&self.env, occurrence_id)
            .map_err(ReminderError::Store)?;
        match record {
            Some(record) => Ok((decode_occurrence_record(&record)?, true)),
            None => Ok((Occurrence::default(), false)),
        }
    }

    fn put_occurrence(&self, occurrence: &Occurrence) -> Result<(), ReminderError> {
        let record = encode_occurrence_record(occurrence)?;
        self.store
            .lock()
            .upsert_reminder_occurrence(&record)
            .map_err(ReminderError::Store)
    }

    fn list_action_docs(&self, reminder_id: &str) -> Result<Vec<ActionRecord>, ReminderError> {
        let records = self
            .store
            .lock()
            .list_reminder_actions(&self.env, reminder_id)
            .map_err(ReminderError::Store)?;
        let mut items = Vec::with_capacity(records.len());
        for record in &records {
            items.push(decode_action_record(record)?);
        }
        Ok(items)
    }

    fn put_action(&self, action: &ActionRecord) -> Result<(), ReminderError> {
        let record = encode_action_record(action)?;
        self.store
            .lock()
            .append_reminder_action(&record)
            .map_err(ReminderError::Store)
    }
}

fn encode_reminder_record(item: &Reminder) -> Result<ReminderRecord, ReminderError> {
    let document = serde_json::to_string(item).map_err(|e| {
        ReminderError::Encode(format!("marshal reminder {}: {e}", item.reminder_id))
    })?;
    Ok(ReminderRecord {
        reminder_id: item.reminder_id.clone(),
        environment_scope: item.environment_scope.clone(),
        tenant_id: item.tenant_id.clone(),
        behavior_mode: item.behavior_mode.as_str().to_string(),
        current_state: item.current_state.as_str().to_string(),
        next_due_at: item.next_due_at,
        active_occurrence_id: item.active_occurrence_id.clone(),
        updated_at: item.updated_at,
        document,
    })
}

fn decode_reminder_record(record: &ReminderRecord) -> Result<Reminder, ReminderError> {
    let mut item: Reminder = serde_json::from_str(&record.document).map_err(|e| {
        ReminderError::Decode(format!("decode reminder {}: {e}", record.reminder_id))
    })?;
    // The tenant column is authoritative (Go overrides it after unmarshal).
    item.tenant_id = record.tenant_id.clone();
    Ok(item)
}

fn encode_occurrence_record(item: &Occurrence) -> Result<ReminderOccurrenceRecord, ReminderError> {
    let document = serde_json::to_string(item).map_err(|e| {
        ReminderError::Encode(format!(
            "marshal reminder occurrence {}: {e}",
            item.occurrence_id
        ))
    })?;
    Ok(ReminderOccurrenceRecord {
        occurrence_id: item.occurrence_id.clone(),
        reminder_id: item.reminder_id.clone(),
        environment_scope: item.environment_scope.clone(),
        state: item.state.as_str().to_string(),
        scheduled_for: item.scheduled_for,
        run_id: item.run_id.clone(),
        workflow_id: item.workflow_id.clone(),
        latest_delivery_id: item.latest_delivery_id.clone(),
        latest_delivery_status: item.latest_delivery_status.clone(),
        updated_at: item.updated_at,
        document,
    })
}

fn decode_occurrence_record(
    record: &ReminderOccurrenceRecord,
) -> Result<Occurrence, ReminderError> {
    serde_json::from_str(&record.document).map_err(|e| {
        ReminderError::Decode(format!(
            "decode reminder occurrence {}: {e}",
            record.occurrence_id
        ))
    })
}

fn encode_action_record(item: &ActionRecord) -> Result<ReminderActionRecord, ReminderError> {
    let document = serde_json::to_string(item).map_err(|e| {
        ReminderError::Encode(format!("marshal reminder action {}: {e}", item.action_id))
    })?;
    Ok(ReminderActionRecord {
        action_id: item.action_id.clone(),
        reminder_id: item.reminder_id.clone(),
        occurrence_id: item.occurrence_id.clone(),
        action_kind: item.action_kind.as_str().to_string(),
        new_state: item.new_state.clone(),
        run_id: item.run_id.clone(),
        workflow_id: item.workflow_id.clone(),
        delivery_id: item.delivery_id.clone(),
        created_at: item.created_at,
        document,
    })
}

fn decode_action_record(record: &ReminderActionRecord) -> Result<ActionRecord, ReminderError> {
    serde_json::from_str(&record.document).map_err(|e| {
        ReminderError::Decode(format!("decode reminder action {}: {e}", record.action_id))
    })
}

/// Go nextReminderDueAfter: one-time triggers never produce another due time.
fn next_reminder_due_after(
    trigger: &Trigger,
    after: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, ReminderError> {
    if trigger.kind == TriggerKind::Once {
        return Ok(None);
    }
    next_due_after(trigger, after).map_err(|e| ReminderError::Other(e.to_string()))
}

/// Go cloneWorkflowLaunchConfig: shallow copy.
fn clone_workflow_launch_config(
    item: &Option<crate::types::WorkflowLaunchConfig>,
) -> Option<crate::types::WorkflowLaunchConfig> {
    item.clone()
}

/// Go newReminderID.
fn new_reminder_id() -> String {
    format!("rem_{}", random_hex(8))
}

/// Go newOccurrenceID.
fn new_occurrence_id() -> String {
    format!("rem_occ_{}", random_hex(8))
}

/// Go newActionID.
fn new_action_id() -> String {
    format!("rem_act_{}", random_hex(8))
}

/// Go randomHex: size random bytes hex-encoded (8 bytes -> 16 hex chars).
fn random_hex(size: usize) -> String {
    let hex = Uuid::new_v4().simple().to_string();
    hex.chars().take(size * 2).collect()
}
