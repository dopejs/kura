//! routines route family (port of daemon/internal/api/routine.go, Roadmap 66).
//!
//! Routes: GET/POST /v1/routines, POST /v1/routines/preview,
//! GET|PUT /v1/routines/{routine_id}, and POST
//! /v1/routines/{routine_id}/{pause|resume|cancel|repair}. Routines are
//! explicit configuration compiled onto the scheduler; preview is a dry-run
//! that references no existing routine. Error mapping preserves Go
//! writeRoutineError: not-found -> 404, invalid definition / cancelled -> 400,
//! scheduler compile/repair failures -> 500.

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use kura_routine as routine;

use crate::error::ApiError;
use crate::middleware::TenantContext;
use crate::state::AppState;

use super::{
    bind_document_tenant, decode_json_required, guard_document_tenant, tenant_visible_document_ids,
};

/// Manager-document kind backing this family; ownership lives on that row.
const DOC_KIND: &str = routine::DOC_KIND_ROUTINE;

/// Route family router. `/v1/routines/preview` is registered as a static
/// route so it wins over the `{routine_id}` capture (the Go handler special-
/// cases the "preview" path segment).
#[must_use]
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/routines", get(list_routines).post(create_routine))
        .route("/v1/routines/preview", post(preview_routine))
        .route(
            "/v1/routines/{routine_id}",
            get(get_routine).put(update_routine),
        )
        .route("/v1/routines/{routine_id}/{action}", post(routine_action))
}

/// Go RoutineRequest — create, update, or preview payload.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct RoutineRequest {
    definition: routine::Definition,
}

#[derive(Debug, Serialize)]
struct RoutineListResponse {
    items: Vec<routine::Routine>,
}

fn manager(state: &AppState) -> Result<&routine::Manager, ApiError> {
    state
        .routines
        .as_deref()
        .ok_or_else(|| ApiError::internal("routine manager is not configured"))
}

fn map_routine_error(err: routine::RoutineError) -> ApiError {
    let message = err.to_string();
    match err {
        routine::RoutineError::RoutineNotFound => ApiError::NotFound(message),
        routine::RoutineError::RoutineCancelled
        | routine::RoutineError::InvalidNameRequired
        | routine::RoutineError::InvalidGoalRequired
        | routine::RoutineError::InvalidCronExprRequired
        | routine::RoutineError::InvalidFireAtRequired
        | routine::RoutineError::InvalidTriggerKind => ApiError::BadRequest(message),
        routine::RoutineError::CompileSchedule(_) | routine::RoutineError::RepairSchedule(_) => {
            ApiError::Internal(message)
        }
    }
}

