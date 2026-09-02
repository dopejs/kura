//! reminders route family (port of daemon/internal/api/reminders.go).
//!
//! Routes under `/v1/reminders/*`:
//! - `GET/POST /v1/reminders` — list all reminders / create one (201)
//! - `GET /v1/reminders/{id}` — fetch one reminder (404 when absent)
//! - `GET /v1/reminders/{id}/actions` — action history for one reminder
//! - `POST /v1/reminders/{id}/{acknowledge|snooze|complete|dismiss|cancel|reschedule}`
//!   — lifecycle transitions; the response carries `{reminder, occurrence, action}`
//! - `GET /v1/reminders/occurrences` — occurrence list filtered by query params
//! - `GET /v1/reminders/occurrences/{occurrence_id}` — one occurrence
//!
//! Wire shape follows the Go json tags (camelCase); status-code mapping mirrors
//! `handleReminders` / `handleReminderRoutes` / `handleReminderTransition`:
//! manager absent -> 500, create/build/transition errors -> 400 (404 only for
//! the reminder/occurrence-not-found sentinels), reads -> 500, missing rows -> 404.
//!
//! Middleware note: the Go registrations wrap these routes with `protected()` and
//! `withByIDTenantGuard(..., "reminders", "reminder_id", "reminder", ...)`.
//! `router()` takes no state (the outer `mod::router` owns it and applies
//! `.with_state` at the end), so axum's `from_fn_with_state(protected)` and the
//! state-carrying `ByIDTenantGuardLayer` cannot be constructed here. Handlers
//! therefore apply the by-id tenant guard inline (see `guard_reminder_for_tenant`),
//! reading the `TenantContext` extension that `protected()` installs once an auth
//! manager is wired; with no auth manager configured requests pass through
//! unauthenticated, exactly like Go's nil-auth behavior.

use std::collections::HashMap;

use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{Method, StatusCode, Uri};
use axum::routing::{get, post};
use axum::Router;
use chrono::{DateTime, Utc};
use kura_reminders::{
    ActionRecord, ActorKind, BehaviorMode, CreateInput, Occurrence, OccurrenceFilter, Reminder,
    ReminderError, State as ReminderState, TransitionInput, WorkflowLaunchConfig,
};
use kura_scheduler::{Trigger, TriggerKind};
use serde::Deserialize;
use serde_json::json;

use crate::error::ApiError;
use crate::middleware::{guard_resource_for_tenant, TenantContext};
use crate::response::Json;
use crate::state::AppState;
use crate::types::{
    CalendarWorkflowActionRequest, ListResponse, MailAttachmentRefRequest, MailWorkflowActionRequest,
    ReminderWorkflowLaunchRequest, ScheduleTriggerRequest,
};

