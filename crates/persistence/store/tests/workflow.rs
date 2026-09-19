//! Round-trip integration tests for the workflow-orchestration CRUD methods ported from
//! `daemon/internal/store/store.go` into `workflow.rs`: workflows and their child
//! steps/dependencies/handoffs. Each test constructs a workflow under a seeded run,
//! upserts it (and replaces its child rows), then lists/gets it back. The `workflow`
//! row decodes its document from `document_json` while children load from their own
//! tables, mirroring the Go read path. Wiring required before these compile: declare
//! `pub mod workflow;` in `lib.rs` and add `kura-orchestration.workspace = true` to
//! `Cargo.toml` (the record structs live at `kura_store::workflow::*`).

use chrono::{DateTime, Utc};
use kura_orchestration::{
    Dependency, DependencyType, Handoff, HandoffStatus, StepStatus, Workflow, WorkflowStatus,
    WorkflowStep,
};
use kura_runtime::{Run, RunStatus};
use kura_store::SQLiteStore;

fn temp_dir(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("kura_store_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().to_string()
}

/// Creates a run so the workflow row's `run_id` FK references an existing row.
fn upsert_run(store: &SQLiteStore, run_id: &str) {
    let now = Utc::now();
    let run = Run {
        run_id: run_id.to_string(),
        session_id: String::new(),
        entrypoint: "test entrypoint".to_string(),
        status: RunStatus::Running,
        goal: "test goal".to_string(),
        created_at: now,
        updated_at: now,
        ..Run::default()
    };
    store.upsert_run(&run).unwrap();
}

fn sample_workflow(
    workflow_id: &str,
    run_id: &str,
    status: WorkflowStatus,
    at: DateTime<Utc>,
) -> Workflow {
    Workflow {
        workflow_id: workflow_id.to_string(),
        run_id: run_id.to_string(),
        schedule_id: "sched_1".to_string(),
        schedule_attempt_id: "sched_att_1".to_string(),
        environment_scope: "test".to_string(),
        goal: "ship the release".to_string(),
        status,
        plan_summary: "two steps".to_string(),
        failure_summary: "no blockers".to_string(),
        created_at: at,
        updated_at: at,
        started_at: Some(at),
        ..Workflow::default()
    }
}

fn sample_step(
    workflow_id: &str,
    step_id: &str,
    position: i64,
    status: StepStatus,
) -> WorkflowStep {
    let now = Utc::now();
    WorkflowStep {
        workflow_step_id: step_id.to_string(),
        workflow_id: workflow_id.to_string(),
        title: format!("Step {step_id}"),
        position,
        consumer_kind: "tool_call".to_string(),
        consumer_id: "tc_1".to_string(),
        tool_name: "shell".to_string(),
        input: Some(serde_json::json!({ "cmd": "echo hi" })),
        status,
        selection_rationale: "test step".to_string(),
        approval_mode_expected: "allow".to_string(),
        runtime_step_id: format!("runstep_{step_id}"),
        active_tool_call_id: format!("tc_{step_id}"),
        attempt_count: 1,
        max_attempts: 3,
        created_at: now,
        updated_at: now,
        ..WorkflowStep::default()
    }
}

#[test]
fn workflow_round_trips_through_sqlite() {
    let dir = temp_dir("workflow");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();

    upsert_run(&store, "run_wf");

    let mut workflow = sample_workflow("wf_1", "run_wf", WorkflowStatus::Running, now);
    store.upsert_workflow(&workflow).unwrap();
    // Upsert again through the ON CONFLICT path with changed fields.
    workflow.status = WorkflowStatus::Completed;
    workflow.plan_summary = "revised plan".to_string();
    workflow.completed_at = Some(now);
    store.upsert_workflow(&workflow).unwrap();

    let listed = store.list_workflows("test", "run_wf").unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.workflow_id, "wf_1");
    assert_eq!(got.run_id, "run_wf");
    assert_eq!(got.schedule_id, "sched_1");
    assert_eq!(got.schedule_attempt_id, "sched_att_1");
    assert_eq!(got.environment_scope, "test");
    assert_eq!(got.goal, "ship the release");
    assert_eq!(got.status, WorkflowStatus::Completed);
    assert_eq!(got.plan_summary, "revised plan");
    assert_eq!(got.failure_summary, "no blockers");
    assert_eq!(got.created_at, now);
    assert_eq!(got.updated_at, now);
    assert_eq!(got.started_at, Some(now));
    assert_eq!(got.completed_at, Some(now));
    assert_eq!(got.interrupted_at, None);

    // No rows for a different run or environment scope.
    assert!(
        store
            .list_workflows("test", "other_run")
            .unwrap()
            .is_empty()
    );
    assert!(store.list_workflows("prod", "run_wf").unwrap().is_empty());

    // List ordering is created_at ASC, workflow_id ASC.
    let second = sample_workflow(
        "wf_2",
        "run_wf",
        WorkflowStatus::Planned,
        now + chrono::Duration::seconds(10),
    );
    store.upsert_workflow(&second).unwrap();
    let listed = store.list_workflows("test", "run_wf").unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].workflow_id, "wf_1");
    assert_eq!(listed[1].workflow_id, "wf_2");

    let fetched = store
        .get_workflow("test", "run_wf", "wf_1")
        .unwrap()
        .expect("found");
    assert_eq!(fetched.workflow_id, "wf_1");
    assert_eq!(fetched.status, WorkflowStatus::Completed);
    assert_eq!(
        store.get_workflow("test", "run_wf", "missing").unwrap(),
        None
    );
    assert_eq!(store.get_workflow("prod", "run_wf", "wf_1").unwrap(), None);

    let by_id = store
        .get_workflow_by_id("test", "wf_1")
        .unwrap()
        .expect("found");
    assert_eq!(by_id.goal, "ship the release");
    assert_eq!(by_id.status, WorkflowStatus::Completed);
    assert_eq!(store.get_workflow_by_id("prod", "wf_1").unwrap(), None);
    assert_eq!(store.get_workflow_by_id("test", "missing").unwrap(), None);
}

