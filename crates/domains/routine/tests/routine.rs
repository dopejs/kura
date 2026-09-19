//! Manager-behavior + round-trip tests for `kura-routine`, mirroring the Go
//! `manager_test.go` / `persistence_test.go` coverage with a fake `Scheduler`.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use chrono::{Duration, Utc};
use kura_routine::{
    CreateInput, Definition, Manager, Routine, RoutineError, Schedule, Scheduler,
    SchedulerRetryBackoffKind, SchedulerTargetKind, SchedulerTriggerKind, State, Trigger,
    TriggerKind, Workflow,
};
use kura_store::SQLiteStore;
use uuid::Uuid;

/// Records compiled schedules and lifecycle calls (Go `fakeScheduler`).
#[derive(Clone)]
struct FakeScheduler {
    seq: Arc<Mutex<usize>>,
    created: Arc<Mutex<Vec<CreateInput>>>,
    paused: Arc<Mutex<Vec<String>>>,
    resumed: Arc<Mutex<Vec<String>>>,
    cancelled: Arc<Mutex<Vec<String>>>,
    missing: Arc<Mutex<HashSet<String>>>,
}

impl Default for FakeScheduler {
    fn default() -> Self {
        FakeScheduler {
            seq: Arc::new(Mutex::new(0)),
            created: Arc::new(Mutex::new(Vec::new())),
            paused: Arc::new(Mutex::new(Vec::new())),
            resumed: Arc::new(Mutex::new(Vec::new())),
            cancelled: Arc::new(Mutex::new(Vec::new())),
            missing: Arc::new(Mutex::new(HashSet::new())),
        }
    }
}

impl Scheduler for FakeScheduler {
    fn create(&self, input: &CreateInput) -> Result<Schedule, String> {
        let mut seq = self.seq.lock().unwrap();
        *seq += 1;
        self.created.lock().unwrap().push(input.clone());
        Ok(Schedule {
            schedule_id: format!("sched_{seq}"),
            ..Schedule::default()
        })
    }

    fn pause(&self, schedule_id: &str) -> Result<(Schedule, bool), String> {
        self.paused.lock().unwrap().push(schedule_id.to_string());
        Ok((
            Schedule {
                schedule_id: schedule_id.to_string(),
                ..Schedule::default()
            },
            true,
        ))
    }

    fn resume(&self, schedule_id: &str) -> Result<(Schedule, bool), String> {
        self.resumed.lock().unwrap().push(schedule_id.to_string());
        Ok((
            Schedule {
                schedule_id: schedule_id.to_string(),
                ..Schedule::default()
            },
            true,
        ))
    }

    fn cancel(&self, schedule_id: &str) -> Result<(Schedule, bool), String> {
        self.cancelled.lock().unwrap().push(schedule_id.to_string());
        Ok((
            Schedule {
                schedule_id: schedule_id.to_string(),
                ..Schedule::default()
            },
            true,
        ))
    }

    fn get(&self, schedule_id: &str) -> Result<(Schedule, bool), String> {
        if self.missing.lock().unwrap().contains(schedule_id) {
            return Ok((Schedule::default(), false));
        }
        Ok((
            Schedule {
                schedule_id: schedule_id.to_string(),
                ..Schedule::default()
            },
            true,
        ))
    }
}

fn fake() -> Box<FakeScheduler> {
    Box::new(FakeScheduler::default())
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "kura_routine_{tag}_{}_{}",
        std::process::id(),
        Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Mirrors the Go `dailyDef` helper.
fn daily_def() -> Definition {
    Definition {
        name: "Daily summary".to_string(),
        trigger: Trigger {
            kind: TriggerKind::Cron,
            cron_expr: "0 8 * * *".to_string(),
            timezone: "UTC".to_string(),
            fire_at: None,
        },
        workflow: Workflow {
            entrypoint: String::new(),
            goal: "summarize my day".to_string(),
        },
        approval_expectation: "ask".to_string(),
        delivery_preference_id: String::new(),
        max_retries: 0,
    }
}

#[test]
fn create_compiles_to_workflow_schedule() {
    let f = fake();
    let m = Manager::new("test", f.clone());
    let r = m.create(daily_def()).unwrap();
    assert_eq!(r.state, State::Active);
    assert_eq!(r.current_version, 1);
    assert!(!r.current_schedule_id.is_empty());
    assert!(r.routine_id.starts_with("routine_"));
    let created = f.created.lock().unwrap();
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].target.kind, SchedulerTargetKind::Workflow);
    assert_eq!(
        created[0].target.workflow.as_ref().unwrap().workflow_goal,
        "summarize my day"
    );
    assert_eq!(
        created[0].target.workflow.as_ref().unwrap().entrypoint,
        "operator"
    );
    assert_eq!(created[0].target.summary, "Daily summary");
    assert!(created[0].target.active);
    assert_eq!(created[0].trigger.kind, SchedulerTriggerKind::Cron);
    assert_eq!(created[0].trigger.cron_expr, "0 8 * * *");
    assert_eq!(created[0].trigger.timezone, "UTC");
    assert_eq!(created[0].retry_policy.max_retries, 1); // defaulted
    assert_eq!(
        created[0].retry_policy.backoff_kind,
        SchedulerRetryBackoffKind::Fixed
    );
    assert_eq!(created[0].retry_policy.base_delay_seconds, 5);
    assert_eq!(created[0].retry_policy.max_delay_seconds, 5);
    drop(created);
    // Version 1 snapshots the definition + schedule id.
    assert_eq!(r.versions.len(), 1);
    assert_eq!(r.versions[0].version, 1);
    assert_eq!(
        r.versions[0].schedule_id.as_str(),
        r.current_schedule_id.as_str()
    );
    assert_eq!(r.versions[0].definition.name, "Daily summary");
}

