//! Port of `daemon/internal/scheduler`: the schedule ledger, trigger due-time computation
//! (one-time/cron), dispatch attempts with retry/exhaustion semantics, downstream
//! run/workflow reconciliation, and catch-up of missed recurring intervals. The ledger is
//! persisted through the `kura-store` schedule CRUD (`store/src/schedule.rs`), and domain
//! events fan out over the `kura-events` Bus.
//!
//! The Go package threads a `context.Context` through every method (used only for
//! cancellation and tenant/identity plumbing) and runs a background catch-up + tick loop.
//! This is a SYNC port: `context` is dropped, `catch_up`/`tick` are plain callable
//! methods (the loop cadence is exposed through `Scheduler::tick_interval`), and
//! `start`/`close` only track lifecycle state.
//!
//! Divergences from the Go manager (all documented at the call sites):
//! - Event persistence via `store.AppendEvent` is excluded because `kura-store`'s events
//!   module is not yet public; events are published to the Bus only.
//! - `store.UpsertRun`, `store.SaveThreadRuntimeProjectionForRun`, and
//!   `store.GetThreadForSession` (thread-archive guard) are excluded for the same reason;
//!   created runs live in the runtime manager, and the archived-thread guard is inert.
//! - The billing quota reservation/commit/release branch (`kura-billing` Manager is async)
//!   and the checkpoints save step are excluded.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::Mutex;

