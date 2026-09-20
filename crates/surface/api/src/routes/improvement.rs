//! `/v1/improvement/proposals` — the audited self-improvement loop.
//!
//! Slice 1 targets one bounded change class: a single plugin-profile config
//! value. The agent proposes (evidence required, rate-bounded per target by
//! operator config); the operator applies or rejects; apply snapshots the
//! full prior profile into the proposal (no change without a recorded
//! rollback path) and atomically rewrites `plugins.json` (boot-time effect,
//! `restartRequired: true`); rollback restores the snapshot. Every
//! transition emits an `improvement.*` event — proposal → decision →
//! application → keep/rollback is a complete, inspectable audit chain.
//! Follow-up-evaluation auto-rollback is the next slice (documented).

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use serde::Serialize;

use kura_improvement::{ImprovementError, ImprovementProposal, ProposeInput};

use crate::error::ApiError;
use crate::middleware::{TenantContext, require_daemon_global_operator};
use crate::response::Json;
use crate::state::AppState;

fn manager(state: &AppState) -> Result<&kura_improvement::Manager, ApiError> {
    state
        .improvement
        .as_deref()
        .ok_or_else(|| ApiError::internal("improvement manager is not configured"))
}

fn map_error(err: ImprovementError) -> ApiError {
    let message = err.to_string();
    match err {
        ImprovementError::NotFound => ApiError::NotFound(message),
        ImprovementError::RateBounded(_) => ApiError::Forbidden(message),
        ImprovementError::InvalidTransition(_) => ApiError::Conflict(message),
        ImprovementError::Io(_) => ApiError::internal(&message),
        _ => ApiError::BadRequest(message),
    }
}

fn publish_event(state: &AppState, name: &str, proposal: &ImprovementProposal) {
    let mut payload = serde_json::Map::new();
    payload.insert(
        "proposalId".to_string(),
        serde_json::json!(proposal.proposal_id),
    );
    payload.insert(
        "targetPlugin".to_string(),
        serde_json::json!(proposal.target_plugin),
    );
    payload.insert(
        "configKey".to_string(),
        serde_json::json!(proposal.config_key),
    );
    payload.insert("status".to_string(), serde_json::json!(proposal.status));
    if !proposal.reason.is_empty() {
        payload.insert("reason".to_string(), serde_json::json!(proposal.reason));
    }
    let event = kura_events::Event {
        category: "improvement".to_string(),
        name: name.to_string(),
        resource: kura_events::Resource {
            kind: "improvement_proposal".to_string(),
            id: proposal.proposal_id.clone(),
        },
        payload,
        ..kura_events::Event::default()
    };
    let event = state.store.lock().append_event(&event).unwrap_or(event);
    state.event_bus.publish(event);
}

/// Atomically writes the profile (same tmp+rename discipline as the
/// profile PUT route).
fn write_profile(state: &AppState, profile: &kura_plugin::PluginProfile) -> Result<(), ApiError> {
    let dir = std::path::Path::new(&state.config.data_dir);
    let path = dir.join(kura_plugin::PROFILE_FILE_NAME);
    let tmp = dir.join(format!("{}.tmp", kura_plugin::PROFILE_FILE_NAME));
    let encoded = serde_json::to_vec_pretty(profile)
        .map_err(|err| ApiError::internal(&format!("encode plugin profile: {err}")))?;
    std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(&tmp, &encoded))
        .and_then(|()| std::fs::rename(&tmp, &path))
        .map_err(|err| ApiError::internal(&format!("write plugin profile: {err}")))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppliedResponse {
    pub proposal: ImprovementProposal,
    pub restart_required: bool,
}

/// POST /v1/improvement/proposals — agent-side drafting.
#[allow(clippy::unused_async)]
pub async fn propose(
    State(state): State<AppState>,
    tenant: Option<axum::extract::Extension<TenantContext>>,
    body: Bytes,
) -> Result<Json<ImprovementProposal>, ApiError> {
    require_daemon_global_operator(
        tenant.as_ref().map(|e| &e.0),
        "POST /v1/improvement/proposals",
    )?;
    let input: ProposeInput = super::decode_json_required(&body)?;
    // Stage 6.2: the measurement must exist in the evaluation plane, not
    // merely be named. Checked here because the crate is persistence-free.
    if input.evaluation.kind.trim() == "replay_attempt" {
        if let Some(evaluation) = state.evaluation.as_deref() {
            match evaluation.get_replay_attempt(input.evaluation.id.trim()) {
                Ok(Some(_)) => {}
                Ok(None) => {
                    return Err(ApiError::BadRequest(format!(
                        "evaluation replay attempt {} does not exist",
                        input.evaluation.id.trim()
                    )));
                }
                Err(err) => return Err(ApiError::internal(&format!("evaluation lookup: {err}"))),
            }
        }
    }
    let proposal = manager(&state)?.propose(input).map_err(map_error)?;
    publish_event(&state, "improvement.proposed", &proposal);
    Ok(Json(proposal))
}

