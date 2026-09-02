//! runs / schedules / delivery / events / operator route families.
//!
//! Port of the Go surface in daemon/internal/api:
//! - runs:  handleRuns + handleRunByID (daemon/internal/api/server.go); workflows/steps/
//!   tool-call subroutes under /v1/runs/{runId}/* belong to other route families
//!   (workflows.rs) or later waves.
//! - schedules: handleSchedules / handleScheduleRoutes (daemon/internal/api/schedules.go).
//! - delivery: handleDeliveryTargets / handleDeliveryTargetRoutes / handleDeliveryPreferences /
//!   handleDeliveryPreferenceRoutes / handleDeliveries / handleDeliveryRoutes /
//!   handleDeliveryWindows / handleDeliveryWindowRoutes (daemon/internal/api/delivery.go).
//! - events: handleEvents + streamEvents (server.go).
//! - operator: handleOperatorOnboarding / handleOperatorActivity / handleOperatorDiagnostics
//!   + the operator_projection.go builder.
//!
//! Status codes, DTOs and tenant scoping mirror the Go handlers: manager absent -> 500,
//! create/validation errors -> 400, reads -> 500, missing rows -> 404, and the by-id
//! tenant guard (Go withByIDTenantGuard) is applied inline per family because router()
//! takes no state (see reminders.rs for the same convention).
//!
//! Reported, not duplicated (manager methods / store CRUD missing in the Rust port):
//! - run-create billing quota reservation/commit (Go skips when the billing manager is nil).
//! - run/session profile projections (activeProfileProjection) — the store profile
//!   projection CRUD is not ported; the Go path is a no-op without it.
//! - /v1/runs/{runId}/events and the cancel/resume/steps/tool-calls subroutes (out of
//!   scope for this wave; the runs family here ports list/get/create only).
//! - operator activation + setup-wizard diagnostic findings (store ListActivationStates /
//!   ResolveActiveTenantBinding / ListSetupSessions are not ported to kura-store).
//! - scheduler-manager event persistence: kura-scheduler publishes its domain events to
//!   the Bus only (its own documented divergence from Go), so schedule events are not
//!   appended to the store by the create handler either.

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::routing::{get, post};
use axum::Router;
use chrono::{DateTime, Utc};
use futures::stream::{self, Stream};
use futures::StreamExt;

use kura_calendar as calendar;
use kura_computeruse as computeruse;
use kura_connectors as connectors;
use kura_delivery as delivery;
use kura_events as events;
use kura_integrations as integrations;
use kura_mail as mail;
use kura_orchestration as orchestration;
use kura_policy as policy;
use kura_providers as providers;
use kura_router as router;
use kura_runtime as runtime;
use kura_scheduler as scheduler;
use kura_store::SQLiteStore;

use crate::error::ApiError;
use crate::middleware::{environment_scope_from_config, guard_resource_for_tenant, AuthenticatedToken, TenantContext};
use crate::response::Json;
use crate::state::AppState;
use crate::types::{
    CalendarWorkflowActionRequest, CreateDeliveryTargetRequest, CreateRunRequest,
    CreateScheduleRequest, DeliveryOutcomeListResponse, DeliveryPreferenceListResponse,
    DeliverySummaryWindowListResponse, DeliveryTargetListResponse, EventListResponse, ListResponse,
    MailWorkflowActionRequest, OperatorActivityListResponse, OperatorDiagnosticListResponse,
    OperatorOnboardingResponse, ScheduleListResponse, ScheduleWorkflowTargetRequest,
    UpsertDeliveryPreferenceRequest,
};

/// Go operatorShellTestRunEntrypoint.
const OPERATOR_SHELL_TEST_ENTRYPOINT: &str = "operator.shell.test";

/// Route family router. Only the methods the Go handlers accept are registered;
/// axum answers other methods with 405 (Go w.WriteHeader(http.StatusMethodNotAllowed)).
#[must_use]
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/runs", get(list_runs).post(create_run))
        .route("/v1/runs/{run_id}", get(get_run))
        .route("/v1/schedules", get(list_schedules).post(create_schedule))
        .route("/v1/schedules/{schedule_id}", get(get_schedule))
        .route("/v1/schedules/{schedule_id}/pause", post(pause_schedule))
        .route("/v1/schedules/{schedule_id}/resume", post(resume_schedule))
        .route("/v1/schedules/{schedule_id}/cancel", post(cancel_schedule))
        .route(
            "/v1/delivery/targets",
            get(list_delivery_targets).post(create_delivery_target),
        )
        .route("/v1/delivery/targets/{target_id}", get(get_delivery_target))
        .route(
            "/v1/delivery/targets/{target_id}/activate",
            post(activate_delivery_target),
        )
        .route(
            "/v1/delivery/targets/{target_id}/disable",
            post(disable_delivery_target),
        )
        .route(
            "/v1/delivery/preferences",
            get(list_delivery_preferences).post(upsert_delivery_preference),
        )
        .route(
            "/v1/delivery/preferences/{preference_id}",
            get(get_delivery_preference),
        )
        .route("/v1/deliveries", get(list_deliveries))
        .route("/v1/deliveries/{delivery_id}", get(get_delivery))
        .route("/v1/delivery/windows", get(list_delivery_windows))
        .route(
            "/v1/delivery/windows/{summary_window_id}",
            get(get_delivery_window),
        )
        .route("/v1/events", get(list_events))
        .route("/v1/events/stream", get(stream_events))
        .route("/v1/operator/onboarding", get(operator_onboarding))
        .route("/v1/operator/activity", get(operator_activity))
        .route("/v1/operator/diagnostics", get(operator_diagnostics))
}

// ---------------------------------------------------------------------------
// Shared helpers (Go decodeJSONBody / writeError mappings)
// ---------------------------------------------------------------------------

fn decode_json_body<T: serde::de::DeserializeOwned>(body: &Bytes) -> Result<T, ApiError> {
    if body.is_empty() {
        return Err(ApiError::BadRequest("request body is required".to_string()));
    }
    serde_json::from_slice(body).map_err(|err| ApiError::BadRequest(err.to_string()))
}

fn runtime_manager(state: &AppState) -> Result<&runtime::Manager, ApiError> {
    state
        .runtime
        .as_deref()
        .ok_or_else(|| ApiError::internal("runtime manager is not configured"))
}

fn scheduler_manager(state: &AppState) -> Result<&scheduler::Scheduler, ApiError> {
    state
        .scheduler
        .as_deref()
        .ok_or_else(|| ApiError::internal("scheduler is not configured"))
}

fn delivery_manager(state: &AppState) -> Result<&delivery::Manager, ApiError> {
    state
        .delivery
        .as_deref()
        .ok_or_else(|| ApiError::internal("delivery manager is not configured"))
}

