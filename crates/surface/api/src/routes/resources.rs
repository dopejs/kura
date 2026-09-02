//! resources route family (port of daemon/internal/api/thread_lifecycle.go,
//! thread_handoff.go and agent_profiles.go).
//!
//! The Go surface mounts these route families under /v1:
//! - `/v1/threads` + `/v1/threads/` — thread list, detail, lifecycle actions
//!   (reset/archive/reopen), handoff creation, continuity-preview detail
//! - `/v1/profiles` + `/v1/profiles/` — agent profile CRUD + lifecycle
//!
//! The workspace / binding / capability-visibility families are ported in
//! workspace_bindings.rs (Go workspace_bindings.go) so the two routers do not
//! overlap.
//!
//! Port status:
//! - The thread list/detail/lifecycle surface is fully ported on the kura-store
//!   thread DAOs (`crates/persistence/store`), the kura-threads domain
//!   types and the kura-events thread event builders. Status codes, DTOs,
//!   validation (empty body -> 400, unknown action -> 404, missing row -> 404,
//!   transition conflicts -> 409) and the tenant-scoped permission gates
//!   (credentials.inspect for reads, connectors.manage for mutations) mirror the
//!   Go handlers.
//! - Thread detail additionally attaches the latest runtime binding evidence as
//!   an additive `bindingProjection` field for callers holding bindings.inspect
//!   (FR-013, SC-012; Go writeThreadDetailWithBindingProjection) on the new
//!   kura-store binding evidence DAO.
//! - POST /v1/threads/{id}/handoffs and
//!   GET /v1/threads/{id}/continuity-previews/{preview_id} are fully ported on
//!   the kura-store thread_handoff + thread_continuity DAOs, the kura-threads
//!   handoff/continuity domain (validate_handoff /
//!   build_handoff_source_references / resolve_conversation_shape) and the
//!   active-profile projection DAOs (kura-store profiles.rs).
//! - The agent profile family is fully ported on the kura-store profile DAOs
//!   (list/create/get/update/activate/versions/rollback/retire) with the
//!   kura-events profile lifecycle + version-created events.

use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json as AxumJson, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use kura_connectors as connectors;
use kura_events as events;
use kura_identity::{has_permission, Permission};
use kura_profiles as profiles;
use kura_threads as threads;

use crate::error::ApiError;
use crate::middleware::TenantContext;
use crate::response::Json;
use crate::state::AppState;

/// Route family router. Only the methods the Go handlers accept are
/// registered; axum answers the other methods with 405 (Go
/// w.WriteHeader(http.StatusMethodNotAllowed)). The workspace / binding /
/// capability-visibility families live in workspace_bindings.rs.
#[must_use]
pub fn router() -> Router<AppState> {
    Router::new()
        // Threads
        .route("/v1/threads", get(list_threads))
        .route("/v1/threads/{thread_id}", get(thread_detail))
        .route(
            "/v1/threads/{thread_id}/handoffs",
            post(thread_handoff_create),
        )
        .route(
            "/v1/threads/{thread_id}/continuity-previews/{preview_id}",
            get(thread_continuity_preview_detail),
        )
        .route("/v1/threads/{thread_id}/reset", post(thread_reset))
        .route("/v1/threads/{thread_id}/archive", post(thread_archive))
        .route("/v1/threads/{thread_id}/reopen", post(thread_reopen))
        // Agent profiles
        .route("/v1/profiles", get(list_profiles).post(create_profile))
        .route(
            "/v1/profiles/{profile_id}",
            get(get_profile).patch(update_profile),
        )
        .route("/v1/profiles/{profile_id}/activate", post(activate_profile))
        .route(
            "/v1/profiles/{profile_id}/versions",
            get(list_profile_versions),
        )
        .route("/v1/profiles/{profile_id}/rollback", post(rollback_profile))
        .route("/v1/profiles/{profile_id}/archive", post(archive_profile))
        .route("/v1/profiles/{profile_id}/disable", post(disable_profile))
}

// ---------------------------------------------------------------------------
// Request/response DTOs (local ports of the Go api-package types)
// ---------------------------------------------------------------------------

/// Go handleThreadList query params (limit/cursor/state/sourceKind).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadListQuery {
    #[serde(default)]
    limit: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    source_kind: Option<String>,
}

/// Go `threadLifecycleActionRequest` (Go decodes note but never uses it;
/// kept for wire parity).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadLifecycleActionRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    reason_code: String,
    #[allow(dead_code)]
    #[serde(default, skip_serializing_if = "String::is_empty")]
    note: String,
}

/// Go `threadLifecycleActionResponse`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ThreadLifecycleActionResponse {
    thread_id: String,
    lifecycle_state: kura_threads::LifecycleState,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    previous_session_segment_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    current_session_segment_id: String,
    audit_event_id: String,
    changed_at: DateTime<Utc>,
    action: kura_threads::LifecycleActionKind,
    available_actions: Vec<kura_threads::LifecycleActionKind>,
}

/// Go threadHandoffDestinationRequest.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadHandoffDestinationRequest {
    surface: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    connector_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    source_account_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    source_conversation_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    conversation_shape: String,
}

/// Go threadHandoffRequest.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadHandoffRequest {
    destination: ThreadHandoffDestinationRequest,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    reason_code: String,
}

// ---------------------------------------------------------------------------
// Threads: list / detail / lifecycle actions
// ---------------------------------------------------------------------------

/// GET /v1/threads — tenant-scoped thread list with limit/cursor/state/
/// sourceKind filters (Go handleThreadList).
async fn list_threads(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Query(query): Query<ThreadListQuery>,
) -> Result<Json<kura_threads::ThreadListResponse>, ApiError> {
    let tc = require_thread_permission(tenant.as_ref().map(|e| &e.0), Permission::CredentialsInspect)?;
    // Go parseThreadLifecycleLimit: unparseable/zero limits default to 20 (the
    // store applies that default).
    let limit = query
        .limit
        .as_deref()
        .and_then(|raw| raw.parse::<i64>().ok())
        .unwrap_or(0);
    let store_query = kura_store::ThreadListQuery {
        tenant_id: tc.tenant_id.clone(),
        limit,
        cursor: query.cursor.unwrap_or_default(),
        state_filter: query.state.unwrap_or_default(),
        source_filter: query.source_kind.unwrap_or_default(),
    };
    let response = state
        .store
        .lock()
        .list_threads_for_tenant(&store_query)
        .map_err(ApiError::from_store)?;
    Ok(Json(response))
}

