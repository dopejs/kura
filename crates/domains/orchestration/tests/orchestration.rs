use chrono::Utc;
use kura_capabilities::Supervisor;
use kura_orchestration::{
    apply_computer_use_projection, dependencies_missing, is_terminal_step_status,
    is_terminal_workflow_status, plan_workflow, shell_escape, summarize_output,
    AddDependencyInput, AddHandoffInput, AddWorkflowStepInput, BlockedReason,
    CreateWorkflowInput, Dependency, DependencyType, Handoff, HandoffStatus,
    MCPPlanningServer, MCPPlanningSource, MCPPlanningTool, Manager, OrchestrationError,
    SkillPlanningCandidate, SkillPlanningSource, StepStatus, Workflow, WorkflowStatus,
    WorkflowStep,
};
use kura_runtime::{Run, RunStatus, ToolCall, ToolCallStatus};
use serde_json::json;

fn test_config() -> kura_config::Config {
    kura_config::Config {
        project_root: String::new(),
        environment: kura_config::Environment::Test,
        bind_addr: "127.0.0.1:19192".to_string(),
        data_dir: std::env::temp_dir()
            .join("kura_orchestration")
            .to_string_lossy()
            .to_string(),
        log_level: "info".to_string(),
        version: "0.1.0".to_string(),
        llm: kura_config::LlmConfig::default(),
        connectors: kura_config::ConnectorConfig::default(),
    }
}

fn test_run(goal: &str) -> Run {
    let now = Utc::now();
    Run {
        run_id: "run_test_1".to_string(),
        entrypoint: "do thing".to_string(),
        goal: goal.to_string(),
        status: RunStatus::Queued,
        created_at: now,
        updated_at: now,
        ..Run::default()
    }
}

fn tool_call(workflow_step_id: &str, status: ToolCallStatus, failure_class: &str) -> ToolCall {
    let now = Utc::now();
    ToolCall {
        tool_call_id: format!("tc_{workflow_step_id}"),
        run_id: "run_test_1".to_string(),
        step_id: "rt_step_1".to_string(),
        workflow_step_id: workflow_step_id.to_string(),
        tool_name: "echo".to_string(),
        status,
        failure_class: failure_class.to_string(),
        output: Some(json!({ "ok": true })),
        created_at: now,
        updated_at: now,
        ..ToolCall::default()
    }
}

struct TestSkillSource(Vec<SkillPlanningCandidate>);

impl SkillPlanningSource for TestSkillSource {
    fn list_skills(&self) -> Vec<SkillPlanningCandidate> {
        self.0.clone()
    }
}

struct TestMCPSource(Vec<MCPPlanningServer>);

impl MCPPlanningSource for TestMCPSource {
    fn list_servers(&self) -> Vec<MCPPlanningServer> {
        self.0.clone()
    }

    fn list_tools(&self, server_id: &str) -> Result<Vec<MCPPlanningTool>, String> {
        Ok(self
            .0
            .iter()
            .find(|server| server.server_id == server_id)
            .map(|server| server.tools.clone())
            .unwrap_or_default())
    }
}

#[test]
fn enum_wire_values() {
    assert_eq!(serde_json::to_value(WorkflowStatus::PlanningFailed).unwrap(), json!("planning_failed"));
    assert_eq!(serde_json::to_value(WorkflowStatus::PartialFailed).unwrap(), json!("partial_failed"));
    assert_eq!(serde_json::to_value(WorkflowStatus::Interrupted).unwrap(), json!("interrupted"));
    assert_eq!(serde_json::to_value(StepStatus::WaitingDependency).unwrap(), json!("waiting_dependency"));
    assert_eq!(serde_json::to_value(StepStatus::Skipped).unwrap(), json!("skipped"));
    assert_eq!(serde_json::to_value(DependencyType::Completion).unwrap(), json!("completion"));
    assert_eq!(serde_json::to_value(HandoffStatus::Consumed).unwrap(), json!("consumed"));
    assert_eq!(serde_json::to_value(BlockedReason::PolicyBlocked).unwrap(), json!("policy_blocked"));
    assert_eq!(WorkflowStatus::Planning.as_str(), "planning");
    assert_eq!(StepStatus::Skipped.to_string(), "skipped");
    let status: WorkflowStatus = serde_json::from_str("\"partial_failed\"").unwrap();
    assert_eq!(status, WorkflowStatus::PartialFailed);
}

#[test]
fn terminal_status_helpers() {
    assert!(is_terminal_workflow_status(WorkflowStatus::PlanningFailed));
    assert!(is_terminal_workflow_status(WorkflowStatus::Completed));
    assert!(is_terminal_workflow_status(WorkflowStatus::PartialFailed));
    assert!(is_terminal_workflow_status(WorkflowStatus::Failed));
    assert!(is_terminal_workflow_status(WorkflowStatus::Cancelled));
    assert!(is_terminal_workflow_status(WorkflowStatus::Interrupted));
    assert!(!is_terminal_workflow_status(WorkflowStatus::Planning));
    assert!(!is_terminal_workflow_status(WorkflowStatus::Running));
    assert!(is_terminal_step_status(StepStatus::Blocked));
    assert!(is_terminal_step_status(StepStatus::Skipped));
    assert!(is_terminal_step_status(StepStatus::Interrupted));
    assert!(!is_terminal_step_status(StepStatus::Ready));
    assert!(!is_terminal_step_status(StepStatus::WaitingDependency));
}

