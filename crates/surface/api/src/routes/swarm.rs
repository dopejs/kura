//! `/v1/swarm/runs` — bounded concurrent sub-agents (Stage 4).
//!
//! Execution lives here, not in `kura-swarm`: a child turn *is* a
//! `chat::Service::query` on its own thread (`swarm:<run>:<index>`), so it
//! passes through every chat hook and its dispatch is persisted exactly like a
//! user turn. What this route adds around each child, in order:
//!
//! 1. **quota reserve** through the billing plane (`RUN_LAUNCHES`) before the
//!    child is spawned — a fan-out multiplies spend, and the bound comes
//!    first;
//! 2. the child turn, on a worker bounded by `maxConcurrentChildren`;
//! 3. **commit on success, release on failure** — a child that produced no
//!    answer must not consume the tenant's budget;
//! 4. a progress event per transition, so a running fan-out is inspectable
//!    rather than a silent gap.
//!
//! Opt-in: the plugin ships disabled; every launch is refused until
//! `swarm.config.enabled = true`.

use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;

use crate::error::ApiError;
use crate::middleware::{AuthenticatedToken, TenantContext};
use crate::response::Json;
use crate::state::AppState;

use super::{
    bind_document_tenant, decode_json_required, guard_document_tenant, tenant_visible_document_ids,
};

const DOC_KIND: &str = kura_swarm::DOC_KIND_SWARM_RUN;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/swarm/runs", get(list_runs).post(launch_run))
        .route("/v1/swarm/runs/{run_id}", get(get_run))
}

#[derive(Debug, serde::Serialize)]
struct RunListResponse {
    items: Vec<kura_swarm::SwarmRun>,
}

fn manager(state: &AppState) -> Result<Arc<kura_swarm::Manager>, ApiError> {
    state
        .swarm
        .clone()
        .ok_or_else(|| ApiError::internal("swarm manager is not configured"))
}

fn acting_tenant(tenant: Option<&TenantContext>) -> String {
    tenant
        .map(|tc| tc.0.tenant_id.trim().to_string())
        .unwrap_or_default()
}

fn persist(state: &AppState, run: &kura_swarm::SwarmRun) {
    let store = state.store.lock();
    if let Err(err) =
        kura_store::put_document(&store, DOC_KIND, &run.run_id, "", &run.tenant_id, run)
    {
        eprintln!("[kura] swarm: persisting run {} failed: {err}", run.run_id);
    }
}

fn publish(
    state: &AppState,
    name: &str,
    run: &kura_swarm::SwarmRun,
    child: Option<&kura_swarm::SwarmChild>,
) {
    let mut payload = serde_json::Map::new();
    payload.insert(
        "runId".into(),
        serde_json::Value::String(run.run_id.clone()),
    );
    payload.insert(
        "status".into(),
        serde_json::to_value(run.status).unwrap_or_default(),
    );
    payload.insert(
        "summary".into(),
        serde_json::to_value(run.summary()).unwrap_or_default(),
    );
    if let Some(c) = child {
        payload.insert("childIndex".into(), serde_json::json!(c.index));
        payload.insert(
            "childStatus".into(),
            serde_json::to_value(c.status).unwrap_or_default(),
        );
        payload.insert(
            "threadId".into(),
            serde_json::Value::String(c.thread_id.clone()),
        );
        if !c.dispatch_id.is_empty() {
            payload.insert(
                "dispatchId".into(),
                serde_json::Value::String(c.dispatch_id.clone()),
            );
        }
        if !c.error.is_empty() {
            payload.insert("error".into(), serde_json::Value::String(c.error.clone()));
        }
    }
    let event = kura_events::Event {
        category: "swarm".into(),
        name: name.into(),
        tenant_id: run.tenant_id.clone(),
        resource: kura_events::Resource {
            kind: "swarm_run".into(),
            id: run.run_id.clone(),
        },
        payload,
        ..kura_events::Event::default()
    };
    let event = state.store.lock().append_event(&event).unwrap_or(event);
    state.event_bus.publish(event);
}

