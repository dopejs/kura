//! workflows route family (port of daemon/internal/api/workflows.go).
//!
//! The Go surface mounts these routes under /v1/runs/{runId}:
//! - GET|POST /v1/runs/{runId}/workflows (list / create+plan)
//! - GET /v1/runs/{runId}/workflows/{workflowId} (get by id)
//! - POST /v1/runs/{runId}/workflows/{workflowId}/start (initialize + advance)
//! - POST /v1/runs/{runId}/workflows/{workflowId}/cancel
//!
//! Handlers map to kura_orchestration (planning + the stateless transition
//! helpers: initialize_execution / advance_ready_steps / start_step_attempt /
//! bind_tool_call / apply_tool_call_result / reconcile_status / computer-use
//! projection), kura_runtime (run / step / tool-call truth) and the SQLite
//! store (workflow ledger + calendar/mail operation projections), preserving
//! the Go status codes, DTOs, validation and environment scoping. Tenant
//! scoping rides the shared protected() middleware; these handlers use the
//! legacy store paths exactly like the Go handlers.
//!
//! Deliberately not ported (reported, not duplicated):
//! - billing quota reservation/commit (billing manager Reserve/Commit not yet
//!   exposed in the api surface; Go skips when the billing manager is nil)
//! - agent-profile runtime projections (store ActiveAgentProfileSelection /
//!   RecordRuntimeProfileProjection not yet ported)
//! - the delivery latest-summary projection (delivery Manager lacks
//!   LatestSummaryForWorkflow)
//! - skill / capability sandbox tool-call preparation (Go prepareExecutable
//!   SkillToolCall / prepareCapabilityToolCall pipeline in toolcalls.go);
//!   those steps fail with consumer_unavailable so the workflow truth stays
//!   coherent
//! - thread runtime projections (kura-threads is not a dependency of kura-api)
//! - per-domain calendar/mail operation events (events crate has no
//!   calendar/mail builders)

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};

use kura_calendar as calendar;
use kura_computeruse as computeruse;
use kura_events as events;
use kura_integrations as integrations;
use kura_mail as mail;
use kura_mcp as mcp;
use kura_orchestration as orchestration;
use kura_runtime as runtime;
use kura_store::SQLiteStore;

use crate::error::ApiError;
use crate::middleware::environment_scope_from_config;
use crate::state::AppState;
use crate::types::{
    CalendarWorkflowActionRequest, CreateWorkflowRequest, MailWorkflowActionRequest,
    WorkflowListResponse,
};

/// Route family router. Only the methods the Go handlers accept are
/// registered; axum answers the other methods with 405 (Go
/// w.WriteHeader(http.StatusMethodNotAllowed)).
#[must_use]
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/v1/runs/{run_id}/workflows",
            get(list_workflows).post(create_workflow),
        )
        .route("/v1/runs/{run_id}/workflows/{workflow_id}", get(get_workflow))
        .route(
            "/v1/runs/{run_id}/workflows/{workflow_id}/start",
            post(start_workflow),
        )
        .route(
            "/v1/runs/{run_id}/workflows/{workflow_id}/cancel",
            post(cancel_workflow),
        )
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /v1/runs/{run_id}/workflows — list the run's workflows in the current
/// environment scope with calendar/mail summaries projected (Go
/// handleRunWorkflows GET branch).
async fn list_workflows(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> Result<Json<WorkflowListResponse>, ApiError> {
    let environment_scope = environment_scope_from_config(&state.config);
    let store = state.store.lock();
    let mut items = store
        .list_workflows(&environment_scope, &run_id)
        .map_err(ApiError::from_store)?;
    // Go: projectWorkflowDeliverySummaries — requires
    // deliveryManager.LatestSummaryForWorkflow (not ported); Go is a no-op
    // when the delivery manager is nil, so items pass through unchanged.
    items = project_workflows_calendar_summaries(&store, items).map_err(ApiError::from_store)?;
    items = project_workflows_mail_summaries(&store, items).map_err(ApiError::from_store)?;
    // Go: projectWorkflowProfileProjections — requires the tenant context and
    // store profile projection methods (not ported); Go is a no-op when the
    // requirements are unmet, so items pass through unchanged.
    Ok(Json(WorkflowListResponse { items }))
}

/// POST /v1/runs/{run_id}/workflows — plan a workflow for the run and persist
/// it, then publish workflow.planned (Go handleRunWorkflows POST branch).
/// Returns 201 with the planned workflow document.
async fn create_workflow(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    body: Bytes,
) -> Result<(StatusCode, Json<orchestration::Workflow>), ApiError> {
    // Go: decodeJSONBody — an empty body maps to "request body is required"
    // (400); malformed JSON maps to the decoder error (400).
    let input: CreateWorkflowRequest = if body.is_empty() {
        return Err(ApiError::BadRequest("request body is required".to_string()));
    } else {
        serde_json::from_slice(&body).map_err(|err| ApiError::BadRequest(err.to_string()))?
    };

    let runtime_manager = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::internal("runtime manager is not configured"))?;
    // Go: run, ok := manager.GetRun(runID); if !ok { http.NotFound }.
    let run = runtime_manager
        .get_run(&run_id)
        .ok_or_else(|| ApiError::NotFound("not found".to_string()))?;

    let calendar_action =
        build_calendar_action(input.calendar_action.as_ref()).map_err(ApiError::BadRequest)?;
    let mail_action = build_mail_action(input.mail_action.as_ref()).map_err(ApiError::BadRequest)?;

    // Go: orchestration.NewManager().Plan(...) with the skill/MCP planning
    // adapters; a fresh manager per request mirrors the stateless Go Manager.
    let orchestration_manager = orchestration::Manager::new();
    let skill_source = state.skills.as_deref().map(SkillPlanningAdapter::new);
    let mcp_source = state.mcp.as_deref().map(McpPlanningAdapter::new);
    let workflow = orchestration_manager.plan(
        &state.config,
        &run,
        &orchestration::CreateWorkflowInput {
            goal: input.goal,
            calendar_action,
            mail_action,
        },
        state.capabilities.as_deref(),
        skill_source
            .as_ref()
            .map(|adapter| adapter as &dyn orchestration::SkillPlanningSource),
        mcp_source
            .as_ref()
            .map(|adapter| adapter as &dyn orchestration::MCPPlanningSource),
    );

    // Go: reserveWorkflowLaunchQuota — billing not yet wired (Go is a no-op
    // when the billing manager is nil), so no reservation is held.
    persist_workflow_detail(&state, &workflow).map_err(ApiError::from_store)?;
    // Go: recordActiveProfileProjectionForTarget — profile store not ported.
    // Go: billing Commit/Release — skipped with the reservation above.
    publish_workflow_event(&state, "workflow.planned", &workflow, None, None)?;
    Ok((StatusCode::CREATED, Json(workflow)))
}

/// GET /v1/runs/{run_id}/workflows/{workflow_id} — fetch one workflow with
/// calendar/mail summaries projected (Go handleRunWorkflowByID).
async fn get_workflow(
    State(state): State<AppState>,
    Path((run_id, workflow_id)): Path<(String, String)>,
) -> Result<Json<orchestration::Workflow>, ApiError> {
    let environment_scope = environment_scope_from_config(&state.config);
    let store = state.store.lock();
    let Some(mut workflow) = store
        .get_workflow(&environment_scope, &run_id, &workflow_id)
        .map_err(ApiError::from_store)?
    else {
        return Err(ApiError::NotFound("not found".to_string()));
    };
    // Go: projectWorkflowDeliverySummary — delivery projection not ported.
    workflow =
        project_workflow_calendar_summaries(&store, workflow).map_err(ApiError::from_store)?;
    workflow = project_workflow_mail_summaries(&store, workflow).map_err(ApiError::from_store)?;
    // Go: projectWorkflowProfileProjection — not ported (no-op like the
    // unmet-requirements Go path).
    Ok(Json(workflow))
}

/// POST /v1/runs/{run_id}/workflows/{workflow_id}/start — validate the
/// workflow is startable, initialize execution, publish workflow.started
/// and advance the ready steps (Go handleRunWorkflowStart).
async fn start_workflow(
    State(state): State<AppState>,
    Path((run_id, workflow_id)): Path<(String, String)>,
) -> Result<Json<orchestration::Workflow>, ApiError> {
    let environment_scope = environment_scope_from_config(&state.config);
    let store = state.store.lock();
    let Some(workflow) = store
        .get_workflow(&environment_scope, &run_id, &workflow_id)
        .map_err(ApiError::from_store)?
    else {
        return Err(ApiError::NotFound("not found".to_string()));
    };
    drop(store);

    if workflow.status == orchestration::WorkflowStatus::PlanningFailed {
        return Err(ApiError::Conflict("workflow planning failed".to_string()));
    }
    if workflow.status != orchestration::WorkflowStatus::Planned
        && workflow.status != orchestration::WorkflowStatus::Blocked
    {
        return Err(ApiError::Conflict("workflow is not startable".to_string()));
    }

    let mut workflow = orchestration::initialize_execution(workflow, Utc::now());
    // Go: recordActiveProfileProjectionForTarget — not ported.
    persist_workflow_detail(&state, &workflow).map_err(ApiError::from_store)?;
    // Go: billing Commit — skipped with the reservation.
    publish_workflow_event(&state, "workflow.started", &workflow, None, None)?;

    workflow = advance_workflow_execution(&state, workflow)?;

    let store = state.store.lock();
    workflow =
        project_workflow_calendar_summaries(&store, workflow).map_err(ApiError::from_store)?;
    workflow = project_workflow_mail_summaries(&store, workflow).map_err(ApiError::from_store)?;
    // Go: projectWorkflowProfileProjection — not ported.
    Ok(Json(workflow))
}

/// POST /v1/runs/{run_id}/workflows/{workflow_id}/cancel — mark the workflow
/// cancelled, cancel in-flight runtime steps / sandbox executions, persist and
/// publish workflow.status_changed (Go handleRunWorkflowCancel).
async fn cancel_workflow(
    State(state): State<AppState>,
    Path((run_id, workflow_id)): Path<(String, String)>,
) -> Result<Json<orchestration::Workflow>, ApiError> {
    let environment_scope = environment_scope_from_config(&state.config);
    let store = state.store.lock();
    let Some(mut workflow) = store
        .get_workflow(&environment_scope, &run_id, &workflow_id)
        .map_err(ApiError::from_store)?
    else {
        return Err(ApiError::NotFound("not found".to_string()));
    };
    drop(store);

    let now = Utc::now();
    workflow.status = orchestration::WorkflowStatus::Cancelled;
    workflow.updated_at = now;
    workflow.completed_at = Some(now);

    let runtime_manager = state.runtime.as_ref();
    for step in &mut workflow.steps {
        if orchestration::is_terminal_step_status(step.status) {
            continue;
        }
        step.status = orchestration::StepStatus::Cancelled;
        step.updated_at = now;
        if !step.runtime_step_id.is_empty() {
            if let Some(manager) = runtime_manager {
                if let Ok((cancelled_step, run_update, _)) =
                    manager.cancel_step(&run_id, &step.runtime_step_id)
                {
                    let _ = persist_step_cancel_mutation(&state, &cancelled_step, run_update.as_ref());
                }
            }
        }
        if !step.active_tool_call_id.is_empty() {
            if let Some(manager) = runtime_manager {
                if let Some(tool_call) = manager.get_tool_call(
                    &run_id,
                    &step.runtime_step_id,
                    &step.active_tool_call_id,
                ) {
                    if !tool_call.sandbox_execution_id.is_empty() {
                        if let Some(sandboxes) = &state.sandboxes {
                            let _ = sandboxes.cancel_execution(&tool_call.sandbox_execution_id);
                        }
                    }
                }
            }
        }
    }

    persist_workflow_detail(&state, &workflow).map_err(ApiError::from_store)?;
    let mut extra = serde_json::Map::new();
    extra.insert("status".to_string(), serde_json::json!(workflow.status.as_str()));
    publish_workflow_event(&state, "workflow.status_changed", &workflow, None, Some(&extra))?;

    let store = state.store.lock();
    workflow =
        project_workflow_calendar_summaries(&store, workflow).map_err(ApiError::from_store)?;
    workflow = project_workflow_mail_summaries(&store, workflow).map_err(ApiError::from_store)?;
    // Go: projectWorkflowProfileProjection — not ported.
    Ok(Json(workflow))
}

// ---------------------------------------------------------------------------
// Planning adapters (Go skillPlanningAdapter / mcpPlanningAdapter)
// ---------------------------------------------------------------------------

struct SkillPlanningAdapter<'a> {
    registry: &'a kura_skills::Registry,
}

impl<'a> SkillPlanningAdapter<'a> {
    fn new(registry: &'a kura_skills::Registry) -> Self {
        Self { registry }
    }
}

impl orchestration::SkillPlanningSource for SkillPlanningAdapter<'_> {
    fn list_skills(&self) -> Vec<orchestration::SkillPlanningCandidate> {
        self.registry
            .list()
            .iter()
            .map(|skill| orchestration::SkillPlanningCandidate {
                skill_id: skill.skill_id.clone(),
                executable: skill.execution_manifest.is_some(),
                available: skill.availability_status
                    == kura_skills::SkillAvailabilityStatus::Available,
                approval_mode_expected: skill
                    .execution_manifest
                    .as_ref()
                    .map(|manifest| manifest.approval_mode.as_str().to_string())
                    .unwrap_or_default(),
            })
            .collect()
    }
}

struct McpPlanningAdapter<'a> {
    manager: &'a kura_mcp::Manager,
}

impl<'a> McpPlanningAdapter<'a> {
    fn new(manager: &'a kura_mcp::Manager) -> Self {
        Self { manager }
    }
}

