//! calendar route family (port of daemon/internal/api/calendar.go).
//!
//! Routes under the /v1/calendar prefix: account projections (provider sync
//! surface), event list/create/get/update/cancel, availability queries, and
//! the operation ledger (diagnostics). Every backend-backed handler funnels
//! through `kura_calendar::Manager`; the activity ledger (accounts,
//! operations, artifacts) is mirrored into the SQLite store and re-published
//! on the event bus exactly like the Go `recordCalendar*` helpers, with the
//! resolved tenant context bound onto tenant-owned event rows.

use std::sync::Arc;

use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json as AxumJson, Router};
use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use uuid::Uuid;

use kura_billing::{integration_operation_key, Category, ReserveInput, ResolveInput, UsageReservation};
use kura_calendar as calendar;
use kura_events as events;
use kura_integrations as integrations;

use crate::error::ApiError;
use crate::middleware::{environment_scope_from_config, guard_resource_for_tenant, TenantContext};
use crate::response::Json;
use crate::state::AppState;
use crate::types::{
    self, CalendarAttendeeRequest, CalendarAvailabilityQueryResponse, CalendarEventListResponse,
    CalendarEventResponse, CalendarOperationResponse, CalendarSourceLinkageRequest,
    CancelCalendarEventRequest, CreateCalendarAvailabilityQueryRequest, CreateCalendarEventRequest,
    ListResponse,
};

/// Route family router for the /v1/calendar prefix.
///
/// The Go server additionally wraps /v1/calendar/accounts/{id} and
/// /v1/calendar/operations/{id} in `withByIDTenantGuard`; those two
/// handlers call [`guard_resource_for_tenant`] directly so the behavior
/// (cross-tenant access is never leaked — 404) is preserved without needing
/// the guard layer at router-build time.
#[must_use]
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/calendar/accounts", get(list_accounts))
        .route("/v1/calendar/accounts/{integration_id}", get(get_account))
        .route("/v1/calendar/events", get(list_events).post(create_event))
        .route("/v1/calendar/events/{external_event_id}", get(get_event))
        .route("/v1/calendar/events/{external_event_id}/update", post(update_event))
        .route("/v1/calendar/events/{external_event_id}/cancel", post(cancel_event))
        .route("/v1/calendar/availability/queries", post(create_availability_query))
        .route("/v1/calendar/availability/queries/{query_id}", get(get_availability_query))
        .route("/v1/calendar/operations", get(list_operations))
        .route("/v1/calendar/operations/{operation_id}", get(get_operation))
}

