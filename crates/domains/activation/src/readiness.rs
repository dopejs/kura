//! Readiness evaluation and quota baseline projection (port of
//! `readiness.go`).

use chrono::DateTime;
use chrono::SecondsFormat;
use chrono::Utc;
use kura_billing::BillingError;
use kura_billing::EffectiveQuota;
use kura_billing::EnforcementMode;
use kura_billing::go_zero_time;
use kura_identity::Principal;
use kura_identity::Tenant;
use serde_json::Map;
use serde_json::Value;

use crate::error::ActivationError;
use crate::error::activation_error;
use crate::service::Service;
use crate::service::stable_activation_id;
use crate::types::FailureReason;
use crate::types::FailureStage;
use crate::types::QuotaBaseline;
use crate::types::QuotaBaselineStatus;
use crate::types::QuotaProjection;
use crate::types::ReadinessItem;
use crate::types::ReadinessKind;
use crate::types::ReadinessStatus;
use crate::types::ReasonCode;
use crate::types::RemediationOwner;
use crate::types::STEP_QUOTA_BASELINE;
use crate::types::STEP_QUOTA_BASELINE_READY;
use crate::types::STEP_TENANT_RESOLVED;
use crate::types::STEP_TEST_CHAT;
use crate::types::State;
use crate::types::Status;
use crate::types::default_test_chat_first_action;

/// Builds the freshly evaluated active (or quota-blocked) activation state
/// for a personal tenant.
pub(crate) async fn active_state_for_personal_tenant(
    service: &Service,
    principal: &Principal,
    tenant: &Tenant,
    now: DateTime<Utc>,
) -> Result<State, ActivationError> {
    let environment_scope = if service.environment_scope.is_empty() {
        "test".to_string()
    } else {
        service.environment_scope.clone()
    };
    let mut state = State {
        activation_id: stable_activation_id("act", &[&principal.principal_id, &tenant.tenant_id]),
        principal_id: principal.principal_id.clone(),
        tenant_id: tenant.tenant_id.clone(),
        environment_scope,
        status: Status::ACTIVE.into(),
        current_step_id: STEP_TEST_CHAT.to_string(),
        completed_step_ids: vec![
            STEP_TENANT_RESOLVED.to_string(),
            STEP_QUOTA_BASELINE_READY.to_string(),
        ],
        blocking_reason_codes: Vec::new(),
        readiness_items: vec![
            ready_readiness_item(
                "tenant-access",
                ReadinessKind::TENANT_ACCESS.into(),
                "Tenant access",
                now,
            ),
            ready_readiness_item(
                "environment",
                ReadinessKind::ENVIRONMENT.into(),
                "Hosted environment",
                now,
            ),
        ],
        quota_baseline: None,
        first_action: default_test_chat_first_action(true, Vec::new()),
        test_chat: None,
        failure_reason: None,
        created_at: now,
        updated_at: now,
        first_action_completed_at: None,
        last_evaluated_at: now,
        last_transition_audit_event: String::new(),
        metadata: None,
    };

    let (baseline, quota_item) = project_quota_baseline(service, &tenant.tenant_id, now).await?;
    let blocked = baseline.status == QuotaBaselineStatus::UNAVAILABLE;
    state.quota_baseline = Some(baseline);
    state.readiness_items.push(quota_item);
    if blocked {
        state.status = Status::BLOCKED.into();
        state.current_step_id = STEP_QUOTA_BASELINE.to_string();
        state.completed_step_ids = vec![STEP_TENANT_RESOLVED.to_string()];
        state.blocking_reason_codes = vec![ReasonCode::QUOTA_BASELINE_UNAVAILABLE.into()];
        state.first_action =
            default_test_chat_first_action(false, vec!["quota-baseline".to_string()]);
        state.failure_reason = Some(FailureReason {
            reason_code: ReasonCode::QUOTA_BASELINE_UNAVAILABLE.into(),
            stage: FailureStage::QUOTA_BASELINE.into(),
            retryable: true,
            remediation_owner: RemediationOwner::OPERATOR.into(),
            message: "quota baseline is unavailable".to_string(),
        });
    }
    Ok(state)
}

