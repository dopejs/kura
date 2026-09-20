#![allow(unused_doc_comments)]
//! Port of `daemon/internal/routine` (Roadmap 66): the routine builder. Routines are explicit,
//! user-defined proactive routines that compile to the existing schedule + workflow + delivery
//! planes — no autonomous planning and no memory. Routine edits create new versions and never
//! rewrite prior execution evidence (the underlying schedule attempts).
//!
//! Wire compatibility with the Go package: string enums serialize as their exact snake_case values
//! and structs use camelCase JSON keys with the same `omitempty` behavior (`skip_serializing_if`).
//!
//! The Go `managerdoc.Store` persistence maps onto `kura_store::{put_document, list_documents}`
//! against `kura_store::SQLiteStore`; a nil store is `Option<&SQLiteStore>` and persistence is
//! skipped while `None`. The Go `Scheduler` interface becomes the `Scheduler` trait, and the
//! scheduler-facing types (`Schedule`, `CreateInput`, `Trigger`, `Target`,
//! `WorkflowTarget`, `RetryPolicy`) mirror the subset of `daemon/internal/scheduler` that the
//! routine builder consumes. The Go `context.Context` parameters are dropped (sync port).

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

/// Selects when a routine fires (Go `TriggerKind`).
string_enum!(TriggerKind {
    Cron => "cron",
    Once => "once",
});

/// The routine lifecycle state (Go `State`).
string_enum!(State {
    Active => "active",
    Paused => "paused",
    Cancelled => "cancelled",
});

// ---------------------------------------------------------------------------------------------
// Scheduler-facing types (mirror of `daemon/internal/scheduler` subset used by the routine
// builder; the Go package depends on the concrete scheduler through the `Scheduler` interface).
// ---------------------------------------------------------------------------------------------

/// Go `scheduler.ScheduleKind`.
string_enum!(ScheduleKind {
    OneTime => "one_time",
    Recurring => "recurring",
});

/// Go `scheduler.ScheduleStatus`.
string_enum!(ScheduleStatus {
    Scheduled => "scheduled",
    Active => "active",
    Paused => "paused",
    Cancelled => "cancelled",
    Completed => "completed",
    DispatchFailed => "dispatch_failed",
});

/// Go `scheduler.TriggerKind`.
string_enum!(SchedulerTriggerKind {
    Once => "once",
    Cron => "cron",
});

/// Go `scheduler.TargetKind`.
string_enum!(SchedulerTargetKind {
    Run => "run",
    Workflow => "workflow",
});

/// Go `scheduler.RetryBackoffKind`.
string_enum!(SchedulerRetryBackoffKind {
    Fixed => "fixed",
    Exponential => "exponential",
});

/// The explicit firing schedule for a routine (Go `Trigger`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Trigger {
    pub kind: TriggerKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cron_expr: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub timezone: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fire_at: Option<DateTime<Utc>>,
}

/// The explicit work a routine runs each fire (Go `Workflow`); the entrypoint defaults to
/// `operator` at compile time.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workflow {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub entrypoint: String,
    pub goal: String,
}

/// The explicit routine configuration a user composes/approves (Go `Definition`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Definition {
    pub name: String,
    pub trigger: Trigger,
    pub workflow: Workflow,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub approval_expectation: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub delivery_preference_id: String,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub max_retries: i64,
}

/// A snapshot of a routine definition plus the schedule it compiled to. Prior versions keep their
/// schedule id so their execution evidence remains inspectable after edits (FR-003)
/// (Go `Version`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Version {
    pub version: i64,
    pub definition: Definition,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub schedule_id: String,
    pub created_at: DateTime<Utc>,
}

/// A product-level proactive routine (Go `Routine`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Routine {
    pub routine_id: String,
    pub environment_scope: String,
    pub name: String,
    pub state: State,
    pub current_version: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub current_schedule_id: String,
    pub definition: Definition,
    pub versions: Vec<Version>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The compiled, pre-activation projection of a routine definition: what schedule and workflow it