use chrono::{DateTime, Datelike, Timelike, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use kura_store::SQLiteStore;
use kura_store::schedule::{ScheduleDispatchAttemptRecord, ScheduleRecord, ScheduleTargetRecord};

macro_rules! string_enum {
    ($name:ident { $first:ident => $first_s:literal $(, $v:ident => $s:literal)* $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
        pub enum $name {
            #[default]
            #[serde(rename = $first_s)]
            $first,
            $(#[serde(rename = $s)] $v),*
        }
        impl $name {
            #[must_use]
            pub fn as_str(self) -> &'static str {
                match self {
                    $name::$first => $first_s,
                    $( $name::$v => $s ),*
                }
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

string_enum!(ScheduleKind {
    OneTime => "one_time",
    Recurring => "recurring",
});

string_enum!(ScheduleStatus {
    Scheduled => "scheduled",
    Active => "active",
    Paused => "paused",
    Cancelled => "cancelled",
    Completed => "completed",
    DispatchFailed => "dispatch_failed",
});

string_enum!(TriggerKind {
    Once => "once",
    Cron => "cron",
});

string_enum!(TargetKind {
    Run => "run",
    Workflow => "workflow",
});

string_enum!(TriggerSource {
    Normal => "normal",
    CatchUp => "catch_up",
    Retry => "retry",
});

string_enum!(DispatchStatus {
    Pending => "pending",
    Dispatching => "dispatching",
    Dispatched => "dispatched",
    Failed => "failed",
    Missed => "missed",
    SkippedPaused => "skipped_paused",
    SkippedOverlap => "skipped_overlap",
    SkippedCancelled => "skipped_cancelled",
    Exhausted => "exhausted",
});

string_enum!(DownstreamStatus {
    None => "none",
    Running => "running",
    Completed => "completed",
    Failed => "failed",
    Cancelled => "cancelled",
    Interrupted => "interrupted",
});

string_enum!(RetryBackoffKind {
    Fixed => "fixed",
    Exponential => "exponential",
});

/// Go `IsTerminalScheduleStatus`.
#[must_use]
pub fn is_terminal_schedule_status(status: ScheduleStatus) -> bool {
    matches!(
        status,
        ScheduleStatus::Cancelled | ScheduleStatus::Completed | ScheduleStatus::DispatchFailed
    )
}

/// Go `IsTerminalDispatchStatus`.
#[must_use]
pub fn is_terminal_dispatch_status(status: DispatchStatus) -> bool {
    matches!(
        status,
        DispatchStatus::Dispatched
            | DispatchStatus::Missed
            | DispatchStatus::SkippedPaused
            | DispatchStatus::SkippedOverlap
            | DispatchStatus::SkippedCancelled
            | DispatchStatus::Exhausted
    )
}

/// Go `IsActiveDownstreamStatus`.
#[must_use]
pub fn is_active_downstream_status(status: DownstreamStatus) -> bool {
    status == DownstreamStatus::Running
}

#[must_use]
fn is_terminal_downstream(status: DownstreamStatus) -> bool {
    matches!(
        status,
        DownstreamStatus::Completed
            | DownstreamStatus::Failed
            | DownstreamStatus::Cancelled
            | DownstreamStatus::Interrupted
    )
}

#[must_use]
fn is_zero_i64(v: &i64) -> bool {
    *v == 0
}

/// Matches Go `time.Time.IsZero()` for the Rust port: the derived `Default` for
/// `DateTime<Utc>` is the Unix epoch, which is what an unset target timestamp carries.
#[must_use]
fn is_zero_datetime(dt: DateTime<Utc>) -> bool {
    dt == DateTime::<Utc>::default()
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Schedule {
    pub schedule_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub environment_scope: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub kind: ScheduleKind,
    pub status: ScheduleStatus,
    pub target_ref_id: String,
    pub trigger: Trigger,
    pub target: Target,
    pub retry_policy: RetryPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_due_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_attempt_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_outcome: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancelled_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attempts: Vec<DispatchAttempt>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Trigger {
    pub kind: TriggerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fire_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cron_expr: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub timezone: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_due_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub kind: TargetKind,
    pub revision: i64,
    pub active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<RunTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<WorkflowTarget>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunTarget {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    pub entrypoint: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub goal: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowTarget {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    pub entrypoint: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub run_goal: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workflow_goal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar_action: Option<kura_calendar::Action>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mail_action: Option<kura_mail::Action>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryPolicy {
    pub max_retries: i64,
    pub backoff_kind: RetryBackoffKind,
    pub base_delay_seconds: i64,
    pub max_delay_seconds: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatchAttempt {
    /// Wire key is `scheduleAttemptId` (matches the Go field tag).
    #[serde(rename = "scheduleAttemptId")]
    pub attempt_id: String,
    pub schedule_id: String,
    pub due_at: DateTime<Utc>,
    pub trigger_source: TriggerSource,
    pub dispatch_status: DispatchStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub failure_class: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub failure_reason: String,
    pub retry_count: i64,
    pub retry_budget: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_retry_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub resolved_target_revision: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub run_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workflow_id: String,
    pub downstream_status: DownstreamStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub latest_delivery_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub latest_delivery_status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub latest_delivery_target_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub calendar_operation_summaries: Vec<kura_calendar::OperationSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mail_operation_summaries: Vec<kura_mail::OperationSummary>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub skipped_reason: String,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub missed_count: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Go `CreateInput` for `Scheduler.Create`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateInput {
    pub trigger: Trigger,
    pub target: Target,
    pub retry_policy: RetryPolicy,
}

#[derive(Debug, thiserror::Error)]
pub enum SchedulerError {
    /// Go `fmt.Errorf("scheduler store is not configured")`; retained for parity even
    /// though the Rust port requires a store.
    #[error("scheduler store is not configured")]
    StoreNotConfigured,
    #[error("store: {0}")]
    Store(String),
    #[error("decode schedule {0}: {1}")]
    DecodeSchedule(String, String),
    #[error("decode schedule target {0}: {1}")]
    DecodeScheduleTarget(String, String),
    #[error("decode schedule attempt {0}: {1}")]
    DecodeScheduleAttempt(String, String),
    #[error("marshal schedule {0}: {1}")]
    MarshalSchedule(String, String),
    #[error("marshal schedule target {0}: {1}")]
    MarshalScheduleTarget(String, String),
    #[error("marshal schedule attempt {0}: {1}")]
    MarshalScheduleAttempt(String, String),
    #[error("{0}")]
    Trigger(String),
    #[error("{0}")]
    Cron(String),
}

/// Go `WorkflowLaunchResult` returned by the `WorkflowLauncher`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkflowLaunchResult {
    pub run_id: String,
    pub workflow_id: String,
    pub downstream_status: DownstreamStatus,
}

/// Go `WorkflowLauncher` interface: launches a scheduled workflow target.
pub trait WorkflowLauncher: Send + Sync {
    fn launch_scheduled_workflow(
        &self,
        target: &WorkflowTarget,
        schedule_id: &str,
        schedule_attempt_id: &str,
    ) -> Result<WorkflowLaunchResult, String>;
}

/// Go `Clock` interface: injectable now source.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

/// Go `realClock`: `time.Now().UTC()`.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealClock;

impl Clock for RealClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// Go `Dependencies` for `New`. `clock`/tick defaults are applied in
/// `Scheduler::new` (`RealClock`, 500ms) exactly like the Go constructor.
pub struct Dependencies {
    pub environment: kura_config::Environment,
    pub runtime: Arc<kura_runtime::Manager>,
    pub event_bus: Option<kura_events::Bus>,
    pub store: Arc<Mutex<SQLiteStore>>,
    pub workflow_launcher: Option<Arc<dyn WorkflowLauncher>>,
    pub clock: Option<Box<dyn Clock>>,
    pub tick_interval: Duration,
}

pub struct Scheduler {
    environment: kura_config::Environment,
    runtime: Arc<kura_runtime::Manager>,
    event_bus: Option<kura_events::Bus>,
    store: Arc<Mutex<SQLiteStore>>,
    workflow_launcher: Option<Arc<dyn WorkflowLauncher>>,
    clock: Box<dyn Clock>,
    tick_interval: Duration,
    started: AtomicBool,
}

impl Scheduler {
    /// Go `New`.
    pub fn new(deps: Dependencies) -> Self {
        let clock = deps.clock.unwrap_or_else(|| Box::new(RealClock));
        let tick_interval = if deps.tick_interval <= Duration::ZERO {
            Duration::from_millis(500)
        } else {
            deps.tick_interval
        };
        Scheduler {
            environment: deps.environment,
            runtime: deps.runtime,
            event_bus: deps.event_bus,
            store: deps.store,
            workflow_launcher: deps.workflow_launcher,
            clock,
            tick_interval,
            started: AtomicBool::new(false),
        }
    }

    /// Go `Start`: the Go manager spawns a goroutine that runs `CatchUp` then ticks every
    /// `tick_interval`. The sync port owns no background thread — the caller drives
    /// `catch_up`/`tick` at `tick_interval` — so this method only records lifecycle
    /// state.
    pub fn start(&self) -> Result<(), SchedulerError> {
        if self.started.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        Ok(())
    }

    /// Go `Close`: stops the background loop. In the sync port it resets the lifecycle flag.
    pub fn close(&self) -> Result<(), SchedulerError> {
        self.started.swap(false, Ordering::SeqCst);
        Ok(())
    }

    /// Go `Store`: returns the underlying store.
    #[must_use]
    pub fn store(&self) -> Arc<Mutex<SQLiteStore>> {
        Arc::clone(&self.store)
    }

    /// The tick interval the Go background loop runs at; the sync caller drives `tick` at
    /// this cadence.
    #[must_use]
    pub fn tick_interval(&self) -> Duration {
        self.tick_interval
    }

    /// Go `Create`. Validates the trigger eagerly (`NextDueAfter`), assigns the schedule
    /// identity/target revision, derives kind/status, and persists schedule + target records.
    /// Tenant context from the Go `ctx` is dropped in the sync port, so `tenant_id`
    /// stays empty unless a later layer backfills it.
    pub fn create(&self, mut input: CreateInput) -> Result<Schedule, SchedulerError> {
        let now = self.clock.now();
        if is_zero_datetime(input.target.updated_at) {
            input.target.updated_at = now;
        }
        let next_due_at = next_due_after(&input.trigger, now - chrono::Duration::seconds(1))?;
        let mut schedule = Schedule {
            schedule_id: new_schedule_id(),
            environment_scope: environment_scope(self.environment),
            target_ref_id: new_target_ref_id(),
            trigger: input.trigger,
            target: input.target,
            retry_policy: normalize_retry_policy(input.retry_policy),
            created_at: now,
            updated_at: now,
            next_due_at,
            ..Schedule::default()
        };
        schedule.target.updated_at = now;
        schedule.target.revision = 1;
        schedule.target.active = true;
        schedule.kind = derive_schedule_kind(schedule.trigger.kind);
        schedule.status = initial_schedule_status(schedule.kind);
        schedule.trigger.next_due_at = next_due_at;
        schedule.target.summary = target_summary(&schedule.target);

        self.persist_schedule(&schedule)?;
        self.publish_event(
            "schedule.created",
            &schedule,
            None,
            &kv(&[
                ("status", serde_json::json!(schedule.status.as_str())),
                (
                    "targetKind",
                    serde_json::json!(schedule.target.kind.as_str()),
                ),
                ("targetRefId", serde_json::json!(schedule.target_ref_id)),
            ]),
        )?;
        Ok(schedule)
    }

    /// Go `List`: hydrates every schedule record in the environment scope.
    pub fn list(&self) -> Result<Vec<Schedule>, SchedulerError> {
        let records = self
            .store
            .lock()
            .list_schedules(&environment_scope(self.environment))
            .map_err(SchedulerError::Store)?;
        records
            .into_iter()
            .map(|record| self.hydrate_schedule(record))
            .collect()
    }

    /// Go `Get` (the Go `(Schedule, bool, error)` becomes `Result<Option<Schedule>>`).
    pub fn get(&self, schedule_id: &str) -> Result<Option<Schedule>, SchedulerError> {
        let Some(record) = self
            .store
            .lock()
            .get_schedule(&environment_scope(self.environment), schedule_id)
            .map_err(SchedulerError::Store)?
        else {
            return Ok(None);
        };
        self.hydrate_schedule(record).map(Some)
    }

    /// Go `Pause`. Terminal schedules are returned unchanged.
    pub fn pause(&self, schedule_id: &str) -> Result<Option<Schedule>, SchedulerError> {
        let Some(mut schedule) = self.get(schedule_id)? else {
            return Ok(None);
        };
        if is_terminal_schedule_status(schedule.status) {
            return Ok(Some(schedule));
        }
        let now = self.clock.now();
        schedule.status = ScheduleStatus::Paused;
        schedule.paused_at = Some(now);
        schedule.updated_at = now;
        self.persist_schedule(&schedule)?;
        self.publish_event(
            "schedule.status_changed",
            &schedule,
            None,
            &kv(&[("status", serde_json::json!(schedule.status.as_str()))]),
        )?;
        Ok(Some(schedule))
    }

    /// Go `Resume`. Non-paused schedules are returned unchanged; the next due time is
    /// recomputed per kind.
    pub fn resume(&self, schedule_id: &str) -> Result<Option<Schedule>, SchedulerError> {
        let Some(mut schedule) = self.get(schedule_id)? else {
            return Ok(None);
        };
        if schedule.status != ScheduleStatus::Paused {
            return Ok(Some(schedule));
        }
        let now = self.clock.now();
        schedule.paused_at = None;
        schedule.updated_at = now;
        match schedule.kind {
            ScheduleKind::Recurring => {
                schedule.status = ScheduleStatus::Active;
                let next_due_at = next_due_after(&schedule.trigger, now)?;
                schedule.next_due_at = next_due_at;
                schedule.trigger.next_due_at = next_due_at;
            }
            ScheduleKind::OneTime => {
                schedule.status = ScheduleStatus::Scheduled;
                if let Some(fire_at) = schedule.trigger.fire_at {
                    if fire_at > now {
                        schedule.next_due_at = Some(fire_at);
                        schedule.trigger.next_due_at = Some(fire_at);
                    } else {
                        schedule.next_due_at = None;
                        schedule.trigger.next_due_at = None;
                    }
                }
            }
        }
        self.persist_schedule(&schedule)?;
        self.publish_event(
            "schedule.status_changed",
            &schedule,
            None,
            &kv(&[("status", serde_json::json!(schedule.status.as_str()))]),
        )?;
        Ok(Some(schedule))
    }

    /// Go `Cancel`. Already-cancelled schedules are returned unchanged.
    pub fn cancel(&self, schedule_id: &str) -> Result<Option<Schedule>, SchedulerError> {
        let Some(mut schedule) = self.get(schedule_id)? else {
            return Ok(None);
        };
        if schedule.status == ScheduleStatus::Cancelled {
            return Ok(Some(schedule));
        }
        let now = self.clock.now();
        schedule.status = ScheduleStatus::Cancelled;
        schedule.cancelled_at = Some(now);
        schedule.updated_at = now;
        self.persist_schedule(&schedule)?;
        self.publish_event(
            "schedule.status_changed",
            &schedule,
            None,
            &kv(&[("status", serde_json::json!(schedule.status.as_str()))]),
        )?;
        Ok(Some(schedule))
    }

    /// Go `CatchUp`: records missed recurring intervals and processes every schedule whose
    /// next due time is not in the future, with `catch_up` trigger source.
    pub fn catch_up(&self) -> Result<(), SchedulerError> {
        let items = self.list()?;
        let now = self.clock.now();
        for mut schedule in items {
            if schedule.next_due_at.is_none()
                || schedule.next_due_at > Some(now)
                || is_terminal_schedule_status(schedule.status)
            {
                continue;
            }
            if schedule.kind == ScheduleKind::Recurring
                && schedule.status != ScheduleStatus::Cancelled
            {
                self.record_missed_intervals(&mut schedule, now)?;
            }
            self.process_due_schedule(&mut schedule, now, TriggerSource::CatchUp)?;
        }
        Ok(())
    }

    /// Go `Tick`: per schedule, reconcile downstream status, process retries, then dispatch
    /// any due schedule with the `normal` trigger source.
    pub fn tick(&self) -> Result<(), SchedulerError> {
        let items = self.list()?;
        let now = self.clock.now();
        for mut schedule in items {
            self.reconcile_downstream(&mut schedule)?;
            self.process_retries(&mut schedule, now)?;
            if schedule.next_due_at.is_none() || schedule.next_due_at > Some(now) {
                continue;
            }
            self.process_due_schedule(&mut schedule, now, TriggerSource::Normal)?;
        }
        Ok(())
    }

    fn process_retries(
        &self,
        schedule: &mut Schedule,
        now: DateTime<Utc>,
    ) -> Result<(), SchedulerError> {
        for idx in 0..schedule.attempts.len() {
            let attempt = &schedule.attempts[idx];
            if attempt.dispatch_status != DispatchStatus::Failed
                || attempt.next_retry_at.is_none()
                || attempt.next_retry_at.unwrap() > now
            {
                continue;
            }
            return self.dispatch_attempt(schedule, idx, TriggerSource::Retry);
        }
        Ok(())
    }

    fn process_due_schedule(
        &self,
        schedule: &mut Schedule,
        now: DateTime<Utc>,
        source: TriggerSource,
    ) -> Result<(), SchedulerError> {
        let Some(next_due) = schedule.next_due_at else {
            return Ok(());
        };
        let attempt = DispatchAttempt {
            attempt_id: new_attempt_id(),
            schedule_id: schedule.schedule_id.clone(),
            due_at: next_due,
            trigger_source: source,
            dispatch_status: DispatchStatus::Pending,
            retry_budget: schedule.retry_policy.max_retries,
            downstream_status: DownstreamStatus::None,
            created_at: now,
            updated_at: now,
            ..DispatchAttempt::default()
        };

        if schedule.status == ScheduleStatus::Paused {
            let mut attempt = attempt;
            attempt.dispatch_status = DispatchStatus::SkippedPaused;
            attempt.skipped_reason = "schedule_paused".to_string();
            return self.finish_non_dispatch_attempt(schedule, attempt, now, true);
        }
        if schedule.status == ScheduleStatus::Cancelled {
            let mut attempt = attempt;
            attempt.dispatch_status = DispatchStatus::SkippedCancelled;
            attempt.skipped_reason = "schedule_cancelled".to_string();
            return self.finish_non_dispatch_attempt(schedule, attempt, now, true);
        }
        if is_terminal_schedule_status(schedule.status) {
            return Ok(());
        }
        if has_active_attempt(&schedule.attempts) {
            let mut attempt = attempt;
            attempt.dispatch_status = DispatchStatus::SkippedOverlap;
            attempt.skipped_reason = "schedule_execution_in_progress".to_string();
            return self.finish_non_dispatch_attempt(schedule, attempt, now, true);
        }

        schedule.attempts.insert(0, attempt);
        self.dispatch_attempt(schedule, 0, source)
    }

    fn finish_non_dispatch_attempt(
        &self,
        schedule: &mut Schedule,
        attempt: DispatchAttempt,
        now: DateTime<Utc>,
        advance: bool,
    ) -> Result<(), SchedulerError> {
        schedule.attempts.insert(0, attempt);
        schedule.last_attempt_at = Some(now);
        schedule.last_outcome = schedule.attempts[0].dispatch_status.as_str().to_string();
        schedule.updated_at = now;
        if advance {
            self.advance_schedule_after_due(schedule, now)?;
        }
        self.persist_schedule(schedule)?;
        self.publish_event(
            "schedule.dispatch_recorded",
            schedule,
            Some(&schedule.attempts[0]),
            &kv(&[
                (
                    "dispatchStatus",
                    serde_json::json!(schedule.attempts[0].dispatch_status.as_str()),
                ),
                (
                    "skippedReason",
                    serde_json::json!(schedule.attempts[0].skipped_reason),
                ),
            ]),
        )
    }

    fn dispatch_attempt(
        &self,
        schedule: &mut Schedule,
        attempt_index: usize,
        source: TriggerSource,
    ) -> Result<(), SchedulerError> {
        let now = self.clock.now();
        {
            let attempt = &mut schedule.attempts[attempt_index];
            attempt.trigger_source = source;
            attempt.dispatch_status = DispatchStatus::Dispatching;
            attempt.updated_at = now;
        }
        schedule.last_attempt_at = Some(now);
        schedule.updated_at = now;
        self.persist_schedule(schedule)?;
        self.publish_event(
            "schedule.dispatch_attempted",
            schedule,
            schedule.attempts.get(attempt_index),
            &kv(&[
                (
                    "dispatchStatus",
                    serde_json::json!(schedule.attempts[attempt_index].dispatch_status.as_str()),
                ),
                (
                    "dueAt",
                    serde_json::json!(schedule.attempts[attempt_index].due_at),
                ),
                (
                    "triggerSource",
                    serde_json::json!(schedule.attempts[attempt_index].trigger_source.as_str()),
                ),
            ]),
        )?;

        let target_result = self
            .store
            .lock()
            .get_schedule_target(&schedule.schedule_id, &schedule.target_ref_id)
            .map_err(SchedulerError::Store)?;
        let target_record = match target_result {
            Some(record) if record.active => record,
            Some(_) | None => {
                return self.record_dispatch_failure(
                    schedule,
                    attempt_index,
                    "invalid_target",
                    "schedule target reference is not available",
                    true,
                );
            }
        };
        let target = match decode_target_record(&target_record) {
            Ok(target) => target,
            Err(err) => {
                return self.record_dispatch_failure(
                    schedule,
                    attempt_index,
                    "invalid_target_document",
                    &err.to_string(),
                    true,
                );
            }
        };

        match target.kind {
            TargetKind::Run => {
                // Go guards against archived threads via store.GetThreadForSession, which is
                // not yet public in kura-store; the sync port treats every thread as active.
                let run_target = match target.run.as_ref() {
                    Some(run_target) => run_target.clone(),
                    None => {
                        return self.record_dispatch_failure(
                            schedule,
                            attempt_index,
                            "invalid_target_document",
                            "run target document is missing run details",
                            true,
                        );
                    }
                };
                // Go's billing branch pre-assigns input.RunID; without billing the runtime
                // manager generates the id (CreateRunInput::run_id empty).
                let input = kura_runtime::CreateRunInput {
                    session_id: run_target.session_id,
                    schedule_id: schedule.schedule_id.clone(),
                    schedule_attempt_id: schedule.attempts[attempt_index].attempt_id.clone(),
                    entrypoint: run_target.entrypoint,
                    goal: run_target.goal,
                    ..kura_runtime::CreateRunInput::default()
                };
                let run = match self.runtime.create_run(input) {
                    Ok(run) => run,
                    Err(err) => {
                        return self.record_dispatch_failure(
                            schedule,
                            attempt_index,
                            "run_create_failed",
                            &err.to_string(),
                            true,
                        );
                    }
                };
                // Go persists the run via store.UpsertRun and saves a thread runtime
                // projection; neither is public in kura-store yet, so the run lives in the
                // runtime manager's in-memory ledger only.
                let attempt = &mut schedule.attempts[attempt_index];
                attempt.run_id = run.run_id.clone();
                attempt.resolved_target_revision = target.revision;
                attempt.dispatch_status = DispatchStatus::Dispatched;
                attempt.downstream_status = map_run_status(run.status);
                attempt.updated_at = now;
                schedule.last_outcome = attempt.dispatch_status.as_str().to_string();
                self.advance_schedule_after_dispatch(schedule, now)?;
            }
            TargetKind::Workflow => {
                let Some(launcher) = &self.workflow_launcher else {
                    return self.record_dispatch_failure(
                        schedule,
                        attempt_index,
                        "workflow_launcher_unavailable",
                        "scheduler workflow launcher is not configured",
                        true,
                    );
                };
                let workflow_target = match target.workflow.as_ref() {
                    Some(workflow_target) => workflow_target.clone(),
                    None => {
                        return self.record_dispatch_failure(
                            schedule,
                            attempt_index,
                            "invalid_target_document",
                            "workflow target document is missing workflow details",
                            true,
                        );
                    }
                };
                let result = match launcher.launch_scheduled_workflow(
                    &workflow_target,
                    &schedule.schedule_id,
                    &schedule.attempts[attempt_index].attempt_id,
                ) {
                    Ok(result) => result,
                    Err(err) => {
                        return self.record_dispatch_failure(
                            schedule,
                            attempt_index,
                            "workflow_dispatch_failed",
                            &err,
                            true,
                        );
                    }
                };
                let attempt = &mut schedule.attempts[attempt_index];
                attempt.run_id = result.run_id;
                attempt.workflow_id = result.workflow_id;
                attempt.resolved_target_revision = target.revision;
                attempt.dispatch_status = DispatchStatus::Dispatched;
                attempt.downstream_status = result.downstream_status;
                attempt.updated_at = now;
                schedule.last_outcome = attempt.dispatch_status.as_str().to_string();
                self.advance_schedule_after_dispatch(schedule, now)?;
            }
        }

        self.persist_schedule(schedule)?;
        self.publish_event(
            "schedule.dispatch_recorded",
            schedule,
            schedule.attempts.get(attempt_index),
            &kv(&[
                (
                    "dispatchStatus",
                    serde_json::json!(schedule.attempts[attempt_index].dispatch_status.as_str()),
                ),
                (
                    "resolvedTargetRevision",
                    serde_json::json!(schedule.attempts[attempt_index].resolved_target_revision),
                ),
                (
                    "runId",
                    serde_json::json!(schedule.attempts[attempt_index].run_id),
                ),
                (
                    "workflowId",
                    serde_json::json!(schedule.attempts[attempt_index].workflow_id),
                ),
            ]),
        )
    }

    /// Go `recordDispatchFailure`: either schedules a retry or exhausts the attempt and
    /// advances the schedule.
    fn record_dispatch_failure(
        &self,
        schedule: &mut Schedule,
        attempt_index: usize,
        class: &str,
        reason: &str,
        allow_retry: bool,
    ) -> Result<(), SchedulerError> {
        let now = self.clock.now();
        let policy = schedule.retry_policy;
        let kind = schedule.kind;
        let next_retry_at = next_retry_time(&policy, schedule.attempts[attempt_index].retry_count);
        let mut exhausted = false;
        {
            let attempt = &mut schedule.attempts[attempt_index];
            attempt.failure_class = class.to_string();
            attempt.failure_reason = reason.to_string();
            attempt.updated_at = now;
            if allow_retry && attempt.retry_count < policy.max_retries && next_retry_at.is_some() {
                attempt.dispatch_status = DispatchStatus::Failed;
                attempt.next_retry_at = next_retry_at;
                attempt.retry_count += 1;
            } else {
                attempt.dispatch_status = DispatchStatus::Exhausted;
                attempt.next_retry_at = None;
                exhausted = true;
            }
            schedule.last_outcome = attempt.dispatch_status.as_str().to_string();
        }
        if exhausted {
            if kind == ScheduleKind::OneTime {
                schedule.status = ScheduleStatus::DispatchFailed;
                schedule.completed_at = Some(now);
                schedule.next_due_at = None;
                schedule.trigger.next_due_at = None;
            } else {
                self.advance_schedule_after_dispatch(schedule, now)?;
            }
        }
        schedule.last_attempt_at = Some(now);
        schedule.updated_at = now;
        self.persist_schedule(schedule)?;
        let attempt = schedule.attempts[attempt_index].clone();
        if attempt.next_retry_at.is_some() {
            self.publish_event(
                "schedule.retry_scheduled",
                schedule,
                Some(&attempt),
                &kv(&[
                    (
                        "dispatchStatus",
                        serde_json::json!(attempt.dispatch_status.as_str()),
                    ),
                    ("failureClass", serde_json::json!(attempt.failure_class)),
                    ("failureReason", serde_json::json!(attempt.failure_reason)),
                    ("retryCount", serde_json::json!(attempt.retry_count)),
                    ("nextRetryAt", serde_json::json!(attempt.next_retry_at)),
                ]),
            )?;
        }
        self.publish_event(
            "schedule.dispatch_recorded",
            schedule,
            Some(&attempt),
            &kv(&[
                (
                    "dispatchStatus",
                    serde_json::json!(attempt.dispatch_status.as_str()),
                ),
                ("failureClass", serde_json::json!(attempt.failure_class)),
                ("failureReason", serde_json::json!(attempt.failure_reason)),
                ("retryCount", serde_json::json!(attempt.retry_count)),
                ("nextRetryAt", serde_json::json!(attempt.next_retry_at)),
            ]),
        )
    }

    fn reconcile_downstream(&self, schedule: &mut Schedule) -> Result<(), SchedulerError> {
        let mut changed = false;
        for idx in 0..schedule.attempts.len() {
            let attempt = &mut schedule.attempts[idx];
            if attempt.dispatch_status != DispatchStatus::Dispatched || attempt.run_id.is_empty() {
                continue;
            }
            let next = if attempt.workflow_id.is_empty() {
                match self.runtime.get_run(&attempt.run_id) {
                    Some(run) => map_run_status(run.status),
                    None => continue,
                }
            } else {
                match self
                    .store
                    .lock()
                    .get_workflow(
                        &schedule.environment_scope,
                        &attempt.run_id,
                        &attempt.workflow_id,
                    )
                    .map_err(SchedulerError::Store)?
                {
                    Some(workflow) => map_workflow_status(workflow.status),
                    None => continue,
                }
            };
            if next != attempt.downstream_status {
                attempt.downstream_status = next;
                attempt.updated_at = self.clock.now();
                changed = true;
                if is_terminal_downstream(next) {
                    schedule.last_outcome = next.as_str().to_string();
                    schedule.updated_at = attempt.updated_at;
                }
            }
        }
        if !changed {
            return Ok(());
        }
        self.persist_schedule(schedule)
    }

    fn record_missed_intervals(
        &self,
        schedule: &mut Schedule,
        now: DateTime<Utc>,
    ) -> Result<(), SchedulerError> {
        let Some(next_due) = schedule.next_due_at else {
            return Ok(());
        };
        if schedule.kind != ScheduleKind::Recurring {
            return Ok(());
        }
        let due = next_due;
        let mut missed_count = 0i64;
        let mut cursor = due;
        loop {
            let next_due_at = next_due_after(&schedule.trigger, cursor)?;
            match next_due_at {
                Some(next) if next < now => {
                    missed_count += 1;
                    cursor = next;
                }
                _ => break,
            }
        }
        if missed_count == 0 {
            return Ok(());
        }
        let attempt = DispatchAttempt {
            attempt_id: new_attempt_id(),
            schedule_id: schedule.schedule_id.clone(),
            due_at: due,
            trigger_source: TriggerSource::CatchUp,
            dispatch_status: DispatchStatus::Missed,
            missed_count,
            downstream_status: DownstreamStatus::None,
            created_at: now,
            updated_at: now,
            ..DispatchAttempt::default()
        };
        schedule.attempts.insert(0, attempt);
        schedule.last_attempt_at = Some(now);
        schedule.last_outcome = DispatchStatus::Missed.as_str().to_string();
        schedule.next_due_at = Some(cursor);
        schedule.trigger.next_due_at = Some(cursor);
        schedule.updated_at = now;
        self.persist_schedule(schedule)?;
        self.publish_event(
            "schedule.dispatch_recorded",
            schedule,
            Some(&schedule.attempts[0]),
            &kv(&[
                (
                    "dispatchStatus",
                    serde_json::json!(schedule.attempts[0].dispatch_status.as_str()),
                ),
                (
                    "missedCount",
                    serde_json::json!(schedule.attempts[0].missed_count),
                ),
            ]),
        )
    }

    fn advance_schedule_after_due(
        &self,
        schedule: &mut Schedule,
        now: DateTime<Utc>,
    ) -> Result<(), SchedulerError> {
        match schedule.kind {
            ScheduleKind::OneTime => {
                schedule.next_due_at = None;
                schedule.trigger.next_due_at = None;
            }
            ScheduleKind::Recurring => {
                let next_due_at = next_due_after(&schedule.trigger, now)?;
                schedule.next_due_at = next_due_at;
                schedule.trigger.next_due_at = next_due_at;
            }
        }
        Ok(())
    }

    fn advance_schedule_after_dispatch(
        &self,
        schedule: &mut Schedule,
        now: DateTime<Utc>,
    ) -> Result<(), SchedulerError> {
        match schedule.kind {
            ScheduleKind::OneTime => {
                schedule.status = ScheduleStatus::Completed;
                schedule.completed_at = Some(now);
                schedule.next_due_at = None;
                schedule.trigger.next_due_at = None;
            }
            ScheduleKind::Recurring => {
                schedule.status = ScheduleStatus::Active;
                let next_due_at = next_due_after(&schedule.trigger, now)?;
                schedule.next_due_at = next_due_at;
                schedule.trigger.next_due_at = next_due_at;
            }
        }
        schedule.updated_at = now;
        Ok(())
    }

    /// Go `hydrateSchedule`: decode the document (when present), load target + attempts from
    /// the store, then overwrite the ledger columns with the record values.
    fn hydrate_schedule(&self, record: ScheduleRecord) -> Result<Schedule, SchedulerError> {
        let mut schedule: Schedule = if record.document.is_empty() {
            Schedule::default()
        } else {
            serde_json::from_str(&record.document).map_err(|err| {
                SchedulerError::DecodeSchedule(record.schedule_id.clone(), err.to_string())
            })?
        };
        if schedule.schedule_id.is_empty() {
            schedule.schedule_id = record.schedule_id.clone();
            schedule.environment_scope = record.environment_scope.clone();
            schedule.tenant_id = record.tenant_id.clone();
            schedule.kind = parse_enum(&record.kind)
                .map_err(|err| SchedulerError::DecodeSchedule(record.schedule_id.clone(), err))?;
            schedule.status = parse_enum(&record.status)
                .map_err(|err| SchedulerError::DecodeSchedule(record.schedule_id.clone(), err))?;
            schedule.target_ref_id = record.target_ref_id.clone();
            schedule.created_at = record.created_at;
            schedule.updated_at = record.updated_at;
            schedule.next_due_at = record.next_due_at;
            schedule.last_attempt_at = record.last_attempt_at;
            schedule.last_outcome = record.last_outcome.clone();
            schedule.paused_at = record.paused_at;
            schedule.cancelled_at = record.cancelled_at;
            schedule.completed_at = record.completed_at;
        }
        if let Some(target_record) = self
            .store
            .lock()
            .get_schedule_target(&record.schedule_id, &record.target_ref_id)
            .map_err(SchedulerError::Store)?
        {
            schedule.target = decode_target_record(&target_record)?;
        }
        let attempt_records = self
            .store
            .lock()
            .list_schedule_dispatch_attempts(&record.schedule_id)
            .map_err(SchedulerError::Store)?;
        let mut attempts = Vec::with_capacity(attempt_records.len());
        for attempt_record in attempt_records {
            attempts.push(decode_attempt_record(&attempt_record)?);
        }
        schedule.attempts = attempts;
        schedule.environment_scope = record.environment_scope;
        schedule.tenant_id = record.tenant_id;
        schedule.kind = parse_enum(&record.kind)
            .map_err(|err| SchedulerError::DecodeSchedule(record.schedule_id.clone(), err))?;
        schedule.status = parse_enum(&record.status)
            .map_err(|err| SchedulerError::DecodeSchedule(record.schedule_id.clone(), err))?;
        schedule.target_ref_id = record.target_ref_id;
        schedule.next_due_at = record.next_due_at;
        schedule.last_attempt_at = record.last_attempt_at;
        schedule.last_outcome = record.last_outcome;
        schedule.created_at = record.created_at;
        schedule.updated_at = record.updated_at;
        schedule.paused_at = record.paused_at;
        schedule.cancelled_at = record.cancelled_at;
        schedule.completed_at = record.completed_at;
        schedule.trigger.next_due_at = record.next_due_at;
        Ok(schedule)
    }

    /// Go `persistSchedule`: upsert the schedule, target, and every attempt record with the
    /// camelCase JSON documents.
    fn persist_schedule(&self, schedule: &Schedule) -> Result<(), SchedulerError> {
        let schedule_doc = serde_json::to_string(schedule).map_err(|err| {
            SchedulerError::MarshalSchedule(schedule.schedule_id.clone(), err.to_string())
        })?;
        self.store
            .lock()
            .upsert_schedule(&ScheduleRecord {
                schedule_id: schedule.schedule_id.clone(),
                environment_scope: schedule.environment_scope.clone(),
                tenant_id: schedule.tenant_id.clone(),
                kind: schedule.kind.as_str().to_string(),
                status: schedule.status.as_str().to_string(),
                target_ref_id: schedule.target_ref_id.clone(),
                timezone: schedule.trigger.timezone.clone(),
                next_due_at: schedule.next_due_at,
                last_attempt_at: schedule.last_attempt_at,
                last_outcome: schedule.last_outcome.clone(),
                created_at: schedule.created_at,
                updated_at: schedule.updated_at,
                paused_at: schedule.paused_at,
                cancelled_at: schedule.cancelled_at,
                completed_at: schedule.completed_at,
                document: schedule_doc,
            })
            .map_err(SchedulerError::Store)?;

        let target_doc = serde_json::to_string(&schedule.target).map_err(|err| {
            SchedulerError::MarshalScheduleTarget(schedule.target_ref_id.clone(), err.to_string())
        })?;
        self.store
            .lock()
            .upsert_schedule_target(&ScheduleTargetRecord {
                target_ref_id: schedule.target_ref_id.clone(),
                schedule_id: schedule.schedule_id.clone(),
                target_kind: schedule.target.kind.as_str().to_string(),
                revision: schedule.target.revision,
                active: schedule.target.active,
                updated_at: schedule.target.updated_at,
                document: target_doc,
            })
            .map_err(SchedulerError::Store)?;

        for attempt in &schedule.attempts {
            let attempt_doc = serde_json::to_string(attempt).map_err(|err| {
                SchedulerError::MarshalScheduleAttempt(attempt.attempt_id.clone(), err.to_string())
            })?;
            self.store
                .lock()
                .upsert_schedule_dispatch_attempt(&ScheduleDispatchAttemptRecord {
                    attempt_id: attempt.attempt_id.clone(),
                    schedule_id: schedule.schedule_id.clone(),
                    due_at: attempt.due_at,
                    trigger_source: attempt.trigger_source.as_str().to_string(),
                    dispatch_status: attempt.dispatch_status.as_str().to_string(),
                    failure_class: attempt.failure_class.clone(),
                    failure_reason: attempt.failure_reason.clone(),
                    retry_count: attempt.retry_count,
                    retry_budget: attempt.retry_budget,
                    next_retry_at: attempt.next_retry_at,
                    resolved_target_revision: attempt.resolved_target_revision,
                    run_id: attempt.run_id.clone(),
                    workflow_id: attempt.workflow_id.clone(),
                    downstream_status: attempt.downstream_status.as_str().to_string(),
                    skipped_reason: attempt.skipped_reason.clone(),
                    missed_count: attempt.missed_count,
                    created_at: attempt.created_at,
                    updated_at: attempt.updated_at,
                    document: attempt_doc,
                })
                .map_err(SchedulerError::Store)?;
        }
        Ok(())
    }

    /// Go `publishEvent`: builds the schedule event envelope and publishes to the Bus. The
    /// Go store append (`AppendEvent`) is excluded (not public in kura-store yet).
    fn publish_event(
        &self,
        name: &str,
        schedule: &Schedule,
        attempt: Option<&DispatchAttempt>,
        payload: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), SchedulerError> {
        let Some(bus) = &self.event_bus else {
            return Ok(());
        };
        let mut event = kura_events::Event {
            environment_scope: schedule.environment_scope.clone(),
            tenant_id: schedule.tenant_id.clone(),
            category: "schedule".to_string(),
            name: name.to_string(),
            scope: kura_events::Scope {
                schedule_id: schedule.schedule_id.clone(),
                ..kura_events::Scope::default()
            },
            resource: kura_events::Resource {
                kind: "schedule".to_string(),
                id: schedule.schedule_id.clone(),
            },
            payload: serde_json::Map::new(),
            ..kura_events::Event::default()
        };
        event.payload.insert(
            "scheduleId".to_string(),
            serde_json::json!(schedule.schedule_id),
        );
        event.payload.insert(
            "status".to_string(),
            serde_json::json!(schedule.status.as_str()),
        );
        if let Some(attempt) = attempt {
            event.scope.schedule_attempt_id = attempt.attempt_id.clone();
            event.payload.insert(
                "scheduleAttemptId".to_string(),
                serde_json::json!(attempt.attempt_id),
            );
            event.payload.insert(
                "dispatchStatus".to_string(),
                serde_json::json!(attempt.dispatch_status.as_str()),
            );
            event
                .payload
                .insert("dueAt".to_string(), serde_json::json!(attempt.due_at));
            event.payload.insert(
                "triggerSource".to_string(),
                serde_json::json!(attempt.trigger_source.as_str()),
            );
            if !attempt.run_id.is_empty() {
                event.scope.run_id = attempt.run_id.clone();
                event
                    .payload
                    .insert("runId".to_string(), serde_json::json!(attempt.run_id));
            }
            if !attempt.workflow_id.is_empty() {
                event.scope.workflow_id = attempt.workflow_id.clone();
                event.payload.insert(
                    "workflowId".to_string(),
                    serde_json::json!(attempt.workflow_id),
                );
            }
        }
        for (key, value) in payload {
            event.payload.insert(key.clone(), value.clone());
        }
        bus.publish(event);
        Ok(())
    }
}

/// Go `NextDueAfter` (exported).
pub fn next_due_after(
    trigger: &Trigger,
    after: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, SchedulerError> {
    match trigger.kind {
        TriggerKind::Once => {
            let Some(fire_at) = trigger.fire_at else {
                return Err(SchedulerError::Trigger(
                    "one-time trigger requires fireAt".to_string(),
                ));
            };
            let fire_at = fire_at.with_timezone(&Utc);
            if fire_at > after.with_timezone(&Utc) {
                Ok(Some(fire_at))
            } else {
                Ok(None)
            }
        }
        TriggerKind::Cron => next_cron_due_after(&trigger.cron_expr, &trigger.timezone, after),
    }
}

fn next_cron_due_after(
    expr: &str,
    timezone: &str,
    after: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, SchedulerError> {
    if expr.trim().is_empty() {
        return Err(SchedulerError::Cron(
            "cron expression is required".to_string(),
        ));
    }
    let tz: Tz = timezone
        .trim()
        .parse()
        .map_err(|_| SchedulerError::Cron(format!("load timezone {:?}", timezone)))?;
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(SchedulerError::Cron(
            "cron expression must have 5 fields".to_string(),
        ));
    }
    let minutes = parse_cron_field(fields[0], 0, 59)
        .map_err(|e| SchedulerError::Cron(format!("parse minute field: {e}")))?;
    let hours = parse_cron_field(fields[1], 0, 23)
        .map_err(|e| SchedulerError::Cron(format!("parse hour field: {e}")))?;
    let days = parse_cron_field(fields[2], 1, 31)
        .map_err(|e| SchedulerError::Cron(format!("parse day-of-month field: {e}")))?;
    let months = parse_cron_field(fields[3], 1, 12)
        .map_err(|e| SchedulerError::Cron(format!("parse month field: {e}")))?;
    let weekdays = parse_cron_field(fields[4], 0, 6)
        .map_err(|e| SchedulerError::Cron(format!("parse weekday field: {e}")))?;

    // cursor = after.In(location).Add(time.Minute).Truncate(time.Minute)
    let after_local = after.with_timezone(&tz);
    let mut cursor = after_local + chrono::Duration::minutes(1);
    cursor = cursor
        .with_second(0)
        .and_then(|c| c.with_nanosecond(0))
        .unwrap_or(cursor);
    let Some(deadline) = cursor.checked_add_months(chrono::Months::new(12)) else {
        return Err(SchedulerError::Cron(
            "no matching cron time found within one year".to_string(),
        ));
    };
    loop {
        if cursor > deadline {
            return Err(SchedulerError::Cron(
                "no matching cron time found within one year".to_string(),
            ));
        }
        if minutes.contains(&(cursor.minute() as i64))
            && hours.contains(&(cursor.hour() as i64))
            && days.contains(&(cursor.day() as i64))
            && months.contains(&(cursor.month() as i64))
            && weekdays.contains(&(cursor.weekday().num_days_from_sunday() as i64))
        {
            return Ok(Some(cursor.with_timezone(&Utc)));
        }
        cursor += chrono::Duration::minutes(1);
    }
}

fn parse_cron_field(field: &str, min: i64, max: i64) -> Result<BTreeSet<i64>, String> {
    let mut allowed = BTreeSet::new();
    for part in field.trim().split(',') {
        let part = part.trim();
        if part.is_empty() {
            return Err("empty cron field component".to_string());
        }
        if part == "*" {
            for value in min..=max {
                allowed.insert(value);
            }
            continue;
        }
        if let Some(step) = part.strip_prefix("*/") {
            let step: i64 = step.parse().map_err(|_| format!("invalid step {part:?}"))?;
            if step <= 0 {
                return Err(format!("invalid step {part:?}"));
            }
            let mut value = min;
            while value <= max {
                allowed.insert(value);
                value += step;
            }
            continue;
        }
        if part.contains('-') {
            let mut bounds = part.splitn(2, '-');
            let start: i64 = bounds
                .next()
                .and_then(|b| b.parse().ok())
                .ok_or_else(|| format!("invalid range start {part:?}"))?;
            let end: i64 = bounds
                .next()
                .and_then(|b| b.parse().ok())
                .ok_or_else(|| format!("invalid range end {part:?}"))?;
            if start > end || start < min || end > max {
                return Err(format!("range {part:?} out of bounds"));
            }
            for value in start..=end {
                allowed.insert(value);
            }
            continue;
        }
        let value: i64 = part
            .parse()
            .map_err(|_| format!("invalid value {part:?}"))?;
        if value < min || value > max {
            return Err(format!("value {value} out of bounds"));
        }
        allowed.insert(value);
    }
    if allowed.is_empty() {
        return Err(format!("empty cron field {field:?}"));
    }
    Ok(allowed)
}

fn decode_target_record(record: &ScheduleTargetRecord) -> Result<Target, SchedulerError> {
    let mut target: Target = if record.document.is_empty() {
        Target::default()
    } else {
        serde_json::from_str(&record.document).map_err(|err| {
            SchedulerError::DecodeScheduleTarget(record.target_ref_id.clone(), err.to_string())
        })?
    };
    target.kind = parse_enum(&record.target_kind)
        .map_err(|err| SchedulerError::DecodeScheduleTarget(record.target_ref_id.clone(), err))?;
    target.revision = record.revision;
    target.active = record.active;
    target.updated_at = record.updated_at;
    target.summary = target_summary(&target);
    Ok(target)
}

fn decode_attempt_record(
    record: &ScheduleDispatchAttemptRecord,
) -> Result<DispatchAttempt, SchedulerError> {
    let mut attempt: DispatchAttempt = if record.document.is_empty() {
        DispatchAttempt::default()
    } else {
        serde_json::from_str(&record.document).map_err(|err| {
            SchedulerError::DecodeScheduleAttempt(record.attempt_id.clone(), err.to_string())
        })?
    };
    attempt.attempt_id = record.attempt_id.clone();
    attempt.schedule_id = record.schedule_id.clone();
    attempt.due_at = record.due_at;
    attempt.trigger_source = parse_enum(&record.trigger_source)
        .map_err(|err| SchedulerError::DecodeScheduleAttempt(record.attempt_id.clone(), err))?;
    attempt.dispatch_status = parse_enum(&record.dispatch_status)
        .map_err(|err| SchedulerError::DecodeScheduleAttempt(record.attempt_id.clone(), err))?;
    attempt.failure_class = record.failure_class.clone();
    attempt.failure_reason = record.failure_reason.clone();
    attempt.retry_count = record.retry_count;
    attempt.retry_budget = record.retry_budget;
    attempt.next_retry_at = record.next_retry_at;
    attempt.resolved_target_revision = record.resolved_target_revision;
    attempt.run_id = record.run_id.clone();
    attempt.workflow_id = record.workflow_id.clone();
    attempt.downstream_status = parse_enum(&record.downstream_status)
        .map_err(|err| SchedulerError::DecodeScheduleAttempt(record.attempt_id.clone(), err))?;
    attempt.skipped_reason = record.skipped_reason.clone();
    attempt.missed_count = record.missed_count;
    attempt.created_at = record.created_at;
    attempt.updated_at = record.updated_at;
    Ok(attempt)
}

fn parse_enum<T: serde::de::DeserializeOwned>(value: &str) -> Result<T, String> {
    serde_json::from_str(&format!("\"{value}\""))
        .map_err(|e| format!("invalid enum value {value}: {e}"))
}

#[must_use]
fn derive_schedule_kind(kind: TriggerKind) -> ScheduleKind {
    if kind == TriggerKind::Cron {
        ScheduleKind::Recurring
    } else {
        ScheduleKind::OneTime
    }
}

#[must_use]
fn initial_schedule_status(kind: ScheduleKind) -> ScheduleStatus {
    if kind == ScheduleKind::Recurring {
        ScheduleStatus::Active
    } else {
        ScheduleStatus::Scheduled
    }
}

#[must_use]
fn normalize_retry_policy(mut policy: RetryPolicy) -> RetryPolicy {
    // Go backfills an empty BackoffKind to Fixed; the Rust enum cannot be empty and its
    // default is Fixed, so the behavior is identical without the branch.
    if policy.base_delay_seconds <= 0 {
        policy.base_delay_seconds = 5;
    }
    if policy.max_delay_seconds <= 0 {
        policy.max_delay_seconds = policy.base_delay_seconds;
    }
    if policy.max_retries < 0 {
        policy.max_retries = 0;
    }
    policy
}

/// Go `nextRetryTime` — note it uses the REAL wall clock (`time.Now()`), not the
/// scheduler clock, and that quirk is preserved.
fn next_retry_time(policy: &RetryPolicy, retry_count: i64) -> Option<DateTime<Utc>> {
    if retry_count >= policy.max_retries {
        return None;
    }
    let mut delay = policy.base_delay_seconds;
    if policy.backoff_kind == RetryBackoffKind::Exponential {
        delay = policy.base_delay_seconds << retry_count.min(62);
    }
    if delay > policy.max_delay_seconds {
        delay = policy.max_delay_seconds;
    }
    Some(Utc::now() + chrono::Duration::seconds(delay))
}

#[must_use]
fn has_active_attempt(items: &[DispatchAttempt]) -> bool {
    items.iter().any(|item| {
        item.dispatch_status == DispatchStatus::Dispatched
            && is_active_downstream_status(item.downstream_status)
    })
}

#[must_use]
fn map_run_status(status: kura_runtime::RunStatus) -> DownstreamStatus {
    match status {
        kura_runtime::RunStatus::Completed => DownstreamStatus::Completed,
        kura_runtime::RunStatus::Failed => DownstreamStatus::Failed,
        kura_runtime::RunStatus::Cancelled => DownstreamStatus::Cancelled,
        _ => DownstreamStatus::Running,
    }
}

#[must_use]
fn map_workflow_status(status: kura_orchestration::WorkflowStatus) -> DownstreamStatus {
    match status {
        kura_orchestration::WorkflowStatus::Completed => DownstreamStatus::Completed,
        kura_orchestration::WorkflowStatus::PlanningFailed
        | kura_orchestration::WorkflowStatus::Failed
        | kura_orchestration::WorkflowStatus::PartialFailed
        | kura_orchestration::WorkflowStatus::Blocked => DownstreamStatus::Failed,
        kura_orchestration::WorkflowStatus::Cancelled => DownstreamStatus::Cancelled,
        kura_orchestration::WorkflowStatus::Interrupted => DownstreamStatus::Interrupted,
        _ => DownstreamStatus::Running,
    }
}

#[must_use]
fn target_summary(target: &Target) -> String {
    match target.kind {
        TargetKind::Workflow => match &target.workflow {
            Some(workflow) => first_non_empty(&[
                &workflow.workflow_goal,
                &workflow.run_goal,
                &workflow.entrypoint,
            ]),
            None => target.kind.as_str().to_string(),
        },
        TargetKind::Run => match &target.run {
            Some(run_target) => first_non_empty(&[&run_target.goal, &run_target.entrypoint]),
            None => target.kind.as_str().to_string(),
        },
    }
}

#[must_use]
fn first_non_empty(values: &[&str]) -> String {
    for value in values {
        if !value.is_empty() {
            return value.to_string();
        }
    }
    String::new()
}

#[must_use]
fn environment_scope(environment: kura_config::Environment) -> String {
    match environment {
        kura_config::Environment::Prod => "prod".to_string(),
        kura_config::Environment::Test => "test".to_string(),
    }
}

#[must_use]
fn kv(entries: &[(&str, serde_json::Value)]) -> serde_json::Map<String, serde_json::Value> {
    let mut map = serde_json::Map::new();
    for (key, value) in entries {
        map.insert(key.to_string(), value.clone());
    }
    map
}

#[must_use]
fn new_schedule_id() -> String {
    format!("sched_{}", &Uuid::new_v4().simple().to_string()[..16])
}

#[must_use]
fn new_target_ref_id() -> String {
    format!(
        "sched_target_{}",
        &Uuid::new_v4().simple().to_string()[..16]
    )
}

#[must_use]
fn new_attempt_id() -> String {
    format!(
        "sched_attempt_{}",
        &Uuid::new_v4().simple().to_string()[..16]
    )
}