// ---------------------------------------------------------------------------
// Query DTOs (Go reads r.URL.Query() directly; no types.go counterparts)
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountListQuery {
    integration_id: Option<String>,
    readiness_status: Option<String>,
    canonical_default: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventListQuery {
    integration_id: Option<String>,
    starts_at: Option<String>,
    ends_at: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OperationListQuery {
    integration_id: Option<String>,
    run_id: Option<String>,
    workflow_id: Option<String>,
    schedule_id: Option<String>,
    delivery_id: Option<String>,
    operation_class: Option<String>,
    status: Option<String>,
    external_event_id: Option<String>,
}

// ---------------------------------------------------------------------------
// GET /v1/calendar/accounts
// ---------------------------------------------------------------------------

/// GET /v1/calendar/accounts — project calendar accounts, filtered by
/// `integrationId` / `readinessStatus` / `canonicalDefault`.
async fn list_accounts(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Query(query): Query<AccountListQuery>,
) -> Result<Json<types::CalendarAccountListResponse>, ApiError> {
    let manager = calendar_manager(&state)?;
    let integrations = integrations_manager(&state)?;
    let selection = calendar::Selection {
        integration_id: trim_opt(query.integration_id.as_deref()),
    };
    let mut items = manager
        .list_accounts(&integrations.list(), &selection)
        .map_err(map_calendar_error)?;
    items = filter_calendar_accounts(items, &query);
    record_calendar_accounts(&state, tenant.as_ref().map(|e| &e.0), &items)?;
    Ok(Json(ListResponse { items }))
}

/// GET /v1/calendar/accounts/{integrationId} — one account projection,
/// tenant-guarded on `calendar_accounts`.
async fn get_account(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(integration_id): Path<String>,
) -> Result<Json<calendar::AccountProjection>, ApiError> {
    let manager = calendar_manager(&state)?;
    let integrations = integrations_manager(&state)?;
    let integration_id = integration_id.trim().to_string();
    if integration_id.is_empty() {
        return Err(ApiError::NotFound("not found".to_string()));
    }
    guard_resource_for_tenant(
        &state,
        tenant.as_ref().map(|e| &e.0),
        "api:GET /v1/calendar/accounts/{integrationId}",
        "calendar_accounts",
        "calendar_account_id",
        &integration_id,
        "calendar_account",
    )
    .await?;
    let selection = calendar::Selection { integration_id: integration_id.clone() };
    let items = manager
        .list_accounts(&integrations.list(), &selection)
        .map_err(map_calendar_error)?;
    if items.is_empty() {
        return Err(ApiError::NotFound("not found".to_string()));
    }
    record_calendar_accounts(&state, tenant.as_ref().map(|e| &e.0), &items[..1])?;
    Ok(Json(items[0].clone()))
}

// ---------------------------------------------------------------------------
// GET /v1/calendar/events
// ---------------------------------------------------------------------------

/// GET /v1/calendar/events — list events for the selected account.
async fn list_events(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    headers: HeaderMap,
    Query(query): Query<EventListQuery>,
) -> Result<Json<CalendarEventListResponse>, ApiError> {
    let manager = calendar_manager(&state)?;
    let integrations = integrations_manager(&state)?;
    let starts_at = parse_optional_calendar_timestamp(query.starts_at.as_deref())
        .map_err(|_| ApiError::BadRequest("startsAt must be RFC3339".to_string()))?;
    let ends_at = parse_optional_calendar_timestamp(query.ends_at.as_deref())
        .map_err(|_| ApiError::BadRequest("endsAt must be RFC3339".to_string()))?;

    let operation_id = calendar::new_operation_id();
    let reservation = begin_integration_operation_quota(
        &state,
        tenant.as_ref().map(|e| &e.0),
        &client_key(&headers),
        "calendar",
        &operation_id,
        "GET /v1/calendar/events",
    )
    .await?;
    let input = calendar::ListEventsInput {
        selection: calendar::Selection { integration_id: trim_opt(query.integration_id.as_deref()) },
        starts_at,
        ends_at,
        source: calendar::SourceLinkage { operation_id: operation_id.clone(), ..Default::default() },
    };
    let result = manager.list_events(&integrations.list(), &input);
    let (account, items, operation, artifacts) = match result {
        Ok(tuple) => tuple,
        Err(err) => {
            release_billing_reservation(&state, &reservation, "calendar operation failed before backend attempt").await;
            return Err(map_calendar_error(err));
        }
    };
    if !operation.operation_id.is_empty() {
        record_calendar_activity(&state, tenant.as_ref().map(|e| &e.0), &account, &operation, &artifacts)?;
        commit_billing_reservation(
            &state,
            &reservation,
            "billing.integration_operation_committed",
            "calendar operation recorded after backend attempt",
        )
        .await?;
    }
    Ok(Json(CalendarEventListResponse { account, items, operation, artifacts }))
}

// ---------------------------------------------------------------------------
// GET /v1/calendar/events/{externalEventId}
// ---------------------------------------------------------------------------

/// GET /v1/calendar/events/{externalEventId} — one event snapshot.
async fn get_event(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    headers: HeaderMap,
    Path(external_event_id): Path<String>,
) -> Result<Json<CalendarEventResponse>, ApiError> {
    let manager = calendar_manager(&state)?;
    let integrations = integrations_manager(&state)?;
    let operation_id = calendar::new_operation_id();
    let reservation = begin_integration_operation_quota(
        &state,
        tenant.as_ref().map(|e| &e.0),
        &client_key(&headers),
        "calendar",
        &operation_id,
        "GET /v1/calendar/events/{externalEventId}",
    )
    .await?;
    let input = calendar::GetEventInput {
        selection: calendar::Selection { integration_id: String::new() },
        external_event_id: external_event_id.trim().to_string(),
        source: calendar::SourceLinkage { operation_id: operation_id.clone(), ..Default::default() },
    };
    let result = manager.get_event(&integrations.list(), &input);
    let (account, item, operation, artifacts) = match result {
        Ok(tuple) => tuple,
        Err(err) => {
            release_billing_reservation(&state, &reservation, "calendar operation failed before backend attempt").await;
            return Err(map_calendar_error(err));
        }
    };
    if !operation.operation_id.is_empty() {
        record_calendar_activity(&state, tenant.as_ref().map(|e| &e.0), &account, &operation, &artifacts)?;
        commit_billing_reservation(
            &state,
            &reservation,
            "billing.integration_operation_committed",
            "calendar operation recorded after backend attempt",
        )
        .await?;
    }
    Ok(Json(CalendarEventResponse { account, event: item, operation, artifacts }))
}
// ---------------------------------------------------------------------------
// POST /v1/calendar/events
// ---------------------------------------------------------------------------

/// POST /v1/calendar/events — create an event (201).
async fn create_event(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    headers: HeaderMap,
    body: String,
) -> Result<(StatusCode, AxumJson<CalendarEventResponse>), ApiError> {
    let manager = calendar_manager(&state)?;
    let integrations = integrations_manager(&state)?;
    let request: CreateCalendarEventRequest = decode_json_body(&body)?;
    reject_unsupported_calendar_mutation(&request.calendar_ref)?;
    let starts_at = parse_calendar_timestamp(&request.starts_at)
        .map_err(|_| ApiError::BadRequest("startsAt must be RFC3339".to_string()))?;
    let ends_at = parse_calendar_timestamp(&request.ends_at)
        .map_err(|_| ApiError::BadRequest("endsAt must be RFC3339".to_string()))?;

    let operation_id = calendar::new_operation_id();
    let reservation = begin_integration_operation_quota(
        &state,
        tenant.as_ref().map(|e| &e.0),
        &client_key(&headers),
        "calendar",
        &operation_id,
        "POST /v1/calendar/events",
    )
    .await?;
    let input = calendar::CreateEventInput {
        selection: calendar::Selection { integration_id: request.integration_id.trim().to_string() },
        title: request.title.trim().to_string(),
        description: request.description.trim().to_string(),
        location: request.location.trim().to_string(),
        starts_at,
        ends_at,
        timezone: request.timezone.trim().to_string(),
        all_day: request.all_day,
        start_date: request.start_date.trim().to_string(),
        end_date: request.end_date.trim().to_string(),
        recurring: request.recurring,
        recurrence_rule: request.recurrence_rule.trim().to_string(),
        // Go passes only attendeeRequests; the domain input also carries a
        // plain email list that stays empty here.
        attendees: Vec::new(),
        attendee_requests: calendar_attendee_requests(&request.attendees),
        notify_attendees: request.notify_attendees,
        source: calendar_source_linkage_with_operation(request.source.as_ref(), &operation_id),
    };
    let result = manager.create_event(&integrations.list(), &input);
    let (account, item, operation, artifacts) = match result {
        Ok(tuple) => tuple,
        Err(err) => {
            release_billing_reservation(&state, &reservation, "calendar operation failed before backend attempt").await;
            return Err(map_calendar_error(err));
        }
    };
    if !operation.operation_id.is_empty() {
        record_calendar_activity(&state, tenant.as_ref().map(|e| &e.0), &account, &operation, &artifacts)?;
        commit_billing_reservation(
            &state,
            &reservation,
            "billing.integration_operation_committed",
            "calendar operation recorded after backend attempt",
        )
        .await?;
    }
    Ok((StatusCode::CREATED, AxumJson(CalendarEventResponse { account, event: item, operation, artifacts })))
}

// ---------------------------------------------------------------------------
// POST /v1/calendar/events/{externalEventId}/update
// ---------------------------------------------------------------------------

/// POST /v1/calendar/events/{externalEventId}/update — update an event.
async fn update_event(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    headers: HeaderMap,
    Path(external_event_id): Path<String>,
    body: String,
) -> Result<Json<CalendarEventResponse>, ApiError> {
    let manager = calendar_manager(&state)?;
    let integrations = integrations_manager(&state)?;
    let request: CreateCalendarEventRequest = decode_json_body(&body)?;
    reject_unsupported_calendar_mutation(&request.calendar_ref)?;
    let starts_at = parse_calendar_timestamp(&request.starts_at)
        .map_err(|_| ApiError::BadRequest("startsAt must be RFC3339".to_string()))?;
    let ends_at = parse_calendar_timestamp(&request.ends_at)
        .map_err(|_| ApiError::BadRequest("endsAt must be RFC3339".to_string()))?;

    let operation_id = calendar::new_operation_id();
    let reservation = begin_integration_operation_quota(
        &state,
        tenant.as_ref().map(|e| &e.0),
        &client_key(&headers),
        "calendar",
        &operation_id,
        "POST /v1/calendar/events/{externalEventId}/update",
    )
    .await?;
    let input = calendar::UpdateEventInput {
        selection: calendar::Selection { integration_id: request.integration_id.trim().to_string() },
        external_event_id: external_event_id.trim().to_string(),
        title: request.title.trim().to_string(),
        description: request.description.trim().to_string(),
        location: request.location.trim().to_string(),
        starts_at,
        ends_at,
        timezone: request.timezone.trim().to_string(),
        all_day: request.all_day,
        start_date: request.start_date.trim().to_string(),
        end_date: request.end_date.trim().to_string(),
        recurring: request.recurring,
        recurrence_rule: request.recurrence_rule.trim().to_string(),
        recurrence_scope: calendar_recurrence_scope(&request.recurrence_scope)?,
        // Go passes only attendeeRequests; the domain input also carries a
        // plain email list that stays empty here.
        attendees: Vec::new(),
        attendee_requests: calendar_attendee_requests(&request.attendees),
        notify_attendees: request.notify_attendees,
        source: calendar_source_linkage_with_operation(request.source.as_ref(), &operation_id),
    };
    let result = manager.update_event(&integrations.list(), &input);
    let (account, item, operation, artifacts) = match result {
        Ok(tuple) => tuple,
        Err(err) => {
            release_billing_reservation(&state, &reservation, "calendar operation failed before backend attempt").await;
            return Err(map_calendar_error(err));
        }
    };
    if !operation.operation_id.is_empty() {
        record_calendar_activity(&state, tenant.as_ref().map(|e| &e.0), &account, &operation, &artifacts)?;
        commit_billing_reservation(
            &state,
            &reservation,
            "billing.integration_operation_committed",
            "calendar operation recorded after backend attempt",
        )
        .await?;
    }
    Ok(Json(CalendarEventResponse { account, event: item, operation, artifacts }))
}

// ---------------------------------------------------------------------------
// POST /v1/calendar/events/{externalEventId}/cancel
// ---------------------------------------------------------------------------

/// POST /v1/calendar/events/{externalEventId}/cancel — cancel an event.
async fn cancel_event(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    headers: HeaderMap,
    Path(external_event_id): Path<String>,
    body: String,
) -> Result<Json<CalendarEventResponse>, ApiError> {
    let manager = calendar_manager(&state)?;
    let integrations = integrations_manager(&state)?;
    let request: CancelCalendarEventRequest = decode_json_body(&body)?;
    if !request.calendar_ref.trim().is_empty() {
        return Err(map_calendar_error(calendar::CalendarError::CalendarAlternateCalendarDeny));
    }

    let operation_id = calendar::new_operation_id();
    let reservation = begin_integration_operation_quota(
        &state,
        tenant.as_ref().map(|e| &e.0),
        &client_key(&headers),
        "calendar",
        &operation_id,
        "POST /v1/calendar/events/{externalEventId}/cancel",
    )
    .await?;
    let input = calendar::CancelEventInput {
        selection: calendar::Selection { integration_id: request.integration_id.trim().to_string() },
        external_event_id: external_event_id.trim().to_string(),
        reason: request.reason.trim().to_string(),
        recurrence_scope: calendar_recurrence_scope(&request.recurrence_scope)?,
        source: calendar_source_linkage_with_operation(request.source.as_ref(), &operation_id),
    };
    let result = manager.cancel_event(&integrations.list(), &input);
    let (account, item, operation, artifacts) = match result {
        Ok(tuple) => tuple,
        Err(err) => {
            release_billing_reservation(&state, &reservation, "calendar operation failed before backend attempt").await;
            return Err(map_calendar_error(err));
        }
    };
    if !operation.operation_id.is_empty() {
        record_calendar_activity(&state, tenant.as_ref().map(|e| &e.0), &account, &operation, &artifacts)?;
        commit_billing_reservation(
            &state,
            &reservation,
            "billing.integration_operation_committed",
            "calendar operation recorded after backend attempt",
        )
        .await?;
    }
    Ok(Json(CalendarEventResponse { account, event: item, operation, artifacts }))
}

// ---------------------------------------------------------------------------
// Availability queries
// ---------------------------------------------------------------------------

/// POST /v1/calendar/availability/queries — busy/free window query (201).
async fn create_availability_query(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    headers: HeaderMap,
    body: String,
) -> Result<(StatusCode, AxumJson<CalendarAvailabilityQueryResponse>), ApiError> {
    let manager = calendar_manager(&state)?;
    let integrations = integrations_manager(&state)?;
    let request: CreateCalendarAvailabilityQueryRequest = decode_json_body(&body)?;
    let window_start = parse_calendar_timestamp(&request.window_start)
        .map_err(|_| ApiError::BadRequest("windowStart must be RFC3339".to_string()))?;
    let window_end = parse_calendar_timestamp(&request.window_end)
        .map_err(|_| ApiError::BadRequest("windowEnd must be RFC3339".to_string()))?;

    let operation_id = calendar::new_operation_id();
    let reservation = begin_integration_operation_quota(
        &state,
        tenant.as_ref().map(|e| &e.0),
        &client_key(&headers),
        "calendar",
        &operation_id,
        "POST /v1/calendar/availability/queries",
    )
    .await?;
    let input = calendar::BusyFreeInput {
        selection: calendar::Selection { integration_id: request.integration_id.trim().to_string() },
        window_start,
        window_end,
        timezone: request.timezone.trim().to_string(),
        source: calendar_source_linkage_with_operation(request.source.as_ref(), &operation_id),
    };
    let result = manager.busy_free(&integrations.list(), &input);
    let (account, query, operation, artifacts) = match result {
        Ok(tuple) => tuple,
        Err(err) => {
            release_billing_reservation(&state, &reservation, "calendar operation failed before backend attempt").await;
            return Err(map_calendar_error(err));
        }
    };
    if !operation.operation_id.is_empty() {
        record_calendar_activity(&state, tenant.as_ref().map(|e| &e.0), &account, &operation, &artifacts)?;
        commit_billing_reservation(
            &state,
            &reservation,
            "billing.integration_operation_committed",
            "calendar operation recorded after backend attempt",
        )
        .await?;
    }
    Ok((StatusCode::CREATED, AxumJson(CalendarAvailabilityQueryResponse { account, query, operation, artifacts })))
}

/// GET /v1/calendar/availability/queries/{queryId} — replay one availability
/// query from its operation artifact.
async fn get_availability_query(
    State(state): State<AppState>,
    Path(query_id): Path<String>,
) -> Result<Json<CalendarAvailabilityQueryResponse>, ApiError> {
    let manager = calendar_manager_only(&state)?;
    let query_id = query_id.trim().to_string();
    if query_id.is_empty() {
        return Err(ApiError::NotFound("not found".to_string()));
    }
    let operation = manager
        .get_operation(&query_id)
        .ok_or_else(|| ApiError::NotFound("not found".to_string()))?;
    let artifacts = manager.list_artifacts(&operation.operation_id);
    let mut query = None;
    for item in &artifacts {
        if item.kind == calendar::ArtifactKind::AvailabilityQuery && item.availability_query.is_some() {
            query = item.availability_query.clone();
            break;
        }
    }
    let query = query.ok_or_else(|| ApiError::NotFound("not found".to_string()))?;
    let account = manager
        .get_account(&operation.integration_id)
        .ok_or_else(|| ApiError::internal("calendar account projection is unavailable"))?;
    Ok(Json(CalendarAvailabilityQueryResponse { account, query, operation, artifacts }))
}
// ---------------------------------------------------------------------------
// Operations (ledger / diagnostics)
// ---------------------------------------------------------------------------

/// GET /v1/calendar/operations — list operation ledger entries with filters.
async fn list_operations(
    State(state): State<AppState>,
    Query(query): Query<OperationListQuery>,
) -> Result<Json<types::CalendarOperationListResponse>, ApiError> {
    let manager = calendar_manager_only(&state)?;
    let integration_id = query.integration_id.as_deref().unwrap_or("").trim().to_string();
    let run_id = query.run_id.as_deref().unwrap_or("").trim().to_string();
    let workflow_id = query.workflow_id.as_deref().unwrap_or("").trim().to_string();
    let schedule_id = query.schedule_id.as_deref().unwrap_or("").trim().to_string();
    let delivery_id = query.delivery_id.as_deref().unwrap_or("").trim().to_string();
    let operation_class_raw = query.operation_class.as_deref().unwrap_or("").trim();
    let status_raw = query.status.as_deref().unwrap_or("").trim();
    let external_event_id = query.external_event_id.as_deref().unwrap_or("").trim().to_string();

    // Go casts the raw query strings onto the domain enums; a value that does
    // not parse never equals a real operation class/status, so the list is
    // empty rather than unfiltered.
    let operation_class = parse_operation_class(operation_class_raw);
    let status = parse_operation_status(status_raw);
    if (!operation_class_raw.is_empty() && operation_class.is_none())
        || (!status_raw.is_empty() && status.is_none())
    {
        return Ok(Json(types::CalendarOperationListResponse { items: Vec::new() }));
    }

    let filter = calendar::OperationFilter {
        integration_id,
        run_id,
        workflow_id,
        schedule_id,
        delivery_id,
        operation_class: operation_class.unwrap_or_default(),
        status: status.unwrap_or_default(),
        external_event_id,
    };
    Ok(Json(types::CalendarOperationListResponse { items: manager.list_operations(&filter) }))
}

/// GET /v1/calendar/operations/{operationId} — one operation plus artifacts,
/// tenant-guarded on `calendar_operations`.
async fn get_operation(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(operation_id): Path<String>,
) -> Result<Json<CalendarOperationResponse>, ApiError> {
    let manager = calendar_manager_only(&state)?;
    let operation_id = operation_id.trim().to_string();
    if operation_id.is_empty() {
        return Err(ApiError::NotFound("not found".to_string()));
    }
    guard_resource_for_tenant(
        &state,
        tenant.as_ref().map(|e| &e.0),
        "api:GET /v1/calendar/operations/{operationId}",
        "calendar_operations",
        "operation_id",
        &operation_id,
        "calendar_operation",
    )
    .await?;
    let operation = manager
        .get_operation(&operation_id)
        .ok_or_else(|| ApiError::NotFound("not found".to_string()))?;
    let artifacts = manager.list_artifacts(&operation_id);
    Ok(Json(CalendarOperationResponse { operation, artifacts }))
}

// ---------------------------------------------------------------------------
// Billing quota (Go beginIntegrationOperationQuota / commit / release)
// ---------------------------------------------------------------------------

/// Reserves quota for one calendar operation. Mirrors the Go helper: no
/// resolved tenant → no reservation; nil billing manager → allowed in
/// non-hosted environments, denied in hosted ones; otherwise delegate to
/// `kura_billing::Manager::reserve` and surface denials as errors.
async fn begin_integration_operation_quota(
    state: &AppState,
    tenant: Option<&TenantContext>,
    client_key: &str,
    domain: &str,
    operation_id: &str,
    entry_point: &str,
) -> Result<UsageReservation, ApiError> {
    let Some(tc) = tenant else {
        return Ok(UsageReservation::default());
    };
    if tc.0.tenant_id.is_empty() {
        return Ok(UsageReservation::default());
    }
    let hosted = matches!(state.config.environment, kura_config::Environment::Prod);
    let operation_key = integration_operation_key(&tc.0.tenant_id, domain, operation_id, client_key);
    let Some(billing) = &state.billing else {
        if hosted {
            // Go: quota-state-unavailable denial (503). ApiError has no
            // ServiceUnavailable variant outside the tenant-migration gate, so
            // this nil-manager-hosted edge maps to 500 for now.
            return Err(ApiError::internal("billing quota state unavailable"));
        }
        return Ok(UsageReservation::default());
    };
    let result = billing
        .reserve(ReserveInput {
            tenant_id: tc.0.tenant_id.clone(),
            category: Category::new(Category::INTEGRATION_OPERATIONS),
            amount: 1,
            operation_key,
            reservation_point: format!("{entry_point} before integration backend operation"),
            guarded_entry_point: entry_point.to_string(),
            actor_principal_id: tc.0.principal_id.clone(),
            hosted,
        })
        .await
        .map_err(ApiError::internal)?;
    if !result.allowed {
        // Go: writeBillingDenial → 429 (QuotaDenied) / 503 (state
        // unavailable). See the ApiError surface note above.
        return Err(ApiError::internal("billing reservation denied"));
    }
    Ok(result.reservation.unwrap_or_default())
}

/// Commits a reservation after the backend attempt (Go commitBillingReservation).
async fn commit_billing_reservation(
    state: &AppState,
    reservation: &UsageReservation,
    reason_code: &str,
    reason: &str,
) -> Result<(), ApiError> {
    let Some(billing) = &state.billing else {
        return Ok(());
    };
    if reservation.reservation_id.is_empty() {
        return Ok(());
    }
    billing
        .commit(ResolveInput {
            tenant_id: reservation.tenant_id.clone(),
            category: reservation.category.clone(),
            operation_key: reservation.operation_key.clone(),
            amount: reservation.amount_reserved,
            reason_code: reason_code.to_string(),
            reason: reason.to_string(),
            actor_principal_id: String::new(),
        })
        .await
        .map_err(ApiError::internal)?;
    Ok(())
}

/// Releases a reservation when the backend attempt never happened (Go
/// releaseBillingReservation — errors are ignored).
async fn release_billing_reservation(state: &AppState, reservation: &UsageReservation, reason: &str) {
    let Some(billing) = &state.billing else {
        return;
    };
    if reservation.reservation_id.is_empty() {
        return;
    }
    let _ = billing
        .release(ResolveInput {
            tenant_id: reservation.tenant_id.clone(),
            category: reservation.category.clone(),
            operation_key: reservation.operation_key.clone(),
            amount: reservation.amount_reserved,
            reason_code: "billing.reservation_released".to_string(),
            reason: reason.to_string(),
            actor_principal_id: String::new(),
        })
        .await;
}

// ---------------------------------------------------------------------------
// Activity ledger persistence + event publishing
// (Go recordCalendarAccounts / recordCalendarActivity + publishEvent)
// ---------------------------------------------------------------------------

/// Persists each account projection and publishes `calendar.account_projected`.
fn record_calendar_accounts(
    state: &AppState,
    tenant: Option<&TenantContext>,
    items: &[calendar::AccountProjection],
) -> Result<(), ApiError> {
    for item in items {
        persist_calendar_account(state, item)?;
        publish_calendar_account_projected(state, tenant, item)?;
    }
    Ok(())
}

/// Mirrors Go recordCalendarActivity: account + operation + artifacts are
/// persisted and re-published, followed by the terminal operation event.
fn record_calendar_activity(
    state: &AppState,
    tenant: Option<&TenantContext>,
    account: &calendar::AccountProjection,
    operation: &calendar::Operation,
    artifacts: &[calendar::Artifact],
) -> Result<(), ApiError> {
    if !account.integration_id.is_empty() {
        persist_calendar_account(state, account)?;
        publish_calendar_account_projected(state, tenant, account)?;
    }
    if operation.operation_id.is_empty() {
        return Ok(());
    }
    persist_calendar_operation(state, operation)?;
    publish_calendar_operation_requested(state, tenant, operation)?;
    for artifact in artifacts {
        persist_calendar_artifact(state, artifact)?;
        publish_calendar_artifact_recorded(state, tenant, artifact, operation)?;
    }
    match operation.status {
        calendar::OperationStatus::Completed => publish_calendar_operation_completed(state, tenant, operation),
        calendar::OperationStatus::Failed
        | calendar::OperationStatus::Blocked
        | calendar::OperationStatus::Cancelled => publish_calendar_operation_failed(state, tenant, operation),
        _ => Ok(()),
    }
}

fn persist_calendar_account(state: &AppState, item: &calendar::AccountProjection) -> Result<(), ApiError> {
    state
        .store
        .lock()
        .upsert_calendar_account(item)
        .map_err(ApiError::from_store)
}

fn persist_calendar_operation(state: &AppState, item: &calendar::Operation) -> Result<(), ApiError> {
    state
        .store
        .lock()
        .upsert_calendar_operation(item)
        .map_err(ApiError::from_store)
}

fn persist_calendar_artifact(state: &AppState, item: &calendar::Artifact) -> Result<(), ApiError> {
    state
        .store
        .lock()
        .upsert_calendar_artifact(item)
        .map_err(ApiError::from_store)
}

/// Go publishEvent: bind environment scope + tenant, persist (tenant-owned or
/// global path), then publish on the bus. `calendar` is not a global category,
/// so resolved tenants are bound onto the row.
fn publish_event(state: &AppState, tenant: Option<&TenantContext>, event: events::Event) -> Result<(), ApiError> {
    let mut prepared = event;
    if prepared.environment_scope.is_empty() {
        prepared.environment_scope = environment_scope_from_config(&state.config);
    }
    if prepared.tenant_id.is_empty() {
        if let Some(tc) = tenant {
            if !tc.0.tenant_id.is_empty() && !events::is_global_category(&prepared.category) {
                prepared.tenant_id = tc.0.tenant_id.clone();
            }
        }
    }
    if prepared.event_id.is_empty() {
        prepared.event_id = new_event_id();
    }
    if prepared.occurred_at == DateTime::<Utc>::MIN_UTC {
        prepared.occurred_at = Utc::now();
    }
    let persisted = if prepared.tenant_id.is_empty() {
        state.store.lock().append_event(&prepared)
    } else {
        state
            .store
            .lock()
            .append_event_for_tenant_raw(&prepared, &prepared.tenant_id)
    }
    .map_err(ApiError::from_store)?;
    let _ = state.event_bus.publish(persisted);
    Ok(())
}

fn publish_calendar_account_projected(
    state: &AppState,
    tenant: Option<&TenantContext>,
    account: &calendar::AccountProjection,
) -> Result<(), ApiError> {
    let payload = serde_json::json!({
        "integrationId": account.integration_id,
        "accountKey": account.account_key,
        "primaryCalendarRef": account.primary_calendar_ref,
        "primaryTimezone": account.primary_timezone,
        "readinessStatus": account.readiness_status,
        "canonicalDefault": account.canonical_default,
    });
    publish_event(
        state,
        tenant,
        events::Event {
            category: "calendar".to_string(),
            name: "calendar.account_projected".to_string(),
            resource: events::Resource { kind: "calendar_account".to_string(), id: account.calendar_account_id.clone() },
            payload: payload.as_object().cloned().unwrap_or_default(),
            ..events::Event::default()
        },
    )
}

fn publish_calendar_operation_requested(
    state: &AppState,
    tenant: Option<&TenantContext>,
    operation: &calendar::Operation,
) -> Result<(), ApiError> {
    publish_calendar_operation_event(state, tenant, "calendar.operation_requested", operation)
}

fn publish_calendar_operation_completed(
    state: &AppState,
    tenant: Option<&TenantContext>,
    operation: &calendar::Operation,
) -> Result<(), ApiError> {
    publish_calendar_operation_event(state, tenant, "calendar.operation_completed", operation)
}

fn publish_calendar_operation_failed(
    state: &AppState,
    tenant: Option<&TenantContext>,
    operation: &calendar::Operation,
) -> Result<(), ApiError> {
    publish_calendar_operation_event(state, tenant, "calendar.operation_failed", operation)
}

fn publish_calendar_operation_event(
    state: &AppState,
    tenant: Option<&TenantContext>,
    name: &str,
    operation: &calendar::Operation,
) -> Result<(), ApiError> {
    let scope = events::Scope {
        run_id: operation.run_id.clone(),
        workflow_id: operation.workflow_id.clone(),
        schedule_id: operation.schedule_id.clone(),
        ..events::Scope::default()
    };
    let payload = serde_json::json!({
        "operationId": operation.operation_id,
        "operationClass": operation.operation_class.as_str(),
        "integrationId": operation.integration_id,
        "runId": operation.run_id,
        "workflowId": operation.workflow_id,
        "scheduleId": operation.schedule_id,
        "deliveryId": operation.delivery_id,
        "status": operation.status.as_str(),
        "timezoneUsed": operation.timezone_used,
        "externalEventId": operation.external_event_id,
        "failureClass": operation.failure_class,
    });
    publish_event(
        state,
        tenant,
        events::Event {
            category: "calendar".to_string(),
            name: name.to_string(),
            scope,
            resource: events::Resource { kind: "calendar_operation".to_string(), id: operation.operation_id.clone() },
            payload: payload.as_object().cloned().unwrap_or_default(),
            ..events::Event::default()
        },
    )
}

fn publish_calendar_artifact_recorded(
    state: &AppState,
    tenant: Option<&TenantContext>,
    artifact: &calendar::Artifact,
    operation: &calendar::Operation,
) -> Result<(), ApiError> {
    let scope = events::Scope {
        run_id: operation.run_id.clone(),
        workflow_id: operation.workflow_id.clone(),
        schedule_id: operation.schedule_id.clone(),
        ..events::Scope::default()
    };
    let payload = serde_json::json!({
        "artifactId": artifact.artifact_id,
        "operationId": artifact.operation_id,
        "externalEventId": artifact.external_event_id,
        "calendarRef": artifact.calendar_ref,
        "lifecycleState": artifact.lifecycle_state,
    });
    publish_event(
        state,
        tenant,
        events::Event {
            category: "calendar".to_string(),
            name: "calendar.artifact_recorded".to_string(),
            scope,
            resource: events::Resource { kind: "calendar_artifact".to_string(), id: artifact.artifact_id.clone() },
            payload: payload.as_object().cloned().unwrap_or_default(),
            ..events::Event::default()
        },
    )
}

/// Mirrors the events crate private id generator (Go `newEventID`).
fn new_event_id() -> String {
    let hex = Uuid::new_v4().simple().to_string();
    format!("evt_{}", &hex[..16])
}

// ---------------------------------------------------------------------------
// Small port helpers (Go calendarSourceLinkage / attendee / recurrence / filter)
// ---------------------------------------------------------------------------

fn calendar_source_linkage(source: Option<&CalendarSourceLinkageRequest>) -> calendar::SourceLinkage {
    let Some(s) = source else {
        return calendar::SourceLinkage::default();
    };
    calendar::SourceLinkage {
        operation_id: String::new(),
        run_id: s.run_id.trim().to_string(),
        step_id: s.step_id.trim().to_string(),
        tool_call_id: s.tool_call_id.trim().to_string(),
        workflow_id: s.workflow_id.trim().to_string(),
        workflow_step_id: s.workflow_step_id.trim().to_string(),
        schedule_id: s.schedule_id.trim().to_string(),
        schedule_attempt_id: s.schedule_attempt_id.trim().to_string(),
        delivery_id: s.delivery_id.trim().to_string(),
    }
}

fn calendar_source_linkage_with_operation(
    source: Option<&CalendarSourceLinkageRequest>,
    operation_id: &str,
) -> calendar::SourceLinkage {
    let mut linkage = calendar_source_linkage(source);
    linkage.operation_id = operation_id.trim().to_string();
    linkage
}

/// Go filterCalendarAccounts: readinessStatus exact match, optional
/// canonicalDefault bool filter (ignored when unparsable).
fn filter_calendar_accounts(
    items: Vec<calendar::AccountProjection>,
    query: &AccountListQuery,
) -> Vec<calendar::AccountProjection> {
    let readiness_status = query.readiness_status.as_deref().unwrap_or("").trim();
    let canonical_default = query.canonical_default.as_deref().unwrap_or("").trim();
    let mut want_default = false;
    let mut filter_default = false;
    if !canonical_default.is_empty() {
        if let Ok(parsed) = canonical_default.parse::<bool>() {
            filter_default = true;
            want_default = parsed;
        }
    }
    items
        .into_iter()
        .filter(|item| {
            if !readiness_status.is_empty() && item.readiness_status != readiness_status {
                return false;
            }
            if filter_default && item.canonical_default != want_default {
                return false;
            }
            true
        })
        .collect()
}

/// Rejects mutations targeting an alternate calendar (Go
/// rejectUnsupportedCalendarMutation).
fn reject_unsupported_calendar_mutation(calendar_ref: &str) -> Result<(), ApiError> {
    if !calendar_ref.trim().is_empty() {
        return Err(map_calendar_error(calendar::CalendarError::CalendarAlternateCalendarDeny));
    }
    Ok(())
}

/// Maps the API recurrence-scope string to the domain model; unknown
/// non-empty values are invalid (400), matching the manager validation.
fn calendar_recurrence_scope(raw: &str) -> Result<calendar::RecurrenceScope, ApiError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(calendar::RecurrenceScope::Unspecified);
    }
    serde_json::from_str::<calendar::RecurrenceScope>(&format!("\"{trimmed}\"")).map_err(|_| map_calendar_error(calendar::CalendarError::CalendarRecurrenceScopeInvalid))
}