/// Runs every child of `run` on a bounded pool and records each transition.
/// Blocking; the launcher spawns it on a detached thread.
pub fn execute_run(state: AppState, run_id: String) {
    let Ok(manager) = manager(&state) else { return };
    let Some(chat) = state.chat.clone() else {
        return;
    };
    let Some(quota) = state.swarm_quota.clone() else {
        eprintln!("[kura] swarm: no quota gate wired; refusing to run {run_id}");
        return;
    };
    let Some(run) = manager.mark_running(&run_id) else {
        return;
    };
    persist(&state, &run);
    publish(&state, "swarm.run_started", &run, None);

    let concurrency = manager.config().max_concurrent().max(1);
    let total = run.children.len();
    let tenant_id = run.tenant_id.clone();
    let provider = run.provider.clone();
    let model = run.model.clone();

    for batch_start in (0..total).step_by(concurrency) {
        let batch: Vec<usize> = (batch_start..(batch_start + concurrency).min(total)).collect();
        std::thread::scope(|scope| {
            for index in batch {
                let state = state.clone();
                let manager = manager.clone();
                let chat = chat.clone();
                let quota = quota.clone();
                let run_id = run_id.clone();
                let tenant_id = tenant_id.clone();
                let provider = provider.clone();
                let model = model.clone();
                scope.spawn(move || {
                    let Some(run) = manager.get(&run_id) else {
                        return;
                    };
                    let goal = run.children[index].goal.clone();
                    let thread_id = run.children[index].thread_id.clone();

                    // 1. Quota before anything is spent.
                    let operation_key = match quota.reserve(&tenant_id, &run_id, index) {
                        kura_swarm::ChildQuotaDecision::Allowed { operation_key } => operation_key,
                        kura_swarm::ChildQuotaDecision::Denied { reason_code } => {
                            let updated = manager.update_child(&run_id, index, |c| {
                                c.status = kura_swarm::ChildStatus::QuotaDenied;
                                c.error = reason_code;
                                c.completed_at = Some(chrono::Utc::now());
                            });
                            if let Some(r) = updated {
                                persist(&state, &r);
                                publish(
                                    &state,
                                    "swarm.child_quota_denied",
                                    &r,
                                    r.children.get(index),
                                );
                            }
                            return;
                        }
                    };
                    if let Some(r) = manager.update_child(&run_id, index, |c| {
                        c.status = kura_swarm::ChildStatus::Running;
                        c.started_at = Some(chrono::Utc::now());
                    }) {
                        persist(&state, &r);
                        publish(&state, "swarm.child_started", &r, r.children.get(index));
                    }

                    // 2. The child turn: an ordinary chat query on its own thread.
                    let input = kura_chat::QueryInput {
                        query: goal,
                        provider,
                        model,
                        tenant_id: tenant_id.clone(),
                        thread_id,
                        ..Default::default()
                    };
                    let outcome = chat.query(input, &kura_chat::CancellationToken::new());

                    // 3. Commit or release; 4. record + publish.
                    let (event, updated) = match outcome {
                        Ok(exec) if exec.exec_error.is_none() => {
                            quota.commit(&tenant_id, &operation_key);
                            let dispatch_id = exec.result.dispatch.dispatch_id.clone();
                            let output = kura_swarm::preview(&exec.result.dispatch.output);
                            (
                                "swarm.child_completed",
                                manager.update_child(&run_id, index, |c| {
                                    c.status = kura_swarm::ChildStatus::Completed;
                                    c.dispatch_id = dispatch_id;
                                    c.output_preview = output;
                                    c.completed_at = Some(chrono::Utc::now());
                                }),
                            )
                        }
                        Ok(exec) => {
                            quota.release(&tenant_id, &operation_key, "child_failed");
                            let error = exec.exec_error.map(|e| e.to_string()).unwrap_or_default();
                            let dispatch_id = exec.result.dispatch.dispatch_id.clone();
                            (
                                "swarm.child_failed",
                                manager.update_child(&run_id, index, |c| {
                                    c.status = kura_swarm::ChildStatus::Failed;
                                    c.dispatch_id = dispatch_id;
                                    c.error = error;
                                    c.completed_at = Some(chrono::Utc::now());
                                }),
                            )
                        }
                        Err(err) => {
                            quota.release(&tenant_id, &operation_key, "child_failed");
                            (
                                "swarm.child_failed",
                                manager.update_child(&run_id, index, |c| {
                                    c.status = kura_swarm::ChildStatus::Failed;
                                    c.error = err.to_string();
                                    c.completed_at = Some(chrono::Utc::now());
                                }),
                            )
                        }
                    };
                    if let Some(r) = updated {
                        persist(&state, &r);
                        publish(&state, event, &r, r.children.get(index));
                    }
                });
            }
        });
    }
    if let Some(run) = manager.get(&run_id) {
        persist(&state, &run);
        publish(&state, "swarm.run_completed", &run, None);
    }
}