/// GET /v1/threads/{thread_id} — full operator detail view (Go
/// handleThreadDetail). Callers holding bindings.inspect additionally get the
/// latest runtime binding evidence as an additive `bindingProjection` field
/// (Go writeThreadDetailWithBindingProjection, FR-013/SC-012).
async fn thread_detail(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(thread_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let tc = require_thread_permission(tenant.as_ref().map(|e| &e.0), Permission::CredentialsInspect)?;
    let mut response = state
        .store
        .lock()
        .get_thread_detail_for_tenant(&tc.tenant_id, &thread_id)
        .map_err(ApiError::from_store)?
        .ok_or_else(|| ApiError::NotFound("not found".to_string()))?;
    // Go canInspectProfileRuntime: without profiles.inspect the active-profile
    // projections are stripped from the detail and its handoff links.
    if !can_inspect_profile_runtime(tenant.as_ref().map(|e| &e.0)) {
        response.active_profile_projection = None;
        for link in &mut response.handoff_links {
            link.active_profile_projection = None;
        }
    }
    write_thread_detail_with_binding_projection(&state, tc, &thread_id, response)
}

/// POST /v1/threads/{thread_id}/reset — reset lifecycle mutation.
async fn thread_reset(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(thread_id): Path<String>,
    body: Bytes,
) -> Result<Json<ThreadLifecycleActionResponse>, ApiError> {
    thread_lifecycle_action(state, tenant, thread_id, kura_threads::LifecycleActionKind::Reset, body).await
}

/// POST /v1/threads/{thread_id}/archive — archive lifecycle mutation.
async fn thread_archive(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(thread_id): Path<String>,
    body: Bytes,
) -> Result<Json<ThreadLifecycleActionResponse>, ApiError> {
    thread_lifecycle_action(state, tenant, thread_id, kura_threads::LifecycleActionKind::Archive, body).await
}

/// POST /v1/threads/{thread_id}/reopen — reopen lifecycle mutation.
async fn thread_reopen(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(thread_id): Path<String>,
    body: Bytes,
) -> Result<Json<ThreadLifecycleActionResponse>, ApiError> {
    thread_lifecycle_action(state, tenant, thread_id, kura_threads::LifecycleActionKind::Reopen, body).await
}

/// Shared body of the three lifecycle mutations (Go
/// handleThreadLifecycleAction): apply the mutation with audit evidence,
/// publish the lifecycle event (plus the scoped-reset evidence event for
/// resets), and answer the threadLifecycleActionResponse.
async fn thread_lifecycle_action(
    state: AppState,
    tenant: Option<Extension<TenantContext>>,
    thread_id: String,
    kind: kura_threads::LifecycleActionKind,
    body: Bytes,
) -> Result<Json<ThreadLifecycleActionResponse>, ApiError> {
    let tc = require_thread_permission(tenant.as_ref().map(|e| &e.0), Permission::ConnectorsManage)?;
    // Go decodeJSONBody: empty body -> 400 "request body is required".
    let input: ThreadLifecycleActionRequest = decode_json_body(&body)?;
    let now = Utc::now();
    let audit_event_id = format!(
        "audit_thread_{}_{}",
        lifecycle_action_kind_str(kind),
        now.timestamp_nanos_opt().unwrap_or_default()
    );
    let mutation_input = kura_threads::LifecycleMutationInput {
        actor_principal_id: tc.principal_id.clone(),
        reason_code: coalesce_reason(&input.reason_code, lifecycle_action_kind_str(kind)),
        audit_event_id: audit_event_id.clone(),
        now: Some(now),
        new_segment_id: String::new(),
    };
    let result = state
        .store
        .lock()
        .apply_thread_lifecycle_action(&tc.tenant_id, &thread_id, kind, &mutation_input)
        .map_err(|message| {
            // Go: ThreadAuditFailedClosedEvent for audit-evidence and
            // concurrent-mutation failures, before the error response.
            if is_audit_failed_closed(&message) {
                let _ = publish_thread_event(
                    &state,
                    &tc.tenant_id,
                    events::thread_audit_failed_closed_event(&tc.tenant_id, &thread_id, &message),
                );
            }
            map_thread_lifecycle_error(message)
        })?;
    let Some(result) = result else {
        return Err(ApiError::NotFound("not found".to_string()));
    };
    publish_thread_event(&state, &tc.tenant_id, events::thread_lifecycle_event(result.action.clone()))?;
    if kind == kura_threads::LifecycleActionKind::Reset {
        // Go: ListResetEventsForThread(limit 1) -> ThreadScopedResetEvidenceEvent.
        let reset = state
            .store
            .lock()
            .list_reset_events_for_thread(&tc.tenant_id, &thread_id, 1)
            .map_err(ApiError::from_store)?
            .into_iter()
            .next();
        if let Some(reset) = reset {
            publish_thread_event(&state, &tc.tenant_id, events::thread_scoped_reset_evidence_event(reset))?;
        }
    }
    Ok(Json(ThreadLifecycleActionResponse {
        thread_id: result.thread.thread_id.clone(),
        lifecycle_state: result.thread.lifecycle_state,
        previous_session_segment_id: result.action.prior_session_segment_id.clone(),
        current_session_segment_id: result.thread.current_session_segment_id.clone(),
        audit_event_id: result.action.audit_event_id.clone(),
        changed_at: result.action.completed_at,
        action: kind,
        available_actions: kura_threads::available_actions(result.thread.lifecycle_state),
    }))
}

/// POST /v1/threads/{thread_id}/handoffs — create a thread handoff (201).
/// Full port of Go handleThreadHandoffCreate: validates the source thread and
/// its proven conversation shape, ensures the destination thread (web or
/// channel), persists the handoff link plus its continuity source references,
/// records the destination active-profile projection, and publishes the
/// thread.handoff_linked event.
async fn thread_handoff_create(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(thread_id): Path<String>,
    body: Bytes,
) -> Result<(StatusCode, AxumJson<threads::HandoffLink>), ApiError> {
    let tc = require_thread_permission(tenant.as_ref().map(|e| &e.0), Permission::ConnectorsManage)?;
    let input: ThreadHandoffRequest = decode_json_body(&body)?;
    let now = Utc::now();
    let link = create_thread_handoff(&state, tc, &thread_id, &input, now)?;
    // Go recordActiveProfileProjectionForTarget: anchor the active profile
    // selection to the handoff destination and re-save the link with the
    // projection attached.
    let projection = record_active_profile_projection_for_target(
        &state,
        tc,
        profiles::RuntimeResourceKind::HANDOFF_DESTINATION,
        &link.destination_thread_id,
        &link.destination_thread_id,
        &link.destination_session_segment_id,
        &link.handoff_link_id,
    )?;
    let mut link = link;
    if let Some(projection) = projection {
        link.active_profile_projection = Some(projection.clone());
        link = state
            .store
            .lock()
            .save_handoff_link(link)
            .map_err(ApiError::from_store)?;
    }
    publish_thread_event(
        &state,
        &tc.tenant_id,
        events::thread_handoff_linked_event(link.clone()),
    )?;
    if !can_inspect_profile_runtime(tenant.as_ref().map(|e| &e.0)) {
        link.active_profile_projection = None;
    }
    Ok((StatusCode::CREATED, AxumJson(link)))
}

/// GET /v1/threads/{thread_id}/continuity-previews/{preview_id} — continuity
/// preview detail (Go handleThreadContinuityPreviewDetail) on the store
/// get_continuity_preview_detail DAO.
async fn thread_continuity_preview_detail(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path((thread_id, preview_id)): Path<(String, String)>,
) -> Result<Json<threads::ContinuityPreviewDetail>, ApiError> {
    let tc = require_thread_permission(tenant.as_ref().map(|e| &e.0), Permission::CredentialsInspect)?;
    let detail = state
        .store
        .lock()
        .get_continuity_preview_detail(&tc.tenant_id, &thread_id, &preview_id)
        .map_err(ApiError::from_store)?
        .ok_or_else(|| ApiError::NotFound("not found".to_string()))?;
    Ok(Json(detail))
}

// --- thread handoff helpers (Go createThreadHandoff + friends) ---

/// Go createThreadHandoff: loads the source thread + proven shape, resolves
/// source/destination eligibility, validates, and persists the link with its
/// continuity source references.
fn create_thread_handoff(
    state: &AppState,
    tc: &kura_identity::TenantContext,
    source_thread_id: &str,
    input: &ThreadHandoffRequest,
    now: DateTime<Utc>,
) -> Result<threads::HandoffLink, ApiError> {
    let source_thread = state
        .store
        .lock()
        .get_thread_for_tenant(&tc.tenant_id, source_thread_id)
        .map_err(ApiError::from_store)?
        .ok_or_else(|| ApiError::NotFound("thread not found".to_string()))?;
    let source_shape = state
        .store
        .lock()
        .get_conversation_shape_for_thread(&tc.tenant_id, source_thread_id)
        .map_err(ApiError::from_store)?;
    let Some(source_shape) = source_shape else {
        return Err(handoff_error(threads::ThreadsError::HandoffNotEligible));
    };
    if source_shape.shape_evidence_status != threads::ShapeEvidenceStatus::Proven
        || source_shape.shape == threads::ConversationShape::Unknown
        || source_shape.shape == threads::ConversationShape::Unsupported
    {
        return Err(handoff_error(threads::ThreadsError::HandoffNotEligible));
    }
    let (source_eligible, source_permission_allowed) =
        validate_handoff_source(state, tc, &source_thread, &source_shape)?;
    let (destination_thread, destination_shape, destination_permission_allowed) =
        ensure_handoff_destination_thread(state, tc, &input.destination, now)?;
    let link = threads::HandoffLink {
        handoff_link_id: String::new(),
        tenant_id: tc.tenant_id.clone(),
        source_thread_id: source_thread.thread_id.clone(),
        source_session_segment_id: source_thread.current_session_segment_id.clone(),
        destination_thread_id: destination_thread.thread_id.clone(),
        destination_session_segment_id: destination_thread.current_session_segment_id.clone(),
        source_conversation_shape: source_shape.shape,
        destination_conversation_shape: destination_shape.shape,
        source_kind: Some(source_thread.source_kind),
        destination_kind: Some(destination_thread.source_kind),
        source_connector_id: source_shape.connector_id.clone(),
        destination_connector_id: destination_shape.connector_id.clone(),
        source_conversation_id: source_shape.source_conversation_id.clone(),
        destination_conversation_id: destination_shape.source_conversation_id.clone(),
        actor_principal_id: tc.principal_id.clone(),
        permission_gate: "connectors.manage".to_string(),
        status: threads::HandoffStatus::Succeeded,
        reason_code: coalesce_reason(&input.reason_code, "user_requested_handoff"),
        first_destination_response_id: String::new(),
        source_reference_status: threads::HandoffSourceReferenceStatus::Available,
        active_profile_projection: None,
        created_at: Some(now),
        consumed_at: None,
        retention_expires_at: None,
        redaction_status: threads::RedactionStatus::Redacted,
    };
    threads::validate_handoff(&threads::HandoffValidationInput {
        link: link.clone(),
        has_mutation_permission: true,
        source_eligible,
        destination_eligible: destination_shape.shape_evidence_status == threads::ShapeEvidenceStatus::Proven,
        source_permission_allowed,
        destination_permission_allowed,
    })
    .map_err(handoff_error)?;
    let mut saved = state
        .store
        .lock()
        .save_handoff_link(link.clone())
        .map_err(ApiError::from_store)?;
    let turns = state
        .store
        .lock()
        .list_continuity_turns(&kura_store::thread_continuity::ContinuityLookupQuery {
            tenant_id: tc.tenant_id.clone(),
            thread_id: source_thread.thread_id.clone(),
            session_segment_id: source_thread.current_session_segment_id.clone(),
            limit: 0,
            now: Some(now),
        })
        .map_err(ApiError::from_store)?;
    let mut refs = threads::build_handoff_source_references(&saved, &turns, Some(now));
    if refs.is_empty() {
        saved.source_reference_status = threads::HandoffSourceReferenceStatus::None;
        return state
            .store
            .lock()
            .save_handoff_link(saved)
            .map_err(ApiError::from_store);
    }
    state
        .store
        .lock()
        .save_handoff_source_references(&mut refs)
        .map_err(ApiError::from_store)?;
    Ok(saved)
}

/// Go ensureHandoffDestinationThread: web surfaces create a fresh shell thread;
/// channel surfaces resolve (or create) the current thread for the normalized
/// source continuation key and record a proven shape.
fn ensure_handoff_destination_thread(
    state: &AppState,
    tc: &kura_identity::TenantContext,
    destination: &ThreadHandoffDestinationRequest,
    now: DateTime<Utc>,
) -> Result<(threads::Thread, threads::ConversationShapeEvidence, bool), ApiError> {
    match destination.surface.trim() {
        "web" => {
            let thread_id = format!(
                "thr_handoff_web_{}",
                short_handoff_hash(&format!("{}:{}", tc.tenant_id, rfc3339_nano(now)))
            );
            let segment_id = format!("seg_{thread_id}");
            let retention = state
                .store
                .lock()
                .thread_retention_expiry(&tc.tenant_id, now)
                .map_err(ApiError::from_store)?;
            let thread = threads::Thread {
                thread_id: thread_id.clone(),
                tenant_id: tc.tenant_id.clone(),
                lifecycle_state: threads::LifecycleState::Active,
                current_session_segment_id: segment_id.clone(),
                source_kind: threads::SourceKind::Shell,
                source_summary: "Web handoff destination".to_string(),
                last_activity_at: now,
                created_at: now,
                updated_at: now,
                retention_expires_at: Some(retention),
                redaction_status: threads::RedactionStatus::Redacted,
            };
            state.store.lock().upsert_thread(&thread).map_err(ApiError::from_store)?;
            state
                .store
                .lock()
                .upsert_thread_session_segment(&threads::SessionSegment {
                    session_segment_id: segment_id.clone(),
                    thread_id: thread_id.clone(),
                    tenant_id: tc.tenant_id.clone(),
                    session_id: String::new(),
                    generation: 1,
                    state: "active".to_string(),
                    started_at: now,
                    ended_at: None,
                    last_active_at: now,
                    reset_from_session_segment_id: String::new(),
                    partial_evidence: false,
                })
                .map_err(ApiError::from_store)?;
            let shape = threads::resolve_conversation_shape(&threads::ConversationShapeResolutionInput {
                tenant_id: tc.tenant_id.clone(),
                thread_id: thread_id.clone(),
                session_segment_id: segment_id.clone(),
                source_kind: threads::SourceKind::Shell,
                connector_id: String::new(),
                connector_kind: String::new(),
                source_account_id: String::new(),
                source_conversation_id: String::new(),
                source_conversation_summary: "Web handoff destination".to_string(),
                claimed_shape: Some(threads::ConversationShape::Web),
                now: Some(now),
            });
            state
                .store
                .lock()
                .save_conversation_shape_evidence(&shape)
                .map_err(ApiError::from_store)?;
            Ok((thread, shape, true))
        }
        "channel" => {
            let shape_value = threads::normalize_conversation_shape(&destination.conversation_shape)
                .map_err(|_| handoff_error(threads::ThreadsError::HandoffNotEligible))?;
            if shape_value != threads::ConversationShape::Group
                && shape_value != threads::ConversationShape::Room
                && shape_value != threads::ConversationShape::DirectMessage
            {
                return Err(handoff_error(threads::ThreadsError::HandoffNotEligible));
            }
            let (destination_eligible, destination_permission_allowed) = validate_channel_handoff_endpoint(
                state,
                tc,
                &destination.connector_id,
                &destination.source_conversation_id,
                connectors::HANDOFF_SURFACE_DESTINATION_SUPPORT,
            )?;
            if !destination_permission_allowed {
                return Err(handoff_error(threads::ThreadsError::HandoffPermissionDenied));
            }
            if !destination_eligible {
                return Err(handoff_error(threads::ThreadsError::HandoffNotEligible));
            }
            let destination_connector = find_connector_for_tenant(state, &tc.tenant_id, &destination.connector_id)?
                .ok_or_else(|| handoff_error(threads::ThreadsError::HandoffNotEligible))?;
            let key = threads::normalize_source_continuation_key(&threads::SourceContinuationKey {
                tenant_id: tc.tenant_id.clone(),
                connector_id: destination.connector_id.clone(),
                source_account_id: destination.source_account_id.clone(),
                source_conversation_id: destination.source_conversation_id.clone(),
            })
            .map_err(|_| handoff_error(threads::ThreadsError::HandoffNotEligible))?;
            let current = state
                .store
                .lock()
                .get_current_thread_for_source(&key)
                .map_err(ApiError::from_store)?;
            let current = match current {
                Some(thread) => thread,
                None => {
                    let thread_id = format!(
                        "thr_handoff_channel_{}",
                        short_handoff_hash(&key.to_string())
                    );
                    let segment_id = format!("seg_{thread_id}");
                    let retention = state
                        .store
                        .lock()
                        .thread_retention_expiry(&tc.tenant_id, now)
                        .map_err(ApiError::from_store)?;
                    let thread = threads::Thread {
                        thread_id: thread_id.clone(),
                        tenant_id: tc.tenant_id.clone(),
                        lifecycle_state: threads::LifecycleState::Active,
                        current_session_segment_id: segment_id.clone(),
                        source_kind: threads::SourceKind::Channel,
                        source_summary: format!(
                            "{} / {}",
                            destination.connector_id, destination.source_conversation_id
                        ),
                        last_activity_at: now,
                        created_at: now,
                        updated_at: now,
                        retention_expires_at: Some(retention),
                        redaction_status: threads::RedactionStatus::Redacted,
                    };
                    state.store.lock().upsert_thread(&thread).map_err(ApiError::from_store)?;
                    state
                        .store
                        .lock()
                        .upsert_thread_session_segment(&threads::SessionSegment {
                            session_segment_id: segment_id.clone(),
                            thread_id: thread_id.clone(),
                            tenant_id: tc.tenant_id.clone(),
                            session_id: String::new(),
                            generation: 1,
                            state: "active".to_string(),
                            started_at: now,
                            ended_at: None,
                            last_active_at: now,
                            reset_from_session_segment_id: String::new(),
                            partial_evidence: false,
                        })
                        .map_err(ApiError::from_store)?;
                    thread
                }
            };
            let shape = threads::resolve_conversation_shape(&threads::ConversationShapeResolutionInput {
                tenant_id: tc.tenant_id.clone(),
                thread_id: current.thread_id.clone(),
                session_segment_id: current.current_session_segment_id.clone(),
                source_kind: threads::SourceKind::Channel,
                connector_id: destination.connector_id.clone(),
                connector_kind: destination_connector.kind.clone(),
                source_account_id: destination.source_account_id.clone(),
                source_conversation_id: destination.source_conversation_id.clone(),
                source_conversation_summary: current.source_summary.clone(),
                claimed_shape: Some(shape_value),
                now: Some(now),
            });
            state
                .store
                .lock()
                .save_conversation_shape_evidence(&shape)
                .map_err(ApiError::from_store)?;
            Ok((current, shape, destination_permission_allowed))
        }
        _ => Err(handoff_error(threads::ThreadsError::HandoffNotEligible)),
    }
}

/// Go validateHandoffSource.
fn validate_handoff_source(
    state: &AppState,
    tc: &kura_identity::TenantContext,
    source_thread: &threads::Thread,
    source_shape: &threads::ConversationShapeEvidence,
) -> Result<(bool, bool), ApiError> {
    if source_thread.lifecycle_state == threads::LifecycleState::Archived {
        return Ok((false, true));
    }
    if source_shape.source_kind != Some(threads::SourceKind::Channel) {
        return Ok((true, true));
    }
    validate_channel_handoff_endpoint(
        state,
        tc,
        &source_shape.connector_id,
        &source_shape.source_conversation_id,
        connectors::HANDOFF_SURFACE_SOURCE_SUPPORT,
    )
}

/// Go validateChannelHandoffEndpoint: connector must exist, be healthy, and
/// either declare handoff support or allow the conversation through its route
/// policy.
fn validate_channel_handoff_endpoint(
    state: &AppState,
    tc: &kura_identity::TenantContext,
    connector_id: &str,
    source_conversation_id: &str,
    capability_key: &str,
) -> Result<(bool, bool), ApiError> {
    let Some(connector) = find_connector_for_tenant(state, &tc.tenant_id, connector_id)? else {
        return Ok((false, false));
    };
    if connector.status == connectors::Status::Disabled
        || connector.status == connectors::Status::Failed
        || connector.status == connectors::Status::BackingOff
    {
        return Ok((false, false));
    }
    if connector_handoff_capability_unsupported(&connector, capability_key) {
        return Ok((false, true));
    }
    let policy = state
        .store
        .lock()
        .get_channel_route_policy(&tc.tenant_id, connector_id)
        .map_err(ApiError::from_store)?;
    match policy {
        Some(policy) if connectors::route_policy_is_valid(&policy) => Ok((
            connectors::route_policy_allows_conversation(&policy, source_conversation_id),
            true,
        )),
        _ => Ok((false, false)),
    }
}

/// Go connectorHandoffCapabilityUnsupported: an explicit capability value that
/// is neither supported nor limited means the endpoint cannot hand off.
fn connector_handoff_capability_unsupported(
    connector: &kura_connectors::Connector,
    capability_key: &str,
) -> bool {
    if connector.capability_profile.is_empty() || capability_key.trim().is_empty() {
        return false;
    }
    match connector.capability_profile.get(capability_key) {
        Some(value) => {
            let raw = value.as_str().unwrap_or_default().trim();
            raw != connectors::ConformanceResultStatus::Supported.as_str()
                && raw != connectors::ConformanceResultStatus::Limited.as_str()
        }
        None => false,
    }
}

/// Go findConnectorForTenant: first connector matching id whose tenant is
/// empty or the caller's tenant.
fn find_connector_for_tenant(
    state: &AppState,
    tenant_id: &str,
    connector_id: &str,
) -> Result<Option<kura_connectors::Connector>, ApiError> {
    let items = state.store.lock().list_connectors().map_err(ApiError::from_store)?;
    for item in items {
        if item.connector_id.trim() != connector_id.trim() {
            continue;
        }
        if !item.tenant_id.trim().is_empty() && item.tenant_id.trim() != tenant_id.trim() {
            continue;
        }
        return Ok(Some(item));
    }
    Ok(None)
}

/// Go handleThreadHandoffError: permission failures answer the stable 403
/// denial; same-thread / ineligible destinations answer 409; missing threads
/// answer 404 (handled by the caller); anything else is 500.
fn handoff_error(err: threads::ThreadsError) -> ApiError {
    match err {
        threads::ThreadsError::HandoffPermissionDenied => credential_denial(),
        threads::ThreadsError::HandoffSameThread | threads::ThreadsError::HandoffNotEligible => {
            ApiError::Conflict(err.to_string())
        }
        other => ApiError::from_store(other.to_string()),
    }
}

/// Go shortHandoffHash: 24 hex chars. kura-api has no sha2 dependency, so the
/// uuid v4 hex (32 chars) is truncated to the same id shape; uniqueness comes
/// from the uuid, matching Go's intent.
fn short_handoff_hash(value: &str) -> String {
    let _ = value;
    let hex = uuid::Uuid::new_v4().simple().to_string();
    hex[..24].to_string()
}

/// Go RFC3339Nano timestamp (used for the web handoff id input).
fn rfc3339_nano(ts: DateTime<Utc>) -> String {
    ts.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
}

// ---------------------------------------------------------------------------
// Agent profiles (Go agent_profiles.go)
// ---------------------------------------------------------------------------

/// GET /v1/profiles — tenant profile list (Go handleListAgentProfiles).
async fn list_profiles(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<profiles::ListResponse>, ApiError> {
    let tc = require_profile_permission(
        &state,
        tenant.as_ref().map(|e| &e.0),
        Permission::ProfilesInspect,
        "agent_profile.inspect_denied",
        "",
    )?;
    let limit = parse_limit(params.get("limit"));
    let items = state
        .store
        .lock()
        .list_agent_profiles(&tc.tenant_id, limit)
        .map_err(ApiError::from_store)?;
    Ok(Json(items))
}

/// POST /v1/profiles — create a profile (201; Go handleCreateAgentProfile).
async fn create_profile(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<(StatusCode, AxumJson<profiles::MutationResult>), ApiError> {
    let tc = require_profile_permission(
        &state,
        tenant.as_ref().map(|e| &e.0),
        Permission::ProfilesManage,
        "agent_profile.create_denied",
        "",
    )?;
    let input: profiles::MutationInput = decode_json_body(&body)?;
    // Drop the store guard before the denied event publishes (parking_lot is
    // not reentrant).
    let created = {
        let store = state.store.lock();
        store.create_agent_profile(tc, &input)
    };
    let result = match created {
        Ok(result) => result,
        Err(message) => {
            publish_profile_denied_event(&state, tc, "", "agent_profile.validation_failed", &message)?;
            return Err(map_profile_error(message));
        }
    };
    publish_profile_mutation_events(
        &state,
        tc,
        &result,
        "agent_profile.created",
        "succeeded",
        &default_api_reason(&input.reason_code, "user_created_profile"),
    )?;
    Ok((StatusCode::CREATED, AxumJson(result)))
}

/// GET /v1/profiles/{profile_id} — profile detail with versions, overlays,
/// and audit events (Go handleGetAgentProfile).
async fn get_profile(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(profile_id): Path<String>,
) -> Result<Json<profiles::ProfileDetail>, ApiError> {
    let tc = require_profile_permission(
        &state,
        tenant.as_ref().map(|e| &e.0),
        Permission::ProfilesInspect,
        "agent_profile.inspect_denied",
        &profile_id,
    )?;
    let detail = state
        .store
        .lock()
        .get_agent_profile_detail(&tc.tenant_id, &profile_id)
        .map_err(ApiError::from_store)?
        .ok_or_else(|| ApiError::NotFound("profile not found".to_string()))?;
    Ok(Json(detail))
}

/// PATCH /v1/profiles/{profile_id} — update profile persona/identity
/// (Go handleUpdateAgentProfile).
async fn update_profile(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(profile_id): Path<String>,
    body: Bytes,
) -> Result<Json<profiles::MutationResult>, ApiError> {
    let tc = require_profile_permission(
        &state,
        tenant.as_ref().map(|e| &e.0),
        Permission::ProfilesManage,
        "agent_profile.update_denied",
        &profile_id,
    )?;
    let input: profiles::MutationInput = decode_json_body(&body)?;
    let updated = {
        let store = state.store.lock();
        store.update_agent_profile(tc, &profile_id, &input)
    };
    let result = match updated {
        Ok(result) => result,
        Err(message) => {
            publish_profile_denied_event(&state, tc, &profile_id, "agent_profile.validation_failed", &message)?;
            return Err(map_profile_error(message));
        }
    };
    publish_profile_mutation_events(
        &state,
        tc,
        &result,
        "agent_profile.updated",
        "succeeded",
        &default_api_reason(&input.reason_code, "user_updated_profile"),
    )?;
    Ok(Json(result))
}

/// POST /v1/profiles/{profile_id}/activate — set the tenant default
/// (Go handleActivateAgentProfile). The request body is optional.
async fn activate_profile(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(profile_id): Path<String>,
    body: Bytes,
) -> Result<Json<profiles::ActiveSelection>, ApiError> {
    let tc = require_profile_permission(
        &state,
        tenant.as_ref().map(|e| &e.0),
        Permission::ProfilesManage,
        "agent_profile.activate_denied",
        &profile_id,
    )?;
    let input: profiles::ActivationInput = decode_optional_json_body(&body)?;
    let activated = {
        let store = state.store.lock();
        store.activate_agent_profile(tc, &profile_id, &input)
    };
    let selection = match activated {
        Ok(selection) => selection,
        Err(message) => {
            publish_profile_denied_event(
                &state,
                tc,
                &profile_id,
                "agent_profile.activate_denied",
                &message,
            )?;
            return Err(map_profile_error(message));
        }
    };
    publish_thread_event(
        &state,
        &tc.tenant_id,
        events::agent_profile_lifecycle_event(events::AgentProfileLifecycleInput {
            tenant_id: tc.tenant_id.clone(),
            profile_id: selection.profile_id.clone(),
            profile_version_id: selection.profile_version_id.clone(),
            actor_principal_id: tc.principal_id.clone(),
            event_name: "agent_profile.activated".to_string(),
            outcome: "succeeded".to_string(),
            reason_code: default_api_reason(&input.reason_code, "user_selected_default"),
            permission_gate: "profiles.manage".to_string(),
            audit_event_id: selection.audit_event_id.clone(),
            redaction_status: profiles::RedactionStatus::REDACTED,
            ..Default::default()
        }),
    )?;
    Ok(Json(selection))
}

/// GET /v1/profiles/{profile_id}/versions — profile version history
/// (Go handleListAgentProfileVersions).
async fn list_profile_versions(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(profile_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<crate::types::ListResponse<profiles::ProfileVersion>>, ApiError> {
    let tc = require_profile_permission(
        &state,
        tenant.as_ref().map(|e| &e.0),
        Permission::ProfilesInspect,
        "agent_profile.inspect_denied",
        &profile_id,
    )?;
    let limit = parse_limit(params.get("limit"));
    let items = state
        .store
        .lock()
        .list_agent_profile_versions(&tc.tenant_id, &profile_id, limit)
        .map_err(ApiError::from_store)?;
    Ok(Json(crate::types::ListResponse { items }))
}

/// POST /v1/profiles/{profile_id}/rollback — revert to a source version
/// (Go handleRollbackAgentProfile).
async fn rollback_profile(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(profile_id): Path<String>,
    body: Bytes,
) -> Result<Json<profiles::MutationResult>, ApiError> {
    let tc = require_profile_permission(
        &state,
        tenant.as_ref().map(|e| &e.0),
        Permission::ProfilesManage,
        "agent_profile.rollback_denied",
        &profile_id,
    )?;
    let input: profiles::RollbackInput = decode_json_body(&body)?;
    let rolled_back = {
        let store = state.store.lock();
        store.rollback_agent_profile(tc, &profile_id, &input)
    };
    let result = match rolled_back {
        Ok(result) => result,
        Err(message) => {
            publish_profile_denied_event(
                &state,
                tc,
                &profile_id,
                "agent_profile.rollback_denied",
                &message,
            )?;
            return Err(map_profile_error(message));
        }
    };
    publish_profile_mutation_events(
        &state,
        tc,
        &result,
        "agent_profile.rolled_back",
        "succeeded",
        &default_api_reason(&input.reason_code, "operator_reverted_persona"),
    )?;
    Ok(Json(result))
}

/// POST /v1/profiles/{profile_id}/archive — retire with archived status
/// (Go handleRetireAgentProfile). The request body is optional.
async fn archive_profile(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(profile_id): Path<String>,
    body: Bytes,
) -> Result<Json<profiles::MutationResult>, ApiError> {
    retire_profile(&state, tenant, profile_id, profiles::Status::ARCHIVED, body).await
}

/// POST /v1/profiles/{profile_id}/disable — retire with disabled status
/// (Go handleRetireAgentProfile). The request body is optional.
async fn disable_profile(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(profile_id): Path<String>,
    body: Bytes,
) -> Result<Json<profiles::MutationResult>, ApiError> {
    retire_profile(&state, tenant, profile_id, profiles::Status::DISABLED, body).await
}

/// Shared retirement handler (Go handleRetireAgentProfile): retires the
/// profile, publishes the archived/disabled lifecycle + version events, and —
/// when the retired profile was the tenant default — the safe-default fallback
/// event.
async fn retire_profile(
    state: &AppState,
    tenant: Option<Extension<TenantContext>>,
    profile_id: String,
    status: profiles::Status,
    body: Bytes,
) -> Result<Json<profiles::MutationResult>, ApiError> {
    let tc = require_profile_permission(
        state,
        tenant.as_ref().map(|e| &e.0),
        Permission::ProfilesManage,
        "agent_profile.retirement_denied",
        &profile_id,
    )?;
    let input: profiles::RetirementInput = decode_optional_json_body(&body)?;
    let is_disabled = status == profiles::Status::DISABLED;
    let retired = {
        let store = state.store.lock();
        store.retire_agent_profile(tc, &profile_id, status, &input)
    };
    let result = match retired {
        Ok(result) => result,
        Err(message) => {
            publish_profile_denied_event(
                state,
                tc,
                &profile_id,
                "agent_profile.retirement_denied",
                &message,
            )?;
            return Err(map_profile_error(message));
        }
    };
    let event_name = if is_disabled {
        "agent_profile.disabled"
    } else {
        "agent_profile.archived"
    };
    publish_profile_mutation_events(
        state,
        tc,
        &result,
        event_name,
        "succeeded",
        &default_api_reason(&input.reason_code, "operator_retired_profile"),
    )?;
    if !result.selection.selection_id.is_empty() {
        publish_thread_event(
            state,
            &tc.tenant_id,
            events::agent_profile_lifecycle_event(events::AgentProfileLifecycleInput {
                tenant_id: tc.tenant_id.clone(),
                profile_id: result.selection.profile_id.clone(),
                profile_version_id: result.selection.profile_version_id.clone(),
                actor_principal_id: "system".to_string(),
                event_name: "agent_profile.safe_default_fallback".to_string(),
                outcome: "succeeded".to_string(),
                reason_code: "current_default_retired".to_string(),
                permission_gate: "profiles.manage".to_string(),
                audit_event_id: result.selection.audit_event_id.clone(),
                redaction_status: profiles::RedactionStatus::REDACTED,
                ..Default::default()
            }),
        )?;
    }
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Go requireThreadPermission: the caller needs a resolved tenant context with
/// the given permission; failures answer the stable credential denial (403).
fn require_thread_permission(
    tenant: Option<&TenantContext>,
    permission: Permission,
) -> Result<&kura_identity::TenantContext, ApiError> {
    let Some(tc) = tenant else {
        return Err(credential_denial());
    };
    if tc.0.tenant_id.trim().is_empty() || !has_permission(&tc.0.permissions, permission) {
        return Err(credential_denial());
    }
    Ok(&tc.0)
}

/// Go writeCredentialDenial(403, "permission_missing"): the stable error
/// string is credential_access_denied.
fn credential_denial() -> ApiError {
    ApiError::Forbidden("credential_access_denied".to_string())
}

/// Go canInspectProfileRuntime: profiles.inspect grants the runtime projection
/// visibility on thread detail.
fn can_inspect_profile_runtime(tenant: Option<&TenantContext>) -> bool {
    match tenant {
        Some(tc) => has_permission(&tc.0.permissions, Permission::ProfilesInspect),
        None => false,
    }
}

/// Go coalesceReason.
fn coalesce_reason(value: &str, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

/// Wire string for a lifecycle action kind (Go's `string(kind)`).
fn lifecycle_action_kind_str(kind: kura_threads::LifecycleActionKind) -> &'static str {
    match kind {
        kura_threads::LifecycleActionKind::Reset => "reset",
        kura_threads::LifecycleActionKind::Archive => "archive",
        kura_threads::LifecycleActionKind::Reopen => "reopen",
    }
}

/// Go handleThreadLifecycleMutationError: audit-evidence failures surface as
/// 500 with the stable message; transition/conflict/reopen-eligibility
/// failures surface as 409 with the error text; everything else is 500.
fn map_thread_lifecycle_error(message: String) -> ApiError {
    if message == kura_threads::ThreadsError::AuditEvidenceRequired.to_string() {
        ApiError::Internal("thread lifecycle audit evidence is required".to_string())
    } else if message == kura_threads::ThreadsError::LifecycleTransitionNotAllowed.to_string()
        || message == kura_threads::ThreadsError::LifecycleMutationConflict.to_string()
        || message == kura_threads::ThreadsError::LifecycleReopenNotEligible.to_string()
    {
        ApiError::Conflict(message)
    } else {
        ApiError::from_store(message)
    }
}

/// Go: ThreadAuditFailedClosedEvent is published for audit-evidence and
/// concurrent-mutation failures.
fn is_audit_failed_closed(message: &str) -> bool {
    message == kura_threads::ThreadsError::AuditEvidenceRequired.to_string()
        || message == kura_threads::ThreadsError::LifecycleMutationConflict.to_string()
}

/// Go publishEvent (legacy store-append path then bus publish). The thread
/// event builders carry the tenant id; the environment scope comes from the
/// daemon config.
fn publish_thread_event(state: &AppState, _tenant_id: &str, event: events::Event) -> Result<(), ApiError> {
    let mut event = event;
    if event.environment_scope.is_empty() {
        event.environment_scope = crate::middleware::environment_scope_from_config(&state.config);
    }
    let stored = state
        .store
        .lock()
        .append_event(&event)
        .map_err(ApiError::from_store)?;
    state.event_bus.publish(stored);
    Ok(())
}

/// Go decodeJSONBody: an empty body maps to "request body is required" (400);
/// malformed JSON maps to the decoder error (400).
fn decode_json_body<T: serde::de::DeserializeOwned>(body: &Bytes) -> Result<T, ApiError> {
    if body.is_empty() {
        return Err(ApiError::BadRequest("request body is required".to_string()));
    }
    serde_json::from_slice(body).map_err(|err| ApiError::BadRequest(err.to_string()))
}

/// Go decodeOptionalJSON: an empty body decodes to the zero value (EOF is
/// tolerated); malformed JSON answers 400.
fn decode_optional_json_body<T: serde::de::DeserializeOwned>(body: &Bytes) -> Result<T, ApiError> {
    if body.is_empty() {
        return serde_json::from_slice(b"{}").map_err(|err| ApiError::BadRequest(err.to_string()));
    }
    serde_json::from_slice(body).map_err(|err| ApiError::BadRequest(err.to_string()))
}

/// Go defaultAPIReason.
fn default_api_reason(value: &str, fallback: &str) -> String {
    coalesce_reason(value, fallback)
}

/// Go limit parse: unparseable/zero -> 0 (the store applies its default).
fn parse_limit(raw: Option<&String>) -> i64 {
    match raw {
        Some(value) if !value.trim().is_empty() => value.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

/// Go requireProfilePermission: resolved tenant context + the given
/// permission, else a denied profile lifecycle event (when the tenant is
/// known) followed by the stable 403 credential denial.
fn require_profile_permission<'a>(
    state: &AppState,
    tenant: Option<&'a TenantContext>,
    permission: Permission,
    event_name: &str,
    profile_id: &str,
) -> Result<&'a kura_identity::TenantContext, ApiError> {
    let Some(tc) = tenant else {
        return Err(credential_denial());
    };
    if tc.0.tenant_id.trim().is_empty() || !has_permission(&tc.0.permissions, permission) {
        if !tc.0.tenant_id.trim().is_empty() {
            let _ = publish_thread_event(
                state,
                &tc.0.tenant_id,
                events::agent_profile_lifecycle_event(events::AgentProfileLifecycleInput {
                    tenant_id: tc.0.tenant_id.clone(),
                    profile_id: profile_id.to_string(),
                    actor_principal_id: tc.0.principal_id.clone(),
                    event_name: event_name.to_string(),
                    outcome: "denied".to_string(),
                    reason_code: "permission_denied".to_string(),
                    permission_gate: "profiles.manage".to_string(),
                    redaction_status: profiles::RedactionStatus::REDACTED,
                    ..Default::default()
                }),
            );
        }
        return Err(credential_denial());
    }
    Ok(&tc.0)
}

/// Go publishProfileLifecycle for mutation failures: a denied lifecycle event.
fn publish_profile_denied_event(
    state: &AppState,
    tc: &kura_identity::TenantContext,
    profile_id: &str,
    event_name: &str,
    message: &str,
) -> Result<(), ApiError> {
    publish_thread_event(
        state,
        &tc.tenant_id,
        events::agent_profile_lifecycle_event(events::AgentProfileLifecycleInput {
            tenant_id: tc.tenant_id.clone(),
            profile_id: profile_id.to_string(),
            actor_principal_id: tc.principal_id.clone(),
            event_name: event_name.to_string(),
            outcome: "denied".to_string(),
            reason_code: profile_reason_code(message),
            permission_gate: "profiles.manage".to_string(),
            redaction_status: profiles::RedactionStatus::REDACTED,
            ..Default::default()
        }),
    )
}

/// Go publishProfileMutationEvents: lifecycle + version-created events for a
/// successful profile mutation.
fn publish_profile_mutation_events(
    state: &AppState,
    actor: &kura_identity::TenantContext,
    result: &profiles::MutationResult,
    event_name: &str,
    outcome: &str,
    reason_code: &str,
) -> Result<(), ApiError> {
    publish_thread_event(
        state,
        &actor.tenant_id,
        events::agent_profile_lifecycle_event(events::AgentProfileLifecycleInput {
            tenant_id: actor.tenant_id.clone(),
            profile_id: result.profile.profile_id.clone(),
            profile_version_id: result.version.profile_version_id.clone(),
            actor_principal_id: actor.principal_id.clone(),
            event_name: event_name.to_string(),
            outcome: outcome.to_string(),
            reason_code: reason_code.to_string(),
            permission_gate: "profiles.manage".to_string(),
            safe_summary: profiles::safe_profile_summary(&result.profile),
            audit_event_id: result.audit_event_id.clone(),
            redaction_status: profiles::RedactionStatus::REDACTED,
        }),
    )?;
    if !result.version.profile_version_id.is_empty() {
        publish_thread_event(
            state,
            &actor.tenant_id,
            events::agent_profile_version_created_event(events::AgentProfileVersionInput {
                tenant_id: actor.tenant_id.clone(),
                profile_id: result.profile.profile_id.clone(),
                profile_version_id: result.version.profile_version_id.clone(),
                change_kind: result.version.change_kind.clone(),
                version_number: result.version.version_number,
                reason_code: reason_code.to_string(),
                redaction_status: profiles::RedactionStatus::REDACTED,
            }),
        )?;
    }
    Ok(())
}

/// Go writeProfileError: sentinel mapping for the store error strings.
fn map_profile_error(message: String) -> ApiError {
    if message == "agent profile not found" {
        return ApiError::NotFound("profile not found".to_string());
    }
    if message == profiles::ProfilesError::ProfileNotActivatable.to_string() {
        return ApiError::Conflict(message);
    }
    if message == profiles::ProfilesError::ScopedBindingDeferred.to_string()
        || message.starts_with("profile validation failed")
    {
        return ApiError::BadRequest(message);
    }
    if message == profiles::ProfilesError::ExplicitActorRequired.to_string() {
        return credential_denial();
    }
    ApiError::from_store(message)
}

/// Go reasonCodeForProfileError: the stable reason code for a failed profile
/// mutation (used in the validation_failed denial event).
fn profile_reason_code(message: &str) -> String {
    if let Some(code) = message.strip_prefix("profile validation failed: ") {
        let code = code.trim();
        if code.is_empty() {
            return "profile_validation_failed".to_string();
        }
        return code.to_string();
    }
    if message == profiles::ProfilesError::ScopedBindingDeferred.to_string() {
        return "scoped_binding_deferred".to_string();
    }
    if message == profiles::ProfilesError::ProfileNotActivatable.to_string() {
        return "profile_not_activatable".to_string();
    }
    if message == "agent profile not found" {
        return "profile_not_found".to_string();
    }
    if message == profiles::ProfilesError::ExplicitActorRequired.to_string() {
        return "explicit_actor_required".to_string();
    }
    "profile_operation_failed".to_string()
}

/// Go recordActiveProfileProjectionForTarget: records the active profile
/// selection as a runtime projection anchored to the target resource and
/// publishes agent_profile.runtime_projected. Returns None when the tenant
/// has no active selection or the resource id is empty.
fn record_active_profile_projection_for_target(
    state: &AppState,
    tc: &kura_identity::TenantContext,
    resource_kind: profiles::RuntimeResourceKind,
    resource_id: &str,
    thread_id: &str,
    session_segment_id: &str,
    handoff_id: &str,
) -> Result<Option<profiles::RuntimeProjection>, ApiError> {
    if resource_id.trim().is_empty() {
        return Ok(None);
    }
    let Some((profile, selection)) = state
        .store
        .lock()
        .active_agent_profile_selection(&tc.tenant_id)
        .map_err(ApiError::from_store)?
    else {
        return Ok(None);
    };
    let projection = profiles::build_runtime_projection(
        &profile,
        &selection,
        profiles::RuntimeProjectionInput {
            resource_kind,
            resource_id: resource_id.trim().to_string(),
            thread_id: thread_id.trim().to_string(),
            session_id: session_segment_id.trim().to_string(),
            handoff_id: handoff_id.trim().to_string(),
            ..Default::default()
        },
    );
    let recorded = state
        .store
        .lock()
        .record_runtime_profile_projection(projection)
        .map_err(ApiError::from_store)?;
    publish_thread_event(
        state,
        &tc.tenant_id,
        events::agent_profile_runtime_projected_event(recorded.clone()),
    )?;
    Ok(Some(recorded))
}

/// Go writeThreadDetailWithBindingProjection: for callers holding
/// bindings.inspect, attaches the latest runtime binding evidence as an
/// additive `bindingProjection` field (FR-013, SC-012).
fn write_thread_detail_with_binding_projection(
    state: &AppState,
    tc: &kura_identity::TenantContext,
    thread_id: &str,
    response: threads::ThreadDetailResponse,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut merged = serde_json::to_value(response)
        .map_err(ApiError::from)?
        .as_object()
        .cloned()
        .unwrap_or_default();
    if has_permission(&tc.permissions, Permission::BindingsInspect) {
        let evidence = state
            .store
            .lock()
            .latest_runtime_binding_evidence(&tc.tenant_id, "thread", thread_id)
            .map_err(ApiError::from_store)?;
        if let Some(evidence) = evidence {
            merged.insert(
                "bindingProjection".to_string(),
                serde_json::to_value(kura_bindings::to_runtime_evidence_resource(&evidence))
                    .map_err(ApiError::from)?,
            );
        }
    }
    Ok(Json(serde_json::Value::Object(merged)))
}
#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use axum::http::header::CONTENT_TYPE;
    use chrono::{Duration, TimeZone};
    use kura_identity::Permission;
    use kura_threads as threads;
    use parking_lot::Mutex;
    use tower::ServiceExt;
    use uuid::Uuid;

    fn test_config() -> kura_config::Config {
        kura_config::Config {
            project_root: String::new(),
            environment: kura_config::Environment::Test,
            bind_addr: "127.0.0.1:19192".to_string(),
            data_dir: "/tmp/kura-api-resources".to_string(),
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

    fn test_state() -> AppState {
        let dir = std::env::temp_dir().join(format!("kura-api-resources-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let store = Arc::new(Mutex::new(
            kura_store::SQLiteStore::new(dir.to_str().expect("path")).expect("store"),
        ));
        AppState::new(test_config(), Arc::new(kura_events::Bus::new()), store)
    }

    fn request(method: &str, uri: &str, body: Option<&str>) -> Request<Body> {
        let builder = Request::builder()
            .method(method)
            .uri(uri)
            .header(CONTENT_TYPE, "application/json");
        let req = match body {
            Some(payload) => builder.body(Body::from(payload.to_string())).expect("request"),
            None => builder.body(Body::empty()).expect("request"),
        };
        req
    }

    /// Builds a request with a resolved tenant context extension (the 
    /// protected() middleware installs this once auth is wired; tests inject it
    /// directly, matching reminders.rs).
    fn tenant_request(
        method: &str,
        uri: &str,
        body: Option<&str>,
        tenant_id: &str,
        permissions: Vec<Permission>,
    ) -> Request<Body> {
        let mut req = request(method, uri, body);
        req.extensions_mut().insert(TenantContext(kura_identity::TenantContext {
            tenant_id: tenant_id.to_string(),
            principal_id: format!("prn_{tenant_id}"),
            permissions,
            ..Default::default()
        }));
        req
    }

    async fn send(app: &axum::Router, req: Request<Body>) -> (StatusCode, serde_json::Value) {
        let response = app.clone().oneshot(req).await.expect("oneshot");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.expect("body");
        // axum's default 404 (route miss) has an empty body; ApiError responses
        // carry the {code,message,error} envelope.
        let json = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("json body")
        };
        (status, json)
    }

    fn seed_thread(state: &AppState, thread: &threads::Thread) {
        state.store.lock().upsert_thread(thread).expect("upsert thread");
    }

    fn thread(
        id: &str,
        tenant: &str,
        lifecycle: threads::LifecycleState,
        source: threads::SourceKind,
        summary: &str,
        last_activity: chrono::DateTime<chrono::Utc>,
        created: chrono::DateTime<chrono::Utc>,
    ) -> threads::Thread {
        threads::Thread {
            thread_id: id.to_string(),
            tenant_id: tenant.to_string(),
            lifecycle_state: lifecycle,
            current_session_segment_id: format!("seg_{id}"),
            source_kind: source,
            source_summary: summary.to_string(),
            last_activity_at: last_activity,
            created_at: created,
            updated_at: last_activity,
            retention_expires_at: Some(created + Duration::days(90)),
            redaction_status: threads::RedactionStatus::Redacted,
        }
    }

    // Port of TestThreadLifecycleListDetailPaginationAndDenial.
    #[tokio::test]
    async fn thread_list_detail_pagination_and_denial() {
        let state = test_state();
        // Base the fixture on the real clock (minus a small offset) so the
        // 90-day retention expiries the store derives are still in the future.
        let now = chrono::Utc::now() - Duration::minutes(5);
        seed_thread(&state, &thread("thr_active", "ten_threads", threads::LifecycleState::Active, threads::SourceKind::Channel, "Slack Main / #support", now + Duration::minutes(1), now));
        seed_thread(&state, &thread("thr_archived", "ten_threads", threads::LifecycleState::Archived, threads::SourceKind::Workflow, "Workflow", now + Duration::minutes(2), now));
        seed_thread(&state, &thread("thr_other", "ten_other", threads::LifecycleState::Active, threads::SourceKind::Channel, "Other", now + Duration::minutes(3), now));
        {
            let store = state.store.lock();
            store
                .save_thread_source_linkage(&threads::SourceLinkage {
                    source_linkage_id: "src_active".to_string(),
                    thread_id: "thr_active".to_string(),
                    tenant_id: "ten_threads".to_string(),
                    source_kind: threads::SourceKind::Channel,
                    connector_id: "slack-main".to_string(),
                    connector_kind: "slack".to_string(),
                    source_account_id: "workspace_redacted".to_string(),
                    source_conversation_id: "channel_redacted".to_string(),
                    source_message_id: "msg_redacted".to_string(),
                    routing_outcome: threads::RoutingOutcome::Accepted,
                    current: true,
                    linked_at: Some(now),
                    retention_expires_at: None,
                    redaction_status: threads::RedactionStatus::Redacted,
                })
                .expect("save source linkage");
            store
                .save_thread_runtime_projection(&threads::RuntimeProjection {
                    runtime_projection_id: "rtp_run_active".to_string(),
                    thread_id: "thr_active".to_string(),
                    tenant_id: "ten_threads".to_string(),
                    session_segment_id: "seg_thr_active".to_string(),
                    resource_kind: threads::RuntimeResourceKind::Run,
                    resource_id: "run_1".to_string(),
                    status: "completed".to_string(),
                    reason_code: "accepted".to_string(),
                    occurred_at: now,
                    route: "/v1/runs/run_1".to_string(),
                    safe_summary: "Assistant run completed".to_string(),
                    retention_expires_at: None,
                    redaction_status: threads::RedactionStatus::Redacted,
                })
                .expect("save runtime projection");
            let shape = threads::resolve_conversation_shape(&threads::ConversationShapeResolutionInput {
                tenant_id: "ten_threads".to_string(),
                thread_id: "thr_active".to_string(),
                session_segment_id: "seg_thr_active".to_string(),
                source_kind: threads::SourceKind::Channel,
                connector_id: "slack-main".to_string(),
                connector_kind: "slack".to_string(),
                source_account_id: "workspace_redacted".to_string(),
                source_conversation_id: "channel_redacted".to_string(),
                source_conversation_summary: "Slack Main / #support".to_string(),
                claimed_shape: Some(threads::ConversationShape::Room),
                now: Some(now),
            });
            store.save_conversation_shape_evidence(&shape).expect("save shape");
            store
                .save_participation_decision(&threads::ParticipationDecision {
                    participation_decision_id: String::new(),
                    tenant_id: "ten_threads".to_string(),
                    thread_id: "thr_active".to_string(),
                    session_segment_id: "seg_thr_active".to_string(),
                    connector_id: "slack-main".to_string(),
                    connector_kind: "slack".to_string(),
                    source_account_id: "workspace_redacted".to_string(),
                    source_conversation_id: "channel_redacted".to_string(),
                    source_message_id: "msg_redacted".to_string(),
                    conversation_shape: threads::ConversationShape::Room,
                    policy_id: String::new(),
                    mention_status: threads::MentionStatus::Missing,
                    allowlist_status: threads::AllowlistStatus::Eligible,
                    decision: threads::ParticipationDecisionValue::Ignored,
                    reason_code: threads::GROUP_ROOM_REASON_MISSING_QUALIFYING_MENTION.to_string(),
                    created_assistant_work: false,
                    occurred_at: Some(now),
                    retention_expires_at: None,
                    redaction_status: threads::RedactionStatus::Redacted,
                    safe_summary: "Room message ignored by participation policy".to_string(),
                })
                .expect("save participation decision");
        }
        let app = crate::routes::router(state.clone());

        // First page: limit 1 -> active thread (archived sorts last).
        let req = tenant_request("GET", "/v1/threads?limit=1", None, "ten_threads", vec![Permission::CredentialsInspect]);
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::OK, "list body: {json}");
        assert_eq!(json["tenantId"], "ten_threads");
        assert_eq!(json["items"].as_array().map(|v| v.len()), Some(1));
        assert_eq!(json["items"][0]["threadId"], "thr_active");
        assert_ne!(json["page"]["nextCursor"], "");

        // Second page via cursor.
        let cursor = json["page"]["nextCursor"].as_str().unwrap();
        let req = tenant_request("GET", &format!("/v1/threads?limit=1&cursor={cursor}"), None, "ten_threads", vec![Permission::CredentialsInspect]);
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::OK, "next page body: {json}");
        assert_eq!(json["items"][0]["threadId"], "thr_archived");

        // State + source-kind filters.
        let req = tenant_request("GET", "/v1/threads?state=archived&sourceKind=workflow", None, "ten_threads", vec![Permission::CredentialsInspect]);
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::OK, "filtered body: {json}");
        assert_eq!(json["items"].as_array().map(|v| v.len()), Some(1));
        assert_eq!(json["items"][0]["threadId"], "thr_archived");

        // Detail with the full operator trace.
        let req = tenant_request("GET", "/v1/threads/thr_active", None, "ten_threads", vec![Permission::CredentialsInspect]);
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::OK, "detail body: {json}");
        assert_eq!(json["thread"]["threadId"], "thr_active");
        assert_eq!(json["thread"]["tenantId"], "ten_threads");
        assert_eq!(json["sourceLinkages"].as_array().map(|v| v.len()), Some(1));
        assert_eq!(json["sourceLinkages"][0]["routingOutcome"], "accepted");
        assert_eq!(json["runtimeProjections"].as_array().map(|v| v.len()), Some(1));
        assert_eq!(json["runtimeProjections"][0]["resourceKind"], "run");
        assert_eq!(json["conversationShape"]["shape"], "room");
        assert_eq!(json["participationDecisions"].as_array().map(|v| v.len()), Some(1));
        assert_eq!(json["participationDecisions"][0]["reasonCode"], "missing_qualifying_mention");
        let raw = serde_json::to_string(&json).expect("marshal detail");
        for forbidden in ["semanticSummary", "recalledMemory", "contextPacking", "autonomousPruning"] {
            assert!(!raw.contains(forbidden), "detail leaked {forbidden}: {raw}");
        }

        // Denial without the inspect permission.
        let req = tenant_request("GET", "/v1/threads", None, "ten_threads", vec![]);
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "denied list body: {json}");
        assert_eq!(json["error"], "credential_access_denied");

        let req = tenant_request("GET", "/v1/threads/thr_active", None, "ten_threads", vec![]);
        let (status, _) = send(&app, req).await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        // A tenant with no threads gets an empty page (not an error).
        let req = tenant_request("GET", "/v1/threads", None, "ten_empty", vec![Permission::CredentialsInspect]);
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::OK, "empty body: {json}");
        assert_eq!(json["tenantId"], "ten_empty");
        assert_eq!(json["items"].as_array().map(|v| v.len()), Some(0));

        // Missing thread -> 404.
        let req = tenant_request("GET", "/v1/threads/thr_missing", None, "ten_threads", vec![Permission::CredentialsInspect]);
        let (status, _) = send(&app, req).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    // Port of TestThreadLifecycleMutationsRequireManagePermissionAndPersistAudit.
    #[tokio::test]
    async fn thread_lifecycle_mutations_require_manage_permission_and_persist_audit() {
        let state = test_state();
        let now = chrono::Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap();
        seed_thread(&state, &thread("thr_mutate", "ten_threads", threads::LifecycleState::Active, threads::SourceKind::Channel, "Slack", now, now));
        {
            let store = state.store.lock();
            store
                .upsert_thread_session_segment(&threads::SessionSegment {
                    session_segment_id: "seg_thr_mutate".to_string(),
                    thread_id: "thr_mutate".to_string(),
                    tenant_id: "ten_threads".to_string(),
                    session_id: "sess_1".to_string(),
                    generation: 1,
                    state: "active".to_string(),
                    started_at: now,
                    ended_at: None,
                    last_active_at: now,
                    reset_from_session_segment_id: String::new(),
                    partial_evidence: false,
                })
                .expect("upsert segment");
            store
                .save_thread_source_linkage(&threads::SourceLinkage {
                    source_linkage_id: "src_mutate_current".to_string(),
                    thread_id: "thr_mutate".to_string(),
                    tenant_id: "ten_threads".to_string(),
                    source_kind: threads::SourceKind::Channel,
                    connector_id: "slack-main".to_string(),
                    connector_kind: "slack".to_string(),
                    source_account_id: "acct_redacted".to_string(),
                    source_conversation_id: "conv_redacted".to_string(),
                    source_message_id: String::new(),
                    routing_outcome: threads::RoutingOutcome::Accepted,
                    current: true,
                    linked_at: Some(now),
                    retention_expires_at: Some(now + Duration::days(90)),
                    redaction_status: threads::RedactionStatus::Redacted,
                })
                .expect("save source linkage");
            let shape = threads::resolve_conversation_shape(&threads::ConversationShapeResolutionInput {
                tenant_id: "ten_threads".to_string(),
                thread_id: "thr_mutate".to_string(),
                session_segment_id: "seg_thr_mutate".to_string(),
                source_kind: threads::SourceKind::Channel,
                connector_id: "slack-main".to_string(),
                connector_kind: "slack".to_string(),
                source_account_id: "acct_redacted".to_string(),
                source_conversation_id: "conv_redacted".to_string(),
                source_conversation_summary: "Slack / #support".to_string(),
                claimed_shape: Some(threads::ConversationShape::Room),
                now: Some(now),
            });
            store.save_conversation_shape_evidence(&shape).expect("save shape");
        }
        let app = crate::routes::router(state.clone());

        // Inspect-only callers cannot mutate.
        let req = tenant_request("POST", "/v1/threads/thr_mutate/archive", Some("{}"), "ten_threads", vec![Permission::CredentialsInspect]);
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "denied archive body: {json}");

        // Archive with connectors.manage.
        let req = tenant_request("POST", "/v1/threads/thr_mutate/archive", Some(r#"{"reasonCode":"operator_archive"}"#), "ten_threads", vec![Permission::ConnectorsManage]);
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::OK, "archive body: {json}");
        assert_eq!(json["lifecycleState"], "archived");
        assert_eq!(json["action"], "archive");
        assert_ne!(json["auditEventId"], "");

        // Reopen an archived thread.
        let req = tenant_request("POST", "/v1/threads/thr_mutate/reopen", Some("{}"), "ten_threads", vec![Permission::ConnectorsManage]);
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::OK, "reopen body: {json}");
        assert_eq!(json["lifecycleState"], "reopened");

        // Reset publishes the scoped reset evidence event.
        let req = tenant_request("POST", "/v1/threads/thr_mutate/reset", Some("{}"), "ten_threads", vec![Permission::ConnectorsManage]);
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::OK, "reset body: {json}");
        assert_eq!(json["lifecycleState"], "reset");

        // The mutation trail persisted: 2 segments, 3 lifecycle actions, 1
        // scoped reset event.
        let detail = state
            .store
            .lock()
            .get_thread_detail_for_tenant("ten_threads", "thr_mutate")
            .expect("detail")
            .expect("found");
        assert_eq!(detail.thread.lifecycle_state, threads::LifecycleState::Reset);
        assert_eq!(detail.session_segments.len(), 2);
        assert_eq!(detail.lifecycle_actions.len(), 3);
        assert_eq!(detail.reset_events.len(), 1);
        assert_eq!(detail.reset_events[0].conversation_shape, threads::ConversationShape::Room);
        assert_eq!(detail.reset_events[0].permission_gate, "connectors.manage");

        let thread_events = state.event_bus.list(&kura_events::Filter {
            category: "thread".to_string(),
            ..Default::default()
        });
        assert!(
            thread_events
                .iter()
                .any(|event| event.name == kura_events::THREAD_RESET_SCOPED_NAME
                    && event.payload.get("conversationShape").and_then(|v| v.as_str()) == Some("room")),
            "expected thread.reset_scoped event with room shape, got {thread_events:?}"
        );
    }

    #[tokio::test]
    async fn thread_lifecycle_validation_404_and_conflict() {
        let state = test_state();
        let now = chrono::Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap();
        seed_thread(&state, &thread("thr_validate", "ten_threads", threads::LifecycleState::Active, threads::SourceKind::Channel, "Slack", now, now));
        seed_thread(&state, &thread("thr_other", "ten_other", threads::LifecycleState::Active, threads::SourceKind::Channel, "Other", now, now));
        {
            let store = state.store.lock();
            store
                .upsert_thread_session_segment(&threads::SessionSegment {
                    session_segment_id: "seg_thr_validate".to_string(),
                    thread_id: "thr_validate".to_string(),
                    tenant_id: "ten_threads".to_string(),
                    session_id: String::new(),
                    generation: 1,
                    state: "active".to_string(),
                    started_at: now,
                    ended_at: None,
                    last_active_at: now,
                    reset_from_session_segment_id: String::new(),
                    partial_evidence: false,
                })
                .expect("upsert segment");
        }
        let app = crate::routes::router(state.clone());

        // Empty body -> 400 "request body is required".
        let req = tenant_request("POST", "/v1/threads/thr_validate/archive", None, "ten_threads", vec![Permission::ConnectorsManage]);
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "empty body: {json}");
        assert_eq!(json["message"], "request body is required");

        // Malformed body -> 400.
        let req = tenant_request("POST", "/v1/threads/thr_validate/archive", Some("{not json"), "ten_threads", vec![Permission::ConnectorsManage]);
        let (status, _) = send(&app, req).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // Unknown action segment -> 404 (axum route miss).
        let req = tenant_request("POST", "/v1/threads/thr_validate/frobnicate", Some("{}"), "ten_threads", vec![Permission::ConnectorsManage]);
        let (status, _) = send(&app, req).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // Missing thread -> 404.
        let req = tenant_request("POST", "/v1/threads/thr_missing/archive", Some("{}"), "ten_threads", vec![Permission::ConnectorsManage]);
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "missing body: {json}");

        // Cross-tenant mutation is scoped: ten_threads cannot touch ten_other's
        // thread (404, never leaked as 403/200).
        let req = tenant_request("POST", "/v1/threads/thr_other/archive", Some("{}"), "ten_threads", vec![Permission::ConnectorsManage]);
        let (status, _) = send(&app, req).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // Archiving an archived thread -> 409 lifecycle transition not allowed.
        let req = tenant_request("POST", "/v1/threads/thr_validate/archive", Some("{}"), "ten_threads", vec![Permission::ConnectorsManage]);
        let (status, _) = send(&app, req).await;
        assert_eq!(status, StatusCode::OK);
        let req = tenant_request("POST", "/v1/threads/thr_validate/archive", Some("{}"), "ten_threads", vec![Permission::ConnectorsManage]);
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::CONFLICT, "conflict body: {json}");
        assert_eq!(json["message"], "lifecycle transition is not allowed");
    }

    // Port of TestThreadHandoffCreateChannelToWebPersistsSeparateDestinationAndReferences.
    #[tokio::test]
    async fn thread_handoff_channel_to_web_persists_destination_and_references() {
        let state = test_state();
        // Base the fixture on the real clock so the 90-day retention the
        // handler derives stays in the future.
        let now = chrono::Utc::now();
        {
            let store = state.store.lock();
            let source = threads::Thread {
                thread_id: "thr_handoff_source".to_string(),
                tenant_id: "ten_threads".to_string(),
                lifecycle_state: threads::LifecycleState::Active,
                current_session_segment_id: "seg_source".to_string(),
                source_kind: threads::SourceKind::Channel,
                source_summary: "Slack / #support".to_string(),
                last_activity_at: now,
                created_at: now,
                updated_at: now,
                retention_expires_at: Some(now + Duration::days(90)),
                redaction_status: threads::RedactionStatus::Redacted,
            };
            store.upsert_thread(&source).expect("upsert source");
            store
                .upsert_thread_session_segment(&threads::SessionSegment {
                    session_segment_id: "seg_source".to_string(),
                    thread_id: source.thread_id.clone(),
                    tenant_id: source.tenant_id.clone(),
                    session_id: String::new(),
                    generation: 1,
                    state: "active".to_string(),
                    started_at: now,
                    ended_at: None,
                    last_active_at: now,
                    reset_from_session_segment_id: String::new(),
                    partial_evidence: false,
                })
                .expect("upsert source segment");
            let shape = threads::resolve_conversation_shape(&threads::ConversationShapeResolutionInput {
                tenant_id: source.tenant_id.clone(),
                thread_id: source.thread_id.clone(),
                session_segment_id: source.current_session_segment_id.clone(),
                source_kind: threads::SourceKind::Channel,
                connector_id: "slack-main".to_string(),
                connector_kind: "slack".to_string(),
                source_account_id: "workspace_redacted".to_string(),
                source_conversation_id: "channel_redacted".to_string(),
                source_conversation_summary: source.source_summary.clone(),
                claimed_shape: Some(threads::ConversationShape::Room),
                now: Some(now),
            });
            store.save_conversation_shape_evidence(&shape).expect("save source shape");
            store
                .save_continuity_turn(&threads::ContinuityTurn {
                    continuity_turn_id: "turn_source_1".to_string(),
                    tenant_id: source.tenant_id.clone(),
                    thread_id: source.thread_id.clone(),
                    session_segment_id: source.current_session_segment_id.clone(),
                    acceptance_sequence: 1,
                    role: threads::ContinuityRole::User,
                    source_kind: threads::SourceKind::Channel,
                    source_linkage_id: String::new(),
                    source_message_id: String::new(),
                    source_timestamp: None,
                    dispatch_id: String::new(),
                    response_to_turn_id: String::new(),
                    safe_content: "safe source context".to_string(),
                    content_redaction_status: threads::RedactionStatus::Redacted,
                    artifact_excerpt_refs: Vec::new(),
                    recorded_at: now,
                    retention_expires_at: Some(now + Duration::days(90)),
                    source_event_key: String::new(),
                })
                .expect("save continuity turn");
            // Connector + route policy make the channel endpoint eligible.
            store
                .upsert_connector(&kura_connectors::Connector {
                    tenant_id: "ten_threads".to_string(),
                    connector_id: "slack-main".to_string(),
                    kind: "slack".to_string(),
                    display_name: "Slack Main".to_string(),
                    status: kura_connectors::Status::Healthy,
                    capability_profile: {
                        let mut profile = serde_json::Map::new();
                        profile.insert(
                            kura_connectors::HANDOFF_SURFACE_DESTINATION_SUPPORT.to_string(),
                            serde_json::Value::String("supported".to_string()),
                        );
                        profile
                    },
                    created_at: now,
                    updated_at: now,
                    ..Default::default()
                })
                .expect("upsert connector");
            store
                .save_channel_route_policy(&kura_connectors::RoutePolicy {
                    route_policy_id: "route_policy_slack-main".to_string(),
                    tenant_id: "ten_threads".to_string(),
                    connector_id: "slack-main".to_string(),
                    eligible_rooms: vec!["channel_redacted".to_string()],
                    eligible_channels: vec!["channel_redacted".to_string()],
                    eligible_conversations: vec!["channel_redacted".to_string()],
                    validation_state: "valid".to_string(),
                    validated_at: now,
                    redaction_status: kura_connectors::RedactionStatus::Redacted,
                    ..Default::default()
                })
                .expect("save route policy");
            let result = store
                .create_agent_profile(
                    &kura_identity::TenantContext {
                        tenant_id: "ten_threads".to_string(),
                        principal_id: "prn_profile_admin".to_string(),
                        ..Default::default()
                    },
                    &profiles::MutationInput {
                        display_name: "Handoff Agent".to_string(),
                        persona: profiles::Persona { safe_summary: "handoff profile".to_string(), ..Default::default() },
                        activate: true,
                        ..Default::default()
                    },
                )
                .expect("create profile");
            let _profile_id = result.profile.profile_id;
        }
        let app = crate::routes::router(state.clone());
        let req = tenant_request(
            "POST",
            "/v1/threads/thr_handoff_source/handoffs",
            Some(r#"{"destination":{"surface":"web"},"reasonCode":"user_requested_handoff"}"#),
            "ten_threads",
            vec![Permission::ConnectorsManage, Permission::ProfilesInspect],
        );
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::CREATED, "handoff body: {json}");
        assert_eq!(json["sourceThreadId"], "thr_handoff_source");
        let destination = json["destinationThreadId"].as_str().expect("destination").to_string();
        assert!(!destination.is_empty() && destination != "thr_handoff_source");
        assert_eq!(json["destinationConversationShape"], "web");
        let profile_id = {
            let store = state.store.lock();
            store
                .active_agent_profile_selection("ten_threads")
                .expect("selection")
                .expect("found")
                .0
                .profile_id
        };
        assert_eq!(json["activeProfileProjection"]["profileId"], profile_id.as_str());

        // The continuity source reference was persisted with the Referenced decision.
        let link_id = json["handoffLinkId"].as_str().expect("link id").to_string();
        let refs = {
            let store = state.store.lock();
            store
                .list_handoff_source_references_for_link("ten_threads", &link_id)
                .expect("list refs")
        };
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].decision, threads::HandoffSourceReferenceDecision::Referenced);
        assert_eq!(refs[0].continuity_turn_id, "turn_source_1");

        // thread.handoff_linked was published.
        let events = state
            .event_bus
            .list(&kura_events::Filter { category: "thread".to_string(), ..Default::default() });
        assert!(
            events.iter().any(|event| event.name == kura_events::THREAD_HANDOFF_LINKED_NAME
                && event.payload.get("sourceThreadId").and_then(|v| v.as_str()) == Some("thr_handoff_source")),
            "expected handoff linked event: {events:?}"
        );
    }

    // Port of TestThreadHandoffCreateRequiresManagePermission.
    #[tokio::test]
    async fn thread_handoff_requires_manage_permission() {
        let state = test_state();
        let app = crate::routes::router(state.clone());
        let req = tenant_request(
            "POST",
            "/v1/threads/missing/handoffs",
            Some(r#"{"destination":{"surface":"web"}}"#),
            "ten_threads",
            vec![Permission::CredentialsInspect],
        );
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "denied body: {json}");
    }

    // Continuity preview detail answers 200 with the preview + items and 404
    // for missing previews.
    #[tokio::test]
    async fn thread_continuity_preview_detail_ok_and_404() {
        let state = test_state();
        let now = chrono::Utc::now();
        {
            let store = state.store.lock();
            // The preview references the thread and its session segment (FK).
            store
                .upsert_thread(&threads::Thread {
                    thread_id: "thr_preview".to_string(),
                    tenant_id: "ten_threads".to_string(),
                    lifecycle_state: threads::LifecycleState::Active,
                    current_session_segment_id: "seg_preview".to_string(),
                    source_kind: threads::SourceKind::Channel,
                    source_summary: "Preview thread".to_string(),
                    last_activity_at: now,
                    created_at: now,
                    updated_at: now,
                    retention_expires_at: Some(now + Duration::days(90)),
                    redaction_status: threads::RedactionStatus::Redacted,
                })
                .expect("upsert thread");
            store
                .upsert_thread_session_segment(&threads::SessionSegment {
                    session_segment_id: "seg_preview".to_string(),
                    thread_id: "thr_preview".to_string(),
                    tenant_id: "ten_threads".to_string(),
                    session_id: String::new(),
                    generation: 1,
                    state: "active".to_string(),
                    started_at: now,
                    ended_at: None,
                    last_active_at: now,
                    reset_from_session_segment_id: String::new(),
                    partial_evidence: false,
                })
                .expect("upsert segment");
            let mut items = vec![threads::ContinuityPreviewItem {
                preview_item_id: "contitem_1".to_string(),
                continuity_preview_id: "contprev_1".to_string(),
                tenant_id: "ten_threads".to_string(),
                thread_id: "thr_preview".to_string(),
                item_kind: threads::ContinuityItemKind::Turn,
                continuity_turn_id: "turn_source_1".to_string(),
                role: Some(threads::ContinuityRole::User),
                artifact_ref: String::new(),
                artifact_excerpt_id: String::new(),
                handoff_source_reference_id: String::new(),
                decision: threads::ContinuityDecision::Included,
                reason_code: threads::ContinuityReason::IncludedRecent,
                acceptance_sequence: 1,
                source_timestamp: None,
                safe_summary: "safe preview".to_string(),
                redaction_status: threads::RedactionStatus::Redacted,
                item_order: 0,
            }];
            store
                .save_continuity_preview(
                    threads::ContinuityPreview {
                        continuity_preview_id: "contprev_1".to_string(),
                        tenant_id: "ten_threads".to_string(),
                        thread_id: "thr_preview".to_string(),
                        session_segment_id: "seg_preview".to_string(),
                        dispatch_id: String::new(),
                        request_turn_id: String::new(),
                        response_turn_id: String::new(),
                        window_policy_id: String::new(),
                        max_prior_turns: 0,
                        active_window_days: 0,
                        included_count: 1,
                        excluded_count: 0,
                        continuity_applied: true,
                        status: threads::ContinuityStatus::Applied,
                        failure_class: String::new(),
                        assembly_started_at: now,
                        assembly_completed_at: now,
                        assembly_duration_ms: 0,
                        retention_expires_at: now + Duration::days(90),
                        redaction_status: threads::RedactionStatus::Redacted,
                    },
                    &mut items,
                )
                .expect("save preview");
        }
        let app = crate::routes::router(state.clone());
        let req = tenant_request(
            "GET",
            "/v1/threads/thr_preview/continuity-previews/contprev_1",
            None,
            "ten_threads",
            vec![Permission::CredentialsInspect],
        );
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::OK, "preview body: {json}");
        assert_eq!(json["preview"]["continuityPreviewId"], "contprev_1");
        assert_eq!(json["items"].as_array().map(|v| v.len()), Some(1));

        let req = tenant_request(
            "GET",
            "/v1/threads/thr_preview/continuity-previews/contprev_missing",
            None,
            "ten_threads",
            vec![Permission::CredentialsInspect],
        );
        let (status, _) = send(&app, req).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    // Port of TestAgentProfileAPIRequiresPermissionsAndMutatesProfiles.
    #[tokio::test]
    async fn agent_profile_lifecycle_requires_permissions_and_mutates() {
        let state = test_state();
        let app = crate::routes::router(state.clone());
        let admin = vec![Permission::ProfilesInspect, Permission::ProfilesManage];
        let viewer = vec![Permission::ProfilesInspect];

        let (status, json) = send(
            &app,
            tenant_request("POST", "/v1/profiles", Some(r#"{"displayName":"Support","persona":{"tone":"direct"},"activate":true}"#), "ten_threads", admin.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "create body: {json}");
        let profile_id = json["profile"]["profileId"].as_str().expect("profile id").to_string();

        // List works for the viewer.
        let (status, json) = send(&app, tenant_request("GET", "/v1/profiles", None, "ten_threads", viewer.clone())).await;
        assert_eq!(status, StatusCode::OK, "list body: {json}");
        assert!(json["items"].as_array().map(|v| !v.is_empty()).unwrap_or(false));

        // Update denied without profiles.manage.
        let (status, json) = send(
            &app,
            tenant_request("PATCH", &format!("/v1/profiles/{profile_id}"), Some(r#"{"displayName":"Denied"}"#), "ten_threads", viewer.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "denied update: {json}");

        // Invalid create answers 400 with the safe reason code.
        let (status, json) = send(
            &app,
            tenant_request(
                "POST",
                "/v1/profiles",
                Some(r#"{"displayName":"Invalid","defaultProviderPreference":{"providerId":"bad provider"}}"#),
                "ten_threads",
                admin.clone(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "invalid create: {json}");
        assert!(
            json.to_string().contains("provider_preference_malformed"),
            "expected safe reason code: {json}"
        );

        // Archive succeeds for the admin.
        let (status, json) = send(
            &app,
            tenant_request("POST", &format!("/v1/profiles/{profile_id}/archive"), Some(r#"{"reasonCode":"test_archive"}"#), "ten_threads", admin.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "archive body: {json}");
        assert_eq!(json["profile"]["status"], "archived");

        // Durable profile events: denied update + validation failure.
        let events = state
            .event_bus
            .list(&kura_events::Filter { category: "agent_profile".to_string(), ..Default::default() });
        assert!(
            events.iter().any(|event| event.name == "agent_profile.update_denied"
                && event.payload.get("reasonCode").and_then(|v| v.as_str()) == Some("permission_denied")),
            "expected update_denied event: {events:?}"
        );
        assert!(
            events.iter().any(|event| event.name == "agent_profile.validation_failed"
                && event.payload.get("reasonCode").and_then(|v| v.as_str()) == Some("provider_preference_malformed")),
            "expected validation_failed event: {events:?}"
        );
    }

    // Port of TestAgentProfileAPIVersionsRollbackDisableAndOverlays (subset).
    #[tokio::test]
    async fn agent_profile_versions_rollback_and_disable() {
        let state = test_state();
        let app = crate::routes::router(state.clone());
        let admin = vec![Permission::ProfilesInspect, Permission::ProfilesManage];

        let (status, json) = send(
            &app,
            tenant_request(
                "POST",
                "/v1/profiles",
                Some(r#"{"displayName":"Support","persona":{"tone":"direct"},"overlayReferences":[{"referenceKind":"prompt","referenceUri":"prompt://profiles/support","scope":"profile"}]}"#),
                "ten_threads",
                admin.clone(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "create: {json}");
        let profile_id = json["profile"]["profileId"].as_str().expect("profile id").to_string();
        let first_version_id = json["version"]["profileVersionId"].as_str().expect("version id").to_string();

        // Update creates a second version.
        let (status, json) = send(
            &app,
            tenant_request(
                "PATCH",
                &format!("/v1/profiles/{profile_id}"),
                Some(r#"{"displayName":"Support Updated","persona":{"tone":"calm"}}"#),
                "ten_threads",
                admin.clone(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "update: {json}");

        // Versions list shows both.
        let (status, json) = send(
            &app,
            tenant_request("GET", &format!("/v1/profiles/{profile_id}/versions"), None, "ten_threads", admin.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "versions: {json}");
        assert_eq!(json["items"].as_array().map(|v| v.len()), Some(2));

        // Rollback to the first version.
        let (status, json) = send(
            &app,
            tenant_request(
                "POST",
                &format!("/v1/profiles/{profile_id}/rollback"),
                Some(&format!(r#"{{"sourceProfileVersionId":"{first_version_id}"}}"#)),
                "ten_threads",
                admin.clone(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "rollback: {json}");
        assert_eq!(json["version"]["sourceVersionId"], first_version_id.as_str());

        // Disable the profile.
        let (status, json) = send(
            &app,
            tenant_request("POST", &format!("/v1/profiles/{profile_id}/disable"), Some("{}"), "ten_threads", admin.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "disable: {json}");
        assert_eq!(json["profile"]["status"], "disabled");
    }

    // Thread detail attaches bindingProjection only for bindings.inspect callers.
    #[tokio::test]
    async fn thread_detail_binding_projection_gated_by_permission() {
        let state = test_state();
        let now = chrono::Utc::now() - Duration::minutes(5);
        seed_thread(&state, &thread("thr_binding", "ten_threads", threads::LifecycleState::Active, threads::SourceKind::Channel, "Slack / #support", now, now));
        {
            let store = state.store.lock();
            store
                .record_runtime_binding_evidence(kura_bindings::RuntimeBindingEvidence {
                    tenant_id: "ten_threads".to_string(),
                    resource_kind: "thread".to_string(),
                    resource_id: "thr_binding".to_string(),
                    binding_scope: kura_bindings::BindingRuntimeScope::CHANNEL,
                    classification: kura_bindings::Classification::APPLIED,
                    selection_reason: "explicit_binding_selection".to_string(),
                    occurred_at: now,
                    redaction_status: kura_bindings::RedactionStatus::REDACTED,
                    ..Default::default()
                })
                .expect("record binding evidence");
        }
        let app = crate::routes::router(state.clone());

        // Without bindings.inspect there is no projection field.
        let (status, json) = send(
            &app,
            tenant_request("GET", "/v1/threads/thr_binding", None, "ten_threads", vec![Permission::CredentialsInspect]),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "detail body: {json}");
        assert!(json.get("bindingProjection").is_none(), "leaked projection: {json}");

        // With bindings.inspect the additive projection is attached.
        let (status, json) = send(
            &app,
            tenant_request(
                "GET",
                "/v1/threads/thr_binding",
                None,
                "ten_threads",
                vec![Permission::CredentialsInspect, Permission::BindingsInspect],
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "detail body: {json}");
        assert_eq!(json["bindingProjection"]["resourceKind"], "thread");
        assert_eq!(json["bindingProjection"]["resourceId"], "thr_binding");
    }
}