/// GET /v1/routines (Go handleRoutines GET branch).
async fn list_routines(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Json<RoutineListResponse>, ApiError> {
    let manager = manager(&state)?;
    let visible = tenant_visible_document_ids(&state, tenant.as_ref().map(|e| &e.0), DOC_KIND)?;
    let items = manager
        .list()
        .into_iter()
        .filter(|routine| match &visible {
            Some(ids) => ids.contains(routine.routine_id.trim()),
            None => true,
        })
        .collect();
    Ok(Json(RoutineListResponse { items }))
}

/// POST /v1/routines (Go handleRoutines POST branch) — 201.
async fn create_routine(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<(StatusCode, Json<routine::Routine>), ApiError> {
    let request: RoutineRequest = decode_json_required(&body)?;
    let manager = manager(&state)?;
    let created = manager
        .create(request.definition)
        .map_err(map_routine_error)?;
    bind_document_tenant(
        &state,
        tenant.as_ref().map(|e| &e.0),
        DOC_KIND,
        created.routine_id.trim(),
    )?;
    Ok((StatusCode::CREATED, Json(created)))
}

/// POST /v1/routines/preview (Go handleRoutineRoutes preview branch) — a
/// dry-run compilation that touches no scheduler state.
async fn preview_routine(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<routine::Preview>, ApiError> {
    let request: RoutineRequest = decode_json_required(&body)?;
    let manager = manager(&state)?;
    let preview = manager
        .preview(&request.definition)
        .map_err(map_routine_error)?;
    Ok(Json(preview))
}

/// GET /v1/routines/{routine_id} (Go handleRoutineRoutes get branch).
async fn get_routine(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(routine_id): Path<String>,
) -> Result<Json<routine::Routine>, ApiError> {
    let manager = manager(&state)?;
    guard_document_tenant(
        &state,
        tenant.as_ref().map(|e| &e.0),
        DOC_KIND,
        routine_id.trim(),
    )?;
    manager
        .get(routine_id.trim())
        .map(Json)
        .ok_or_else(|| map_routine_error(routine::RoutineError::RoutineNotFound))
}

/// PUT /v1/routines/{routine_id} (Go handleRoutineRoutes update branch).
async fn update_routine(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(routine_id): Path<String>,
    body: Bytes,
) -> Result<Json<routine::Routine>, ApiError> {
    let request: RoutineRequest = decode_json_required(&body)?;
    let manager = manager(&state)?;
    let tenant = tenant.as_ref().map(|e| &e.0);
    guard_document_tenant(&state, tenant, DOC_KIND, routine_id.trim())?;
    let updated = manager
        .update(routine_id.trim(), request.definition)
        .map_err(map_routine_error)?;
    // The manager rewrites the document (with an empty tenant) on every save,
    // so ownership is re-bound after each mutation.
    bind_document_tenant(&state, tenant, DOC_KIND, updated.routine_id.trim())?;
    Ok(Json(updated))
}

/// POST /v1/routines/{routine_id}/{pause|resume|cancel|repair} (Go
/// handleRoutineRoutes action branch); unknown actions are 404 like the Go
/// http.NotFound fallthrough.
async fn routine_action(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path((routine_id, action)): Path<(String, String)>,
) -> Result<Json<routine::Routine>, ApiError> {
    let manager = manager(&state)?;
    let tenant = tenant.as_ref().map(|e| &e.0);
    let routine_id = routine_id.trim();
    guard_document_tenant(&state, tenant, DOC_KIND, routine_id)?;
    let result = match action.as_str() {
        "pause" => manager.pause(routine_id),
        "resume" => manager.resume(routine_id),
        "cancel" => manager.cancel(routine_id),
        "repair" => manager.repair(routine_id),
        _ => return Err(ApiError::NotFound("not found".to_string())),
    };
    let updated = result.map_err(map_routine_error)?;
    bind_document_tenant(&state, tenant, DOC_KIND, updated.routine_id.trim())?;
    Ok(Json(updated))
}

#[cfg(test)]
mod tests {
    use super::super::tests_support::{request_json, request_json_as_tenant, test_state};
    use axum::http::StatusCode;
    use std::sync::Arc;

    /// Scheduler fake mirroring the Go test scheduler: every call succeeds
    /// with a paused/resumed/cancelled schedule echo.
    struct FakeScheduler;

    impl kura_routine::Scheduler for FakeScheduler {
        fn create(
            &self,
            input: &kura_routine::CreateInput,
        ) -> Result<kura_routine::Schedule, String> {
            Ok(kura_routine::Schedule {
                schedule_id: "sched_fake_1".to_string(),
                trigger: input.trigger.clone(),
                target: input.target.clone(),
                retry_policy: input.retry_policy.clone(),
                ..kura_routine::Schedule::default()
            })
        }

        fn pause(&self, schedule_id: &str) -> Result<(kura_routine::Schedule, bool), String> {
            Ok((echo(schedule_id), true))
        }

        fn resume(&self, schedule_id: &str) -> Result<(kura_routine::Schedule, bool), String> {
            Ok((echo(schedule_id), true))
        }

        fn cancel(&self, schedule_id: &str) -> Result<(kura_routine::Schedule, bool), String> {
            Ok((echo(schedule_id), true))
        }

        fn get(&self, schedule_id: &str) -> Result<(kura_routine::Schedule, bool), String> {
            Ok((echo(schedule_id), true))
        }
    }

    fn echo(schedule_id: &str) -> kura_routine::Schedule {
        kura_routine::Schedule {
            schedule_id: schedule_id.to_string(),
            ..kura_routine::Schedule::default()
        }
    }

    fn state_with_manager() -> crate::state::AppState {
        let mut state = test_state();
        // Mirror the production assembly (`plugins.rs:1171`): without the
        // store handle the manager never writes through, and tenant ownership
        // has nothing to attach to.
        let mut manager = kura_routine::Manager::new("test", Box::new(FakeScheduler));
        manager.with_store(state.store.clone());
        state.routines = Some(Arc::new(manager));
        state
    }

    fn definition_body() -> serde_json::Value {
        serde_json::json!({
            "definition": {
                "name": "morning briefing",
                "trigger": { "kind": "cron", "cronExpr": "0 8 * * *" },
                "workflow": { "goal": "summarize the inbox" }
            }
        })
    }

    #[tokio::test]
    async fn preview_create_get_and_pause_routine() {
        let state = state_with_manager();
        let (status, preview) = request_json(
            state.clone(),
            "POST",
            "/v1/routines/preview",
            Some(definition_body()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{preview}");

        let (status, created) = request_json(
            state.clone(),
            "POST",
            "/v1/routines",
            Some(definition_body()),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let routine_id = created["routineId"]
            .as_str()
            .expect("routineId")
            .to_string();

        let (status, fetched) = request_json(
            state.clone(),
            "GET",
            &format!("/v1/routines/{routine_id}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(fetched["routineId"], routine_id.as_str());

        let (status, paused) = request_json(
            state,
            "POST",
            &format!("/v1/routines/{routine_id}/pause"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{paused}");
    }

    #[tokio::test]
    async fn invalid_definition_is_400_and_missing_routine_is_404() {
        let state = state_with_manager();
        let (status, body) = request_json(
            state.clone(),
            "POST",
            "/v1/routines",
            Some(serde_json::json!({
                "definition": {
                    "name": "",
                    "trigger": { "kind": "cron", "cronExpr": "0 8 * * *" },
                    "workflow": { "goal": "g" }
                }
            })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

        let (status, _) =
            request_json(state.clone(), "GET", "/v1/routines/routine_missing", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, _) = request_json(
            state,
            "POST",
            "/v1/routines/routine_missing/frobnicate",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// Routines live in a daemon-wide in-memory manager; ownership is carried
    /// by the backing `manager_documents` row. One tenant must neither
    /// enumerate nor address another's.
    #[tokio::test]
    async fn routines_are_scoped_to_the_acting_tenant() {
        let state = state_with_manager();

        let (status, created) = request_json_as_tenant(
            state.clone(),
            "tnt_a",
            "POST",
            "/v1/routines",
            Some(definition_body()),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let routine_id = created["routineId"]
            .as_str()
            .expect("routineId")
            .to_string();

        let (status, listed) =
            request_json_as_tenant(state.clone(), "tnt_a", "GET", "/v1/routines", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(listed["items"].as_array().expect("items").len(), 1);

        let (status, listed) =
            request_json_as_tenant(state.clone(), "tnt_b", "GET", "/v1/routines", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            listed["items"].as_array().expect("items").is_empty(),
            "tenant b must not enumerate tenant a's routines: {listed}"
        );

        let (status, _) = request_json_as_tenant(
            state.clone(),
            "tnt_b",
            "GET",
            &format!("/v1/routines/{routine_id}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // Lifecycle actions are guarded on the same check.
        let (status, _) = request_json_as_tenant(
            state.clone(),
            "tnt_b",
            "POST",
            &format!("/v1/routines/{routine_id}/pause"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // The owner keeps working, and ownership survives a mutation (the
        // manager rewrites the document with an empty tenant on every save).
        let (status, _) = request_json_as_tenant(
            state.clone(),
            "tnt_a",
            "POST",
            &format!("/v1/routines/{routine_id}/pause"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, listed) =
            request_json_as_tenant(state.clone(), "tnt_a", "GET", "/v1/routines", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(listed["items"].as_array().expect("items").len(), 1);
    }

    /// Single-user assembly: no tenant, no filtering.
    #[tokio::test]
    async fn routines_without_a_tenant_are_unfiltered() {
        let state = state_with_manager();
        let (status, _) = request_json(
            state.clone(),
            "POST",
            "/v1/routines",
            Some(definition_body()),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let (status, listed) = request_json(state.clone(), "GET", "/v1/routines", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(listed["items"].as_array().expect("items").len(), 1);
    }
}