#[test]
fn summarize_output_truncates_long_payload() {
    assert_eq!(summarize_output(None), "");
    let value = json!({ "data": "x".repeat(300) });
    let summary = summarize_output(Some(&value));
    assert_eq!(summary.chars().count(), 160);
    assert!(summary.starts_with("{\"data\":\"xxx"));
}

#[test]
fn shell_escape_quotes_single_quotes() {
    assert_eq!(shell_escape("it's"), "it'\"'\"'s");
}

#[test]
fn create_workflow_planning_and_listed() {
    let manager = Manager::new();
    let workflow = manager
        .create_workflow("run_1", CreateWorkflowInput { goal: "g".to_string(), ..CreateWorkflowInput::default() })
        .unwrap();
    assert_eq!(workflow.status, WorkflowStatus::Planning);
    assert!(workflow.workflow_id.starts_with("wf_"));
    assert_eq!(workflow.run_id, "run_1");
    assert_eq!(workflow.goal, "g");
    assert_eq!(manager.list_workflows().len(), 1);
    assert_eq!(manager.list_workflows()[0].workflow_id, workflow.workflow_id);
    assert!(manager.get_workflow(&workflow.workflow_id).is_some());
    assert!(manager.get_workflow("missing").is_none());
}

#[test]
fn add_step_validates_required_fields() {
    let manager = Manager::new();
    let workflow = manager.create_workflow("run_1", CreateWorkflowInput::default()).unwrap();
    let err = manager
        .add_step(&workflow.workflow_id, AddWorkflowStepInput::default())
        .unwrap_err();
    assert!(matches!(err, OrchestrationError::TitleRequired));
    let err = manager
        .add_step(
            &workflow.workflow_id,
            AddWorkflowStepInput { title: "A".to_string(), consumer_kind: "skill".to_string(), ..AddWorkflowStepInput::default() },
        )
        .unwrap_err();
    assert!(matches!(err, OrchestrationError::ConsumerIDRequired));
}

#[test]
fn add_step_assigns_position_and_workflow() {
    let manager = Manager::new();
    let workflow = manager.create_workflow("run_1", CreateWorkflowInput::default()).unwrap();
    let first = manager
        .add_step(
            &workflow.workflow_id,
            AddWorkflowStepInput { title: "A".to_string(), consumer_kind: "calendar".to_string(), consumer_id: "cal_1".to_string(), ..AddWorkflowStepInput::default() },
        )
        .unwrap();
    assert!(first.workflow_step_id.starts_with("wfstep_"));
    assert_eq!(first.workflow_id, workflow.workflow_id);
    assert_eq!(first.position, 1);
    assert_eq!(first.status, StepStatus::Planned);
    assert_eq!(first.max_attempts, 1);
    let second = manager
        .add_step(
            &workflow.workflow_id,
            AddWorkflowStepInput { title: "B".to_string(), consumer_kind: "skill".to_string(), consumer_id: "s1".to_string(), max_attempts: 3, ..AddWorkflowStepInput::default() },
        )
        .unwrap();
    assert_eq!(second.position, 2);
    assert_eq!(second.max_attempts, 3);
    let stored = manager.get_workflow(&workflow.workflow_id).unwrap();
    assert_eq!(stored.steps.len(), 2);
    assert_eq!(stored.steps[0].workflow_step_id, first.workflow_step_id);
    assert_eq!(stored.steps[1].workflow_step_id, second.workflow_step_id);
}

#[test]
fn add_dependency_links_target_step() {
    let manager = Manager::new();
    let workflow = manager.create_workflow("run_1", CreateWorkflowInput::default()).unwrap();
    let first = manager
        .add_step(&workflow.workflow_id, AddWorkflowStepInput { title: "A".to_string(), consumer_kind: "calendar".to_string(), consumer_id: "cal_1".to_string(), ..AddWorkflowStepInput::default() })
        .unwrap();
    let second = manager
        .add_step(&workflow.workflow_id, AddWorkflowStepInput { title: "B".to_string(), consumer_kind: "skill".to_string(), consumer_id: "s1".to_string(), ..AddWorkflowStepInput::default() })
        .unwrap();
    let dependency = manager
        .add_dependency(
            &workflow.workflow_id,
            AddDependencyInput {
                from_workflow_step_id: first.workflow_step_id.clone(),
                to_workflow_step_id: second.workflow_step_id.clone(),
                dependency_type: DependencyType::Success,
                reason: "needs evidence".to_string(),
            },
        )
        .unwrap();
    assert!(dependency.dependency_id.starts_with("wfdep_"));
    assert_eq!(dependency.workflow_id, workflow.workflow_id);
    let stored = manager.get_workflow(&workflow.workflow_id).unwrap();
    assert_eq!(stored.dependencies.len(), 1);
    let target = stored.steps.iter().find(|step| step.workflow_step_id == second.workflow_step_id).unwrap();
    assert_eq!(target.dependency_ids, vec![dependency.dependency_id]);
}