#[test]
fn workflow_step_round_trips_through_sqlite() {
    let dir = temp_dir("workflow_step");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();

    upsert_run(&store, "run_wf");
    store
        .upsert_workflow(&sample_workflow(
            "wf_1",
            "run_wf",
            WorkflowStatus::Running,
            now,
        ))
        .unwrap();

    let mut steps = vec![
        sample_step("wf_1", "wfstep_1", 1, StepStatus::Running),
        sample_step("wf_1", "wfstep_2", 2, StepStatus::Ready),
    ];
    store.replace_workflow_steps("wf_1", &steps).unwrap();
    // Replace again (delete + reinsert) with changed fields.
    steps[0].status = StepStatus::Completed;
    steps[0].runtime_step_id = "runstep_wfstep_1_v2".to_string();
    store.replace_workflow_steps("wf_1", &steps).unwrap();

    let got = store
        .get_workflow("test", "run_wf", "wf_1")
        .unwrap()
        .expect("found");
    assert_eq!(got.steps.len(), 2);
    // Ordered by position ASC, workflow_step_id ASC.
    let first = &got.steps[0];
    assert_eq!(first.workflow_step_id, "wfstep_1");
    assert_eq!(first.workflow_id, "wf_1");
    assert_eq!(first.position, 1);
    assert_eq!(first.status, StepStatus::Completed);
    assert_eq!(first.title, "Step wfstep_1");
    assert_eq!(first.consumer_kind, "tool_call");
    assert_eq!(first.tool_name, "shell");
    assert_eq!(first.runtime_step_id, "runstep_wfstep_1_v2");
    assert_eq!(first.active_tool_call_id, "tc_wfstep_1");
    assert_eq!(first.attempt_count, 1);
    assert_eq!(first.max_attempts, 3);
    assert_eq!(first.input, Some(serde_json::json!({ "cmd": "echo hi" })));
    assert_eq!(got.steps[1].workflow_step_id, "wfstep_2");
    assert_eq!(got.steps[1].status, StepStatus::Ready);
    assert_eq!(got.steps[1].position, 2);
}