/// POST /v1/swarm/runs — 202 with the queued run; execution proceeds in the
/// background and is observable through `GET` and the `swarm.*` events.
async fn launch_run(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    token: Option<Extension<AuthenticatedToken>>,
    body: Bytes,
) -> Result<(StatusCode, Json<kura_swarm::SwarmRun>), ApiError> {
    let mut input: kura_swarm::LaunchInput = decode_json_required(&body)?;
    if input.requested_by.trim().is_empty() {
        input.requested_by = token
            .as_ref()
            .map(|t| t.0.0.principal_id.clone())
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| "operator".into());
    }
    let manager = manager(&state)?;
    let tenant_ref = tenant.as_ref().map(|e| &e.0);
    let tenant_id = acting_tenant(tenant_ref);
    let run = manager.launch(&tenant_id, input).map_err(|err| match err {
        kura_swarm::SwarmError::Disabled => ApiError::Forbidden(err.to_string()),
        kura_swarm::SwarmError::NotFound => ApiError::NotFound("not found".into()),
        other => ApiError::BadRequest(other.to_string()),
    })?;
    persist(&state, &run);
    bind_document_tenant(&state, tenant_ref, DOC_KIND, &run.run_id)?;
    publish(&state, "swarm.run_queued", &run, None);
    let bg_state = state.clone();
    let run_id = run.run_id.clone();
    std::thread::spawn(move || execute_run(bg_state, run_id));
    Ok((StatusCode::ACCEPTED, Json(run)))
}

async fn list_runs(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Query(_params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<RunListResponse>, ApiError> {
    let manager = manager(&state)?;
    let tenant_ref = tenant.as_ref().map(|e| &e.0);
    let tenant_id = acting_tenant(tenant_ref);
    let visible = tenant_visible_document_ids(&state, tenant_ref, DOC_KIND)?;
    let items = manager
        .list(&tenant_id)
        .into_iter()
        .filter(|r| match &visible {
            Some(ids) => ids.contains(&r.run_id),
            None => true,
        })
        .collect();
    Ok(Json(RunListResponse { items }))
}

async fn get_run(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(run_id): Path<String>,
) -> Result<Json<kura_swarm::SwarmRun>, ApiError> {
    let manager = manager(&state)?;
    let tenant_ref = tenant.as_ref().map(|e| &e.0);
    guard_document_tenant(&state, tenant_ref, DOC_KIND, run_id.trim())?;
    let run = manager
        .get(&run_id)
        .ok_or_else(|| ApiError::NotFound("not found".into()))?;
    if !acting_tenant(tenant_ref).is_empty() && run.tenant_id != acting_tenant(tenant_ref) {
        return Err(ApiError::NotFound("not found".into()));
    }
    Ok(Json(run))
}