#[test]
fn add_dependency_rejects_missing_or_unknown_steps() {
    let manager = Manager::new();
    let workflow = manager.create_workflow("run_1", CreateWorkflowInput::default()).unwrap();
    let err = manager
        .add_dependency(&workflow.workflow_id, AddDependencyInput { from_workflow_step_id: String::new(), to_workflow_step_id: "b".to_string(), ..AddDependencyInput::default() })
        .unwrap_err();
    assert!(matches!(err, OrchestrationError::StepIDRequired));
    let err = manager
        .add_dependency(&workflow.workflow_id, AddDependencyInput { from_workflow_step_id: "a".to_string(), to_workflow_step_id: "b".to_string(), ..AddDependencyInput::default() })
        .unwrap_err();
    assert!(matches!(err, OrchestrationError::StepNotFound));
    let err = manager.add_dependency("missing", AddDependencyInput::default()).unwrap_err();
    assert!(matches!(err, OrchestrationError::WorkflowNotFound));
}

#[test]
fn add_handoff_pending() {
    let manager = Manager::new();
    let workflow = manager.create_workflow("run_1", CreateWorkflowInput::default()).unwrap();
    let first = manager
        .add_step(&workflow.workflow_id, AddWorkflowStepInput { title: "A".to_string(), consumer_kind: "calendar".to_string(), consumer_id: "cal_1".to_string(), ..AddWorkflowStepInput::default() })
        .unwrap();
    let second = manager
        .add_step(&workflow.workflow_id, AddWorkflowStepInput { title: "B".to_string(), consumer_kind: "skill".to_string(), consumer_id: "s1".to_string(), ..AddWorkflowStepInput::default() })
        .unwrap();
    let handoff = manager
        .add_handoff(
            &workflow.workflow_id,
            AddHandoffInput {
                from_workflow_step_id: first.workflow_step_id.clone(),
                to_workflow_step_id: second.workflow_step_id.clone(),
                payload_summary: "evidence".to_string(),
                source_path: "step.output".to_string(),
            },
        )
        .unwrap();
    assert!(handoff.handoff_id.starts_with("wfhandoff_"));
    assert_eq!(handoff.status, HandoffStatus::Pending);
    assert_eq!(handoff.consumed_at, None);
    let stored = manager.get_workflow(&workflow.workflow_id).unwrap();
    assert_eq!(stored.handoffs.len(), 1);
}

#[test]
fn initialize_execution_marks_ready_and_waiting() {
    let manager = Manager::new();
    let workflow = manager.create_workflow("run_1", CreateWorkflowInput::default()).unwrap();
    let first = manager
        .add_step(&workflow.workflow_id, AddWorkflowStepInput { title: "A".to_string(), consumer_kind: "calendar".to_string(), consumer_id: "cal_1".to_string(), ..AddWorkflowStepInput::default() })
        .unwrap();
    let second = manager
        .add_step(&workflow.workflow_id, AddWorkflowStepInput { title: "B".to_string(), consumer_kind: "skill".to_string(), consumer_id: "s1".to_string(), ..AddWorkflowStepInput::default() })
        .unwrap();
    manager
        .add_dependency(
            &workflow.workflow_id,
            AddDependencyInput { from_workflow_step_id: first.workflow_step_id.clone(), to_workflow_step_id: second.workflow_step_id.clone(), dependency_type: DependencyType::Success, ..AddDependencyInput::default() },
        )
        .unwrap();
    let now = Utc::now();
    let wf = manager.initialize_execution(&workflow.workflow_id, now).unwrap();
    assert_eq!(wf.status, WorkflowStatus::Running);
    assert!(wf.started_at.is_some());
    assert_eq!(wf.updated_at, now);
    let a = wf.steps.iter().find(|step| step.workflow_step_id == first.workflow_step_id).unwrap();
    let b = wf.steps.iter().find(|step| step.workflow_step_id == second.workflow_step_id).unwrap();
    assert_eq!(a.status, StepStatus::Ready);
    assert_eq!(b.status, StepStatus::WaitingDependency);
}

