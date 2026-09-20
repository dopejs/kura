//! Routine collector tests (wave 8 parity): section shape, unknown/missing
//! routine fallbacks, non-routine scopes, and the full generate() flow with
//! the real `kura-routine` manager.

use std::sync::Arc;

use kura_evidence::{
    Bundle, Collector, Manager, RedactionStatus, RoutineCollector, Scope, ScopeKind,
};
use kura_routine::{CreateInput, Definition, Schedule, Scheduler, Trigger, TriggerKind, Workflow};

fn run_scope(ref_value: &str) -> Scope {
    Scope {
        kind: ScopeKind::Routine,
        r#ref: ref_value.to_string(),
        window_start: None,
        window_end: None,
    }
}

fn scope_for(kind: ScopeKind, ref_value: &str) -> Scope {
    Scope {
        kind,
        r#ref: ref_value.to_string(),
        window_start: None,
        window_end: None,
    }
}

#[derive(Default)]
struct FakeScheduler {
    next_schedule_id: std::sync::atomic::AtomicUsize,
}

impl Scheduler for FakeScheduler {
    fn create(&self, _input: &CreateInput) -> Result<Schedule, String> {
        let n = self
            .next_schedule_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Schedule {
            schedule_id: format!("sched_{n}"),
            ..Schedule::default()
        })
    }
    fn pause(&self, _schedule_id: &str) -> Result<(Schedule, bool), String> {
        Ok((Schedule::default(), true))
    }
    fn resume(&self, _schedule_id: &str) -> Result<(Schedule, bool), String> {
        Ok((Schedule::default(), true))
    }
    fn cancel(&self, _schedule_id: &str) -> Result<(Schedule, bool), String> {
        Ok((Schedule::default(), true))
    }
    fn get(&self, _schedule_id: &str) -> Result<(Schedule, bool), String> {
        Ok((Schedule::default(), false))
    }
}

fn routine_manager() -> Arc<kura_routine::Manager> {
    let manager = Arc::new(kura_routine::Manager::new(
        "test",
        Box::<FakeScheduler>::default(),
    ));
    let def = Definition {
        name: "Weekly digest".to_string(),
        trigger: Trigger {
            kind: TriggerKind::Cron,
            cron_expr: "0 9 * * 1".to_string(),
            ..Trigger::default()
        },
        workflow: Workflow {
            entrypoint: String::new(),
            goal: "Send the weekly digest".to_string(),
        },
        ..Definition::default()
    };
    manager.create(def).expect("create routine");
    manager
}

#[test]
fn routine_collector_builds_routine_section() {
    let routines = routine_manager();
    let collector = RoutineCollector::new(Some(routines.clone()));
    let routine = routines.list().first().cloned().expect("one routine");

    let sections = collector
        .collect("tenant-a", &run_scope(&routine.routine_id))
        .expect("collect");
    assert_eq!(sections.len(), 1);
    let section = &sections[0];
    assert_eq!(section.kind, "routine");
    assert_eq!(
        section.resource_refs,
        vec![
            routine.routine_id.clone(),
            routine.current_schedule_id.clone()
        ]
    );
    assert_eq!(section.summary["name"], "Weekly digest");
    assert_eq!(section.summary["state"], "active");
    assert_eq!(section.summary["currentVersion"], "1");
    assert_eq!(
        section.links,
        vec![format!("/v1/routines/{}", routine.routine_id)]
    );
}

#[test]
fn routine_collector_returns_empty_for_unknown_routine() {
    let routines = routine_manager();
    let collector = RoutineCollector::new(Some(routines));
    let sections = collector
        .collect("tenant-a", &run_scope("routine_nope"))
        .expect("collect");
    assert!(sections.is_empty());
}

#[test]
fn routine_collector_ignores_non_routine_scopes() {
    let routines = routine_manager();
    let collector = RoutineCollector::new(Some(routines));
    for kind in [
        ScopeKind::Run,
        ScopeKind::Thread,
        ScopeKind::Connector,
        ScopeKind::TimeWindow,
        ScopeKind::QuotaDenial,
    ] {
        let sections = collector
            .collect("tenant-a", &scope_for(kind, "some_ref"))
            .expect("collect");
        assert!(sections.is_empty(), "{kind:?} must collect nothing");
    }
}

#[test]
fn routine_collector_without_manager_returns_empty() {
    let collector = RoutineCollector::new(None);
    let sections = collector
        .collect("tenant-a", &run_scope("routine_1"))
        .expect("collect");
    assert!(sections.is_empty());
}

#[test]
fn generate_includes_routine_section() {
    let routines = routine_manager();
    let routine = routines.list().first().cloned().expect("one routine");
    let collector = RoutineCollector::new(Some(routines));
    let manager = Manager::new("test", Some(Box::new(collector)), None);

    let bundle: Bundle = manager
        .generate("tenant-a", "support@kura", run_scope(&routine.routine_id))
        .expect("generate");
    assert_eq!(bundle.redaction_status, RedactionStatus::Redacted);
    assert_eq!(bundle.sections.len(), 1);
    assert_eq!(bundle.sections[0].kind, "routine");
    assert_eq!(bundle.sections[0].summary["name"], "Weekly digest");
    assert!(bundle.sections[0].summary.contains_key("state"));
}

#[test]
fn generate_with_unknown_routine_produces_bundle_without_sections() {
    let routines = routine_manager();
    let collector = RoutineCollector::new(Some(routines));
    let manager = Manager::new("test", Some(Box::new(collector)), None);
    let bundle = manager
        .generate("tenant-a", "support@kura", run_scope("routine_nope"))
        .expect("generate");
    assert!(bundle.sections.is_empty());
}