/// compiles to, and the approval/delivery/quota expectations to confirm (FR-004)
/// (Go `Preview`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub schedule_kind: String,
    pub trigger_summary: String,
    pub workflow_summary: String,
    pub approval_expectation: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub delivery_preference_id: String,
    pub retry_summary: String,
}

/// Go `scheduler.Schedule` (subset the routine builder consumes: the compiled schedule identity
/// plus the compiled trigger/target/retry policy).
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
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub target_ref_id: String,
    pub trigger: SchedulerTrigger,
    pub target: SchedulerTarget,
    pub retry_policy: SchedulerRetryPolicy,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Go `scheduler.Trigger` (subset set by the routine builder).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchedulerTrigger {
    pub kind: SchedulerTriggerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fire_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cron_expr: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub timezone: String,
}

/// Go `scheduler.Target` (subset set by the routine builder).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchedulerTarget {
    pub kind: SchedulerTargetKind,
    #[serde(default, skip_serializing_if = "is_false")]
    pub active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<SchedulerWorkflowTarget>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
}

/// Go `scheduler.WorkflowTarget` (subset set by the routine builder).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchedulerWorkflowTarget {
    pub entrypoint: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workflow_goal: String,
}

/// Go `scheduler.RetryPolicy`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchedulerRetryPolicy {
    pub max_retries: i64,
    pub backoff_kind: SchedulerRetryBackoffKind,
    pub base_delay_seconds: i64,
    pub max_delay_seconds: i64,
}

/// Go `scheduler.CreateInput`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateInput {
    pub trigger: SchedulerTrigger,
    pub target: SchedulerTarget,
    pub retry_policy: SchedulerRetryPolicy,
}

/// Manager validation/lookup failures. Go surfaces `ErrRoutineNotFound`,
/// `ErrRoutineCancelled`, and `ErrInvalidRoutine` (the latter wrapped with a detail message);
/// here the invalid-definition details are typed variants carrying the same messages.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum RoutineError {
    #[error("routine not found")]
    RoutineNotFound,
    #[error("routine is cancelled")]
    RoutineCancelled,
    #[error("routine definition is invalid: name is required")]
    InvalidNameRequired,
    #[error("routine definition is invalid: workflow goal is required")]
    InvalidGoalRequired,
    #[error("routine definition is invalid: cron trigger requires a cron expression")]
    InvalidCronExprRequired,
    #[error("routine definition is invalid: once trigger requires a fire time")]
    InvalidFireAtRequired,
    #[error("routine definition is invalid: unsupported trigger kind")]
    InvalidTriggerKind,
    #[error("compile routine to schedule: {0}")]
    CompileSchedule(String),
    #[error("repair routine schedule: {0}")]
    RepairSchedule(String),
}

/// Document kind used for durable routines (Go `docKindRoutine`).
pub const DOC_KIND_ROUTINE: &str = "routine";

/// The subset of the scheduler the routine builder compiles to (Go `Scheduler` interface). The
/// concrete `*scheduler.Scheduler` satisfies it; tests use a fake.
pub trait Scheduler: Send + Sync {
    fn create(&self, input: &CreateInput) -> Result<Schedule, String>;
    fn pause(&self, schedule_id: &str) -> Result<(Schedule, bool), String>;
    fn resume(&self, schedule_id: &str) -> Result<(Schedule, bool), String>;
    fn cancel(&self, schedule_id: &str) -> Result<(Schedule, bool), String>;
    fn get(&self, schedule_id: &str) -> Result<(Schedule, bool), String>;
}

#[derive(Default)]
struct ManagerInner {
    by_id: HashMap<String, Routine>,
    ids: Vec<String>,
}

/// Owns routines and compiles them to the scheduler/workflow plane (Go `Manager`). Routines are
/// stored in-memory with `restore`/`load_from_store`; the compiled schedules (and their attempt
/// evidence) persist in the scheduler's own store.
pub struct Manager {
    inner: parking_lot::RwLock<ManagerInner>,
    env: String,
    sched: Box<dyn Scheduler>,
    docs: Option<Arc<parking_lot::Mutex<kura_store::SQLiteStore>>>,
}