#[test]
fn advance_ready_steps_unblocks_after_dependency_completes() {
    let manager = Manager::new();
    let workflow = manager.create_workflow("run_1", CreateWorkflowInput::default()).unwrap();
    let first = manager
        .add_step(&workflow.workflow_id, AddWorkflowStepInput { title: "A".to_string(), consumer_kind: "calendar".to_string(), consumer_id: "cal_1".to_string(), ..AddWorkflowStepInput::default() })
        .unwrap();
    let second = manager
        .add_step(&workflow.workflow_id, AddWorkflowStepInput { title: "B".to_string(), consumer_kind: "skill".to_string(), consumer_id: "s1".to_string(), ..AddWorkflowStepInput::default() })
        .unwrap();
    manager
        .add_dependency(
            &workflow.workflow_id,
            AddDependencyInput { from_workflow_step_id: first.workflow_step_id.clone(), to_workflow_step_id: second.workflow_step_id.clone(), dependency_type: DependencyType::Success, ..AddDependencyInput::default() },
        )
        .unwrap();
    let now = Utc::now();
    let _ = manager.initialize_execution(&workflow.workflow_id, now).unwrap();

    let completed = tool_call(&first.workflow_step_id, ToolCallStatus::Completed, "");
    let wf = manager
        .apply_tool_call_result(&workflow.workflow_id, &completed, None, "", now)
        .unwrap();
    let a = wf.steps.iter().find(|step| step.workflow_step_id == first.workflow_step_id).unwrap();
    assert_eq!(a.status, StepStatus::Completed);
    assert!(a.side_effects_visible);
    assert_eq!(a.output_summary, json!({ "ok": true }).to_string());

    let (wf, changed) = manager.advance_ready_steps(&workflow.workflow_id, now).unwrap();
    assert!(changed);
    let b = wf.steps.iter().find(|step| step.workflow_step_id == second.workflow_step_id).unwrap();
    assert_eq!(b.status, StepStatus::Ready);
}

#[test]
fn completion_reconciles_workflow_completed() {
    let manager = Manager::new();
    let workflow = manager.create_workflow("run_1", CreateWorkflowInput::default()).unwrap();
    let step = manager
        .add_step(&workflow.workflow_id, AddWorkflowStepInput { title: "A".to_string(), consumer_kind: "calendar".to_string(), consumer_id: "cal_1".to_string(), ..AddWorkflowStepInput::default() })
        .unwrap();
    let now = Utc::now();
    let _ = manager.initialize_execution(&workflow.workflow_id, now).unwrap();
    let completed = tool_call(&step.workflow_step_id, ToolCallStatus::Completed, "");
    let wf = manager.apply_tool_call_result(&workflow.workflow_id, &completed, None, "", now).unwrap();
    assert_eq!(wf.status, WorkflowStatus::Completed);
    assert!(wf.completed_at.is_some());
}

#[test]
fn denied_outcome_blocks_workflow() {
    let manager = Manager::new();
    let workflow = manager.create_workflow("run_1", CreateWorkflowInput::default()).unwrap();
    let step = manager
        .add_step(&workflow.workflow_id, AddWorkflowStepInput { title: "A".to_string(), consumer_kind: "skill".to_string(), consumer_id: "s1".to_string(), ..AddWorkflowStepInput::default() })
        .unwrap();
    let now = Utc::now();
    let _ = manager.initialize_execution(&workflow.workflow_id, now).unwrap();
    let denied = tool_call(&step.workflow_step_id, ToolCallStatus::Denied, "");
    let wf = manager.apply_tool_call_result(&workflow.workflow_id, &denied, None, "", now).unwrap();
    let updated = wf.steps.iter().find(|s| s.workflow_step_id == step.workflow_step_id).unwrap();
    assert_eq!(updated.status, StepStatus::Blocked);
    assert_eq!(updated.blocked_reason, BlockedReason::ApprovalDenied.as_str());
    assert_eq!(wf.status, WorkflowStatus::Blocked);
}

#[test]
fn failed_outcome_retries_then_fails() {
    let manager = Manager::new();
    let workflow = manager.create_workflow("run_1", CreateWorkflowInput::default()).unwrap();
    let step = manager
        .add_step(
            &workflow.workflow_id,
            AddWorkflowStepInput { title: "A".to_string(), consumer_kind: "skill".to_string(), consumer_id: "s1".to_string(), max_attempts: 2, ..AddWorkflowStepInput::default() },
        )
        .unwrap();
    let now = Utc::now();
    let _ = manager.initialize_execution(&workflow.workflow_id, now).unwrap();
    let _ = manager.start_step_attempt(&workflow.workflow_id, &step.workflow_step_id, "rt_1", now).unwrap();

    let failed = tool_call(&step.workflow_step_id, ToolCallStatus::Failed, "transient_error");
    let wf = manager.apply_tool_call_result(&workflow.workflow_id, &failed, None, "", now).unwrap();
    let updated = wf.steps.iter().find(|s| s.workflow_step_id == step.workflow_step_id).unwrap();
    assert_eq!(updated.status, StepStatus::Ready, "attempt 1 of 2 should retry");
    assert_eq!(updated.active_tool_call_id, "");
    assert_eq!(updated.last_failure_class, "transient_error");

    let _ = manager.start_step_attempt(&workflow.workflow_id, &step.workflow_step_id, "rt_2", now).unwrap();
    let wf = manager.apply_tool_call_result(&workflow.workflow_id, &failed, None, "", now).unwrap();
    let updated = wf.steps.iter().find(|s| s.workflow_step_id == step.workflow_step_id).unwrap();
    assert_eq!(updated.status, StepStatus::Failed, "attempt 2 of 2 should fail");
    assert_eq!(wf.status, WorkflowStatus::Failed);
}