/// GET /v1/improvement/proposals — full audit list, newest first.
#[allow(clippy::unused_async)]
pub async fn list(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    let items = manager(&state)?.list();
    Ok(Json(serde_json::json!({ "items": items })))
}

/// POST /v1/improvement/proposals/{id}/apply — operator approval: snapshot
/// the prior profile (the rollback path), write the proposed value, mark
/// applied.
#[allow(clippy::unused_async)]
pub async fn apply(
    State(state): State<AppState>,
    tenant: Option<axum::extract::Extension<TenantContext>>,
    Path(proposal_id): Path<String>,
) -> Result<Json<AppliedResponse>, ApiError> {
    require_daemon_global_operator(
        tenant.as_ref().map(|e| &e.0),
        "POST /v1/improvement/proposals/{id}/apply",
    )?;
    let improvement = manager(&state)?;
    let Some(proposal) = improvement.get(&proposal_id) else {
        return Err(ApiError::NotFound(
            "improvement proposal not found".to_string(),
        ));
    };
    let profile = kura_plugin::PluginProfile::load(&state.config.data_dir)
        .map_err(|err| ApiError::internal(&format!("load plugin profile: {err}")))?;
    let prior = serde_json::to_value(&profile)
        .map_err(|err| ApiError::internal(&format!("snapshot profile: {err}")))?;

    let mut updated = profile;
    let entry = updated
        .entries
        .entry(proposal.target_plugin.clone())
        .or_default();
    entry
        .config
        .insert(proposal.config_key.clone(), proposal.proposed_value.clone());
    write_profile(&state, &updated)?;

    let proposal = improvement
        .mark_applied(&proposal_id, prior)
        .map_err(map_error)?;
    publish_event(&state, "improvement.applied", &proposal);
    Ok(Json(AppliedResponse {
        proposal,
        restart_required: true,
    }))
}

/// POST /v1/improvement/proposals/{id}/reject — operator veto.
#[allow(clippy::unused_async)]
pub async fn reject(
    State(state): State<AppState>,
    tenant: Option<axum::extract::Extension<TenantContext>>,
    Path(proposal_id): Path<String>,
    body: Bytes,
) -> Result<Json<ImprovementProposal>, ApiError> {
    require_daemon_global_operator(
        tenant.as_ref().map(|e| &e.0),
        "POST /v1/improvement/proposals/{id}/reject",
    )?;
    #[derive(Default, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    struct ReasonBody {
        reason: String,
    }
    let reason: ReasonBody = super::decode_json_or_default(&body)?;
    let proposal = manager(&state)?
        .reject(&proposal_id, &reason.reason)
        .map_err(map_error)?;
    publish_event(&state, "improvement.rejected", &proposal);
    Ok(Json(proposal))
}