impl Manager {
    /// Go `NewManager`.
    #[must_use]
    pub fn new(environment_scope: &str, sched: Box<dyn Scheduler>) -> Self {
        Manager {
            inner: parking_lot::RwLock::new(ManagerInner::default()),
            env: environment_scope.trim().to_string(),
            sched,
            docs: None,
        }
    }

    /// Go `WithStore`: installs durable persistence for routines and returns the manager.
    pub fn with_store(
        &mut self,
        store: Arc<parking_lot::Mutex<kura_store::SQLiteStore>>,
    ) -> &mut Self {
        self.docs = Some(store);
        self
    }

    /// Go `Restore`: reloads routines from an in-memory slice.
    pub fn restore(&self, routines: Vec<Routine>) {
        let mut inner = self.inner.write();
        inner.by_id.clear();
        inner.ids.clear();
        for routine in routines {
            inner.ids.push(routine.routine_id.clone());
            inner.by_id.insert(routine.routine_id.clone(), routine);
        }
    }

    /// Go `LoadFromStore`: reloads persisted routines from the document store on startup.
    /// A no-op when no store is installed.
    pub fn load_from_store(&self) -> Result<(), String> {
        let Some(docs) = &self.docs else {
            return Ok(());
        };
        let routines = kura_store::list_documents::<Routine>(&docs.lock(), DOC_KIND_ROUTINE)?;
        self.restore(routines);
        Ok(())
    }

    /// Compiles a definition without activating it and returns the schedule/workflow/approval/
    /// delivery/quota expectations to confirm before activation (FR-004) (Go `Preview`).
    pub fn preview(&self, def: &Definition) -> Result<Preview, RoutineError> {
        validate_definition(def)?;
        let kind = if def.trigger.kind == TriggerKind::Once {
            "one_time"
        } else {
            "recurring"
        };
        let approval = def.approval_expectation.trim();
        let approval = if approval.is_empty() {
            "ask".to_string()
        } else {
            approval.to_string()
        };
        Ok(Preview {
            schedule_kind: kind.to_string(),
            trigger_summary: trigger_summary(&def.trigger),
            workflow_summary: def.workflow.goal.trim().to_string(),
            approval_expectation: approval,
            delivery_preference_id: def.delivery_preference_id.trim().to_string(),
            retry_summary: format!("max {} retries", max_retries(def)),
        })
    }

    /// Validates a definition, compiles it to a schedule, and stores the routine at version 1
    /// (Go `Create`).
    pub fn create(&self, def: Definition) -> Result<Routine, RoutineError> {
        validate_definition(&def)?;
        let schedule = self
            .sched
            .create(&compile(&def))
            .map_err(RoutineError::CompileSchedule)?;
        let now = Utc::now();
        let routine = Routine {
            routine_id: new_id("routine"),
            environment_scope: self.env.clone(),
            name: def.name.trim().to_string(),
            state: State::Active,
            current_version: 1,
            current_schedule_id: schedule.schedule_id.clone(),
            definition: def.clone(),
            versions: vec![Version {
                version: 1,
                definition: def,
                schedule_id: schedule.schedule_id,
                created_at: now,
            }],
            created_at: now,
            updated_at: now,
        };
        self.store(routine.clone());
        Ok(routine)
    }

    /// Go `Get`.
    pub fn get(&self, routine_id: &str) -> Option<Routine> {
        self.inner.read().by_id.get(routine_id.trim()).cloned()
    }

    /// Go `List` (insertion order, mirroring the `kura-runtime` manager convention).
    pub fn list(&self) -> Vec<Routine> {
        let inner = self.inner.read();
        inner
            .ids
            .iter()
            .filter_map(|id| inner.by_id.get(id).cloned())
            .collect()
    }