#[test]
fn consumer_unavailable_blocks_workflow() {
    let manager = Manager::new();
    let workflow = manager.create_workflow("run_1", CreateWorkflowInput::default()).unwrap();
    let step = manager
        .add_step(&workflow.workflow_id, AddWorkflowStepInput { title: "A".to_string(), consumer_kind: "skill".to_string(), consumer_id: "s1".to_string(), ..AddWorkflowStepInput::default() })
        .unwrap();
    let now = Utc::now();
    let _ = manager.initialize_execution(&workflow.workflow_id, now).unwrap();
    let _ = manager.start_step_attempt(&workflow.workflow_id, &step.workflow_step_id, "rt_1", now).unwrap();
    let failed = tool_call(&step.workflow_step_id, ToolCallStatus::Failed, "consumer_unavailable");
    let wf = manager.apply_tool_call_result(&workflow.workflow_id, &failed, None, "", now).unwrap();
    let updated = wf.steps.iter().find(|s| s.workflow_step_id == step.workflow_step_id).unwrap();
    assert_eq!(updated.status, StepStatus::Blocked);
    assert_eq!(updated.blocked_reason, BlockedReason::ConsumerUnavailable.as_str());
}

#[test]
fn handoff_available_on_completion_consumed_on_start() {
    let manager = Manager::new();
    let workflow = manager.create_workflow("run_1", CreateWorkflowInput::default()).unwrap();
    let first = manager
        .add_step(&workflow.workflow_id, AddWorkflowStepInput { title: "A".to_string(), consumer_kind: "computer_use".to_string(), consumer_id: "browser".to_string(), ..AddWorkflowStepInput::default() })
        .unwrap();
    let second = manager
        .add_step(&workflow.workflow_id, AddWorkflowStepInput { title: "B".to_string(), consumer_kind: "skill".to_string(), consumer_id: "s1".to_string(), ..AddWorkflowStepInput::default() })
        .unwrap();
    manager
        .add_handoff(
            &workflow.workflow_id,
            AddHandoffInput { from_workflow_step_id: first.workflow_step_id.clone(), to_workflow_step_id: second.workflow_step_id.clone(), payload_summary: "evidence".to_string(), source_path: "step.computerUseArtifacts".to_string() },
        )
        .unwrap();
    let now = Utc::now();
    let _ = manager.initialize_execution(&workflow.workflow_id, now).unwrap();
    let completed = tool_call(&first.workflow_step_id, ToolCallStatus::Completed, "");
    let wf = manager.apply_tool_call_result(&workflow.workflow_id, &completed, None, "", now).unwrap();
    let handoff = &wf.handoffs[0];
    assert_eq!(handoff.status, HandoffStatus::Available);

    let _ = manager.start_step_attempt(&workflow.workflow_id, &second.workflow_step_id, "rt_2", now).unwrap();
    let stored = manager.get_workflow(&workflow.workflow_id).unwrap();
    assert_eq!(stored.handoffs[0].status, HandoffStatus::Consumed);
    assert!(stored.handoffs[0].consumed_at.is_some());
    let second_step = stored.steps.iter().find(|s| s.workflow_step_id == second.workflow_step_id).unwrap();
    assert_eq!(second_step.status, StepStatus::Running);
    assert_eq!(second_step.attempt_count, 1);
    assert_eq!(second_step.runtime_step_id, "rt_2");
}

#[test]
fn apply_computer_use_projection_records_artifacts() {
    let manager = Manager::new();
    let workflow = manager.create_workflow("run_1", CreateWorkflowInput::default()).unwrap();
    let step = manager
        .add_step(&workflow.workflow_id, AddWorkflowStepInput { title: "A".to_string(), consumer_kind: "computer_use".to_string(), consumer_id: "browser".to_string(), ..AddWorkflowStepInput::default() })
        .unwrap();
    let now = Utc::now();
    let artifact = kura_computeruse::Artifact {
        artifact_id: "art_1".to_string(),
        run_id: workflow.run_id.clone(),
        kind: kura_computeruse::ArtifactKind::Screenshot,
        status: kura_computeruse::ArtifactStatus::Available,
        created_at: now,
        ..kura_computeruse::Artifact::default()
    };
    let stored = manager.get_workflow(&workflow.workflow_id).unwrap();
    let wf = apply_computer_use_projection(
        stored,
        &step.workflow_step_id,
        "cu_sess_1",
        &["navigate".to_string(), "snapshot".to_string()],
        &[artifact],
        now,
    );
    let updated = wf.steps.iter().find(|s| s.workflow_step_id == step.workflow_step_id).unwrap();
    assert_eq!(updated.computer_use_session_id, "cu_sess_1");
    assert_eq!(updated.computer_use_action_ids, vec!["navigate".to_string(), "snapshot".to_string()]);
    assert_eq!(updated.computer_use_artifacts.len(), 1);
    assert_eq!(updated.computer_use_artifacts[0].artifact_id, "art_1");
}

