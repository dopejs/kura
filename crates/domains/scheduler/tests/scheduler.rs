//! Integration tests for the kura-scheduler port: wire-format round trips, the schedule
//! lifecycle (create/list/get/pause/resume/cancel), one-time dispatch exactly-once, recurring
//! overlap + pause/resume truth, retry/exhausted failure semantics, workflow-target dispatch
//! through the launcher, cron due computation, catch-up missed-interval recording, and
//! persistence across scheduler instances. Persistence tests use `kura_store::SQLiteStore`
//! in a temp dir, mirroring the Go scheduler harness.

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Utc};
use kura_events::Filter;
use kura_scheduler::{
    Clock, CreateInput, Dependencies, DispatchAttempt, DispatchStatus, DownstreamStatus,
    RetryBackoffKind, RetryPolicy, RunTarget, Schedule, ScheduleKind, ScheduleStatus, Scheduler,
    Target, TargetKind, Trigger, TriggerKind, WorkflowLaunchResult, WorkflowLauncher,
    WorkflowTarget, next_due_after,
};
use kura_store::SQLiteStore;

fn parse(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}

#[derive(Clone)]
struct FakeClock {
    now: Arc<Mutex<DateTime<Utc>>>,
}

impl FakeClock {
    fn new(now: DateTime<Utc>) -> Self {
        FakeClock {
            now: Arc::new(Mutex::new(now)),
        }
    }

    fn set(&self, now: DateTime<Utc>) {
        *self.now.lock().unwrap() = now;
    }
}

impl Clock for FakeClock {
    fn now(&self) -> DateTime<Utc> {
        *self.now.lock().unwrap()
    }
}

fn temp_dir() -> String {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("kura_scheduler_test_{}_{}", std::process::id(), n));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().to_string()
}

struct Harness {
    clock: FakeClock,
    store: Arc<parking_lot::Mutex<SQLiteStore>>,
    runtime: Arc<kura_runtime::Manager>,
    scheduler: Scheduler,
    bus: kura_events::Bus,
}

fn harness_with_launcher(
    now: DateTime<Utc>,
    launcher: Option<Arc<dyn WorkflowLauncher>>,
) -> Harness {
    let store = Arc::new(parking_lot::Mutex::new(
        SQLiteStore::new(&temp_dir()).unwrap(),
    ));
    let runtime = Arc::new(kura_runtime::Manager::new());
    let bus = kura_events::Bus::new();
    let clock = FakeClock::new(now);
    let scheduler = Scheduler::new(Dependencies {
        environment: kura_config::Environment::Test,
        runtime: Arc::clone(&runtime),
        event_bus: Some(bus.clone()),
        store: Arc::clone(&store),
        workflow_launcher: launcher,
        clock: Some(Box::new(clock.clone())),
        tick_interval: Duration::from_millis(10),
    });
    Harness {
        clock,
        store,
        runtime,
        scheduler,
        bus,
    }
}

fn harness(now: DateTime<Utc>) -> Harness {
    harness_with_launcher(now, None)
}

fn one_time_run_input(fire_at: DateTime<Utc>, goal: &str, max_retries: i64) -> CreateInput {
    CreateInput {
        trigger: Trigger {
            kind: TriggerKind::Once,
            fire_at: Some(fire_at),
            ..Default::default()
        },
        target: Target {
            kind: TargetKind::Run,
            run: Some(RunTarget {
                entrypoint: "operator".to_string(),
                goal: goal.to_string(),
                ..Default::default()
            }),
            ..Default::default()
        },
        retry_policy: RetryPolicy {
            max_retries,
            backoff_kind: RetryBackoffKind::Fixed,
            base_delay_seconds: 5,
            max_delay_seconds: 5,
        },
    }
}

fn complete_run(runtime: &kura_runtime::Manager, run_id: &str) {
    let step = runtime
        .create_step(
            run_id,
            kura_runtime::CreateStepInput {
                title: "complete scheduled run".to_string(),
                kind: "task".to_string(),
                ..Default::default()
            },
        )
        .unwrap();
    for (status, output) in [
        (kura_runtime::StepStatus::Planning, None),
        (kura_runtime::StepStatus::ExecutingTool, None),
        (
            kura_runtime::StepStatus::Completed,
            Some(serde_json::json!({ "ok": true })),
        ),
    ] {
        let _ = runtime
            .update_step_status_and_reconcile_run(
                run_id,
                &step.step_id,
                kura_runtime::UpdateStepStatusInput {
                    status,
                    output,
                    ..Default::default()
                },
            )
            .unwrap();
    }
}