/// Maps API attendee requests to the domain attendee model, skipping empty
/// emails and defaulting the role to `required` (Go calendarAttendeeRequests).
fn calendar_attendee_requests(items: &[CalendarAttendeeRequest]) -> Vec<calendar::AttendeeRequest> {
    if items.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(items.len());
    for attendee in items {
        if attendee.email.trim().is_empty() {
            continue;
        }
        let mut role = calendar::AttendeeRole::Required.as_str();
        if attendee.role.trim().eq_ignore_ascii_case(calendar::AttendeeRole::Optional.as_str()) {
            role = calendar::AttendeeRole::Optional.as_str();
        }
        out.push(calendar::AttendeeRequest {
            email: attendee.email.trim().to_string(),
            display_name: attendee.display_name.trim().to_string(),
            role: role.to_string(),
        });
    }
    out
}

/// Go writeCalendarError mapping.
fn map_calendar_error(err: calendar::CalendarError) -> ApiError {
    match err {
        calendar::CalendarError::CalendarIntegrationNotFound
        | calendar::CalendarError::CalendarEventNotFound
        | calendar::CalendarError::CalendarOperationNotFound
        | calendar::CalendarError::CalendarAccountNotFound => ApiError::NotFound("not found".to_string()),
        calendar::CalendarError::CalendarUnavailable => ApiError::Conflict(err.to_string()),
        calendar::CalendarError::CalendarSelectionInvalid
        | calendar::CalendarError::CalendarRecurringUnsupported
        | calendar::CalendarError::CalendarAllDayUnsupported
        | calendar::CalendarError::CalendarAttendeesUnsupported
        | calendar::CalendarError::CalendarAlternateCalendarDeny
        | calendar::CalendarError::CalendarInvalidTimeRange
        | calendar::CalendarError::CalendarRecurrenceScopeRequired
        | calendar::CalendarError::CalendarRecurrenceScopeInvalid => ApiError::BadRequest(err.to_string()),
        other => ApiError::internal(other),
    }
}