#[test]
fn transformations_require_existing_workflow() {
    let manager = Manager::new();
    let now = Utc::now();
    let err = manager.initialize_execution("missing", now).unwrap_err();
    assert!(matches!(err, OrchestrationError::WorkflowNotFound));
    let err = manager.add_step("missing", AddWorkflowStepInput { title: "A".to_string(), consumer_kind: "skill".to_string(), consumer_id: "s1".to_string(), ..AddWorkflowStepInput::default() }).unwrap_err();
    assert!(matches!(err, OrchestrationError::WorkflowNotFound));
    let err = manager.advance_ready_steps("missing", now).unwrap_err();
    assert!(matches!(err, OrchestrationError::WorkflowNotFound));
}

#[test]
fn dependencies_missing_reports_unmet_deps() {
    let now = Utc::now();
    let mut workflow = Workflow {
        workflow_id: "wf_1".to_string(),
        run_id: "run_1".to_string(),
        status: WorkflowStatus::Planned,
        created_at: now,
        updated_at: now,
        steps: vec![
            WorkflowStep {
                workflow_step_id: "a".to_string(),
                workflow_id: "wf_1".to_string(),
                title: "A".to_string(),
                position: 1,
                consumer_kind: "local_tool".to_string(),
                consumer_id: "cap_1".to_string(),
                tool_name: "shell".to_string(),
                status: StepStatus::Planned,
                created_at: now,
                updated_at: now,
                ..WorkflowStep::default()
            },
            WorkflowStep {
                workflow_step_id: "b".to_string(),
                workflow_id: "wf_1".to_string(),
                title: "B".to_string(),
                position: 2,
                consumer_kind: "skill".to_string(),
                consumer_id: "s1".to_string(),
                tool_name: "s1".to_string(),
                status: StepStatus::Planned,
                created_at: now,
                updated_at: now,
                ..WorkflowStep::default()
            },
        ],
        dependencies: vec![Dependency {
            dependency_id: "d1".to_string(),
            workflow_id: "wf_1".to_string(),
            from_workflow_step_id: "a".to_string(),
            to_workflow_step_id: "b".to_string(),
            dependency_type: DependencyType::Success,
            reason: String::new(),
        }],
        ..Workflow::default()
    };
    let step_b = workflow.steps[1].clone();
    assert_eq!(dependencies_missing(&workflow, &step_b), vec!["d1".to_string()]);
    workflow.steps[0].status = StepStatus::Completed;
    assert!(dependencies_missing(&workflow, &step_b).is_empty());
}

#[test]
fn plan_planning_failed_without_consumers() {
    let workflow = plan_workflow(
        &test_config(),
        &test_run("g"),
        &CreateWorkflowInput { goal: "g".to_string(), ..CreateWorkflowInput::default() },
        None,
        None,
        None,
    );
    assert_eq!(workflow.status, WorkflowStatus::PlanningFailed);
    assert!(workflow.failure_summary.contains("No executable workflow consumers"));
    assert!(workflow.workflow_id.starts_with("wf_"));
    assert_eq!(workflow.run_id, "run_test_1");
    assert_eq!(workflow.environment_scope, "test");
}

#[test]
fn plan_local_shell_capability() {
    let supervisor = Supervisor::new();
    supervisor
        .register(kura_capabilities::RegisterInput { capability_id: "cap_shell".to_string(), kind: "shell".to_string(), display_name: "Shell".to_string() })
        .unwrap();
    let workflow = plan_workflow(
        &test_config(),
        &test_run("g"),
        &CreateWorkflowInput { goal: "g".to_string(), ..CreateWorkflowInput::default() },
        Some(&supervisor),
        None,
        None,
    );
    assert_eq!(workflow.status, WorkflowStatus::Planned);
    assert_eq!(workflow.plan_summary, "Plan one local tool step.");
    assert_eq!(workflow.steps.len(), 1);
    let step = &workflow.steps[0];
    assert_eq!(step.title, "Run local shell capability");
    assert_eq!(step.consumer_kind, "local_tool");
    assert_eq!(step.consumer_id, "cap_shell");
    assert_eq!(step.tool_name, "shell");
    assert_eq!(step.position, 1);
    assert_eq!(step.status, StepStatus::Planned);
    assert_eq!(step.approval_mode_expected, "ask");
    assert_eq!(step.max_attempts, 1);
    let input = step.input.as_ref().unwrap();
    assert_eq!(input["cmd"], json!("printf %s g"));
    assert_eq!(input["cwd"], json!(test_config().data_dir));
}