/// Projects the quota baseline from billing usage. Without a billing
/// projector the baseline defaults to the free enforced plan; an unavailable
/// quota state degrades to a blocked readiness item instead of an error.
async fn project_quota_baseline(
    service: &Service,
    tenant_id: &str,
    now: DateTime<Utc>,
) -> Result<(QuotaBaseline, ReadinessItem), ActivationError> {
    let Some(billing) = &service.billing else {
        return Ok((
            default_quota_baseline(tenant_id, now),
            ready_readiness_item(
                "quota-baseline",
                ReadinessKind::QUOTA_BASELINE.into(),
                "Quota baseline",
                now,
            ),
        ));
    };
    let summary = match billing.usage_summary(tenant_id, service.hosted).await {
        Ok(summary) => summary,
        Err(BillingError::QuotaStateUnavailable) => {
            return Ok((
                unavailable_quota_baseline(tenant_id, now),
                blocked_quota_readiness(now),
            ));
        }
        Err(err) => {
            return Err(activation_error(
                ReasonCode::QUOTA_BASELINE_UNAVAILABLE.into(),
                FailureStage::QUOTA_BASELINE.into(),
                true,
                RemediationOwner::OPERATOR.into(),
                err.to_string(),
            ));
        }
    };
    let baseline = QuotaBaseline {
        tenant_id: first_non_empty(&[&summary.tenant_id, tenant_id]),
        plan_key: first_non_empty(&[&summary.plan_key, "unknown"]),
        enforcement_mode: first_non_empty(&[
            summary.enforcement_mode.as_str(),
            EnforcementMode::NOT_MEASURABLE,
        ]),
        status: QuotaBaselineStatus::AVAILABLE.into(),
        quotas: summary.quotas.iter().map(quota_projection).collect(),
        projected_at: now,
        projection_source: "billing_usage_summary".to_string(),
        reason_code: ReasonCode::default(),
    };
    Ok((
        baseline,
        ready_readiness_item(
            "quota-baseline",
            ReadinessKind::QUOTA_BASELINE.into(),
            "Quota baseline",
            now,
        ),
    ))
}

pub(crate) fn ready_readiness_item(
    item_id: &str,
    kind: ReadinessKind,
    display_name: &str,
    now: DateTime<Utc>,
) -> ReadinessItem {
    ReadinessItem {
        item_id: item_id.to_string(),
        item_kind: kind,
        status: ReadinessStatus::READY.into(),
        reason_code: ReasonCode::default(),
        display_name: display_name.to_string(),
        required_for_activation: true,
        retryable: false,
        remediation_owner: RemediationOwner::NONE_REQUIRED.into(),
        updated_at: now,
    }
}

fn blocked_quota_readiness(now: DateTime<Utc>) -> ReadinessItem {
    ReadinessItem {
        item_id: "quota-baseline".to_string(),
        item_kind: ReadinessKind::QUOTA_BASELINE.into(),
        status: ReadinessStatus::BLOCKED.into(),
        reason_code: ReasonCode::QUOTA_BASELINE_UNAVAILABLE.into(),
        display_name: "Quota baseline".to_string(),
        required_for_activation: true,
        retryable: true,
        remediation_owner: RemediationOwner::OPERATOR.into(),
        updated_at: now,
    }
}

fn default_quota_baseline(tenant_id: &str, now: DateTime<Utc>) -> QuotaBaseline {
    QuotaBaseline {
        tenant_id: tenant_id.to_string(),
        plan_key: "free".to_string(),
        enforcement_mode: EnforcementMode::ENFORCED.to_string(),
        status: QuotaBaselineStatus::AVAILABLE.into(),
        quotas: Vec::new(),
        projected_at: now,
        projection_source: "activation_default".to_string(),
        reason_code: ReasonCode::default(),
    }
}

fn unavailable_quota_baseline(tenant_id: &str, now: DateTime<Utc>) -> QuotaBaseline {
    QuotaBaseline {
        tenant_id: tenant_id.to_string(),
        plan_key: "unknown".to_string(),
        enforcement_mode: EnforcementMode::NOT_MEASURABLE.to_string(),
        status: QuotaBaselineStatus::UNAVAILABLE.into(),
        quotas: Vec::new(),
        projected_at: now,
        projection_source: "billing_usage_summary".to_string(),
        reason_code: ReasonCode::QUOTA_BASELINE_UNAVAILABLE.into(),
    }
}