// ---------------------------------------------------------------------------
// Request DTOs (local ports of the Go api-package types; the shared vocabulary
// in types.rs keeps serde_json::Value placeholders for the reminders-package
// enums, so the concrete reminders types are declared here).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateReminderRequest {
    title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    details: String,
    #[serde(default)]
    behavior_mode: BehaviorMode,
    trigger: ScheduleTriggerRequest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    workflow_launch_config: Option<ReminderWorkflowLaunchRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    follow_up_link: Option<ReminderFollowUpLinkRequest>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReminderFollowUpLinkRequest {
    link_kind: kura_reminders::FollowUpLinkKind,
    source_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    environment_scope: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    source_summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    source_display_state: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReminderTransitionRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    occurrence_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    reason: String,
    #[serde(default)]
    actor_kind: ActorKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    snoozed_until: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    trigger: Option<ScheduleTriggerRequest>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `reminders_manager` — the shared manager lookup (Go `if manager == nil`).
fn reminders_manager(state: &AppState) -> Result<&kura_reminders::Manager, ApiError> {
    state
        .reminders
        .as_deref()
        .ok_or_else(|| ApiError::Internal("reminders are not configured".to_string()))
}

/// GET /v1/reminders — all reminders (Go handleReminders GET).
#[allow(clippy::unused_async)]
pub async fn handle_reminders_list(
    State(state): State<AppState>,
) -> Result<Json<ListResponse<Reminder>>, ApiError> {
    let manager = reminders_manager(&state)?;
    let items = manager.list().map_err(ApiError::internal)?;
    Ok(Json(ListResponse { items }))
}

/// POST /v1/reminders — create a reminder (Go handleReminders POST; 201).
#[allow(clippy::unused_async)]
pub async fn handle_reminders_create(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<(StatusCode, axum::Json<Reminder>), ApiError> {
    let manager = reminders_manager(&state)?;
    let request: CreateReminderRequest = serde_json::from_slice(body.as_ref())
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let input = build_create_reminder_input(request)?;
    // Go maps every manager.Create failure to 400.
    let item = manager
        .create(&input)
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    Ok((StatusCode::CREATED, axum::Json(item)))
}

/// GET /v1/reminders/{id} — one reminder (Go handleReminderByID).
#[allow(clippy::unused_async)]
pub async fn handle_reminder_by_id(
    State(state): State<AppState>,
    Path(reminder_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
    method: Method,
    uri: Uri,
) -> Result<Json<Reminder>, ApiError> {
    guard_reminder_for_tenant(&state, &method, &uri, tenant.as_ref().map(|e| &e.0), &reminder_id)
        .await?;
    let manager = reminders_manager(&state)?;
    let (item, ok) = manager.get(&reminder_id).map_err(ApiError::internal)?;
    if !ok {
        return Err(ApiError::NotFound("not found".to_string()));
    }
    Ok(Json(item))
}

/// GET /v1/reminders/{id}/actions — action history (Go handleReminderActions).
#[allow(clippy::unused_async)]
pub async fn handle_reminder_actions(
    State(state): State<AppState>,
    Path(reminder_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
    method: Method,
    uri: Uri,
) -> Result<Json<ListResponse<ActionRecord>>, ApiError> {
    guard_reminder_for_tenant(&state, &method, &uri, tenant.as_ref().map(|e| &e.0), &reminder_id)
        .await?;
    let manager = reminders_manager(&state)?;
    let items = manager.list_actions(&reminder_id).map_err(ApiError::internal)?;
    Ok(Json(ListResponse { items }))
}

/// GET /v1/reminders/occurrences — filtered occurrence list (Go
/// handleReminderOccurrences).
#[allow(clippy::unused_async)]
pub async fn handle_reminder_occurrences(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    tenant: Option<Extension<TenantContext>>,
    method: Method,
    uri: Uri,
) -> Result<Json<ListResponse<Occurrence>>, ApiError> {
    // Go's withByIDTenantGuard extracts "occurrences" as the id segment here;
    // the lookup misses (no reminder row keyed "occurrences") and passes.
    guard_reminder_for_tenant(&state, &method, &uri, tenant.as_ref().map(|e| &e.0), "occurrences")
        .await?;
    let manager = reminders_manager(&state)?;
    let filter = reminder_occurrence_filter_from_request(&params)?;
    let items = manager.list_occurrences(&filter).map_err(ApiError::internal)?;
    Ok(Json(ListResponse { items }))
}

/// GET /v1/reminders/occurrences/{occurrence_id} — one occurrence (Go
/// handleReminderOccurrenceByID).
#[allow(clippy::unused_async)]
pub async fn handle_reminder_occurrence_by_id(
    State(state): State<AppState>,
    Path(occurrence_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
    method: Method,
    uri: Uri,
) -> Result<Json<Occurrence>, ApiError> {
    guard_reminder_for_tenant(&state, &method, &uri, tenant.as_ref().map(|e| &e.0), "occurrences")
        .await?;
    let manager = reminders_manager(&state)?;
    let (item, ok) = manager.get_occurrence(&occurrence_id).map_err(ApiError::internal)?;
    if !ok {
        return Err(ApiError::NotFound("not found".to_string()));
    }
    Ok(Json(item))
}

/// The lifecycle transition being applied (Go's per-route closures).
#[derive(Debug, Clone, Copy)]
enum ReminderTransitionKind {
    Acknowledge,
    Snooze,
    Complete,
    Dismiss,
    Cancel,
    Reschedule,
}

macro_rules! reminder_transition_handler {
    ($name:ident, $kind:expr) => {
        /// POST /v1/reminders/{id}/<transition> (Go handleReminderTransition).
        #[allow(clippy::unused_async)]
        pub async fn $name(
            State(state): State<AppState>,
            Path(reminder_id): Path<String>,
            tenant: Option<Extension<TenantContext>>,
            method: Method,
            uri: Uri,
            body: Bytes,
        ) -> Result<Json<serde_json::Value>, ApiError> {
            handle_reminder_transition(state, reminder_id, tenant, method, uri, body, $kind).await
        }
    };
}

reminder_transition_handler!(handle_reminder_acknowledge, ReminderTransitionKind::Acknowledge);
reminder_transition_handler!(handle_reminder_snooze, ReminderTransitionKind::Snooze);
reminder_transition_handler!(handle_reminder_complete, ReminderTransitionKind::Complete);
reminder_transition_handler!(handle_reminder_dismiss, ReminderTransitionKind::Dismiss);
reminder_transition_handler!(handle_reminder_cancel, ReminderTransitionKind::Cancel);
reminder_transition_handler!(handle_reminder_reschedule, ReminderTransitionKind::Reschedule);

async fn handle_reminder_transition(
    state: AppState,
    reminder_id: String,
    tenant: Option<Extension<TenantContext>>,
    method: Method,
    uri: Uri,
    body: Bytes,
    kind: ReminderTransitionKind,
) -> Result<Json<serde_json::Value>, ApiError> {
    guard_reminder_for_tenant(&state, &method, &uri, tenant.as_ref().map(|e| &e.0), &reminder_id)
        .await?;
    let manager = reminders_manager(&state)?;
    let request: ReminderTransitionRequest = serde_json::from_slice(body.as_ref())
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let input = build_reminder_transition_input(request)?;
    let (reminder, occurrence, action) = match kind {
        ReminderTransitionKind::Acknowledge => manager.acknowledge(&reminder_id, &input),
        ReminderTransitionKind::Snooze => manager.snooze(&reminder_id, &input),
        ReminderTransitionKind::Complete => manager.complete(&reminder_id, &input),
        ReminderTransitionKind::Dismiss => manager.dismiss(&reminder_id, &input),
        ReminderTransitionKind::Cancel => manager.cancel(&reminder_id, &input),
        ReminderTransitionKind::Reschedule => manager.reschedule(&reminder_id, &input),
    }
    .map_err(transition_error)?;
    Ok(Json(json!({
        "reminder": reminder,
        "occurrence": occurrence,
        "action": action,
    })))
}

// ---------------------------------------------------------------------------
// Tenant guard (Go withByIDTenantGuard for the reminders table)
// ---------------------------------------------------------------------------

/// `withByIDTenantGuard` equivalent for the reminders family: verifies the
/// reminder id against the caller's resolved tenant before the handler runs.
/// The canonical surface label is built from the request method + path exactly
/// like Go's `surfaceFromRequest` (api:<METHOD route>).
async fn guard_reminder_for_tenant(
    state: &AppState,
    method: &Method,
    uri: &Uri,
    tenant: Option<&TenantContext>,
    reminder_id: &str,
) -> Result<(), ApiError> {
    let surface = format!("api:{} {}", method.as_str(), uri.path());
    guard_resource_for_tenant(
        state,
        tenant,
        &surface,
        "reminders",
        "reminder_id",
        reminder_id,
        "reminder",
    )
    .await
}

// ---------------------------------------------------------------------------
// Request -> input builders (ports of buildCreateReminderInput /
// buildReminderTransitionInput and the shared schedule/calendar/mail helpers)
// ---------------------------------------------------------------------------

fn build_create_reminder_input(request: CreateReminderRequest) -> Result<CreateInput, ApiError> {
    let trigger = schedule_trigger_from_request(request.trigger)?;
    let workflow_launch_config = match request.workflow_launch_config {
        None => None,
        Some(cfg) => {
            let calendar_action = build_calendar_action(cfg.calendar_action)?;
            let mail_action = build_mail_action(cfg.mail_action)?;
            Some(WorkflowLaunchConfig {
                session_id: cfg.session_id,
                entrypoint: cfg.entrypoint,
                run_goal: cfg.run_goal,
                workflow_goal: cfg.workflow_goal,
                calendar_action,
                mail_action,
            })
        }
    };
    let follow_up_link = request.follow_up_link.map(|link| kura_reminders::FollowUpLink {
        link_kind: link.link_kind,
        source_id: link.source_id.trim().to_string(),
        environment_scope: link.environment_scope.trim().to_string(),
        source_summary: link.source_summary.trim().to_string(),
        source_display_state: link.source_display_state.trim().to_string(),
        ..kura_reminders::FollowUpLink::default()
    });
    Ok(CreateInput {
        title: request.title.trim().to_string(),
        details: request.details.trim().to_string(),
        behavior_mode: request.behavior_mode,
        trigger,
        workflow_launch_config,
        follow_up_link,
    })
}

fn build_reminder_transition_input(
    request: ReminderTransitionRequest,
) -> Result<TransitionInput, ApiError> {
    let mut input = TransitionInput {
        occurrence_id: request.occurrence_id.trim().to_string(),
        reason: request.reason.trim().to_string(),
        actor_kind: request.actor_kind,
        ..TransitionInput::default()
    };
    if !request.snoozed_until.trim().is_empty() {
        input.snoozed_until = Some(parse_rfc3339(request.snoozed_until.trim())?);
    }
    if let Some(trigger) = request.trigger {
        input.trigger = Some(schedule_trigger_from_request(trigger)?);
    }
    Ok(input)
}

/// Go `scheduleTriggerFromRequest` (daemon/internal/api/schedules.go).
fn schedule_trigger_from_request(input: ScheduleTriggerRequest) -> Result<Trigger, ApiError> {
    let mut trigger = Trigger {
        kind: input.kind,
        cron_expr: input.cron_expr.trim().to_string(),
        timezone: input.timezone.trim().to_string(),
        ..Trigger::default()
    };
    match input.kind {
        TriggerKind::Once => {
            trigger.fire_at = Some(parse_rfc3339(input.fire_at.trim())?);
        }
        TriggerKind::Cron => {
            if trigger.timezone.is_empty() {
                return Err(ApiError::BadRequest(
                    "cron schedule requires timezone".to_string(),
                ));
            }
        }
    }
    Ok(trigger)
}

/// Go `buildCalendarAction` (daemon/internal/api/calendar_execution.go).
fn build_calendar_action(
    request: Option<CalendarWorkflowActionRequest>,
) -> Result<Option<kura_calendar::Action>, ApiError> {
    let Some(request) = request else {
        return Ok(None);
    };
    let mut action = kura_calendar::Action {
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
            .into_iter()
            .map(|a| a.trim().to_string())
            .filter(|a| !a.is_empty())
            .collect(),
        reason: request.reason.trim().to_string(),
        ..kura_calendar::Action::default()
    };
    action.window_start = parse_optional_calendar_action_time(&request.window_start)
        .map_err(|e| ApiError::BadRequest(format!("parse windowStart: {e}")))?;
    action.window_end = parse_optional_calendar_action_time(&request.window_end)
        .map_err(|e| ApiError::BadRequest(format!("parse windowEnd: {e}")))?;
    action.starts_at = parse_optional_calendar_action_time(&request.starts_at)
        .map_err(|e| ApiError::BadRequest(format!("parse startsAt: {e}")))?;
    action.ends_at = parse_optional_calendar_action_time(&request.ends_at)
        .map_err(|e| ApiError::BadRequest(format!("parse endsAt: {e}")))?;
    if action.operation_class.as_str().trim().is_empty() {
        return Err(ApiError::BadRequest(
            "calendarAction.operationClass is required".to_string(),
        ));
    }
    Ok(Some(action))
}

/// Go `buildMailAction` (daemon/internal/api/mail_execution.go).
fn build_mail_action(
    request: Option<MailWorkflowActionRequest>,
) -> Result<Option<kura_mail::Action>, ApiError> {
    let Some(request) = request else {
        return Ok(None);
    };
    let action = kura_mail::Action {
        operation_class: request.operation_class,
        integration_id: request.integration_id.trim().to_string(),
        thread_id: request.thread_id.trim().to_string(),
        message_id: request.message_id.trim().to_string(),
        draft_id: request.draft_id.trim().to_string(),
        compose_mode: request.compose_mode.as_str().to_string(),
        result_mode: request.result_mode.as_str().to_string(),
        to: request.to,
        cc: request.cc,
        bcc: request.bcc,
        subject: request.subject.trim().to_string(),
        body: request.body,
        attachment_refs: mail_attachment_inputs(request.attachment_refs),
        allow_send_side_effects: request.allow_send_side_effects,
    };
    if action.operation_class.as_str().trim().is_empty() {
        return Err(ApiError::BadRequest(
            "mailAction.operationClass is required".to_string(),
        ));
    }
    Ok(Some(action))
}

/// Go `mailAttachmentInputs` (daemon/internal/api/mail_execution.go).
fn mail_attachment_inputs(items: Vec<MailAttachmentRefRequest>) -> Vec<kura_mail::AttachmentRefInput> {
    if items.is_empty() {
        return Vec::new();
    }
    items
        .into_iter()
        .map(|item| kura_mail::AttachmentRefInput {
            attachment_ref_id: item.attachment_ref_id.trim().to_string(),
            display_name: item.display_name.trim().to_string(),
            media_type: item.media_type.trim().to_string(),
            size_bytes: (item.size_bytes != 0).then_some(item.size_bytes),
        })
        .collect()
}

/// Go `reminderOccurrenceFilterFromRequest`.
fn reminder_occurrence_filter_from_request(
    query: &HashMap<String, String>,
) -> Result<OccurrenceFilter, ApiError> {
    let get = |key: &str| {
        query
            .get(key)
            .map(|value| value.trim().to_string())
            .unwrap_or_default()
    };
    let mut filter = OccurrenceFilter {
        reminder_id: get("reminderId"),
        run_id: get("runId"),
        workflow_id: get("workflowId"),
        delivery_id: get("deliveryId"),
        ..OccurrenceFilter::default()
    };
    let state = get("state");
    if !state.is_empty() {
        filter.state = Some(parse_state(&state)?);
    }
    if let Some(value) = query
        .get("scheduledBefore")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        filter.scheduled_before = Some(parse_rfc3339(value)?);
    }
    if let Some(value) = query
        .get("scheduledAfter")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        filter.scheduled_after = Some(parse_rfc3339(value)?);
    }
    Ok(filter)
}

/// Parses an RFC3339 timestamp (Go `time.Parse(time.RFC3339, ...)`, UTC).
fn parse_rfc3339(value: &str) -> Result<DateTime<Utc>, ApiError> {
    let parsed =
        DateTime::parse_from_rfc3339(value).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    Ok(parsed.with_timezone(&Utc))
}

/// Parses the `state` occurrence filter (Go's string cast to reminders.State).
fn parse_state(value: &str) -> Result<ReminderState, ApiError> {
    serde_json::from_value(serde_json::Value::String(value.to_string()))
        .map_err(|e| ApiError::BadRequest(e.to_string()))
}

/// Go `parseOptionalCalendarActionTime`.
fn parse_optional_calendar_action_time(
    raw: &str,
) -> Result<Option<DateTime<Utc>>, chrono::ParseError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let parsed = DateTime::parse_from_rfc3339(trimmed)?;
    Ok(Some(parsed.with_timezone(&Utc)))
}

/// Error mapping for transition handlers (Go handleReminderTransition's switch).
fn transition_error(err: ReminderError) -> ApiError {
    match err {
        ReminderError::ReminderNotFound | ReminderError::OccurrenceNotFound => {
            ApiError::NotFound("not found".to_string())
        }
        other => ApiError::BadRequest(other.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Route family router.
#[must_use = "route family routers must be merged into the API router"]
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/v1/reminders",
            get(handle_reminders_list).post(handle_reminders_create),
        )
        .route("/v1/reminders/{id}", get(handle_reminder_by_id))
        .route("/v1/reminders/{id}/actions", get(handle_reminder_actions))
        .route(
            "/v1/reminders/{id}/acknowledge",
            post(handle_reminder_acknowledge),
        )
        .route("/v1/reminders/{id}/snooze", post(handle_reminder_snooze))
        .route("/v1/reminders/{id}/complete", post(handle_reminder_complete))
        .route("/v1/reminders/{id}/dismiss", post(handle_reminder_dismiss))
        .route("/v1/reminders/{id}/cancel", post(handle_reminder_cancel))
        .route(
            "/v1/reminders/{id}/reschedule",
            post(handle_reminder_reschedule),
        )
        .route(
            "/v1/reminders/occurrences",
            get(handle_reminder_occurrences),
        )
        .route(
            "/v1/reminders/occurrences/{occurrence_id}",
            get(handle_reminder_occurrence_by_id),
        )
}


#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;
    use std::time::Duration;

    use axum::body::to_bytes;
    use axum::http::Request as HttpRequest;
    use kura_delivery::{
        DeliveryAdapter, DeliveryPreference, DeliveryTarget, Manager as DeliveryManager,
        PreferenceScopeKind, ResultClass, TargetKind, TestSinkAdapter,
    };
    use kura_events::Bus;
    use kura_identity::{LifecycleStatus, Tenant, TenantKind};
    use kura_reminders::{Clock, Dependencies};
    use kura_store::SQLiteStore;
    use parking_lot::Mutex;
    use tower::ServiceExt;
    use uuid::Uuid;

    /// Go reminderTestClock.
    struct TestClock {
        now: Mutex<DateTime<Utc>>,
    }

    impl TestClock {
        fn new(now: DateTime<Utc>) -> Self {
            TestClock { now: Mutex::new(now) }
        }

        fn set(&self, now: DateTime<Utc>) {
            *self.now.lock() = now.with_timezone(&Utc);
        }
    }

    impl Clock for TestClock {
        fn now(&self) -> DateTime<Utc> {
            *self.now.lock()
        }
    }

    /// Go reminderWorkflowLauncherStub.
    struct FakeWorkflowLauncher {
        result: kura_reminders::WorkflowLaunchResult,
        err: Option<String>,
    }

    impl FakeWorkflowLauncher {
        fn ok(result: kura_reminders::WorkflowLaunchResult) -> Self {
            FakeWorkflowLauncher { result, err: None }
        }
    }

    impl kura_reminders::WorkflowLauncher for FakeWorkflowLauncher {
        fn launch_reminder_workflow(
            &self,
            _cfg: &kura_reminders::WorkflowLaunchConfig,
            _reminder_id: &str,
            _occurrence_id: &str,
        ) -> Result<kura_reminders::WorkflowLaunchResult, String> {
            match &self.err {
                Some(err) => Err(err.clone()),
                None => Ok(self.result.clone()),
            }
        }
    }

    struct Harness {
        state: AppState,
        manager: kura_reminders::Manager,
        clock: Arc<TestClock>,
    }

    fn dt(value: &str) -> DateTime<Utc> {
        value.parse().expect("valid rfc3339 timestamp")
    }

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

    /// Go bootstrapTestPersonalTenant: primes the default-tenant cache so the
    /// delivery target/preference seeding binds tenant_id correctly.
    fn bootstrap_personal_tenant(store: &Arc<Mutex<SQLiteStore>>) {
        let now = dt("2026-04-01T00:00:00Z");
        let tenant = Tenant {
            tenant_id: "ten_test_personal".to_string(),
            tenant_kind: TenantKind::Personal,
            display_name: "Test Personal".to_string(),
            status: LifecycleStatus::Active,
            created_at: now,
            updated_at: now,
            created_by_principal_id: String::new(),
            default_owner_principal_id: String::new(),
            caller_membership_role: None,
            caller_membership_status: None,
            caller_permissions: Vec::new(),
            default_for_current_token: false,
            default_for_current_principal: false,
        };
        store.lock().upsert_tenant(&tenant).expect("upsert tenant");
        store
            .lock()
            .seed_default_tenant_cache()
            .expect("seed default tenant cache");
    }

    /// Port of Go newReminderServerHarness: temp store, delivery test-sink
    /// target/preference, reminders manager with a fake clock, and an AppState
    /// wired with both managers.
    fn harness(workflow_launcher: Option<Arc<dyn kura_reminders::WorkflowLauncher>>) -> Harness {
        let dir = std::env::temp_dir().join(format!("kura-api-reminders-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let store = Arc::new(Mutex::new(
            SQLiteStore::new(dir.to_str().expect("path")).expect("store"),
        ));
        bootstrap_personal_tenant(&store);
        let bus = Arc::new(Bus::new());
        let delivery = DeliveryManager::new(
            "test",
            (*bus).clone(),
            Arc::clone(&store),
            vec![Arc::new(TestSinkAdapter::new()) as Arc<dyn DeliveryAdapter>],
        );
        let target = delivery
            .create_target(DeliveryTarget {
                target_id: "reminder-api-target".to_string(),
                display_name: "Reminder API Target".to_string(),
                target_kind: TargetKind::TestSink,
                environment_scope: "test".to_string(),
                ..DeliveryTarget::default()
            })
            .expect("create target");
        let mut by_class = HashMap::new();
        by_class.insert(ResultClass::RoutineSuccess, target.target_id.clone());
        by_class.insert(ResultClass::Urgent, target.target_id.clone());
        by_class.insert(ResultClass::Failure, target.target_id.clone());
        delivery
            .upsert_preference(DeliveryPreference {
                preference_id: "reminder-api-pref".to_string(),
                environment_scope: "test".to_string(),
                scope_kind: PreferenceScopeKind::UserDefault,
                preferred_targets_by_class: by_class,
                ..DeliveryPreference::default()
            })
            .expect("upsert preference");
        let clock = Arc::new(TestClock::new(dt("2026-04-23T09:00:00Z")));
        let manager = kura_reminders::Manager::new(Dependencies {
            environment_scope: "test".to_string(),
            store: Arc::clone(&store),
            event_bus: Some((*bus).clone()),
            delivery: Some(delivery.clone()),
            workflow_launcher,
            clock: Some(clock.clone() as Arc<dyn Clock>),
            tick_interval: Duration::from_millis(10),
        });
        let mut state = AppState::new(test_config(), bus, store);
        state.reminders = Some(Arc::new(manager.clone()));
        state.delivery = Some(Arc::new(delivery));
        Harness {
            state,
            manager,
            clock,
        }
    }

    fn request(method: &str, uri: &str, body: Option<&str>) -> HttpRequest<axum::body::Body> {
        HttpRequest::builder()
            .method(method)
            .uri(uri)
            .body(match body {
                Some(body) => axum::body::Body::from(body.to_string()),
                None => axum::body::Body::empty(),
            })
            .expect("request")
    }

    async fn send(
        app: &axum::Router,
        req: HttpRequest<axum::body::Body>,
    ) -> (StatusCode, serde_json::Value) {
        let response = app.clone().oneshot(req).await.expect("oneshot");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    #[tokio::test]
    async fn create_list_inspect_occurrences_and_actions() {
        // Port of Go TestReminderRoutesCreateInspectOccurrencesAndActions.
        let h = harness(None);
        let due_at = dt("2026-04-23T10:05:00Z");
        h.clock.set(due_at - chrono::Duration::minutes(1));

        let (status, json) = send(
            &crate::routes::router(h.state.clone()),
            request(
                "POST",
                "/v1/reminders",
                Some(
                    r#"{"title":"Check nightly backup","details":"Inspect last run","trigger":{"kind":"once","fireAt":"2026-04-23T10:05:00Z"}}"#,
                ),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "create body: {json}");
        let created: Reminder = serde_json::from_value(json).expect("created reminder");
        assert_eq!(created.behavior_mode, BehaviorMode::NotifyOnly);
        assert_eq!(created.current_state, ReminderState::Pending);

        let (status, json) = send(
            &crate::routes::router(h.state.clone()),
            request("GET", "/v1/reminders", None),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "list body: {json}");
        let list: ListResponse<Reminder> = serde_json::from_value(json).expect("list response");
        assert_eq!(list.items.len(), 1);
        assert_eq!(list.items[0].reminder_id, created.reminder_id);

        h.clock.set(due_at);
        h.manager.tick().expect("tick");

        let (status, json) = send(
            &crate::routes::router(h.state.clone()),
            request(
                "GET",
                &format!("/v1/reminders/{}", created.reminder_id),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "detail body: {json}");
        let detail: Reminder = serde_json::from_value(json).expect("detail reminder");
        assert_eq!(detail.current_state, ReminderState::Due);
        assert!(!detail.active_occurrence_id.is_empty());

        let (status, json) = send(
            &crate::routes::router(h.state.clone()),
            request(
                "GET",
                &format!("/v1/reminders/occurrences?reminderId={}", created.reminder_id),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "occurrence list body: {json}");
        let occ_list: ListResponse<Occurrence> =
            serde_json::from_value(json).expect("occurrence list");
        assert_eq!(occ_list.items.len(), 1);
        assert!(!occ_list.items[0].latest_delivery_id.is_empty());
        assert_eq!(occ_list.items[0].latest_delivery_status, "delivered");

        let (status, json) = send(
            &crate::routes::router(h.state.clone()),
            request(
                "GET",
                &format!("/v1/reminders/{}/actions", created.reminder_id),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "actions body: {json}");
        let actions: ListResponse<ActionRecord> =
            serde_json::from_value(json).expect("actions response");
        assert_eq!(
            actions.items.len(),
            3,
            "expected created, due, and delivery-linked actions: {actions:?}"
        );
    }

    #[tokio::test]
    async fn lifecycle_routes_and_workflow_linkage() {
        // Port of Go TestReminderLifecycleRoutesAndWorkflowLinkage.
        let launcher: Arc<dyn kura_reminders::WorkflowLauncher> = Arc::new(
            FakeWorkflowLauncher::ok(kura_reminders::WorkflowLaunchResult {
                run_id: "run_reminder_api".to_string(),
                workflow_id: "wf_reminder_api".to_string(),
            }),
        );
        let h = harness(Some(launcher));
        let base = dt("2026-04-23T12:00:00Z");
        let app = || crate::routes::router(h.state.clone());

        // acknowledge
        h.clock.set(base - chrono::Duration::minutes(1));
        let (status, json) = send(
            &app(),
            request(
                "POST",
                "/v1/reminders",
                Some(
                    r#"{"title":"Lifecycle reminder","trigger":{"kind":"once","fireAt":"2026-04-23T12:00:00Z"}}"#,
                ),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "create body: {json}");
        let lifecycle: Reminder = serde_json::from_value(json).expect("lifecycle reminder");
        h.clock.set(base);
        h.manager.tick().expect("tick lifecycle due");
        let (lifecycle_current, ok) = h.manager.get(&lifecycle.reminder_id).expect("get lifecycle");
        assert!(ok);

        let (status, json) = send(
            &app(),
            request(
                "POST",
                &format!("/v1/reminders/{}/acknowledge", lifecycle.reminder_id),
                Some(&format!(
                    r#"{{"occurrenceId":"{}","reason":"saw it"}}"#,
                    lifecycle_current.active_occurrence_id
                )),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "ack body: {json}");
        assert_eq!(json["occurrence"]["state"], "acknowledged");

        // reschedule
        let (status, json) = send(
            &app(),
            request(
                "POST",
                &format!("/v1/reminders/{}/reschedule", lifecycle.reminder_id),
                Some(
                    r#"{"trigger":{"kind":"once","fireAt":"2026-04-23T12:30:00Z"},"reason":"later"}"#,
                ),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "reschedule body: {json}");
        assert_eq!(json["reminder"]["currentState"], "pending");

        // snooze
        h.clock.set(base - chrono::Duration::minutes(1));
        let (status, json) = send(
            &app(),
            request(
                "POST",
                "/v1/reminders",
                Some(
                    r#"{"title":"Snooze reminder","trigger":{"kind":"once","fireAt":"2026-04-23T12:01:00Z"}}"#,
                ),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "snooze create body: {json}");
        let snooze_reminder: Reminder = serde_json::from_value(json).expect("snooze reminder");
        h.clock.set(base + chrono::Duration::minutes(1));
        h.manager.tick().expect("tick snooze due");
        let (snooze_current, ok) = h.manager.get(&snooze_reminder.reminder_id).expect("get snooze");
        assert!(ok);
        let (status, json) = send(
            &app(),
            request(
                "POST",
                &format!("/v1/reminders/{}/snooze", snooze_reminder.reminder_id),
                Some(&format!(
                    r#"{{"occurrenceId":"{}","snoozedUntil":"2026-04-23T12:05:00Z"}}"#,
                    snooze_current.active_occurrence_id
                )),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "snooze body: {json}");
        assert_eq!(json["occurrence"]["state"], "snoozed");

        // complete
        let (status, json) = send(
            &app(),
            request(
                "POST",
                &format!("/v1/reminders/{}/complete", snooze_reminder.reminder_id),
                Some(&format!(
                    r#"{{"occurrenceId":"{}","reason":"done"}}"#,
                    snooze_current.active_occurrence_id
                )),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "complete body: {json}");
        assert_eq!(json["occurrence"]["state"], "completed");

        // dismiss
        h.clock.set(base - chrono::Duration::minutes(1));
        let (status, json) = send(
            &app(),
            request(
                "POST",
                "/v1/reminders",
                Some(
                    r#"{"title":"Dismiss reminder","trigger":{"kind":"once","fireAt":"2026-04-23T12:02:00Z"}}"#,
                ),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "dismiss create body: {json}");
        let dismiss_reminder: Reminder = serde_json::from_value(json).expect("dismiss reminder");
        h.clock.set(base + chrono::Duration::minutes(2));
        h.manager.tick().expect("tick dismiss due");
        let (dismiss_current, ok) = h.manager.get(&dismiss_reminder.reminder_id).expect("get dismiss");
        assert!(ok);
        let (status, json) = send(
            &app(),
            request(
                "POST",
                &format!("/v1/reminders/{}/dismiss", dismiss_reminder.reminder_id),
                Some(&format!(
                    r#"{{"occurrenceId":"{}"}}"#,
                    dismiss_current.active_occurrence_id
                )),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "dismiss body: {json}");
        assert_eq!(json["occurrence"]["state"], "dismissed");

        // cancel
        h.clock.set(base - chrono::Duration::minutes(1));
        let (status, json) = send(
            &app(),
            request(
                "POST",
                "/v1/reminders",
                Some(
                    r#"{"title":"Cancel reminder","trigger":{"kind":"once","fireAt":"2026-04-23T12:10:00Z"}}"#,
                ),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "cancel create body: {json}");
        let cancel_reminder: Reminder = serde_json::from_value(json).expect("cancel reminder");
        let (status, json) = send(
            &app(),
            request(
                "POST",
                &format!("/v1/reminders/{}/cancel", cancel_reminder.reminder_id),
                Some(r#"{"reason":"not needed"}"#),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "cancel body: {json}");
        assert_eq!(json["reminder"]["currentState"], "cancelled");

        // workflow-launch linkage
        h.clock.set(base - chrono::Duration::minutes(1));
        let workflow_body = r#"{"title":"Workflow reminder","behaviorMode":"launch_workflow","trigger":{"kind":"once","fireAt":"2026-04-23T12:03:00Z"},"workflowLaunchConfig":{"entrypoint":"operator","workflowGoal":"follow up"}}"#;
        let (status, json) = send(&app(), request("POST", "/v1/reminders", Some(workflow_body)))
            .await;
        assert_eq!(status, StatusCode::CREATED, "workflow create body: {json}");
        let workflow_reminder: Reminder = serde_json::from_value(json).expect("workflow reminder");
        h.clock.set(base + chrono::Duration::minutes(3));
        h.manager.tick().expect("tick workflow due");

        let (status, json) = send(
            &app(),
            request(
                "GET",
                &format!("/v1/reminders/{}", workflow_reminder.reminder_id),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "workflow detail body: {json}");
        let detail: Reminder = serde_json::from_value(json).expect("workflow detail");
        assert_eq!(detail.current_state, ReminderState::Acknowledged);

        let (status, json) = send(
            &app(),
            request(
                "GET",
                &format!("/v1/reminders/occurrences/{}", detail.active_occurrence_id),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "workflow occurrence body: {json}");
        let occurrence: Occurrence = serde_json::from_value(json).expect("workflow occurrence");
        assert_eq!(occurrence.run_id, "run_reminder_api");
        assert_eq!(occurrence.workflow_id, "wf_reminder_api");
        assert_eq!(occurrence.state, ReminderState::Acknowledged);
    }

    #[tokio::test]
    async fn validation_and_not_found() {
        let h = harness(None);
        let app = || crate::routes::router(h.state.clone());

        // once triggers require a fireAt
        let (status, json) = send(
            &app(),
            request(
                "POST",
                "/v1/reminders",
                Some(r#"{"title":"No fire","trigger":{"kind":"once"}}"#),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "body: {json}");
        assert_eq!(json["code"], "bad_request");

        // title is required
        let (status, json) = send(
            &app(),
            request(
                "POST",
                "/v1/reminders",
                Some(r#"{"trigger":{"kind":"once","fireAt":"2026-04-23T12:00:00Z"}}"#),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "body: {json}");

        // launch_workflow requires a workflowLaunchConfig
        let (status, json) = send(
            &app(),
            request(
                "POST",
                "/v1/reminders",
                Some(
                    r#"{"title":"wf","behaviorMode":"launch_workflow","trigger":{"kind":"once","fireAt":"2026-04-23T12:00:00Z"}}"#,
                ),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "body: {json}");

        // unknown reminder id -> 404
        let (status, json) = send(&app(), request("GET", "/v1/reminders/rem_nope", None)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "body: {json}");
        assert_eq!(json["code"], "not_found");

        // transition on an unknown reminder -> 404
        let (status, json) = send(
            &app(),
            request(
                "POST",
                "/v1/reminders/rem_nope/acknowledge",
                Some(r#"{"occurrenceId":"occ_x"}"#),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "body: {json}");

        // unknown occurrence -> 404
        let (status, json) = send(
            &app(),
            request("GET", "/v1/reminders/occurrences/rem_occ_nope", None),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "body: {json}");

        // invalid occurrence filter timestamps -> 400
        let (status, json) = send(
            &app(),
            request(
                "GET",
                "/v1/reminders/occurrences?scheduledBefore=not-a-time",
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "body: {json}");

        // snooze requires snoozedUntil once an occurrence is due
        let due_at = dt("2026-04-23T10:05:00Z");
        h.clock.set(due_at - chrono::Duration::minutes(1));
        let (status, json) = send(
            &app(),
            request(
                "POST",
                "/v1/reminders",
                Some(
                    r#"{"title":"Snooze me","trigger":{"kind":"once","fireAt":"2026-04-23T10:05:00Z"}}"#,
                ),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "body: {json}");
        let created: Reminder = serde_json::from_value(json).expect("reminder");
        h.clock.set(due_at);
        h.manager.tick().expect("tick");
        let (current, ok) = h.manager.get(&created.reminder_id).expect("get");
        assert!(ok);
        let (status, json) = send(
            &app(),
            request(
                "POST",
                &format!("/v1/reminders/{}/snooze", created.reminder_id),
                Some(&format!(
                    r#"{{"occurrenceId":"{}"}}"#,
                    current.active_occurrence_id
                )),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "body: {json}");
    }

    #[tokio::test]
    async fn tenant_scope_guards_by_id() {
        // withByIDTenantGuard parity: a row owned by another tenant surfaces as
        // 404 before the handler runs; the owning tenant sees it normally.
        let h = harness(None);
        let (status, json) = send(
            &crate::routes::router(h.state.clone()),
            request(
                "POST",
                "/v1/reminders",
                Some(
                    r#"{"title":"Tenant reminder","trigger":{"kind":"once","fireAt":"2026-04-23T10:05:00Z"}}"#,
                ),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "body: {json}");
        let reminder: Reminder = serde_json::from_value(json).expect("reminder");

        // Bind the row to an owning tenant via the store's row-tenancy layer
        // (upsert_reminder intentionally leaves tenant_id to bind_row_tenant).
        {
            let store = h.state.store.lock();
            store
                .bind_row_tenant("reminders", "reminder_id", &reminder.reminder_id, "tenant-a")
                .expect("bind row tenant");
        }

        let uri = format!("/v1/reminders/{}", reminder.reminder_id);
        let mut owner_req = HttpRequest::builder()
            .uri(&uri)
            .body(axum::body::Body::empty())
            .expect("request");
        owner_req.extensions_mut().insert(TenantContext(
            kura_identity::TenantContext {
                tenant_id: "tenant-a".to_string(),
                ..Default::default()
            },
        ));
        let (status, _) = send(&crate::routes::router(h.state.clone()), owner_req).await;
        assert_eq!(status, StatusCode::OK);

        let mut other_req = HttpRequest::builder()
            .uri(&uri)
            .body(axum::body::Body::empty())
            .expect("request");
        other_req.extensions_mut().insert(TenantContext(
            kura_identity::TenantContext {
                tenant_id: "tenant-b".to_string(),
                ..Default::default()
            },
        ));
        let (status, json) = send(&crate::routes::router(h.state.clone()), other_req).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "body: {json}");
    }

    #[tokio::test]
    async fn unconfigured_manager_returns_500() {
        let h = harness(None);
        let mut state = h.state.clone();
        state.reminders = None;
        let app = crate::routes::router(state);
        let (status, json) = send(&app, request("GET", "/v1/reminders", None)).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "body: {json}");
        assert_eq!(json["code"], "internal");
        assert_eq!(json["message"], "reminders are not configured");
    }
}