#[test]
fn plan_mcp_skill_combo_wires_dependency_and_handoff() {
    let skill_source = TestSkillSource(vec![SkillPlanningCandidate {
        skill_id: "s1".to_string(),
        approval_mode_expected: "allow".to_string(),
        executable: true,
        available: true,
    }]);
    let mcp_source = TestMCPSource(vec![MCPPlanningServer {
        server_id: "mcp_1".to_string(),
        tools: vec![MCPPlanningTool { tool_name: "lookup".to_string() }],
    }]);
    let workflow = plan_workflow(
        &test_config(),
        &test_run("g"),
        &CreateWorkflowInput { goal: "g".to_string(), ..CreateWorkflowInput::default() },
        None,
        Some(&skill_source),
        Some(&mcp_source),
    );
    assert_eq!(workflow.status, WorkflowStatus::Planned);
    assert_eq!(workflow.plan_summary, "Plan one MCP step followed by one executable skill handoff.");
    assert_eq!(workflow.steps.len(), 2);
    assert_eq!(workflow.steps[0].consumer_kind, "mcp_tool");
    assert_eq!(workflow.steps[0].title, "Use MCP tool lookup");
    assert_eq!(workflow.steps[1].consumer_kind, "skill");
    assert_eq!(workflow.steps[1].title, "Run executable skill s1");
    assert_eq!(workflow.dependencies.len(), 1);
    assert_eq!(workflow.dependencies[0].from_workflow_step_id, workflow.steps[0].workflow_step_id);
    assert_eq!(workflow.dependencies[0].to_workflow_step_id, workflow.steps[1].workflow_step_id);
    assert_eq!(workflow.dependencies[0].dependency_type, DependencyType::Success);
    assert_eq!(workflow.steps[1].dependency_ids, vec![workflow.dependencies[0].dependency_id.clone()]);
    assert_eq!(workflow.handoffs.len(), 1);
    assert_eq!(workflow.handoffs[0].status, HandoffStatus::Pending);
    assert_eq!(workflow.handoffs[0].from_workflow_step_id, workflow.steps[0].workflow_step_id);
}

#[test]
fn plan_browser_goal_picks_computer_use() {
    let workflow = plan_workflow(
        &test_config(),
        &test_run("automate the browser"),
        &CreateWorkflowInput { goal: "automate the browser".to_string(), ..CreateWorkflowInput::default() },
        None,
        None,
        None,
    );
    assert_eq!(workflow.status, WorkflowStatus::Planned);
    assert_eq!(workflow.plan_summary, "Plan one browser-first computer-use step.");
    assert_eq!(workflow.steps.len(), 1);
    assert_eq!(workflow.steps[0].consumer_kind, "computer_use");
    assert_eq!(workflow.steps[0].consumer_id, "browser");
    assert_eq!(workflow.steps[0].tool_name, "browser");
    let input = workflow.steps[0].input.as_ref().unwrap();
    assert_eq!(input["driverKind"], json!("browser"));
    assert_eq!(input["actions"][0]["actionKind"], json!("navigate"));
}

#[test]
fn plan_calendar_action_step() {
    let action = kura_calendar::Action {
        operation_class: kura_calendar::OperationClass::ListEvents,
        integration_id: "cal_main".to_string(),
        ..kura_calendar::Action::default()
    };
    let workflow = plan_workflow(
        &test_config(),
        &test_run(""),
        &CreateWorkflowInput { goal: String::new(), calendar_action: Some(action), ..CreateWorkflowInput::default() },
        None,
        None,
        None,
    );
    assert_eq!(workflow.status, WorkflowStatus::Planned);
    assert_eq!(workflow.plan_summary, "Plan one calendar domain step on the normal workflow runtime.");
    assert_eq!(workflow.steps.len(), 1);
    assert_eq!(workflow.steps[0].title, "Inspect calendar events");
    assert_eq!(workflow.steps[0].consumer_kind, "calendar");
    assert_eq!(workflow.steps[0].consumer_id, "cal_main");
    assert_eq!(workflow.steps[0].tool_name, "list_events");
    let input = workflow.steps[0].input.as_ref().unwrap();
    assert_eq!(input["operationClass"], json!("list_events"));
    assert_eq!(input["integrationId"], json!("cal_main"));
}

#[test]
fn plan_mail_action_step() {
    let action = kura_mail::Action {
        operation_class: kura_mail::OperationClass::SendMessage,
        ..kura_mail::Action::default()
    };
    let workflow = plan_workflow(
        &test_config(),
        &test_run(""),
        &CreateWorkflowInput { goal: String::new(), mail_action: Some(action), ..CreateWorkflowInput::default() },
        None,
        None,
        None,
    );
    assert_eq!(workflow.status, WorkflowStatus::Planned);
    assert_eq!(workflow.plan_summary, "Plan one mail domain step on the normal workflow runtime.");
    assert_eq!(workflow.steps.len(), 1);
    assert_eq!(workflow.steps[0].title, "Send mail message");
    assert_eq!(workflow.steps[0].consumer_kind, "mail");
    assert_eq!(workflow.steps[0].consumer_id, "mail");
    assert_eq!(workflow.steps[0].tool_name, "send_message");
}