// ---------------------------------------------------------------------------
// Wire format
// ---------------------------------------------------------------------------

#[test]
fn wire_format_round_trips() {
    let now = parse("2026-04-22T10:00:00Z");
    let schedule = Schedule {
        schedule_id: "sched_wire".to_string(),
        environment_scope: "test".to_string(),
        kind: ScheduleKind::OneTime,
        status: ScheduleStatus::Scheduled,
        target_ref_id: "sched_target_wire".to_string(),
        trigger: Trigger {
            kind: TriggerKind::Once,
            fire_at: Some(now),
            ..Default::default()
        },
        target: Target {
            kind: TargetKind::Run,
            revision: 1,
            active: true,
            run: Some(RunTarget {
                entrypoint: "operator".to_string(),
                goal: "wire".to_string(),
                ..Default::default()
            }),
            summary: "wire".to_string(),
            updated_at: now,
            ..Default::default()
        },
        retry_policy: RetryPolicy {
            max_retries: 2,
            backoff_kind: RetryBackoffKind::Exponential,
            base_delay_seconds: 5,
            max_delay_seconds: 60,
        },
        next_due_at: Some(now),
        created_at: now,
        updated_at: now,
        attempts: vec![DispatchAttempt {
            attempt_id: "sched_attempt_wire".to_string(),
            schedule_id: "sched_wire".to_string(),
            due_at: now,
            trigger_source: kura_scheduler::TriggerSource::Normal,
            dispatch_status: DispatchStatus::Dispatched,
            retry_count: 0,
            retry_budget: 2,
            run_id: "run_wire".to_string(),
            downstream_status: DownstreamStatus::Running,
            created_at: now,
            updated_at: now,
            ..Default::default()
        }],
        ..Default::default()
    };
    let json = serde_json::to_string(&schedule).unwrap();
    assert!(
        json.contains("\"scheduleAttemptId\":\"sched_attempt_wire\""),
        "{json}"
    );
    assert!(json.contains("\"scheduleId\":\"sched_wire\""), "{json}");
    assert!(
        json.contains("\"targetRefId\":\"sched_target_wire\""),
        "{json}"
    );
    assert!(json.contains("\"retryPolicy\""), "{json}");
    assert!(json.contains("\"backoffKind\":\"exponential\""), "{json}");
    assert!(json.contains("\"kind\":\"one_time\""), "{json}");
    assert!(json.contains("\"status\":\"scheduled\""), "{json}");
    assert!(json.contains("\"dispatchStatus\":\"dispatched\""), "{json}");
    assert!(json.contains("\"downstreamStatus\":\"running\""), "{json}");
    assert!(json.contains("\"fireAt\":"), "{json}");
    assert!(json.contains("\"nextDueAt\":"), "{json}");
    assert!(json.contains("\"runId\":\"run_wire\""), "{json}");
    let decoded: Schedule = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, schedule);

    // Empty optionals are omitted (Go omitempty parity).
    let minimal = Schedule {
        schedule_id: "sched_min".to_string(),
        kind: ScheduleKind::OneTime,
        status: ScheduleStatus::Scheduled,
        target_ref_id: "t".to_string(),
        trigger: Trigger {
            kind: TriggerKind::Once,
            ..Default::default()
        },
        target: Target {
            kind: TargetKind::Run,
            updated_at: now,
            ..Default::default()
        },
        retry_policy: RetryPolicy::default(),
        created_at: now,
        updated_at: now,
        ..Default::default()
    };
    let json = serde_json::to_string(&minimal).unwrap();
    assert!(!json.contains("lastOutcome"), "{json}");
    assert!(!json.contains("\"attempts\""), "{json}");
    assert!(!json.contains("pausedAt"), "{json}");
    assert!(!json.contains("fireAt"), "{json}");
    assert!(!json.contains("tenantId"), "{json}");
    assert!(!json.contains("runId"), "{json}");
}

// ---------------------------------------------------------------------------
// One-time dispatch exactly once (Go TestSchedulerDispatchesOneTimeScheduleExactlyOnce)
// ---------------------------------------------------------------------------

