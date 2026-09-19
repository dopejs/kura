//! Port of `daemon/internal/runtime`: the run/step/tool-call lifecycle ledger with status
//! transitions, checkpoint snapshot/restore, and the live-validation matrix.

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

string_enum!(RunStatus {
    Queued => "queued",
    Running => "running",
    WaitingInput => "waiting_input",
    Blocked => "blocked",
    Completed => "completed",
    Failed => "failed",
    Cancelled => "cancelled",
});

string_enum!(StepStatus {
    Queued => "queued",
    Planning => "planning",
    CallingModel => "calling_model",
    ExecutingTool => "executing_tool",
    WaitingInput => "waiting_input",
    Blocked => "blocked",
    Completed => "completed",
    Failed => "failed",
    Cancelled => "cancelled",
});

string_enum!(ToolCallStatus {
    Requested => "requested",
    Running => "running",
    Completed => "completed",
    Failed => "failed",
    Cancelled => "cancelled",
    Denied => "denied",
});

string_enum!(ToolCallInvocationKind {
    LocalTool => "local_tool",
    Skill => "skill",
    McpTool => "mcp_tool",
    DomainTool => "domain_tool",
});

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum RuntimeError {
    #[error("entrypoint is required")]
    EntrypointRequired,
    #[error("run already exists")]
    RunAlreadyExists,
    #[error("run not found")]
    RunNotFound,
    #[error("title is required")]
    TitleRequired,
    #[error("step not found")]
    StepNotFound,
    #[error("invalid step transition")]
    InvalidStepTransition,
    #[error("run is in a terminal state")]
    RunTerminal,
    #[error("step is in a terminal state")]
    StepTerminal,
    #[error("tool name is required")]
    ToolNameRequired,
    #[error("capability id or skill id is required")]
    ToolTargetRequired,
    #[error("tool call not found")]
    ToolCallNotFound,
    #[error("invalid tool call status transition")]
    InvalidToolCallStatus,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub run_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub schedule_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub schedule_attempt_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reminder_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reminder_occurrence_id: String,
    pub entrypoint: String,
    pub status: RunStatus,
    pub goal: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub active_workflow_id: String,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub workflow_count: i64,
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
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRunInput {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub run_id: String,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub schedule_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub schedule_attempt_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reminder_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reminder_occurrence_id: String,
    pub entrypoint: String,
    pub goal: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    pub step_id: String,
    pub run_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workflow_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workflow_step_id: String,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub attempt: i64,
    pub title: String,
    pub kind: String,
    pub status: StepStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateStepInput {
    pub title: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workflow_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workflow_step_id: String,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub attempt: i64,
    #[serde(default)]
    pub input: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStepStatusInput {
    pub status: StepStatus,
    #[serde(default)]
    pub output: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub tool_call_id: String,
    pub run_id: String,
    pub step_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workflow_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workflow_step_id: String,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub attempt: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub computer_use_session_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub computer_use_action_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub invocation_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub domain_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub capability_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub skill_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mcp_server_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mcp_server_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mcp_tool_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mcp_transport_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mcp_session_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub authorization_result: String,
    pub tool_name: String,
    pub status: ToolCallStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sandbox_execution_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub failure_class: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub integration_bindings: Vec<kura_integrations::BindingSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub calendar_operation_summaries: Vec<kura_calendar::OperationSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mail_operation_summaries: Vec<kura_mail::OperationSummary>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub sandbox: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunCheckpoint {
    pub run: Run,
    pub steps: Vec<Step>,
    pub tool_calls: Vec<ToolCall>,
    pub captured_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateToolCallInput {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool_call_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workflow_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workflow_step_id: String,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub attempt: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub computer_use_session_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub computer_use_action_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub invocation_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub domain_kind: String,
    pub capability_id: String,
    pub skill_id: String,
    pub mcp_server_id: String,
    pub mcp_server_name: String,
    pub mcp_tool_name: String,
    pub mcp_transport_kind: String,
    pub mcp_session_id: String,
    pub authorization_result: String,
    pub tool_name: String,
    #[serde(default)]
    pub input: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sandbox_execution_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub failure_class: String,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub sandbox: serde_json::Map<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub integration_bindings: Vec<kura_integrations::BindingSummary>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompleteToolCallInput {
    #[serde(default)]
    pub output: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sandbox_execution_id: String,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub sandbox: serde_json::Map<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub integration_bindings: Vec<kura_integrations::BindingSummary>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FailToolCallInput {
    #[serde(default)]
    pub output: Option<serde_json::Value>,
    pub error: String,
    pub failure_class: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sandbox_execution_id: String,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub sandbox: serde_json::Map<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub integration_bindings: Vec<kura_integrations::BindingSummary>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DenyToolCallInput {
    #[serde(default)]
    pub output: Option<serde_json::Value>,
    pub error: String,
    pub failure_class: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sandbox_execution_id: String,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub sandbox: serde_json::Map<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub integration_bindings: Vec<kura_integrations::BindingSummary>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelToolCallInput {
    #[serde(default)]
    pub output: Option<serde_json::Value>,
    pub error: String,
    pub failure_class: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sandbox_execution_id: String,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub sandbox: serde_json::Map<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub integration_bindings: Vec<kura_integrations::BindingSummary>,
}

#[must_use]
fn is_zero_i64(v: &i64) -> bool {
    *v == 0
}

#[derive(Default)]
struct ManagerInner {
    by_id: HashMap<String, Run>,
    run_ids: Vec<String>,
    steps_by_id: HashMap<String, Step>,
    steps_by_run: HashMap<String, Vec<String>>,
    tool_calls_by_id: HashMap<String, ToolCall>,
    tool_calls_by_step: HashMap<String, Vec<String>>,
}

pub struct Manager {
    inner: parking_lot::RwLock<ManagerInner>,
}

impl Default for Manager {
    fn default() -> Self {
        Self::new()
    }
}

impl Manager {
    pub fn new() -> Self {
        Manager {
            inner: parking_lot::RwLock::new(ManagerInner::default()),
        }
    }

    pub fn create_run(&self, input: CreateRunInput) -> Result<Run, RuntimeError> {
        if input.entrypoint.is_empty() {
            return Err(RuntimeError::EntrypointRequired);
        }
        let now = Utc::now();
        let mut run_id = input.run_id.trim().to_string();
        if run_id.is_empty() {
            run_id = new_run_id();
        }
        let run = Run {
            run_id: run_id.clone(),
            session_id: input.session_id.clone(),
            schedule_id: input.schedule_id.clone(),
            schedule_attempt_id: input.schedule_attempt_id.clone(),
            reminder_id: input.reminder_id.clone(),
            reminder_occurrence_id: input.reminder_occurrence_id.clone(),
            entrypoint: input.entrypoint,
            status: RunStatus::Queued,
            goal: input.goal,
            created_at: now,
            updated_at: now,
            ..Run::default()
        };
        let mut inner = self.inner.write();
        if inner.by_id.contains_key(&run.run_id) {
            return Err(RuntimeError::RunAlreadyExists);
        }
        inner.by_id.insert(run_id.clone(), run.clone());
        inner.run_ids.push(run_id);
        Ok(run)
    }

    pub fn snapshot_run(&self, run_id: &str) -> Result<RunCheckpoint, RuntimeError> {
        let inner = self.inner.read();
        let run = inner
            .by_id
            .get(run_id)
            .cloned()
            .ok_or(RuntimeError::RunNotFound)?;
        let mut steps = Vec::new();
        let mut tool_calls = Vec::new();
        if let Some(step_ids) = inner.steps_by_run.get(run_id) {
            for step_id in step_ids {
                if let Some(step) = inner.steps_by_id.get(step_id) {
                    steps.push(step.clone());
                }
                if let Some(tc_ids) = inner.tool_calls_by_step.get(step_id) {
                    for tc_id in tc_ids {
                        if let Some(tc) = inner.tool_calls_by_id.get(tc_id) {
                            tool_calls.push(tc.clone());
                        }
                    }
                }
            }
        }
        Ok(RunCheckpoint {
            run,
            steps,
            tool_calls,
            captured_at: Utc::now(),
        })
    }

    pub fn list_runs(&self) -> Vec<Run> {
        let inner = self.inner.read();
        inner
            .run_ids
            .iter()
            .filter_map(|id| inner.by_id.get(id).cloned())
            .collect()
    }

    pub fn get_run(&self, run_id: &str) -> Option<Run> {
        self.inner.read().by_id.get(run_id).cloned()
    }

    pub fn create_step(&self, run_id: &str, input: CreateStepInput) -> Result<Step, RuntimeError> {
        if input.title.is_empty() {
            return Err(RuntimeError::TitleRequired);
        }
        let mut inner = self.inner.write();
        if !inner.by_id.contains_key(run_id) {
            return Err(RuntimeError::RunNotFound);
        }
        let now = Utc::now();
        let kind = if input.kind.is_empty() {
            "task".to_string()
        } else {
            input.kind.clone()
        };
        let step = Step {
            step_id: new_step_id(),
            run_id: run_id.to_string(),
            workflow_id: input.workflow_id.trim().to_string(),
            workflow_step_id: input.workflow_step_id.trim().to_string(),
            attempt: input.attempt,
            title: input.title,
            kind,
            status: StepStatus::Queued,
            created_at: now,
            updated_at: now,
            input: input.input,
            ..Step::default()
        };
        inner.steps_by_id.insert(step.step_id.clone(), step.clone());
        inner
            .steps_by_run
            .entry(run_id.to_string())
            .or_default()
            .push(step.step_id.clone());
        Ok(step)
    }

    pub fn list_steps(&self, run_id: &str) -> Result<Vec<Step>, RuntimeError> {
        let inner = self.inner.read();
        if !inner.by_id.contains_key(run_id) {
            return Err(RuntimeError::RunNotFound);
        }
        Ok(inner
            .steps_by_run
            .get(run_id)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| inner.steps_by_id.get(id).cloned())
                    .collect()
            })
            .unwrap_or_default())
    }

    pub fn get_step(&self, run_id: &str, step_id: &str) -> Option<Step> {
        let inner = self.inner.read();
        match inner.steps_by_id.get(step_id) {
            Some(step) if step.run_id == run_id => Some(step.clone()),
            _ => None,
        }
    }

    pub fn update_step_status(
        &self,
        run_id: &str,
        step_id: &str,
        input: UpdateStepStatusInput,
    ) -> Result<Step, RuntimeError> {
        let mut inner = self.inner.write();
        if !inner.by_id.contains_key(run_id) {
            return Err(RuntimeError::RunNotFound);
        }
        let step = inner.steps_by_id.get(step_id).cloned();
        let Some(mut step) = step else {
            return Err(RuntimeError::StepNotFound);
        };
        if step.run_id != run_id {
            return Err(RuntimeError::StepNotFound);
        }
        if !can_transition(step.status, input.status) {
            return Err(RuntimeError::InvalidStepTransition);
        }
        step.status = input.status;
        step.updated_at = Utc::now();
        if let Some(output) = input.output {
            step.output = Some(output);
        }
        inner.steps_by_id.insert(step_id.to_string(), step.clone());
        Ok(step)
    }

    pub fn update_step_status_and_reconcile_run(
        &self,
        run_id: &str,
        step_id: &str,
        input: UpdateStepStatusInput,
    ) -> Result<(Step, Option<Run>), RuntimeError> {
        let mut inner = self.inner.write();
        let run = inner
            .by_id
            .get(run_id)
            .cloned()
            .ok_or(RuntimeError::RunNotFound)?;
        let step = inner.steps_by_id.get(step_id).cloned();
        let Some(mut step) = step else {
            return Err(RuntimeError::StepNotFound);
        };
        if step.run_id != run_id {
            return Err(RuntimeError::StepNotFound);
        }
        if !can_transition(step.status, input.status) {
            if step.status == input.status {
                return Ok((step, None));
            }
            if is_step_terminal(step.status) {
                return Err(RuntimeError::StepTerminal);
            }
            if is_run_terminal(run.status) {
                return Err(RuntimeError::RunTerminal);
            }
            return Err(RuntimeError::InvalidStepTransition);
        }
        step.status = input.status;
        step.updated_at = Utc::now();
        if let Some(output) = input.output {
            step.output = Some(output);
        }
        inner.steps_by_id.insert(step_id.to_string(), step.clone());

        let next_run_status = derive_run_status_locked(&inner, run_id);
        if next_run_status == run.status {
            return Ok((step, None));
        }
        let mut run = run;
        run.status = next_run_status;
        run.updated_at = Utc::now();
        inner.by_id.insert(run_id.to_string(), run.clone());
        Ok((step, Some(run)))
    }

    pub fn cancel_run(&self, run_id: &str) -> Result<(Run, Vec<Step>, bool), RuntimeError> {
        let mut inner = self.inner.write();
        let run = inner
            .by_id
            .get(run_id)
            .cloned()
            .ok_or(RuntimeError::RunNotFound)?;
        if run.status == RunStatus::Cancelled {
            return Ok((run, Vec::new(), true));
        }
        if is_run_terminal(run.status) {
            return Err(RuntimeError::RunTerminal);
        }
        let now = Utc::now();
        let mut updated_steps = Vec::new();
        if let Some(step_ids) = inner.steps_by_run.get(run_id).cloned() {
            for step_id in step_ids {
                if let Some(mut step) = inner.steps_by_id.get(&step_id).cloned() {
                    if is_step_terminal(step.status) {
                        continue;
                    }
                    step.status = StepStatus::Cancelled;
                    step.updated_at = now;
                    inner.steps_by_id.insert(step_id, step.clone());
                    updated_steps.push(step);
                }
            }
        }
        let mut run = run;
        run.status = RunStatus::Cancelled;
        run.updated_at = now;
        inner.by_id.insert(run_id.to_string(), run.clone());
        Ok((run, updated_steps, false))
    }

    pub fn resume_run(&self, run_id: &str) -> Result<(Run, Vec<Step>, bool), RuntimeError> {
        let mut inner = self.inner.write();
        let run = inner
            .by_id
            .get(run_id)
            .cloned()
            .ok_or(RuntimeError::RunNotFound)?;
        if run.status != RunStatus::Cancelled {
            if is_run_terminal(run.status) {
                return Err(RuntimeError::RunTerminal);
            }
            return Ok((run, Vec::new(), true));
        }
        let now = Utc::now();
        let mut updated_steps = Vec::new();
        if let Some(step_ids) = inner.steps_by_run.get(run_id).cloned() {
            for step_id in step_ids {
                if let Some(mut step) = inner.steps_by_id.get(&step_id).cloned() {
                    if step.status != StepStatus::Cancelled {
                        continue;
                    }
                    step.status = StepStatus::Queued;
                    step.updated_at = now;
                    inner.steps_by_id.insert(step_id, step.clone());
                    updated_steps.push(step);
                }
            }
        }
        let next_run_status = derive_run_status_locked(&inner, run_id);
        let mut run = run;
        run.status = next_run_status;
        run.updated_at = now;
        inner.by_id.insert(run_id.to_string(), run.clone());
        Ok((run, updated_steps, false))
    }

    pub fn cancel_step(
        &self,
        run_id: &str,
        step_id: &str,
    ) -> Result<(Step, Option<Run>, bool), RuntimeError> {
        let mut inner = self.inner.write();
        let run = inner
            .by_id
            .get(run_id)
            .cloned()
            .ok_or(RuntimeError::RunNotFound)?;
        let step = inner.steps_by_id.get(step_id).cloned();
        let Some(mut step) = step else {
            return Err(RuntimeError::StepNotFound);
        };
        if step.run_id != run_id {
            return Err(RuntimeError::StepNotFound);
        }
        if step.status == StepStatus::Cancelled {
            return Ok((step, None, true));
        }
        if is_step_terminal(step.status) {
            return Err(RuntimeError::StepTerminal);
        }
        if is_run_terminal(run.status) && run.status != RunStatus::Cancelled {
            return Err(RuntimeError::RunTerminal);
        }
        let now = Utc::now();
        step.status = StepStatus::Cancelled;
        step.updated_at = now;
        inner.steps_by_id.insert(step_id.to_string(), step.clone());

        let next_run_status = derive_run_status_locked(&inner, run_id);
        if next_run_status == run.status {
            return Ok((step, None, false));
        }
        let mut run = run;
        run.status = next_run_status;
        run.updated_at = now;
        inner.by_id.insert(run_id.to_string(), run.clone());
        Ok((step, Some(run), false))
    }

    pub fn create_tool_call(
        &self,
        run_id: &str,
        step_id: &str,
        input: CreateToolCallInput,
    ) -> Result<ToolCall, RuntimeError> {
        if input.tool_name.is_empty() {
            return Err(RuntimeError::ToolNameRequired);
        }
        if input.capability_id.trim().is_empty()
            && input.skill_id.trim().is_empty()
            && input.mcp_server_id.trim().is_empty()
            && input.domain_kind.trim().is_empty()
        {
            return Err(RuntimeError::ToolTargetRequired);
        }
        let mut inner = self.inner.write();
        if !inner.by_id.contains_key(run_id) {
            return Err(RuntimeError::RunNotFound);
        }
        let step = inner.steps_by_id.get(step_id).cloned();
        let Some(step) = step else {
            return Err(RuntimeError::StepNotFound);
        };
        if step.run_id != run_id {
            return Err(RuntimeError::StepNotFound);
        }
        let now = Utc::now();
        let mut tool_call_id = input.tool_call_id.trim().to_string();
        if tool_call_id.is_empty() {
            tool_call_id = new_tool_call_id();
        }
        let invocation_kind = if !input.invocation_kind.trim().is_empty() {
            input.invocation_kind.trim().to_string()
        } else if !input.skill_id.trim().is_empty() {
            ToolCallInvocationKind::Skill.as_str().to_string()
        } else if !input.mcp_server_id.trim().is_empty() {
            ToolCallInvocationKind::McpTool.as_str().to_string()
        } else {
            ToolCallInvocationKind::LocalTool.as_str().to_string()
        };
        let tool_call = ToolCall {
            tool_call_id: tool_call_id.clone(),
            run_id: run_id.to_string(),
            step_id: step_id.to_string(),
            workflow_id: input.workflow_id.trim().to_string(),
            workflow_step_id: input.workflow_step_id.trim().to_string(),
            attempt: input.attempt,
            computer_use_session_id: input.computer_use_session_id.trim().to_string(),
            computer_use_action_id: input.computer_use_action_id.trim().to_string(),
            invocation_kind,
            domain_kind: input.domain_kind.trim().to_string(),
            capability_id: input.capability_id.trim().to_string(),
            skill_id: input.skill_id.trim().to_string(),
            mcp_server_id: input.mcp_server_id.trim().to_string(),
            mcp_server_name: input.mcp_server_name.trim().to_string(),
            mcp_tool_name: input.mcp_tool_name.trim().to_string(),
            mcp_transport_kind: input.mcp_transport_kind.trim().to_string(),
            mcp_session_id: input.mcp_session_id.trim().to_string(),
            authorization_result: input.authorization_result.trim().to_string(),
            tool_name: input.tool_name,
            status: ToolCallStatus::Requested,
            sandbox_execution_id: input.sandbox_execution_id.trim().to_string(),
            failure_class: input.failure_class.trim().to_string(),
            integration_bindings: input.integration_bindings.clone(),
            created_at: now,
            updated_at: now,
            input: input.input,
            sandbox: input.sandbox.clone(),
            ..ToolCall::default()
        };
        inner
            .tool_calls_by_id
            .insert(tool_call_id.clone(), tool_call.clone());
        inner
            .tool_calls_by_step
            .entry(step_id.to_string())
            .or_default()
            .push(tool_call_id);
        Ok(tool_call)
    }

    pub fn list_tool_calls(
        &self,
        run_id: &str,
        step_id: &str,
    ) -> Result<Vec<ToolCall>, RuntimeError> {
        let inner = self.inner.read();
        if !inner.by_id.contains_key(run_id) {
            return Err(RuntimeError::RunNotFound);
        }
        let step = inner.steps_by_id.get(step_id);
        let Some(step) = step else {
            return Err(RuntimeError::StepNotFound);
        };
        if step.run_id != run_id {
            return Err(RuntimeError::StepNotFound);
        }
        Ok(inner
            .tool_calls_by_step
            .get(step_id)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| inner.tool_calls_by_id.get(id).cloned())
                    .collect()
            })
            .unwrap_or_default())
    }

    pub fn get_tool_call(
        &self,
        run_id: &str,
        step_id: &str,
        tool_call_id: &str,
    ) -> Option<ToolCall> {
        let inner = self.inner.read();
        match inner.tool_calls_by_id.get(tool_call_id) {
            Some(tc) if tc.run_id == run_id && tc.step_id == step_id => Some(tc.clone()),
            _ => None,
        }
    }

    pub fn complete_tool_call(
        &self,
        run_id: &str,
        step_id: &str,
        tool_call_id: &str,
        input: CompleteToolCallInput,
    ) -> Result<ToolCall, RuntimeError> {
        let mut inner = self.inner.write();
        let mut tool_call = require_mutable_tool_call(&inner, run_id, step_id, tool_call_id)?;
        if tool_call.status != ToolCallStatus::Requested
            && tool_call.status != ToolCallStatus::Running
        {
            return Err(RuntimeError::InvalidToolCallStatus);
        }
        tool_call.status = ToolCallStatus::Completed;
        tool_call.updated_at = Utc::now();
        tool_call.output = input.output;
        if !input.sandbox_execution_id.trim().is_empty() {
            tool_call.sandbox_execution_id = input.sandbox_execution_id.trim().to_string();
        }
        if !input.integration_bindings.is_empty() {
            tool_call.integration_bindings = input.integration_bindings.clone();
        }
        if !input.sandbox.is_empty() {
            tool_call.sandbox = input.sandbox.clone();
        }
        inner
            .tool_calls_by_id
            .insert(tool_call_id.to_string(), tool_call.clone());
        Ok(tool_call)
    }

    pub fn fail_tool_call(
        &self,
        run_id: &str,
        step_id: &str,
        tool_call_id: &str,
        input: FailToolCallInput,
    ) -> Result<ToolCall, RuntimeError> {
        let mut inner = self.inner.write();
        let mut tool_call = require_mutable_tool_call(&inner, run_id, step_id, tool_call_id)?;
        if tool_call.status != ToolCallStatus::Requested
            && tool_call.status != ToolCallStatus::Running
        {
            return Err(RuntimeError::InvalidToolCallStatus);
        }
        tool_call.status = ToolCallStatus::Failed;
        tool_call.updated_at = Utc::now();
        tool_call.output = input.output;
        tool_call.error = input.error;
        tool_call.failure_class = input.failure_class.trim().to_string();
        if !input.sandbox_execution_id.trim().is_empty() {
            tool_call.sandbox_execution_id = input.sandbox_execution_id.trim().to_string();
        }
        if !input.integration_bindings.is_empty() {
            tool_call.integration_bindings = input.integration_bindings.clone();
        }
        if !input.sandbox.is_empty() {
            tool_call.sandbox = input.sandbox.clone();
        }
        inner
            .tool_calls_by_id
            .insert(tool_call_id.to_string(), tool_call.clone());
        Ok(tool_call)
    }

    pub fn deny_tool_call(
        &self,
        run_id: &str,
        step_id: &str,
        tool_call_id: &str,
        input: DenyToolCallInput,
    ) -> Result<ToolCall, RuntimeError> {
        let mut inner = self.inner.write();
        let mut tool_call = require_mutable_tool_call(&inner, run_id, step_id, tool_call_id)?;
        if tool_call.status != ToolCallStatus::Requested
            && tool_call.status != ToolCallStatus::Running
        {
            return Err(RuntimeError::InvalidToolCallStatus);
        }
        tool_call.status = ToolCallStatus::Denied;
        tool_call.updated_at = Utc::now();
        tool_call.output = input.output;
        tool_call.error = input.error;
        tool_call.failure_class = input.failure_class.trim().to_string();
        if !input.sandbox_execution_id.trim().is_empty() {
            tool_call.sandbox_execution_id = input.sandbox_execution_id.trim().to_string();
        }
        if !input.integration_bindings.is_empty() {
            tool_call.integration_bindings = input.integration_bindings.clone();
        }
        if !input.sandbox.is_empty() {
            tool_call.sandbox = input.sandbox.clone();
        }
        inner
            .tool_calls_by_id
            .insert(tool_call_id.to_string(), tool_call.clone());
        Ok(tool_call)
    }

    pub fn cancel_tool_call(
        &self,
        run_id: &str,
        step_id: &str,
        tool_call_id: &str,
        input: CancelToolCallInput,
    ) -> Result<ToolCall, RuntimeError> {
        let mut inner = self.inner.write();
        let mut tool_call = require_mutable_tool_call(&inner, run_id, step_id, tool_call_id)?;
        if tool_call.status != ToolCallStatus::Requested
            && tool_call.status != ToolCallStatus::Running
        {
            return Err(RuntimeError::InvalidToolCallStatus);
        }
        tool_call.status = ToolCallStatus::Cancelled;
        tool_call.updated_at = Utc::now();
        tool_call.output = input.output;
        tool_call.error = input.error;
        tool_call.failure_class = input.failure_class.trim().to_string();
        if !input.sandbox_execution_id.trim().is_empty() {
            tool_call.sandbox_execution_id = input.sandbox_execution_id.trim().to_string();
        }
        if !input.integration_bindings.is_empty() {
            tool_call.integration_bindings = input.integration_bindings.clone();
        }
        if !input.sandbox.is_empty() {
            tool_call.sandbox = input.sandbox.clone();
        }
        inner
            .tool_calls_by_id
            .insert(tool_call_id.to_string(), tool_call.clone());
        Ok(tool_call)
    }

    pub fn mark_tool_call_running(
        &self,
        run_id: &str,
        step_id: &str,
        tool_call_id: &str,
        sandbox_execution_id: &str,
        sandbox_view: serde_json::Map<String, serde_json::Value>,
    ) -> Result<ToolCall, RuntimeError> {
        let mut inner = self.inner.write();
        let mut tool_call = require_mutable_tool_call(&inner, run_id, step_id, tool_call_id)?;
        if tool_call.status != ToolCallStatus::Requested {
            return Err(RuntimeError::InvalidToolCallStatus);
        }
        tool_call.status = ToolCallStatus::Running;
        tool_call.updated_at = Utc::now();
        if !sandbox_execution_id.trim().is_empty() {
            tool_call.sandbox_execution_id = sandbox_execution_id.trim().to_string();
        }
        if !sandbox_view.is_empty() {
            tool_call.sandbox = sandbox_view.clone();
        }
        inner
            .tool_calls_by_id
            .insert(tool_call_id.to_string(), tool_call.clone());
        Ok(tool_call)
    }

    pub fn restore_checkpoints(&self, checkpoints: Vec<RunCheckpoint>) {
        let mut inner = self.inner.write();
        inner.by_id.clear();
        inner.run_ids.clear();
        inner.steps_by_id.clear();
        inner.steps_by_run.clear();
        inner.tool_calls_by_id.clear();
        inner.tool_calls_by_step.clear();
        for checkpoint in checkpoints {
            let run = checkpoint.run;
            inner.by_id.insert(run.run_id.clone(), run.clone());
            inner.run_ids.push(run.run_id.clone());
            for step in checkpoint.steps {
                inner.steps_by_id.insert(step.step_id.clone(), step.clone());
                inner
                    .steps_by_run
                    .entry(run.run_id.clone())
                    .or_default()
                    .push(step.step_id);
            }
            for tool_call in checkpoint.tool_calls {
                inner
                    .tool_calls_by_id
                    .insert(tool_call.tool_call_id.clone(), tool_call.clone());
                inner
                    .tool_calls_by_step
                    .entry(tool_call.step_id.clone())
                    .or_default()
                    .push(tool_call.tool_call_id);
            }
        }
    }

    pub fn restore_run_checkpoint(&self, checkpoint: RunCheckpoint) {
        let mut inner = self.inner.write();
        let run = checkpoint.run;
        if !inner.by_id.contains_key(&run.run_id) {
            inner.run_ids.push(run.run_id.clone());
        }
        inner.by_id.insert(run.run_id.clone(), run.clone());
        if let Some(existing) = inner.steps_by_run.remove(&run.run_id) {
            for step_id in existing {
                inner.steps_by_id.remove(&step_id);
                inner.tool_calls_by_step.remove(&step_id);
            }
        }
        let stale: Vec<String> = inner
            .tool_calls_by_id
            .iter()
            .filter(|(_, tc)| tc.run_id == run.run_id)
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            inner.tool_calls_by_id.remove(&id);
        }
        for step in checkpoint.steps {
            inner.steps_by_id.insert(step.step_id.clone(), step.clone());
            inner
                .steps_by_run
                .entry(run.run_id.clone())
                .or_default()
                .push(step.step_id);
        }
        for tool_call in checkpoint.tool_calls {
            inner
                .tool_calls_by_id
                .insert(tool_call.tool_call_id.clone(), tool_call.clone());
            inner
                .tool_calls_by_step
                .entry(tool_call.step_id.clone())
                .or_default()
                .push(tool_call.tool_call_id);
        }
    }
}

fn require_mutable_tool_call(
    inner: &ManagerInner,
    run_id: &str,
    step_id: &str,
    tool_call_id: &str,
) -> Result<ToolCall, RuntimeError> {
    if !inner.by_id.contains_key(run_id) {
        return Err(RuntimeError::RunNotFound);
    }
    let step = inner.steps_by_id.get(step_id);
    let Some(step) = step else {
        return Err(RuntimeError::StepNotFound);
    };
    if step.run_id != run_id {
        return Err(RuntimeError::StepNotFound);
    }
    match inner.tool_calls_by_id.get(tool_call_id) {
        Some(tc) if tc.run_id == run_id && tc.step_id == step_id => Ok(tc.clone()),
        _ => Err(RuntimeError::ToolCallNotFound),
    }
}

fn derive_run_status_locked(inner: &ManagerInner, run_id: &str) -> RunStatus {
    let step_ids = inner.steps_by_run.get(run_id);
    let Some(step_ids) = step_ids else {
        return RunStatus::Queued;
    };
    if step_ids.is_empty() {
        return RunStatus::Queued;
    }
    let mut has_planning_or_execution = false;
    let mut has_waiting_input = false;
    let mut has_blocked = false;
    let mut has_failed = false;
    let mut has_queued = false;
    let mut all_completed = true;
    let mut all_cancelled = true;
    for step_id in step_ids {
        if let Some(step) = inner.steps_by_id.get(step_id) {
            match step.status {
                StepStatus::Planning | StepStatus::CallingModel | StepStatus::ExecutingTool => {
                    has_planning_or_execution = true
                }
                StepStatus::WaitingInput => has_waiting_input = true,
                StepStatus::Blocked => has_blocked = true,
                StepStatus::Failed => has_failed = true,
                StepStatus::Queued => has_queued = true,
                _ => {}
            }
            if step.status != StepStatus::Completed {
                all_completed = false;
            }
            if step.status != StepStatus::Cancelled {
                all_cancelled = false;
            }
        }
    }
    if has_failed {
        RunStatus::Failed
    } else if all_completed {
        RunStatus::Completed
    } else if all_cancelled {
        RunStatus::Cancelled
    } else if has_blocked {
        RunStatus::Blocked
    } else if has_waiting_input {
        RunStatus::WaitingInput
    } else if has_planning_or_execution {
        RunStatus::Running
    } else if has_queued {
        RunStatus::Queued
    } else {
        RunStatus::Queued
    }
}

#[must_use]
pub fn is_run_terminal(status: RunStatus) -> bool {
    matches!(
        status,
        RunStatus::Completed | RunStatus::Failed | RunStatus::Cancelled
    )
}

#[must_use]
pub fn is_step_terminal(status: StepStatus) -> bool {
    matches!(
        status,
        StepStatus::Completed | StepStatus::Failed | StepStatus::Cancelled
    )
}

#[must_use]
fn can_transition(from: StepStatus, to: StepStatus) -> bool {
    use StepStatus::*;
    match (from, to) {
        (Queued, Planning) | (Queued, Cancelled) => true,
        (Planning, CallingModel)
        | (Planning, ExecutingTool)
        | (Planning, WaitingInput)
        | (Planning, Blocked)
        | (Planning, Failed)
        | (Planning, Cancelled) => true,
        (CallingModel, Planning)
        | (CallingModel, ExecutingTool)
        | (CallingModel, WaitingInput)
        | (CallingModel, Blocked)
        | (CallingModel, Completed)
        | (CallingModel, Failed)
        | (CallingModel, Cancelled) => true,
        (ExecutingTool, Planning)
        | (ExecutingTool, WaitingInput)
        | (ExecutingTool, Blocked)
        | (ExecutingTool, Completed)
        | (ExecutingTool, Failed)
        | (ExecutingTool, Cancelled) => true,
        (WaitingInput, Planning) | (WaitingInput, Cancelled) | (WaitingInput, Failed) => true,
        (Blocked, Planning) | (Blocked, Cancelled) | (Blocked, Failed) => true,
        _ => false,
    }
}

#[must_use]
pub fn new_run_id() -> String {
    let hex = Uuid::new_v4().simple().to_string();
    format!("run_{}", &hex[..16])
}

#[must_use]
pub fn new_tool_call_id() -> String {
    let hex = Uuid::new_v4().simple().to_string();
    format!("toolcall_{}", &hex[..16])
}

#[must_use]
fn new_step_id() -> String {
    let hex = Uuid::new_v4().simple().to_string();
    format!("step_{}", &hex[..16])
}

#[must_use]
pub fn live_validation_matrix_rows() -> Vec<kura_livevalidation::MatrixRow> {
    let tool_class = kura_livevalidation::ToolClass::from(
        kura_livevalidation::ToolClass::RUNTIME_LOCAL_TOOL_CALL,
    );
    match kura_livevalidation::default_matrix_row(&tool_class) {
        Some(row) => vec![row],
        None => Vec::new(),
    }
}