/// Go firstOperatorNonEmpty.
fn first_non_empty(values: &[&str]) -> String {
    for value in values {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    String::new()
}

// ---------------------------------------------------------------------------
// Runs (Go handleRuns / handleRunByID)
// ---------------------------------------------------------------------------

/// GET /v1/runs — list runs with delivery summaries projected and scoped to the
/// caller's tenant (Go handleRuns GET branch + filterRunsByTenant).
#[allow(clippy::unused_async)]
pub async fn list_runs(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Json<ListResponse<runtime::Run>>, ApiError> {
    let manager = runtime_manager(&state)?;
    let mut runs = manager.list_runs();
    runs = project_run_delivery_summaries(&state, runs).map_err(ApiError::internal)?;
    runs = filter_runs_by_tenant(&state, tenant.as_ref().map(|e| &e.0), runs)?;
    Ok(Json(ListResponse { items: runs }))
}

/// POST /v1/runs — resolve/route a session, create the run, persist session +
/// run, publish session.route + run.created events (Go handleRuns POST branch;
/// billing reservation and profile projections are reported-not-duplicated).
#[allow(clippy::unused_async)]
pub async fn create_run(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<(StatusCode, Json<runtime::Run>), ApiError> {
    let request: CreateRunRequest = decode_json_body(&body)?;
    let session_router = state
        .router
        .as_ref()
        .ok_or_else(|| ApiError::internal("session router is not configured"))?;
    let (session, created_session) = resolve_run_session(session_router, &request)?;
    let manager = runtime_manager(&state)?;
    let run = manager
        .create_run(runtime::CreateRunInput {
            session_id: session.session_id.clone(),
            entrypoint: request.entrypoint,
            goal: request.goal,
            ..runtime::CreateRunInput::default()
        })
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;

    let tenant_id = tenant
        .as_ref()
        .map(|t| t.0.0.tenant_id.clone())
        .unwrap_or_default();
    persist_session(&state, &session, &tenant_id)?;
    persist_run(&state, &run, &tenant_id)?;
    publish_session_route_events(&state, &session, created_session)?;
    publish_run_created_event(&state, &run)?;
    Ok((StatusCode::CREATED, Json(run)))
}

/// GET /v1/runs/{run_id} — one run with the delivery summary projected (Go
/// handleRunByID; the by-id tenant guard rides on runs.run_id).
#[allow(clippy::unused_async)]
pub async fn get_run(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
    method: Method,
    uri: Uri,
) -> Result<Json<runtime::Run>, ApiError> {
    guard_run_for_tenant(&state, &method, &uri, tenant.as_ref().map(|e| &e.0), &run_id).await?;
    let manager = runtime_manager(&state)?;
    let mut run = manager
        .get_run(&run_id)
        .ok_or_else(|| ApiError::NotFound("not found".to_string()))?;
    run = project_run_delivery_summary(&state, run).map_err(ApiError::internal)?;
    Ok(Json(run))
}

/// Go resolveRunSession: sessionId XOR route XOR default local route.
fn resolve_run_session(
    session_router: &router::SessionRouter,
    request: &CreateRunRequest,
) -> Result<(router::Session, bool), ApiError> {
    if !request.session_id.is_empty() && request.route.is_some() {
        return Err(ApiError::BadRequest(
            "sessionId and route cannot be provided together".to_string(),
        ));
    }
    if !request.session_id.is_empty() {
        session_router
            .get_session(&request.session_id)
            .ok_or_else(|| ApiError::NotFound("not found".to_string()))?;
        let session = session_router
            .touch_session(&request.session_id)
            .map_err(|err| ApiError::BadRequest(err.to_string()))?;
        return Ok((session, false));
    }
    if let Some(route) = &request.route {
        let input = router::RouteInput {
            kind: route.kind.unwrap_or(router::SessionKind::Direct),
            channel: route.channel.clone(),
            account_id: route.account_id.clone(),
            peer_id: route.peer_id.clone(),
            thread_id: route.thread_id.clone(),
        };
        return session_router
            .route(input)
            .map_err(|err| ApiError::BadRequest(err.to_string()));
    }
    let channel = "local".to_string();
    let peer_id = if request.entrypoint.is_empty() {
        "chat".to_string()
    } else {
        request.entrypoint.clone()
    };
    session_router
        .route(router::RouteInput {
            kind: router::SessionKind::Direct,
            channel,
            account_id: "local".to_string(),
            peer_id,
            thread_id: String::new(),
        })
        .map_err(|err| ApiError::BadRequest(err.to_string()))
}

/// Go filterRunsByTenant: keep runs whose store tenant_id is NULL (legacy
/// pre-backfill) or matches the caller; drop cross-tenant rows. Divergence:
/// kura-store's lookup_row_tenant conflates an absent row with a NULL-tenant
/// row (both return None), so non-persisted in-memory runs are kept here where
/// Go would drop them.
fn filter_runs_by_tenant(
    state: &AppState,
    tenant: Option<&TenantContext>,
    runs: Vec<runtime::Run>,
) -> Result<Vec<runtime::Run>, ApiError> {
    let Some(tc) = tenant else {
        return Ok(runs);
    };
    if tc.0.tenant_id.is_empty() {
        return Ok(runs);
    }
    let store = state.store.lock();
    let mut out = Vec::with_capacity(runs.len());
    for run in runs {
        let owner = store
            .lookup_row_tenant("runs", "run_id", &run.run_id)
            .map_err(ApiError::from_store)?;
        match owner {
            None => out.push(run),
            Some(owner) if owner.is_empty() || owner == tc.0.tenant_id => out.push(run),
            Some(_) => {}
        }
    }
    Ok(out)
}

/// Go persistRun / persistSession: tenant-safe upsert when a tenant context is
/// resolved; a cross-tenant collision maps to 404 (existence is not leaked).
fn persist_run(state: &AppState, run: &runtime::Run, tenant_id: &str) -> Result<(), ApiError> {
    let store = state.store.lock();
    let result = if tenant_id.is_empty() {
        store.upsert_run(run)
    } else {
        store.upsert_run_for_tenant_safe(run, tenant_id)
    };
    result.map_err(map_persist_error)
}

fn persist_session(state: &AppState, session: &router::Session, tenant_id: &str) -> Result<(), ApiError> {
    let store = state.store.lock();
    let result = if tenant_id.is_empty() {
        store.upsert_session(session)
    } else {
        store.upsert_session_for_tenant_safe(session, tenant_id)
    };
    result.map_err(map_persist_error)
}

/// Maps the store cross-tenant sentinel to 404 (Go ErrTenantOwnershipDenied)
/// and everything else to 500.
fn map_persist_error(err: String) -> ApiError {
    if SQLiteStore::is_cross_tenant_row(&err) {
        ApiError::NotFound("not found".to_string())
    } else {
        ApiError::from_store(err)
    }
}

/// Go publishSessionRouteEvents: session.created when the session is new,
/// always session.routed.
fn publish_session_route_events(
    state: &AppState,
    session: &router::Session,
    created_session: bool,
) -> Result<(), ApiError> {
    let payload = || {
        let mut payload = serde_json::Map::new();
        payload.insert("kind".to_string(), serde_json::json!(session.kind.as_str()));
        payload.insert("channel".to_string(), serde_json::json!(session.channel));
        payload.insert("routingKey".to_string(), serde_json::json!(session.routing_key));
        payload.insert("generation".to_string(), serde_json::json!(session.generation));
        payload.insert("source".to_string(), serde_json::json!("run.create"));
        payload
    };
    if created_session {
        publish_event(state, events::Event {
            category: "session".to_string(),
            name: "session.created".to_string(),
            scope: events::Scope {
                session_id: session.session_id.clone(),
                ..events::Scope::default()
            },
            resource: events::Resource {
                kind: "session".to_string(),
                id: session.session_id.clone(),
            },
            payload: payload(),
            ..events::Event::default()
        })?;
    }
    publish_event(state, events::Event {
        category: "session".to_string(),
        name: "session.routed".to_string(),
        scope: events::Scope {
            session_id: session.session_id.clone(),
            ..events::Scope::default()
        },
        resource: events::Resource {
            kind: "session".to_string(),
            id: session.session_id.clone(),
        },
        payload: payload(),
        ..events::Event::default()
    })
}

/// Go publishEvent for run.created.
fn publish_run_created_event(state: &AppState, run: &runtime::Run) -> Result<(), ApiError> {
    let mut payload = serde_json::Map::new();
    payload.insert("entrypoint".to_string(), serde_json::json!(run.entrypoint));
    payload.insert("goal".to_string(), serde_json::json!(run.goal));
    payload.insert("status".to_string(), serde_json::json!(run.status.as_str()));
    publish_event(state, events::Event {
        category: "run".to_string(),
        name: "run.created".to_string(),
        scope: events::Scope {
            session_id: run.session_id.clone(),
            run_id: run.run_id.clone(),
            ..events::Scope::default()
        },
        resource: events::Resource {
            kind: "run".to_string(),
            id: run.run_id.clone(),
        },
        payload,
        ..events::Event::default()
    })
}

/// Appends the event to the store ledger and fans it out on the bus (Go
/// publishEvent + ensureEventDefaults: an empty event id is generated before
/// the ledger insert because the events table keys on event_id).
fn publish_event(state: &AppState, mut event: events::Event) -> Result<(), ApiError> {
    event.environment_scope = environment_scope_from_config(&state.config);
    if event.event_id.is_empty() {
        let hex = uuid::Uuid::new_v4().simple().to_string();
        event.event_id = format!("evt_{}", &hex[..16]);
    }
    let stored = state
        .store
        .lock()
        .append_event(&event)
        .map_err(ApiError::from_store)?;
    state.event_bus.publish(stored);
    Ok(())
}

/// Go withByIDTenantGuard for the runs table.
async fn guard_run_for_tenant(
    state: &AppState,
    method: &Method,
    uri: &Uri,
    tenant: Option<&TenantContext>,
    run_id: &str,
) -> Result<(), ApiError> {
    let surface = format!("api:{} {}", method.as_str(), uri.path());
    guard_resource_for_tenant(state, tenant, &surface, "runs", "run_id", run_id, "run").await
}

// ---------------------------------------------------------------------------
// Runs delivery-summary projection (Go delivery_projection.go)
// ---------------------------------------------------------------------------

fn project_run_delivery_summaries(
    state: &AppState,
    runs: Vec<runtime::Run>,
) -> Result<Vec<runtime::Run>, String> {
    if runs.is_empty() {
        return Ok(runs);
    }
    let Some(manager) = state.delivery.as_deref() else {
        return Ok(runs);
    };
    let mut items = Vec::with_capacity(runs.len());
    for run in runs {
        items.push(project_run_delivery_summary_for(manager, run)?);
    }
    Ok(items)
}

fn project_run_delivery_summary(state: &AppState, run: runtime::Run) -> Result<runtime::Run, String> {
    let Some(manager) = state.delivery.as_deref() else {
        return Ok(run);
    };
    project_run_delivery_summary_for(manager, run)
}

fn project_run_delivery_summary_for(
    manager: &delivery::Manager,
    mut run: runtime::Run,
) -> Result<runtime::Run, String> {
    let (summary, ok) = manager
        .latest_summary_for_run(&run.run_id)
        .map_err(|err| err.to_string())?;
    if !ok {
        return Ok(run);
    }
    run.latest_delivery_id = summary.latest_delivery_id;
    run.latest_delivery_status = summary.latest_delivery_status;
    run.latest_delivery_target_id = summary.latest_delivery_target_id;
    Ok(run)
}

// ---------------------------------------------------------------------------
// Schedules (Go schedules.go)
// ---------------------------------------------------------------------------

/// GET /v1/schedules — list schedules in the environment scope with delivery /
/// calendar / mail projections (Go handleSchedules GET branch).
#[allow(clippy::unused_async)]
pub async fn list_schedules(
    State(state): State<AppState>,
) -> Result<Json<ScheduleListResponse>, ApiError> {
    let manager = scheduler_manager(&state)?;
    let mut items = manager.list().map_err(ApiError::internal)?;
    items = project_schedule_delivery_summaries(&state, items).map_err(ApiError::internal)?;
    items = project_schedules_calendar_summaries(&state, items).map_err(ApiError::from_store)?;
    items = project_schedules_mail_summaries(&state, items).map_err(ApiError::from_store)?;
    Ok(Json(ScheduleListResponse { items }))
}

/// POST /v1/schedules — create a schedule (Go handleSchedules POST branch;
/// trigger validation maps to 400, create errors to 400).
#[allow(clippy::unused_async)]
pub async fn create_schedule(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<(StatusCode, Json<scheduler::Schedule>), ApiError> {
    let input: CreateScheduleRequest = decode_json_body(&body)?;
    let trigger = schedule_trigger_from_request(input.trigger)?;
    let workflow_target = build_schedule_workflow_target(input.target.workflow.as_ref())?;
    let target = scheduler::Target {
        kind: input.target.kind,
        run: input.target.run,
        workflow: workflow_target,
        ..scheduler::Target::default()
    };
    let manager = scheduler_manager(&state)?;
    let item = manager
        .create(scheduler::CreateInput {
            trigger,
            target,
            retry_policy: input.retry_policy,
        })
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    Ok((StatusCode::CREATED, Json(item)))
}

/// GET /v1/schedules/{schedule_id} — one schedule (Go handleScheduleByID;
/// by-id guard on schedules.schedule_id).
#[allow(clippy::unused_async)]
pub async fn get_schedule(
    State(state): State<AppState>,
    Path(schedule_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
    method: Method,
    uri: Uri,
) -> Result<Json<scheduler::Schedule>, ApiError> {
    guard_schedule_for_tenant(&state, &method, &uri, tenant.as_ref().map(|e| &e.0), &schedule_id).await?;
    let manager = scheduler_manager(&state)?;
    let mut item = manager
        .get(&schedule_id)
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::NotFound("not found".to_string()))?;
    item = project_schedule_delivery_summary(&state, item).map_err(ApiError::internal)?;
    item = project_schedule_calendar_summaries(&state, item).map_err(ApiError::from_store)?;
    item = project_schedule_mail_summaries(&state, item).map_err(ApiError::from_store)?;
    Ok(Json(item))
}

/// POST /v1/schedules/{schedule_id}/pause — Go handleSchedulePause.
#[allow(clippy::unused_async)]
pub async fn pause_schedule(
    State(state): State<AppState>,
    Path(schedule_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
    method: Method,
    uri: Uri,
) -> Result<Json<scheduler::Schedule>, ApiError> {
    guard_schedule_for_tenant(&state, &method, &uri, tenant.as_ref().map(|e| &e.0), &schedule_id).await?;
    let manager = scheduler_manager(&state)?;
    let item = manager
        .pause(&schedule_id)
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::NotFound("not found".to_string()))?;
    Ok(Json(item))
}

/// POST /v1/schedules/{schedule_id}/resume — Go handleScheduleResume.
#[allow(clippy::unused_async)]
pub async fn resume_schedule(
    State(state): State<AppState>,
    Path(schedule_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
    method: Method,
    uri: Uri,
) -> Result<Json<scheduler::Schedule>, ApiError> {
    guard_schedule_for_tenant(&state, &method, &uri, tenant.as_ref().map(|e| &e.0), &schedule_id).await?;
    let manager = scheduler_manager(&state)?;
    let item = manager
        .resume(&schedule_id)
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::NotFound("not found".to_string()))?;
    Ok(Json(item))
}

/// POST /v1/schedules/{schedule_id}/cancel — Go handleScheduleCancel.
#[allow(clippy::unused_async)]
pub async fn cancel_schedule(
    State(state): State<AppState>,
    Path(schedule_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
    method: Method,
    uri: Uri,
) -> Result<Json<scheduler::Schedule>, ApiError> {
    guard_schedule_for_tenant(&state, &method, &uri, tenant.as_ref().map(|e| &e.0), &schedule_id).await?;
    let manager = scheduler_manager(&state)?;
    let item = manager
        .cancel(&schedule_id)
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::NotFound("not found".to_string()))?;
    Ok(Json(item))
}

/// Go scheduleTriggerFromRequest: once parses fireAt (RFC3339 -> UTC); cron
/// requires a timezone; unknown kinds fail JSON decode before this runs.
fn schedule_trigger_from_request(
    input: crate::types::ScheduleTriggerRequest,
) -> Result<scheduler::Trigger, ApiError> {
    let mut trigger = scheduler::Trigger {
        kind: input.kind,
        cron_expr: input.cron_expr.trim().to_string(),
        timezone: input.timezone.trim().to_string(),
        ..scheduler::Trigger::default()
    };
    match input.kind {
        scheduler::TriggerKind::Once => {
            let fire_at = DateTime::parse_from_rfc3339(input.fire_at.trim())
                .map_err(|err| ApiError::BadRequest(err.to_string()))?
                .with_timezone(&Utc);
            trigger.fire_at = Some(fire_at);
        }
        scheduler::TriggerKind::Cron => {
            if trigger.timezone.is_empty() {
                return Err(ApiError::BadRequest(
                    "cron schedule requires timezone".to_string(),
                ));
            }
        }
    }
    Ok(trigger)
}

/// Go handleSchedules workflow target construction (buildCalendarAction /
/// buildMailAction reuse the workflows-family builders).
fn build_schedule_workflow_target(
    request: Option<&ScheduleWorkflowTargetRequest>,
) -> Result<Option<scheduler::WorkflowTarget>, ApiError> {
    let Some(request) = request else {
        return Ok(None);
    };
    let calendar_action =
        build_calendar_action(request.calendar_action.as_ref()).map_err(ApiError::BadRequest)?;
    let mail_action = build_mail_action(request.mail_action.as_ref()).map_err(ApiError::BadRequest)?;
    Ok(Some(scheduler::WorkflowTarget {
        session_id: request.session_id.clone(),
        entrypoint: request.entrypoint.clone(),
        run_goal: request.run_goal.clone(),
        workflow_goal: request.workflow_goal.clone(),
        calendar_action,
        mail_action,
    }))
}

/// Go withByIDTenantGuard for the schedules table.
async fn guard_schedule_for_tenant(
    state: &AppState,
    method: &Method,
    uri: &Uri,
    tenant: Option<&TenantContext>,
    schedule_id: &str,
) -> Result<(), ApiError> {
    let surface = format!("api:{} {}", method.as_str(), uri.path());
    guard_resource_for_tenant(
        state,
        tenant,
        &surface,
        "schedules",
        "schedule_id",
        schedule_id,
        "schedule",
    )
    .await
}

// ---------------------------------------------------------------------------
// Schedule projections (Go delivery_projection.go / calendar_projection.go /
// mail_projection.go)
// ---------------------------------------------------------------------------

fn project_schedule_delivery_summaries(
    state: &AppState,
    schedules: Vec<scheduler::Schedule>,
) -> Result<Vec<scheduler::Schedule>, String> {
    if schedules.is_empty() {
        return Ok(schedules);
    }
    let Some(manager) = state.delivery.as_deref() else {
        return Ok(schedules);
    };
    let mut items = Vec::with_capacity(schedules.len());
    for schedule in schedules {
        items.push(project_schedule_delivery_summary_for(manager, schedule)?);
    }
    Ok(items)
}

fn project_schedule_delivery_summary(
    state: &AppState,
    schedule: scheduler::Schedule,
) -> Result<scheduler::Schedule, String> {
    let Some(manager) = state.delivery.as_deref() else {
        return Ok(schedule);
    };
    project_schedule_delivery_summary_for(manager, schedule)
}

fn project_schedule_delivery_summary_for(
    manager: &delivery::Manager,
    mut schedule: scheduler::Schedule,
) -> Result<scheduler::Schedule, String> {
    if schedule.attempts.is_empty() {
        return Ok(schedule);
    }
    let summaries = manager
        .latest_summaries_for_schedule_attempts(&schedule.schedule_id)
        .map_err(|err| err.to_string())?;
    if summaries.is_empty() {
        return Ok(schedule);
    }
    for attempt in &mut schedule.attempts {
        let Some(summary) = summaries.get(&attempt.attempt_id) else {
            continue;
        };
        attempt.latest_delivery_id = summary.latest_delivery_id.clone();
        attempt.latest_delivery_status = summary.latest_delivery_status.clone();
        attempt.latest_delivery_target_id = summary.latest_delivery_target_id.clone();
    }
    Ok(schedule)
}

fn project_schedules_calendar_summaries(
    state: &AppState,
    schedules: Vec<scheduler::Schedule>,
) -> Result<Vec<scheduler::Schedule>, String> {
    if schedules.is_empty() {
        return Ok(schedules);
    }
    let mut items = Vec::with_capacity(schedules.len());
    for schedule in schedules {
        items.push(project_schedule_calendar_summaries(state, schedule)?);
    }
    Ok(items)
}

fn project_schedule_calendar_summaries(
    state: &AppState,
    mut schedule: scheduler::Schedule,
) -> Result<scheduler::Schedule, String> {
    if schedule.environment_scope.trim().is_empty() || schedule.schedule_id.trim().is_empty() {
        return Ok(schedule);
    }
    let filter = kura_store::calendar::CalendarOperationFilter {
        schedule_id: schedule.schedule_id.clone(),
        ..kura_store::calendar::CalendarOperationFilter::default()
    };
    let operations = state
        .store
        .lock()
        .list_calendar_operations(&schedule.environment_scope, &filter)?;
    for attempt in &mut schedule.attempts {
        let filtered = operations
            .iter()
            .filter(|item| item.schedule_attempt_id.trim() == attempt.attempt_id)
            .cloned()
            .collect();
        attempt.calendar_operation_summaries = summarize_calendar_operations(filtered);
    }
    Ok(schedule)
}

fn project_schedules_mail_summaries(
    state: &AppState,
    schedules: Vec<scheduler::Schedule>,
) -> Result<Vec<scheduler::Schedule>, String> {
    if schedules.is_empty() {
        return Ok(schedules);
    }
    let mut items = Vec::with_capacity(schedules.len());
    for schedule in schedules {
        items.push(project_schedule_mail_summaries(state, schedule)?);
    }
    Ok(items)
}

fn project_schedule_mail_summaries(
    state: &AppState,
    mut schedule: scheduler::Schedule,
) -> Result<scheduler::Schedule, String> {
    if schedule.environment_scope.trim().is_empty() || schedule.schedule_id.trim().is_empty() {
        return Ok(schedule);
    }
    let filter = kura_store::mail::MailOperationFilter {
        schedule_id: schedule.schedule_id.clone(),
        ..kura_store::mail::MailOperationFilter::default()
    };
    let operations = state
        .store
        .lock()
        .list_mail_operations(&schedule.environment_scope, &filter)?;
    for attempt in &mut schedule.attempts {
        let filtered = operations
            .iter()
            .filter(|item| item.schedule_attempt_id.trim() == attempt.attempt_id)
            .cloned()
            .collect();
        attempt.mail_operation_summaries = summarize_mail_operations(filtered);
    }
    Ok(schedule)
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
// Delivery (Go delivery.go)
// ---------------------------------------------------------------------------

/// GET /v1/delivery/targets — Go handleDeliveryTargets GET branch.
#[allow(clippy::unused_async)]
pub async fn list_delivery_targets(
    State(state): State<AppState>,
) -> Result<Json<DeliveryTargetListResponse>, ApiError> {
    let manager = delivery_manager(&state)?;
    let items = manager.list_targets().map_err(ApiError::internal)?;
    Ok(Json(DeliveryTargetListResponse { items }))
}

/// POST /v1/delivery/targets — Go handleDeliveryTargets POST branch (create
/// errors -> 400).
#[allow(clippy::unused_async)]
pub async fn create_delivery_target(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<(StatusCode, Json<delivery::DeliveryTarget>), ApiError> {
    let input: CreateDeliveryTargetRequest = decode_json_body(&body)?;
    let manager = delivery_manager(&state)?;
    let target = manager
        .create_target(delivery::DeliveryTarget {
            target_id: input.target_id,
            display_name: input.display_name,
            target_kind: input.target_kind,
            connector_binding: input.connector_binding,
            address_summary: input.address_summary,
            ..delivery::DeliveryTarget::default()
        })
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    Ok((StatusCode::CREATED, Json(target)))
}

/// GET /v1/delivery/targets/{target_id} — Go handleDeliveryTargetRoutes GET
/// (by-id guard on delivery_targets.target_id).
#[allow(clippy::unused_async)]
pub async fn get_delivery_target(
    State(state): State<AppState>,
    Path(target_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
    method: Method,
    uri: Uri,
) -> Result<Json<delivery::DeliveryTarget>, ApiError> {
    guard_delivery_target_for_tenant(&state, &method, &uri, tenant.as_ref().map(|e| &e.0), &target_id).await?;
    let manager = delivery_manager(&state)?;
    let (target, ok) = manager.get_target(&target_id).map_err(ApiError::internal)?;
    if !ok {
        return Err(ApiError::NotFound("not found".to_string()));
    }
    Ok(Json(target))
}

/// POST /v1/delivery/targets/{target_id}/activate — Go handleDeliveryTargetRoutes
/// activate branch.
#[allow(clippy::unused_async)]
pub async fn activate_delivery_target(
    State(state): State<AppState>,
    Path(target_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
    method: Method,
    uri: Uri,
) -> Result<Json<delivery::DeliveryTarget>, ApiError> {
    guard_delivery_target_for_tenant(&state, &method, &uri, tenant.as_ref().map(|e| &e.0), &target_id).await?;
    let manager = delivery_manager(&state)?;
    let (target, ok) = manager
        .update_target_status(&target_id, delivery::TargetStatus::Active)
        .map_err(ApiError::internal)?;
    if !ok {
        return Err(ApiError::NotFound("not found".to_string()));
    }
    Ok(Json(target))
}

/// POST /v1/delivery/targets/{target_id}/disable — Go handleDeliveryTargetRoutes
/// disable branch.
#[allow(clippy::unused_async)]
pub async fn disable_delivery_target(
    State(state): State<AppState>,
    Path(target_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
    method: Method,
    uri: Uri,
) -> Result<Json<delivery::DeliveryTarget>, ApiError> {
    guard_delivery_target_for_tenant(&state, &method, &uri, tenant.as_ref().map(|e| &e.0), &target_id).await?;
    let manager = delivery_manager(&state)?;
    let (target, ok) = manager
        .update_target_status(&target_id, delivery::TargetStatus::Disabled)
        .map_err(ApiError::internal)?;
    if !ok {
        return Err(ApiError::NotFound("not found".to_string()));
    }
    Ok(Json(target))
}

/// GET /v1/delivery/preferences — Go handleDeliveryPreferences GET branch.
#[allow(clippy::unused_async)]
pub async fn list_delivery_preferences(
    State(state): State<AppState>,
) -> Result<Json<DeliveryPreferenceListResponse>, ApiError> {
    let manager = delivery_manager(&state)?;
    let items = manager.list_preferences().map_err(ApiError::internal)?;
    Ok(Json(DeliveryPreferenceListResponse { items }))
}

/// POST /v1/delivery/preferences — Go handleDeliveryPreferences POST branch
/// (create errors -> 400).
#[allow(clippy::unused_async)]
pub async fn upsert_delivery_preference(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<(StatusCode, Json<delivery::DeliveryPreference>), ApiError> {
    let input: UpsertDeliveryPreferenceRequest = decode_json_body(&body)?;
    let manager = delivery_manager(&state)?;
    let pref = manager
        .upsert_preference(delivery::DeliveryPreference {
            preference_id: input.preference_id,
            environment_scope: input.environment_scope,
            scope_kind: input.scope_kind,
            integration_id: input.integration_id,
            preferred_targets_by_class: input.preferred_targets_by_class,
            summary_policy: Some(input.summary_policy),
            suppression_policy: Some(input.suppression_policy),
            ..delivery::DeliveryPreference::default()
        })
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    Ok((StatusCode::CREATED, Json(pref)))
}

/// GET /v1/delivery/preferences/{preference_id} — Go
/// handleDeliveryPreferenceRoutes (by-id guard on delivery_preferences.preference_id).
#[allow(clippy::unused_async)]
pub async fn get_delivery_preference(
    State(state): State<AppState>,
    Path(preference_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
    method: Method,
    uri: Uri,
) -> Result<Json<delivery::DeliveryPreference>, ApiError> {
    guard_delivery_preference_for_tenant(&state, &method, &uri, tenant.as_ref().map(|e| &e.0), &preference_id)
        .await?;
    let manager = delivery_manager(&state)?;
    let (item, ok) = manager
        .get_preference(&preference_id)
        .map_err(ApiError::internal)?;
    if !ok {
        return Err(ApiError::NotFound("not found".to_string()));
    }
    Ok(Json(item))
}

/// GET /v1/deliveries — Go handleDeliveries: filter by query params, project
/// calendar/mail linkage.
#[allow(clippy::unused_async)]
pub async fn list_deliveries(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<DeliveryOutcomeListResponse>, ApiError> {
    let manager = delivery_manager(&state)?;
    let filter = delivery::OutcomeFilter {
        source_kind: query_param(&params, "sourceKind"),
        source_id: query_param(&params, "sourceId"),
        run_id: query_param(&params, "runId"),
        workflow_id: query_param(&params, "workflowId"),
        schedule_id: query_param(&params, "scheduleId"),
        integration_id: query_param(&params, "integrationId"),
        status: parse_outcome_status(&query_param(&params, "status")),
        target_id: query_param(&params, "targetId"),
    };
    let mut items = manager.list_outcomes(filter).map_err(ApiError::internal)?;
    items = project_delivery_outcomes_calendar_linkage(&state, items).map_err(ApiError::from_store)?;
    items = project_delivery_outcomes_mail_linkage(&state, items).map_err(ApiError::from_store)?;
    Ok(Json(DeliveryOutcomeListResponse { items }))
}

/// GET /v1/deliveries/{delivery_id} — Go handleDeliveryRoutes (by-id guard on
/// delivery_outcomes.delivery_id).
#[allow(clippy::unused_async)]
pub async fn get_delivery(
    State(state): State<AppState>,
    Path(delivery_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
    method: Method,
    uri: Uri,
) -> Result<Json<delivery::DeliveryOutcome>, ApiError> {
    guard_delivery_for_tenant(&state, &method, &uri, tenant.as_ref().map(|e| &e.0), &delivery_id).await?;
    let manager = delivery_manager(&state)?;
    let (mut item, ok) = manager.get_outcome(&delivery_id).map_err(ApiError::internal)?;
    if !ok {
        return Err(ApiError::NotFound("not found".to_string()));
    }
    item = project_delivery_outcome_calendar_linkage(&state, item).map_err(ApiError::from_store)?;
    item = project_delivery_outcome_mail_linkage(&state, item).map_err(ApiError::from_store)?;
    Ok(Json(item))
}

/// GET /v1/delivery/windows — Go handleDeliveryWindows.
#[allow(clippy::unused_async)]
pub async fn list_delivery_windows(
    State(state): State<AppState>,
) -> Result<Json<DeliverySummaryWindowListResponse>, ApiError> {
    let manager = delivery_manager(&state)?;
    let items = manager.list_summary_windows().map_err(ApiError::internal)?;
    Ok(Json(DeliverySummaryWindowListResponse { items }))
}

/// GET /v1/delivery/windows/{summary_window_id} — Go handleDeliveryWindowRoutes
/// (by-id guard on delivery_summary_windows.summary_window_id).
#[allow(clippy::unused_async)]
pub async fn get_delivery_window(
    State(state): State<AppState>,
    Path(summary_window_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
    method: Method,
    uri: Uri,
) -> Result<Json<delivery::SummaryWindow>, ApiError> {
    guard_delivery_window_for_tenant(&state, &method, &uri, tenant.as_ref().map(|e| &e.0), &summary_window_id)
        .await?;
    let manager = delivery_manager(&state)?;
    let (item, ok) = manager
        .get_summary_window(&summary_window_id)
        .map_err(ApiError::internal)?;
    if !ok {
        return Err(ApiError::NotFound("not found".to_string()));
    }
    Ok(Json(item))
}

pub(crate) fn query_param(params: &HashMap<String, String>, key: &str) -> String {
    params
        .get(key)
        .map(|value| value.trim().to_string())
        .unwrap_or_default()
}

/// Go OutcomeStatus(strings.TrimSpace(...)). The Rust OutcomeStatus enum has no
/// unknown variant; an unrecognized value maps to None (no filter), a documented
/// divergence from Go's pass-through string compare.
fn parse_outcome_status(raw: &str) -> Option<delivery::OutcomeStatus> {
    match raw {
        "pending" => Some(delivery::OutcomeStatus::Pending),
        "queued" => Some(delivery::OutcomeStatus::Queued),
        "dispatching" => Some(delivery::OutcomeStatus::Dispatching),
        "delivered" => Some(delivery::OutcomeStatus::Delivered),
        "suppressed" => Some(delivery::OutcomeStatus::Suppressed),
        "failed" => Some(delivery::OutcomeStatus::Failed),
        _ => None,
    }
}

/// Go withByIDTenantGuard wrappers for the delivery tables.
async fn guard_delivery_target_for_tenant(
    state: &AppState,
    method: &Method,
    uri: &Uri,
    tenant: Option<&TenantContext>,
    target_id: &str,
) -> Result<(), ApiError> {
    let surface = format!("api:{} {}", method.as_str(), uri.path());
    guard_resource_for_tenant(
        state,
        tenant,
        &surface,
        "delivery_targets",
        "target_id",
        target_id,
        "delivery_target",
    )
    .await
}

async fn guard_delivery_preference_for_tenant(
    state: &AppState,
    method: &Method,
    uri: &Uri,
    tenant: Option<&TenantContext>,
    preference_id: &str,
) -> Result<(), ApiError> {
    let surface = format!("api:{} {}", method.as_str(), uri.path());
    guard_resource_for_tenant(
        state,
        tenant,
        &surface,
        "delivery_preferences",
        "preference_id",
        preference_id,
        "delivery_preference",
    )
    .await
}

async fn guard_delivery_for_tenant(
    state: &AppState,
    method: &Method,
    uri: &Uri,
    tenant: Option<&TenantContext>,
    delivery_id: &str,
) -> Result<(), ApiError> {
    let surface = format!("api:{} {}", method.as_str(), uri.path());
    guard_resource_for_tenant(
        state,
        tenant,
        &surface,
        "delivery_outcomes",
        "delivery_id",
        delivery_id,
        "delivery_outcome",
    )
    .await
}

async fn guard_delivery_window_for_tenant(
    state: &AppState,
    method: &Method,
    uri: &Uri,
    tenant: Option<&TenantContext>,
    summary_window_id: &str,
) -> Result<(), ApiError> {
    let surface = format!("api:{} {}", method.as_str(), uri.path());
    guard_resource_for_tenant(
        state,
        tenant,
        &surface,
        "delivery_summary_windows",
        "summary_window_id",
        summary_window_id,
        "delivery_summary_window",
    )
    .await
}

// ---------------------------------------------------------------------------
// Delivery outcome calendar/mail linkage (Go calendar_projection.go /
// mail_projection.go)
// ---------------------------------------------------------------------------

fn project_delivery_outcomes_calendar_linkage(
    state: &AppState,
    items: Vec<delivery::DeliveryOutcome>,
) -> Result<Vec<delivery::DeliveryOutcome>, String> {
    if items.is_empty() {
        return Ok(items);
    }
    let mut projected = Vec::with_capacity(items.len());
    for item in items {
        projected.push(project_delivery_outcome_calendar_linkage(state, item)?);
    }
    Ok(projected)
}

fn project_delivery_outcome_calendar_linkage(
    state: &AppState,
    mut outcome: delivery::DeliveryOutcome,
) -> Result<delivery::DeliveryOutcome, String> {
    if outcome.environment_scope.trim().is_empty() {
        return Ok(outcome);
    }
    let mut filter = kura_store::calendar::CalendarOperationFilter::default();
    if !outcome.delivery_id.trim().is_empty() {
        filter.delivery_id = outcome.delivery_id.clone();
    } else if !outcome.workflow_id.trim().is_empty() {
        filter.workflow_id = outcome.workflow_id.clone();
    } else if !outcome.schedule_id.trim().is_empty() {
        filter.schedule_id = outcome.schedule_id.clone();
    } else if !outcome.run_id.trim().is_empty() {
        filter.run_id = outcome.run_id.clone();
    }
    let operations = state
        .store
        .lock()
        .list_calendar_operations(&outcome.environment_scope, &filter)?;
    outcome.calendar_operation_summaries = summarize_calendar_operations(operations);
    outcome.calendar_operation_ids = outcome
        .calendar_operation_summaries
        .iter()
        .map(|item| item.operation_id.clone())
        .collect();
    Ok(outcome)
}

fn project_delivery_outcomes_mail_linkage(
    state: &AppState,
    items: Vec<delivery::DeliveryOutcome>,
) -> Result<Vec<delivery::DeliveryOutcome>, String> {
    if items.is_empty() {
        return Ok(items);
    }
    let mut projected = Vec::with_capacity(items.len());
    for item in items {
        projected.push(project_delivery_outcome_mail_linkage(state, item)?);
    }
    Ok(projected)
}

fn project_delivery_outcome_mail_linkage(
    state: &AppState,
    mut outcome: delivery::DeliveryOutcome,
) -> Result<delivery::DeliveryOutcome, String> {
    if outcome.environment_scope.trim().is_empty() {
        return Ok(outcome);
    }
    let mut filter = kura_store::mail::MailOperationFilter::default();
    if !outcome.delivery_id.trim().is_empty() {
        filter.delivery_id = outcome.delivery_id.clone();
    } else if !outcome.workflow_id.trim().is_empty() {
        filter.workflow_id = outcome.workflow_id.clone();
    } else if !outcome.schedule_id.trim().is_empty() {
        filter.schedule_id = outcome.schedule_id.clone();
    } else if !outcome.run_id.trim().is_empty() {
        filter.run_id = outcome.run_id.clone();
    }
    let operations = state
        .store
        .lock()
        .list_mail_operations(&outcome.environment_scope, &filter)?;
    outcome.mail_operation_summaries = summarize_mail_operations(operations);
    outcome.mail_operation_ids = outcome
        .mail_operation_summaries
        .iter()
        .map(|item| item.operation_id.clone())
        .collect();
    Ok(outcome)
}

// ---------------------------------------------------------------------------
// Events (Go handleEvents + streamEvents)
// ---------------------------------------------------------------------------

/// GET /v1/events — cursor + filter based event ledger read, scoped to the
/// caller's tenant when resolved (Go handleEvents).
#[allow(clippy::unused_async)]
pub async fn list_events(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Json<EventListResponse>, ApiError> {
    let cursor = parse_event_cursor(&params, &headers)?;
    let mut filter = events_filter_from_request(&state, &params, cursor);
    if let Some(tc) = tenant {
        if !tc.0.0.tenant_id.is_empty() {
            filter.tenant_owned_tenant_id = tc.0.0.tenant_id.clone();
        }
    }
    let items = read_events(&state, &filter)?;
    Ok(Json(build_event_list_response(items)))
}

/// GET /v1/events/stream — SSE replay of matching history followed by live
/// fan-out from the bus with a 15s keep-alive (Go streamEvents). Frames match
/// Go writeRuntimeSSEEvent: id (when > 0), event name, data json.
#[allow(clippy::unused_async)]
pub async fn stream_events(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>, ApiError> {
    let cursor = parse_event_cursor(&params, &headers)?;
    let mut filter = events_filter_from_request(&state, &params, cursor);
    if let Some(tc) = tenant {
        if !tc.0.0.tenant_id.is_empty() {
            filter.tenant_owned_tenant_id = tc.0.0.tenant_id.clone();
        }
    }
    let history = read_events(&state, &filter)?;

    // Bridge the std mpsc bus subscription onto a tokio channel so the stream
    // can await live events; the unsubscribe handle lives in the stream state
    // and drops (unsubscribing) when the client disconnects.
    let (sender, receiver) = tokio::sync::mpsc::channel::<events::Event>(32);
    let (bus_receiver, unsubscribe) = state.event_bus.subscribe(filter.clone());
    std::thread::spawn(move || {
        while let Ok(event) = bus_receiver.recv() {
            if sender.blocking_send(event).is_err() {
                break;
            }
        }
    });

    let history_stream = stream::iter(
        history
            .into_iter()
            .map(|event| Ok::<_, Infallible>(to_sse_event(&event))),
    );
    let live_stream = stream::unfold(
        (receiver, unsubscribe),
        |(mut receiver, unsubscribe)| async move {
            receiver
                .recv()
                .await
                .map(|event| (Ok::<_, Infallible>(to_sse_event(&event)), (receiver, unsubscribe)))
        },
    );
    let events = stream::iter([Ok::<SseEvent, Infallible>(SseEvent::default().comment("stream-open"))])
        .chain(history_stream)
        .chain(live_stream);
    Ok(Sse::new(events).keep_alive(
        KeepAlive::default()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    ))
}

fn events_filter_from_request(
    state: &AppState,
    params: &HashMap<String, String>,
    cursor: i64,
) -> events::Filter {
    events::Filter {
        environment_scope: environment_scope_from_config(&state.config),
        category: query_param(params, "category"),
        run_id: query_param(params, "runId"),
        session_id: query_param(params, "sessionId"),
        schedule_id: query_param(params, "scheduleId"),
        schedule_attempt_id: query_param(params, "scheduleAttemptId"),
        resource_kind: query_param(params, "resourceKind"),
        cursor,
        ..events::Filter::default()
    }
}

/// Go parseEventCursor: query cursor, else Last-Event-ID header; must be a
/// non-negative integer.
pub(crate) fn parse_event_cursor(
    params: &HashMap<String, String>,
    headers: &HeaderMap,
) -> Result<i64, ApiError> {
    let mut raw = query_param(params, "cursor");
    if raw.is_empty() {
        raw = headers
            .get("Last-Event-ID")
            .and_then(|value| value.to_str().ok())
            .map(|value| value.trim().to_string())
            .unwrap_or_default();
    }
    if raw.is_empty() {
        return Ok(0);
    }
    match raw.parse::<i64>() {
        Ok(cursor) if cursor >= 0 => Ok(cursor),
        _ => Err(ApiError::BadRequest(
            "cursor must be a non-negative integer".to_string(),
        )),
    }
}

/// Go listEvents: tenant-aware store read when a tenant id is resolved, else
/// the plain ledger read.
pub(crate) fn read_events(state: &AppState, filter: &events::Filter) -> Result<Vec<events::Event>, ApiError> {
    let store = state.store.lock();
    if !filter.tenant_owned_tenant_id.is_empty() {
        store
            .list_events_for_tenant_raw(&filter.tenant_owned_tenant_id, filter)
            .map_err(ApiError::from_store)
    } else {
        store.list_events(filter).map_err(ApiError::from_store)
    }
}

/// Go buildEventListResponse: nextCursor is the last item's sequence.
pub(crate) fn build_event_list_response(items: Vec<events::Event>) -> EventListResponse {
    let next_cursor = items.last().map_or(0, |event| event.sequence);
    EventListResponse { items, next_cursor }
}

fn to_sse_event(event: &events::Event) -> SseEvent {
    let data = serde_json::to_string(event).unwrap_or_else(|_| "{}".to_string());
    let mut sse = SseEvent::default();
    if event.sequence > 0 {
        sse = sse.id(event.sequence.to_string());
    }
    sse = sse.event(event.name.clone()).data(data);
    sse
}

// ---------------------------------------------------------------------------
// Operator (Go operator.go + operator_projection.go)
// ---------------------------------------------------------------------------

/// GET /v1/operator/onboarding — Go handleOperatorOnboarding.
#[allow(clippy::unused_async)]
pub async fn operator_onboarding(
    State(state): State<AppState>,
    token: Option<Extension<AuthenticatedToken>>,
) -> Result<Json<OperatorOnboardingResponse>, ApiError> {
    let authenticated = token.is_some();
    let token_id = token.map(|t| t.0.0.token_id.clone()).unwrap_or_default();
    Ok(Json(build_onboarding(&state, &token_id, authenticated)))
}

/// GET /v1/operator/activity — Go handleOperatorActivity with the
/// sourceKind / attentionOnly / limit client filters.
#[allow(clippy::unused_async)]
pub async fn operator_activity(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<OperatorActivityListResponse>, ApiError> {
    let mut response = build_activity(&state)?;
    let source_kind = query_param(&params, "sourceKind");
    let attention_only = query_param(&params, "attentionOnly").eq_ignore_ascii_case("true");
    let limit = parse_operator_limit(&query_param(&params, "limit"), 25);
    let mut filtered = Vec::with_capacity(response.items.len());
    for item in response.items {
        if !source_kind.is_empty() && item.source_kind != source_kind {
            continue;
        }
        if attention_only && item.attention_level == "info" {
            continue;
        }
        filtered.push(item);
        if filtered.len() >= limit {
            break;
        }
    }
    response.items = filtered;
    Ok(Json(response))
}

/// GET /v1/operator/diagnostics — Go handleOperatorDiagnostics with the
/// sourceKind / plane / severity client filters.
#[allow(clippy::unused_async)]
pub async fn operator_diagnostics(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<OperatorDiagnosticListResponse>, ApiError> {
    let mut response = build_diagnostics(&state)?;
    let source_kind = query_param(&params, "sourceKind");
    let plane = query_param(&params, "plane");
    let severity = query_param(&params, "severity");
    let filtered = response
        .items
        .into_iter()
        .filter(|item| {
            (source_kind.is_empty() || item.source_kind == source_kind)
                && (plane.is_empty() || item.plane == plane)
                && (severity.is_empty() || item.severity == severity)
        })
        .collect();
    response.items = filtered;
    Ok(Json(response))
}

/// Go parseOperatorLimit.
fn parse_operator_limit(raw: &str, fallback: usize) -> usize {
    match raw.trim().parse::<usize>() {
        Ok(value) if value > 0 => value.min(200),
        _ => fallback,
    }
}

// ---------------------------------------------------------------------------
// Operator projection builder (Go operator_projection.go)
// ---------------------------------------------------------------------------

fn build_onboarding(state: &AppState, token_id: &str, authenticated: bool) -> OperatorOnboardingResponse {
    let now = Utc::now();
    let environment_scope = environment_scope_from_config(&state.config);
    let mut readiness_items: Vec<crate::types::OperatorReadinessItem> = Vec::new();
    let mut completed_step_ids: Vec<String> = Vec::new();
    let mut blocking_item_ids: Vec<String> = Vec::new();
    let mut optional_follow_up_item_ids: Vec<String> = Vec::new();

    let mut auth_item = crate::types::OperatorReadinessItem {
        item_id: "auth-token".to_string(),
        item_kind: "auth".to_string(),
        resource_id: token_id.to_string(),
        display_name: "Operator access token".to_string(),
        status: "ready".to_string(),
        health_state: String::new(),
        reason: "Authenticated shell session is active.".to_string(),
        diagnostic_freshness: String::new(),
        remediation_owner: String::new(),
        retry_safety: String::new(),
        required_operator_action: String::new(),
        required_for_selected_action: true,
        detail_route: "/v1/auth/me".to_string(),
        environment_scope: environment_scope.clone(),
        updated_at: now,
    };
    if !authenticated {
        auth_item.status = "blocked".to_string();
        auth_item.reason = "Authentication is required before the operator shell can load.".to_string();
        auth_item.required_operator_action = "Pair or reuse a local access token.".to_string();
        blocking_item_ids.push(auth_item.item_id.clone());
    } else {
        completed_step_ids.push("auth-ready".to_string());
    }
    readiness_items.push(auth_item);

    let mut query_action = crate::types::OperatorFirstUsefulAction {
        action_id: "test_query".to_string(),
        action_kind: "test_query".to_string(),
        display_name: "Run test query".to_string(),
        recommended: false,
        available: false,
        blocking_item_ids: vec!["provider-query".to_string()],
        summary: "Reuse /v1/chat/query to confirm the shell can produce a bounded test result."
            .to_string(),
        invoke_route: "/v1/chat/query".to_string(),
        result_route: "/v1/chat/query".to_string(),
    };

    if let Some(provider) = first_ready_query_provider(state) {
        query_action.available = true;
        query_action.blocking_item_ids = Vec::new();
        readiness_items.push(crate::types::OperatorReadinessItem {
            item_id: "provider-query".to_string(),
            item_kind: "provider".to_string(),
            resource_id: provider.provider_id.clone(),
            display_name: provider.title.clone(),
            status: "ready".to_string(),
            health_state: "healthy".to_string(),
            reason: "Provider is ready for bounded query execution.".to_string(),
            diagnostic_freshness: String::new(),
            remediation_owner: String::new(),
            retry_safety: String::new(),
            required_operator_action: String::new(),
            required_for_selected_action: false,
            detail_route: format!("/v1/providers/{}", provider.provider_id),
            environment_scope: environment_scope.clone(),
            updated_at: now,
        });
    } else if state.providers.is_some() {
        let mut reason = "No ready chat provider is currently configured.".to_string();
        if let Some(profiles) = state.providers.as_deref() {
            let profiles = profiles.list_profiles();
            if let Some(first) = profiles.first() {
                reason = first_non_empty(&[&first.issues.join("; "), &reason]);
            }
        }
        readiness_items.push(crate::types::OperatorReadinessItem {
            item_id: "provider-query".to_string(),
            item_kind: "provider".to_string(),
            resource_id: String::new(),
            display_name: "Chat provider".to_string(),
            status: "optional".to_string(),
            health_state: "degraded".to_string(),
            reason,
            diagnostic_freshness: String::new(),
            remediation_owner: String::new(),
            retry_safety: String::new(),
            required_operator_action: "Configure or authenticate a chat-capable provider to unlock test queries."
                .to_string(),
            required_for_selected_action: false,
            detail_route: "/v1/providers".to_string(),
            environment_scope: environment_scope.clone(),
            updated_at: now,
        });
    }

    for item in optional_integration_readiness(state, now) {
        if !item.required_for_selected_action {
            optional_follow_up_item_ids.push(item.item_id.clone());
        }
        readiness_items.push(item);
    }
    for item in optional_connector_readiness(state, now) {
        if !item.required_for_selected_action {
            optional_follow_up_item_ids.push(item.item_id.clone());
        }
        readiness_items.push(item);
    }
    for item in optional_capability_readiness(state, now) {
        if !item.required_for_selected_action {
            optional_follow_up_item_ids.push(item.item_id.clone());
        }
        readiness_items.push(item);
    }

    let mut test_run_action = crate::types::OperatorFirstUsefulAction {
        action_id: "test_run".to_string(),
        action_kind: "test_run".to_string(),
        display_name: "Launch test run".to_string(),
        recommended: true,
        available: authenticated,
        blocking_item_ids: Vec::new(),
        summary: "Reuse /v1/runs to persist a bounded operator test action that survives refresh and restart."
            .to_string(),
        invoke_route: "/v1/runs".to_string(),
        result_route: "/v1/runs".to_string(),
    };
    if !authenticated {
        test_run_action.blocking_item_ids = vec!["auth-token".to_string()];
    }
    let mut activation_action = crate::types::OperatorFirstUsefulAction {
        action_id: "test_chat".to_string(),
        action_kind: "test_chat".to_string(),
        display_name: "Run activation test chat".to_string(),
        recommended: false,
        available: authenticated,
        blocking_item_ids: Vec::new(),
        summary: "Complete the hosted personal-tenant activation first action without live connectors or production secrets."
            .to_string(),
        invoke_route: "/v1/activation/test-chat".to_string(),
        result_route: "/v1/activation".to_string(),
    };
    if !authenticated {
        activation_action.blocking_item_ids = vec!["auth-token".to_string()];
    }

    let mut first_useful_actions = vec![test_run_action, activation_action];
    if state.providers.is_some() {
        first_useful_actions.push(query_action);
    }

    let mut status = "ready_for_action".to_string();
    let mut current_step_id = "run-first-action".to_string();
    if !blocking_item_ids.is_empty() {
        status = "blocked".to_string();
        current_step_id = "resolve-blockers".to_string();
    }
    if has_recorded_shell_test_run(state) {
        status = "completed".to_string();
        current_step_id = "completed".to_string();
        completed_step_ids.push("test-run-recorded".to_string());
    }

    OperatorOnboardingResponse {
        environment_scope,
        status,
        current_step_id,
        completed_step_ids,
        blocking_item_ids,
        optional_follow_up_item_ids,
        recommended_action_id: "test_run".to_string(),
        readiness_items,
        first_useful_actions,
        last_evaluated_at: now,
    }
}

fn optional_integration_readiness(
    state: &AppState,
    _now: DateTime<Utc>,
) -> Vec<crate::types::OperatorReadinessItem> {
    let Some(manager) = state.integrations.as_deref() else {
        return Vec::new();
    };
    let mut items = Vec::new();
    for item in manager.list() {
        let (status, health_state) = map_integration_readiness(&item);
        let status = if status == "ready" { "optional".to_string() } else { status };
        items.push(crate::types::OperatorReadinessItem {
            item_id: format!("integration-{}", item.integration_id),
            item_kind: "integration".to_string(),
            resource_id: item.integration_id.clone(),
            display_name: item.display_name.clone(),
            status,
            health_state,
            reason: first_non_empty(&[
                &item.readiness_reason,
                "Integration readiness is projected from daemon state.",
            ]),
            diagnostic_freshness: String::new(),
            remediation_owner: String::new(),
            retry_safety: String::new(),
            required_operator_action: item.required_operator_action.clone(),
            required_for_selected_action: false,
            detail_route: format!("/v1/integrations/{}", item.integration_id),
            environment_scope: environment_scope_from_config(&state.config),
            updated_at: item.updated_at,
        });
    }
    items.sort_by(|a, b| a.display_name.cmp(&b.display_name));
    items
}

fn optional_connector_readiness(
    state: &AppState,
    _now: DateTime<Utc>,
) -> Vec<crate::types::OperatorReadinessItem> {
    let Some(supervisor) = state.connectors.as_deref() else {
        return Vec::new();
    };
    let mut items = Vec::new();
    for item in supervisor.list() {
        let mut remediation_owner = "none_required".to_string();
        let mut retry_safety = "no_action_needed".to_string();
        if item.status != connectors::Status::Healthy {
            remediation_owner = "operator".to_string();
            retry_safety = "blocked".to_string();
        }
        items.push(crate::types::OperatorReadinessItem {
            item_id: format!("connector-{}", item.connector_id),
            item_kind: "connector".to_string(),
            resource_id: item.connector_id.clone(),
            display_name: item.display_name.clone(),
            status: map_connector_status(item.status),
            health_state: item.status.as_str().to_string(),
            reason: first_non_empty(&[
                &item.last_failure_reason,
                "Connector health is projected from the supervisor.",
            ]),
            diagnostic_freshness: "fresh".to_string(),
            remediation_owner,
            retry_safety,
            required_operator_action: connector_operator_action(&item),
            required_for_selected_action: false,
            detail_route: format!("/v1/connectors/{}", item.connector_id),
            environment_scope: environment_scope_from_config(&state.config),
            updated_at: item.updated_at,
        });
    }
    items.sort_by(|a, b| a.display_name.cmp(&b.display_name));
    items
}

fn optional_capability_readiness(
    state: &AppState,
    _now: DateTime<Utc>,
) -> Vec<crate::types::OperatorReadinessItem> {
    let Some(supervisor) = state.capabilities.as_deref() else {
        return Vec::new();
    };
    let mut items = Vec::new();
    for item in supervisor.list() {
        items.push(crate::types::OperatorReadinessItem {
            item_id: format!("capability-{}", item.capability_id),
            item_kind: "capability".to_string(),
            resource_id: item.capability_id.clone(),
            display_name: item.display_name.clone(),
            status: map_capability_status(item.status),
            health_state: item.status.as_str().to_string(),
            reason: first_non_empty(&[
                &item.last_failure_reason,
                "Capability health is projected from the supervisor.",
            ]),
            diagnostic_freshness: String::new(),
            remediation_owner: String::new(),
            retry_safety: String::new(),
            required_operator_action: capability_operator_action(&item),
            required_for_selected_action: false,
            detail_route: format!("/v1/capabilities/{}", item.capability_id),
            environment_scope: environment_scope_from_config(&state.config),
            updated_at: item.updated_at,
        });
    }
    items.sort_by(|a, b| a.display_name.cmp(&b.display_name));
    items
}

fn first_ready_query_provider(state: &AppState) -> Option<providers::Profile> {
    let manager = state.providers.as_deref()?;
    manager
        .list_profiles()
        .into_iter()
        .find(|profile| profile.ready && profile.capabilities.chat)
}

fn has_recorded_shell_test_run(state: &AppState) -> bool {
    let Some(manager) = state.runtime.as_deref() else {
        return false;
    };
    manager.list_runs().iter().any(|run| {
        run.entrypoint == OPERATOR_SHELL_TEST_ENTRYPOINT && run.status != runtime::RunStatus::Cancelled
    })
}

fn build_activity(state: &AppState) -> Result<OperatorActivityListResponse, ApiError> {
    let now = Utc::now();
    let environment_scope = environment_scope_from_config(&state.config);
    let mut records = Vec::new();

    if let Some(policy) = state.policy.as_deref() {
        for approval in policy.list_approvals(None) {
            records.push(crate::types::OperatorActivityRecord {
                activity_id: format!("approval-{}", approval.approval_id),
                source_kind: "approval".to_string(),
                source_id: approval.approval_id.clone(),
                title: format!("Approval {}", approval.action),
                status: approval.status.as_str().to_string(),
                summary: approval.reason.clone(),
                attention_level: attention_level_for_approval(&approval),
                occurred_at: approval.updated_at,
                detail_route: format!("/v1/policy/approvals/{}", approval.approval_id),
                related_resource_refs: Vec::new(),
                environment_scope: environment_scope.clone(),
            });
        }
    }

    if let Some(scheduler) = state.scheduler.as_deref() {
        for item in scheduler.list().map_err(ApiError::internal)? {
            records.push(crate::types::OperatorActivityRecord {
                activity_id: format!("schedule-{}", item.schedule_id),
                source_kind: "schedule".to_string(),
                source_id: item.schedule_id.clone(),
                title: format!("Schedule {}", item.target.summary),
                status: item.status.as_str().to_string(),
                summary: schedule_summary(&item),
                attention_level: attention_level_for_schedule(&item),
                occurred_at: item.updated_at,
                detail_route: format!("/v1/schedules/{}", item.schedule_id),
                related_resource_refs: build_schedule_refs(&item),
                environment_scope: environment_scope.clone(),
            });
        }
    }

    if let Some(manager) = state.runtime.as_deref() {
        let runs = manager.list_runs();
        let mut workflow_rows = Vec::new();
        {
            let store = state.store.lock();
            for run in &runs {
                let workflows = store
                    .list_workflows(&environment_scope, &run.run_id)
                    .map_err(ApiError::from_store)?;
                workflow_rows.extend(workflows);
            }
        }
        for run in runs {
            records.push(crate::types::OperatorActivityRecord {
                activity_id: format!("run-{}", run.run_id),
                source_kind: source_kind_for_run(&run),
                source_id: run.run_id.clone(),
                title: format!("Run {}", first_non_empty(&[&run.goal, &run.entrypoint])),
                status: run.status.as_str().to_string(),
                summary: run_summary(&run),
                attention_level: attention_level_for_run(&run),
                occurred_at: run.updated_at,
                detail_route: format!("/v1/runs/{}", run.run_id),
                related_resource_refs: Vec::new(),
                environment_scope: environment_scope.clone(),
            });
        }
        for workflow in workflow_rows {
            records.push(crate::types::OperatorActivityRecord {
                activity_id: format!("workflow-{}", workflow.workflow_id),
                source_kind: "workflow".to_string(),
                source_id: workflow.workflow_id.clone(),
                title: format!(
                    "Workflow {}",
                    first_non_empty(&[&workflow.goal, &workflow.workflow_id]),
                ),
                status: workflow.status.as_str().to_string(),
                summary: workflow_summary(&workflow),
                attention_level: attention_level_for_workflow(&workflow),
                occurred_at: workflow.updated_at,
                detail_route: format!(
                    "/v1/runs/{}/workflows/{}",
                    workflow.run_id, workflow.workflow_id,
                ),
                related_resource_refs: vec![crate::types::OperatorResourceRef {
                    kind: "run".to_string(),
                    id: workflow.run_id.clone(),
                    route: format!("/v1/runs/{}", workflow.run_id),
                }],
                environment_scope: environment_scope.clone(),
            });
        }
    }

    if let Some(delivery) = state.delivery.as_deref() {
        for item in delivery
            .list_outcomes(delivery::OutcomeFilter::default())
            .map_err(ApiError::internal)?
        {
            records.push(crate::types::OperatorActivityRecord {
                activity_id: format!("delivery-{}", item.delivery_id),
                source_kind: "delivery".to_string(),
                source_id: item.delivery_id.clone(),
                title: format!("Delivery {}", item.result_class.as_str()),
                status: item.status.as_str().to_string(),
                summary: delivery_summary(&item),
                attention_level: attention_level_for_delivery(&item),
                occurred_at: item.updated_at,
                detail_route: format!("/v1/deliveries/{}", item.delivery_id),
                related_resource_refs: build_delivery_refs(&item),
                environment_scope: environment_scope.clone(),
            });
        }
    }

    // Go buildEventBackedActivity: event-ledger records not already surfaced by
    // the manager enumerations above.
    let ledger_events = {
        let store = state.store.lock();
        store
            .list_events(&events::Filter {
                environment_scope: environment_scope.clone(),
                ..events::Filter::default()
            })
            .map_err(ApiError::from_store)?
    };
    let mut seen: HashSet<String> = records
        .iter()
        .map(|record| format!("{}::{}", record.source_kind, record.source_id))
        .collect();
    for event in ledger_events {
        let Some(record) = operator_activity_record_from_event(&event, &environment_scope) else {
            continue;
        };
        let key = format!("{}::{}", record.source_kind, record.source_id);
        if seen.insert(key) {
            records.push(record);
        }
    }

    records.sort_by(|a, b| b.occurred_at.cmp(&a.occurred_at));
    Ok(OperatorActivityListResponse {
        environment_scope,
        items: records,
        generated_at: now,
    })
}

fn build_diagnostics(state: &AppState) -> Result<OperatorDiagnosticListResponse, ApiError> {
    let now = Utc::now();
    let environment_scope = environment_scope_from_config(&state.config);
    let mut findings = Vec::new();

    let onboarding = build_onboarding(state, "", true);
    for item in onboarding.readiness_items {
        if item.status == "ready" || item.status == "optional" {
            continue;
        }
        findings.push(crate::types::OperatorDiagnosticFinding {
            finding_id: format!("readiness-{}", item.item_id),
            source_kind: item.item_kind,
            source_id: item.resource_id,
            plane: "readiness".to_string(),
            severity: severity_for_readiness(&item.status),
            status: item.status,
            reason: first_non_empty(&[&item.reason, "Readiness is not satisfied."]),
            recommended_action: item.required_operator_action,
            detail_route: item.detail_route,
            related_resource_refs: Vec::new(),
            environment_scope: environment_scope.clone(),
            captured_at: item.updated_at,
        });
    }

    if let Some(policy) = state.policy.as_deref() {
        for approval in policy.list_approvals(Some(policy::ApprovalStatus::Pending)) {
            findings.push(crate::types::OperatorDiagnosticFinding {
                finding_id: format!("approval-{}", approval.approval_id),
                source_kind: "approval".to_string(),
                source_id: approval.approval_id.clone(),
                plane: "approval".to_string(),
                severity: "warning".to_string(),
                status: approval.status.as_str().to_string(),
                reason: approval.reason.clone(),
                recommended_action: "Approve or reject the pending request.".to_string(),
                detail_route: format!("/v1/policy/approvals/{}", approval.approval_id),
                related_resource_refs: Vec::new(),
                environment_scope: environment_scope.clone(),
                captured_at: approval.updated_at,
            });
        }
    }

    if let Some(scheduler) = state.scheduler.as_deref() {
        for item in scheduler.list().map_err(ApiError::internal)? {
            if matches!(
                item.status,
                scheduler::ScheduleStatus::Active
                    | scheduler::ScheduleStatus::Scheduled
                    | scheduler::ScheduleStatus::Completed,
            ) {
                continue;
            }
            findings.push(crate::types::OperatorDiagnosticFinding {
                finding_id: format!("schedule-{}", item.schedule_id),
                source_kind: "schedule".to_string(),
                source_id: item.schedule_id.clone(),
                plane: "execution".to_string(),
                severity: severity_for_schedule(&item),
                status: item.status.as_str().to_string(),
                reason: schedule_summary(&item),
                recommended_action: "Inspect the schedule target and its latest attempt.".to_string(),
                detail_route: format!("/v1/schedules/{}", item.schedule_id),
                related_resource_refs: build_schedule_refs(&item),
                environment_scope: environment_scope.clone(),
                captured_at: item.updated_at,
            });
        }
    }

    if let Some(manager) = state.runtime.as_deref() {
        let runs = manager.list_runs();
        let mut workflow_rows = Vec::new();
        {
            let store = state.store.lock();
            for run in &runs {
                let workflows = store
                    .list_workflows(&environment_scope, &run.run_id)
                    .map_err(ApiError::from_store)?;
                workflow_rows.extend(workflows);
            }
        }
        for run in runs {
            if !matches!(
                run.status,
                runtime::RunStatus::Blocked
                    | runtime::RunStatus::Failed
                    | runtime::RunStatus::Cancelled,
            ) {
                continue;
            }
            findings.push(crate::types::OperatorDiagnosticFinding {
                finding_id: format!("run-{}", run.run_id),
                source_kind: "run".to_string(),
                source_id: run.run_id.clone(),
                plane: "execution".to_string(),
                severity: severity_for_run(&run),
                status: run.status.as_str().to_string(),
                reason: run_summary(&run),
                recommended_action: "Inspect the run and any linked workflow or approval blockers."
                    .to_string(),
                detail_route: format!("/v1/runs/{}", run.run_id),
                related_resource_refs: Vec::new(),
                environment_scope: environment_scope.clone(),
                captured_at: run.updated_at,
            });
        }
        for workflow in workflow_rows {
            if matches!(
                workflow.status,
                orchestration::WorkflowStatus::Completed
                    | orchestration::WorkflowStatus::Planned
                    | orchestration::WorkflowStatus::Running,
            ) {
                continue;
            }
            findings.push(crate::types::OperatorDiagnosticFinding {
                finding_id: format!("workflow-{}", workflow.workflow_id),
                source_kind: "workflow".to_string(),
                source_id: workflow.workflow_id.clone(),
                plane: "execution".to_string(),
                severity: severity_for_workflow(&workflow),
                status: workflow.status.as_str().to_string(),
                reason: workflow_summary(&workflow),
                recommended_action: "Inspect the workflow plan, steps, and linked delivery state."
                    .to_string(),
                detail_route: format!(
                    "/v1/runs/{}/workflows/{}",
                    workflow.run_id, workflow.workflow_id,
                ),
                related_resource_refs: Vec::new(),
                environment_scope: environment_scope.clone(),
                captured_at: workflow.updated_at,
            });
            findings.extend(computer_use_findings(state, &workflow)?);
        }
    }

    if let Some(delivery) = state.delivery.as_deref() {
        for item in delivery
            .list_outcomes(delivery::OutcomeFilter::default())
            .map_err(ApiError::internal)?
        {
            if !matches!(
                item.status,
                delivery::OutcomeStatus::Failed | delivery::OutcomeStatus::Suppressed,
            ) {
                continue;
            }
            findings.push(crate::types::OperatorDiagnosticFinding {
                finding_id: format!("delivery-{}", item.delivery_id),
                source_kind: "delivery".to_string(),
                source_id: item.delivery_id.clone(),
                plane: "delivery".to_string(),
                severity: severity_for_delivery(&item),
                status: item.status.as_str().to_string(),
                reason: delivery_summary(&item),
                recommended_action: "Inspect delivery attempts, target state, and source execution."
                    .to_string(),
                detail_route: format!("/v1/deliveries/{}", item.delivery_id),
                related_resource_refs: build_delivery_refs(&item),
                environment_scope: environment_scope.clone(),
                captured_at: item.updated_at,
            });
        }
    }

    // Go activationFindings + setupWizardFindings are NOT ported: kura-store has
    // no ListActivationStates / ResolveActiveTenantBinding / ListSetupSessions
    // CRUD yet (reported, not duplicated).

    findings.sort_by(|a, b| b.captured_at.cmp(&a.captured_at));
    Ok(OperatorDiagnosticListResponse {
        environment_scope,
        items: findings,
        generated_at: now,
    })
}

fn computer_use_findings(
    state: &AppState,
    workflow: &orchestration::Workflow,
) -> Result<Vec<crate::types::OperatorDiagnosticFinding>, ApiError> {
    let Some(computer_use) = state.computer_use.as_deref() else {
        return Ok(Vec::new());
    };
    let mut items = Vec::new();
    for step in &workflow.steps {
        if step.computer_use_session_id.is_empty() {
            continue;
        }
        let Ok(Some(session)) = computer_use.get_session(&workflow.run_id, &step.computer_use_session_id)
        else {
            continue;
        };
        match session.status {
            computeruse::SessionStatus::Blocked
            | computeruse::SessionStatus::Failed
            | computeruse::SessionStatus::Interrupted => {
                items.push(crate::types::OperatorDiagnosticFinding {
                    finding_id: format!("computer-use-session-{}", session.computer_use_session_id),
                    source_kind: "computer_use_session".to_string(),
                    source_id: session.computer_use_session_id.clone(),
                    plane: "execution".to_string(),
                    severity: "critical".to_string(),
                    status: session.status.as_str().to_string(),
                    reason: "Computer-use session needs operator attention.".to_string(),
                    recommended_action: "Inspect the browser session and its latest action.".to_string(),
                    detail_route: format!(
                        "/v1/runs/{}/computer-use/sessions/{}",
                        workflow.run_id, session.computer_use_session_id,
                    ),
                    related_resource_refs: Vec::new(),
                    environment_scope: environment_scope_from_config(&state.config),
                    captured_at: session.updated_at,
                });
            }
            _ => {}
        }
    }
    Ok(items)
}

// -- status / severity / attention mappers -----------------------------------

fn map_integration_readiness(item: &integrations::Resource) -> (String, String) {
    match item.readiness_status {
        integrations::ReadinessStatus::Healthy => (
            "ready".to_string(),
            first_non_empty(&[&item.health_state, "healthy"]),
        ),
        integrations::ReadinessStatus::Degraded => (
            "degraded".to_string(),
            first_non_empty(&[&item.health_state, "degraded"]),
        ),
        integrations::ReadinessStatus::Unavailable => (
            "blocked".to_string(),
            first_non_empty(&[&item.health_state, "unavailable"]),
        ),
        integrations::ReadinessStatus::AuthPending | integrations::ReadinessStatus::NotConfigured => (
            "missing_configuration".to_string(),
            first_non_empty(&[&item.health_state, "unknown"]),
        ),
    }
}

fn map_connector_status(status: connectors::Status) -> String {
    match status {
        connectors::Status::Healthy => "optional".to_string(),
        connectors::Status::Degraded | connectors::Status::BackingOff => "degraded".to_string(),
        connectors::Status::Failed => "blocked".to_string(),
        connectors::Status::Registered => "optional".to_string(),
        connectors::Status::Disabled => "blocked".to_string(),
    }
}

fn map_capability_status(status: kura_capabilities::Status) -> String {
    match status {
        kura_capabilities::Status::Healthy => "optional".to_string(),
        kura_capabilities::Status::Degraded | kura_capabilities::Status::BackingOff => {
            "degraded".to_string()
        }
        kura_capabilities::Status::Failed => "blocked".to_string(),
        kura_capabilities::Status::Registered => "optional".to_string(),
    }
}

fn connector_operator_action(item: &connectors::Connector) -> String {
    match item.status {
        connectors::Status::BackingOff => {
            "Wait for the scheduled restart or inspect the connector logs.".to_string()
        }
        connectors::Status::Failed => "Restart or reconfigure the connector.".to_string(),
        connectors::Status::Degraded => {
            "Inspect connector health and recover the downstream transport.".to_string()
        }
        connectors::Status::Healthy | connectors::Status::Registered | connectors::Status::Disabled => {
            String::new()
        }
    }
}

fn capability_operator_action(item: &kura_capabilities::Capability) -> String {
    match item.status {
        kura_capabilities::Status::BackingOff => {
            "Wait for the capability restart window or inspect its worker.".to_string()
        }
        kura_capabilities::Status::Failed => {
            "Restart or repair the capability implementation.".to_string()
        }
        kura_capabilities::Status::Degraded => {
            "Inspect capability health and recover the degraded dependency.".to_string()
        }
        kura_capabilities::Status::Healthy | kura_capabilities::Status::Registered => {
            String::new()
        }
    }
}

fn attention_level_for_approval(item: &policy::Approval) -> String {
    if item.status == policy::ApprovalStatus::Pending {
        "warning".to_string()
    } else {
        "info".to_string()
    }
}

fn attention_level_for_schedule(item: &scheduler::Schedule) -> String {
    match item.status {
        scheduler::ScheduleStatus::DispatchFailed | scheduler::ScheduleStatus::Cancelled => {
            "critical".to_string()
        }
        scheduler::ScheduleStatus::Paused => "warning".to_string(),
        _ => "info".to_string(),
    }
}

fn attention_level_for_run(item: &runtime::Run) -> String {
    match item.status {
        runtime::RunStatus::Failed | runtime::RunStatus::Cancelled => "critical".to_string(),
        runtime::RunStatus::Blocked | runtime::RunStatus::WaitingInput => "warning".to_string(),
        _ => "info".to_string(),
    }
}

fn attention_level_for_workflow(item: &orchestration::Workflow) -> String {
    match item.status {
        orchestration::WorkflowStatus::Failed
        | orchestration::WorkflowStatus::Interrupted
        | orchestration::WorkflowStatus::PlanningFailed => "critical".to_string(),
        orchestration::WorkflowStatus::Blocked
        | orchestration::WorkflowStatus::PartialFailed
        | orchestration::WorkflowStatus::Cancelled => "warning".to_string(),
        _ => "info".to_string(),
    }
}

fn attention_level_for_delivery(item: &delivery::DeliveryOutcome) -> String {
    match item.status {
        delivery::OutcomeStatus::Failed => "critical".to_string(),
        delivery::OutcomeStatus::Suppressed => "warning".to_string(),
        _ => "info".to_string(),
    }
}

fn severity_for_readiness(status: &str) -> String {
    if status == "missing_configuration" || status == "blocked" {
        "critical".to_string()
    } else {
        "warning".to_string()
    }
}

fn severity_for_schedule(item: &scheduler::Schedule) -> String {
    if matches!(
        item.status,
        scheduler::ScheduleStatus::DispatchFailed | scheduler::ScheduleStatus::Cancelled,
    ) {
        "critical".to_string()
    } else {
        "warning".to_string()
    }
}

fn severity_for_run(item: &runtime::Run) -> String {
    if item.status == runtime::RunStatus::Failed {
        "critical".to_string()
    } else {
        "warning".to_string()
    }
}

fn severity_for_workflow(item: &orchestration::Workflow) -> String {
    match item.status {
        orchestration::WorkflowStatus::Failed
        | orchestration::WorkflowStatus::PlanningFailed
        | orchestration::WorkflowStatus::Interrupted => "critical".to_string(),
        _ => "warning".to_string(),
    }
}

fn severity_for_delivery(item: &delivery::DeliveryOutcome) -> String {
    if item.status == delivery::OutcomeStatus::Failed {
        "critical".to_string()
    } else {
        "warning".to_string()
    }
}

// -- summaries ----------------------------------------------------------------

fn source_kind_for_run(item: &runtime::Run) -> String {
    if item.entrypoint == OPERATOR_SHELL_TEST_ENTRYPOINT {
        "first_action".to_string()
    } else {
        "run".to_string()
    }
}

fn run_summary(item: &runtime::Run) -> String {
    let mut parts = vec![format!("Entrypoint {}", item.entrypoint)];
    if !item.goal.is_empty() {
        parts.push(format!("goal: {}", item.goal));
    }
    if !item.active_workflow_id.is_empty() {
        parts.push(format!("active workflow {}", item.active_workflow_id));
    }
    if !item.latest_delivery_status.is_empty() {
        parts.push(format!("latest delivery {}", item.latest_delivery_status));
    }
    parts.join(" | ")
}

fn workflow_summary(item: &orchestration::Workflow) -> String {
    let mut parts = vec![first_non_empty(&[
        &item.plan_summary,
        "Workflow state projected from daemon truth.",
    ])];
    if !item.failure_summary.is_empty() {
        parts.push(item.failure_summary.clone());
    }
    if !item.latest_delivery_status.is_empty() {
        parts.push(format!("latest delivery {}", item.latest_delivery_status));
    }
    parts.join(" | ")
}

fn schedule_summary(item: &scheduler::Schedule) -> String {
    let mut parts = vec![format!("Trigger {}", item.trigger.kind.as_str())];
    if !item.last_outcome.is_empty() {
        parts.push(format!("last outcome {}", item.last_outcome));
    }
    if let Some(last) = item.attempts.last() {
        if !last.failure_reason.is_empty() {
            parts.push(last.failure_reason.clone());
        } else if !last.dispatch_status.as_str().is_empty() {
            parts.push(format!("dispatch {}", last.dispatch_status.as_str()));
        }
    }
    parts.join(" | ")
}

fn delivery_summary(item: &delivery::DeliveryOutcome) -> String {
    let mut parts = vec![format!("Source {}", item.source_kind)];
    if !item.payload_preview.is_empty() {
        parts.push(item.payload_preview.clone());
    }
    if !item.suppression_reason.is_empty() {
        parts.push(item.suppression_reason.clone());
    }
    parts.join(" | ")
}

// -- refs ---------------------------------------------------------------------

fn build_schedule_refs(item: &scheduler::Schedule) -> Vec<crate::types::OperatorResourceRef> {
    let mut refs = Vec::new();
    for attempt in &item.attempts {
        if !attempt.run_id.is_empty() {
            refs.push(crate::types::OperatorResourceRef {
                kind: "run".to_string(),
                id: attempt.run_id.clone(),
                route: format!("/v1/runs/{}", attempt.run_id),
            });
        }
        if !attempt.workflow_id.is_empty() {
            refs.push(crate::types::OperatorResourceRef {
                kind: "workflow".to_string(),
                id: attempt.workflow_id.clone(),
                route: format!("/v1/runs/{}/workflows/{}", attempt.run_id, attempt.workflow_id),
            });
        }
        if !attempt.latest_delivery_id.is_empty() {
            refs.push(crate::types::OperatorResourceRef {
                kind: "delivery".to_string(),
                id: attempt.latest_delivery_id.clone(),
                route: format!("/v1/deliveries/{}", attempt.latest_delivery_id),
            });
        }
    }
    refs
}

fn build_delivery_refs(item: &delivery::DeliveryOutcome) -> Vec<crate::types::OperatorResourceRef> {
    let mut refs = Vec::new();
    if !item.run_id.is_empty() {
        refs.push(crate::types::OperatorResourceRef {
            kind: "run".to_string(),
            id: item.run_id.clone(),
            route: format!("/v1/runs/{}", item.run_id),
        });
    }
    if !item.workflow_id.is_empty() {
        refs.push(crate::types::OperatorResourceRef {
            kind: "workflow".to_string(),
            id: item.workflow_id.clone(),
            route: format!("/v1/runs/{}/workflows/{}", item.run_id, item.workflow_id),
        });
    }
    if !item.schedule_id.is_empty() {
        refs.push(crate::types::OperatorResourceRef {
            kind: "schedule".to_string(),
            id: item.schedule_id.clone(),
            route: format!("/v1/schedules/{}", item.schedule_id),
        });
    }
    refs
}

fn build_event_refs(event: &events::Event) -> Vec<crate::types::OperatorResourceRef> {
    let mut refs = Vec::new();
    if !event.scope.run_id.is_empty() {
        refs.push(crate::types::OperatorResourceRef {
            kind: "run".to_string(),
            id: event.scope.run_id.clone(),
            route: format!("/v1/runs/{}", event.scope.run_id),
        });
    }
    if !event.scope.workflow_id.is_empty() && !event.scope.run_id.is_empty() {
        refs.push(crate::types::OperatorResourceRef {
            kind: "workflow".to_string(),
            id: event.scope.workflow_id.clone(),
            route: format!(
                "/v1/runs/{}/workflows/{}",
                event.scope.run_id, event.scope.workflow_id,
            ),
        });
    }
    if !event.scope.schedule_id.is_empty() {
        refs.push(crate::types::OperatorResourceRef {
            kind: "schedule".to_string(),
            id: event.scope.schedule_id.clone(),
            route: format!("/v1/schedules/{}", event.scope.schedule_id),
        });
    }
    refs
}

// -- event-backed activity -----------------------------------------------------

fn operator_activity_record_from_event(
    event: &events::Event,
    environment: &str,
) -> Option<crate::types::OperatorActivityRecord> {
    let source_kind = operator_source_kind_for_event(event);
    if source_kind.is_empty() || event.resource.id.is_empty() {
        return None;
    }
    Some(crate::types::OperatorActivityRecord {
        activity_id: format!("event-{}", event.event_id),
        source_kind,
        source_id: event.resource.id.clone(),
        title: operator_event_title(event),
        status: operator_event_status(event),
        summary: operator_event_summary(event),
        attention_level: attention_level_for_event(event),
        occurred_at: event.occurred_at,
        detail_route: operator_detail_route_for_event(event),
        related_resource_refs: build_event_refs(event),
        environment_scope: environment.to_string(),
    })
}

fn operator_source_kind_for_event(event: &events::Event) -> String {
    match event.resource.kind.as_str() {
        "approval" | "schedule" | "run" | "workflow" | "delivery" | "computer_use_session"
        | "computer_use_action" => event.resource.kind.clone(),
        "decision" => "approval".to_string(),
        _ => match event.category.as_str() {
            "approval" | "schedule" | "run" | "workflow" | "delivery" | "computer_use" => {
                event.category.clone()
            }
            _ => String::new(),
        },
    }
}

fn operator_event_title(event: &events::Event) -> String {
    let source_kind = operator_source_kind_for_event(event);
    let resource_label = first_non_empty(&[&event.resource.id, &event.name]);
    if source_kind.is_empty() {
        return first_non_empty(&[&event.name, "Operator event"]);
    }
    format!("{} {}", operator_source_label(&source_kind), resource_label)
}

fn operator_event_status(event: &events::Event) -> String {
    if let Some(serde_json::Value::String(status)) = event.payload.get("status") {
        let trimmed = status.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    if event.name.contains('.') {
        if let Some(last) = event.name.split('.').last() {
            if !last.is_empty() {
                return last.to_string();
            }
        }
    }
    first_non_empty(&[&event.category, "observed"])
}

fn operator_event_summary(event: &events::Event) -> String {
    let status = operator_event_status(event);
    format!(
        "{} via persisted event {}",
        status.replace('_', " "),
        event.name,
    )
}

fn attention_level_for_event(event: &events::Event) -> String {
    let status = operator_event_status(event).to_lowercase();
    let name = event.name.to_lowercase();
    if status.contains("fail")
        || status.contains("cancel")
        || status.contains("reject")
        || name.contains("failed")
        || name.contains("cancelled")
        || name.contains("rejected")
    {
        return "critical".to_string();
    }
    if status.contains("block")
        || status.contains("wait")
        || status.contains("pending")
        || status.contains("pause")
        || name.contains("blocked")
        || name.contains("pending")
        || name.contains("paused")
    {
        return "warning".to_string();
    }
    "info".to_string()
}

fn operator_detail_route_for_event(event: &events::Event) -> String {
    match operator_source_kind_for_event(event).as_str() {
        "approval" => {
            if let Some(serde_json::Value::String(approval_id)) = event.payload.get("approvalId") {
                let trimmed = approval_id.trim();
                if !trimmed.is_empty() {
                    return format!("/v1/policy/approvals/{}", trimmed);
                }
            }
            format!("/v1/policy/approvals/{}", event.resource.id)
        }
        "schedule" => format!("/v1/schedules/{}", event.resource.id),
        "run" => format!("/v1/runs/{}", event.resource.id),
        "workflow" => {
            if !event.scope.run_id.is_empty() {
                format!("/v1/runs/{}/workflows/{}", event.scope.run_id, event.resource.id)
            } else {
                String::new()
            }
        }
        "delivery" => format!("/v1/deliveries/{}", event.resource.id),
        "computer_use_session" => {
            if !event.scope.run_id.is_empty() {
                format!(
                    "/v1/runs/{}/computer-use/sessions/{}",
                    event.scope.run_id, event.resource.id,
                )
            } else {
                String::new()
            }
        }
        "computer_use_action" => {
            if !event.scope.run_id.is_empty() && !event.scope.computer_use_session_id.is_empty() {
                format!(
                    "/v1/runs/{}/computer-use/sessions/{}/actions/{}",
                    event.scope.run_id, event.scope.computer_use_session_id, event.resource.id,
                )
            } else {
                String::new()
            }
        }
        _ => String::new(),
    }
}

fn operator_source_label(source_kind: &str) -> String {
    match source_kind {
        "approval" => "Approval".to_string(),
        "schedule" => "Schedule".to_string(),
        "run" => "Run".to_string(),
        "workflow" => "Workflow".to_string(),
        "delivery" => "Delivery".to_string(),
        "computer_use_session" => "Computer Use Session".to_string(),
        "computer_use_action" => "Computer Use Action".to_string(),
        _ => "Operator Event".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Workflow action builders (Go buildCalendarAction / buildMailAction; shared
// with the workflows family)
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
    action.window_end = parse_optional_action_time(&request.window_end)
        .map_err(|err| format!("parse windowEnd: {err}"))?;
    action.starts_at = parse_optional_action_time(&request.starts_at)
        .map_err(|err| format!("parse startsAt: {err}"))?;
    action.ends_at = parse_optional_action_time(&request.ends_at)
        .map_err(|err| format!("parse endsAt: {err}"))?;
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
// Tests (ports of the Go handler tests for runs / schedules / delivery /
// events / operator)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;
    use std::time::Duration as StdDuration;

    use axum::body::Body;
    use axum::body::to_bytes;
    use axum::http::Request as HttpRequest;
    use kura_delivery::{
        DeliveryAdapter, DeliveryOutcome, DeliveryTarget, Manager as DeliveryManager, SendResult,
        TargetKind, TestSinkAdapter,
    };
    use kura_events::Bus;
    use kura_identity::TenantContext as IdentityTenantContext;
    use kura_runtime::Manager as RuntimeManager;
    use kura_router::SessionRouter;
    use kura_scheduler::{
        Dependencies as SchedulerDependencies, Scheduler,
    };
    use kura_store::SQLiteStore;
    use futures::StreamExt;
    use parking_lot::Mutex;
    use tower::ServiceExt;
    use uuid::Uuid;

    fn test_config() -> kura_config::Config {
        kura_config::Config {
            project_root: String::new(),
            environment: kura_config::Environment::Test,
            bind_addr: "127.0.0.1:19192".to_string(),
            data_dir: "/tmp/kura-api-test".to_string(),
            log_level: "info".to_string(),
            version: "0.1.0".to_string(),
            llm: kura_config::LlmConfig::default(),
            connectors: kura_config::ConnectorConfig {
                discord: kura_config::DiscordConnectorConfig {
                    enabled: false,
                    ..Default::default()
                },
                telegram: kura_config::TelegramConnectorConfig {
                    enabled: false,
                    ..Default::default()
                },
                slack: kura_config::SlackConnectorConfig {
                    enabled: false,
                    ..Default::default()
                },
                matrix: kura_config::MatrixConnectorConfig {
                    enabled: false,
                    ..Default::default()
                },
            },
        }
    }

    fn new_store() -> Arc<Mutex<SQLiteStore>> {
        let dir = std::env::temp_dir().join(format!("kura-api-runs-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        Arc::new(Mutex::new(
            SQLiteStore::new(dir.to_str().expect("path")).expect("store"),
        ))
    }

    fn request(method: &str, uri: &str, body: Option<&str>) -> HttpRequest<Body> {
        HttpRequest::builder()
            .method(method)
            .uri(uri)
            .body(match body {
                Some(body) => Body::from(body.to_string()),
                None => Body::empty(),
            })
            .expect("request")
    }

    /// Request carrying an authenticated token extension (as the protected
    /// middleware would install once an auth manager is wired).
    fn authenticated_request(uri: &str) -> HttpRequest<Body> {
        let now = Utc::now();
        let token = kura_identity::auth::AccessToken {
            token_id: "tok_operator_test".to_string(),
            principal_id: String::new(),
            label: String::new(),
            mode: kura_identity::auth::PairingMode::Local,
            token_hash: String::new(),
            token_preview: String::new(),
            status: kura_identity::auth::TokenStatus::Active,
            default_tenant_id: String::new(),
            created_at: now,
            updated_at: now,
            last_used_at: None,
            expires_at: None,
            revoked_at: None,
            rotated_from_token_id: String::new(),
            rotated_to_token_id: String::new(),
        };
        HttpRequest::builder()
            .method("GET")
            .uri(uri)
            .extension(AuthenticatedToken(token))
            .body(Body::empty())
            .expect("request")
    }

    /// Request carrying a resolved tenant context (as the protected middleware
    /// would install once an auth manager is wired).
    fn tenant_request(method: &str, uri: &str, tenant_id: &str) -> HttpRequest<Body> {
        HttpRequest::builder()
            .method(method)
            .uri(uri)
            .extension(TenantContext(IdentityTenantContext {
                tenant_id: tenant_id.to_string(),
                ..Default::default()
            }))
            .body(Body::empty())
            .expect("request")
    }

    async fn send(
        app: &axum::Router,
        req: HttpRequest<Body>,
    ) -> (StatusCode, serde_json::Value) {
        let response = app.clone().oneshot(req).await.expect("oneshot");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    fn append_test_event(
        store: &Arc<Mutex<SQLiteStore>>,
        event_id: &str,
        environment_scope: &str,
        category: &str,
        name: &str,
        schedule_id: &str,
        attempt_id: &str,
    ) {
        let event = events::Event {
            event_id: event_id.to_string(),
            environment_scope: environment_scope.to_string(),
            category: category.to_string(),
            name: name.to_string(),
            occurred_at: Utc::now(),
            scope: events::Scope {
                schedule_id: schedule_id.to_string(),
                schedule_attempt_id: attempt_id.to_string(),
                ..events::Scope::default()
            },
            resource: events::Resource {
                kind: "schedule".to_string(),
                id: schedule_id.to_string(),
            },
            payload: serde_json::json!({ "dispatchStatus": "dispatched" })
                .as_object()
                .cloned()
                .expect("payload object"),
            ..events::Event::default()
        };
        store.lock().append_event(&event).expect("append event");
    }

    // -- runs ----------------------------------------------------------------

    fn runs_state() -> (AppState, Arc<RuntimeManager>, Arc<SessionRouter>) {
        let store = new_store();
        let bus = Arc::new(Bus::new());
        let runtime = Arc::new(RuntimeManager::new());
        let session_router = Arc::new(SessionRouter::new());
        let mut state = AppState::new(test_config(), bus, store);
        state.runtime = Some(Arc::clone(&runtime));
        state.router = Some(Arc::clone(&session_router));
        (state, runtime, session_router)
    }

    #[tokio::test]
    async fn runs_lifecycle_create_list_get_and_event() {
        // Port of Go TestRunsLifecycleRoutes (runs portion): create, list, get,
        // and the run-scoped run.created event.
        let (state, _runtime, _router) = runs_state();
        let app = crate::routes::router(state);

        let (status, json) = send(
            &app,
            request(
                "POST",
                "/v1/runs",
                Some(r#"{"entrypoint":"chat","goal":"ship a task"}"#),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "create body: {json}");
        let run_id = json["runId"].as_str().expect("runId").to_string();
        assert!(!run_id.is_empty());

        let (status, json) = send(&app, request("GET", "/v1/runs", None)).await;
        assert_eq!(status, StatusCode::OK, "list body: {json}");
        assert_eq!(json["items"][0]["runId"], run_id);

        let (status, json) = send(
            &app,
            request("GET", &format!("/v1/runs/{run_id}"), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "get body: {json}");
        assert_eq!(json["runId"], run_id);

        let (status, json) = send(
            &app,
            request("GET", &format!("/v1/events?runId={run_id}"), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "events body: {json}");
        let items = json["items"].as_array().expect("items");
        assert_eq!(items.len(), 1, "expected 1 run-scoped event after run create");
        assert_eq!(items[0]["name"], "run.created");
    }

    #[tokio::test]
    async fn create_run_requires_body_and_entrypoint() {
        // Port of Go TestCreateRunRequiresBodyAndEntrypoint: a request without an
        // entrypoint maps to 400.
        let (state, _runtime, _router) = runs_state();
        let app = crate::routes::router(state);
        let (status, json) = send(&app, request("POST", "/v1/runs", Some("{}"))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "body: {json}");
    }

    #[tokio::test]
    async fn create_run_with_explicit_route() {
        // Port of Go TestCreateRunWithExplicitRoute: a route-based create binds the
        // run to a routed group session and persists it.
        let (state, _runtime, session_router) = runs_state();
        let store = Arc::clone(&state.store);
        let app = crate::routes::router(state);
        let (status, json) = send(
            &app,
            request(
                "POST",
                "/v1/runs",
                Some(
                    r#"{"entrypoint":"connector.message","goal":"route-aware run","route":{"kind":"group","channel":"telegram","accountId":"bot-main","peerId":"chat-1","threadId":"thread-1"}}"#,
                ),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "body: {json}");
        let session_id = json["sessionId"].as_str().expect("sessionId").to_string();
        assert!(!session_id.is_empty());
        let session = session_router
            .get_session(&session_id)
            .expect("routed session");
        assert_eq!(session.kind, router::SessionKind::Group);
        assert_eq!(session.channel, "telegram");
        let runs = store
            .lock()
            .list_runs_all_tenants_for_test()
            .expect("list runs");
        assert_eq!(runs.len(), 1, "expected persisted run");
        assert_eq!(runs[0].session_id, session_id);
    }

    #[tokio::test]
    async fn run_by_id_scoped_to_tenant() {
        // Port of the by-id tenant guard behavior for runs.run_id: a resolved
        // tenant that does not own the run sees 404.
        let store = new_store();
        let runtime = Arc::new(RuntimeManager::new());
        let run = runtime
            .create_run(runtime::CreateRunInput {
                entrypoint: "chat".to_string(),
                goal: "guarded".to_string(),
                ..runtime::CreateRunInput::default()
            })
            .expect("create run");
        store.lock().upsert_run(&run).expect("upsert run");
        store
            .lock()
            .bind_row_tenant("runs", "run_id", &run.run_id, "ten_a")
            .expect("bind tenant");
        let mut state = AppState::new(test_config(), Arc::new(Bus::new()), store);
        state.runtime = Some(runtime);
        let app = crate::routes::router(state);

        let (status, _) = send(
            &app,
            request("GET", &format!("/v1/runs/{}", run.run_id), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "no tenant context passes");
        let (status, _) = send(
            &app,
            tenant_request("GET", &format!("/v1/runs/{}", run.run_id), "ten_a"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "owning tenant passes");
        let (status, json) = send(
            &app,
            tenant_request("GET", &format!("/v1/runs/{}", run.run_id), "ten_b"),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "cross-tenant 404 body: {json}");
    }

    // -- schedules ------------------------------------------------------------

    fn schedule_state() -> (AppState, Arc<Scheduler>) {
        let store = new_store();
        let bus = Arc::new(Bus::new());
        let runtime = Arc::new(RuntimeManager::new());
        let scheduler = Arc::new(Scheduler::new(SchedulerDependencies {
            environment: kura_config::Environment::Test,
            runtime: Arc::clone(&runtime),
            event_bus: Some((*bus).clone()),
            store: Arc::clone(&store),
            workflow_launcher: None,
            clock: None,
            tick_interval: StdDuration::from_millis(10),
        }));
        let mut state = AppState::new(test_config(), bus, store);
        state.scheduler = Some(Arc::clone(&scheduler));
        state.runtime = Some(runtime);
        (state, scheduler)
    }

    #[tokio::test]
    async fn schedule_create_and_inspect_one_time() {
        // Port of Go TestScheduleRoutesCreateAndInspectOneTimeSchedule.
        let (state, _scheduler) = schedule_state();
        let app = crate::routes::router(state);
        let fire_at = (Utc::now() + chrono::Duration::minutes(1)).to_rfc3339();
        let body = format!(
            r#"{{"trigger":{{"kind":"once","fireAt":"{fire_at}"}},"target":{{"kind":"run","run":{{"entrypoint":"operator","goal":"dispatch one test run"}}}},"retryPolicy":{{"maxRetries":2,"backoffKind":"fixed","baseDelaySeconds":5,"maxDelaySeconds":5}}}}"#
        );
        let (status, json) = send(
            &app,
            request("POST", "/v1/schedules", Some(&body)),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "create body: {json}");
        let schedule_id = json["scheduleId"].as_str().expect("scheduleId").to_string();
        assert_eq!(json["status"], "scheduled");
        assert!(!json["targetRefId"].as_str().unwrap_or("").is_empty());

        let (status, json) = send(
            &app,
            request("GET", &format!("/v1/schedules/{schedule_id}"), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "get body: {json}");
        assert_eq!(json["scheduleId"], schedule_id);
        assert!(!json["nextDueAt"].as_str().unwrap_or("").is_empty());
    }

    #[tokio::test]
    async fn schedule_cancel_before_due_prevents_dispatch() {
        // Port of Go TestScheduleRoutesCancelBeforeDuePreventsDispatch.
        let (state, scheduler) = schedule_state();
        let app = crate::routes::router(state.clone());
        let fire_at = (Utc::now() + chrono::Duration::minutes(1)).to_rfc3339();
        let body = format!(
            r#"{{"trigger":{{"kind":"once","fireAt":"{fire_at}"}},"target":{{"kind":"run","run":{{"entrypoint":"operator","goal":"cancel via api"}}}},"retryPolicy":{{"maxRetries":0,"backoffKind":"fixed","baseDelaySeconds":5,"maxDelaySeconds":5}}}}"#
        );
        let (status, json) = send(
            &app,
            request("POST", "/v1/schedules", Some(&body)),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "create body: {json}");
        let schedule_id = json["scheduleId"].as_str().expect("scheduleId").to_string();
        let (status, json) = send(
            &app,
            request(
                "POST",
                &format!("/v1/schedules/{schedule_id}/cancel"),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "cancel body: {json}");
        assert_eq!(json["status"], "cancelled");
        scheduler.tick().expect("scheduler tick");
        assert!(
            state
                .runtime
                .as_ref()
                .expect("runtime manager")
                .list_runs()
                .is_empty(),
            "expected no run dispatch after cancel"
        );
    }

    #[tokio::test]
    async fn schedules_hidden_from_other_environment() {
        // Port of Go TestScheduleRoutesHideSchedulesFromOtherEnvironments.
        let (state, _scheduler) = schedule_state();
        let now = Utc::now();
        state
            .store
            .lock()
            .upsert_schedule(&kura_store::schedule::ScheduleRecord {
                schedule_id: "sched_prod_hidden".to_string(),
                environment_scope: "prod".to_string(),
                kind: "one_time".to_string(),
                status: "scheduled".to_string(),
                target_ref_id: "target_prod_hidden".to_string(),
                created_at: now,
                updated_at: now,
                document: serde_json::json!({
                    "scheduleId": "sched_prod_hidden",
                    "environmentScope": "prod",
                    "kind": "one_time",
                    "status": "scheduled",
                    "targetRefId": "target_prod_hidden",
                })
                .to_string(),
                ..Default::default()
            })
            .expect("upsert schedule");
        let app = crate::routes::router(state);
        let (status, json) = send(&app, request("GET", "/v1/schedules", None)).await;
        assert_eq!(status, StatusCode::OK, "list body: {json}");
        assert_eq!(
            json["items"].as_array().map(|items| items.len()).unwrap_or(0),
            0,
            "expected no cross-environment schedules"
        );
        let (status, _) = send(
            &app,
            request("GET", "/v1/schedules/sched_prod_hidden", None),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    // -- delivery -------------------------------------------------------------

    #[tokio::test]
    async fn delivery_routes_expose_targets_preferences_and_suppression() {
        // Port of Go TestDeliveryRoutesExposeTargetsPreferencesSuppressionAndEvents.
        let store = new_store();
        let bus = Arc::new(Bus::new());
        let delivery = Arc::new(DeliveryManager::new(
            "test",
            (*bus).clone(),
            Arc::clone(&store),
            vec![Arc::new(TestSinkAdapter::new()) as Arc<dyn DeliveryAdapter>],
        ));
        let mut state = AppState::new(test_config(), bus, store.clone());
        state.runtime = Some(Arc::new(RuntimeManager::new()));
        state.delivery = Some(Arc::clone(&delivery));
        let app = crate::routes::router(state.clone());

        let (status, json) = send(
            &app,
            request(
                "POST",
                "/v1/delivery/targets",
                Some(
                    r#"{"targetId":"ops-target","displayName":"Ops Target","targetKind":"test_sink","addressSummary":"ops sink"}"#,
                ),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "target body: {json}");
        assert_eq!(json["targetId"], "ops-target");
        assert_eq!(json["status"], "active");

        let (status, json) = send(
            &app,
            request("POST", "/v1/delivery/targets/ops-target/disable", None),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "disable body: {json}");
        assert_eq!(json["status"], "disabled");
        let (status, json) = send(
            &app,
            request("POST", "/v1/delivery/targets/ops-target/activate", None),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "activate body: {json}");
        assert_eq!(json["status"], "active");

        let (status, json) = send(
            &app,
            request(
                "POST",
                "/v1/delivery/preferences",
                Some(
                    r#"{"preferenceId":"ops-pref","scopeKind":"user_default","preferredTargetsByClass":{"routine_success":"ops-target","urgent":"ops-target","failure":"ops-target"},"suppressionPolicy":{"suppressFailure":true}}"#,
                ),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "pref body: {json}");
        assert_eq!(json["suppressionPolicy"]["suppressFailure"], true);

        let outcome = delivery
            .emit_outcome(delivery::OutcomeInput {
                source_kind: "run".to_string(),
                source_id: "suppressed_run".to_string(),
                run_id: "suppressed_run".to_string(),
                result_class: delivery::ResultClass::Failure,
                payload_preview: "suppressed failure".to_string(),
                ..delivery::OutcomeInput::default()
            })
            .expect("emit outcome");
        assert_eq!(outcome.status, delivery::OutcomeStatus::Suppressed);

        let (status, json) = send(
            &app,
            request("GET", "/v1/deliveries?sourceId=suppressed_run", None),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "deliveries body: {json}");
        let items = json["items"].as_array().expect("items");
        assert_eq!(items.len(), 1, "expected one delivery outcome");
        assert_eq!(items[0]["status"], "suppressed");
        assert!(!items[0]["suppressionReason"].as_str().unwrap_or("").is_empty());

        let names: Vec<String> = store
            .lock()
            .list_events(&events::Filter {
                category: "delivery".to_string(),
                ..events::Filter::default()
            })
            .expect("list delivery events")
            .iter()
            .map(|event| event.name.clone())
            .collect();
        assert!(names.contains(&"delivery.target_registered".to_string()));
        assert!(names.contains(&"delivery.target_status_changed".to_string()));
        assert!(names.contains(&"delivery.preference_updated".to_string()));
    }

    // -- events ---------------------------------------------------------------

    #[tokio::test]
    async fn events_list_filters_by_environment_and_scope() {
        // Port of Go TestScheduleEventsRouteFiltersByEnvironmentAndScheduleScope:
        // events are read from the environment-scoped ledger; the prod-scope row
        // stays hidden. (The scheduler-manager divergence — schedule events are
        // bus-only — is worked around by appending the ledger rows directly.)
        let store = new_store();
        let state = AppState::new(test_config(), Arc::new(Bus::new()), store.clone());
        let app = crate::routes::router(state.clone());
        append_test_event(
            &store,
            "evt_prod_schedule_hidden",
            "prod",
            "schedule",
            "schedule.dispatch_recorded",
            "sched_prod_hidden",
            "sched_attempt_prod_hidden",
        );
        append_test_event(
            &store,
            "evt_test_schedule",
            "test",
            "schedule",
            "schedule.dispatch_recorded",
            "sched_test",
            "sched_attempt_test",
        );
        let (status, json) = send(
            &app,
            request(
                "GET",
                "/v1/events?category=schedule&scheduleId=sched_test",
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "events body: {json}");
        let items = json["items"].as_array().expect("items");
        assert_eq!(
            items.len(),
            1,
            "expected exactly one schedule event for current env and schedule"
        );
        assert_eq!(items[0]["scope"]["scheduleId"], "sched_test");
    }

    #[tokio::test]
    async fn events_cursor_must_be_non_negative() {
        // Port of Go parseEventCursor validation: a negative or non-numeric cursor
        // maps to 400.
        let state = AppState::new(test_config(), Arc::new(Bus::new()), new_store());
        let app = crate::routes::router(state);
        let (status, json) = send(
            &app,
            request("GET", "/v1/events?cursor=-1", None),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "body: {json}");
        let (status, _) = send(
            &app,
            request("GET", "/v1/events?cursor=abc", None),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn event_stream_replays_matching_history() {
        // Port of Go TestEventStreamReplaysMatchingHistory: the SSE stream replays
        // the matching history with the id/event/data frame shape; the test reads a
        // bounded number of frames then drops the body (the live fan-out keeps the
        // stream open, exactly like Go's keep-alive loop).
        let store = new_store();
        let bus = Arc::new(Bus::new());
        let runtime = Arc::new(RuntimeManager::new());
        let mut state = AppState::new(test_config(), bus.clone(), store);
        state.runtime = Some(Arc::clone(&runtime));
        let run = runtime
            .create_run(runtime::CreateRunInput {
                entrypoint: "chat".to_string(),
                goal: "stream events".to_string(),
                ..runtime::CreateRunInput::default()
            })
            .expect("create run");
        let event = events::Event {
            environment_scope: "test".to_string(),
            category: "run".to_string(),
            name: "run.created".to_string(),
            scope: events::Scope {
                run_id: run.run_id.clone(),
                ..events::Scope::default()
            },
            resource: events::Resource {
                kind: "run".to_string(),
                id: run.run_id.clone(),
            },
            payload: serde_json::json!({
                "entrypoint": run.entrypoint,
                "goal": run.goal,
            })
            .as_object()
            .cloned()
            .expect("payload object"),
            ..events::Event::default()
        };
        let stored = state.store.lock().append_event(&event).expect("append event");
        bus.publish(stored);
        let app = crate::routes::router(state);

        let response = app
            .clone()
            .oneshot(request(
                "GET",
                &format!("/v1/events/stream?runId={}", run.run_id),
                None,
            ))
            .await
            .expect("oneshot");
        assert_eq!(response.status(), StatusCode::OK);
        let mut stream = response.into_body().into_data_stream();
        let mut content = String::new();
        let mut reads = 0;
        while reads < 12 {
            match stream.next().await {
                Some(Ok(chunk)) => {
                    content.push_str(&String::from_utf8_lossy(&chunk));
                    if content.contains("run.created") && content.contains("id: 1") {
                        break;
                    }
                }
                _ => break,
            }
            reads += 1;
        }
        assert!(
            content.contains("run.created"),
            "expected SSE stream to contain run.created, got {content:?}"
        );
        assert!(
            content.contains("id: 1"),
            "expected SSE stream to contain cursor id, got {content:?}"
        );
    }

    // -- operator -------------------------------------------------------------

    /// Go operatorFailingDeliveryAdapter: makes immediate delivery fail so
    /// outcomes reach the failed state.
    struct FailingDeliveryAdapter {
        target_kind: TargetKind,
        err: String,
    }

    impl DeliveryAdapter for FailingDeliveryAdapter {
        fn supports(&self, kind: TargetKind) -> bool {
            kind == self.target_kind
        }

        fn send(
            &self,
            _target: DeliveryTarget,
            _outcome: DeliveryOutcome,
        ) -> Result<SendResult, String> {
            Err(self.err.clone())
        }
    }

    struct OperatorHarness {
        state: AppState,
        runtime: Arc<RuntimeManager>,
        policy: Arc<kura_policy::Engine>,
        scheduler: Arc<Scheduler>,
        delivery: Arc<DeliveryManager>,
        store: Arc<Mutex<SQLiteStore>>,
    }

    fn operator_harness(adapters: Vec<Arc<dyn DeliveryAdapter>>) -> OperatorHarness {
        let store = new_store();
        let bus = Arc::new(Bus::new());
        let runtime = Arc::new(RuntimeManager::new());
        let policy = Arc::new(kura_policy::Engine::new());
        let delivery = Arc::new(DeliveryManager::new(
            "test",
            (*bus).clone(),
            Arc::clone(&store),
            adapters,
        ));
        let scheduler = Arc::new(Scheduler::new(SchedulerDependencies {
            environment: kura_config::Environment::Test,
            runtime: Arc::clone(&runtime),
            event_bus: Some((*bus).clone()),
            store: Arc::clone(&store),
            workflow_launcher: None,
            clock: None,
            tick_interval: StdDuration::from_millis(10),
        }));
        let mut state = AppState::new(test_config(), bus, store.clone());
        state.runtime = Some(Arc::clone(&runtime));
        state.policy = Some(Arc::clone(&policy));
        state.scheduler = Some(Arc::clone(&scheduler));
        state.delivery = Some(Arc::clone(&delivery));
        state.integrations = Some(Arc::new(integrations::Manager::new("test")));
        state.connectors = Some(Arc::new(connectors::Supervisor::new()));
        state.capabilities = Some(Arc::new(kura_capabilities::Supervisor::new()));
        state.providers = Some(Arc::new(providers::new_manager(
            kura_config::LlmConfig {
                default_provider: "echo".to_string(),
                ..Default::default()
            },
            None,
            vec![],
        )));
        OperatorHarness {
            state,
            runtime,
            policy,
            scheduler,
            delivery,
            store,
        }
    }

    impl OperatorHarness {
        fn seed_run(&self, entrypoint: &str, goal: &str, status: runtime::RunStatus) -> runtime::Run {
            let mut run = self
                .runtime
                .create_run(runtime::CreateRunInput {
                    entrypoint: entrypoint.to_string(),
                    goal: goal.to_string(),
                    ..runtime::CreateRunInput::default()
                })
                .expect("create run");
            run.status = status;
            run.updated_at = Utc::now();
            self.runtime.restore_run_checkpoint(runtime::RunCheckpoint {
                run: run.clone(),
                ..runtime::RunCheckpoint::default()
            });
            self.store.lock().upsert_run(&run).expect("upsert run");
            run
        }

        fn seed_workflow(
            &self,
            run: &runtime::Run,
            status: orchestration::WorkflowStatus,
            plan_summary: &str,
        ) -> orchestration::Workflow {
            let now = Utc::now();
            let workflow = orchestration::Workflow {
                workflow_id: format!("wf_{}", run.run_id),
                run_id: run.run_id.clone(),
                environment_scope: "test".to_string(),
                goal: "Inspect operator workflow state".to_string(),
                status,
                plan_summary: plan_summary.to_string(),
                failure_summary: "workflow is blocked on operator follow-up".to_string(),
                created_at: now - chrono::Duration::minutes(1),
                updated_at: now,
                steps: vec![orchestration::WorkflowStep {
                    workflow_step_id: format!("wfstep_{}", run.run_id),
                    workflow_id: format!("wf_{}", run.run_id),
                    title: "Review operator state".to_string(),
                    position: 1,
                    consumer_kind: "skill".to_string(),
                    consumer_id: "operator".to_string(),
                    tool_name: "inspect".to_string(),
                    status: orchestration::StepStatus::Blocked,
                    approval_mode_expected: "allow".to_string(),
                    attempt_count: 1,
                    max_attempts: 1,
                    created_at: now - chrono::Duration::minutes(1),
                    updated_at: now,
                    ..orchestration::WorkflowStep::default()
                }],
                ..orchestration::Workflow::default()
            };
            let store = self.store.lock();
            store.upsert_workflow(&workflow).expect("upsert workflow");
            store
                .replace_workflow_steps(&workflow.workflow_id, &workflow.steps)
                .expect("replace workflow steps");
            workflow
        }

        fn seed_paused_schedule(&self, run: &runtime::Run) -> scheduler::Schedule {
            let fire_at = Utc::now() + chrono::Duration::minutes(10);
            let created = self
                .scheduler
                .create(scheduler::CreateInput {
                    trigger: scheduler::Trigger {
                        kind: scheduler::TriggerKind::Once,
                        fire_at: Some(fire_at),
                        ..scheduler::Trigger::default()
                    },
                    target: scheduler::Target {
                        kind: scheduler::TargetKind::Run,
                        run: Some(scheduler::RunTarget {
                            entrypoint: run.entrypoint.clone(),
                            goal: run.goal.clone(),
                            ..scheduler::RunTarget::default()
                        }),
                        ..scheduler::Target::default()
                    },
                    retry_policy: scheduler::RetryPolicy {
                        max_retries: 1,
                        backoff_kind: scheduler::RetryBackoffKind::Fixed,
                        base_delay_seconds: 5,
                        max_delay_seconds: 5,
                    },
                })
                .expect("create schedule");
            self.scheduler
                .pause(&created.schedule_id)
                .expect("pause schedule")
                .expect("paused schedule exists")
        }

        fn seed_failed_delivery(
            &self,
            run: &runtime::Run,
            workflow: &orchestration::Workflow,
        ) -> delivery::DeliveryOutcome {
            self.delivery.configure_for_testing(
                1,
                StdDuration::from_millis(1),
                StdDuration::from_millis(1),
            );
            let target = self
                .delivery
                .create_target(delivery::DeliveryTarget {
                    target_id: "target-operator".to_string(),
                    display_name: "Operator Target".to_string(),
                    target_kind: delivery::TargetKind::TestSink,
                    environment_scope: "test".to_string(),
                    ..delivery::DeliveryTarget::default()
                })
                .expect("create target");
            let mut by_class = HashMap::new();
            by_class.insert(delivery::ResultClass::Failure, target.target_id.clone());
            self.delivery
                .upsert_preference(delivery::DeliveryPreference {
                    preference_id: "pref-operator".to_string(),
                    environment_scope: "test".to_string(),
                    scope_kind: delivery::PreferenceScopeKind::UserDefault,
                    preferred_targets_by_class: by_class,
                    ..delivery::DeliveryPreference::default()
                })
                .expect("upsert preference");
            let outcome = self
                .delivery
                .emit_outcome(delivery::OutcomeInput {
                    source_kind: "workflow".to_string(),
                    source_id: workflow.workflow_id.clone(),
                    run_id: run.run_id.clone(),
                    workflow_id: workflow.workflow_id.clone(),
                    result_class: delivery::ResultClass::Failure,
                    payload_preview: "transport failure".to_string(),
                    ..delivery::OutcomeInput::default()
                })
                .expect("emit outcome");
            // The failed transition runs on the detached retry thread; poll until
            // the outcome reaches the failed terminal state (Go polls 2s).
            let deadline = std::time::Instant::now() + StdDuration::from_secs(2);
            loop {
                let (current, ok) = self
                    .delivery
                    .get_outcome(&outcome.delivery_id)
                    .expect("get outcome");
                if ok && current.status == delivery::OutcomeStatus::Failed {
                    return current;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "delivery {} did not reach failed status",
                    outcome.delivery_id,
                );
                std::thread::sleep(StdDuration::from_millis(20));
            }
        }
    }

    #[tokio::test]
    async fn operator_onboarding_projects_readiness_and_first_actions() {
        // Port of Go TestOperatorOnboardingRouteProjectsReadinessAndFirstActions.
        let h = operator_harness(vec![Arc::new(TestSinkAdapter::new()) as Arc<dyn DeliveryAdapter>]);
        h.state
            .integrations
            .as_ref()
            .expect("integrations")
            .create(integrations::CreateInput {
                integration_id: "calendar-a".to_string(),
                domain_kind: "calendar".to_string(),
                display_name: "Calendar A".to_string(),
                canonical_default: true,
                environment_scope: "test".to_string(),
                backend_binding: integrations::BackendBinding {
                    backend_kind: integrations::BackendKind::FakeLocal,
                    ..Default::default()
                },
                ..Default::default()
            })
            .expect("create integration");
        h.state
            .integrations
            .as_ref()
            .expect("integrations")
            .update_readiness(
                "calendar-a",
                integrations::UpdateReadinessInput {
                    readiness_status: integrations::ReadinessStatus::Degraded,
                    auth_state: "authorized".to_string(),
                    health_state: "degraded".to_string(),
                    reason: "reauth required".to_string(),
                    required_operator_action: "Refresh calendar auth.".to_string(),
                    ..Default::default()
                },
            )
            .expect("update readiness");
        h.state
            .connectors
            .as_ref()
            .expect("connectors")
            .register(connectors::RegisterInput {
                connector_id: "telegram-main".to_string(),
                kind: "telegram".to_string(),
                display_name: "Telegram Main".to_string(),
                ..Default::default()
            })
            .expect("register connector");
        h.state
            .connectors
            .as_ref()
            .expect("connectors")
            .report_failure(
                "telegram-main",
                connectors::ReportFailureInput {
                    reason: "network backoff".to_string(),
                },
            )
            .expect("report connector failure");
        h.state
            .capabilities
            .as_ref()
            .expect("capabilities")
            .register(kura_capabilities::RegisterInput {
                capability_id: "browser".to_string(),
                kind: "browser".to_string(),
                display_name: "Browser".to_string(),
            })
            .expect("register capability");
        h.state
            .capabilities
            .as_ref()
            .expect("capabilities")
            .report_health(
                "browser",
                kura_capabilities::ReportHealthInput {
                    status: kura_capabilities::Status::Degraded,
                },
            )
            .expect("report capability health");
        h.seed_run(OPERATOR_SHELL_TEST_ENTRYPOINT, "operator smoke", runtime::RunStatus::Queued);

        let app = crate::routes::router(h.state.clone());
        let (status, json) = send(
            &app,
            authenticated_request("/v1/operator/onboarding"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "onboarding body: {json}");
        assert_eq!(json["environmentScope"], "test");
        assert_eq!(json["status"], "completed");
        assert_eq!(json["recommendedActionId"], "test_run");
        assert_eq!(
            json["blockingItemIds"].as_array().map(|items| items.len()).unwrap_or(0),
            0,
        );

        let actions = json["firstUsefulActions"].as_array().expect("actions");
        let query_available = actions
            .iter()
            .find(|action| action["actionId"] == "test_query")
            .map(|action| action["available"].as_bool().unwrap_or(false))
            .unwrap_or(false);
        let activation_action = actions
            .iter()
            .find(|action| action["actionId"] == "test_chat")
            .expect("test_chat action");
        assert!(query_available, "expected ready echo query action");
        assert_eq!(activation_action["available"], true);
        assert_eq!(activation_action["invokeRoute"], "/v1/activation/test-chat");

        let follow_ups: Vec<String> = json["optionalFollowUpItemIds"]
            .as_array()
            .expect("follow ups")
            .iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect();
        assert!(follow_ups.contains(&"connector-telegram-main".to_string()));
        assert!(follow_ups.contains(&"capability-browser".to_string()));
        assert!(follow_ups.contains(&"integration-calendar-a".to_string()));
        let completed = json["completedStepIds"]
            .as_array()
            .expect("completed steps")
            .iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect::<Vec<String>>();
        assert!(completed.contains(&"test-run-recorded".to_string()));
    }

    #[tokio::test]
    async fn operator_activity_includes_approvals_schedules_runs_workflows_and_deliveries() {
        // Port of Go TestOperatorActivityRouteIncludesApprovalsSchedulesRunsWorkflowsAndDeliveries.
        let h = operator_harness(vec![Arc::new(FailingDeliveryAdapter {
            target_kind: TargetKind::TestSink,
            err: "transport offline".to_string(),
        }) as Arc<dyn DeliveryAdapter>]);

        let (approval, _decision) = h
            .policy
            .request_approval(policy::RequestApprovalInput {
                action: "workflow.launch".to_string(),
                resource_kind: "workflow".to_string(),
                resource_id: "wf_manual".to_string(),
                reason: "manual review required".to_string(),
                requested_by: "operator-test".to_string(),
                ..Default::default()
            })
            .expect("request approval");
        let run = h.seed_run(
            "operator",
            "follow up delivery",
            runtime::RunStatus::Blocked,
        );
        let workflow = h.seed_workflow(
            &run,
            orchestration::WorkflowStatus::Failed,
            "workflow failed after operator review",
        );
        let schedule_item = h.seed_paused_schedule(&run);
        let failed_outcome = h.seed_failed_delivery(&run, &workflow);
        h.store
            .lock()
            .append_event(&events::Event {
                event_id: "evt_operator_delivery_orphan".to_string(),
                environment_scope: "test".to_string(),
                category: "delivery".to_string(),
                name: "delivery.failed".to_string(),
                occurred_at: Utc::now() + chrono::Duration::minutes(2),
                resource: events::Resource {
                    kind: "delivery".to_string(),
                    id: "delivery_orphan".to_string(),
                },
                payload: serde_json::json!({ "status": "failed" })
                    .as_object()
                    .cloned()
                    .expect("payload object"),
                ..events::Event::default()
            })
            .expect("append orphan event");

        let app = crate::routes::router(h.state.clone());
        let (status, json) = send(
            &app,
            request(
                "GET",
                "/v1/operator/activity?attentionOnly=true&limit=10",
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "activity body: {json}");
        let items = json["items"].as_array().expect("items");
        let mut found_approval = false;
        let mut found_schedule = false;
        let mut found_run = false;
        let mut found_workflow = false;
        let mut found_delivery = false;
        let mut found_event_backed_delivery = false;
        for item in items {
            assert_ne!(
                item["attentionLevel"],
                "info",
                "expected attentionOnly filter to remove info items: {item}"
            );
            match item["sourceKind"].as_str() {
                Some("approval") => {
                    found_approval = item["sourceId"] == approval.approval_id;
                }
                Some("schedule") => {
                    found_schedule = item["sourceId"] == schedule_item.schedule_id;
                }
                Some("run") => {
                    found_run = item["sourceId"] == run.run_id
                        && item["detailRoute"] == format!("/v1/runs/{}", run.run_id);
                }
                Some("workflow") => {
                    found_workflow = item["sourceId"] == workflow.workflow_id
                        && item["detailRoute"]
                            == format!("/v1/runs/{}/workflows/{}", run.run_id, workflow.workflow_id);
                }
                Some("delivery") => {
                    if item["sourceId"] == failed_outcome.delivery_id
                        && item["detailRoute"]
                            == format!("/v1/deliveries/{}", failed_outcome.delivery_id)
                    {
                        found_delivery = true;
                    }
                    if item["sourceId"] == "delivery_orphan"
                        && item["detailRoute"] == "/v1/deliveries/delivery_orphan"
                    {
                        found_event_backed_delivery = true;
                    }
                }
                _ => {}
            }
        }
        assert!(found_approval, "expected approval record: {json}");
        assert!(found_schedule, "expected schedule record: {json}");
        assert!(found_run, "expected run record: {json}");
        assert!(found_workflow, "expected workflow record: {json}");
        assert!(found_delivery, "expected delivery record: {json}");
        assert!(
            found_event_backed_delivery,
            "expected persisted-event-backed delivery history: {json}"
        );
    }

    #[tokio::test]
    async fn operator_diagnostics_supports_plane_and_severity_filters() {
        // Port of Go TestOperatorDiagnosticsRouteSupportsPlaneAndSeverityFilters.
        let h = operator_harness(vec![Arc::new(FailingDeliveryAdapter {
            target_kind: TargetKind::TestSink,
            err: "transport offline".to_string(),
        }) as Arc<dyn DeliveryAdapter>]);

        h.state
            .integrations
            .as_ref()
            .expect("integrations")
            .create(integrations::CreateInput {
                integration_id: "mail-a".to_string(),
                domain_kind: "mail".to_string(),
                display_name: "Mail A".to_string(),
                canonical_default: true,
                environment_scope: "test".to_string(),
                backend_binding: integrations::BackendBinding {
                    backend_kind: integrations::BackendKind::FakeLocal,
                    ..Default::default()
                },
                ..Default::default()
            })
            .expect("create integration");
        h.state
            .integrations
            .as_ref()
            .expect("integrations")
            .update_readiness(
                "mail-a",
                integrations::UpdateReadinessInput {
                    readiness_status: integrations::ReadinessStatus::Unavailable,
                    auth_state: "expired".to_string(),
                    health_state: "unavailable".to_string(),
                    reason: "mail auth expired".to_string(),
                    required_operator_action: "Reconnect mail integration.".to_string(),
                    ..Default::default()
                },
            )
            .expect("update readiness");
        h.policy
            .request_approval(policy::RequestApprovalInput {
                action: "delivery.override".to_string(),
                resource_kind: "delivery".to_string(),
                resource_id: "delivery_manual".to_string(),
                reason: "operator approval required".to_string(),
                requested_by: "operator-test".to_string(),
                ..Default::default()
            })
            .expect("request approval");
        let run = h.seed_run(
            "operator",
            "diagnose failed delivery",
            runtime::RunStatus::Failed,
        );
        let workflow = h.seed_workflow(
            &run,
            orchestration::WorkflowStatus::Failed,
            "workflow failed after retries",
        );
        let failed_outcome = h.seed_failed_delivery(&run, &workflow);

        let app = crate::routes::router(h.state.clone());
        let (status, json) = send(
            &app,
            request("GET", "/v1/operator/diagnostics", None),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "diagnostics body: {json}");
        let all_findings = json["items"].as_array().expect("items");
        assert!(
            all_findings.len() >= 4,
            "expected multiple findings, got {json}"
        );

        let (status, json) = send(
            &app,
            request(
                "GET",
                "/v1/operator/diagnostics?plane=delivery&severity=critical",
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "filtered body: {json}");
        let items = json["items"].as_array().expect("items");
        assert_eq!(
            items.len(),
            1,
            "expected exactly one filtered finding, got {json}"
        );
        let item = &items[0];
        assert_eq!(item["sourceKind"], "delivery");
        assert_eq!(item["sourceId"], failed_outcome.delivery_id);
        assert_eq!(item["plane"], "delivery");
        assert_eq!(item["severity"], "critical");
        assert_eq!(
            item["detailRoute"],
            format!("/v1/deliveries/{}", failed_outcome.delivery_id),
        );
    }
}

