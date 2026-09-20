//! Port of the Go `daemon/internal/orchestration` package: the workflow types
//! (workflow / workflow-step / dependency / handoff), the planning pipeline that
//! derives a planned step graph from a goal and the available consumers, the
//! step-state transition helpers that drive execution, and an in-memory workflow
//! manager backed by `parking_lot::RwLock` with insertion-ordered ids,
//! mirroring the `kura-runtime` crate.
//!
//! The Go package's `Manager` is stateless: every method takes and returns a
//! `Workflow` value. Those transformations are ported verbatim as free
//! functions (`plan_workflow`, `initialize_execution`, `advance_ready_steps`,
//! `start_step_attempt`, `bind_tool_call`, `apply_tool_call_result`,
//! `reconcile_status`, `apply_computer_use_projection`, ...). On top of them,
//! this crate provides a stateful `Manager` that stores workflows in memory
//! (insertion-ordered ids) and applies the same transformations to the stored
//! copy, so callers can create/list/get workflows and drive their lifecycle
//! through one handle. The `add_step` / `add_dependency` / `add_handoff`
//! methods are additions for building a workflow graph incrementally; the Go
//! planner builds the graph at `Plan` time instead.

use std::collections::HashMap;

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

string_enum!(WorkflowStatus {
    Planning => "planning",
    PlanningFailed => "planning_failed",
    Planned => "planned",
    Running => "running",
    Blocked => "blocked",
    Completed => "completed",
    PartialFailed => "partial_failed",
    Failed => "failed",
    Cancelled => "cancelled",
    Interrupted => "interrupted",
});

string_enum!(StepStatus {
    Planned => "planned",
    WaitingDependency => "waiting_dependency",
    Ready => "ready",
    Blocked => "blocked",
    Running => "running",
    Completed => "completed",
    Failed => "failed",
    Cancelled => "cancelled",
    Interrupted => "interrupted",
    Skipped => "skipped",
});

string_enum!(DependencyType {
    Success => "success",
    Failure => "failure",
    Completion => "completion",
});

string_enum!(HandoffStatus {
    Pending => "pending",
    Available => "available",
    Consumed => "consumed",
    Invalid => "invalid",
});

string_enum!(BlockedReason {
    ApprovalDenied => "approval_denied",
    PolicyBlocked => "policy_blocked",
    ConsumerUnavailable => "consumer_unavailable",
});