#[test]
fn update_preserves_prior_evidence() {
    let f = fake();
    let m = Manager::new("test", f.clone());
    let r = m.create(daily_def()).unwrap();
    let prior_schedule_id = r.current_schedule_id.clone();

    let mut def2 = daily_def();
    def2.workflow.goal = "summarize my day and inbox".to_string();
    let updated = m.update(&r.routine_id, def2).unwrap();
    assert_eq!(updated.current_version, 2);
    assert_ne!(updated.current_schedule_id, prior_schedule_id);
    assert_eq!(
        updated.definition.workflow.goal,
        "summarize my day and inbox"
    );
    let cancelled = f.cancelled.lock().unwrap();
    assert_eq!(cancelled.len(), 1);
    assert_eq!(cancelled[0].as_str(), prior_schedule_id.as_str());
    drop(cancelled);
    // The prior version keeps its schedule id (its execution evidence).
    assert_eq!(
        updated.versions[0].schedule_id.as_str(),
        prior_schedule_id.as_str()
    );
    assert_eq!(updated.versions[1].version, 2);
}

#[test]
fn lifecycle_pause_resume_cancel() {
    let f = fake();
    let m = Manager::new("test", f.clone());
    let r = m.create(daily_def()).unwrap();

    let paused = m.pause(&r.routine_id).unwrap();
    assert_eq!(paused.state, State::Paused);
    let paused_calls = f.paused.lock().unwrap();
    assert_eq!(paused_calls.len(), 1);
    assert_eq!(paused_calls[0].as_str(), r.current_schedule_id.as_str());
    drop(paused_calls);

    let resumed = m.resume(&r.routine_id).unwrap();
    assert_eq!(resumed.state, State::Active);
    assert_eq!(f.resumed.lock().unwrap().len(), 1);

    let cancelled = m.cancel(&r.routine_id).unwrap();
    assert_eq!(cancelled.state, State::Cancelled);
    assert_eq!(f.cancelled.lock().unwrap().len(), 1);

    // A cancelled routine rejects further transitions.
    let err = m.pause(&r.routine_id).unwrap_err();
    assert_eq!(err, RoutineError::RoutineCancelled);
    let err = m.update(&r.routine_id, daily_def()).unwrap_err();
    assert_eq!(err, RoutineError::RoutineCancelled);
}

#[test]
fn repair_recreates_missing_schedule() {
    let f = fake();
    let m = Manager::new("test", f.clone());
    let r = m.create(daily_def()).unwrap();
    f.missing
        .lock()
        .unwrap()
        .insert(r.current_schedule_id.clone());

    let repaired = m.repair(&r.routine_id).unwrap();
    assert_ne!(repaired.current_schedule_id, r.current_schedule_id);
    assert_eq!(repaired.current_version, 1, "repair must not bump version");
    // The current version reflects the repaired schedule id.
    assert_eq!(
        repaired.versions[0].schedule_id.as_str(),
        repaired.current_schedule_id.as_str()
    );
}

#[test]
fn repair_is_noop_when_healthy() {
    let f = fake();
    let m = Manager::new("test", f.clone());
    let r = m.create(daily_def()).unwrap();
    let repaired = m.repair(&r.routine_id).unwrap();
    assert_eq!(repaired.current_schedule_id, r.current_schedule_id);
    assert_eq!(
        f.created.lock().unwrap().len(),
        1,
        "healthy repair must not recreate"
    );
}