/// Go parseCalendarTimestamp (RFC3339; offsets normalize to UTC).
fn parse_calendar_timestamp(raw: &str) -> Result<DateTime<Utc>, ()> {
    DateTime::parse_from_rfc3339(raw.trim())
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|_| ())
}

/// Go parseOptionalCalendarTimestamp.
fn parse_optional_calendar_timestamp(raw: Option<&str>) -> Result<Option<DateTime<Utc>>, ()> {
    let raw = raw.unwrap_or("").trim();
    if raw.is_empty() {
        return Ok(None);
    }
    parse_calendar_timestamp(raw).map(Some)
}

/// Go decodeJSONBody: empty body → 400 "request body is required"; malformed
/// JSON → 400 with the serde error text.
fn decode_json_body<T: DeserializeOwned>(body: &str) -> Result<T, ApiError> {
    if body.is_empty() {
        return Err(ApiError::BadRequest("request body is required".to_string()));
    }
    serde_json::from_str(body).map_err(|e| ApiError::BadRequest(e.to_string()))
}

/// Parses a calendar operation-class query value (Go direct cast semantics).
fn parse_operation_class(raw: &str) -> Option<calendar::OperationClass> {
    serde_json::from_str::<calendar::OperationClass>(&format!("\"{}\"", raw.trim())).ok()
}