#[test]
fn workflow_dependency_round_trips_through_sqlite() {
    let dir = temp_dir("workflow_dependency");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();

    upsert_run(&store, "run_wf");
    store
        .upsert_workflow(&sample_workflow(
            "wf_1",
            "run_wf",
            WorkflowStatus::Running,
            now,
        ))
        .unwrap();

    let items = vec![
        Dependency {
            dependency_id: "wfdep_1".to_string(),
            workflow_id: "wf_1".to_string(),
            from_workflow_step_id: "wfstep_1".to_string(),
            to_workflow_step_id: "wfstep_2".to_string(),
            dependency_type: DependencyType::Success,
            reason: "step 1 completes first".to_string(),
        },
        Dependency {
            dependency_id: "wfdep_2".to_string(),
            workflow_id: "wf_1".to_string(),
            from_workflow_step_id: "wfstep_2".to_string(),
            to_workflow_step_id: "wfstep_3".to_string(),
            dependency_type: DependencyType::Completion,
            reason: String::new(),
        },
    ];
    store.replace_workflow_dependencies("wf_1", &items).unwrap();
    // Replace again with a changed field.
    let mut revised = items.clone();
    revised[0].dependency_type = DependencyType::Failure;
    store
        .replace_workflow_dependencies("wf_1", &revised)
        .unwrap();

    let got = store
        .get_workflow("test", "run_wf", "wf_1")
        .unwrap()
        .expect("found");
    assert_eq!(got.dependencies.len(), 2);
    // Ordered by dependency_id ASC.
    assert_eq!(got.dependencies[0].dependency_id, "wfdep_1");
    assert_eq!(got.dependencies[0].workflow_id, "wf_1");
    assert_eq!(got.dependencies[0].from_workflow_step_id, "wfstep_1");
    assert_eq!(got.dependencies[0].to_workflow_step_id, "wfstep_2");
    assert_eq!(got.dependencies[0].dependency_type, DependencyType::Failure);
    assert_eq!(got.dependencies[0].reason, "step 1 completes first");
    assert_eq!(got.dependencies[1].dependency_id, "wfdep_2");
    assert_eq!(
        got.dependencies[1].dependency_type,
        DependencyType::Completion
    );
}

#[test]
fn workflow_handoff_round_trips_through_sqlite() {
    let dir = temp_dir("workflow_handoff");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();

    upsert_run(&store, "run_wf");
    store
        .upsert_workflow(&sample_workflow(
            "wf_1",
            "run_wf",
            WorkflowStatus::Running,
            now,
        ))
        .unwrap();

    let mut items = vec![Handoff {
        handoff_id: "wfhandoff_1".to_string(),
        workflow_id: "wf_1".to_string(),
        from_workflow_step_id: "wfstep_1".to_string(),
        to_workflow_step_id: "wfstep_2".to_string(),
        status: HandoffStatus::Pending,
        payload_summary: "handoff payload".to_string(),
        source_path: "step.output".to_string(),
        consumed_at: None,
        invalid_reason: String::new(),
    }];
    store.replace_workflow_handoffs("wf_1", &items).unwrap();
    // Replace again with a changed field (status is mirrored in its own column).
    items[0].status = HandoffStatus::Consumed;
    items[0].consumed_at = Some(now);
    store.replace_workflow_handoffs("wf_1", &items).unwrap();

    let got = store
        .get_workflow("test", "run_wf", "wf_1")
        .unwrap()
        .expect("found");
    assert_eq!(got.handoffs.len(), 1);
    let handoff = &got.handoffs[0];
    assert_eq!(handoff.handoff_id, "wfhandoff_1");
    assert_eq!(handoff.workflow_id, "wf_1");
    assert_eq!(handoff.from_workflow_step_id, "wfstep_1");
    assert_eq!(handoff.to_workflow_step_id, "wfstep_2");
    assert_eq!(handoff.status, HandoffStatus::Consumed);
    assert_eq!(handoff.payload_summary, "handoff payload");
    assert_eq!(handoff.source_path, "step.output");
    assert_eq!(handoff.consumed_at, Some(now));
}