fn quota_projection(quota: &EffectiveQuota) -> QuotaProjection {
    let used = quota.consumed_amount + quota.reserved_amount + quota.adjusted_amount
        - quota.carryover_applied;
    let mut metadata = Map::new();
    if quota.period_start != go_zero_time() {
        metadata.insert(
            "periodStart".to_string(),
            Value::String(rfc3339(quota.period_start)),
        );
    }
    if quota.period_end != go_zero_time() {
        metadata.insert(
            "periodEnd".to_string(),
            Value::String(rfc3339(quota.period_end)),
        );
    }
    if !quota.period_anchor.is_empty() {
        metadata.insert(
            "periodAnchor".to_string(),
            Value::String(quota.period_anchor.clone()),
        );
    }
    if !quota.denial_reason_code.is_empty() {
        metadata.insert(
            "denialReasonCode".to_string(),
            Value::String(quota.denial_reason_code.clone()),
        );
    }
    if quota.over_limit {
        metadata.insert("overLimit".to_string(), Value::Bool(true));
    }
    QuotaProjection {
        category: quota.category.to_string(),
        unit: quota.unit.to_string(),
        limit: Some(quota.limit),
        used: Some(used),
        remaining: Some(quota.remaining_amount),
        period: format!(
            "{}/{}",
            rfc3339(quota.period_start),
            rfc3339(quota.period_end)
        ),
        metadata: if metadata.is_empty() {
            None
        } else {
            Some(metadata)
        },
    }
}

