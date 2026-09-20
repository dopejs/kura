//! policy approvals route family (port of the /v1/policy/approvals handlers
//! in Go daemon/internal/api/server.go, Roadmap 4 trust boundary).
//!
//! Routes: GET/POST /v1/policy/approvals, GET /v1/policy/approvals/{approval_id},
//! POST /v1/policy/approvals/{approval_id}/resolve. Approvals and decisions
//! are enriched with their sandbox consumer-contract views (Go
//! enrichApprovalsWithSandbox), resolution syncs the linked consumer policy
//! record, and an approved resolution resumes any pending computer-use
//! action and advances its owning workflow.
//!
//! Deliberately not ported (documented divergence, matching the wave-8
//! conventions): recordThreadApprovalProjection (kura-threads is not an api
//! dependency; Go no-ops when the projection prerequisites are unmet).
//!
//! **Tenancy.** The policy engine holds approvals in memory for the whole
//! daemon, so tenant scoping is applied at this layer against the
//! `approvals`/`decisions` tenant columns:
//! - writes bind the row to the acting tenant ([`bind_persisted_row`]);
//! - the list intersects the engine's view with
//!   `list_approvals_for_tenant_raw`, which excludes pre-backfill
//!   (`tenant_id IS NULL`) rows — the documented `kura-tenancy` read
//!   convention;
//! - the by-id routes are covered by `ByIDTenantGuardLayer`, attached in
//!   [`super::router`], which instead *admits* legacy NULL rows and answers
//!   404 (never a disclosure) for rows owned by another tenant.
//!
//! Within a tenant, approvals stay visible to every member: resolving another
//! principal's request is the point of an approval gate, and who may resolve
//! is a role question the `protected()` permission model already answers.

use std::collections::HashMap;

use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use chrono::Utc;
use serde::Serialize;

use kura_events as events;
use kura_policy as policy;
use kura_sandbox as sandbox;

use crate::error::ApiError;
use crate::middleware::{TenantContext, environment_scope_from_config};
use crate::state::AppState;

use super::decode_json_required;

/// Route family router.
#[must_use]
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/v1/policy/approvals",
            get(list_approvals).post(request_approval),
        )
        .route("/v1/policy/approvals/{approval_id}", get(get_approval))
        .route(
            "/v1/policy/approvals/{approval_id}/resolve",
            axum::routing::post(resolve_approval),
        )
}

#[derive(Debug, Serialize)]
struct ApprovalListResponse {
    items: Vec<policy::Approval>,
}

#[derive(Debug, Serialize)]
struct ApprovalDecisionResponse {
    approval: policy::Approval,
    decision: policy::Decision,
}

fn engine(state: &AppState) -> Result<&policy::Engine, ApiError> {
    state
        .policy
        .as_deref()
        .ok_or_else(|| ApiError::internal("policy engine is not configured"))
}

fn map_policy_error(err: policy::PolicyError) -> ApiError {
    match err {
        policy::PolicyError::ApprovalNotFound => ApiError::NotFound("not found".to_string()),
        other => ApiError::BadRequest(other.to_string()),
    }
}

fn json_value<T: Serialize>(value: &T) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or(serde_json::Value::Null)
}

// ---------------------------------------------------------------------------
// Consumer-policy record index (Go loadConsumerPolicyRecordIndex)
// ---------------------------------------------------------------------------

struct ConsumerPolicyIndex {
    by_approval_id: HashMap<String, sandbox::ConsumerContractView>,
    by_decision_id: HashMap<String, sandbox::ConsumerContractView>,
}

fn load_consumer_policy_index(state: &AppState) -> Result<ConsumerPolicyIndex, ApiError> {
    let records = state
        .store_pool
        .read()
        .list_consumer_policy_records()
        .map_err(ApiError::from_store)?;
    let mut index = ConsumerPolicyIndex {
        by_approval_id: HashMap::new(),
        by_decision_id: HashMap::new(),
    };
    for record in records {
        let Ok(view) = serde_json::from_str::<sandbox::ConsumerContractView>(&record.document)
        else {
            continue;
        };
        let Some(policy_record) = &view.policy_record else {
            continue;
        };
        if !policy_record.approval_id.trim().is_empty() {
            index
                .by_approval_id
                .insert(policy_record.approval_id.trim().to_string(), view.clone());
        }
        if !policy_record.decision_id.trim().is_empty() {
            index
                .by_decision_id
                .insert(policy_record.decision_id.trim().to_string(), view);
        }
    }
    Ok(index)
}