/// Manager validation/lookup failures. The Go package surfaces sentinel errors
/// from its runtime-adjacent callers; the stateful manager exposes them as a
/// typed enum in the style of `kura-runtime`.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum OrchestrationError {
    #[error("workflow not found")]
    WorkflowNotFound,
    #[error("workflow step not found")]
    StepNotFound,
    #[error("workflow step id is required")]
    StepIDRequired,
    #[error("title is required")]
    TitleRequired,
    #[error("consumer kind is required")]
    ConsumerKindRequired,
    #[error("consumer id is required")]
    ConsumerIDRequired,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateWorkflowInput {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub goal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar_action: Option<kura_calendar::Action>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mail_action: Option<kura_mail::Action>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workflow {
    pub workflow_id: String,
    pub run_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub schedule_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub schedule_attempt_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reminder_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reminder_occurrence_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub environment_scope: String,
    pub goal: String,
    pub status: WorkflowStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub plan_summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub failure_summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub latest_delivery_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub latest_delivery_status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub latest_delivery_target_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_profile_projection: Option<kura_profiles::RuntimeProjection>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interrupted_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<WorkflowStep>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<Dependency>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub handoffs: Vec<Handoff>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowStep {
    pub workflow_step_id: String,
    pub workflow_id: String,
    pub title: String,
    pub position: i64,
    pub consumer_kind: String,
    pub consumer_id: String,
    pub tool_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Value>,
    pub status: StepStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub selection_rationale: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub approval_mode_expected: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependency_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub runtime_step_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub active_tool_call_id: String,
    pub attempt_count: i64,
    pub max_attempts: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_failure_class: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub blocked_reason: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub side_effects_visible: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub output_summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub computer_use_session_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub computer_use_action_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub computer_use_artifacts: Vec<kura_computeruse::Artifact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub integration_bindings: Vec<kura_integrations::BindingSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub calendar_operation_summaries: Vec<kura_calendar::OperationSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mail_operation_summaries: Vec<kura_mail::OperationSummary>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dependency {
    pub dependency_id: String,
    pub workflow_id: String,
    pub from_workflow_step_id: String,
    pub to_workflow_step_id: String,
    pub dependency_type: DependencyType,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Handoff {
    pub handoff_id: String,
    pub workflow_id: String,
    pub from_workflow_step_id: String,
    pub to_workflow_step_id: String,
    pub status: HandoffStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub payload_summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumed_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub invalid_reason: String,
}

/// Input to `Manager::add_step` — builds a planned `WorkflowStep` inside an
/// existing workflow (manager-only convenience; Go builds steps at plan time).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddWorkflowStepInput {
    pub title: String,
    pub position: i64,
    pub consumer_kind: String,
    pub consumer_id: String,
    pub tool_name: String,
    #[serde(default)]
    pub input: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub approval_mode_expected: String,
    pub max_attempts: i64,
}

/// Input to `Manager::add_dependency`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddDependencyInput {
    pub from_workflow_step_id: String,
    pub to_workflow_step_id: String,
    pub dependency_type: DependencyType,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

/// Input to `Manager::add_handoff`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddHandoffInput {
    pub from_workflow_step_id: String,
    pub to_workflow_step_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub payload_summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_path: String,
}

/// Planning source for MCP tool consumers (Go `MCPPlanningSource`).
pub trait MCPPlanningSource: Send + Sync {
    fn list_servers(&self) -> Vec<MCPPlanningServer>;
    fn list_tools(&self, server_id: &str) -> Result<Vec<MCPPlanningTool>, String>;
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MCPPlanningServer {
    pub server_id: String,
    pub tools: Vec<MCPPlanningTool>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MCPPlanningTool {
    pub tool_name: String,
}

/// Planning source for skill consumers (Go `SkillPlanningSource`).
pub trait SkillPlanningSource: Send + Sync {
    fn list_skills(&self) -> Vec<SkillPlanningCandidate>;
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SkillPlanningCandidate {
    pub skill_id: String,
    pub approval_mode_expected: String,
    pub executable: bool,
    pub available: bool,
}

#[must_use]
fn is_false(v: &bool) -> bool {
    !*v
}

#[must_use]
pub fn is_terminal_workflow_status(status: WorkflowStatus) -> bool {
    matches!(
        status,
        WorkflowStatus::PlanningFailed
            | WorkflowStatus::Completed
            | WorkflowStatus::PartialFailed
            | WorkflowStatus::Failed
            | WorkflowStatus::Cancelled
            | WorkflowStatus::Interrupted
    )
}

#[must_use]
pub fn is_terminal_step_status(status: StepStatus) -> bool {
    matches!(
        status,
        StepStatus::Completed
            | StepStatus::Failed
            | StepStatus::Cancelled
            | StepStatus::Interrupted
            | StepStatus::Skipped
            | StepStatus::Blocked
    )
}

/// Go `firstNonEmpty`: returns the first value whose trimmed form is
/// non-empty (the original, untrimmed value), else "".
#[must_use]
pub fn first_non_empty(values: &[&str]) -> String {
    values
        .iter()
        .find(|value| !value.trim().is_empty())
        .map(|value| (*value).to_string())
        .unwrap_or_default()
}

/// Go `shellEscape`: single-quote shell escaping for embedded command args.
#[must_use]
pub fn shell_escape(value: &str) -> String {
    value.replace('\'', "'\"'\"'")
}

/// Go `SummarizeOutput`: JSON-serializes a value and truncates to 160 chars.
#[must_use]
pub fn summarize_output(value: Option<&serde_json::Value>) -> String {
    let Some(value) = value else {
        return String::new();
    };
    let text = serde_json::to_string(value).unwrap_or_else(|_| format!("{value:?}"));
    text.chars().take(160).collect()
}

/// Go `WorkflowStepByID`.
#[must_use]
pub fn workflow_step_by_id<'a>(
    workflow: &'a Workflow,
    workflow_step_id: &str,
) -> Option<&'a WorkflowStep> {
    workflow
        .steps
        .iter()
        .find(|step| step.workflow_step_id == workflow_step_id)
}

/// Go `DependenciesMissing`: dependency ids whose upstream step has not yet
/// satisfied the dependency condition.
#[must_use]
pub fn dependencies_missing(workflow: &Workflow, step: &WorkflowStep) -> Vec<String> {
    let mut missing = Vec::new();
    for dependency in &workflow.dependencies {
        if dependency.to_workflow_step_id != step.workflow_step_id {
            continue;
        }
        let Some(from) = workflow_step_by_id(workflow, &dependency.from_workflow_step_id) else {
            missing.push(dependency.dependency_id.clone());
            continue;
        };
        match dependency.dependency_type {
            DependencyType::Success => {
                if from.status != StepStatus::Completed {
                    missing.push(dependency.dependency_id.clone());
                }
            }
            DependencyType::Failure => {
                if from.status != StepStatus::Failed {
                    missing.push(dependency.dependency_id.clone());
                }
            }
            DependencyType::Completion => {
                if !is_terminal_step_status(from.status) {
                    missing.push(dependency.dependency_id.clone());
                }
            }
        }
    }
    missing
}

#[must_use]
pub fn new_workflow_id() -> String {
    format!("wf_{}", new_workflow_token())
}

#[must_use]
pub fn new_workflow_step_id() -> String {
    format!("wfstep_{}", new_workflow_token())
}

#[must_use]
pub fn new_workflow_dependency_id() -> String {
    format!("wfdep_{}", new_workflow_token())
}

#[must_use]
pub fn new_workflow_handoff_id() -> String {
    format!("wfhandoff_{}", new_workflow_token())
}

#[must_use]
fn new_workflow_token() -> String {
    // Go: 6 random bytes, hex-encoded (12 hex chars).
    let hex = Uuid::new_v4().simple().to_string();
    hex[..12].to_string()
}

/// Go `InitializeExecution`: marks the workflow running and resolves each
/// step to Ready or WaitingDependency.
#[must_use]
pub fn initialize_execution(mut workflow: Workflow, now: DateTime<Utc>) -> Workflow {
    if workflow.started_at.is_none() {
        workflow.started_at = Some(now);
    }
    workflow.status = WorkflowStatus::Running;
    workflow.updated_at = now;
    let missing: Vec<bool> = workflow
        .steps
        .iter()
        .map(|step| !dependencies_missing(&workflow, step).is_empty())
        .collect();
    for (idx, step) in workflow.steps.iter_mut().enumerate() {
        if missing[idx] {
            step.status = StepStatus::WaitingDependency;
        } else {
            step.status = StepStatus::Ready;
        }
        step.updated_at = now;
    }
    workflow
}

/// Go `AdvanceReadySteps`: flips Planned/WaitingDependency steps to Ready
/// when their dependencies are met. Returns the workflow and whether anything
/// changed.
#[must_use]
pub fn advance_ready_steps(mut workflow: Workflow, now: DateTime<Utc>) -> (Workflow, bool) {
    let missing: Vec<bool> = workflow
        .steps
        .iter()
        .map(|step| !dependencies_missing(&workflow, step).is_empty())
        .collect();
    let mut changed = false;
    for (idx, step) in workflow.steps.iter_mut().enumerate() {
        if step.status != StepStatus::Planned && step.status != StepStatus::WaitingDependency {
            continue;
        }
        if !missing[idx] {
            if step.status != StepStatus::Ready {
                step.status = StepStatus::Ready;
                step.updated_at = now;
                changed = true;
            }
            continue;
        }
        if step.status != StepStatus::WaitingDependency {
            step.status = StepStatus::WaitingDependency;
            step.updated_at = now;
            changed = true;
        }
    }
    if changed {
        workflow.updated_at = now;
    }
    (workflow, changed)
}

/// Go `StartStepAttempt`: binds a runtime step id, increments the attempt
/// counter, marks the step running, and consumes any available handoffs
/// targeting it.
#[must_use]
pub fn start_step_attempt(
    mut workflow: Workflow,
    workflow_step_id: &str,
    runtime_step_id: &str,
    now: DateTime<Utc>,
) -> Workflow {
    if let Some(step) = workflow
        .steps
        .iter_mut()
        .find(|step| step.workflow_step_id == workflow_step_id)
    {
        step.runtime_step_id = runtime_step_id.to_string();
        step.attempt_count += 1;
        step.status = StepStatus::Running;
        step.updated_at = now;
    }
    for handoff in &mut workflow.handoffs {
        if handoff.to_workflow_step_id == workflow_step_id
            && handoff.status == HandoffStatus::Available
        {
            handoff.status = HandoffStatus::Consumed;
            handoff.consumed_at = Some(now);
        }
    }
    workflow.updated_at = now;
    workflow
}

/// Go `BindToolCall`: records the active tool call and the runtime
/// integration/mail projections on the matching step.
#[must_use]
pub fn bind_tool_call(
    mut workflow: Workflow,
    workflow_step_id: &str,
    tool_call: &kura_runtime::ToolCall,
    now: DateTime<Utc>,
) -> Workflow {
    if let Some(step) = workflow
        .steps
        .iter_mut()
        .find(|step| step.workflow_step_id == workflow_step_id)
    {
        step.active_tool_call_id = tool_call.tool_call_id.clone();
        step.runtime_step_id = tool_call.step_id.clone();
        step.integration_bindings = tool_call.integration_bindings.clone();
        step.mail_operation_summaries = tool_call.mail_operation_summaries.clone();
        step.updated_at = now;
    }
    workflow.updated_at = now;
    workflow
}

/// Go `ApplyToolCallResult`: applies a runtime tool call outcome to the
/// matching workflow step, then reconciles the workflow status. The hinted
/// status maps to Go's non-empty `hintedStatus` (only used for in-flight
/// outcomes).
#[must_use]
pub fn apply_tool_call_result(
    mut workflow: Workflow,
    tool_call: &kura_runtime::ToolCall,
    hinted_status: Option<StepStatus>,
    blocked_reason: &str,
    now: DateTime<Utc>,
) -> Workflow {
    if let Some(step) = workflow
        .steps
        .iter_mut()
        .find(|step| step.workflow_step_id == tool_call.workflow_step_id)
    {
        step.active_tool_call_id = tool_call.tool_call_id.clone();
        step.runtime_step_id = tool_call.step_id.clone();
        step.integration_bindings = tool_call.integration_bindings.clone();
        step.mail_operation_summaries = tool_call.mail_operation_summaries.clone();
        step.updated_at = now;
        match tool_call.status {
            kura_runtime::ToolCallStatus::Completed => {
                step.status = StepStatus::Completed;
                step.side_effects_visible = true;
                step.output_summary = summarize_output(tool_call.output.as_ref());
                step.blocked_reason = String::new();
                for handoff in &mut workflow.handoffs {
                    if handoff.from_workflow_step_id == step.workflow_step_id {
                        handoff.status = HandoffStatus::Available;
                    }
                }
            }
            kura_runtime::ToolCallStatus::Denied => {
                step.status = StepStatus::Blocked;
                step.blocked_reason =
                    first_non_empty(&[blocked_reason, BlockedReason::ApprovalDenied.as_str()]);
            }
            kura_runtime::ToolCallStatus::Cancelled => {
                step.status = StepStatus::Cancelled;
            }
            kura_runtime::ToolCallStatus::Failed => {
                step.last_failure_class = tool_call.failure_class.clone();
                if tool_call.failure_class == "approval_rejected"
                    || tool_call.failure_class.contains("approval")
                {
                    step.status = StepStatus::Blocked;
                    step.blocked_reason =
                        first_non_empty(&[blocked_reason, BlockedReason::ApprovalDenied.as_str()]);
                } else if tool_call.failure_class == "consumer_unavailable" {
                    step.status = StepStatus::Blocked;
                    step.blocked_reason = BlockedReason::ConsumerUnavailable.as_str().to_string();
                } else if step.attempt_count < step.max_attempts {
                    step.status = StepStatus::Ready;
                    step.active_tool_call_id = String::new();
                    step.blocked_reason = String::new();
                } else {
                    step.status = StepStatus::Failed;
                }
            }
            _ => {
                if let Some(hinted) = hinted_status {
                    step.status = hinted;
                }
            }
        }
    }
    reconcile_status(workflow, now)
}

/// Go `ReconcileStatus`: derives the workflow status from the step statuses.
/// A workflow explicitly cancelled/interrupted keeps its status.
#[must_use]
pub fn reconcile_status(mut workflow: Workflow, now: DateTime<Utc>) -> Workflow {
    let mut has_running = false;
    let mut has_blocked = false;
    let mut has_failed = false;
    let mut has_cancelled = false;
    let mut all_complete = !workflow.steps.is_empty();
    let mut side_effects = false;
    for step in &workflow.steps {
        if step.side_effects_visible {
            side_effects = true;
        }
        match step.status {
            StepStatus::Ready | StepStatus::WaitingDependency | StepStatus::Running => {
                has_running = true;
                all_complete = false;
            }
            StepStatus::Blocked => {
                has_blocked = true;
                all_complete = false;
            }
            StepStatus::Failed => {
                has_failed = true;
                all_complete = false;
            }
            StepStatus::Cancelled => {
                has_cancelled = true;
                all_complete = false;
            }
            StepStatus::Completed | StepStatus::Skipped => {}
            _ => {
                all_complete = false;
            }
        }
    }
    if workflow.status != WorkflowStatus::Cancelled
        && workflow.status != WorkflowStatus::Interrupted
    {
        if has_running {
            workflow.status = WorkflowStatus::Running;
        } else if has_blocked {
            workflow.status = WorkflowStatus::Blocked;
        } else if has_failed && side_effects {
            workflow.status = WorkflowStatus::PartialFailed;
            workflow.completed_at = Some(now);
        } else if has_failed {
            workflow.status = WorkflowStatus::Failed;
            workflow.completed_at = Some(now);
        } else if has_cancelled {
            workflow.status = WorkflowStatus::Cancelled;
            workflow.completed_at = Some(now);
        } else if all_complete {
            workflow.status = WorkflowStatus::Completed;
            workflow.completed_at = Some(now);
        } else {
            workflow.status = WorkflowStatus::Running;
        }
    }
    workflow.updated_at = now;
    workflow
}

/// Go `ApplyComputerUseProjection`.
#[must_use]
pub fn apply_computer_use_projection(
    mut workflow: Workflow,
    workflow_step_id: &str,
    session_id: &str,
    actions: &[String],
    artifacts: &[kura_computeruse::Artifact],
    now: DateTime<Utc>,
) -> Workflow {
    if let Some(step) = workflow
        .steps
        .iter_mut()
        .find(|step| step.workflow_step_id == workflow_step_id)
    {
        step.computer_use_session_id = session_id.trim().to_string();
        step.computer_use_action_ids = actions.to_vec();
        step.computer_use_artifacts = artifacts.to_vec();
        step.updated_at = now;
    }
    workflow.updated_at = now;
    workflow
}

/// Go `Manager.Plan`: derives a planned workflow for the run from the goal
/// (or a calendar/mail action), preferring browser-first computer-use, then
/// MCP/skill handoffs, then local capabilities.
#[must_use]
pub fn plan_workflow(
    cfg: &kura_config::Config,
    run: &kura_runtime::Run,
    input: &CreateWorkflowInput,
    capability_supervisor: Option<&kura_capabilities::Supervisor>,
    skill_source: Option<&dyn SkillPlanningSource>,
    mcp_source: Option<&dyn MCPPlanningSource>,
) -> Workflow {
    let now = Utc::now();
    let goal = {
        let goal = input.goal.trim().to_string();
        if goal.is_empty() {
            run.goal.trim().to_string()
        } else {
            goal
        }
    };
    let mut workflow = Workflow {
        workflow_id: new_workflow_id(),
        run_id: run.run_id.clone(),
        environment_scope: environment_scope(cfg.environment),
        goal,
        status: WorkflowStatus::Planning,
        created_at: now,
        updated_at: now,
        ..Workflow::default()
    };

    if let Some(calendar_action) = &input.calendar_action {
        let calendar_step = pick_calendar_workflow_step(calendar_action, now);
        workflow.plan_summary =
            "Plan one calendar domain step on the normal workflow runtime.".to_string();
        workflow.steps = vec![calendar_step];
        workflow.status = WorkflowStatus::Planned;
        for (idx, step) in workflow.steps.iter_mut().enumerate() {
            step.workflow_id = workflow.workflow_id.clone();
            step.position = (idx + 1) as i64;
            step.status = StepStatus::Planned;
            step.created_at = now;
            step.updated_at = now;
            step.max_attempts = step.max_attempts.max(1);
        }
        return workflow;
    }
    if let Some(mail_action) = &input.mail_action {
        let mail_step = pick_mail_workflow_step(mail_action, now);
        workflow.plan_summary =
            "Plan one mail domain step on the normal workflow runtime.".to_string();
        workflow.steps = vec![mail_step];
        workflow.status = WorkflowStatus::Planned;
        for (idx, step) in workflow.steps.iter_mut().enumerate() {
            step.workflow_id = workflow.workflow_id.clone();
            step.position = (idx + 1) as i64;
            step.status = StepStatus::Planned;
            step.created_at = now;
            step.updated_at = now;
            step.max_attempts = step.max_attempts.max(1);
        }
        return workflow;
    }

    let (mcp_step, has_mcp) = pick_mcp_workflow_step(&workflow.goal, mcp_source, now);
    let (skill_step, has_skill) = pick_skill_workflow_step(&workflow.goal, skill_source, now);
    let (computer_use_step, has_computer_use) =
        pick_computer_use_workflow_step(&workflow.goal, now);
    let (local_step, has_local) =
        pick_local_workflow_step(cfg, &workflow.goal, capability_supervisor, now);

    if has_computer_use && has_skill {
        workflow.plan_summary =
            "Plan one browser-first computer-use step followed by one executable skill handoff."
                .to_string();
        workflow.steps = vec![computer_use_step.clone(), skill_step.clone()];
        workflow.dependencies = vec![Dependency {
            dependency_id: new_workflow_dependency_id(),
            workflow_id: workflow.workflow_id.clone(),
            from_workflow_step_id: computer_use_step.workflow_step_id.clone(),
            to_workflow_step_id: skill_step.workflow_step_id.clone(),
            dependency_type: DependencyType::Success,
            reason: "workflow consumes browser evidence before local continuation".to_string(),
        }];
        workflow.handoffs = vec![Handoff {
            handoff_id: new_workflow_handoff_id(),
            workflow_id: workflow.workflow_id.clone(),
            from_workflow_step_id: computer_use_step.workflow_step_id.clone(),
            to_workflow_step_id: skill_step.workflow_step_id.clone(),
            status: HandoffStatus::Pending,
            payload_summary: "Browser evidence summary".to_string(),
            source_path: "step.computerUseArtifacts".to_string(),
            ..Handoff::default()
        }];
    } else if has_mcp && has_skill {
        workflow.plan_summary =
            "Plan one MCP step followed by one executable skill handoff.".to_string();
        workflow.steps = vec![mcp_step.clone(), skill_step.clone()];
        workflow.dependencies = vec![Dependency {
            dependency_id: new_workflow_dependency_id(),
            workflow_id: workflow.workflow_id.clone(),
            from_workflow_step_id: mcp_step.workflow_step_id.clone(),
            to_workflow_step_id: skill_step.workflow_step_id.clone(),
            dependency_type: DependencyType::Success,
            reason: "workflow consumes MCP output before local continuation".to_string(),
        }];
        workflow.handoffs = vec![Handoff {
            handoff_id: new_workflow_handoff_id(),
            workflow_id: workflow.workflow_id.clone(),
            from_workflow_step_id: mcp_step.workflow_step_id.clone(),
            to_workflow_step_id: skill_step.workflow_step_id.clone(),
            status: HandoffStatus::Pending,
            payload_summary: "MCP lookup result summary".to_string(),
            source_path: "step.output.result".to_string(),
            ..Handoff::default()
        }];
    } else if has_skill {
        workflow.plan_summary = "Plan one executable skill step.".to_string();
        workflow.steps = vec![skill_step.clone()];
    } else if has_computer_use {
        workflow.plan_summary = "Plan one browser-first computer-use step.".to_string();
        workflow.steps = vec![computer_use_step.clone()];
    } else if has_mcp {
        workflow.plan_summary = "Plan one MCP tool step.".to_string();
        workflow.steps = vec![mcp_step.clone()];
    } else if has_local {
        workflow.plan_summary = "Plan one local tool step.".to_string();
        workflow.steps = vec![local_step.clone()];
    } else {
        workflow.status = WorkflowStatus::PlanningFailed;
        workflow.failure_summary =
            "No executable workflow consumers are available for the current daemon state."
                .to_string();
        return workflow;
    }

    workflow.status = WorkflowStatus::Planned;
    for (idx, step) in workflow.steps.iter_mut().enumerate() {
        step.workflow_id = workflow.workflow_id.clone();
        step.position = (idx + 1) as i64;
        step.status = StepStatus::Planned;
        step.created_at = now;
        step.updated_at = now;
        step.max_attempts = step.max_attempts.max(1);
    }
    for dependency in &mut workflow.dependencies {
        dependency.workflow_id = workflow.workflow_id.clone();
    }
    for handoff in &mut workflow.handoffs {
        handoff.workflow_id = workflow.workflow_id.clone();
    }
    for step in &mut workflow.steps {
        let dependency_ids: Vec<String> = workflow
            .dependencies
            .iter()
            .filter(|dependency| dependency.to_workflow_step_id == step.workflow_step_id)
            .map(|dependency| dependency.dependency_id.clone())
            .collect();
        step.dependency_ids = dependency_ids;
    }
    workflow
}

fn environment_scope(env: kura_config::Environment) -> String {
    match env {
        kura_config::Environment::Prod => "prod".to_string(),
        kura_config::Environment::Test => "test".to_string(),
        // Embedded shares the non-production isolation scope with test.
        kura_config::Environment::Embedded => "test".to_string(),
    }
}

fn pick_mcp_workflow_step(
    goal: &str,
    mcp_source: Option<&dyn MCPPlanningSource>,
    now: DateTime<Utc>,
) -> (WorkflowStep, bool) {
    let Some(mcp_source) = mcp_source else {
        return (WorkflowStep::default(), false);
    };
    for server in mcp_source.list_servers() {
        let mut tools = server.tools.clone();
        if tools.is_empty() {
            if let Ok(listed) = mcp_source.list_tools(&server.server_id) {
                tools = listed;
            }
        }
        if tools.is_empty() {
            continue;
        }
        let mut tool_name = tools[0].tool_name.trim().to_string();
        if tool_name.is_empty() {
            tool_name = "lookup".to_string();
        }
        return (
            WorkflowStep {
                workflow_step_id: new_workflow_step_id(),
                title: format!("Use MCP tool {tool_name}"),
                consumer_kind: kura_runtime::ToolCallInvocationKind::McpTool.as_str().to_string(),
                consumer_id: server.server_id.clone(),
                tool_name,
                input: Some(serde_json::json!({ "query": goal })),
                selection_rationale: "Selected the first available MCP tool to satisfy the goal through the existing MCP runtime plane.".to_string(),
                approval_mode_expected: "allow".to_string(),
                max_attempts: 1,
                created_at: now,
                updated_at: now,
                ..WorkflowStep::default()
            },
            true,
        );
    }
    (WorkflowStep::default(), false)
}

fn pick_skill_workflow_step(
    goal: &str,
    skill_source: Option<&dyn SkillPlanningSource>,
    now: DateTime<Utc>,
) -> (WorkflowStep, bool) {
    let Some(skill_source) = skill_source else {
        return (WorkflowStep::default(), false);
    };
    for skill in skill_source.list_skills() {
        if !skill.executable || !skill.available {
            continue;
        }
        let approval = if skill.approval_mode_expected.is_empty() {
            "allow".to_string()
        } else {
            skill.approval_mode_expected.clone()
        };
        return (
            WorkflowStep {
                workflow_step_id: new_workflow_step_id(),
                title: format!("Run executable skill {}", skill.skill_id),
                consumer_kind: kura_runtime::ToolCallInvocationKind::Skill.as_str().to_string(),
                consumer_id: skill.skill_id.clone(),
                tool_name: skill.skill_id.clone(),
                input: Some(serde_json::json!({ "args": vec![goal.to_string()] })),
                selection_rationale: "Selected the first available executable skill to continue the workflow without a new execution boundary.".to_string(),
                approval_mode_expected: approval,
                max_attempts: 2,
                created_at: now,
                updated_at: now,
                ..WorkflowStep::default()
            },
            true,
        );
    }
    (WorkflowStep::default(), false)
}

fn pick_computer_use_workflow_step(goal: &str, now: DateTime<Utc>) -> (WorkflowStep, bool) {
    let normalized = goal.trim().to_lowercase();
    if normalized.is_empty() {
        return (WorkflowStep::default(), false);
    }
    if !normalized.contains("browser")
        && !normalized.contains("computer-use")
        && !normalized.contains("computer use")
    {
        return (WorkflowStep::default(), false);
    }
    (
        WorkflowStep {
            workflow_step_id: new_workflow_step_id(),
            title: "Inspect deterministic browser fixture".to_string(),
            consumer_kind: "computer_use".to_string(),
            consumer_id: "browser".to_string(),
            tool_name: "browser".to_string(),
            input: Some(serde_json::json!({
                "driverKind": "browser",
                "initialUrl": "https://example.test/browser",
                "actions": [
                    { "actionKind": "navigate", "url": "https://example.test/browser" },
                    { "actionKind": "snapshot" }
                ]
            })),
            selection_rationale: "Selected the browser-first computer-use plane to keep browser work on the normal runtime and workflow truth surfaces.".to_string(),
            approval_mode_expected: "allow".to_string(),
            max_attempts: 1,
            created_at: now,
            updated_at: now,
            ..WorkflowStep::default()
        },
        true,
    )
}

fn pick_calendar_workflow_step(action: &kura_calendar::Action, now: DateTime<Utc>) -> WorkflowStep {
    let title = match action.operation_class {
        kura_calendar::OperationClass::ListEvents => "Inspect calendar events",
        kura_calendar::OperationClass::GetEvent => "Inspect calendar event",
        kura_calendar::OperationClass::BusyFree => "Inspect calendar availability",
        kura_calendar::OperationClass::CreateEvent => "Create calendar event",
        kura_calendar::OperationClass::UpdateEvent => "Update calendar event",
        kura_calendar::OperationClass::CancelEvent => "Cancel calendar event",
        kura_calendar::OperationClass::UpdateAttendees => "Run calendar operation",
    };
    let consumer_id = {
        let trimmed = action.integration_id.trim().to_string();
        if trimmed.is_empty() {
            "calendar".to_string()
        } else {
            trimmed
        }
    };
    WorkflowStep {
        workflow_step_id: new_workflow_step_id(),
        title: title.to_string(),
        consumer_kind: "calendar".to_string(),
        consumer_id,
        tool_name: action.operation_class.as_str().to_string(),
        input: serde_json::to_value(action).ok(),
        selection_rationale: "Selected the calendar domain runtime so the workflow can execute calendar work on the normal workflow and delivery truth planes.".to_string(),
        approval_mode_expected: "allow".to_string(),
        max_attempts: 1,
        created_at: now,
        updated_at: now,
        ..WorkflowStep::default()
    }
}

fn pick_mail_workflow_step(action: &kura_mail::Action, now: DateTime<Utc>) -> WorkflowStep {
    let title = match action.operation_class {
        kura_mail::OperationClass::ListThreads => "Inspect mail threads",
        kura_mail::OperationClass::GetThread => "Inspect mail thread",
        kura_mail::OperationClass::GetMessage => "Inspect mail message",
        kura_mail::OperationClass::ListDrafts => "Inspect mail drafts",
        kura_mail::OperationClass::GetDraft => "Inspect mail draft",
        kura_mail::OperationClass::CreateDraft => "Create mail draft",
        kura_mail::OperationClass::UpdateDraft => "Update mail draft",
        kura_mail::OperationClass::SendMessage => "Send mail message",
        kura_mail::OperationClass::SendDraft => "Send mail draft",
        kura_mail::OperationClass::ReplyMessage => "Reply to mail message",
        kura_mail::OperationClass::ForwardMessage => "Forward mail message",
        kura_mail::OperationClass::DownloadAttachment => "Run mail operation",
    };
    let consumer_id = {
        let trimmed = action.integration_id.trim().to_string();
        if trimmed.is_empty() {
            "mail".to_string()
        } else {
            trimmed
        }
    };
    WorkflowStep {
        workflow_step_id: new_workflow_step_id(),
        title: title.to_string(),
        consumer_kind: "mail".to_string(),
        consumer_id,
        tool_name: action.operation_class.as_str().to_string(),
        input: serde_json::to_value(action).ok(),
        selection_rationale: "Selected the mail domain runtime so the workflow can execute mail work on the normal workflow and delivery truth planes.".to_string(),
        approval_mode_expected: "allow".to_string(),
        max_attempts: 1,
        created_at: now,
        updated_at: now,
        ..WorkflowStep::default()
    }
}

fn pick_local_workflow_step(
    cfg: &kura_config::Config,
    goal: &str,
    capability_supervisor: Option<&kura_capabilities::Supervisor>,
    now: DateTime<Utc>,
) -> (WorkflowStep, bool) {
    let Some(supervisor) = capability_supervisor else {
        return (WorkflowStep::default(), false);
    };
    for capability in supervisor.list() {
        if capability.status == kura_capabilities::Status::Failed {
            continue;
        }
        match capability.kind.as_str() {
            "shell" => {
                return (
                    WorkflowStep {
                        workflow_step_id: new_workflow_step_id(),
                        title: "Run local shell capability".to_string(),
                        consumer_kind: kura_runtime::ToolCallInvocationKind::LocalTool.as_str().to_string(),
                        consumer_id: capability.capability_id.clone(),
                        tool_name: "shell".to_string(),
                        input: Some(serde_json::json!({
                            "cmd": format!("printf %s {}", shell_escape(goal)),
                            "cwd": cfg.data_dir
                        })),
                        selection_rationale: "Selected a local shell capability because no better allow-mode executable consumer was available.".to_string(),
                        approval_mode_expected: "ask".to_string(),
                        max_attempts: 1,
                        created_at: now,
                        updated_at: now,
                        ..WorkflowStep::default()
                    },
                    true,
                );
            }
            "exec" => {
                return (
                    WorkflowStep {
                        workflow_step_id: new_workflow_step_id(),
                        title: "Run local exec capability".to_string(),
                        consumer_kind: kura_runtime::ToolCallInvocationKind::LocalTool.as_str().to_string(),
                        consumer_id: capability.capability_id.clone(),
                        tool_name: "exec".to_string(),
                        input: Some(serde_json::json!({
                            "command": "echo",
                            "args": vec![goal.to_string()],
                            "cwd": cfg.data_dir
                        })),
                        selection_rationale: "Selected a local exec capability because no better allow-mode executable consumer was available.".to_string(),
                        approval_mode_expected: "ask".to_string(),
                        max_attempts: 1,
                        created_at: now,
                        updated_at: now,
                        ..WorkflowStep::default()
                    },
                    true,
                );
            }
            _ => {}
        }
    }
    (WorkflowStep::default(), false)
}

#[derive(Default)]
struct ManagerInner {
    by_id: HashMap<String, Workflow>,
    workflow_ids: Vec<String>,
}

/// Thread-safe in-memory store of workflows, mirroring the `kura-runtime`
/// manager pattern: insertion-ordered ids, `parking_lot::RwLock` guard.
pub struct Manager {
    inner: parking_lot::RwLock<ManagerInner>,
}

impl Default for Manager {
    fn default() -> Self {
        Self::new()
    }
}

impl Manager {
    #[must_use]
    pub fn new() -> Self {
        Manager {
            inner: parking_lot::RwLock::new(ManagerInner::default()),
        }
    }

    /// Creates a skeleton workflow in Planning status for the run. The
    /// calendar/mail action inputs are consumed by `plan` instead.
    pub fn create_workflow(
        &self,
        run_id: &str,
        input: CreateWorkflowInput,
    ) -> Result<Workflow, OrchestrationError> {
        let now = Utc::now();
        let workflow = Workflow {
            workflow_id: new_workflow_id(),
            run_id: run_id.trim().to_string(),
            goal: input.goal,
            status: WorkflowStatus::Planning,
            created_at: now,
            updated_at: now,
            ..Workflow::default()
        };
        let mut inner = self.inner.write();
        inner
            .by_id
            .insert(workflow.workflow_id.clone(), workflow.clone());
        inner.workflow_ids.push(workflow.workflow_id.clone());
        Ok(workflow)
    }

    /// Plans a workflow for the run and stores it (Go `Manager.Plan`).
    pub fn plan(
        &self,
        cfg: &kura_config::Config,
        run: &kura_runtime::Run,
        input: &CreateWorkflowInput,
        capability_supervisor: Option<&kura_capabilities::Supervisor>,
        skill_source: Option<&dyn SkillPlanningSource>,
        mcp_source: Option<&dyn MCPPlanningSource>,
    ) -> Workflow {
        let workflow = plan_workflow(
            cfg,
            run,
            input,
            capability_supervisor,
            skill_source,
            mcp_source,
        );
        let mut inner = self.inner.write();
        inner
            .by_id
            .insert(workflow.workflow_id.clone(), workflow.clone());
        inner.workflow_ids.push(workflow.workflow_id.clone());
        workflow
    }

    /// Lists workflows in insertion order.
    #[must_use]
    pub fn list_workflows(&self) -> Vec<Workflow> {
        let inner = self.inner.read();
        inner
            .workflow_ids
            .iter()
            .filter_map(|id| inner.by_id.get(id))
            .cloned()
            .collect()
    }

    /// Returns a workflow by id.
    #[must_use]
    pub fn get_workflow(&self, workflow_id: &str) -> Option<Workflow> {
        self.inner.read().by_id.get(workflow_id).cloned()
    }

    /// Appends a planned step to the workflow, assigning position and ids.
    pub fn add_step(
        &self,
        workflow_id: &str,
        input: AddWorkflowStepInput,
    ) -> Result<WorkflowStep, OrchestrationError> {
        if input.title.trim().is_empty() {
            return Err(OrchestrationError::TitleRequired);
        }
        if input.consumer_kind.trim().is_empty() {
            return Err(OrchestrationError::ConsumerKindRequired);
        }
        if input.consumer_id.trim().is_empty() {
            return Err(OrchestrationError::ConsumerIDRequired);
        }
        let mut inner = self.inner.write();
        let workflow = inner
            .by_id
            .get_mut(workflow_id)
            .ok_or(OrchestrationError::WorkflowNotFound)?;
        let now = Utc::now();
        let position = if input.position > 0 {
            input.position
        } else {
            (workflow.steps.len() + 1) as i64
        };
        let step = WorkflowStep {
            workflow_step_id: new_workflow_step_id(),
            workflow_id: workflow_id.to_string(),
            title: input.title,
            position,
            consumer_kind: input.consumer_kind,
            consumer_id: input.consumer_id,
            tool_name: input.tool_name,
            input: input.input,
            status: StepStatus::Planned,
            approval_mode_expected: input.approval_mode_expected,
            attempt_count: 0,
            max_attempts: if input.max_attempts > 0 {
                input.max_attempts
            } else {
                1
            },
            created_at: now,
            updated_at: now,
            ..WorkflowStep::default()
        };
        workflow.steps.push(step.clone());
        workflow.updated_at = now;
        Ok(step)
    }

    /// Appends a dependency and wires its id into the target step's
    /// `dependency_ids`.
    pub fn add_dependency(
        &self,
        workflow_id: &str,
        input: AddDependencyInput,
    ) -> Result<Dependency, OrchestrationError> {
        let mut inner = self.inner.write();
        let workflow = inner
            .by_id
            .get_mut(workflow_id)
            .ok_or(OrchestrationError::WorkflowNotFound)?;
        if input.from_workflow_step_id.trim().is_empty()
            || input.to_workflow_step_id.trim().is_empty()
        {
            return Err(OrchestrationError::StepIDRequired);
        }
        if workflow_step_by_id(workflow, &input.from_workflow_step_id).is_none()
            || workflow_step_by_id(workflow, &input.to_workflow_step_id).is_none()
        {
            return Err(OrchestrationError::StepNotFound);
        }
        let dependency = Dependency {
            dependency_id: new_workflow_dependency_id(),
            workflow_id: workflow_id.to_string(),
            from_workflow_step_id: input.from_workflow_step_id,
            to_workflow_step_id: input.to_workflow_step_id,
            dependency_type: input.dependency_type,
            reason: input.reason,
        };
        if let Some(step) = workflow
            .steps
            .iter_mut()
            .find(|step| step.workflow_step_id == dependency.to_workflow_step_id)
        {
            step.dependency_ids.push(dependency.dependency_id.clone());
        }
        workflow.dependencies.push(dependency.clone());
        workflow.updated_at = Utc::now();
        Ok(dependency)
    }

    /// Appends a pending handoff between two steps.
    pub fn add_handoff(
        &self,
        workflow_id: &str,
        input: AddHandoffInput,
    ) -> Result<Handoff, OrchestrationError> {
        if input.from_workflow_step_id.trim().is_empty()
            || input.to_workflow_step_id.trim().is_empty()
        {
            return Err(OrchestrationError::StepIDRequired);
        }
        let mut inner = self.inner.write();
        let workflow = inner
            .by_id
            .get_mut(workflow_id)
            .ok_or(OrchestrationError::WorkflowNotFound)?;
        if workflow_step_by_id(workflow, &input.from_workflow_step_id).is_none()
            || workflow_step_by_id(workflow, &input.to_workflow_step_id).is_none()
        {
            return Err(OrchestrationError::StepNotFound);
        }
        let handoff = Handoff {
            handoff_id: new_workflow_handoff_id(),
            workflow_id: workflow_id.to_string(),
            from_workflow_step_id: input.from_workflow_step_id,
            to_workflow_step_id: input.to_workflow_step_id,
            status: HandoffStatus::Pending,
            payload_summary: input.payload_summary,
            source_path: input.source_path,
            ..Handoff::default()
        };
        workflow.handoffs.push(handoff.clone());
        workflow.updated_at = Utc::now();
        Ok(handoff)
    }

    /// Go `InitializeExecution` applied to the stored workflow.
    pub fn initialize_execution(
        &self,
        workflow_id: &str,
        now: DateTime<Utc>,
    ) -> Result<Workflow, OrchestrationError> {
        let mut inner = self.inner.write();
        let workflow = inner
            .by_id
            .get(workflow_id)
            .cloned()
            .ok_or(OrchestrationError::WorkflowNotFound)?;
        let updated = initialize_execution(workflow, now);
        inner.by_id.insert(workflow_id.to_string(), updated.clone());
        Ok(updated)
    }

    /// Go `AdvanceReadySteps` applied to the stored workflow.
    pub fn advance_ready_steps(
        &self,
        workflow_id: &str,
        now: DateTime<Utc>,
    ) -> Result<(Workflow, bool), OrchestrationError> {
        let mut inner = self.inner.write();
        let workflow = inner
            .by_id
            .get(workflow_id)
            .cloned()
            .ok_or(OrchestrationError::WorkflowNotFound)?;
        let (updated, changed) = advance_ready_steps(workflow, now);
        inner.by_id.insert(workflow_id.to_string(), updated.clone());
        Ok((updated, changed))
    }

    /// Go `StartStepAttempt` applied to the stored workflow.
    pub fn start_step_attempt(
        &self,
        workflow_id: &str,
        workflow_step_id: &str,
        runtime_step_id: &str,
        now: DateTime<Utc>,
    ) -> Result<Workflow, OrchestrationError> {
        let mut inner = self.inner.write();
        let workflow = inner
            .by_id
            .get(workflow_id)
            .cloned()
            .ok_or(OrchestrationError::WorkflowNotFound)?;
        let updated = start_step_attempt(workflow, workflow_step_id, runtime_step_id, now);
        inner.by_id.insert(workflow_id.to_string(), updated.clone());
        Ok(updated)
    }

    /// Go `BindToolCall` applied to the stored workflow.
    pub fn bind_tool_call(
        &self,
        workflow_id: &str,
        workflow_step_id: &str,
        tool_call: &kura_runtime::ToolCall,
        now: DateTime<Utc>,
    ) -> Result<Workflow, OrchestrationError> {
        let mut inner = self.inner.write();
        let workflow = inner
            .by_id
            .get(workflow_id)
            .cloned()
            .ok_or(OrchestrationError::WorkflowNotFound)?;
        let updated = bind_tool_call(workflow, workflow_step_id, tool_call, now);
        inner.by_id.insert(workflow_id.to_string(), updated.clone());
        Ok(updated)
    }

    /// Go `ApplyToolCallResult` applied to the stored workflow.
    pub fn apply_tool_call_result(
        &self,
        workflow_id: &str,
        tool_call: &kura_runtime::ToolCall,
        hinted_status: Option<StepStatus>,
        blocked_reason: &str,
        now: DateTime<Utc>,
    ) -> Result<Workflow, OrchestrationError> {
        let mut inner = self.inner.write();
        let workflow = inner
            .by_id
            .get(workflow_id)
            .cloned()
            .ok_or(OrchestrationError::WorkflowNotFound)?;
        let updated =
            apply_tool_call_result(workflow, tool_call, hinted_status, blocked_reason, now);
        inner.by_id.insert(workflow_id.to_string(), updated.clone());
        Ok(updated)
    }

    /// Go `ReconcileStatus` applied to the stored workflow.
    pub fn reconcile_status(
        &self,
        workflow_id: &str,
        now: DateTime<Utc>,
    ) -> Result<Workflow, OrchestrationError> {
        let mut inner = self.inner.write();
        let workflow = inner
            .by_id
            .get(workflow_id)
            .cloned()
            .ok_or(OrchestrationError::WorkflowNotFound)?;
        let updated = reconcile_status(workflow, now);
        inner.by_id.insert(workflow_id.to_string(), updated.clone());
        Ok(updated)
    }

    /// Go `ApplyComputerUseProjection` applied to the stored workflow.
    pub fn apply_computer_use_projection(
        &self,
        workflow_id: &str,
        workflow_step_id: &str,
        session_id: &str,
        actions: &[String],
        artifacts: &[kura_computeruse::Artifact],
        now: DateTime<Utc>,
    ) -> Result<Workflow, OrchestrationError> {
        let mut inner = self.inner.write();
        let workflow = inner
            .by_id
            .get(workflow_id)
            .cloned()
            .ok_or(OrchestrationError::WorkflowNotFound)?;
        let updated = apply_computer_use_projection(
            workflow,
            workflow_step_id,
            session_id,
            actions,
            artifacts,
            now,
        );
        inner.by_id.insert(workflow_id.to_string(), updated.clone());
        Ok(updated)
    }
}