#[test]
fn preview_and_validation() {
    let f = fake();
    let m = Manager::new("test", f.clone());
    let preview = m.preview(&daily_def()).unwrap();
    assert_eq!(preview.schedule_kind, "recurring");
    assert_eq!(preview.workflow_summary, "summarize my day");
    assert_eq!(preview.approval_expectation, "ask");
    assert_eq!(preview.retry_summary, "max 1 retries");
    assert!(preview.trigger_summary.contains("cron 0 8 * * *"));
    // Preview must not activate (create) a schedule.
    assert!(f.created.lock().unwrap().is_empty());

    // Invalid definitions are rejected with the Go-equivalent messages.
    let err = m
        .create(Definition {
            name: "bad".to_string(),
            ..Definition::default()
        })
        .unwrap_err();
    assert_eq!(err, RoutineError::InvalidGoalRequired);
    let err = m
        .create(Definition {
            name: "x".to_string(),
            trigger: Trigger {
                kind: TriggerKind::Cron,
                ..Trigger::default()
            },
            workflow: Workflow {
                goal: "g".to_string(),
                ..Workflow::default()
            },
            ..Definition::default()
        })
        .unwrap_err();
    assert_eq!(err, RoutineError::InvalidCronExprRequired);
    let err = m
        .create(Definition {
            name: "x".to_string(),
            trigger: Trigger {
                kind: TriggerKind::Once,
                ..Trigger::default()
            },
            workflow: Workflow {
                goal: "g".to_string(),
                ..Workflow::default()
            },
            ..Definition::default()
        })
        .unwrap_err();
    assert_eq!(err, RoutineError::InvalidFireAtRequired);
    // Unknown routine lookups.
    assert!(m.get("missing").is_none());
    assert_eq!(
        m.update("missing", daily_def()).unwrap_err(),
        RoutineError::RoutineNotFound
    );
}

#[test]
fn once_definition_compiles_to_once_trigger() {
    let f = fake();
    let m = Manager::new("test", f.clone());
    let fire_at = Utc::now() + Duration::hours(24);
    let mut def = daily_def();
    def.trigger = Trigger {
        kind: TriggerKind::Once,
        fire_at: Some(fire_at),
        ..Trigger::default()
    };
    let preview = m.preview(&def).unwrap();
    assert_eq!(preview.schedule_kind, "one_time");
    assert!(
        preview.trigger_summary.starts_with("once at "),
        "{}",
        preview.trigger_summary
    );
    let r = m.create(def.clone()).unwrap();
    let created = f.created.lock().unwrap();
    assert_eq!(created[0].trigger.kind, SchedulerTriggerKind::Once);
    assert_eq!(created[0].trigger.fire_at, def.trigger.fire_at);
    drop(created);
    assert!(r.current_schedule_id.starts_with("sched_"));
}

#[test]
fn get_and_list() {
    let f = fake();
    let m = Manager::new("test", f.clone());
    let a = m.create(daily_def()).unwrap();
    let mut def2 = daily_def();
    def2.name = "Evening digest".to_string();
    let b = m.create(def2).unwrap();
    let listed = m.list();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].routine_id.as_str(), a.routine_id.as_str());
    assert_eq!(listed[1].routine_id, b.routine_id);
    assert_eq!(m.get(&a.routine_id).unwrap(), a);
}

#[test]
fn routine_wire_round_trip() {
    let f = fake();
    let m = Manager::new("test", f.clone());
    let r = m.create(daily_def()).unwrap();
    let json = serde_json::to_string(&r).unwrap();
    for key in [
        "\"routineId\"",
        "\"environmentScope\"",
        "\"currentVersion\"",
        "\"currentScheduleId\"",
        "\"versions\"",
        "\"definition\"",
        "\"approvalExpectation\"",
    ] {
        assert!(json.contains(key), "missing {key} in {json}");
    }
    assert!(json.contains("\"cron\""));
    let decoded: Routine = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, r);
}

#[test]
fn persistence_round_trip() {
    let dir = temp_dir("persist");
    let routine_id;
    {
        let store = Arc::new(parking_lot::Mutex::new(
            SQLiteStore::new(&dir.to_string_lossy()).unwrap(),
        ));
        let mut m = Manager::new("test", fake());
        m.with_store(Arc::clone(&store));
        let r = m.create(daily_def()).unwrap();
        let _ = m.pause(&r.routine_id);
        routine_id = r.routine_id.clone();
    }
    {
        let store = Arc::new(parking_lot::Mutex::new(
            SQLiteStore::new(&dir.to_string_lossy()).unwrap(),
        ));
        let mut m = Manager::new("test", fake());
        m.with_store(Arc::clone(&store));
        m.load_from_store().unwrap();
        let got = m.get(&routine_id).expect("routine survived restart");
        assert_eq!(got.state, State::Paused);
        assert_eq!(got.current_version, 1);
        assert_eq!(got.name, "Daily summary");
    }
}
/// Compile-time guard: this manager must be usable from axum `AppState` (Send + Sync).
#[test]
fn manager_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<kura_routine::Manager>();
}