/// Formats like Go's `time.RFC3339` on a UTC time (second precision, `Z`).
fn rfc3339(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub(crate) fn first_non_empty(values: &[&str]) -> String {
    for value in values {
        if !value.is_empty() {
            return (*value).to_string();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::TimeZone;
    use kura_billing::BillingError;
    use kura_billing::Category;
    use kura_billing::EffectiveQuota;
    use kura_billing::EnforcementMode;
    use kura_billing::PERIOD_ANCHOR_UTC;
    use kura_billing::Unit;
    use kura_billing::UsageSummary;
    use kura_identity::TenantContext;

    use super::*;
    use crate::ActivateInput;
    use crate::Dependencies;
    use crate::Service;
    use crate::StateStore;
    use crate::error::reason_code_from_error;
    use crate::testutil::*;

    fn quota_usage_summary(tenant_id: &str) -> UsageSummary {
        let limit = 10;
        let used = 2;
        UsageSummary {
            tenant_id: tenant_id.to_string(),
            plan_key: "hosted-free".to_string(),
            enforcement_mode: EnforcementMode::ENFORCED.into(),
            quotas: vec![EffectiveQuota {
                tenant_id: tenant_id.to_string(),
                plan_key: "hosted-free".to_string(),
                category: Category::RUN_LAUNCHES.into(),
                unit: Unit::COUNT.into(),
                period_start: Utc
                    .with_ymd_and_hms(2026, 5, 1, 0, 0, 0)
                    .single()
                    .unwrap_or_default(),
                period_end: Utc
                    .with_ymd_and_hms(2026, 6, 1, 0, 0, 0)
                    .single()
                    .unwrap_or_default(),
                period_anchor: PERIOD_ANCHOR_UTC.to_string(),
                limit,
                consumed_amount: used,
                remaining_amount: limit - used,
                enforcement_mode: EnforcementMode::ENFORCED.into(),
                denial_reason_code: "quota_denied:run_launches_exhausted".to_string(),
                ..EffectiveQuota::default()
            }],
            ..UsageSummary::default()
        }
    }

    fn has_readiness(state: &State, item_id: &str, status: &str, reason: &str) -> bool {
        state.readiness_items.iter().any(|item| {
            item.item_id == item_id
                && item.status == status
                && (reason.is_empty() || item.reason_code == reason)
        })
    }

    #[tokio::test]
    async fn activate_projects_quota_baseline_readiness() {
        let now = test_now();
        let repo = Arc::new(MemoryIdentityRepository::default());
        repo.principals
            .lock()
            .insert("prn_quota".to_string(), active_principal("prn_quota", now));
        let state_store = Arc::new(MemoryStateStore::default());
        let svc = Service::new(Dependencies {
            state_store: Some(state_store),
            identity: Some(repo),
            billing: Some(Arc::new(StaticBillingProjector {
                summary: Some(quota_usage_summary("ten_personal_prn_quota")),
                err: None,
            })),
            chat: None,
            audit: Some(Arc::new(RecordingAuditSink::default())),
            now: Some(Box::new(move || now)),
            environment_scope: "test".to_string(),
            hosted: true,
        });

        let state = svc
            .activate(ActivateInput {
                token: active_token("tok_quota", "prn_quota"),
                tenant_context: TenantContext::default(),
                source: String::new(),
            })
            .await
            .expect("activate");

        assert_eq!(state.status, Status::ACTIVE, "expected active state");
        let baseline = state.quota_baseline.as_ref().expect("quota baseline");
        assert_eq!(baseline.plan_key, "hosted-free");
        assert_eq!(baseline.status, QuotaBaselineStatus::AVAILABLE);
        assert_eq!(baseline.quotas.len(), 1);
        assert_eq!(baseline.quotas[0].category, Category::RUN_LAUNCHES);
        assert!(state.first_action.available);
        assert!(state.blocking_reason_codes.is_empty());
        assert!(
            has_readiness(&state, "quota-baseline", ReadinessStatus::READY, ""),
            "expected ready quota readiness item: {:?}",
            state.readiness_items
        );
    }

    #[tokio::test]
    async fn activate_blocks_when_quota_baseline_unavailable() {
        let now = test_now();
        let repo = Arc::new(MemoryIdentityRepository::default());
        repo.principals.lock().insert(
            "prn_blocked".to_string(),
            active_principal("prn_blocked", now),
        );
        let state_store = Arc::new(MemoryStateStore::default());
        let svc = Service::new(Dependencies {
            state_store: Some(state_store.clone()),
            identity: Some(repo),
            billing: Some(Arc::new(StaticBillingProjector {
                summary: None,
                err: Some(BillingError::QuotaStateUnavailable),
            })),
            chat: None,
            audit: Some(Arc::new(RecordingAuditSink::default())),
            now: Some(Box::new(move || now)),
            environment_scope: "prod".to_string(),
            hosted: true,
        });

        let state = svc
            .activate(ActivateInput {
                token: active_token("tok_blocked", "prn_blocked"),
                tenant_context: TenantContext::default(),
                source: String::new(),
            })
            .await
            .expect("retryable quota blocker must not error");

        assert_eq!(state.status, Status::BLOCKED);
        assert_eq!(state.current_step_id, STEP_QUOTA_BASELINE);
        let baseline = state.quota_baseline.as_ref().expect("quota baseline");
        assert_eq!(baseline.status, QuotaBaselineStatus::UNAVAILABLE);
        assert_eq!(baseline.reason_code, ReasonCode::QUOTA_BASELINE_UNAVAILABLE);
        assert!(!state.first_action.available);
        assert_eq!(
            state.first_action.blocking_item_ids,
            vec!["quota-baseline".to_string()]
        );
        let failure = state.failure_reason.as_ref().expect("failure reason");
        assert_eq!(failure.reason_code, ReasonCode::QUOTA_BASELINE_UNAVAILABLE);
        assert!(failure.retryable);
        assert!(
            has_readiness(
                &state,
                "quota-baseline",
                ReadinessStatus::BLOCKED,
                ReasonCode::QUOTA_BASELINE_UNAVAILABLE
            ),
            "expected blocked quota readiness item"
        );
        let persisted = state_store
            .get_activation_state_for_principal_tenant("prn_blocked", &state.tenant_id)
            .await
            .expect("store")
            .expect("persisted blocked activation");
        assert_eq!(persisted.status, Status::BLOCKED);
    }

    #[tokio::test]
    async fn activate_propagates_unexpected_quota_projection_failures() {
        let now = test_now();
        let repo = Arc::new(MemoryIdentityRepository::default());
        repo.principals.lock().insert(
            "prn_quota_error".to_string(),
            active_principal("prn_quota_error", now),
        );
        let svc = Service::new(Dependencies {
            state_store: Some(Arc::new(MemoryStateStore::default())),
            identity: Some(repo),
            billing: Some(Arc::new(StaticBillingProjector {
                summary: None,
                err: Some(BillingError::Repository(
                    "billing database unavailable".to_string(),
                )),
            })),
            chat: None,
            audit: Some(Arc::new(RecordingAuditSink::default())),
            now: Some(Box::new(move || now)),
            environment_scope: "prod".to_string(),
            hosted: true,
        });

        let err = svc
            .activate(ActivateInput {
                token: active_token("tok_quota_error", "prn_quota_error"),
                tenant_context: TenantContext::default(),
                source: String::new(),
            })
            .await
            .expect_err("projection failure must error");
        assert_eq!(
            reason_code_from_error(&err),
            ReasonCode::QUOTA_BASELINE_UNAVAILABLE,
            "expected stable quota reason for projection failure"
        );
    }
}