#[test]
fn one_time_schedule_dispatches_exactly_once() {
    let now = parse("2026-04-22T10:00:00Z");
    let h = harness(now);
    let fire_at = now + chrono::Duration::seconds(30);

    let schedule = h
        .scheduler
        .create(one_time_run_input(fire_at, "dispatch exactly once", 1))
        .unwrap();
    assert_eq!(schedule.kind, ScheduleKind::OneTime);
    assert_eq!(schedule.status, ScheduleStatus::Scheduled);
    assert_eq!(schedule.next_due_at, Some(fire_at));
    assert_eq!(schedule.trigger.next_due_at, Some(fire_at));
    assert_eq!(schedule.target.revision, 1);
    assert!(schedule.target.active);
    assert_eq!(schedule.target.summary, "dispatch exactly once");
    assert_eq!(schedule.environment_scope, "test");

    h.scheduler.tick().unwrap();
    assert_eq!(h.runtime.list_runs().len(), 0, "no runs before due time");

    h.clock.set(fire_at + chrono::Duration::seconds(1));
    h.scheduler.tick().unwrap();
    h.scheduler.tick().unwrap();

    let runs = h.runtime.list_runs();
    assert_eq!(runs.len(), 1, "expected one dispatched run");
    assert_eq!(runs[0].schedule_id, schedule.schedule_id);
    assert!(
        !runs[0].schedule_attempt_id.is_empty(),
        "expected schedule linkage"
    );

    let got = h.scheduler.get(&schedule.schedule_id).unwrap().unwrap();
    assert_eq!(got.status, ScheduleStatus::Completed);
    assert!(got.completed_at.is_some());
    assert!(got.next_due_at.is_none());
    assert_eq!(got.attempts.len(), 1);
    assert_eq!(got.attempts[0].dispatch_status, DispatchStatus::Dispatched);
    assert_eq!(got.attempts[0].run_id, runs[0].run_id);
    assert_eq!(
        got.attempts[0].trigger_source,
        kura_scheduler::TriggerSource::Normal
    );
}

// ---------------------------------------------------------------------------
// Cancel pre-dispatch records visible history (Go TestSchedulerCancelPreDispatch...)
// ---------------------------------------------------------------------------

#[test]
fn cancel_pre_dispatch_records_skipped_history() {
    let now = parse("2026-04-22T11:00:00Z");
    let h = harness(now);
    let fire_at = now + chrono::Duration::seconds(30);

    let schedule = h
        .scheduler
        .create(one_time_run_input(fire_at, "cancel before dispatch", 0))
        .unwrap();
    let cancelled = h.scheduler.cancel(&schedule.schedule_id).unwrap().unwrap();
    assert_eq!(cancelled.status, ScheduleStatus::Cancelled);
    assert!(cancelled.cancelled_at.is_some());

    h.clock.set(fire_at + chrono::Duration::seconds(1));
    h.scheduler.tick().unwrap();
    assert_eq!(h.runtime.list_runs().len(), 0, "no runs after cancel");

    let got = h.scheduler.get(&schedule.schedule_id).unwrap().unwrap();
    assert_eq!(got.status, ScheduleStatus::Cancelled);
    assert!(
        !got.attempts.is_empty(),
        "expected visible cancel/skip history"
    );
    assert_eq!(
        got.attempts[0].dispatch_status,
        DispatchStatus::SkippedCancelled
    );
    assert_eq!(got.attempts[0].skipped_reason, "schedule_cancelled");
}

// ---------------------------------------------------------------------------
// Recurring pause/resume + overlap truth (Go TestSchedulerRecurringPauseResumeAndOverlapTruth)
// ---------------------------------------------------------------------------

