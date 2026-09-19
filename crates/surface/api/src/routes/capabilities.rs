//! capabilities route family (port of the /v1/capabilities handlers in Go
//! daemon/internal/api/server.go, Roadmap 2 supervision plane).
//!
//! Routes: GET/POST /v1/capabilities, GET /v1/capabilities/{capability_id},
//! POST /v1/capabilities/{capability_id}/{health|fail|restart}. Mutations
//! persist the capability and publish the matching supervision event
//! (capability.registered / health_changed / failure_reported /
//! restart_scheduled).

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use kura_capabilities as capabilities;
use kura_events as events;

use crate::error::ApiError;
use crate::middleware::environment_scope_from_config;
use crate::middleware::{TenantContext, require_daemon_global_operator};
use crate::state::AppState;

use super::decode_json_required;

/// Route family router.
#[must_use]
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/v1/capabilities",
            get(list_capabilities).post(register_capability),
        )
        .route("/v1/capabilities/{capability_id}", get(get_capability))
        .route(
            "/v1/capabilities/{capability_id}/{action}",
            axum::routing::post(capability_action),
        )
}

#[derive(Debug, Serialize)]
struct CapabilityListResponse {
    items: Vec<capabilities::Capability>,
}

fn supervisor(state: &AppState) -> Result<&capabilities::Supervisor, ApiError> {
    state
        .capabilities
        .as_deref()
        .ok_or_else(|| ApiError::internal("capability supervisor is not configured"))
}

fn map_supervisor_error(err: capabilities::SupervisorError) -> ApiError {
    match err {
        capabilities::SupervisorError::CapabilityNotFound => {
            ApiError::NotFound("not found".to_string())
        }
        other => ApiError::BadRequest(other.to_string()),
    }
}

fn persist_capability(
    state: &AppState,
    capability: &capabilities::Capability,
) -> Result<(), ApiError> {
    state
        .store
        .lock()
        .upsert_capability(capability)
        .map_err(ApiError::from_store)
}

fn publish_capability_event(
    state: &AppState,
    name: &str,
    capability: &capabilities::Capability,
    payload: serde_json::Map<String, serde_json::Value>,
) -> Result<(), ApiError> {
    let event = events::Event {
        category: "capability".to_string(),
        name: name.to_string(),
        environment_scope: environment_scope_from_config(&state.config),
        scope: events::Scope {
            capability_id: capability.capability_id.clone(),
            ..events::Scope::default()
        },
        resource: events::Resource {
            kind: "capability".to_string(),
            id: capability.capability_id.clone(),
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

fn status_json(capability: &capabilities::Capability) -> serde_json::Value {
    serde_json::json!(capability.status.as_str())
}

/// GET /v1/capabilities (Go handleCapabilities GET branch).
async fn list_capabilities(
    State(state): State<AppState>,
) -> Result<Json<CapabilityListResponse>, ApiError> {
    let supervisor = supervisor(&state)?;
    Ok(Json(CapabilityListResponse {
        items: supervisor.list(),
    }))
}

/// POST /v1/capabilities (Go handleCapabilities POST branch) — 201 on first
/// registration, 200 on re-registration.
async fn register_capability(
    State(state): State<AppState>,
    tenant: Option<axum::extract::Extension<TenantContext>>,
    body: Bytes,
) -> Result<(StatusCode, Json<capabilities::Capability>), ApiError> {
    require_daemon_global_operator(tenant.as_ref().map(|e| &e.0), "POST /v1/capabilities")?;
    let input: capabilities::RegisterInput = decode_json_required(&body)?;
    let supervisor = supervisor(&state)?;
    let (capability, created) = supervisor.register(input).map_err(map_supervisor_error)?;
    persist_capability(&state, &capability)?;
    let mut payload = serde_json::Map::new();
    payload.insert("kind".to_string(), serde_json::json!(capability.kind));
    payload.insert("status".to_string(), status_json(&capability));
    payload.insert("created".to_string(), serde_json::json!(created));
    payload.insert(
        "displayName".to_string(),
        serde_json::json!(capability.display_name),
    );
    publish_capability_event(&state, "capability.registered", &capability, payload)?;
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(capability)))
}

/// GET /v1/capabilities/{capability_id} (Go handleCapabilityByID).
async fn get_capability(
    State(state): State<AppState>,
    Path(capability_id): Path<String>,
) -> Result<Json<capabilities::Capability>, ApiError> {
    let supervisor = supervisor(&state)?;
    supervisor
        .get(capability_id.trim())
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("not found".to_string()))
}

