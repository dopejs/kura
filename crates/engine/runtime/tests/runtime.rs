use kura_runtime::{
    CompleteToolCallInput, CreateRunInput, CreateStepInput, CreateToolCallInput, Manager,
    RunStatus, RuntimeError, StepStatus, ToolCallStatus, UpdateStepStatusInput,
    live_validation_matrix_rows,
};
use serde_json::json;

fn new_run(manager: &Manager) -> kura_runtime::Run {
    manager
        .create_run(CreateRunInput {
            entrypoint: "do thing".to_string(),
            goal: "g".to_string(),
            ..CreateRunInput::default()
        })
        .unwrap()
}

#[test]
fn create_run_requires_entrypoint() {
    let manager = Manager::new();
    let err = manager.create_run(CreateRunInput::default()).unwrap_err();
    assert!(matches!(err, RuntimeError::EntrypointRequired));
}

#[test]
fn create_run_queued_and_listed() {
    let manager = Manager::new();
    let run = new_run(&manager);
    assert_eq!(run.status, RunStatus::Queued);
    assert!(run.run_id.starts_with("run_"));
    assert_eq!(manager.list_runs().len(), 1);
    assert!(manager.get_run(&run.run_id).is_some());
}

#[test]
fn create_step_requires_title() {
    let manager = Manager::new();
    let run = new_run(&manager);
    let err = manager
        .create_step(&run.run_id, CreateStepInput::default())
        .unwrap_err();
    assert!(matches!(err, RuntimeError::TitleRequired));
}

#[test]
fn step_transition_reconciles_run() {
    let manager = Manager::new();
    let run = new_run(&manager);
    let step = manager
        .create_step(
            &run.run_id,
            CreateStepInput {
                title: "Plan".to_string(),
                ..CreateStepInput::default()
            },
        )
        .unwrap();
    assert_eq!(step.status, StepStatus::Queued);
    let (step, run_opt) = manager
        .update_step_status_and_reconcile_run(
            &run.run_id,
            &step.step_id,
            UpdateStepStatusInput {
                status: StepStatus::Planning,
                ..UpdateStepStatusInput::default()
            },
        )
        .unwrap();
    assert_eq!(step.status, StepStatus::Planning);
    let updated = run_opt.expect("run should reconcile");
    assert_eq!(updated.status, RunStatus::Running);
}

#[test]
fn invalid_step_transition_rejected() {
    let manager = Manager::new();
    let run = new_run(&manager);
    let step = manager
        .create_step(
            &run.run_id,
            CreateStepInput {
                title: "Plan".to_string(),
                ..CreateStepInput::default()
            },
        )
        .unwrap();
    let err = manager
        .update_step_status(
            &run.run_id,
            &step.step_id,
            UpdateStepStatusInput {
                status: StepStatus::Completed,
                ..UpdateStepStatusInput::default()
            },
        )
        .unwrap_err();
    assert!(matches!(err, RuntimeError::InvalidStepTransition));
}

#[test]
fn create_tool_call_requires_target() {
    let manager = Manager::new();
    let run = new_run(&manager);
    let step = manager
        .create_step(
            &run.run_id,
            CreateStepInput {
                title: "Do".to_string(),
                ..CreateStepInput::default()
            },
        )
        .unwrap();
    let err = manager
        .create_tool_call(
            &run.run_id,
            &step.step_id,
            CreateToolCallInput {
                tool_name: "echo".to_string(),
                ..CreateToolCallInput::default()
            },
        )
        .unwrap_err();
    assert!(matches!(err, RuntimeError::ToolTargetRequired));
}

#[test]
fn tool_call_lifecycle_completes() {
    let manager = Manager::new();
    let run = new_run(&manager);
    let step = manager
        .create_step(
            &run.run_id,
            CreateStepInput {
                title: "Do".to_string(),
                ..CreateStepInput::default()
            },
        )
        .unwrap();
    let tc = manager
        .create_tool_call(
            &run.run_id,
            &step.step_id,
            CreateToolCallInput {
                tool_name: "echo".to_string(),
                capability_id: "cap_1".to_string(),
                ..CreateToolCallInput::default()
            },
        )
        .unwrap();
    assert_eq!(tc.status, ToolCallStatus::Requested);
    assert_eq!(tc.invocation_kind, "local_tool");
    let completed = manager
        .complete_tool_call(
            &run.run_id,
            &step.step_id,
            &tc.tool_call_id,
            CompleteToolCallInput {
                output: Some(json!({ "ok": true })),
                ..CompleteToolCallInput::default()
            },
        )
        .unwrap();
    assert_eq!(completed.status, ToolCallStatus::Completed);
    assert_eq!(completed.output, Some(json!({ "ok": true })));
}

#[test]
fn cancel_run_cancels_nonterminal_steps() {
    let manager = Manager::new();
    let run = new_run(&manager);
    let _ = manager
        .create_step(
            &run.run_id,
            CreateStepInput {
                title: "Plan".to_string(),
                ..CreateStepInput::default()
            },
        )
        .unwrap();
    let (cancelled, steps, already) = manager.cancel_run(&run.run_id).unwrap();
    assert!(!already);
    assert_eq!(cancelled.status, RunStatus::Cancelled);
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].status, StepStatus::Cancelled);
}

#[test]
fn snapshot_run_roundtrips() {
    let manager = Manager::new();
    let run = new_run(&manager);
    let step = manager
        .create_step(
            &run.run_id,
            CreateStepInput {
                title: "Plan".to_string(),
                ..CreateStepInput::default()
            },
        )
        .unwrap();
    let snapshot = manager.snapshot_run(&run.run_id).unwrap();
    assert_eq!(snapshot.run.run_id, run.run_id);
    assert_eq!(snapshot.steps.len(), 1);
    assert_eq!(snapshot.steps[0].step_id, step.step_id);
}

#[test]
fn live_validation_rows_covers_local_tool_call() {
    assert_eq!(live_validation_matrix_rows().len(), 1);
}