#[test]
fn manager_plan_stores_workflow() {
    let manager = Manager::new();
    let supervisor = Supervisor::new();
    supervisor
        .register(kura_capabilities::RegisterInput { capability_id: "cap_shell".to_string(), kind: "shell".to_string(), display_name: "Shell".to_string() })
        .unwrap();
    let workflow = manager.plan(
        &test_config(),
        &test_run("g"),
        &CreateWorkflowInput { goal: "g".to_string(), ..CreateWorkflowInput::default() },
        Some(&supervisor),
        None,
        None,
    );
    assert_eq!(manager.list_workflows().len(), 1);
    let stored = manager.get_workflow(&workflow.workflow_id).unwrap();
    assert_eq!(stored.status, WorkflowStatus::Planned);
    assert_eq!(stored.steps.len(), 1);
}

#[test]
fn workflow_serialization_roundtrip() {
    let now = Utc::now();
    let workflow = Workflow {
        workflow_id: "wf_1".to_string(),
        run_id: "run_1".to_string(),
        schedule_id: "sched_1".to_string(),
        environment_scope: "test".to_string(),
        goal: "g".to_string(),
        status: WorkflowStatus::Running,
        plan_summary: "Plan one step.".to_string(),
        created_at: now,
        updated_at: now,
        started_at: Some(now),
        steps: vec![
            WorkflowStep {
                workflow_step_id: "wfstep_1".to_string(),
                workflow_id: "wf_1".to_string(),
                title: "Step one".to_string(),
                position: 1,
                consumer_kind: "calendar".to_string(),
                consumer_id: "cal_main".to_string(),
                tool_name: "list_events".to_string(),
                input: Some(json!({ "query": "g" })),
                status: StepStatus::Completed,
                attempt_count: 1,
                max_attempts: 1,
                side_effects_visible: true,
                output_summary: json!({ "ok": true }).to_string(),
                created_at: now,
                updated_at: now,
                ..WorkflowStep::default()
            },
            WorkflowStep {
                workflow_step_id: "wfstep_2".to_string(),
                workflow_id: "wf_1".to_string(),
                title: "Step two".to_string(),
                position: 2,
                consumer_kind: "skill".to_string(),
                consumer_id: "s1".to_string(),
                tool_name: "s1".to_string(),
                status: StepStatus::WaitingDependency,
                dependency_ids: vec!["wfdep_1".to_string()],
                max_attempts: 2,
                created_at: now,
                updated_at: now,
                ..WorkflowStep::default()
            },
        ],
        dependencies: vec![Dependency {
            dependency_id: "wfdep_1".to_string(),
            workflow_id: "wf_1".to_string(),
            from_workflow_step_id: "wfstep_1".to_string(),
            to_workflow_step_id: "wfstep_2".to_string(),
            dependency_type: DependencyType::Success,
            reason: "needs evidence".to_string(),
        }],
        handoffs: vec![Handoff {
            handoff_id: "wfhandoff_1".to_string(),
            workflow_id: "wf_1".to_string(),
            from_workflow_step_id: "wfstep_1".to_string(),
            to_workflow_step_id: "wfstep_2".to_string(),
            status: HandoffStatus::Available,
            payload_summary: "evidence".to_string(),
            source_path: "step.output".to_string(),
            consumed_at: Some(now),
            ..Handoff::default()
        }],
        ..Workflow::default()
    };

    let serialized = serde_json::to_string(&workflow).unwrap();
    let value: serde_json::Value = serde_json::from_str(&serialized).unwrap();
    assert_eq!(value["workflowId"], json!("wf_1"));
    assert_eq!(value["runId"], json!("run_1"));
    assert_eq!(value["scheduleId"], json!("sched_1"));
    assert_eq!(value["environmentScope"], json!("test"));
    assert_eq!(value["status"], json!("running"));
    assert_eq!(value["planSummary"], json!("Plan one step."));
    assert_eq!(value["steps"][0]["workflowStepId"], json!("wfstep_1"));
    assert_eq!(value["steps"][0]["status"], json!("completed"));
    assert_eq!(value["steps"][0]["sideEffectsVisible"], json!(true));
    assert_eq!(value["steps"][1]["dependencyIds"], json!(["wfdep_1"]));
    assert_eq!(value["dependencies"][0]["dependencyType"], json!("success"));
    assert_eq!(value["handoffs"][0]["status"], json!("available"));
    assert_eq!(value["handoffs"][0]["consumedAt"].is_string(), true);

    let decoded: Workflow = serde_json::from_str(&serialized).unwrap();
    assert_eq!(decoded, workflow);

    // Temp-dir file round trip, matching the store-crate test style.
    let dir = std::env::temp_dir().join(format!("kura_orchestration_tests_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("workflow_roundtrip.json");
    std::fs::write(&path, &serialized).unwrap();
    let reread = std::fs::read_to_string(&path).unwrap();
    let decoded: Workflow = serde_json::from_str(&reread).unwrap();
    assert_eq!(decoded, workflow);
    let _ = std::fs::remove_dir_all(&dir);
}