    /// Creates a new routine version: compiles a new schedule and cancels the previous one; the
    /// prior version keeps its schedule id so its execution evidence is preserved (FR-003)
    /// (Go `Update`).
    pub fn update(&self, routine_id: &str, def: Definition) -> Result<Routine, RoutineError> {
        let routine = self.get(routine_id).ok_or(RoutineError::RoutineNotFound)?;
        if routine.state == State::Cancelled {
            return Err(RoutineError::RoutineCancelled);
        }
        validate_definition(&def)?;
        let schedule = self
            .sched
            .create(&compile(&def))
            .map_err(RoutineError::CompileSchedule)?;
        if !routine.current_schedule_id.is_empty() {
            // Prior schedule + its attempts remain as evidence.
            let _ = self.sched.cancel(&routine.current_schedule_id);
        }
        let now = Utc::now();
        let mut routine = routine;
        routine.current_version += 1;
        routine.definition = def.clone();
        routine.name = def.name.trim().to_string();
        routine.current_schedule_id = schedule.schedule_id.clone();
        routine.updated_at = now;
        if routine.state == State::Paused {
            // A paused routine that is edited recompiles paused.
            let _ = self.sched.pause(&schedule.schedule_id);
        }
        routine.versions.push(Version {
            version: routine.current_version,
            definition: def,
            schedule_id: schedule.schedule_id,
            created_at: now,
        });
        self.store(routine.clone());
        Ok(routine)
    }

    /// Go `Pause`.
    pub fn pause(&self, routine_id: &str) -> Result<Routine, RoutineError> {
        self.transition(routine_id, State::Paused, &|schedule_id| {
            let _ = self.sched.pause(schedule_id);
        })
    }

    /// Go `Resume`.
    pub fn resume(&self, routine_id: &str) -> Result<Routine, RoutineError> {
        self.transition(routine_id, State::Active, &|schedule_id| {
            let _ = self.sched.resume(schedule_id);
        })
    }

    /// Go `Cancel`.
    pub fn cancel(&self, routine_id: &str) -> Result<Routine, RoutineError> {
        self.transition(routine_id, State::Cancelled, &|schedule_id| {
            let _ = self.sched.cancel(schedule_id);
        })
    }

    /// Re-creates the routine's compiled schedule when it has gone missing (e.g. external
    /// cancellation), restoring the active routine to a working state without rewriting versions
    /// (Go `Repair`).
    pub fn repair(&self, routine_id: &str) -> Result<Routine, RoutineError> {
        let routine = self.get(routine_id).ok_or(RoutineError::RoutineNotFound)?;
        if routine.state == State::Cancelled {
            return Err(RoutineError::RoutineCancelled);
        }
        if !routine.current_schedule_id.is_empty() {
            if let Ok((_, exists)) = self.sched.get(&routine.current_schedule_id) {
                if exists {
                    return Ok(routine); // healthy; nothing to repair
                }
            }
        }
        let schedule = self
            .sched
            .create(&compile(&routine.definition))
            .map_err(RoutineError::RepairSchedule)?;
        if routine.state == State::Paused {
            let _ = self.sched.pause(&schedule.schedule_id);
        }
        let now = Utc::now();
        let mut routine = routine;
        routine.current_schedule_id = schedule.schedule_id.clone();
        routine.updated_at = now;
        // Reflect the repaired schedule id on the current version.
        if let Some(last) = routine.versions.last_mut() {
            last.schedule_id = schedule.schedule_id;
        }
        self.store(routine.clone());
        Ok(routine)
    }

    /// Go `transition`: applies a scheduler action to the current schedule, then moves state.
    fn transition(
        &self,
        routine_id: &str,
        state: State,
        sched_action: &dyn Fn(&str),
    ) -> Result<Routine, RoutineError> {
        let routine = self.get(routine_id).ok_or(RoutineError::RoutineNotFound)?;
        if routine.state == State::Cancelled {
            return Err(RoutineError::RoutineCancelled);
        }
        if !routine.current_schedule_id.is_empty() {
            sched_action(&routine.current_schedule_id);
        }
        let mut routine = routine;
        routine.state = state;
        routine.updated_at = Utc::now();
        self.store(routine.clone());
        Ok(routine)
    }

