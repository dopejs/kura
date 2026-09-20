//! `/v1/threads/{thread_id}/frame` — session frames (Stage 5.1).
//!
//! A frame is an explicit, durable record of what a thread is for: a goal and
//! constraints. The session-strategy plugin renders it as a system message on
//! every turn, and system messages are what window elision never touches — so
//! the goal survives however much history is evicted, instead of depending on
//! where in the transcript it was last said.
//!
//! **Tenancy.** Frames are manager documents keyed by thread id; ownership
//! lives on the document row and is checked before every read and write.

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::routing::get;

use crate::error::ApiError;
use crate::middleware::TenantContext;
use crate::response::Json;
use crate::state::AppState;

use super::{bind_document_tenant, decode_json_required, guard_document_tenant};

const DOC_KIND: &str = kura_session::DOC_KIND_SESSION_FRAME;

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct FrameWriteRequest {
    goal: String,
    constraints: Vec<String>,
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/v1/threads/{thread_id}/frame",
        get(get_frame).put(put_frame).delete(delete_frame),
    )
}

fn acting_tenant(tenant: Option<&TenantContext>) -> String {
    tenant
        .map(|tc| tc.0.tenant_id.trim().to_string())
        .unwrap_or_default()
}

/// Loads a thread's frame; `None` when the thread has no frame.
pub fn load_frame(
    state: &AppState,
    tenant_id: &str,
    thread_id: &str,
) -> Result<Option<kura_session::SessionFrame>, ApiError> {
    let docs = state
        .store_pool
        .read()
        .list_manager_documents(DOC_KIND)
        .map_err(ApiError::from_store)?;
    let Some(doc) = docs.into_iter().find(|d| d.doc_id == thread_id.trim()) else {
        return Ok(None);
    };
    // Ownership: a frame written under another tenant is invisible here.
    // Pre-tenancy rows (empty) stay readable, matching the by-id convention.
    if !doc.tenant_id.is_empty() && !tenant_id.is_empty() && doc.tenant_id != tenant_id {
        return Ok(None);
    }
    serde_json::from_str::<kura_session::SessionFrame>(&doc.document_json)
        .map(Some)
        .map_err(|err| ApiError::internal(&format!("decode session frame {thread_id}: {err}")))
}

async fn get_frame(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(thread_id): Path<String>,
) -> Result<Json<kura_session::SessionFrame>, ApiError> {
    let tenant_id = acting_tenant(tenant.as_ref().map(|e| &e.0));
    guard_document_tenant(
        &state,
        tenant.as_ref().map(|e| &e.0),
        DOC_KIND,
        thread_id.trim(),
    )?;
    load_frame(&state, &tenant_id, &thread_id)?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("not found".to_string()))
}

async fn put_frame(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(thread_id): Path<String>,
    body: Bytes,
) -> Result<Json<kura_session::SessionFrame>, ApiError> {
    let request: FrameWriteRequest = decode_json_required(&body)?;
    if request.goal.trim().is_empty() {
        return Err(ApiError::BadRequest("goal is required".to_string()));
    }
    let tenant_ref = tenant.as_ref().map(|e| &e.0);
    let tenant_id = acting_tenant(tenant_ref);
    guard_document_tenant(&state, tenant_ref, DOC_KIND, thread_id.trim())?;
    let frame = kura_session::SessionFrame {
        thread_id: thread_id.trim().to_string(),
        tenant_id: tenant_id.clone(),
        goal: request.goal.trim().to_string(),
        constraints: request
            .constraints
            .into_iter()
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty())
            .collect(),
        updated_at: chrono::Utc::now(),
    };
    {
        let store = state.store.lock();
        kura_store::put_document(&store, DOC_KIND, &frame.thread_id, "", &tenant_id, &frame)
            .map_err(ApiError::from_store)?;
    }
    bind_document_tenant(&state, tenant_ref, DOC_KIND, &frame.thread_id)?;
    Ok(Json(frame))
}

async fn delete_frame(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(thread_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let tenant_ref = tenant.as_ref().map(|e| &e.0);
    guard_document_tenant(&state, tenant_ref, DOC_KIND, thread_id.trim())?;
    if load_frame(&state, &acting_tenant(tenant_ref), &thread_id)?.is_none() {
        return Err(ApiError::NotFound("not found".to_string()));
    }
    let store = state.store.lock();
    kura_store::delete_document(&store, DOC_KIND, thread_id.trim())
        .map_err(ApiError::from_store)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::super::tests_support::{request_json, request_json_as_tenant, test_state};
    use axum::http::StatusCode;

    #[tokio::test]
    async fn put_get_delete_round_trip() {
        let state = test_state();
        let (status, frame) = request_json(
            state.clone(), "PUT", "/v1/threads/thr_1/frame",
            Some(serde_json::json!({ "goal": " ship it ", "constraints": ["no sends", " ", "cite"] })),
        ).await;
        assert_eq!(status, StatusCode::OK, "{frame}");
        assert_eq!(frame["goal"], "ship it");
        assert_eq!(
            frame["constraints"],
            serde_json::json!(["no sends", "cite"])
        );

        let (status, read) =
            request_json(state.clone(), "GET", "/v1/threads/thr_1/frame", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(read["goal"], "ship it");

        let (status, _) =
            request_json(state.clone(), "DELETE", "/v1/threads/thr_1/frame", None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, _) = request_json(state.clone(), "GET", "/v1/threads/thr_1/frame", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_missing_goal_is_a_400() {
        let state = test_state();
        let (status, _) = request_json(
            state.clone(),
            "PUT",
            "/v1/threads/thr_1/frame",
            Some(serde_json::json!({ "goal": "  " })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn frames_are_scoped_to_the_acting_tenant() {
        let state = test_state();
        let (status, _) = request_json_as_tenant(
            state.clone(),
            "tnt_a",
            "PUT",
            "/v1/threads/thr_1/frame",
            Some(serde_json::json!({ "goal": "a's goal" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        for method in ["GET", "DELETE"] {
            let (status, _) = request_json_as_tenant(
                state.clone(),
                "tnt_b",
                method,
                "/v1/threads/thr_1/frame",
                None,
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{method}");
        }
        let (status, _) = request_json_as_tenant(
            state.clone(),
            "tnt_b",
            "PUT",
            "/v1/threads/thr_1/frame",
            Some(serde_json::json!({ "goal": "stolen" })),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "another tenant cannot overwrite"
        );
        let (status, read) = request_json_as_tenant(
            state.clone(),
            "tnt_a",
            "GET",
            "/v1/threads/thr_1/frame",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(read["goal"], "a's goal");
    }
}