impl orchestration::MCPPlanningSource for McpPlanningAdapter<'_> {
    fn list_servers(&self) -> Vec<orchestration::MCPPlanningServer> {
        self.manager
            .list_servers()
            .iter()
            .map(|server| orchestration::MCPPlanningServer {
                server_id: server.server.server_id.clone(),
                tools: server
                    .tools
                    .iter()
                    .map(|tool| orchestration::MCPPlanningTool {
                        tool_name: tool.tool.tool_name.clone(),
                    })
                    .collect(),
            })
            .collect()
    }

    fn list_tools(&self, server_id: &str) -> Result<Vec<orchestration::MCPPlanningTool>, String> {
        let tools = self
            .manager
            .list_tools(server_id)
            .map_err(|err| err.to_string())?;
        Ok(tools
            .into_iter()
            .map(|tool| orchestration::MCPPlanningTool {
                tool_name: tool.tool.tool_name,
            })
            .collect())
    }
}

// ---------------------------------------------------------------------------
// Request → action builders (Go buildCalendarAction / buildMailAction)
// ---------------------------------------------------------------------------

fn parse_optional_action_time(raw: &str) -> Result<Option<DateTime<Utc>>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    DateTime::parse_from_rfc3339(trimmed)
        .map(|parsed| Some(parsed.with_timezone(&Utc)))
        .map_err(|err| err.to_string())
}

fn build_calendar_action(
    request: Option<&CalendarWorkflowActionRequest>,
) -> Result<Option<calendar::Action>, String> {
    let Some(request) = request else {
        return Ok(None);
    };
    let mut action = calendar::Action {
        operation_class: request.operation_class,
        integration_id: request.integration_id.trim().to_string(),
        external_event_id: request.external_event_id.trim().to_string(),
        title: request.title.trim().to_string(),
        description: request.description.trim().to_string(),
        location: request.location.trim().to_string(),
        timezone: request.timezone.trim().to_string(),
        calendar_ref: request.calendar_ref.trim().to_string(),
        all_day: request.all_day,
        recurring: request.recurring,
        attendees: request
            .attendees
            .iter()
            .filter_map(|attendee| {
                let email = attendee.trim().to_string();
                if email.is_empty() {
                    None
                } else {
                    Some(email)
                }
            })
            .collect(),
        reason: request.reason.trim().to_string(),
        ..calendar::Action::default()
    };
    action.window_start = parse_optional_action_time(&request.window_start)
        .map_err(|err| format!("parse windowStart: {err}"))?;
    action.window_end =
        parse_optional_action_time(&request.window_end).map_err(|err| format!("parse windowEnd: {err}"))?;
    action.starts_at =
        parse_optional_action_time(&request.starts_at).map_err(|err| format!("parse startsAt: {err}"))?;
    action.ends_at =
        parse_optional_action_time(&request.ends_at).map_err(|err| format!("parse endsAt: {err}"))?;
    if action.operation_class.as_str().trim().is_empty() {
        return Err("calendarAction.operationClass is required".to_string());
    }
    Ok(Some(action))
}

fn build_mail_action(
    request: Option<&MailWorkflowActionRequest>,
) -> Result<Option<mail::Action>, String> {
    let Some(request) = request else {
        return Ok(None);
    };
    let action = mail::Action {
        operation_class: request.operation_class,
        integration_id: request.integration_id.trim().to_string(),
        thread_id: request.thread_id.trim().to_string(),
        message_id: request.message_id.trim().to_string(),
        draft_id: request.draft_id.trim().to_string(),
        compose_mode: request.compose_mode.as_str().to_string(),
        result_mode: request.result_mode.as_str().to_string(),
        to: request.to.clone(),
        cc: request.cc.clone(),
        bcc: request.bcc.clone(),
        subject: request.subject.trim().to_string(),
        body: request.body.clone(),
        attachment_refs: request
            .attachment_refs
            .iter()
            .map(|reference| mail::AttachmentRefInput {
                attachment_ref_id: reference.attachment_ref_id.clone(),
                display_name: reference.display_name.clone(),
                media_type: reference.media_type.clone(),
                size_bytes: if reference.size_bytes > 0 {
                    Some(reference.size_bytes)
                } else {
                    None
                },
            })
            .collect(),
        allow_send_side_effects: request.allow_send_side_effects,
    };
    if action.operation_class.as_str().trim().is_empty() {
        return Err("mailAction.operationClass is required".to_string());
    }
    Ok(Some(action))
}

// ---------------------------------------------------------------------------
// Workflow ledger persistence (Go persistWorkflowDetail)
// ---------------------------------------------------------------------------

fn persist_workflow_detail(state: &AppState, workflow: &orchestration::Workflow) -> Result<(), String> {
    let store = state.store.lock();
    store.upsert_workflow(workflow)?;
    store.replace_workflow_steps(&workflow.workflow_id, &workflow.steps)?;
    store.replace_workflow_dependencies(&workflow.workflow_id, &workflow.dependencies)?;
    store.replace_workflow_handoffs(&workflow.workflow_id, &workflow.handoffs)
}

// ---------------------------------------------------------------------------
// Runtime persistence helpers (Go persistRun / persistStep / persistToolCall /
// persistStepCancelMutation)
// ---------------------------------------------------------------------------

fn persist_run(state: &AppState, run: &runtime::Run) -> Result<(), String> {
    state.store.lock().upsert_run(run)
}

fn persist_step(state: &AppState, step: &runtime::Step) -> Result<(), String> {
    state.store.lock().upsert_step(step)
}

/// Go persistToolCall: persist the owning run + step first, then the tool call
/// (legacy path — the tenant-safe variants are deferred to the tenancy layer).
fn persist_tool_call(state: &AppState, tool_call: &runtime::ToolCall) -> Result<(), ApiError> {
    let store = state.store.lock();
    if let Some(manager) = state.runtime.as_ref() {
        if let Some(run) = manager.get_run(&tool_call.run_id) {
            store.upsert_run(&run).map_err(ApiError::from_store)?;
        } else {
            return Err(ApiError::internal(runtime::RuntimeError::RunNotFound));
        }
        let step = manager
            .get_step(&tool_call.run_id, &tool_call.step_id)
            .ok_or_else(|| ApiError::internal(runtime::RuntimeError::StepNotFound))?;
        store.upsert_step(&step).map_err(ApiError::from_store)?;
    }
    store.upsert_tool_call(tool_call).map_err(ApiError::from_store)
}

fn persist_step_cancel_mutation(
    state: &AppState,
    step: &runtime::Step,
    run_update: Option<&runtime::Run>,
) -> Result<(), String> {
    state.store.lock().upsert_step(step)?;
    if let Some(run) = run_update {
        persist_run(state, run)?;
    }
    // Go: persistCheckpoint — checkpoint durability is not part of this wave.
    Ok(())
}

// ---------------------------------------------------------------------------
// Events (Go publishWorkflowEvent / publishToolCallEvent)
// ---------------------------------------------------------------------------

fn workflow_event_step<'a>(
    workflow: &'a orchestration::Workflow,
    tool_call: Option<&runtime::ToolCall>,
    extra: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Option<&'a orchestration::WorkflowStep> {
    if let Some(tool_call) = tool_call {
        if !tool_call.workflow_step_id.is_empty() {
            return orchestration::workflow_step_by_id(workflow, &tool_call.workflow_step_id);
        }
    }
    let Some(extra) = extra else { return None };
    let workflow_step_id = extra
        .get("workflowStepId")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .unwrap_or("");
    if workflow_step_id.is_empty() {
        return None;
    }
    orchestration::workflow_step_by_id(workflow, workflow_step_id)
}

fn workflow_event_step_id(
    tool_call: Option<&runtime::ToolCall>,
    step: Option<&orchestration::WorkflowStep>,
    extra: Option<&serde_json::Map<String, serde_json::Value>>,
) -> String {
    if let Some(tool_call) = tool_call {
        if !tool_call.workflow_step_id.trim().is_empty() {
            return tool_call.workflow_step_id.clone();
        }
    }
    if let Some(step) = step {
        if !step.workflow_step_id.trim().is_empty() {
            return step.workflow_step_id.clone();
        }
    }
    if let Some(extra) = extra {
        if let Some(value) = extra.get("workflowStepId").and_then(|v| v.as_str()) {
            return value.to_string();
        }
    }
    String::new()
}