/// Go enrichApprovalsWithSandbox.
fn enrich_approval(
    index: &ConsumerPolicyIndex,
    mut approval: policy::Approval,
) -> policy::Approval {
    approval.sandbox = index
        .by_approval_id
        .get(approval.approval_id.trim())
        .and_then(|view| serde_json::to_value(view).ok());
    approval
}

/// Go enrichDecisionsWithSandbox.
fn enrich_decision(
    index: &ConsumerPolicyIndex,
    mut decision: policy::Decision,
) -> policy::Decision {
    decision.sandbox = index
        .by_decision_id
        .get(decision.decision_id.trim())
        .and_then(|view| serde_json::to_value(view).ok());
    decision
}

/// Go syncConsumerPolicyRecordForApprovalResolution: flip the linked consumer
/// policy record to the approval outcome and persist the updated view.
fn sync_consumer_policy_record(
    state: &AppState,
    approval: &policy::Approval,
    decision: &policy::Decision,
) -> Result<(), ApiError> {
    let records = state
        .store_pool
        .read()
        .list_consumer_policy_records()
        .map_err(ApiError::from_store)?;
    for record_row in records {
        let Ok(mut view) =
            serde_json::from_str::<sandbox::ConsumerContractView>(&record_row.document)
        else {
            continue;
        };
        let Some(record) = view.policy_record.as_mut() else {
            continue;
        };
        if record.approval_id.trim() != approval.approval_id.trim() {
            continue;
        }
        record.approval_id = approval.approval_id.clone();
        record.decision_id = decision.decision_id.clone();
        match approval.status {
            policy::ApprovalStatus::Approved => {
                record.decision = sandbox::DecisionResolution::Allow;
                record.approval_status = sandbox::DecisionApprovalStatus::Approved;
                record.status = sandbox::PolicyRecordStatus::PreflightAllowed;
                record.failure_class = String::new();
            }
            policy::ApprovalStatus::Rejected => {
                record.decision = sandbox::DecisionResolution::Deny;
                record.approval_status = sandbox::DecisionApprovalStatus::Rejected;
                record.status = sandbox::PolicyRecordStatus::Denied;
                record.failure_class = "approval_rejected".to_string();
            }
            policy::ApprovalStatus::Pending => {
                record.decision = sandbox::DecisionResolution::Ask;
                record.approval_status = sandbox::DecisionApprovalStatus::Pending;
                record.status = sandbox::PolicyRecordStatus::ApprovalPending;
                record.failure_class = "approval_required".to_string();
            }
        }
        record.completed_at = Some(Utc::now());

        let record = record.clone();
        let document = serde_json::to_string(&view)
            .map_err(|err| ApiError::internal(format!("marshal consumer policy view: {err}")))?;
        state
            .store
            .lock()
            .upsert_consumer_policy_record(&kura_store::ConsumerPolicyRecordRecord {
                policy_record_id: record.policy_record_id.clone(),
                consumer_kind: record.consumer_kind.as_str().to_string(),
                consumer_id: record.consumer_id.clone(),
                operation_kind: record.operation_kind.clone(),
                declaration_id: record.declaration_id.clone(),
                status: record.status.as_str().to_string(),
                decision: record.decision.as_str().to_string(),
                approval_status: record.approval_status.as_str().to_string(),
                secret_resolution: record.secret_resolution.as_str().to_string(),
                requested_by: record.requested_by.clone(),
                sandbox_execution_id: record.sandbox_execution_id.clone(),
                tool_call_id: record.tool_call_id.clone(),
                provider_operation_id: record.provider_operation_id.clone(),
                started_at: record.started_at,
                completed_at: record.completed_at,
                document,
            })
            .map_err(ApiError::from_store)?;
        break;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Persistence + events
// ---------------------------------------------------------------------------

/// Binds a just-persisted row to the acting tenant, mirroring the
/// `kura-tenancy` `*ForTenant` accessors (which cannot be used here: they own
/// an `SQLiteStore` by value while the API holds a shared handle). A row owned
/// by another tenant refuses the write rather than silently rebinding.
fn bind_persisted_row(
    state: &AppState,
    tenant: Option<&TenantContext>,
    table: &str,
    pk_column: &str,
    pk: &str,
) -> Result<(), ApiError> {
    let Some(tc) = tenant else { return Ok(()) };
    let tenant_id = tc.0.tenant_id.trim();
    if tenant_id.is_empty() {
        return Ok(());
    }
    match state
        .store
        .lock()
        .bind_row_tenant(table, pk_column, pk, tenant_id)
    {
        Ok(()) => Ok(()),
        Err(e) if kura_store::SQLiteStore::is_cross_tenant_row(&e) => {
            Err(ApiError::NotFound("not found".to_string()))
        }
        Err(e) => Err(ApiError::from_store(e)),
    }
}

fn persist_approval(
    state: &AppState,
    tenant: Option<&TenantContext>,
    approval: &policy::Approval,
) -> Result<(), ApiError> {
    state
        .store
        .lock()
        .upsert_approval(approval)
        .map_err(ApiError::from_store)?;
    bind_persisted_row(
        state,
        tenant,
        "approvals",
        "approval_id",
        &approval.approval_id,
    )
}

fn persist_decision(
    state: &AppState,
    tenant: Option<&TenantContext>,
    decision: &policy::Decision,
) -> Result<(), ApiError> {
    state
        .store
        .lock()
        .upsert_decision(decision)
        .map_err(ApiError::from_store)?;
    bind_persisted_row(
        state,
        tenant,
        "decisions",
        "decision_id",
        &decision.decision_id,
    )
}

fn publish_policy_event(
    state: &AppState,
    name: &str,
    resource_kind: &str,
    resource_id: &str,
    payload: serde_json::Map<String, serde_json::Value>,
) -> Result<(), ApiError> {
    let event = events::Event {
        category: "policy".to_string(),
        name: name.to_string(),
        environment_scope: environment_scope_from_config(&state.config),
        resource: events::Resource {
            kind: resource_kind.to_string(),
            id: resource_id.to_string(),
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

fn approval_payload(
    approval: &policy::Approval,
    include_resolution: bool,
) -> serde_json::Map<String, serde_json::Value> {
    let mut payload = serde_json::Map::new();
    payload.insert("action".to_string(), serde_json::json!(approval.action));
    payload.insert(
        "resourceKind".to_string(),
        serde_json::json!(approval.resource_kind),
    );
    payload.insert(
        "resourceId".to_string(),
        serde_json::json!(approval.resource_id),
    );
    payload.insert("status".to_string(), json_value(&approval.status));
    if include_resolution {
        payload.insert(
            "resolution".to_string(),
            serde_json::json!(approval.resolution),
        );
    }
    if let Some(sandbox) = &approval.sandbox {
        payload.insert("sandbox".to_string(), sandbox.clone());
    }
    payload
}

fn decision_payload(decision: &policy::Decision) -> serde_json::Map<String, serde_json::Value> {
    let mut payload = serde_json::Map::new();
    payload.insert("action".to_string(), serde_json::json!(decision.action));
    payload.insert(
        "resourceKind".to_string(),
        serde_json::json!(decision.resource_kind),
    );
    payload.insert(
        "resourceId".to_string(),
        serde_json::json!(decision.resource_id),
    );
    payload.insert("outcome".to_string(), json_value(&decision.outcome));
    payload.insert(
        "approvalId".to_string(),
        serde_json::json!(decision.approval_id),
    );
    if let Some(sandbox) = &decision.sandbox {
        payload.insert("sandbox".to_string(), sandbox.clone());
    }
    payload
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /v1/policy/approvals (Go handlePolicyApprovals GET branch).
async fn list_approvals(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<ApprovalListResponse>, ApiError> {
    let engine = engine(&state)?;
    let raw_status = params
        .get("status")
        .map(String::as_str)
        .unwrap_or("")
        .trim();
    let status = if raw_status.is_empty() {
        None
    } else {
        match serde_json::from_value::<policy::ApprovalStatus>(serde_json::json!(raw_status)) {
            Ok(status) => Some(status),
            // Go filters by the raw status string: an unknown status matches
            // no approvals.
            Err(_) => return Ok(Json(ApprovalListResponse { items: Vec::new() })),
        }
    };
    let index = load_consumer_policy_index(&state)?;
    let visible = tenant_visible_approval_ids(&state, tenant.as_ref().map(|e| &e.0))?;
    let items = engine
        .list_approvals(status)
        .into_iter()
        .filter(|approval| match &visible {
            Some(ids) => ids.contains(approval.approval_id.trim()),
            None => true,
        })
        .map(|approval| enrich_approval(&index, approval))
        .collect();
    Ok(Json(ApprovalListResponse { items }))
}

/// Approval ids the caller's tenant owns, or `None` when no tenant is acting
/// (single-user assembly) and the engine's full view is the answer.
///
/// Pre-backfill rows (`tenant_id IS NULL`) are **not** listed: that is the
/// `kura-tenancy` read convention, and listing them would hand every tenant
/// the daemon's pre-tenancy history. The by-id routes stay reachable for them
/// through `ByIDTenantGuardLayer`, so nothing becomes unrecoverable — it stops
/// being enumerable.
fn tenant_visible_approval_ids(
    state: &AppState,
    tenant: Option<&TenantContext>,
) -> Result<Option<std::collections::HashSet<String>>, ApiError> {
    let Some(tc) = tenant else { return Ok(None) };
    let tenant_id = tc.0.tenant_id.trim();
    if tenant_id.is_empty() {
        return Ok(None);
    }
    let rows = state
        .store_pool
        .read()
        .list_approvals_for_tenant_raw(tenant_id)
        .map_err(ApiError::from_store)?;
    Ok(Some(
        rows.into_iter()
            .map(|approval| approval.approval_id.trim().to_string())
            .collect(),
    ))
}

/// POST /v1/policy/approvals (Go handlePolicyApprovals POST branch) — 201
/// with {approval, decision}.
async fn request_approval(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<(StatusCode, Json<ApprovalDecisionResponse>), ApiError> {
    let input: policy::RequestApprovalInput = decode_json_required(&body)?;
    let engine = engine(&state)?;
    let tenant = tenant.as_ref().map(|e| &e.0);
    let (approval, decision) = engine.request_approval(input).map_err(map_policy_error)?;
    persist_approval(&state, tenant, &approval)?;
    persist_decision(&state, tenant, &decision)?;
    publish_policy_event(
        &state,
        "policy.approval_requested",
        "approval",
        &approval.approval_id,
        approval_payload(&approval, false),
    )?;
    publish_policy_event(
        &state,
        "policy.decision_recorded",
        "decision",
        &decision.decision_id,
        decision_payload(&decision),
    )?;
    let index = load_consumer_policy_index(&state)?;
    Ok((
        StatusCode::CREATED,
        Json(ApprovalDecisionResponse {
            approval: enrich_approval(&index, approval),
            decision: enrich_decision(&index, decision),
        }),
    ))
}

/// GET /v1/policy/approvals/{approval_id} (Go handlePolicyApprovalByID).
async fn get_approval(
    State(state): State<AppState>,
    Path(approval_id): Path<String>,
) -> Result<Json<policy::Approval>, ApiError> {
    let engine = engine(&state)?;
    let approval = engine
        .get_approval(approval_id.trim())
        .ok_or_else(|| ApiError::NotFound("not found".to_string()))?;
    let index = load_consumer_policy_index(&state)?;
    Ok(Json(enrich_approval(&index, approval)))
}

/// POST /v1/policy/approvals/{approval_id}/resolve (Go
/// handlePolicyApprovalResolve): resolve, persist, sync the consumer policy
/// record, publish resolution events, and resume any pending computer-use
/// action (advancing its owning workflow) on approval.
async fn resolve_approval(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(approval_id): Path<String>,
    body: Bytes,
) -> Result<Json<ApprovalDecisionResponse>, ApiError> {
    let input: policy::ResolveApprovalInput = decode_json_required(&body)?;
    let engine = engine(&state)?;
    let tenant = tenant.as_ref().map(|e| &e.0);
    let approval_id = approval_id.trim().to_string();
    let (approval, decision) = engine
        .resolve_approval(&approval_id, input)
        .map_err(map_policy_error)?;
    persist_approval(&state, tenant, &approval)?;
    persist_decision(&state, tenant, &decision)?;
    sync_consumer_policy_record(&state, &approval, &decision)?;

    let index = load_consumer_policy_index(&state)?;
    let approval = enrich_approval(&index, approval);
    let decision = enrich_decision(&index, decision);

    publish_policy_event(
        &state,
        "policy.approval_resolved",
        "approval",
        &approval.approval_id,
        approval_payload(&approval, true),
    )?;
    publish_policy_event(
        &state,
        "policy.decision_recorded",
        "decision",
        &decision.decision_id,
        decision_payload(&decision),
    )?;

    // Go: computer-use resume of the action gated behind this approval, plus
    // the owning workflow's advancement.
    if let Some(computer_use) = state.computer_use.as_deref() {
        let (action, resumed) = computer_use
            .resume_pending_action(&approval_id)
            .map_err(ApiError::from_store)?;
        if resumed {
            super::computer_use::persist_computer_use_runtime_tracking(&state, &action)?;
            super::computer_use::publish_computer_use_artifacts(&state, None, &action);
            if action.failure_class == "target_mismatch" {
                super::computer_use::publish_computer_use_target_mismatch(&state, None, &action)?;
            }
            let mut payload = serde_json::Map::new();
            payload.insert("status".to_string(), json_value(&action.status));
            payload.insert(
                "failureClass".to_string(),
                serde_json::json!(action.failure_class),
            );
            payload.insert(
                "computerUseActionId".to_string(),
                serde_json::json!(action.computer_use_action_id),
            );
            payload.insert(
                "computerUseSessionId".to_string(),
                serde_json::json!(action.computer_use_session_id),
            );
            let event = events::Event {
                category: "capability".to_string(),
                name: "computer_use.action_status_changed".to_string(),
                environment_scope: environment_scope_from_config(&state.config),
                scope: events::Scope {
                    run_id: action.run_id.clone(),
                    step_id: action.step_id.clone(),
                    computer_use_session_id: action.computer_use_session_id.clone(),
                    computer_use_action_id: action.computer_use_action_id.clone(),
                    ..events::Scope::default()
                },
                resource: events::Resource {
                    kind: "computer_use_action".to_string(),
                    id: action.computer_use_action_id.clone(),
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

            if !action.workflow_id.is_empty() && !action.tool_call_id.is_empty() {
                if let Some(runtime_manager) = state.runtime.as_deref() {
                    let environment_scope = environment_scope_from_config(&state.config);
                    let workflow = state
                        .store_pool
                        .read()
                        .get_workflow(&environment_scope, &action.run_id, &action.workflow_id)
                        .map_err(ApiError::from_store)?;
                    if let Some(workflow) = workflow {
                        if let Some(tool_call) = runtime_manager.get_tool_call(
                            &action.run_id,
                            &action.step_id,
                            &action.tool_call_id,
                        ) {
                            super::workflows::advance_workflow_after_tool_call(
                                &state,
                                workflow,
                                &tool_call,
                                Some(kura_orchestration::StepStatus::Running),
                                "",
                            )?;
                        }
                    }
                }
            }
        }
    }

    Ok(Json(ApprovalDecisionResponse { approval, decision }))
}

#[cfg(test)]
mod tests {
    use super::super::tests_support::{request_json, request_json_as_tenant, test_state};
    use axum::http::StatusCode;
    use std::sync::Arc;

    fn state_with_engine() -> crate::state::AppState {
        let mut state = test_state();
        state.policy = Some(Arc::new(kura_policy::Engine::new()));
        state
    }

    #[tokio::test]
    async fn request_list_get_and_resolve_approval() {
        let state = state_with_engine();
        let (status, created) = request_json(
            state.clone(),
            "POST",
            "/v1/policy/approvals",
            Some(serde_json::json!({
                "action": "tool.execute",
                "resourceKind": "skill",
                "resourceId": "skill_deploy",
                "reason": "deploy touches production",
                "requestedBy": "operator_a"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let approval_id = created["approval"]["approvalId"]
            .as_str()
            .expect("approvalId")
            .to_string();

        let (status, listed) = request_json(
            state.clone(),
            "GET",
            "/v1/policy/approvals?status=pending",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{listed}");
        assert_eq!(listed["items"].as_array().expect("items").len(), 1);

        let (status, fetched) = request_json(
            state.clone(),
            "GET",
            &format!("/v1/policy/approvals/{approval_id}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{fetched}");

        let (status, resolved) = request_json(
            state.clone(),
            "POST",
            &format!("/v1/policy/approvals/{approval_id}/resolve"),
            Some(serde_json::json!({ "resolution": "approved", "comment": "ok" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{resolved}");
        assert_eq!(resolved["approval"]["status"], "approved");

        let (status, _) =
            request_json(state, "GET", "/v1/policy/approvals/approval_missing", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// Approvals are tenant-scoped: one tenant's approval is neither listed
    /// nor readable by another, and the denial is a 404 rather than a 403 so
    /// the row's existence is not disclosed.
    #[tokio::test]
    async fn approvals_are_scoped_to_the_acting_tenant() {
        let state = state_with_engine();

        let (status, created) = request_json_as_tenant(
            state.clone(),
            "tnt_a",
            "POST",
            "/v1/policy/approvals",
            Some(serde_json::json!({
                "action": "tool.execute",
                "resourceKind": "skill",
                "resourceId": "skill_deploy",
                "reason": "tenant a work",
                "requestedBy": "operator_a"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let approval_id = created["approval"]["approvalId"]
            .as_str()
            .expect("approvalId")
            .to_string();

        // Owner sees it in the list.
        let (status, listed) =
            request_json_as_tenant(state.clone(), "tnt_a", "GET", "/v1/policy/approvals", None)
                .await;
        assert_eq!(status, StatusCode::OK, "{listed}");
        assert_eq!(listed["items"].as_array().expect("items").len(), 1);

        // Another tenant does not — the policy engine holds it in memory for
        // the whole daemon, so this is exactly the leak being closed.
        let (status, listed) =
            request_json_as_tenant(state.clone(), "tnt_b", "GET", "/v1/policy/approvals", None)
                .await;
        assert_eq!(status, StatusCode::OK, "{listed}");
        assert!(
            listed["items"].as_array().expect("items").is_empty(),
            "tenant b must not see tenant a's approvals: {listed}"
        );

        // By-id is guarded too, and does not disclose existence.
        let (status, _) = request_json_as_tenant(
            state.clone(),
            "tnt_b",
            "GET",
            &format!("/v1/policy/approvals/{approval_id}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // Resolving another tenant's approval is refused on the same guard.
        let (status, _) = request_json_as_tenant(
            state.clone(),
            "tnt_b",
            "POST",
            &format!("/v1/policy/approvals/{approval_id}/resolve"),
            Some(serde_json::json!({ "resolution": "approved" })),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // The owner still reaches both.
        let (status, _) = request_json_as_tenant(
            state.clone(),
            "tnt_a",
            "GET",
            &format!("/v1/policy/approvals/{approval_id}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    /// The single-user assembly has no tenant context; nothing may start
    /// filtering there.
    #[tokio::test]
    async fn without_a_tenant_the_engine_view_is_unfiltered() {
        let state = state_with_engine();
        let (status, _) = request_json(
            state.clone(),
            "POST",
            "/v1/policy/approvals",
            Some(serde_json::json!({
                "action": "tool.execute",
                "resourceKind": "skill",
                "resourceId": "skill_local",
                "reason": "local",
                "requestedBy": "operator"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);

        let (status, listed) =
            request_json(state.clone(), "GET", "/v1/policy/approvals", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(listed["items"].as_array().expect("items").len(), 1);
    }
}