/// Parses a calendar operation-status query value (Go direct cast semantics).
fn parse_operation_status(raw: &str) -> Option<calendar::OperationStatus> {
    serde_json::from_str::<calendar::OperationStatus>(&format!("\"{}\"", raw.trim())).ok()
}

fn calendar_manager(state: &AppState) -> Result<Arc<calendar::Manager>, ApiError> {
    state
        .calendar
        .clone()
        .ok_or_else(|| ApiError::internal("calendar dependencies are not configured"))
}

fn integrations_manager(state: &AppState) -> Result<Arc<integrations::Manager>, ApiError> {
    state
        .integrations
        .clone()
        .ok_or_else(|| ApiError::internal("calendar dependencies are not configured"))
}

fn calendar_manager_only(state: &AppState) -> Result<Arc<calendar::Manager>, ApiError> {
    state
        .calendar
        .clone()
        .ok_or_else(|| ApiError::internal("calendar manager is not configured"))
}

fn trim_opt(raw: Option<&str>) -> String {
    raw.unwrap_or("").trim().to_string()
}

fn client_key(headers: &HeaderMap) -> String {
    headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use parking_lot::Mutex;
    use tower::ServiceExt;
    use uuid::Uuid;

    fn test_config() -> kura_config::Config {
        kura_config::Config {
            project_root: String::new(),
            environment: kura_config::Environment::Test,
            bind_addr: "127.0.0.1:19192".to_string(),
            data_dir: "/tmp/kura-api-calendar-test".to_string(),
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

    /// AppState with calendar + integrations managers wired (billing stays
    /// None — the test environment allows without a manager, like Go).
    fn test_state() -> AppState {
        let dir = std::env::temp_dir().join(format!("kura-api-calendar-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let store = Arc::new(Mutex::new(
            kura_store::SQLiteStore::new(dir.to_str().expect("path")).expect("store"),
        ));
        let mut state = AppState::new(test_config(), Arc::new(kura_events::Bus::new()), store);
        state.calendar = Some(Arc::new(calendar::Manager::new("test")));
        state.integrations = Some(Arc::new(integrations::Manager::new("test")));
        state
    }

    /// Go seedHealthyCalendarIntegration.
    fn seed_healthy_calendar_integration(state: &AppState, integration_id: &str, canonical_default: bool) {
        let manager = state.integrations.as_ref().expect("integrations manager");
        let resource = manager
            .create(integrations::CreateInput {
                integration_id: integration_id.to_string(),
                domain_kind: "calendar".to_string(),
                display_name: integration_id.to_string(),
                environment_scope: "test".to_string(),
                canonical_default,
                account_binding: integrations::AccountBinding {
                    account_key: "acct_calendar".to_string(),
                    account_label: "Primary Calendar".to_string(),
                    ..Default::default()
                },
                backend_binding: integrations::BackendBinding {
                    backend_kind: integrations::BackendKind::FakeLocal,
                    supports_probe_read: true,
                    supports_probe_mutation: true,
                    ..Default::default()
                },
                ..Default::default()
            })
            .expect("create integration");
        manager
            .update_readiness(
                &resource.integration_id,
                integrations::UpdateReadinessInput {
                    readiness_status: integrations::ReadinessStatus::Healthy,
                    auth_state: "authorized".to_string(),
                    health_state: "healthy".to_string(),
                    secret_resolution: "resolved".to_string(),
                    ..Default::default()
                },
            )
            .expect("update readiness");
    }

    async fn request_json(
        app: &Router,
        method: &str,
        uri: &str,
        body: Option<&str>,
        tenant: Option<&str>,
    ) -> (StatusCode, serde_json::Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        if body.is_some() {
            builder = builder.header(axum::http::header::CONTENT_TYPE, "application/json");
        }
        let mut req = builder.body(Body::from(body.unwrap_or("").to_string())).expect("request");
        if let Some(tenant_id) = tenant {
            let ctx = kura_identity::TenantContext { tenant_id: tenant_id.to_string(), ..Default::default() };
            req.extensions_mut().insert(TenantContext(ctx));
        }
        let response = app.clone().oneshot(req).await.expect("oneshot");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.expect("body");
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    #[tokio::test]
    async fn accounts_selection_fallback_and_availability() {
        let state = test_state();
        seed_healthy_calendar_integration(&state, "calendar-a", true);
        seed_healthy_calendar_integration(&state, "calendar-b", false);
        let app = router().with_state(state.clone());

        // canonicalDefault filter projects only the default account.
        let (status, json) = request_json(&app, "GET", "/v1/calendar/accounts?canonicalDefault=true", None, None).await;
        assert_eq!(status, StatusCode::OK);
        let items = json["items"].as_array().expect("items");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["integrationId"], "calendar-a");
        assert!(!items[0]["primaryTimezone"].as_str().unwrap_or("").is_empty());
        assert!(!items[0]["primaryCalendarRef"].as_str().unwrap_or("").is_empty());

        // Explicit selection lists events for calendar-b.
        let (status, json) = request_json(&app, "GET", "/v1/calendar/events?integrationId=calendar-b", None, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["account"]["integrationId"], "calendar-b");
        assert_eq!(json["operation"]["selectionMode"], "explicit");
        assert!(!json["items"].as_array().unwrap().is_empty());
        assert!(!json["artifacts"].as_array().unwrap().is_empty());

        // Default selection falls back to the canonical account.
        let (status, json) = request_json(&app, "GET", "/v1/calendar/events", None, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["account"]["integrationId"], "calendar-a");
        assert_eq!(json["operation"]["selectionMode"], "canonical_default");

        // Availability query: primary timezone default + completed busy_free op.
        let (status, json) = request_json(
            &app,
            "POST",
            "/v1/calendar/availability/queries",
            Some(r#"{"integrationId":"calendar-b","windowStart":"2026-04-23T09:00:00-07:00","windowEnd":"2026-04-23T11:00:00-07:00"}"#),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(json["query"]["timezone"], "America/Los_Angeles");
        assert_eq!(json["operation"]["operationClass"], "busy_free");
        assert_eq!(json["operation"]["status"], "completed");

        // The activity ledger was persisted to the store.
        let persisted = state
            .store
            .lock()
            .list_calendar_operations("test", &kura_store::calendar::CalendarOperationFilter::default())
            .expect("list operations");
        // The default list (calendar-a) also persists its operation, so the
        // ledger holds the explicit list + default list + busy_free entries.
        assert!(persisted.len() >= 3, "expected list + busy_free operations, got {}", persisted.len());
        assert!(
            persisted.iter().any(|op| op.integration_id == "calendar-b"),
            "expected a persisted calendar-b operation"
        );
    }

    #[tokio::test]
    async fn mutation_routes_preserve_identity_and_reject_unsupported() {
        let state = test_state();
        seed_healthy_calendar_integration(&state, "calendar-a", true);
        let app = router().with_state(state.clone());

        let (status, created) = request_json(
            &app,
            "POST",
            "/v1/calendar/events",
            Some(r#"{"integrationId":"calendar-a","title":"Phase 29 event","startsAt":"2026-04-23T13:00:00-07:00","endsAt":"2026-04-23T13:30:00-07:00","location":"Desk"}"#),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let external_event_id = created["event"]["externalEventId"].as_str().expect("event id").to_string();
        assert!(!external_event_id.is_empty());

        let (status, updated) = request_json(
            &app,
            "POST",
            &format!("/v1/calendar/events/{external_event_id}/update"),
            Some(r#"{"title":"Phase 29 moved","startsAt":"2026-04-23T14:00:00-07:00","endsAt":"2026-04-23T14:30:00-07:00"}"#),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(updated["event"]["externalEventId"], external_event_id);

        let (status, cancelled) = request_json(
            &app,
            "POST",
            &format!("/v1/calendar/events/{external_event_id}/cancel"),
            Some("{}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(cancelled["event"]["externalEventId"], external_event_id);
        assert_eq!(cancelled["event"]["lifecycleState"], "cancelled");

        // Roadmap 62: all-day creates are accepted with date boundaries.
        let (status, all_day) = request_json(
            &app,
            "POST",
            "/v1/calendar/events",
            Some(r#"{"title":"All day event","startsAt":"2026-04-24T00:00:00Z","endsAt":"2026-04-25T00:00:00Z","allDay":true,"startDate":"2026-04-24","endDate":"2026-04-25"}"#),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(all_day["event"]["allDay"], true);

        // Roadmap 61: attendee-bearing updates are accepted and record an outcome.
        let (status, attendee_update) = request_json(
            &app,
            "POST",
            &format!("/v1/calendar/events/{external_event_id}/update"),
            Some(r#"{"title":"Attendee update","startsAt":"2026-04-23T15:00:00-07:00","endsAt":"2026-04-23T15:30:00-07:00","attendees":[{"email":"bob@example.com","role":"required"}],"notifyAttendees":true}"#),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let body = serde_json::to_string(&attendee_update).expect("serialize");
        assert!(body.contains("bob@example.com"));
        assert!(body.contains("attendeeOutcome"));

        // Alternate-calendar cancel is rejected with 400.
        let (status, alternate) = request_json(
            &app,
            "POST",
            &format!("/v1/calendar/events/{external_event_id}/cancel"),
            Some(r#"{"calendarRef":"secondary"}"#),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            alternate["error"].as_str().unwrap_or("").contains("alternate-calendar"),
            "expected alternate-calendar denial, got {alternate}"
        );

        // Create/update/cancel artifacts were persisted.
        let artifacts = state
            .store
            .lock()
            .list_calendar_artifacts("test", "")
            .expect("list artifacts");
        assert!(artifacts.len() >= 3, "expected persisted create/update/cancel artifacts");

        assert_eq!(created["operation"]["timezoneUsed"], "America/Los_Angeles");
    }

    #[tokio::test]
    async fn validation_rejects_bad_timestamps_and_ranges() {
        let state = test_state();
        seed_healthy_calendar_integration(&state, "calendar-a", true);
        let app = router().with_state(state);

        let (status, json) = request_json(
            &app,
            "POST",
            "/v1/calendar/events",
            Some(r#"{"title":"Bad","startsAt":"not-a-time","endsAt":"2026-04-23T14:00:00Z"}"#),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json["error"], "startsAt must be RFC3339");

        let (status, json) = request_json(
            &app,
            "POST",
            "/v1/calendar/events",
            Some(r#"{"title":"Inverted","startsAt":"2026-04-23T14:00:00Z","endsAt":"2026-04-23T13:00:00Z"}"#),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json["error"], "invalid calendar time range");

        let (status, json) = request_json(&app, "GET", "/v1/calendar/events?startsAt=bogus", None, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json["error"], "startsAt must be RFC3339");

        let (status, json) = request_json(&app, "POST", "/v1/calendar/events", Some(""), None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json["error"], "request body is required");
    }

    #[tokio::test]
    async fn missing_entities_are_404() {
        let state = test_state();
        seed_healthy_calendar_integration(&state, "calendar-a", true);
        let app = router().with_state(state);

        let (status, _) = request_json(&app, "GET", "/v1/calendar/events/unknown-event", None, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, _) = request_json(&app, "GET", "/v1/calendar/operations/unknown-op", None, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, _) = request_json(&app, "GET", "/v1/calendar/availability/queries/unknown-query", None, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, _) = request_json(&app, "GET", "/v1/calendar/accounts/calendar-unknown", None, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn missing_managers_are_500() {
        let mut state = test_state();
        state.calendar = None;
        state.integrations = None;
        let app = router().with_state(state);

        let (status, json) = request_json(&app, "GET", "/v1/calendar/accounts", None, None).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(json["error"], "calendar dependencies are not configured");

        let (status, json) = request_json(&app, "GET", "/v1/calendar/operations", None, None).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(json["error"], "calendar manager is not configured");
    }

    #[tokio::test]
    async fn operations_list_supports_filters() {
        let state = test_state();
        seed_healthy_calendar_integration(&state, "calendar-a", true);
        let app = router().with_state(state.clone());

        let (status, _created) = request_json(
            &app,
            "POST",
            "/v1/calendar/events",
            Some(r#"{"integrationId":"calendar-a","title":"Ledger event","startsAt":"2026-04-23T10:00:00Z","endsAt":"2026-04-23T10:30:00Z"}"#),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);

        let (status, json) = request_json(&app, "GET", "/v1/calendar/operations?integrationId=calendar-a", None, None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(!json["items"].as_array().unwrap().is_empty());

        let (status, json) = request_json(&app, "GET", "/v1/calendar/operations?operationClass=create_event", None, None).await;
        assert_eq!(status, StatusCode::OK);
        let items = json["items"].as_array().unwrap();
        assert!(!items.is_empty());
        assert!(items.iter().all(|item| item["operationClass"] == "create_event"));

        // Unknown enum values match nothing (Go direct-cast semantics).
        let (status, json) = request_json(&app, "GET", "/v1/calendar/operations?operationClass=bogus", None, None).await;
        assert_eq!(status, StatusCode::OK);
        // ListResponse omits an empty items array (omitempty), so the body is
        // either `{}` or an explicit empty list.
        let items = json.get("items").and_then(|v| v.as_array());
        assert!(items.map_or(true, |v| v.is_empty()), "expected no items for unknown class, got {json}");
    }

    #[tokio::test]
    async fn tenant_isolation_hides_cross_tenant_rows() {
        let state = test_state();
        seed_healthy_calendar_integration(&state, "calendar-a", true);
        let app = router().with_state(state.clone());

        // Seed a store row owned by tenant-a and guard the accounts route.
        // The guard keys on calendar_account_id; the integration_id must not
        // collide with the account the handler itself persists (the table has
        // a UNIQUE constraint on integration_id).
        let account = calendar::AccountProjection {
            calendar_account_id: "calendar-a".to_string(),
            integration_id: "seed-tenant-a".to_string(),
            domain_kind: "calendar".to_string(),
            environment_scope: "test".to_string(),
            readiness_status: "healthy".to_string(),
            primary_calendar_ref: "primary".to_string(),
            primary_timezone: "America/Los_Angeles".to_string(),
            ..Default::default()
        };
        state
            .store
            .lock()
            .upsert_calendar_account(&account)
            .expect("upsert account");
        state
            .store
            .lock()
            .bind_row_tenant("calendar_accounts", "calendar_account_id", "calendar-a", "tenant-a")
            .expect("bind tenant");

        let (status, _) = request_json(&app, "GET", "/v1/calendar/accounts/calendar-a", None, Some("tenant-b")).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "cross-tenant account access must be 404");
        let (status, json) = request_json(&app, "GET", "/v1/calendar/accounts/calendar-a", None, Some("tenant-a")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["integrationId"], "calendar-a");

        // Create an event so the operation ledger row exists, bind it to
        // tenant-a, and guard the operations route.
        let (status, created) = request_json(
            &app,
            "POST",
            "/v1/calendar/events",
            Some(r#"{"integrationId":"calendar-a","title":"Tenant event","startsAt":"2026-04-23T10:00:00Z","endsAt":"2026-04-23T10:30:00Z"}"#),
            Some("tenant-a"),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let operation_id = created["operation"]["operationId"].as_str().expect("operation id").to_string();
        state
            .store
            .lock()
            .bind_row_tenant("calendar_operations", "operation_id", &operation_id, "tenant-a")
            .expect("bind operation tenant");

        let (status, _) = request_json(&app, "GET", &format!("/v1/calendar/operations/{operation_id}"), None, Some("tenant-b")).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "cross-tenant operation access must be 404");
        let (status, json) = request_json(&app, "GET", &format!("/v1/calendar/operations/{operation_id}"), None, Some("tenant-a")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["operation"]["operationId"], operation_id);
    }
}