/// Go publishWorkflowEvent: builds the workflow category payload, appends
/// the event to the store and publishes it on the bus. The thread runtime
/// projection side effect is skipped (kura-threads is not an api dependency).
fn publish_workflow_event(
    state: &AppState,
    name: &str,
    workflow: &orchestration::Workflow,
    tool_call: Option<&runtime::ToolCall>,
    extra: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Result<events::Event, ApiError> {
    let step = workflow_event_step(workflow, tool_call, extra);
    let mut payload = serde_json::Map::new();
    payload.insert("workflowId".to_string(), serde_json::json!(workflow.workflow_id));
    payload.insert("runId".to_string(), serde_json::json!(workflow.run_id));
    payload.insert("status".to_string(), serde_json::json!(workflow.status.as_str()));
    if let Some(tool_call) = tool_call {
        payload.insert("workflowStepId".to_string(), serde_json::json!(tool_call.workflow_step_id));
        payload.insert("runtimeStepId".to_string(), serde_json::json!(tool_call.step_id));
        payload.insert("toolCallId".to_string(), serde_json::json!(tool_call.tool_call_id));
        payload.insert("attempt".to_string(), serde_json::json!(tool_call.attempt));
        payload.insert("toolName".to_string(), serde_json::json!(tool_call.tool_name));
        payload.insert("invocationKind".to_string(), serde_json::json!(tool_call.invocation_kind));
        if !tool_call.capability_id.is_empty() {
            payload.insert("consumerId".to_string(), serde_json::json!(tool_call.capability_id));
            payload.insert("consumerKind".to_string(), serde_json::json!(tool_call.invocation_kind));
        }
        if !tool_call.skill_id.is_empty() {
            payload.insert("consumerId".to_string(), serde_json::json!(tool_call.skill_id));
            payload.insert("consumerKind".to_string(), serde_json::json!(tool_call.invocation_kind));
        }
        if !tool_call.mcp_server_id.is_empty() {
            payload.insert("consumerId".to_string(), serde_json::json!(tool_call.mcp_server_id));
            payload.insert("consumerKind".to_string(), serde_json::json!(tool_call.invocation_kind));
        }
        if !tool_call.failure_class.is_empty() {
            payload.insert("failureClass".to_string(), serde_json::json!(tool_call.failure_class));
        }
    }
    if let Some(step) = step {
        if name == "workflow.step_status_changed" {
            payload.insert("status".to_string(), serde_json::json!(step.status.as_str()));
        }
        if !step.workflow_step_id.is_empty() {
            payload.insert("workflowStepId".to_string(), serde_json::json!(step.workflow_step_id));
        }
        if !step.consumer_kind.is_empty() {
            payload.insert("consumerKind".to_string(), serde_json::json!(step.consumer_kind));
        }
        if !step.consumer_id.is_empty() {
            payload.insert("consumerId".to_string(), serde_json::json!(step.consumer_id));
        }
        if !step.tool_name.is_empty() {
            payload.insert("toolName".to_string(), serde_json::json!(step.tool_name));
        }
        if !step.approval_mode_expected.is_empty() {
            payload.insert(
                "approvalModeExpected".to_string(),
                serde_json::json!(step.approval_mode_expected),
            );
        }
        if !step.blocked_reason.is_empty() {
            payload.insert("blockedReason".to_string(), serde_json::json!(step.blocked_reason));
        }
        if !step.last_failure_class.is_empty() && !payload.contains_key("failureClass") {
            payload.insert("failureClass".to_string(), serde_json::json!(step.last_failure_class));
        }
    }
    if let Some(extra) = extra {
        for (key, value) in extra {
            payload.insert(key.clone(), value.clone());
        }
    }

    let event = events::Event {
        category: "workflow".to_string(),
        name: name.to_string(),
        environment_scope: environment_scope_from_config(&state.config),
        scope: events::Scope {
            run_id: workflow.run_id.clone(),
            workflow_id: workflow.workflow_id.clone(),
            workflow_step_id: workflow_event_step_id(tool_call, step, extra),
            ..events::Scope::default()
        },
        resource: events::Resource {
            kind: "workflow".to_string(),
            id: workflow.workflow_id.clone(),
        },
        payload,
        ..events::Event::default()
    };

    // Go publishEvent: store append (legacy path) then bus publish.
    let stored = state
        .store
        .lock()
        .append_event(&event)
        .map_err(ApiError::from_store)?;
    Ok(state.event_bus.publish(stored))
}

/// Go publishToolCallEvent (category tool_call, scope run/step).
fn publish_tool_call_event(
    state: &AppState,
    name: &str,
    run_id: &str,
    step_id: &str,
    tool_call: &runtime::ToolCall,
) -> Result<events::Event, ApiError> {
    let mut payload = serde_json::Map::new();
    payload.insert("toolName".to_string(), serde_json::json!(tool_call.tool_name));
    payload.insert("status".to_string(), serde_json::json!(tool_call.status.as_str()));
    payload.insert("invocationKind".to_string(), serde_json::json!(tool_call.invocation_kind));
    if !tool_call.capability_id.is_empty() {
        payload.insert("capabilityId".to_string(), serde_json::json!(tool_call.capability_id));
    }
    if !tool_call.domain_kind.is_empty() {
        payload.insert("domainKind".to_string(), serde_json::json!(tool_call.domain_kind));
    }
    if !tool_call.skill_id.is_empty() {
        payload.insert("skillId".to_string(), serde_json::json!(tool_call.skill_id));
    }
    if !tool_call.mcp_server_id.is_empty() {
        payload.insert("mcpServerId".to_string(), serde_json::json!(tool_call.mcp_server_id));
    }
    if !tool_call.mcp_server_name.is_empty() {
        payload.insert("mcpServerName".to_string(), serde_json::json!(tool_call.mcp_server_name));
    }
    if !tool_call.mcp_tool_name.is_empty() {
        payload.insert("mcpToolName".to_string(), serde_json::json!(tool_call.mcp_tool_name));
    }
    if !tool_call.mcp_transport_kind.is_empty() {
        payload.insert("mcpTransportKind".to_string(), serde_json::json!(tool_call.mcp_transport_kind));
    }
    if !tool_call.mcp_session_id.is_empty() {
        payload.insert("mcpSessionId".to_string(), serde_json::json!(tool_call.mcp_session_id));
    }
    if !tool_call.authorization_result.is_empty() {
        payload.insert(
            "authorizationResult".to_string(),
            serde_json::json!(tool_call.authorization_result),
        );
    }
    if !tool_call.error.is_empty() {
        payload.insert("error".to_string(), serde_json::json!(tool_call.error));
    }
    if let Some(output) = &tool_call.output {
        payload.insert("output".to_string(), output.clone());
    }
    if !tool_call.sandbox_execution_id.is_empty() {
        payload.insert(
            "sandboxExecutionId".to_string(),
            serde_json::json!(tool_call.sandbox_execution_id),
        );
    }
    if !tool_call.failure_class.is_empty() {
        payload.insert("failureClass".to_string(), serde_json::json!(tool_call.failure_class));
    }

    let event = events::Event {
        category: "tool_call".to_string(),
        name: name.to_string(),
        environment_scope: environment_scope_from_config(&state.config),
        scope: events::Scope {
            run_id: run_id.to_string(),
            step_id: step_id.to_string(),
            ..events::Scope::default()
        },
        resource: events::Resource {
            kind: "tool_call".to_string(),
            id: tool_call.tool_call_id.clone(),
        },
        payload,
        ..events::Event::default()
    };
    let stored = state
        .store
        .lock()
        .append_event(&event)
        .map_err(ApiError::from_store)?;
    Ok(state.event_bus.publish(stored))
}

// ---------------------------------------------------------------------------
// Projections (Go calendar_projection.go / mail_projection.go)
// ---------------------------------------------------------------------------

fn project_workflows_calendar_summaries(
    store: &SQLiteStore,
    workflows: Vec<orchestration::Workflow>,
) -> Result<Vec<orchestration::Workflow>, String> {
    if workflows.is_empty() {
        return Ok(workflows);
    }
    let mut items = Vec::with_capacity(workflows.len());
    for workflow in workflows {
        items.push(project_workflow_calendar_summaries(store, workflow)?);
    }
    Ok(items)
}

fn project_workflow_calendar_summaries(
    store: &SQLiteStore,
    mut workflow: orchestration::Workflow,
) -> Result<orchestration::Workflow, String> {
    if workflow.environment_scope.trim().is_empty() || workflow.workflow_id.trim().is_empty() {
        return Ok(workflow);
    }
    let filter = kura_store::calendar::CalendarOperationFilter {
        workflow_id: workflow.workflow_id.clone(),
        ..kura_store::calendar::CalendarOperationFilter::default()
    };
    let operations = store.list_calendar_operations(&workflow.environment_scope, &filter)?;
    for step in &mut workflow.steps {
        let mut filtered = Vec::new();
        for item in &operations {
            if item.workflow_step_id.trim() == step.workflow_step_id {
                filtered.push(item.clone());
                continue;
            }
            if !step.runtime_step_id.is_empty()
                && item.step_id.trim() == step.runtime_step_id
            {
                filtered.push(item.clone());
            }
        }
        step.calendar_operation_summaries = summarize_calendar_operations(filtered);
    }
    Ok(workflow)
}

fn summarize_calendar_operations(mut items: Vec<calendar::Operation>) -> Vec<calendar::OperationSummary> {
    if items.is_empty() {
        return Vec::new();
    }
    items.sort_by(|left, right| {
        if left.updated_at == right.updated_at {
            left.operation_id.cmp(&right.operation_id)
        } else {
            left.updated_at.cmp(&right.updated_at)
        }
    });
    items
        .into_iter()
        .map(|item| {
            let captured_at = item.completed_at.unwrap_or(item.updated_at);
            calendar::OperationSummary {
                operation_id: item.operation_id,
                operation_class: item.operation_class,
                integration_id: item.integration_id,
                external_event_id: item.external_event_id,
                status: item.status,
                timezone_used: item.timezone_used,
                captured_at,
            }
        })
        .collect()
}

fn project_workflows_mail_summaries(
    store: &SQLiteStore,
    workflows: Vec<orchestration::Workflow>,
) -> Result<Vec<orchestration::Workflow>, String> {
    if workflows.is_empty() {
        return Ok(workflows);
    }
    let mut items = Vec::with_capacity(workflows.len());
    for workflow in workflows {
        items.push(project_workflow_mail_summaries(store, workflow)?);
    }
    Ok(items)
}

fn project_workflow_mail_summaries(
    store: &SQLiteStore,
    mut workflow: orchestration::Workflow,
) -> Result<orchestration::Workflow, String> {
    if workflow.environment_scope.trim().is_empty() || workflow.workflow_id.trim().is_empty() {
        return Ok(workflow);
    }
    let filter = kura_store::mail::MailOperationFilter {
        workflow_id: workflow.workflow_id.clone(),
        ..kura_store::mail::MailOperationFilter::default()
    };
    let operations = store.list_mail_operations(&workflow.environment_scope, &filter)?;
    for step in &mut workflow.steps {
        let mut filtered = Vec::new();
        for item in &operations {
            if item.workflow_step_id.trim() == step.workflow_step_id {
                filtered.push(item.clone());
                continue;
            }
            if !step.runtime_step_id.is_empty() && item.step_id.trim() == step.runtime_step_id {
                filtered.push(item.clone());
            }
        }
        step.mail_operation_summaries = summarize_mail_operations(filtered);
    }
    Ok(workflow)
}

fn summarize_mail_operations(mut items: Vec<mail::Operation>) -> Vec<mail::OperationSummary> {
    if items.is_empty() {
        return Vec::new();
    }
    items.sort_by(|left, right| {
        if left.updated_at == right.updated_at {
            left.operation_id.cmp(&right.operation_id)
        } else {
            left.updated_at.cmp(&right.updated_at)
        }
    });
    items
        .into_iter()
        .map(|item| {
            let captured_at = item.completed_at.unwrap_or(item.updated_at);
            mail::OperationSummary {
                operation_id: item.operation_id,
                operation_class: item.operation_class,
                integration_id: item.integration_id,
                thread_id: item.thread_id,
                message_id: item.message_id,
                draft_id: item.draft_id,
                result_mode: item.result_mode,
                send_path: item.send_path,
                status: item.status,
                captured_at,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Execution advancement (Go advanceWorkflowExecution)
// ---------------------------------------------------------------------------

/// Drives ready steps to completion: advance ready steps, start each Ready
/// step, and reconcile the workflow status after each pass (Go
/// advanceWorkflowExecution).
fn advance_workflow_execution(
    state: &AppState,
    mut workflow: orchestration::Workflow,
) -> Result<orchestration::Workflow, ApiError> {
    loop {
        let now = Utc::now();
        let (next, changed) = orchestration::advance_ready_steps(workflow, now);
        workflow = next;
        if changed {
            persist_workflow_detail(state, &workflow).map_err(ApiError::from_store)?;
        }

        let mut progressed = false;
        let ready_step_ids: Vec<String> = workflow
            .steps
            .iter()
            .filter(|step| step.status == orchestration::StepStatus::Ready)
            .map(|step| step.workflow_step_id.clone())
            .collect();
        for step_id in ready_step_ids {
            let wf_step = workflow
                .steps
                .iter()
                .find(|step| step.workflow_step_id == step_id)
                .cloned()
                .ok_or_else(|| ApiError::internal("workflow step disappeared during execution"))?;
            let (next_workflow, terminal_sync) =
                start_workflow_step_execution(state, workflow, &wf_step)?;
            workflow = next_workflow;
            progressed = true;
            if !terminal_sync {
                workflow = orchestration::reconcile_status(workflow, Utc::now());
                persist_workflow_detail(state, &workflow).map_err(ApiError::from_store)?;
                return Ok(workflow);
            }
        }

        workflow = orchestration::reconcile_status(workflow, Utc::now());
        persist_workflow_detail(state, &workflow).map_err(ApiError::from_store)?;
        if !progressed {
            capture_terminal_workflow_memory(state, &workflow);
            return Ok(workflow);
        }
    }
}

/// Spec 058 phase 2 W1: terminal workflows capture one L0 memory ref so task
/// outcomes are extractable evidence (fire-and-forget).
fn capture_terminal_workflow_memory(state: &AppState, workflow: &orchestration::Workflow) {
    if !matches!(
        workflow.status,
        orchestration::WorkflowStatus::Completed
            | orchestration::WorkflowStatus::Failed
            | orchestration::WorkflowStatus::Cancelled
    ) {
        return;
    }
    let text = format!(
        "workflow {} ({}) finished with status {}",
        workflow.workflow_id,
        workflow.goal,
        workflow.status.as_str()
    );
    let captured = super::memory::capture_l0(
        state,
        "",
        kura_memory::Actor {
            kind: kura_memory::ActorKind::System,
            id: "workflow".to_string(),
        },
        "workflow_result",
        &text,
        vec![
            kura_memory::SourceLink {
                kind: kura_memory::SourceKind::Run,
                id: workflow.run_id.clone(),
                ..kura_memory::SourceLink::default()
            },
        ],
    );
    if captured.is_some_and(|(_, due)| due) {
        let state = state.clone();
        std::thread::spawn(move || {
            if let Err(err) = super::memory::execute_consolidation(&state, "", "turns", None) {
                eprintln!("memory: workflow turn-trigger consolidation failed: {err:?}");
            }
        });
    }
}

/// Starts one Ready workflow step: creates the runtime step (Planning →
/// ExecutingTool), records the workflow step attempt, then dispatches to the
/// consumer-specific executor. Returns (workflow, terminal_sync) mirroring
/// Go startWorkflowStepExecution.
fn start_workflow_step_execution(
    state: &AppState,
    workflow: orchestration::Workflow,
    wf_step: &orchestration::WorkflowStep,
) -> Result<(orchestration::Workflow, bool), ApiError> {
    if wf_step.consumer_kind == "computer_use" {
        return execute_workflow_computer_use_step(state, workflow, wf_step);
    }

    let runtime_manager = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::internal("runtime manager is not configured"))?;
    let runtime_step = runtime_manager
        .create_step(
            &workflow.run_id,
            runtime::CreateStepInput {
                title: wf_step.title.clone(),
                kind: "workflow".to_string(),
                workflow_id: workflow.workflow_id.clone(),
                workflow_step_id: wf_step.workflow_step_id.clone(),
                attempt: wf_step.attempt_count + 1,
                input: wf_step.input.clone(),
            },
        )
        .map_err(ApiError::internal)?;
    let (runtime_step, run_update) = runtime_manager
        .update_step_status_and_reconcile_run(
            &workflow.run_id,
            &runtime_step.step_id,
            runtime::UpdateStepStatusInput {
                status: runtime::StepStatus::Planning,
                output: None,
            },
        )
        .map_err(ApiError::internal)?;
    persist_step(state, &runtime_step).map_err(ApiError::from_store)?;
    if let Some(run_update) = run_update {
        persist_run(state, &run_update).map_err(ApiError::from_store)?;
    }
    let (runtime_step, run_update) = runtime_manager
        .update_step_status_and_reconcile_run(
            &workflow.run_id,
            &runtime_step.step_id,
            runtime::UpdateStepStatusInput {
                status: runtime::StepStatus::ExecutingTool,
                output: None,
            },
        )
        .map_err(ApiError::internal)?;
    persist_step(state, &runtime_step).map_err(ApiError::from_store)?;
    if let Some(run_update) = run_update {
        persist_run(state, &run_update).map_err(ApiError::from_store)?;
    }
    // Go: persistCheckpoint — checkpoint durability is not part of this wave.

    let workflow =
        orchestration::start_step_attempt(workflow, &wf_step.workflow_step_id, &runtime_step.step_id, Utc::now());

    match wf_step.consumer_kind.as_str() {
        "calendar" => {
            let (tool_call, step_status, blocked_reason) =
                execute_workflow_calendar_step(state, &workflow, &runtime_step, wf_step)?;
            advance_workflow_after_tool_call(state, workflow, &tool_call, Some(step_status), &blocked_reason)
        }
        "mail" => {
            let (tool_call, step_status, blocked_reason) =
                execute_workflow_mail_step(state, &workflow, &runtime_step, wf_step)?;
            advance_workflow_after_tool_call(state, workflow, &tool_call, Some(step_status), &blocked_reason)
        }
        "mcp_tool" => {
            let (tool_call, step_status, blocked_reason) =
                execute_workflow_mcp_tool(state, &workflow, &runtime_step, wf_step)?;
            advance_workflow_after_tool_call(state, workflow, &tool_call, Some(step_status), &blocked_reason)
        }
        _ => {
            // skill / local_tool / capability consumers: the Go sandbox/policy
            // preparation pipeline is not ported; create the runtime tool call
            // and fail it as consumer_unavailable so the workflow truth stays
            // coherent (matches the Go MCP-authorize blocked path shape).
            let (tool_call, terminal_sync, step_status, blocked_reason) =
                execute_workflow_capability_tool(state, &workflow, &runtime_step, wf_step)?;
            if terminal_sync {
                advance_workflow_after_tool_call(state, workflow, &tool_call, Some(step_status), &blocked_reason)
            } else {
                let workflow =
                    orchestration::bind_tool_call(workflow, &wf_step.workflow_step_id, &tool_call, Utc::now());
                persist_workflow_detail(state, &workflow).map_err(ApiError::from_store)?;
                Ok((workflow, false))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Consumer executors (Go executeWorkflowCalendarStep / MailStep / MCPTool /
// CapabilityTool / ComputerUseStep)
// ---------------------------------------------------------------------------

struct CalendarExecutionResult {
    account: calendar::AccountProjection,
    operation: calendar::Operation,
    artifacts: Vec<calendar::Artifact>,
    output: Option<serde_json::Value>,
}

impl Default for CalendarExecutionResult {
    fn default() -> Self {
        Self {
            account: calendar::AccountProjection::default(),
            operation: calendar::Operation::default(),
            artifacts: Vec::new(),
            output: None,
        }
    }
}

fn calendar_tool_call_output(result: &CalendarExecutionResult) -> serde_json::Value {
    let mut output = serde_json::Map::new();
    output.insert(
        "operation".to_string(),
        serde_json::to_value(&result.operation).unwrap_or(serde_json::Value::Null),
    );
    if !result.artifacts.is_empty() {
        output.insert(
            "artifacts".to_string(),
            serde_json::to_value(&result.artifacts).unwrap_or(serde_json::Value::Null),
        );
    }
    if let Some(result_output) = &result.output {
        output.insert("result".to_string(), result_output.clone());
    }
    serde_json::Value::Object(output)
}

/// Go executeCalendarAction — dispatch one calendar action to the calendar
/// manager. Returns (result, error); on error the operation/account are
/// defaulted because the Rust manager surfaces only the error (the Go manager
/// returns the partial operation alongside the error).
fn execute_calendar_action(
    state: &AppState,
    action: &calendar::Action,
    source: calendar::SourceLinkage,
) -> (CalendarExecutionResult, Option<String>) {
    let (Some(calendar_manager), Some(integrations_manager)) =
        (state.calendar.as_deref(), state.integrations.as_deref())
    else {
        return (
            CalendarExecutionResult::default(),
            Some("calendar dependencies are not configured".to_string()),
        );
    };
    let resources = integrations_manager.list();
    let selection = calendar::Selection {
        integration_id: action.integration_id.trim().to_string(),
    };
    let map_err = |err: calendar::CalendarError| (CalendarExecutionResult::default(), Some(err.to_string()));

    let result = match action.operation_class {
        calendar::OperationClass::ListEvents => {
            match calendar_manager.list_events(
                &resources,
                &calendar::ListEventsInput {
                    selection,
                    starts_at: action.window_start,
                    ends_at: action.window_end,
                    source,
                },
            ) {
                Ok((account, items, operation, artifacts)) => CalendarExecutionResult {
                    account: account.clone(),
                    operation: operation.clone(),
                    artifacts: artifacts.clone(),
                    output: Some(serde_json::json!({
                        "account": account,
                        "items": items,
                        "operation": operation,
                        "artifacts": artifacts,
                    })),
                },
                Err(err) => return map_err(err),
            }
        }
        calendar::OperationClass::GetEvent => {
            match calendar_manager.get_event(
                &resources,
                &calendar::GetEventInput {
                    selection,
                    external_event_id: action.external_event_id.trim().to_string(),
                    source,
                },
            ) {
                Ok((account, item, operation, artifacts)) => CalendarExecutionResult {
                    account: account.clone(),
                    operation: operation.clone(),
                    artifacts: artifacts.clone(),
                    output: Some(serde_json::json!({
                        "account": account,
                        "event": item,
                        "operation": operation,
                        "artifacts": artifacts,
                    })),
                },
                Err(err) => return map_err(err),
            }
        }
        calendar::OperationClass::BusyFree => {
            let (Some(window_start), Some(window_end)) = (action.window_start, action.window_end)
            else {
                return (
                    CalendarExecutionResult::default(),
                    Some(
                        "calendarAction.windowStart and calendarAction.windowEnd are required for busy_free"
                            .to_string(),
                    ),
                );
            };
            match calendar_manager.busy_free(
                &resources,
                &calendar::BusyFreeInput {
                    selection,
                    window_start,
                    window_end,
                    timezone: action.timezone.trim().to_string(),
                    source,
                },
            ) {
                Ok((account, query, operation, artifacts)) => CalendarExecutionResult {
                    account: account.clone(),
                    operation: operation.clone(),
                    artifacts: artifacts.clone(),
                    output: Some(serde_json::json!({
                        "account": account,
                        "query": query,
                        "operation": operation,
                        "artifacts": artifacts,
                    })),
                },
                Err(err) => return map_err(err),
            }
        }
        calendar::OperationClass::CreateEvent | calendar::OperationClass::UpdateEvent => {
            if !action.calendar_ref.trim().is_empty() {
                return (
                    CalendarExecutionResult::default(),
                    Some(calendar::CalendarError::CalendarAlternateCalendarDeny.to_string()),
                );
            }
            let (Some(starts_at), Some(ends_at)) = (action.starts_at, action.ends_at) else {
                return (
                    CalendarExecutionResult::default(),
                    Some(
                        "calendarAction.startsAt and calendarAction.endsAt are required for create_event / update_event"
                            .to_string(),
                    ),
                );
            };
            let update = action.operation_class == calendar::OperationClass::UpdateEvent;
            let outcome = if update {
                calendar_manager.update_event(
                    &resources,
                    &calendar::UpdateEventInput {
                        selection,
                        external_event_id: action.external_event_id.trim().to_string(),
                        title: action.title.trim().to_string(),
                        description: action.description.trim().to_string(),
                        location: action.location.trim().to_string(),
                        starts_at,
                        ends_at,
                        timezone: action.timezone.trim().to_string(),
                        all_day: action.all_day,
                        recurring: action.recurring,
                        attendees: action.attendees.clone(),
                        ..calendar::UpdateEventInput::default()
                    },
                )
            } else {
                calendar_manager.create_event(
                    &resources,
                    &calendar::CreateEventInput {
                        selection,
                        title: action.title.trim().to_string(),
                        description: action.description.trim().to_string(),
                        location: action.location.trim().to_string(),
                        starts_at,
                        ends_at,
                        timezone: action.timezone.trim().to_string(),
                        all_day: action.all_day,
                        recurring: action.recurring,
                        attendees: action.attendees.clone(),
                        source,
                        ..calendar::CreateEventInput::default()
                    },
                )
            };
            match outcome {
                Ok((account, item, operation, artifacts)) => CalendarExecutionResult {
                    account: account.clone(),
                    operation: operation.clone(),
                    artifacts: artifacts.clone(),
                    output: Some(serde_json::json!({
                        "account": account,
                        "event": item,
                        "operation": operation,
                        "artifacts": artifacts,
                    })),
                },
                Err(err) => return map_err(err),
            }
        }
        calendar::OperationClass::CancelEvent => {
            if !action.calendar_ref.trim().is_empty() {
                return (
                    CalendarExecutionResult::default(),
                    Some(calendar::CalendarError::CalendarAlternateCalendarDeny.to_string()),
                );
            }
            match calendar_manager.cancel_event(
                &resources,
                &calendar::CancelEventInput {
                    selection,
                    external_event_id: action.external_event_id.trim().to_string(),
                    reason: action.reason.trim().to_string(),
                    recurrence_scope: calendar::RecurrenceScope::Unspecified,
                    source,
                },
            ) {
                Ok((account, item, operation, artifacts)) => CalendarExecutionResult {
                    account: account.clone(),
                    operation: operation.clone(),
                    artifacts: artifacts.clone(),
                    output: Some(serde_json::json!({
                        "account": account,
                        "event": item,
                        "operation": operation,
                        "artifacts": artifacts,
                    })),
                },
                Err(err) => return map_err(err),
            }
        }
        calendar::OperationClass::UpdateAttendees => {
            return (
                CalendarExecutionResult::default(),
                Some(format!(
                    "unsupported calendar action {:?}",
                    action.operation_class.as_str()
                )),
            );
        }
    };
    (result, None)
}

/// Go executeWorkflowCalendarStep — run the calendar action and complete/fail
/// the runtime tool call with the operation summary output.
fn execute_workflow_calendar_step(
    state: &AppState,
    workflow: &orchestration::Workflow,
    runtime_step: &runtime::Step,
    wf_step: &orchestration::WorkflowStep,
) -> Result<(runtime::ToolCall, orchestration::StepStatus, String), ApiError> {
    let action = decode_calendar_action(wf_step.input.as_ref())?;
    let runtime_manager = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::internal("runtime manager is not configured"))?;
    let tool_call = runtime_manager
        .create_tool_call(
            &workflow.run_id,
            &runtime_step.step_id,
            runtime::CreateToolCallInput {
                workflow_id: workflow.workflow_id.clone(),
                workflow_step_id: wf_step.workflow_step_id.clone(),
                attempt: wf_step.attempt_count + 1,
                invocation_kind: runtime::ToolCallInvocationKind::DomainTool.as_str().to_string(),
                domain_kind: "calendar".to_string(),
                tool_name: action.operation_class.as_str().to_string(),
                input: serde_json::to_value(&action).ok(),
                ..runtime::CreateToolCallInput::default()
            },
        )
        .map_err(ApiError::internal)?;
    persist_tool_call(state, &tool_call)?;
    publish_tool_call_event(state, "tool_call.requested", &workflow.run_id, &runtime_step.step_id, &tool_call)?;

    let (result, exec_err) = execute_calendar_action(
        state,
        &action,
        calendar::SourceLinkage {
            run_id: workflow.run_id.clone(),
            step_id: runtime_step.step_id.clone(),
            tool_call_id: tool_call.tool_call_id.clone(),
            workflow_id: workflow.workflow_id.clone(),
            workflow_step_id: wf_step.workflow_step_id.clone(),
            schedule_id: workflow.schedule_id.clone(),
            schedule_attempt_id: workflow.schedule_attempt_id.clone(),
            ..calendar::SourceLinkage::default()
        },
    );

    // Go recordCalendarActivity: persist account/operation/artifacts to the
    // store (the domain events are not ported). Skip when no operation id.
    if !result.operation.operation_id.is_empty() {
        record_calendar_activity(state, &result.account, &result.operation, &result.artifacts)?;
    }

    let bindings = calendar_integration_bindings(
        state,
        orchestration::first_non_empty(&[
            &result.operation.integration_id,
            &action.integration_id,
        ]),
    );
    let output = calendar_tool_call_output(&result);

    if let Some(exec_err) = exec_err {
        let tool_call = runtime_manager
            .fail_tool_call(
                &workflow.run_id,
                &runtime_step.step_id,
                &tool_call.tool_call_id,
                runtime::FailToolCallInput {
                    output: Some(output),
                    error: exec_err.clone(),
                    failure_class: calendar_failure_class_for_string(&exec_err).to_string(),
                    integration_bindings: bindings,
                    ..runtime::FailToolCallInput::default()
                },
            )
            .map_err(ApiError::internal)?;
        persist_tool_call(state, &tool_call)?;
        publish_tool_call_event(state, "tool_call.failed", &workflow.run_id, &runtime_step.step_id, &tool_call)?;
        return Ok((tool_call, orchestration::StepStatus::Failed, String::new()));
    }

    let tool_call = runtime_manager
        .complete_tool_call(
            &workflow.run_id,
            &runtime_step.step_id,
            &tool_call.tool_call_id,
            runtime::CompleteToolCallInput {
                output: Some(output),
                integration_bindings: bindings,
                ..runtime::CompleteToolCallInput::default()
            },
        )
        .map_err(ApiError::internal)?;
    persist_tool_call(state, &tool_call)?;
    publish_tool_call_event(state, "tool_call.completed", &workflow.run_id, &runtime_step.step_id, &tool_call)?;
    Ok((tool_call, orchestration::StepStatus::Completed, String::new()))
}

/// Maps a calendar execution error string to its stable failure class. The Go
/// version matches typed sentinel errors; the Rust manager surfaces strings,
/// so the classification is applied to the message.
fn calendar_failure_class_for_string(err: &str) -> &'static str {
    if err.contains("integration is unavailable") {
        "calendar_unavailable"
    } else if err.contains("integration not found") {
        "integration_not_found"
    } else if err.contains("selection is invalid") {
        "selection_invalid"
    } else if err.contains("event not found") {
        "event_not_found"
    } else if err.contains("time range") {
        "invalid_time_range"
    } else if err.contains("out of scope") || err.contains("alternate-calendar") {
        "scope_violation"
    } else {
        "calendar_error"
    }
}

/// Go decodeCalendarAction — the workflow step input is the serialized action.
fn decode_calendar_action(input: Option<&serde_json::Value>) -> Result<calendar::Action, ApiError> {
    let Some(input) = input else {
        return Err(ApiError::internal("calendar workflow step input is missing"));
    };
    serde_json::from_value(input.clone()).map_err(ApiError::internal)
}

fn record_calendar_activity(
    state: &AppState,
    account: &calendar::AccountProjection,
    operation: &calendar::Operation,
    artifacts: &[calendar::Artifact],
) -> Result<(), ApiError> {
    let store = state.store.lock();
    if !account.integration_id.is_empty() {
        store.upsert_calendar_account(account).map_err(ApiError::from_store)?;
    }
    if operation.operation_id.is_empty() {
        return Ok(());
    }
    store.upsert_calendar_operation(operation).map_err(ApiError::from_store)?;
    for artifact in artifacts {
        store.upsert_calendar_artifact(artifact).map_err(ApiError::from_store)?;
    }
    Ok(())
}

fn calendar_integration_bindings(
    state: &AppState,
    integration_id: String,
) -> Vec<integrations::BindingSummary> {
    let Some(integrations_manager) = state.integrations.as_deref() else {
        return Vec::new();
    };
    if integration_id.trim().is_empty() {
        return Vec::new();
    }
    match integrations_manager.binding_summary(&integration_id, Utc::now()) {
        Ok(binding) => vec![binding],
        Err(_) => Vec::new(),
    }
}

struct MailExecutionResult {
    account: mail::AccountProjection,
    operation: mail::Operation,
    artifacts: Vec<mail::Artifact>,
    output: Option<serde_json::Value>,
}

impl Default for MailExecutionResult {
    fn default() -> Self {
        Self {
            account: mail::AccountProjection::default(),
            operation: mail::Operation::default(),
            artifacts: Vec::new(),
            output: None,
        }
    }
}

fn mail_tool_call_output(result: &MailExecutionResult) -> serde_json::Value {
    serde_json::json!({
        "account": result.account,
        "operation": result.operation,
        "artifacts": result.artifacts,
        "result": result.output,
    })
}

/// Go executeMailAction — dispatch one mail action to the mail manager.
fn execute_mail_action(
    state: &AppState,
    action: &mail::Action,
    source: mail::SourceLinkage,
) -> (MailExecutionResult, Option<String>) {
    let (Some(mail_manager), Some(integrations_manager)) =
        (state.mail.as_deref(), state.integrations.as_deref())
    else {
        return (
            MailExecutionResult::default(),
            Some("mail dependencies are not configured".to_string()),
        );
    };
    let resources = integrations_manager.list();
    let selection = mail::Selection {
        integration_id: action.integration_id.trim().to_string(),
    };
    let map_err = |err: mail::MailError| (MailExecutionResult::default(), Some(err.to_string()));

    let result = match action.operation_class {
        mail::OperationClass::ListThreads => {
            match mail_manager.list_threads(
                &resources,
                &mail::ListThreadsInput {
                    selection,
                    source,
                    ..mail::ListThreadsInput::default()
                },
            ) {
                Ok((account, items, operation, artifacts)) => MailExecutionResult {
                    account: account.clone(),
                    operation: operation.clone(),
                    artifacts: artifacts.clone(),
                    output: Some(serde_json::json!({ "account": account, "items": items, "operation": operation, "artifacts": artifacts })),
                },
                Err(err) => return map_err(err),
            }
        }
        mail::OperationClass::GetThread => {
            match mail_manager.get_thread(
                &resources,
                &mail::GetThreadInput {
                    selection,
                    thread_id: action.thread_id.trim().to_string(),
                    source,
                    ..mail::GetThreadInput::default()
                },
            ) {
                Ok((account, item, operation, artifacts)) => MailExecutionResult {
                    account: account.clone(),
                    operation: operation.clone(),
                    artifacts: artifacts.clone(),
                    output: Some(serde_json::json!({ "account": account, "thread": item, "operation": operation, "artifacts": artifacts })),
                },
                Err(err) => return map_err(err),
            }
        }
        mail::OperationClass::GetMessage => {
            match mail_manager.get_message(
                &resources,
                &mail::GetMessageInput {
                    selection,
                    message_id: action.message_id.trim().to_string(),
                    source,
                    ..mail::GetMessageInput::default()
                },
            ) {
                Ok((account, item, operation, artifacts)) => MailExecutionResult {
                    account: account.clone(),
                    operation: operation.clone(),
                    artifacts: artifacts.clone(),
                    output: Some(serde_json::json!({ "account": account, "message": item, "operation": operation, "artifacts": artifacts })),
                },
                Err(err) => return map_err(err),
            }
        }
        mail::OperationClass::ListDrafts => {
            match mail_manager.list_drafts(
                &resources,
                &mail::ListDraftsInput {
                    selection,
                    source,
                    ..mail::ListDraftsInput::default()
                },
            ) {
                Ok((account, items, operation, artifacts)) => MailExecutionResult {
                    account: account.clone(),
                    operation: operation.clone(),
                    artifacts: artifacts.clone(),
                    output: Some(serde_json::json!({ "account": account, "items": items, "operation": operation, "artifacts": artifacts })),
                },
                Err(err) => return map_err(err),
            }
        }
        mail::OperationClass::GetDraft => {
            match mail_manager.get_draft(
                &resources,
                &mail::GetDraftInput {
                    selection,
                    draft_id: action.draft_id.trim().to_string(),
                    source,
                    ..mail::GetDraftInput::default()
                },
            ) {
                Ok((account, item, operation, artifacts)) => MailExecutionResult {
                    account: account.clone(),
                    operation: operation.clone(),
                    artifacts: artifacts.clone(),
                    output: Some(serde_json::json!({ "account": account, "draft": item, "operation": operation, "artifacts": artifacts })),
                },
                Err(err) => return map_err(err),
            }
        }
        mail::OperationClass::CreateDraft => {
            match mail_manager.create_draft(
                &resources,
                &mail::CreateDraftInput {
                    selection,
                    compose_mode: deserialize_enum::<mail::ComposeMode>(&action.compose_mode),
                    thread_id: action.thread_id.trim().to_string(),
                    source_message_id: action.message_id.trim().to_string(),
                    to: action.to.clone(),
                    cc: action.cc.clone(),
                    bcc: action.bcc.clone(),
                    subject: action.subject.trim().to_string(),
                    body: action.body.clone(),
                    attachment_refs: action.attachment_refs.clone(),
                    source,
                },
            ) {
                Ok((account, item, operation, artifacts)) => MailExecutionResult {
                    account: account.clone(),
                    operation: operation.clone(),
                    artifacts: artifacts.clone(),
                    output: Some(serde_json::json!({ "account": account, "draft": item, "operation": operation, "artifacts": artifacts })),
                },
                Err(err) => return map_err(err),
            }
        }
        mail::OperationClass::SendMessage => {
            match mail_manager.send_message(
                &resources,
                &mail::SendMessageInput {
                    selection,
                    to: action.to.clone(),
                    cc: action.cc.clone(),
                    bcc: action.bcc.clone(),
                    subject: action.subject.trim().to_string(),
                    body: action.body.clone(),
                    attachment_refs: action.attachment_refs.clone(),
                    source,
                },
            ) {
                Ok((account, item, operation, artifacts)) => MailExecutionResult {
                    account: account.clone(),
                    operation: operation.clone(),
                    artifacts: artifacts.clone(),
                    output: Some(serde_json::json!({ "account": account, "message": item, "operation": operation, "artifacts": artifacts })),
                },
                Err(err) => return map_err(err),
            }
        }
        mail::OperationClass::SendDraft => {
            match mail_manager.send_draft(
                &resources,
                &mail::SendDraftInput {
                    selection,
                    draft_id: action.draft_id.trim().to_string(),
                    source,
                },
            ) {
                Ok((account, _, item, operation, artifacts)) => MailExecutionResult {
                    account: account.clone(),
                    operation: operation.clone(),
                    artifacts: artifacts.clone(),
                    output: Some(serde_json::json!({ "account": account, "message": item, "operation": operation, "artifacts": artifacts })),
                },
                Err(err) => return map_err(err),
            }
        }
        mail::OperationClass::ReplyMessage | mail::OperationClass::ForwardMessage => {
            let forward = action.operation_class == mail::OperationClass::ForwardMessage;
            let result_mode = deserialize_enum::<mail::ReplyForwardResultMode>(&action.result_mode);
            let outcome = if forward {
                mail_manager.forward_message(
                    &resources,
                    &mail::ForwardMessageInput {
                        selection,
                        message_id: action.message_id.trim().to_string(),
                        result_mode,
                        to: action.to.clone(),
                        cc: action.cc.clone(),
                        bcc: action.bcc.clone(),
                        subject: action.subject.trim().to_string(),
                        body: action.body.clone(),
                        attachment_refs: action.attachment_refs.clone(),
                        source,
                    },
                )
            } else {
                mail_manager.reply_message(
                    &resources,
                    &mail::ReplyMessageInput {
                        selection,
                        message_id: action.message_id.trim().to_string(),
                        result_mode,
                        subject: action.subject.trim().to_string(),
                        body: action.body.clone(),
                        attachment_refs: action.attachment_refs.clone(),
                        source,
                    },
                )
            };
            match outcome {
                Ok((account, draft, message, operation, artifacts)) => {
                    let output = if let Some(draft) = draft {
                        serde_json::json!({ "account": account, "draft": draft, "operation": operation, "artifacts": artifacts })
                    } else {
                        serde_json::json!({ "account": account, "message": message, "operation": operation, "artifacts": artifacts })
                    };
                    MailExecutionResult { account, operation, artifacts, output: Some(output) }
                }
                Err(err) => return map_err(err),
            }
        }
        mail::OperationClass::DownloadAttachment | mail::OperationClass::UpdateDraft => {
            return (
                MailExecutionResult::default(),
                Some(format!(
                    "unsupported mail action {:?}",
                    action.operation_class.as_str()
                )),
            );
        }
    };
    (result, None)
}

/// Go executeWorkflowMailStep — run the mail action and complete/fail the
/// runtime tool call. Billing quota reservation is skipped (not wired).
fn execute_workflow_mail_step(
    state: &AppState,
    workflow: &orchestration::Workflow,
    runtime_step: &runtime::Step,
    wf_step: &orchestration::WorkflowStep,
) -> Result<(runtime::ToolCall, orchestration::StepStatus, String), ApiError> {
    let action = decode_mail_action(wf_step.input.as_ref())?;
    let runtime_manager = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::internal("runtime manager is not configured"))?;
    let tool_call = runtime_manager
        .create_tool_call(
            &workflow.run_id,
            &runtime_step.step_id,
            runtime::CreateToolCallInput {
                workflow_id: workflow.workflow_id.clone(),
                workflow_step_id: wf_step.workflow_step_id.clone(),
                attempt: wf_step.attempt_count + 1,
                invocation_kind: runtime::ToolCallInvocationKind::DomainTool.as_str().to_string(),
                domain_kind: "mail".to_string(),
                tool_name: action.operation_class.as_str().to_string(),
                input: serde_json::to_value(&action).ok(),
                ..runtime::CreateToolCallInput::default()
            },
        )
        .map_err(ApiError::internal)?;
    persist_tool_call(state, &tool_call)?;
    publish_tool_call_event(state, "tool_call.requested", &workflow.run_id, &runtime_step.step_id, &tool_call)?;

    let (result, exec_err) = execute_mail_action(
        state,
        &action,
        mail::SourceLinkage {
            run_id: workflow.run_id.clone(),
            step_id: runtime_step.step_id.clone(),
            tool_call_id: tool_call.tool_call_id.clone(),
            workflow_id: workflow.workflow_id.clone(),
            workflow_step_id: wf_step.workflow_step_id.clone(),
            schedule_id: workflow.schedule_id.clone(),
            schedule_attempt_id: workflow.schedule_attempt_id.clone(),
            allow_send_side_effects: action.allow_send_side_effects,
            ..mail::SourceLinkage::default()
        },
    );

    if !result.operation.operation_id.is_empty() {
        record_mail_activity(state, &result.account, &result.operation, &result.artifacts)?;
    }

    let bindings = calendar_integration_bindings(
        state,
        orchestration::first_non_empty(&[
            &result.operation.integration_id,
            &action.integration_id,
        ]),
    );
    let output = mail_tool_call_output(&result);

    if let Some(exec_err) = exec_err {
        let tool_call = runtime_manager
            .fail_tool_call(
                &workflow.run_id,
                &runtime_step.step_id,
                &tool_call.tool_call_id,
                runtime::FailToolCallInput {
                    output: Some(output),
                    error: exec_err.clone(),
                    failure_class: mail_failure_class_for_string(&exec_err).to_string(),
                    integration_bindings: bindings,
                    ..runtime::FailToolCallInput::default()
                },
            )
            .map_err(ApiError::internal)?;
        persist_tool_call(state, &tool_call)?;
        publish_tool_call_event(state, "tool_call.failed", &workflow.run_id, &runtime_step.step_id, &tool_call)?;
        return Ok((tool_call, orchestration::StepStatus::Failed, String::new()));
    }

    let tool_call = runtime_manager
        .complete_tool_call(
            &workflow.run_id,
            &runtime_step.step_id,
            &tool_call.tool_call_id,
            runtime::CompleteToolCallInput {
                output: Some(output),
                integration_bindings: bindings,
                ..runtime::CompleteToolCallInput::default()
            },
        )
        .map_err(ApiError::internal)?;
    persist_tool_call(state, &tool_call)?;
    publish_tool_call_event(state, "tool_call.completed", &workflow.run_id, &runtime_step.step_id, &tool_call)?;
    Ok((tool_call, orchestration::StepStatus::Completed, String::new()))
}

fn mail_failure_class_for_string(err: &str) -> &'static str {
    if err.contains("integration is unavailable") {
        "mail_unavailable"
    } else if err.contains("integration not found") {
        "integration_not_found"
    } else if err.contains("selection is invalid") {
        "selection_invalid"
    } else if err.contains("thread not found") {
        "thread_not_found"
    } else if err.contains("message not found") {
        "message_not_found"
    } else if err.contains("draft not found") {
        "draft_not_found"
    } else if err.contains("recipient") {
        "recipient_required"
    } else if err.contains("attachment") {
        "attachment_unresolved"
    } else if err.contains("background") {
        "send_permission_required"
    } else {
        "mail_execution_failed"
    }
}

fn decode_mail_action(input: Option<&serde_json::Value>) -> Result<mail::Action, ApiError> {
    let Some(input) = input else {
        return Err(ApiError::internal("mail workflow step input is missing"));
    };
    serde_json::from_value(input.clone()).map_err(ApiError::internal)
}

fn record_mail_activity(
    state: &AppState,
    account: &mail::AccountProjection,
    operation: &mail::Operation,
    artifacts: &[mail::Artifact],
) -> Result<(), ApiError> {
    let store = state.store.lock();
    if !account.integration_id.is_empty() {
        store.upsert_mail_account(account).map_err(ApiError::from_store)?;
    }
    if operation.operation_id.is_empty() {
        return Ok(());
    }
    store.upsert_mail_operation(operation).map_err(ApiError::from_store)?;
    for artifact in artifacts {
        store.upsert_mail_artifact(artifact).map_err(ApiError::from_store)?;
    }
    Ok(())
}

/// Go executeWorkflowMCPTool — authorize, create the tool call, invoke the MCP
/// tool and complete/fail the call.
fn execute_workflow_mcp_tool(
    state: &AppState,
    workflow: &orchestration::Workflow,
    runtime_step: &runtime::Step,
    wf_step: &orchestration::WorkflowStep,
) -> Result<(runtime::ToolCall, orchestration::StepStatus, String), ApiError> {
    let mcp_manager = state
        .mcp
        .as_ref()
        .ok_or_else(|| ApiError::internal("mcp manager is not configured"))?;
    let runtime_manager = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::internal("runtime manager is not configured"))?;

    let authorization = mcp_manager
        .authorize_tool(
            &wf_step.consumer_id,
            &wf_step.tool_name,
            &mcp::AuthorizeToolInput {
                runtime_surface: "chat".to_string(),
                requested_by: format!("workflow:{}", workflow.workflow_id),
                ..mcp::AuthorizeToolInput::default()
            },
        )
        .map_err(ApiError::internal)?;

    if authorization.status != mcp::ToolAuthorizationStatus::Allowed {
        let step_status = orchestration::StepStatus::Blocked;
        let blocked_reason = if authorization.status == mcp::ToolAuthorizationStatus::Rejected
            || authorization.status == mcp::ToolAuthorizationStatus::Pending
        {
            orchestration::BlockedReason::ApprovalDenied.as_str().to_string()
        } else {
            orchestration::BlockedReason::PolicyBlocked.as_str().to_string()
        };
        let tool_call = runtime_manager
            .create_tool_call(
                &workflow.run_id,
                &runtime_step.step_id,
                runtime::CreateToolCallInput {
                    workflow_id: workflow.workflow_id.clone(),
                    workflow_step_id: wf_step.workflow_step_id.clone(),
                    attempt: wf_step.attempt_count + 1,
                    invocation_kind: runtime::ToolCallInvocationKind::McpTool.as_str().to_string(),
                    mcp_server_id: wf_step.consumer_id.clone(),
                    mcp_tool_name: wf_step.tool_name.clone(),
                    tool_name: wf_step.tool_name.clone(),
                    authorization_result: authorization.status.as_str().to_string(),
                    input: wf_step.input.clone(),
                    ..runtime::CreateToolCallInput::default()
                },
            )
            .map_err(ApiError::internal)?;
        persist_tool_call(state, &tool_call)?;
        return Ok((tool_call, step_status, blocked_reason));
    }

    let Some(server) = mcp_manager.get_server_resource(&wf_step.consumer_id) else {
        return Err(ApiError::internal("mcp server not found"));
    };

    let mut tool_call = runtime_manager
        .create_tool_call(
            &workflow.run_id,
            &runtime_step.step_id,
            runtime::CreateToolCallInput {
                workflow_id: workflow.workflow_id.clone(),
                workflow_step_id: wf_step.workflow_step_id.clone(),
                attempt: wf_step.attempt_count + 1,
                invocation_kind: runtime::ToolCallInvocationKind::McpTool.as_str().to_string(),
                mcp_server_id: server.server.server_id.clone(),
                mcp_server_name: server.server.display_name.clone(),
                mcp_tool_name: wf_step.tool_name.clone(),
                mcp_transport_kind: server.server.transport_kind.as_str().to_string(),
                mcp_session_id: authorization.session_id.clone(),
                authorization_result: authorization.status.as_str().to_string(),
                tool_name: wf_step.tool_name.clone(),
                input: wf_step.input.clone(),
                sandbox: authorization
                    .sandbox
                    .as_ref()
                    .map(consumer_view_map)
                    .unwrap_or_default(),
                ..runtime::CreateToolCallInput::default()
            },
        )
        .map_err(ApiError::internal)?;
    persist_tool_call(state, &tool_call)?;
    publish_tool_call_event(state, "tool_call.requested", &workflow.run_id, &runtime_step.step_id, &tool_call)?;

    let input_value = wf_step.input.clone().unwrap_or(serde_json::Value::Null);
    let result = mcp_manager
        .call_tool(&wf_step.consumer_id, &wf_step.tool_name, input_value, &authorization)
        .map_err(ApiError::internal)?;

    let mut output = serde_json::Map::new();
    output.insert(
        "transportKind".to_string(),
        serde_json::json!(server.server.transport_kind.as_str()),
    );
    output.insert("sessionId".to_string(), serde_json::json!(result.session_id));
    if let Some(result_output) = &result.output {
        output.insert("result".to_string(), result_output.clone());
    }
    let output = serde_json::Value::Object(output);

    if result.failure_class.trim().is_empty() {
        tool_call = runtime_manager
            .complete_tool_call(
                &workflow.run_id,
                &runtime_step.step_id,
                &tool_call.tool_call_id,
                runtime::CompleteToolCallInput {
                    output: Some(output),
                    sandbox: authorization
                        .sandbox
                        .as_ref()
                        .map(consumer_view_map)
                        .unwrap_or_default(),
                    ..runtime::CompleteToolCallInput::default()
                },
            )
            .map_err(ApiError::internal)?;
        persist_tool_call(state, &tool_call)?;
        publish_tool_call_event(state, "tool_call.completed", &workflow.run_id, &runtime_step.step_id, &tool_call)?;
        Ok((tool_call, orchestration::StepStatus::Completed, String::new()))
    } else {
        tool_call = runtime_manager
            .fail_tool_call(
                &workflow.run_id,
                &runtime_step.step_id,
                &tool_call.tool_call_id,
                runtime::FailToolCallInput {
                    output: Some(output),
                    error: result.error.clone(),
                    failure_class: result.failure_class.clone(),
                    sandbox: authorization
                        .sandbox
                        .as_ref()
                        .map(consumer_view_map)
                        .unwrap_or_default(),
                    ..runtime::FailToolCallInput::default()
                },
            )
            .map_err(ApiError::internal)?;
        persist_tool_call(state, &tool_call)?;
        publish_tool_call_event(state, "tool_call.failed", &workflow.run_id, &runtime_step.step_id, &tool_call)?;
        Ok((tool_call, orchestration::StepStatus::Failed, String::new()))
    }
}

/// Serializes a ConsumerContractView to the tool call sandbox map (Go
/// consumerViewMap).
fn consumer_view_map(view: &kura_sandbox::ConsumerContractView) -> serde_json::Map<String, serde_json::Value> {
    serde_json::to_value(view)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

/// Go executeWorkflowSkillTool / executeWorkflowCapabilityTool: the sandbox
/// preparation pipeline is not ported. The runtime tool call is created and
/// immediately failed as consumer_unavailable, which apply_tool_call_result
/// maps to a Blocked step with BlockedReason::ConsumerUnavailable — the same
/// shape the Go MCP authorize path produces for a blocked consumer.
fn execute_workflow_capability_tool(
    state: &AppState,
    workflow: &orchestration::Workflow,
    runtime_step: &runtime::Step,
    wf_step: &orchestration::WorkflowStep,
) -> Result<(runtime::ToolCall, bool, orchestration::StepStatus, String), ApiError> {
    let runtime_manager = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::internal("runtime manager is not configured"))?;
    let invocation_kind = if wf_step.consumer_kind == "skill" {
        runtime::ToolCallInvocationKind::Skill.as_str().to_string()
    } else {
        runtime::ToolCallInvocationKind::LocalTool.as_str().to_string()
    };
    let mut tool_call = runtime_manager
        .create_tool_call(
            &workflow.run_id,
            &runtime_step.step_id,
            runtime::CreateToolCallInput {
                workflow_id: workflow.workflow_id.clone(),
                workflow_step_id: wf_step.workflow_step_id.clone(),
                attempt: wf_step.attempt_count + 1,
                invocation_kind,
                capability_id: wf_step.consumer_id.clone(),
                skill_id: wf_step.consumer_id.clone(),
                tool_name: wf_step.tool_name.clone(),
                input: wf_step.input.clone(),
                ..runtime::CreateToolCallInput::default()
            },
        )
        .map_err(ApiError::internal)?;

    let blocked_reason = orchestration::BlockedReason::ConsumerUnavailable.as_str().to_string();
    tool_call = runtime_manager
        .fail_tool_call(
            &workflow.run_id,
            &runtime_step.step_id,
            &tool_call.tool_call_id,
            runtime::FailToolCallInput {
                output: wf_step.input.clone(),
                error: "workflow consumer execution is not yet ported".to_string(),
                failure_class: "consumer_unavailable".to_string(),
                ..runtime::FailToolCallInput::default()
            },
        )
        .map_err(ApiError::internal)?;
    persist_tool_call(state, &tool_call)?;
    publish_tool_call_event(state, "tool_call.failed", &workflow.run_id, &runtime_step.step_id, &tool_call)?;
    Ok((
        tool_call,
        true,
        orchestration::StepStatus::Blocked,
        blocked_reason,
    ))
}

/// Go executeWorkflowComputerUseStep — acquire a computer-use session and run
/// the declared actions, projecting session/actions/artifacts onto the step.
fn execute_workflow_computer_use_step(
    state: &AppState,
    workflow: orchestration::Workflow,
    wf_step: &orchestration::WorkflowStep,
) -> Result<(orchestration::Workflow, bool), ApiError> {
    let computer_use_manager = state
        .computer_use
        .as_ref()
        .ok_or_else(|| ApiError::internal("computer-use manager is not configured"))?;

    let input_value = wf_step
        .input
        .as_ref()
        .ok_or_else(|| ApiError::internal("computer-use workflow input must be an object"))?;
    let Some(input_map) = input_value.as_object() else {
        return Err(ApiError::internal("computer-use workflow input must be an object"));
    };

    let (session, _) = computer_use_manager
        .acquire_session(
            &workflow.run_id,
            computeruse::CreateSessionInput {
                workflow_id: workflow.workflow_id.clone(),
                workflow_step_id: wf_step.workflow_step_id.clone(),
                driver_kind: string_field(input_map, "driverKind"),
                initial_url: string_field(input_map, "initialUrl"),
            },
        )
        .map_err(ApiError::internal)?;

    let mut workflow = orchestration::apply_computer_use_projection(
        workflow,
        &wf_step.workflow_step_id,
        &session.computer_use_session_id,
        &[],
        &[],
        Utc::now(),
    );
    persist_workflow_detail(state, &workflow).map_err(ApiError::from_store)?;

    let action_payloads = input_map
        .get("actions")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    if action_payloads.is_empty() {
        return Err(ApiError::internal(
            "computer-use workflow step requires at least one action",
        ));
    }

    let mut action_ids: Vec<String> = Vec::new();
    let mut artifacts: Vec<computeruse::Artifact> = Vec::new();
    let mut last_action: Option<runtime::ToolCall> = None;
    let mut last_step_id = String::new();
    let runtime_manager = state.runtime.as_ref();

    for (idx, raw) in action_payloads.iter().enumerate() {
        let action_input = decode_workflow_computer_use_action(raw);
        let (result, approval, decision) = computer_use_manager
            .create_action(
                &workflow.run_id,
                &session.computer_use_session_id,
                &format!("workflow:{}", workflow.workflow_id),
                action_input,
            )
            .map_err(ApiError::internal)?;

        if idx == 0 {
            workflow = orchestration::start_step_attempt(
                workflow,
                &wf_step.workflow_step_id,
                &result.action.step_id,
                Utc::now(),
            );
        }
        action_ids.push(result.action.computer_use_action_id.clone());
        artifacts.extend(result.action.artifacts.clone());
        workflow = orchestration::apply_computer_use_projection(
            workflow,
            &wf_step.workflow_step_id,
            &session.computer_use_session_id,
            &action_ids,
            &artifacts,
            Utc::now(),
        );

        // Go persistComputerUseRuntimeTracking: persist the runtime step/tool
        // call the computer-use manager created.
        if let Some(manager) = runtime_manager {
            if let Some(step) = manager.get_step(&result.action.run_id, &result.action.step_id) {
                persist_step(state, &step).map_err(ApiError::from_store)?;
            }
            if let Some(tool_call) = manager.get_tool_call(
                &result.action.run_id,
                &result.action.step_id,
                &result.action.tool_call_id,
            ) {
                persist_tool_call(state, &tool_call)?;
            }
        }

        for artifact in &result.action.artifacts {
            publish_computer_use_artifact_event(state, &result.action, artifact)?;
        }
        if result.action.failure_class == computeruse::FailureClass::TargetMismatch.as_str() {
            publish_computer_use_target_mismatch(state, &result.action)?;
        }
        if let Some(approval) = approval {
            state
                .store
                .lock()
                .upsert_approval(&approval)
                .map_err(ApiError::from_store)?;
        }
        if let Some(decision) = decision {
            state
                .store
                .lock()
                .upsert_decision(&decision)
                .map_err(ApiError::from_store)?;
        }

        let tool_call = runtime_manager.and_then(|manager| {
            manager.get_tool_call(
                &result.action.run_id,
                &result.action.step_id,
                &result.action.tool_call_id,
            )
        });
        if let Some(tool_call) = tool_call {
            last_action = Some(tool_call.clone());
            last_step_id = result.action.step_id.clone();
            workflow = orchestration::bind_tool_call(
                workflow,
                &wf_step.workflow_step_id,
                &tool_call,
                Utc::now(),
            );
        }

        if result.pending {
            persist_workflow_detail(state, &workflow).map_err(ApiError::from_store)?;
            if let Some(tool_call) = last_action {
                return advance_workflow_after_tool_call(
                    state,
                    workflow,
                    &tool_call,
                    Some(orchestration::StepStatus::Blocked),
                    orchestration::BlockedReason::ApprovalDenied.as_str(),
                );
            }
            return Ok((workflow, false));
        }
    }

    if last_step_id.is_empty() {
        return Err(ApiError::internal(
            "computer-use workflow step did not create runtime linkage",
        ));
    }
    persist_workflow_detail(state, &workflow).map_err(ApiError::from_store)?;
    let tool_call = last_action.ok_or_else(|| {
        ApiError::internal("computer-use workflow step did not create runtime linkage")
    })?;
    advance_workflow_after_tool_call(
        state,
        workflow,
        &tool_call,
        Some(orchestration::StepStatus::Completed),
        "",
    )
}

/// Deserializes a string_enum value from its wire string (the string_enum
/// macro does not generate FromStr); unknown values fall back to the
/// default variant like the Go zero-value fallback.
fn deserialize_enum<T: serde::de::DeserializeOwned + Default>(raw: &str) -> T {
    serde_json::from_str(&format!("\"{}\"", raw.trim())).unwrap_or_default()
}

fn string_field(map: &serde_json::Map<String, serde_json::Value>, key: &str) -> String {
    map.get(key)
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string()
}

/// Go decodeWorkflowComputerUseAction.
fn decode_workflow_computer_use_action(payload: &serde_json::Value) -> computeruse::CreateActionInput {
    let map = payload.as_object().cloned().unwrap_or_default();
    computeruse::CreateActionInput {
        action_kind: deserialize_enum::<computeruse::ActionKind>(&string_field(&map, "actionKind")),
        url: string_field(&map, "url"),
        value: string_field(&map, "value"),
        selected_value: string_field(&map, "selectedValue"),
        page_target: string_field(&map, "pageTarget"),
        rationale: string_field(&map, "rationale"),
        ..computeruse::CreateActionInput::default()
    }
}

fn publish_computer_use_artifact_event(
    state: &AppState,
    action: &computeruse::Action,
    artifact: &computeruse::Artifact,
) -> Result<(), ApiError> {
    let mut payload = serde_json::Map::new();
    payload.insert("artifactId".to_string(), serde_json::json!(artifact.artifact_id));
    payload.insert("artifactKind".to_string(), serde_json::json!(artifact.kind.as_str()));
    payload.insert("captureStatus".to_string(), serde_json::json!(artifact.status.as_str()));
    let event = events::Event {
        category: "capability".to_string(),
        name: "computer_use.artifact_recorded".to_string(),
        environment_scope: environment_scope_from_config(&state.config),
        scope: events::Scope {
            run_id: action.run_id.clone(),
            computer_use_session_id: action.computer_use_session_id.clone(),
            computer_use_action_id: action.computer_use_action_id.clone(),
            ..events::Scope::default()
        },
        resource: events::Resource {
            kind: "computer_use_artifact".to_string(),
            id: artifact.artifact_id.clone(),
        },
        payload,
        ..events::Event::default()
    };
    let stored = state
        .store
        .lock()
        .append_event(&event)
        .map_err(ApiError::from_store)?;
    state.event_bus.publish(stored);
    Ok(())
}

fn publish_computer_use_target_mismatch(
    state: &AppState,
    action: &computeruse::Action,
) -> Result<(), ApiError> {
    let mut payload = serde_json::Map::new();
    payload.insert("status".to_string(), serde_json::json!(action.status.as_str()));
    payload.insert("failureClass".to_string(), serde_json::json!(action.failure_class));
    payload.insert(
        "computerUseSessionId".to_string(),
        serde_json::json!(action.computer_use_session_id),
    );
    payload.insert(
        "computerUseActionId".to_string(),
        serde_json::json!(action.computer_use_action_id),
    );
    let event = events::Event {
        category: "capability".to_string(),
        name: "computer_use.action_target_mismatch".to_string(),
        environment_scope: environment_scope_from_config(&state.config),
        scope: events::Scope {
            run_id: action.run_id.clone(),
            step_id: action.step_id.clone(),
            computer_use_session_id: action.computer_use_session_id.clone(),
            computer_use_action_id: action.computer_use_action_id.clone(),
            ..events::Scope::default()
        },
        resource: events::Resource {
            kind: "computer_use_action".to_string(),
            id: action.computer_use_action_id.clone(),
        },
        payload,
        ..events::Event::default()
    };
    let stored = state
        .store
        .lock()
        .append_event(&event)
        .map_err(ApiError::from_store)?;
    state.event_bus.publish(stored);
    Ok(())
}

// ---------------------------------------------------------------------------
// Tool-call result application (Go advanceWorkflowAfterToolCall)
// ---------------------------------------------------------------------------

/// Applies a tool call outcome to the workflow, syncs the runtime step
/// status, publishes step/status events and recurses into
/// advance_workflow_execution (Go advanceWorkflowAfterToolCall).
pub(crate) fn advance_workflow_after_tool_call(
    state: &AppState,
    workflow: orchestration::Workflow,
    tool_call: &runtime::ToolCall,
    hinted_status: Option<orchestration::StepStatus>,
    blocked_reason: &str,
) -> Result<(orchestration::Workflow, bool), ApiError> {
    let previous_status = workflow.status;
    let workflow = orchestration::apply_tool_call_result(
        workflow,
        tool_call,
        hinted_status,
        blocked_reason,
        Utc::now(),
    );

    let step = orchestration::workflow_step_by_id(&workflow, &tool_call.workflow_step_id);
    let runtime_manager = state.runtime.as_ref();
    if let Some(step) = step {
        match step.status {
            orchestration::StepStatus::Completed => {
                if let Some(manager) = runtime_manager {
                    if let Ok((updated_step, run_update)) = manager.update_step_status_and_reconcile_run(
                        &workflow.run_id,
                        &tool_call.step_id,
                        runtime::UpdateStepStatusInput {
                            status: runtime::StepStatus::Completed,
                            output: tool_call.output.clone(),
                        },
                    ) {
                        let _ = persist_step(state, &updated_step);
                        if let Some(run_update) = run_update {
                            let _ = persist_run(state, &run_update);
                        }
                    }
                }
            }
            orchestration::StepStatus::Cancelled => {
                if let Some(manager) = runtime_manager {
                    if let Ok((updated_step, run_update, _)) =
                        manager.cancel_step(&workflow.run_id, &tool_call.step_id)
                    {
                        let _ = persist_step_cancel_mutation(state, &updated_step, run_update.as_ref());
                    }
                }
            }
            orchestration::StepStatus::Failed => {
                if let Some(manager) = runtime_manager {
                    if let Ok((updated_step, run_update)) = manager.update_step_status_and_reconcile_run(
                        &workflow.run_id,
                        &tool_call.step_id,
                        runtime::UpdateStepStatusInput {
                            status: runtime::StepStatus::Failed,
                            output: tool_call.output.clone(),
                        },
                    ) {
                        let _ = persist_step(state, &updated_step);
                        if let Some(run_update) = run_update {
                            let _ = persist_run(state, &run_update);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    persist_workflow_detail(state, &workflow).map_err(ApiError::from_store)?;

    if !tool_call.workflow_step_id.is_empty() {
        let mut extra = serde_json::Map::new();
        extra.insert(
            "workflowStepId".to_string(),
            serde_json::json!(tool_call.workflow_step_id),
        );
        let _ = publish_workflow_event(
            state,
            "workflow.step_status_changed",
            &workflow,
            Some(tool_call),
            Some(&extra),
        );
    }
    if workflow.status != previous_status {
        let _ = publish_workflow_event(
            state,
            "workflow.status_changed",
            &workflow,
            Some(tool_call),
            None,
        );
        if let Err(err) = maybe_emit_workflow_delivery(state, &workflow) {
            return Err(err);
        }
    }

    let next = advance_workflow_execution(state, workflow)?;
    Ok((next, true))
}

/// Go maybeEmitWorkflowDelivery — emit a delivery outcome when a session-less
/// background run's workflow reaches a terminal state.
fn maybe_emit_workflow_delivery(
    state: &AppState,
    workflow: &orchestration::Workflow,
) -> Result<(), ApiError> {
    let Some(delivery_manager) = state.delivery.as_ref() else {
        return Ok(());
    };
    if !orchestration::is_terminal_workflow_status(workflow.status) {
        return Ok(());
    }
    let runtime_manager = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::internal("runtime manager is not configured"))?;
    let Some(run) = runtime_manager.get_run(&workflow.run_id) else {
        return Ok(());
    };
    if !run.session_id.is_empty() {
        return Ok(());
    }

    let result_class = match workflow.status {
        orchestration::WorkflowStatus::Completed => kura_delivery::ResultClass::RoutineSuccess,
        orchestration::WorkflowStatus::Cancelled => kura_delivery::ResultClass::Urgent,
        _ => kura_delivery::ResultClass::Failure,
    };
    let preview = {
        let trimmed = workflow.goal.trim().to_string();
        if trimmed.is_empty() {
            "background workflow reached terminal state".to_string()
        } else {
            trimmed
        }
    };
    let integration_id = resolve_workflow_integration_id(workflow);

    let outcome = delivery_manager
        .emit_outcome(kura_delivery::OutcomeInput {
            source_kind: "workflow".to_string(),
            source_id: workflow.workflow_id.clone(),
            run_id: workflow.run_id.clone(),
            workflow_id: workflow.workflow_id.clone(),
            schedule_id: workflow.schedule_id.clone(),
            schedule_attempt_id: workflow.schedule_attempt_id.clone(),
            integration_id,
            result_class,
            payload_preview: preview,
        })
        .map_err(ApiError::internal)?;

    link_workflow_calendar_operations_to_delivery(state, workflow, &outcome.delivery_id)?;
    link_workflow_mail_operations_to_delivery(state, workflow, &outcome.delivery_id)
}

/// Go resolveWorkflowIntegrationID.
fn resolve_workflow_integration_id(workflow: &orchestration::Workflow) -> String {
    for step in &workflow.steps {
        for binding in &step.integration_bindings {
            if !binding.integration_id.trim().is_empty() {
                return binding.integration_id.trim().to_string();
            }
        }
    }
    String::new()
}

/// Go linkWorkflowCalendarOperationsToDelivery.
fn link_workflow_calendar_operations_to_delivery(
    state: &AppState,
    workflow: &orchestration::Workflow,
    delivery_id: &str,
) -> Result<(), ApiError> {
    if delivery_id.trim().is_empty() {
        return Ok(());
    }
    let store = state.store.lock();
    let filter = kura_store::calendar::CalendarOperationFilter {
        workflow_id: workflow.workflow_id.clone(),
        ..kura_store::calendar::CalendarOperationFilter::default()
    };
    let operations = store
        .list_calendar_operations(&workflow.environment_scope, &filter)
        .map_err(ApiError::from_store)?;
    for mut item in operations {
        if item.delivery_id.trim() == delivery_id.trim() {
            continue;
        }
        item.delivery_id = delivery_id.trim().to_string();
        item.updated_at = Utc::now();
        store.upsert_calendar_operation(&item).map_err(ApiError::from_store)?;
        if let Some(calendar_manager) = state.calendar.as_ref() {
            calendar_manager.store_operation(item);
        }
    }
    Ok(())
}

/// Go linkWorkflowMailOperationsToDelivery.
fn link_workflow_mail_operations_to_delivery(
    state: &AppState,
    workflow: &orchestration::Workflow,
    delivery_id: &str,
) -> Result<(), ApiError> {
    if delivery_id.trim().is_empty() {
        return Ok(());
    }
    let store = state.store.lock();
    let filter = kura_store::mail::MailOperationFilter {
        workflow_id: workflow.workflow_id.clone(),
        ..kura_store::mail::MailOperationFilter::default()
    };
    let operations = store
        .list_mail_operations(&workflow.environment_scope, &filter)
        .map_err(ApiError::from_store)?;
    for mut item in operations {
        if item.delivery_id.trim() == delivery_id.trim() {
            continue;
        }
        item.delivery_id = delivery_id.trim().to_string();
        item.updated_at = Utc::now();
        store.upsert_mail_operation(&item).map_err(ApiError::from_store)?;
        if let Some(mail_manager) = state.mail.as_ref() {
            mail_manager.store_operation(item);
        }
    }
    Ok(())
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use axum::body::to_bytes;
    use axum::http::{Method, Request, StatusCode};
    use parking_lot::Mutex;
    use tower::ServiceExt;
    use uuid::Uuid;

    fn test_config() -> kura_config::Config {
        kura_config::Config {
            project_root: String::new(),
            environment: kura_config::Environment::Test,
            bind_addr: "127.0.0.1:19192".to_string(),
            data_dir: "/tmp/kura-api-workflows-test".to_string(),
            log_level: "info".to_string(),
            version: "0.1.0".to_string(),
            llm: kura_config::LlmConfig::default(),
            connectors: kura_config::ConnectorConfig {
                discord: kura_config::DiscordConnectorConfig { enabled: false, ..Default::default() },
                telegram: kura_config::TelegramConnectorConfig { enabled: false, ..Default::default() },
                slack: kura_config::SlackConnectorConfig { enabled: false, ..Default::default() },
                matrix: kura_config::MatrixConnectorConfig { enabled: false, ..Default::default() },
            },
        }
    }

    fn temp_store() -> Arc<Mutex<SQLiteStore>> {
        let dir = std::env::temp_dir().join(format!("kura-api-workflows-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        Arc::new(Mutex::new(SQLiteStore::new(dir.to_str().expect("path")).expect("store")))
    }

    fn test_state_with_runtime() -> (AppState, Arc<kura_runtime::Manager>) {
        let runtime = Arc::new(kura_runtime::Manager::new());
        let state = AppState::new(test_config(), Arc::new(kura_events::Bus::new()), temp_store());
        let mut state = state;
        state.runtime = Some(runtime.clone());
        (state, runtime)
    }

    /// Mirrors the Go newAllowSkillRegistryForWorkflowTest fixture: a data
    /// root with one executable allow-mode skill so planning picks a skill step.
    fn skill_registry(data_root: &str) -> kura_skills::Registry {
        let home_root = std::env::temp_dir().join(format!("kura-home-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&home_root).expect("mkdir home");
        std::fs::create_dir_all(format!("{data_root}/skills/exec-skill")).expect("mkdir skill");
        std::fs::write(
            format!("{data_root}/skills/exec-skill/SKILL.md"),
            "---\nname: exec-skill\ndescription: executable skill\nexecution.entrypoint: scripts/run.sh\nexecution.working_dir: .\nexecution.profile_id: subprocess_default\nexecution.read_roots: .\nexecution.write_roots: .\nexecution.network_mode: deny\nexecution.timeout_ms: 5000\nexecution.approval_mode: allow\n---\nworkflow test skill\n",
        )
        .expect("write skill");
        std::fs::create_dir_all(format!("{data_root}/skills/exec-skill/scripts")).expect("mkdir scripts");
        std::fs::write(
            format!("{data_root}/skills/exec-skill/scripts/run.sh"),
            "#!/bin/sh\nprintf 'workflow-ok %s' \"$1\"\n",
        )
        .expect("write entrypoint");
        kura_skills::Registry::with_roots(
            home_root.to_str().expect("path"),
            data_root,
        )
        .expect("registry")
    }

    fn run_with_entrypoint(state: &AppState, runtime: &Arc<kura_runtime::Manager>) -> kura_runtime::Run {
        let run = runtime
            .create_run(kura_runtime::CreateRunInput {
                entrypoint: "operator".to_string(),
                goal: "Use a skill to complete a deterministic workflow.".to_string(),
                ..kura_runtime::CreateRunInput::default()
            })
            .expect("create run");
        let _ = state.store.lock().upsert_run(&run);
        run
    }

    async fn request(
        app: &Router,
        method: Method,
        uri: &str,
        body: Option<&str>,
    ) -> (StatusCode, serde_json::Value) {
        let body = body.unwrap_or("");
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .expect("request");
        let response = app.clone().oneshot(request).await.expect("oneshot");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.expect("body");
        let json: serde_json::Value = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("json body")
        };
        (status, json)
    }

    #[tokio::test]
    async fn planning_routes_expose_inspectable_plan_and_environment_isolation() {
        let (mut state, runtime) = test_state_with_runtime();
        let data_root = std::env::temp_dir()
            .join(format!("kura-workflow-data-{}", Uuid::now_v7()))
            .to_str()
            .expect("path")
            .to_string();
        std::fs::create_dir_all(&data_root).expect("mkdir data root");
        let registry = skill_registry(&data_root);
        state.skills = Some(Arc::new(registry));
        let run = run_with_entrypoint(&state, &runtime);
        let app = router().with_state(state.clone());

        // A prod-environment workflow must be invisible to the test scope list.
        let prod_workflow = orchestration::Workflow {
            workflow_id: "wf_prod_hidden".to_string(),
            run_id: run.run_id.clone(),
            environment_scope: "prod".to_string(),
            goal: "hidden".to_string(),
            status: orchestration::WorkflowStatus::Planned,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            ..orchestration::Workflow::default()
        };
        state
            .store
            .lock()
            .upsert_workflow(&prod_workflow)
            .expect("upsert prod workflow");

        // POST create -> 201 planned with one inspect-only skill step.
        let (status, created) = request(
            &app,
            Method::POST,
            &format!("/v1/runs/{}/workflows", run.run_id),
            Some("{}"),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "body={created}");
        assert_eq!(created["status"], "planned");
        assert_eq!(created["runId"], run.run_id);
        assert_eq!(created["steps"].as_array().map(Vec::len), Some(1));
        assert!(created["steps"][0]["runtimeStepId"].is_null(), "body={created}");
        assert!(created["steps"][0]["activeToolCallId"].is_null(), "body={created}");
        assert!(created["steps"][0]["selectionRationale"].as_str().map(str::len).unwrap_or(0) > 0);
        let workflow_id = created["workflowId"].as_str().expect("workflowId").to_string();

        // GET list -> only the test-environment workflow.
        let (status, list) = request(
            &app,
            Method::GET,
            &format!("/v1/runs/{}/workflows", run.run_id),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(list["items"].as_array().map(Vec::len), Some(1), "body={list}");

        // GET by id -> environment scope + inspectable planning truth.
        let (status, got) = request(
            &app,
            Method::GET,
            &format!("/v1/runs/{}/workflows/{workflow_id}", run.run_id),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(got["environmentScope"], "test");
        assert!(got["planSummary"].as_str().map(str::len).unwrap_or(0) > 0);

        // Unknown workflow -> 404.
        let (status, _) = request(
            &app,
            Method::GET,
            &format!("/v1/runs/{}/workflows/wf_nope", run.run_id),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // Missing run -> 404.
        let (status, _) = request(
            &app,
            Method::POST,
            "/v1/runs/run_nope/workflows",
            Some("{}"),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn create_validates_calendar_and_mail_action_builders() {
        let (state, runtime) = test_state_with_runtime();
        let run = run_with_entrypoint(&state, &runtime);
        let app = router().with_state(state);

        // Empty body -> 400 (Go "request body is required").
        let (status, body) = request(
            &app,
            Method::POST,
            &format!("/v1/runs/{}/workflows", run.run_id),
            Some(""),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "body={body}");

        // Malformed JSON -> 400.
        let (status, _) = request(
            &app,
            Method::POST,
            &format!("/v1/runs/{}/workflows", run.run_id),
            Some("{" ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // Missing calendar action operationClass -> 400.
        let (status, body) = request(
            &app,
            Method::POST,
            &format!("/v1/runs/{}/workflows", run.run_id),
            Some(r#"{"calendarAction":{}}"#),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "body={body}");

        // Missing mail action operationClass -> 400.
        let (status, body) = request(
            &app,
            Method::POST,
            &format!("/v1/runs/{}/workflows", run.run_id),
            Some(r#"{"mailAction":{}}"#),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "body={body}");

        // Invalid calendar windowStart -> 400 with parse error message.
        let (status, body) = request(
            &app,
            Method::POST,
            &format!("/v1/runs/{}/workflows", run.run_id),
            Some(r#"{"calendarAction":{"operationClass":"list_events","windowStart":"not-a-time"}}"#),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "body={body}");
        assert!(body["error"].as_str().unwrap_or("").contains("windowStart"));

        // A calendar action with a valid operationClass plans a calendar step.
        let (status, created) = request(
            &app,
            Method::POST,
            &format!("/v1/runs/{}/workflows", run.run_id),
            Some(r#"{"calendarAction":{"operationClass":"list_events"}}"#),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "body={created}");
        assert_eq!(created["steps"][0]["consumerKind"], "calendar");
    }

    #[tokio::test]
    async fn start_workflow_validates_status_and_missing_workflow() {
        let (state, runtime) = test_state_with_runtime();
        let run = run_with_entrypoint(&state, &runtime);
        let app = router().with_state(state.clone());

        // Missing workflow -> 404.
        let (status, _) = request(
            &app,
            Method::POST,
            &format!("/v1/runs/{}/workflows/wf_nope/start", run.run_id),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // Planning-failed workflow -> 409.
        let failed = orchestration::Workflow {
            workflow_id: "wf_failed".to_string(),
            run_id: run.run_id.clone(),
            environment_scope: "test".to_string(),
            goal: "fail".to_string(),
            status: orchestration::WorkflowStatus::PlanningFailed,
            failure_summary: "no consumers".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            ..orchestration::Workflow::default()
        };
        state
            .store
            .lock()
            .upsert_workflow(&failed)
            .expect("upsert failed workflow");
        let (status, body) = request(
            &app,
            Method::POST,
            &format!("/v1/runs/{}/workflows/wf_failed/start", run.run_id),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "body={body}");
        assert_eq!(body["error"], "workflow planning failed");

        // Completed workflow -> 409 "workflow is not startable".
        let done = orchestration::Workflow {
            workflow_id: "wf_done".to_string(),
            run_id: run.run_id.clone(),
            environment_scope: "test".to_string(),
            goal: "done".to_string(),
            status: orchestration::WorkflowStatus::Completed,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            ..orchestration::Workflow::default()
        };
        state
            .store
            .lock()
            .upsert_workflow(&done)
            .expect("upsert done workflow");
        let (status, body) = request(
            &app,
            Method::POST,
            &format!("/v1/runs/{}/workflows/wf_done/start", run.run_id),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "body={body}");
        assert_eq!(body["error"], "workflow is not startable");
    }

    #[tokio::test]
    async fn start_workflow_initializes_execution_and_advances_steps() {
        let (mut state, runtime) = test_state_with_runtime();
        let data_root = std::env::temp_dir()
            .join(format!("kura-workflow-data-{}", Uuid::now_v7()))
            .to_str()
            .expect("path")
            .to_string();
        std::fs::create_dir_all(&data_root).expect("mkdir data root");
        state.skills = Some(Arc::new(skill_registry(&data_root)));
        let run = run_with_entrypoint(&state, &runtime);
        let app = router().with_state(state.clone());

        let (status, created) = request(
            &app,
            Method::POST,
            &format!("/v1/runs/{}/workflows", run.run_id),
            Some("{}"),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "body={created}");
        let workflow_id = created["workflowId"].as_str().expect("workflowId").to_string();
        assert_eq!(created["steps"][0]["consumerKind"], "skill");

        // Start -> 200; the skill consumer backend is not ported, so the step
        // is attempted and lands blocked with consumer_unavailable, which is a
        // faithful transition of the workflow truth (status leaves planned).
        let (status, started) = request(
            &app,
            Method::POST,
            &format!("/v1/runs/{}/workflows/{workflow_id}/start", run.run_id),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body={started}");
        assert_ne!(started["status"], "planned", "body={started}");
        assert!(started["startedAt"].is_string(), "body={started}");
        let step = &started["steps"][0];
        assert_eq!(step["attemptCount"], 1);
        assert!(step["runtimeStepId"].as_str().map(str::len).unwrap_or(0) > 0);
        match started["status"].as_str() {
            Some("blocked") => assert_eq!(step["blockedReason"], "consumer_unavailable"),
            other => panic!("unexpected status {other:?}: {started}"),
        }
    }

    #[tokio::test]
    async fn cancel_workflow_marks_cancelled_and_clears_steps() {
        let (state, runtime) = test_state_with_runtime();
        let run = run_with_entrypoint(&state, &runtime);
        let app = router().with_state(state.clone());

        // Missing workflow -> 404.
        let (status, _) = request(
            &app,
            Method::POST,
            &format!("/v1/runs/{}/workflows/wf_nope/cancel", run.run_id),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let workflow = orchestration::Workflow {
            workflow_id: "wf_cancel".to_string(),
            run_id: run.run_id.clone(),
            environment_scope: "test".to_string(),
            goal: "cancel me".to_string(),
            status: orchestration::WorkflowStatus::Planned,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            steps: vec![orchestration::WorkflowStep {
                workflow_step_id: "wfstep_1".to_string(),
                workflow_id: "wf_cancel".to_string(),
                title: "step".to_string(),
                position: 1,
                consumer_kind: "skill".to_string(),
                consumer_id: "exec-skill".to_string(),
                tool_name: "exec-skill".to_string(),
                status: orchestration::StepStatus::Ready,
                created_at: Utc::now(),
                updated_at: Utc::now(),
                ..orchestration::WorkflowStep::default()
            }],
            ..orchestration::Workflow::default()
        };
        state
            .store
            .lock()
            .upsert_workflow(&workflow)
            .expect("upsert workflow");
        state
            .store
            .lock()
            .replace_workflow_steps(&workflow.workflow_id, &workflow.steps)
            .expect("replace steps");

        let (status, cancelled) = request(
            &app,
            Method::POST,
            &format!("/v1/runs/{}/workflows/wf_cancel/cancel", run.run_id),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body={cancelled}");
        assert_eq!(cancelled["status"], "cancelled");
        assert!(cancelled["completedAt"].is_string());
        assert_eq!(cancelled["steps"][0]["status"], "cancelled");
    }

    #[tokio::test]
    async fn method_not_allowed_returns_405() {
        let (state, runtime) = test_state_with_runtime();
        let run = run_with_entrypoint(&state, &runtime);
        let app = router().with_state(state);

        // DELETE is not registered on the collection -> 405 (axum default).
        let (status, _) = request(
            &app,
            Method::DELETE,
            &format!("/v1/runs/{}/workflows", run.run_id),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);

        // GET is not registered on the start sub-resource -> 405.
        let (status, _) = request(
            &app,
            Method::GET,
            &format!("/v1/runs/{}/workflows/wf_x/start", run.run_id),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    }
}