#[test]
fn recurring_pause_resume_and_overlap_truth() {
    let now = parse("2026-04-22T12:00:00Z");
    let h = harness(now);

    let schedule = h
        .scheduler
        .create(CreateInput {
            trigger: Trigger {
                kind: TriggerKind::Cron,
                cron_expr: "*/1 * * * *".to_string(),
                timezone: "UTC".to_string(),
                ..Default::default()
            },
            target: Target {
                kind: TargetKind::Run,
                run: Some(RunTarget {
                    entrypoint: "operator".to_string(),
                    goal: "recurring dispatch".to_string(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            retry_policy: RetryPolicy {
                max_retries: 0,
                backoff_kind: RetryBackoffKind::Fixed,
                base_delay_seconds: 5,
                max_delay_seconds: 5,
            },
        })
        .unwrap();
    assert_eq!(schedule.kind, ScheduleKind::Recurring);
    assert_eq!(schedule.status, ScheduleStatus::Active);

    h.clock.set(parse("2026-04-22T12:01:01Z"));
    h.scheduler.tick().unwrap();
    let runs = h.runtime.list_runs();
    assert_eq!(runs.len(), 1, "expected first recurring run");

    h.clock.set(parse("2026-04-22T12:02:01Z"));
    h.scheduler.tick().unwrap();
    assert_eq!(
        h.runtime.list_runs().len(),
        1,
        "expected overlap to skip without new run"
    );

    let mut got = h.scheduler.get(&schedule.schedule_id).unwrap().unwrap();
    assert!(
        got.attempts.len() >= 2,
        "expected visible skipped_overlap history"
    );
    assert_eq!(
        got.attempts[0].dispatch_status,
        DispatchStatus::SkippedOverlap
    );
    assert_eq!(
        got.attempts[0].skipped_reason,
        "schedule_execution_in_progress"
    );

    complete_run(&h.runtime, &runs[0].run_id);

    h.scheduler.pause(&schedule.schedule_id).unwrap().unwrap();
    h.clock.set(parse("2026-04-22T12:03:01Z"));
    h.scheduler.tick().unwrap();
    got = h.scheduler.get(&schedule.schedule_id).unwrap().unwrap();
    assert_eq!(
        got.attempts[0].dispatch_status,
        DispatchStatus::SkippedPaused
    );
    assert_eq!(got.attempts[0].skipped_reason, "schedule_paused");

    h.clock.set(parse("2026-04-22T12:03:10Z"));
    let resumed = h.scheduler.resume(&schedule.schedule_id).unwrap().unwrap();
    assert_eq!(resumed.status, ScheduleStatus::Active);
    assert_eq!(resumed.next_due_at, Some(parse("2026-04-22T12:04:00Z")));

    h.clock.set(parse("2026-04-22T12:04:01Z"));
    h.scheduler.tick().unwrap();
    assert_eq!(
        h.runtime.list_runs().len(),
        2,
        "expected second recurring run after resume"
    );
}

// ---------------------------------------------------------------------------
// Retry + exhausted truth (Go TestSchedulerRetryAndExhaustedTruthForDispatchFailure)
// ---------------------------------------------------------------------------

#[test]
fn retry_and_exhausted_truth_for_dispatch_failure() {
    let now = parse("2026-04-22T13:00:00Z");
    let h = harness(now);
    let fire_at = now + chrono::Duration::seconds(30);

    let schedule = h
        .scheduler
        .create(one_time_run_input(fire_at, "retry dispatch failure", 1))
        .unwrap();

    // Deactivate the target record so dispatch fails deterministically.
    let mut target_record = h
        .store
        .lock()
        .get_schedule_target(&schedule.schedule_id, &schedule.target_ref_id)
        .unwrap()
        .unwrap();
    target_record.active = false;
    h.store
        .lock()
        .upsert_schedule_target(&target_record)
        .unwrap();

    h.clock.set(fire_at + chrono::Duration::seconds(1));
    h.scheduler.tick().unwrap();

    let mut got = h.scheduler.get(&schedule.schedule_id).unwrap().unwrap();
    assert_eq!(got.attempts.len(), 1);
    assert_eq!(got.attempts[0].dispatch_status, DispatchStatus::Failed);
    assert!(
        got.attempts[0].next_retry_at.is_some(),
        "expected retryable failure"
    );
    assert_eq!(got.attempts[0].retry_count, 1);
    assert_eq!(got.attempts[0].failure_class, "invalid_target");
    assert_eq!(
        h.runtime.list_runs().len(),
        0,
        "no downstream run on dispatch failure"
    );

    let next_retry_at = got.attempts[0].next_retry_at.unwrap();
    h.clock.set(next_retry_at + chrono::Duration::seconds(1));
    h.scheduler.tick().unwrap();

    got = h.scheduler.get(&schedule.schedule_id).unwrap().unwrap();
    assert_eq!(got.attempts[0].dispatch_status, DispatchStatus::Exhausted);
    assert_eq!(got.attempts[0].retry_count, 1);
    assert_eq!(got.status, ScheduleStatus::DispatchFailed);
    assert!(got.completed_at.is_some());
    assert!(got.next_due_at.is_none());
    assert_eq!(got.last_outcome, "exhausted");
}

// ---------------------------------------------------------------------------
// Workflow targets
// ---------------------------------------------------------------------------

struct StubLauncher {
    result: WorkflowLaunchResult,
}

impl WorkflowLauncher for StubLauncher {
    fn launch_scheduled_workflow(
        &self,
        _target: &WorkflowTarget,
        _schedule_id: &str,
        _schedule_attempt_id: &str,
    ) -> Result<WorkflowLaunchResult, String> {
        Ok(self.result.clone())
    }
}

#[test]
fn workflow_target_dispatches_through_launcher() {
    let now = parse("2026-04-22T09:00:00Z");
    let launcher = Arc::new(StubLauncher {
        result: WorkflowLaunchResult {
            run_id: "run_wf_dispatch".to_string(),
            workflow_id: "wf_scheduled".to_string(),
            downstream_status: DownstreamStatus::Running,
        },
    });
    let h = harness_with_launcher(now, Some(launcher));
    let fire_at = now + chrono::Duration::seconds(30);

    let schedule = h
        .scheduler
        .create(CreateInput {
            trigger: Trigger {
                kind: TriggerKind::Once,
                fire_at: Some(fire_at),
                ..Default::default()
            },
            target: Target {
                kind: TargetKind::Workflow,
                workflow: Some(WorkflowTarget {
                    entrypoint: "operator".to_string(),
                    workflow_goal: "run scheduled workflow".to_string(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            retry_policy: RetryPolicy::default(),
        })
        .unwrap();
    assert_eq!(schedule.target.summary, "run scheduled workflow");

    h.clock.set(fire_at + chrono::Duration::seconds(1));
    h.scheduler.tick().unwrap();

    let got = h.scheduler.get(&schedule.schedule_id).unwrap().unwrap();
    assert_eq!(got.status, ScheduleStatus::Completed);
    assert_eq!(got.attempts[0].dispatch_status, DispatchStatus::Dispatched);
    assert_eq!(got.attempts[0].run_id, "run_wf_dispatch");
    assert_eq!(got.attempts[0].workflow_id, "wf_scheduled");
    assert_eq!(got.attempts[0].downstream_status, DownstreamStatus::Running);
    assert_eq!(got.attempts[0].resolved_target_revision, 1);
}

#[test]
fn workflow_target_without_launcher_is_exhausted() {
    let now = parse("2026-04-22T08:00:00Z");
    let h = harness(now);
    let fire_at = now + chrono::Duration::seconds(30);

    let schedule = h
        .scheduler
        .create(CreateInput {
            trigger: Trigger {
                kind: TriggerKind::Once,
                fire_at: Some(fire_at),
                ..Default::default()
            },
            target: Target {
                kind: TargetKind::Workflow,
                workflow: Some(WorkflowTarget {
                    entrypoint: "operator".to_string(),
                    workflow_goal: "no launcher".to_string(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            retry_policy: RetryPolicy::default(),
        })
        .unwrap();

    h.clock.set(fire_at + chrono::Duration::seconds(1));
    h.scheduler.tick().unwrap();

    let got = h.scheduler.get(&schedule.schedule_id).unwrap().unwrap();
    assert_eq!(got.attempts[0].dispatch_status, DispatchStatus::Exhausted);
    assert_eq!(
        got.attempts[0].failure_class,
        "workflow_launcher_unavailable"
    );
    assert_eq!(got.status, ScheduleStatus::DispatchFailed);
}

// ---------------------------------------------------------------------------
// Cron due computation (Go NextDueAfter / trigger.go)
// ---------------------------------------------------------------------------

#[test]
fn cron_triggers_compute_next_due() {
    let t = parse;

    // Every minute at :00 — next minute boundary.
    let trigger = Trigger {
        kind: TriggerKind::Cron,
        cron_expr: "*/1 * * * *".to_string(),
        timezone: "UTC".to_string(),
        ..Default::default()
    };
    assert_eq!(
        next_due_after(&trigger, t("2026-04-22T12:00:00Z")).unwrap(),
        Some(t("2026-04-22T12:01:00Z"))
    );
    // Sub-minute start still resolves to the next boundary.
    assert_eq!(
        next_due_after(&trigger, t("2026-04-22T12:00:30Z")).unwrap(),
        Some(t("2026-04-22T12:01:00Z"))
    );

    // Specific time-of-day.
    let trigger = Trigger {
        kind: TriggerKind::Cron,
        cron_expr: "0 14 * * *".to_string(),
        timezone: "UTC".to_string(),
        ..Default::default()
    };
    assert_eq!(
        next_due_after(&trigger, t("2026-04-22T12:30:00Z")).unwrap(),
        Some(t("2026-04-22T14:00:00Z"))
    );

    // Weekday filter: 2026-04-22 is a Wednesday; "0 9 * * 1" is Mondays -> 2026-04-27.
    let trigger = Trigger {
        kind: TriggerKind::Cron,
        cron_expr: "0 9 * * 1".to_string(),
        timezone: "UTC".to_string(),
        ..Default::default()
    };
    assert_eq!(
        next_due_after(&trigger, t("2026-04-22T12:30:00Z")).unwrap(),
        Some(t("2026-04-27T09:00:00Z"))
    );

    // Range + step syntax.
    let trigger = Trigger {
        kind: TriggerKind::Cron,
        cron_expr: "*/15 9-17 * * 1-5".to_string(),
        timezone: "UTC".to_string(),
        ..Default::default()
    };
    assert_eq!(
        next_due_after(&trigger, t("2026-04-22T12:30:00Z")).unwrap(),
        Some(t("2026-04-22T12:45:00Z"))
    );

    // Once trigger: future fireAt -> Some, past/equal -> None.
    let trigger = Trigger {
        kind: TriggerKind::Once,
        fire_at: Some(t("2026-04-22T12:00:00Z")),
        ..Default::default()
    };
    assert_eq!(
        next_due_after(&trigger, t("2026-04-22T11:00:00Z")).unwrap(),
        Some(t("2026-04-22T12:00:00Z"))
    );
    assert_eq!(
        next_due_after(&trigger, t("2026-04-22T13:00:00Z")).unwrap(),
        None
    );
}

#[test]
fn create_validates_trigger() {
    let now = parse("2026-04-22T07:00:00Z");
    let h = harness(now);

    let err = h
        .scheduler
        .create(CreateInput {
            trigger: Trigger {
                kind: TriggerKind::Once,
                ..Default::default()
            },
            target: Target {
                kind: TargetKind::Run,
                updated_at: now,
                ..Default::default()
            },
            retry_policy: RetryPolicy::default(),
        })
        .unwrap_err();
    assert!(
        err.to_string().contains("one-time trigger requires fireAt"),
        "{err}"
    );

    let cron_input = |expr: &str, timezone: &str| CreateInput {
        trigger: Trigger {
            kind: TriggerKind::Cron,
            cron_expr: expr.to_string(),
            timezone: timezone.to_string(),
            ..Default::default()
        },
        target: Target {
            kind: TargetKind::Run,
            updated_at: now,
            ..Default::default()
        },
        retry_policy: RetryPolicy::default(),
    };

    let err = h.scheduler.create(cron_input("", "UTC")).unwrap_err();
    assert!(
        err.to_string().contains("cron expression is required"),
        "{err}"
    );

    let err = h
        .scheduler
        .create(cron_input("*/1 * * *", "UTC"))
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("cron expression must have 5 fields"),
        "{err}"
    );

    let err = h
        .scheduler
        .create(cron_input("*/1 * * * *", "Mars/Olympus_Mons"))
        .unwrap_err();
    assert!(err.to_string().contains("load timezone"), "{err}");

    let err = h
        .scheduler
        .create(cron_input("*/0 * * * *", "UTC"))
        .unwrap_err();
    assert!(err.to_string().contains("invalid step"), "{err}");

    let err = h
        .scheduler
        .create(cron_input("* * * 13 *", "UTC"))
        .unwrap_err();
    assert!(err.to_string().contains("value 13 out of bounds"), "{err}");

    let err = h
        .scheduler
        .create(cron_input("10-20 0 32 * *", "UTC"))
        .unwrap_err();
    assert!(err.to_string().contains("value 32 out of bounds"), "{err}");

    // Valid cron create.
    let schedule = h
        .scheduler
        .create(cron_input("*/5 * * * *", "UTC"))
        .unwrap();
    assert_eq!(schedule.kind, ScheduleKind::Recurring);
    assert_eq!(schedule.status, ScheduleStatus::Active);
    assert_eq!(schedule.next_due_at, Some(parse("2026-04-22T07:00:00Z")));
}

// ---------------------------------------------------------------------------
// Persistence across scheduler instances
// ---------------------------------------------------------------------------

#[test]
fn schedule_persists_across_scheduler_instances() {
    let now = parse("2026-04-22T14:00:00Z");
    let dir = temp_dir();
    let store = Arc::new(parking_lot::Mutex::new(SQLiteStore::new(&dir).unwrap()));
    let runtime = Arc::new(kura_runtime::Manager::new());
    let clock = FakeClock::new(now);
    let sched_a = Scheduler::new(Dependencies {
        environment: kura_config::Environment::Test,
        runtime: Arc::clone(&runtime),
        event_bus: None,
        store: Arc::clone(&store),
        workflow_launcher: None,
        clock: Some(Box::new(clock.clone())),
        tick_interval: Duration::from_millis(10),
    });
    let fire_at = now + chrono::Duration::seconds(30);
    let schedule = sched_a
        .create(one_time_run_input(fire_at, "persist me", 0))
        .unwrap();

    // A second scheduler over the same SQLite database sees the same schedule.
    let clock_b = FakeClock::new(now + chrono::Duration::seconds(60));
    let clock_b_handle = clock_b.clone();
    let sched_b = Scheduler::new(Dependencies {
        environment: kura_config::Environment::Test,
        runtime: Arc::clone(&runtime),
        event_bus: None,
        store: Arc::clone(&store),
        workflow_launcher: None,
        clock: Some(Box::new(clock_b)),
        tick_interval: Duration::from_millis(10),
    });
    let got = sched_b.get(&schedule.schedule_id).unwrap().unwrap();
    assert_eq!(got.schedule_id, schedule.schedule_id);
    assert_eq!(got.kind, schedule.kind);
    assert_eq!(got.status, schedule.status);
    assert_eq!(got.trigger.fire_at, schedule.trigger.fire_at);
    assert_eq!(got.target.summary, schedule.target.summary);
    assert_eq!(got.retry_policy, schedule.retry_policy);
    assert_eq!(got.next_due_at, schedule.next_due_at);

    // Dispatch via B and read the attempt history back through A.
    clock_b_handle.set(fire_at + chrono::Duration::seconds(1));
    sched_b.tick().unwrap();
    let got_b = sched_b.get(&schedule.schedule_id).unwrap().unwrap();
    assert_eq!(got_b.attempts.len(), 1);
    assert_eq!(
        got_b.attempts[0].dispatch_status,
        DispatchStatus::Dispatched
    );
    let got_a = sched_a.get(&schedule.schedule_id).unwrap().unwrap();
    assert_eq!(got_a.attempts.len(), 1);
    assert_eq!(got_a.attempts[0].attempt_id, got_b.attempts[0].attempt_id);
    assert_eq!(got_a.attempts[0].run_id, got_b.attempts[0].run_id);
}

// ---------------------------------------------------------------------------
// Catch-up missed intervals
// ---------------------------------------------------------------------------

#[test]
fn catch_up_records_missed_intervals() {
    let now = parse("2026-04-22T12:00:00Z");
    let h = harness(now);

    let schedule = h
        .scheduler
        .create(CreateInput {
            trigger: Trigger {
                kind: TriggerKind::Cron,
                cron_expr: "*/1 * * * *".to_string(),
                timezone: "UTC".to_string(),
                ..Default::default()
            },
            target: Target {
                kind: TargetKind::Run,
                run: Some(RunTarget {
                    entrypoint: "operator".to_string(),
                    goal: "catch up".to_string(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            retry_policy: RetryPolicy::default(),
        })
        .unwrap();
    assert_eq!(
        schedule.next_due_at,
        Some(now),
        "created due at the current minute"
    );

    h.clock.set(parse("2026-04-22T12:03:30Z"));
    h.scheduler.catch_up().unwrap();

    let got = h.scheduler.get(&schedule.schedule_id).unwrap().unwrap();
    assert!(got.attempts.len() >= 2);
    // attempts[0] is the catch-up dispatch for the current due; attempts[1] the missed record.
    assert_eq!(got.attempts[0].dispatch_status, DispatchStatus::Dispatched);
    assert_eq!(
        got.attempts[0].trigger_source,
        kura_scheduler::TriggerSource::CatchUp
    );
    assert_eq!(got.attempts[1].dispatch_status, DispatchStatus::Missed);
    assert_eq!(got.attempts[1].missed_count, 3);
    assert_eq!(got.attempts[1].due_at, parse("2026-04-22T12:00:00Z"));
    assert_eq!(h.runtime.list_runs().len(), 1);
}

// ---------------------------------------------------------------------------
// Events on the bus
// ---------------------------------------------------------------------------

#[test]
fn events_published_to_bus() {
    let now = parse("2026-04-22T15:00:00Z");
    let h = harness(now);
    let fire_at = now + chrono::Duration::seconds(30);

    let schedule = h
        .scheduler
        .create(one_time_run_input(fire_at, "event fan-out", 0))
        .unwrap();
    let filter = Filter {
        category: "schedule".to_string(),
        ..Default::default()
    };
    let names: Vec<String> = h.bus.list(&filter).iter().map(|e| e.name.clone()).collect();
    assert!(names.contains(&"schedule.created".to_string()), "{names:?}");
    let created = h
        .bus
        .list(&filter)
        .into_iter()
        .find(|e| e.name == "schedule.created")
        .unwrap();
    assert_eq!(created.scope.schedule_id, schedule.schedule_id);
    assert_eq!(created.resource.kind, "schedule");
    assert_eq!(
        created.payload.get("status").unwrap().as_str().unwrap(),
        "scheduled"
    );

    h.clock.set(fire_at + chrono::Duration::seconds(1));
    h.scheduler.tick().unwrap();
    let names: Vec<String> = h.bus.list(&filter).iter().map(|e| e.name.clone()).collect();
    assert!(
        names.contains(&"schedule.dispatch_attempted".to_string()),
        "{names:?}"
    );
    assert!(
        names.contains(&"schedule.dispatch_recorded".to_string()),
        "{names:?}"
    );
    let recorded = h
        .bus
        .list(&filter)
        .into_iter()
        .find(|e| e.name == "schedule.dispatch_recorded")
        .unwrap();
    assert_eq!(
        recorded
            .payload
            .get("dispatchStatus")
            .unwrap()
            .as_str()
            .unwrap(),
        "dispatched"
    );
    assert!(recorded.payload.contains_key("runId"));
    assert!(recorded.payload.contains_key("scheduleAttemptId"));
}

// ---------------------------------------------------------------------------
// Misc lifecycle edges
// ---------------------------------------------------------------------------

#[test]
fn pause_resume_cancel_edges() {
    let now = parse("2026-04-22T16:00:00Z");
    let h = harness(now);
    let fire_at = now + chrono::Duration::seconds(30);

    let schedule = h
        .scheduler
        .create(one_time_run_input(fire_at, "edges", 0))
        .unwrap();

    // Resume on a non-paused schedule is a no-op returning the schedule.
    let resumed = h.scheduler.resume(&schedule.schedule_id).unwrap().unwrap();
    assert_eq!(resumed.status, ScheduleStatus::Scheduled);

    // Pause, then resume a one-time schedule whose fireAt is still in the future.
    h.scheduler.pause(&schedule.schedule_id).unwrap().unwrap();
    let resumed = h.scheduler.resume(&schedule.schedule_id).unwrap().unwrap();
    assert_eq!(resumed.status, ScheduleStatus::Scheduled);
    assert_eq!(resumed.next_due_at, Some(fire_at));

    // Cancel twice: second cancel returns the schedule unchanged.
    let first = h.scheduler.cancel(&schedule.schedule_id).unwrap().unwrap();
    let second = h.scheduler.cancel(&schedule.schedule_id).unwrap().unwrap();
    assert_eq!(first.status, ScheduleStatus::Cancelled);
    assert_eq!(second.status, ScheduleStatus::Cancelled);
    assert_eq!(second.cancelled_at, first.cancelled_at);

    // Unknown ids return None.
    assert!(h.scheduler.get("sched_missing").unwrap().is_none());
    assert!(h.scheduler.pause("sched_missing").unwrap().is_none());
    assert!(h.scheduler.resume("sched_missing").unwrap().is_none());
    assert!(h.scheduler.cancel("sched_missing").unwrap().is_none());

    // List sees the single schedule.
    let items = h.scheduler.list().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].schedule_id, schedule.schedule_id);

    // Store accessor + lifecycle flags.
    assert!(
        h.scheduler
            .store()
            .lock()
            .data_dir()
            .contains("kura_scheduler_test")
    );
    h.scheduler.start().unwrap();
    h.scheduler.close().unwrap();
    assert_eq!(h.scheduler.tick_interval(), Duration::from_millis(10));
}

#[test]
fn store_records_written_by_create() {
    let now = parse("2026-04-22T17:00:00Z");
    let h = harness(now);
    let fire_at = now + chrono::Duration::seconds(30);

    let schedule = h
        .scheduler
        .create(one_time_run_input(fire_at, "store records", 0))
        .unwrap();

    let record = h
        .store
        .lock()
        .get_schedule("test", &schedule.schedule_id)
        .unwrap()
        .unwrap();
    assert_eq!(record.kind, "one_time");
    assert_eq!(record.status, "scheduled");
    assert_eq!(record.target_ref_id, schedule.target_ref_id);
    assert_eq!(record.timezone, "");
    assert!(record.document.contains("\"scheduleId\":\""));
    assert!(record.document.contains("\"retryPolicy\""));

    let target = h
        .store
        .lock()
        .get_schedule_target(&schedule.schedule_id, &schedule.target_ref_id)
        .unwrap()
        .unwrap();
    assert_eq!(target.target_kind, "run");
    assert!(target.active);
    assert_eq!(target.revision, 1);
    assert!(target.document.contains("\"entrypoint\":\"operator\""));
}
/// Compile-time guard: this manager must be usable from axum `AppState` (Send + Sync).
#[test]
fn manager_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<kura_scheduler::Scheduler>();
}