#[test]
fn mark_in_flight_workflows_interrupted_round_trips() {
    let dir = temp_dir("workflow_interrupt");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();

    upsert_run(&store, "run_wf");

    // A running workflow with a running step, a completed step, and a pending handoff.
    let mut running = sample_workflow("wf_running", "run_wf", WorkflowStatus::Running, now);
    running.steps = vec![
        sample_step("wf_running", "wfstep_1", 1, StepStatus::Running),
        sample_step("wf_running", "wfstep_2", 2, StepStatus::Completed),
    ];
    running.handoffs = vec![Handoff {
        handoff_id: "wfhandoff_1".to_string(),
        workflow_id: "wf_running".to_string(),
        from_workflow_step_id: "wfstep_1".to_string(),
        to_workflow_step_id: "wfstep_2".to_string(),
        status: HandoffStatus::Pending,
        payload_summary: "handoff payload".to_string(),
        source_path: "step.output".to_string(),
        consumed_at: None,
        invalid_reason: String::new(),
    }];
    store.upsert_workflow(&running).unwrap();
    store
        .replace_workflow_steps("wf_running", &running.steps)
        .unwrap();
    store
        .replace_workflow_handoffs("wf_running", &running.handoffs)
        .unwrap();

    // A completed workflow must not be touched.
    store
        .upsert_workflow(&sample_workflow(
            "wf_done",
            "run_wf",
            WorkflowStatus::Completed,
            now,
        ))
        .unwrap();

    let interrupted_at = now + chrono::Duration::seconds(5);
    let updated = store
        .mark_in_flight_workflows_interrupted("test", interrupted_at)
        .unwrap();
    assert_eq!(updated.len(), 1);
    let got = &updated[0];
    assert_eq!(got.workflow_id, "wf_running");
    assert_eq!(got.status, WorkflowStatus::Interrupted);
    assert_eq!(got.updated_at, interrupted_at);
    assert_eq!(got.interrupted_at, Some(interrupted_at));
    assert_eq!(got.steps[0].status, StepStatus::Interrupted);
    assert_eq!(got.steps[0].updated_at, interrupted_at);
    assert_eq!(got.steps[1].status, StepStatus::Completed);
    assert_eq!(got.handoffs[0].status, HandoffStatus::Invalid);
    assert_eq!(
        got.handoffs[0].invalid_reason,
        "daemon_restart_interrupted_workflow"
    );

    // The mutated workflow, steps, and handoffs were persisted.
    let persisted = store
        .get_workflow_by_id("test", "wf_running")
        .unwrap()
        .expect("found");
    assert_eq!(persisted.status, WorkflowStatus::Interrupted);
    assert_eq!(persisted.interrupted_at, Some(interrupted_at));
    assert_eq!(persisted.steps[0].status, StepStatus::Interrupted);
    assert_eq!(persisted.handoffs[0].status, HandoffStatus::Invalid);
    let untouched = store
        .get_workflow_by_id("test", "wf_done")
        .unwrap()
        .expect("found");
    assert_eq!(untouched.status, WorkflowStatus::Completed);

    // A second call finds nothing left to interrupt.
    assert!(
        store
            .mark_in_flight_workflows_interrupted("test", interrupted_at)
            .unwrap()
            .is_empty()
    );
}