/// POST /v1/capabilities/{capability_id}/{health|fail|restart} (Go
/// handleCapabilityHealth / Fail / Restart); unknown actions are 404.
async fn capability_action(
    State(state): State<AppState>,
    Path((capability_id, action)): Path<(String, String)>,
    body: Bytes,
) -> Result<Json<capabilities::Capability>, ApiError> {
    let supervisor = supervisor(&state)?;
    let capability_id = capability_id.trim();
    let (capability, event_name, payload) = match action.as_str() {
        "health" => {
            let input: capabilities::ReportHealthInput = decode_json_required(&body)?;
            let capability = supervisor
                .report_health(capability_id, input)
                .map_err(map_supervisor_error)?;
            let mut payload = serde_json::Map::new();
            payload.insert("status".to_string(), status_json(&capability));
            (capability, "capability.health_changed", payload)
        }
        "fail" => {
            let input: capabilities::ReportFailureInput = decode_json_required(&body)?;
            let capability = supervisor
                .report_failure(capability_id, input)
                .map_err(map_supervisor_error)?;
            let mut payload = serde_json::Map::new();
            payload.insert("status".to_string(), status_json(&capability));
            payload.insert(
                "failureCount".to_string(),
                serde_json::json!(capability.failure_count),
            );
            payload.insert(
                "backoffSeconds".to_string(),
                serde_json::json!(capability.backoff_seconds),
            );
            payload.insert(
                "reason".to_string(),
                serde_json::json!(capability.last_failure_reason),
            );
            (capability, "capability.failure_reported", payload)
        }
        "restart" => {
            let capability = supervisor
                .restart(capability_id)
                .map_err(map_supervisor_error)?;
            let mut payload = serde_json::Map::new();
            payload.insert("status".to_string(), status_json(&capability));
            payload.insert(
                "restartCount".to_string(),
                serde_json::json!(capability.restart_count),
            );
            (capability, "capability.restart_scheduled", payload)
        }
        _ => return Err(ApiError::NotFound("not found".to_string())),
    };
    persist_capability(&state, &capability)?;
    publish_capability_event(&state, event_name, &capability, payload)?;
    Ok(Json(capability))
}

#[cfg(test)]
mod tests {
    use super::super::tests_support::{request_json, test_state};
    use axum::http::StatusCode;
    use std::sync::Arc;

    fn state_with_supervisor() -> crate::state::AppState {
        let mut state = test_state();
        state.capabilities = Some(Arc::new(kura_capabilities::Supervisor::new()));
        state
    }

    #[tokio::test]
    async fn register_list_get_fail_and_restart() {
        let state = state_with_supervisor();
        let (status, registered) = request_json(
            state.clone(),
            "POST",
            "/v1/capabilities",
            Some(serde_json::json!({
                "capabilityId": "cap_browser",
                "kind": "browser",
                "displayName": "Browser"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{registered}");

        let (status, listed) = request_json(state.clone(), "GET", "/v1/capabilities", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(listed["items"].as_array().expect("items").len(), 1);

        let (status, _) =
            request_json(state.clone(), "GET", "/v1/capabilities/cap_browser", None).await;
        assert_eq!(status, StatusCode::OK);

        let (status, failed) = request_json(
            state.clone(),
            "POST",
            "/v1/capabilities/cap_browser/fail",
            Some(serde_json::json!({ "reason": "probe timeout" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{failed}");
        assert_eq!(failed["failureCount"], 1);

        let (status, restarted) = request_json(
            state.clone(),
            "POST",
            "/v1/capabilities/cap_browser/restart",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{restarted}");

        let (status, _) = request_json(state, "GET", "/v1/capabilities/cap_missing", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
