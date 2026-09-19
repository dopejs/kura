//! sessions route family (port of the /v1/sessions handlers in Go
//! daemon/internal/api/server.go).
//!
//! Routes: GET /v1/sessions, GET /v1/sessions/{session_id},
//! POST /v1/sessions/{session_id}/reset, GET /v1/sessions/{session_id}/events.
//! Reset persists the bumped session and publishes `session.reset`; the
//! events sub-route reuses the shared cursor/filter helpers from the runs
//! family.
//!
//! Tenant isolation (Roadmap 75): when the protected() middleware resolves a
//! tenant context, the list is filtered to sessions persisted under the
//! caller's tenant (Go filterSessionsByTenant).
//!
//! Deliberately not ported (documented divergence, matching the wave-8
//! conventions in workflows.rs):
//! - the Go by-id tenant guard middleware for sub-routes (Roadmap 35)
//! - Go projectSessionProfileProjections: the store profile-projection reads
//!   are not ported; Go is a no-op when the projection is absent.

use std::collections::HashMap;

use axum::extract::{Extension, Path, Query, State};
use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;

use kura_events as events;
use kura_router as router_domain;

use crate::error::ApiError;
use crate::middleware::{TenantContext, environment_scope_from_config};
use crate::state::AppState;
use crate::types::EventListResponse;

use super::runs::{build_event_list_response, parse_event_cursor, read_events};

/// Route family router.
#[must_use]
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/sessions", get(list_sessions))
        .route("/v1/sessions/{session_id}", get(get_session))
        .route("/v1/sessions/{session_id}/reset", post(reset_session))
        .route("/v1/sessions/{session_id}/events", get(session_events))
}

#[derive(Debug, Serialize)]
struct SessionListResponse {
    items: Vec<router_domain::Session>,
}

fn session_router(state: &AppState) -> Result<&router_domain::SessionRouter, ApiError> {
    state
        .router
        .as_deref()
        .ok_or_else(|| ApiError::internal("session router is not configured"))
}

/// GET /v1/sessions (Go handleSessions).
async fn list_sessions(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Json<SessionListResponse>, ApiError> {
    let router = session_router(&state)?;
    let mut items = router.list_sessions();
    // Go filterSessionsByTenant: scope the in-memory router enumeration to
    // the caller's tenant via the store's tenant_id column.
    if let Some(tc) = tenant.as_ref().map(|extension| &extension.0.0) {
        if !tc.tenant_id.trim().is_empty() {
            let owned: std::collections::HashSet<String> = state
                .store_pool
                .read()
                .list_sessions_for_tenant_raw(&tc.tenant_id)
                .map_err(ApiError::from_store)?
                .into_iter()
                .map(|session| session.session_id)
                .collect();
            items.retain(|session| owned.contains(&session.session_id));
        }
    }
    Ok(Json(SessionListResponse { items }))
}

/// GET /v1/sessions/{session_id} (Go handleSessionByID).
async fn get_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<router_domain::Session>, ApiError> {
    let router = session_router(&state)?;
    router
        .get_session(session_id.trim())
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("not found".to_string()))
}

/// POST /v1/sessions/{session_id}/reset (Go handleSessionReset): bump the
/// session generation, persist it, publish `session.reset`.
async fn reset_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<router_domain::Session>, ApiError> {
    let router = session_router(&state)?;
    let session = router
        .reset_session(session_id.trim())
        .map_err(|err| match err {
            router_domain::RouterError::SessionNotFound => {
                ApiError::NotFound("not found".to_string())
            }
            other => ApiError::BadRequest(other.to_string()),
        })?;

    state
        .store
        .lock()
        .upsert_session(&session)
        .map_err(ApiError::from_store)?;

    let mut payload = serde_json::Map::new();
    payload.insert(
        "generation".to_string(),
        serde_json::json!(session.generation),
    );
    let event = events::Event {
        category: "session".to_string(),
        name: "session.reset".to_string(),
        environment_scope: environment_scope_from_config(&state.config),
        scope: events::Scope {
            session_id: session.session_id.clone(),
            ..events::Scope::default()
        },
        resource: events::Resource {
            kind: "session".to_string(),
            id: session.session_id.clone(),
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

    Ok(Json(session))
}

/// GET /v1/sessions/{session_id}/events (Go handleSessionEvents): the event
/// ledger filtered to the session, with the shared cursor semantics.
async fn session_events(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Json<EventListResponse>, ApiError> {
    let cursor = parse_event_cursor(&params, &headers)?;
    let filter = events::Filter {
        environment_scope: environment_scope_from_config(&state.config),
        session_id: session_id.trim().to_string(),
        cursor,
        ..events::Filter::default()
    };
    let items = read_events(&state, &filter)?;
    Ok(Json(build_event_list_response(items)))
}

#[cfg(test)]
mod tests {
    use super::super::tests_support::{request_json, test_state};
    use axum::http::StatusCode;
    use std::sync::Arc;

    fn state_with_router() -> (crate::state::AppState, String) {
        let router = kura_router::SessionRouter::new();
        let (session, _) = router
            .route(kura_router::RouteInput {
                kind: kura_router::SessionKind::Direct,
                channel: "api".to_string(),
                peer_id: "peer_a".to_string(),
                ..kura_router::RouteInput::default()
            })
            .expect("route session");
        let mut state = test_state();
        state.router = Some(Arc::new(router));
        (state, session.session_id)
    }

    #[tokio::test]
    async fn list_get_reset_and_events() {
        let (state, session_id) = state_with_router();
        let (status, listed) = request_json(state.clone(), "GET", "/v1/sessions", None).await;
        assert_eq!(status, StatusCode::OK, "{listed}");
        assert_eq!(listed["items"].as_array().expect("items").len(), 1);

        let (status, fetched) = request_json(
            state.clone(),
            "GET",
            &format!("/v1/sessions/{session_id}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(fetched["sessionId"], session_id.as_str());

        let (status, reset) = request_json(
            state.clone(),
            "POST",
            &format!("/v1/sessions/{session_id}/reset"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{reset}");
        assert_eq!(reset["generation"], 2);

        let (status, events) = request_json(
            state,
            "GET",
            &format!("/v1/sessions/{session_id}/events"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{events}");
        let items = events["items"].as_array().expect("items");
        assert!(
            items.iter().any(|event| event["name"] == "session.reset"),
            "{events}"
        );
    }

    #[tokio::test]
    async fn missing_session_is_404() {
        let (state, _) = state_with_router();
        let (status, _) =
            request_json(state.clone(), "GET", "/v1/sessions/sess_missing", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) =
            request_json(state, "POST", "/v1/sessions/sess_missing/reset", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