/// POST /v1/improvement/proposals/{id}/rollback — restore the recorded
/// prior profile.
#[allow(clippy::unused_async)]
pub async fn rollback(
    State(state): State<AppState>,
    tenant: Option<axum::extract::Extension<TenantContext>>,
    Path(proposal_id): Path<String>,
    body: Bytes,
) -> Result<Json<AppliedResponse>, ApiError> {
    require_daemon_global_operator(
        tenant.as_ref().map(|e| &e.0),
        "POST /v1/improvement/proposals/{id}/rollback",
    )?;
    #[derive(Default, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    struct ReasonBody {
        reason: String,
    }
    let reason: ReasonBody = super::decode_json_or_default(&body)?;
    let improvement = manager(&state)?;
    let Some(proposal) = improvement.get(&proposal_id) else {
        return Err(ApiError::NotFound(
            "improvement proposal not found".to_string(),
        ));
    };
    let prior: kura_plugin::PluginProfile = serde_json::from_value(proposal.prior_profile.clone())
        .map_err(|err| {
            ApiError::Conflict(format!("proposal carries no restorable snapshot: {err}"))
        })?;
    write_profile(&state, &prior)?;
    let proposal = improvement
        .mark_rolled_back(&proposal_id, &reason.reason)
        .map_err(map_error)?;
    publish_event(&state, "improvement.rolled_back", &proposal);
    Ok(Json(AppliedResponse {
        proposal,
        restart_required: true,
    }))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/improvement/proposals", post(propose).get(list))
        .route("/v1/improvement/proposals/{id}/apply", post(apply))
        .route("/v1/improvement/proposals/{id}/reject", post(reject))
        .route("/v1/improvement/proposals/{id}/rollback", post(rollback))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::http::StatusCode;

    use super::super::tests_support::{request_json, request_json_as_role, test_state};

    fn improvement_state() -> crate::state::AppState {
        let mut state = test_state();
        let dir = std::env::temp_dir().join(format!("kura-improve-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        state.config.data_dir = dir.to_string_lossy().into_owned();
        state.improvement = Some(Arc::new(kura_improvement::Manager::new(
            &state.config.data_dir,
            kura_improvement::ImprovementConfig::default(),
        )));
        state
    }

    #[tokio::test]
    async fn propose_apply_rollback_audit_loop() {
        let state = improvement_state();

        let (status, json) = request_json(
            state.clone(),
            "POST",
            "/v1/improvement/proposals",
            Some(serde_json::json!({
                "targetPlugin": "session-strategy",
                "configKey": "personalBudgetChars",
                "currentValue": 48000,
                "proposedValue": 64000,
                "predictedEffect": "fewer elisions",
                "evidenceLinks": [{ "kind": "event", "id": "evt_ctx_1" }],
                "proposedBy": "agent",
                "evaluation": { "kind": "campaign", "id": "cmp_1", "metric": "elisions_per_turn", "baseline": 3, "observed": 1 }
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{json}");
        assert_eq!(json["status"], "pending");
        let id = json["proposalId"].as_str().expect("id").to_string();

        // Apply: profile mutated on disk, snapshot recorded, restart flagged.
        let (status, json) = request_json(
            state.clone(),
            "POST",
            &format!("/v1/improvement/proposals/{id}/apply"),
            Some(serde_json::json!({})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{json}");
        assert_eq!(json["restartRequired"], true);
        assert_eq!(json["proposal"]["status"], "applied");
        let profile = kura_plugin::PluginProfile::load(&state.config.data_dir).expect("profile");
        assert_eq!(
            profile.entries["session-strategy"].config["personalBudgetChars"],
            serde_json::json!(64000)
        );

        // Rollback: profile restored, chain closes as rolled_back.
        let (status, json) = request_json(
            state.clone(),
            "POST",
            &format!("/v1/improvement/proposals/{id}/rollback"),
            Some(serde_json::json!({ "reason": "follow-up regressed" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{json}");
        assert_eq!(json["proposal"]["status"], "rolled_back");
        let profile = kura_plugin::PluginProfile::load(&state.config.data_dir).expect("profile");
        assert!(
            !profile.entries.contains_key("session-strategy"),
            "prior (empty) profile restored"
        );

        // The audit chain is recorded as events.
        let events = state
            .store
            .lock()
            .list_events(&kura_events::Filter::default())
            .expect("events");
        for name in [
            "improvement.proposed",
            "improvement.applied",
            "improvement.rolled_back",
        ] {
            assert!(events.iter().any(|e| e.name == name), "missing {name}");
        }

        // Evidence-less proposals are refused.
        let (status, _) = request_json(
            state,
            "POST",
            "/v1/improvement/proposals",
            Some(serde_json::json!({
                "targetPlugin": "context",
                "configKey": "memoryBudgetChars",
                "proposedValue": 1
            })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    /// Applying a proposal rewrites `plugins.json` for the whole daemon, so
    /// it is gated on the tenant owner role rather than tenant-scoped. A
    /// weaker role is refused with 403 — there is no row to hide, so the
    /// denial is stated rather than disguised as a 404.
    #[tokio::test]
    async fn daemon_global_mutations_require_the_owner_role() {
        let state = improvement_state();
        let proposal = serde_json::json!({
            "targetPlugin": "session-strategy",
            "configKey": "personalBudgetChars",
            "currentValue": 48000,
            "proposedValue": 64000,
            "predictedEffect": "longer personal window",
            "evidenceLinks": [{ "kind": "event", "id": "evt_ctx_1" }],
            "proposedBy": "agent",
                "evaluation": { "kind": "campaign", "id": "cmp_1", "metric": "elisions_per_turn", "baseline": 3, "observed": 1 }
        });

        let (status, _) = request_json_as_role(
            state.clone(),
            "tnt_a",
            kura_identity::Role::Viewer,
            "POST",
            "/v1/improvement/proposals",
            Some(proposal.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        let (status, _) = request_json_as_role(
            state.clone(),
            "tnt_a",
            kura_identity::Role::Operator,
            "POST",
            "/v1/improvement/proposals",
            Some(proposal.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        let (status, created) = request_json_as_role(
            state.clone(),
            "tnt_a",
            kura_identity::Role::Owner,
            "POST",
            "/v1/improvement/proposals",
            Some(proposal),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{created}");
    }

    /// The single-user assembly has no tenant context and must keep working.
    #[tokio::test]
    async fn without_a_tenant_the_global_guard_passes_through() {
        let state = improvement_state();
        let (status, created) = request_json(
            state.clone(),
            "POST",
            "/v1/improvement/proposals",
            Some(serde_json::json!({
                "targetPlugin": "session-strategy",
                "configKey": "personalBudgetChars",
                "currentValue": 48000,
                "proposedValue": 64000,
                "predictedEffect": "longer personal window",
                "evidenceLinks": [{ "kind": "event", "id": "evt_ctx_1" }],
                "proposedBy": "agent",
                "evaluation": { "kind": "campaign", "id": "cmp_1", "metric": "elisions_per_turn", "baseline": 3, "observed": 1 }
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{created}");
    }
}