    /// Go `store`: write-through insert/update plus durable persistence (errors ignored, as in
    /// Go).
    fn store(&self, routine: Routine) {
        {
            let mut inner = self.inner.write();
            if !inner.by_id.contains_key(&routine.routine_id) {
                inner.ids.push(routine.routine_id.clone());
            }
            inner
                .by_id
                .insert(routine.routine_id.clone(), routine.clone());
        }
        if let Some(docs) = &self.docs {
            let _ = kura_store::put_document(
                &docs.lock(),
                DOC_KIND_ROUTINE,
                &routine.routine_id,
                &self.env,
                "",
                &routine,
            );
        }
    }
}

/// Maps a routine definition onto an existing scheduler create input (workflow target)
/// (Go `compile`).
fn compile(def: &Definition) -> CreateInput {
    let mut trigger = SchedulerTrigger {
        timezone: def.trigger.timezone.clone(),
        ..SchedulerTrigger::default()
    };
    if def.trigger.kind == TriggerKind::Once {
        trigger.kind = SchedulerTriggerKind::Once;
        trigger.fire_at = def.trigger.fire_at;
    } else {
        trigger.kind = SchedulerTriggerKind::Cron;
        trigger.cron_expr = def.trigger.cron_expr.clone();
    }
    let entrypoint = def.workflow.entrypoint.trim();
    let entrypoint = if entrypoint.is_empty() {
        "operator".to_string()
    } else {
        entrypoint.to_string()
    };
    CreateInput {
        trigger,
        target: SchedulerTarget {
            kind: SchedulerTargetKind::Workflow,
            active: true,
            workflow: Some(SchedulerWorkflowTarget {
                entrypoint,
                workflow_goal: def.workflow.goal.trim().to_string(),
            }),
            summary: def.name.trim().to_string(),
        },
        retry_policy: SchedulerRetryPolicy {
            max_retries: max_retries(def),
            backoff_kind: SchedulerRetryBackoffKind::Fixed,
            base_delay_seconds: 5,
            max_delay_seconds: 5,
        },
    }
}

/// Go `validateDefinition`.
fn validate_definition(def: &Definition) -> Result<(), RoutineError> {
    if def.name.trim().is_empty() {
        return Err(RoutineError::InvalidNameRequired);
    }
    if def.workflow.goal.trim().is_empty() {
        return Err(RoutineError::InvalidGoalRequired);
    }
    match def.trigger.kind {
        TriggerKind::Cron => {
            if def.trigger.cron_expr.trim().is_empty() {
                return Err(RoutineError::InvalidCronExprRequired);
            }
        }
        TriggerKind::Once => {
            if def.trigger.fire_at.is_none() {
                return Err(RoutineError::InvalidFireAtRequired);
            }
        }
    }
    Ok(())
}

/// Go `maxRetries`: defaults to 1.
#[must_use]
fn max_retries(def: &Definition) -> i64 {
    if def.max_retries > 0 {
        def.max_retries
    } else {
        1
    }
}

/// Go `triggerSummary`.
#[must_use]
fn trigger_summary(trigger: &Trigger) -> String {
    if trigger.kind == TriggerKind::Once {
        if let Some(fire_at) = trigger.fire_at {
            return format!("once at {}", fire_at.with_timezone(&Utc).to_rfc3339());
        }
    }
    format!("cron {}", trigger.cron_expr.trim())
}

/// Go `newID`: `prefix` + 16 hex chars of random bytes (reference `kura-runtime` convention).
#[must_use]
fn new_id(prefix: &str) -> String {
    let hex = Uuid::new_v4().simple().to_string();
    format!("{prefix}_{}", &hex[..16])
}

#[must_use]
fn is_zero_i64(v: &i64) -> bool {
    *v == 0
}

#[must_use]
fn is_false(v: &bool) -> bool {
    !*v
}
