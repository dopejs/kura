//! Failure diagnostics projection (port of `diagnostics.go`).

use crate::error::ActivationError;
use crate::error::activation_error;
use crate::service::GetInput;
use crate::service::Service;
use crate::types::Diagnostic;
use crate::types::FailureStage;
use crate::types::ReadinessStatus;
use crate::types::ReasonCode;
use crate::types::RemediationOwner;
use crate::types::State;
use crate::types::TestChatStatus;

impl Service {
    /// Returns failure diagnostics for the persisted activation state, or an
    /// empty list when activation is healthy or not started.
    pub async fn diagnostics(&self, input: GetInput) -> Result<Vec<Diagnostic>, ActivationError> {
        let Some(state_store) = &self.state_store else {
            return Err(Self::not_configured());
        };
        let mut principal_id = input.tenant_context.principal_id.trim().to_string();
        if principal_id.is_empty() {
            principal_id = input.token.principal_id.trim().to_string();
        }
        let tenant_id = input.tenant_context.tenant_id.trim().to_string();
        if principal_id.is_empty() || tenant_id.is_empty() {
            return Err(activation_error(
                ReasonCode::TENANT_ACCESS_REVOKED.into(),
                FailureStage::AUTHORIZATION.into(),
                false,
                RemediationOwner::PRODUCT_USER.into(),
                "activation tenant context is required",
            ));
        }
        let state = state_store
            .get_activation_state_for_principal_tenant(&principal_id, &tenant_id)
            .await
            .map_err(ActivationError::dependency)?;
        let Some(state) = state else {
            return Ok(Vec::new());
        };
        match diagnostic_from_state(&state) {
            Some(diagnostic) => Ok(vec![diagnostic]),
            None => Ok(Vec::new()),
        }
    }
}

fn diagnostic_from_state(state: &State) -> Option<Diagnostic> {
    let reason = state.failure_reason.as_ref();
    if reason.is_none()
        && state.blocking_reason_codes.is_empty()
        && state
            .test_chat
            .as_ref()
            .is_none_or(|test_chat| test_chat.status == TestChatStatus::COMPLETED)
    {
        return None;
    }
    let mut stage = FailureStage::UNEXPECTED.into();
    let mut reason_code: ReasonCode = ReasonCode::UNEXPECTED_FAILED.into();
    let mut retryable = false;
    let mut owner: RemediationOwner = RemediationOwner::OPERATOR.into();
    if let Some(reason) = reason {
        stage = reason.stage.clone();
        reason_code = reason.reason_code.clone();
        retryable = reason.retryable;
        owner = reason.remediation_owner.clone();
    } else if let Some(first) = state.blocking_reason_codes.first() {
        reason_code = first.clone();
        stage = stage_for_reason(first);
        retryable = *first == ReasonCode::QUOTA_BASELINE_UNAVAILABLE
            || *first == ReasonCode::TEST_CHAT_UNAVAILABLE;
    }
    let mut out = Diagnostic {
        activation_id: state.activation_id.clone(),
        tenant_id: state.tenant_id.clone(),
        principal_id: state.principal_id.clone(),
        status: state.status.clone(),
        stage,
        reason_code,
        retryable,
        remediation_owner: owner,
        last_transition_at: state.updated_at,
        readiness_item_ids: readiness_item_ids_for_state(state),
        quota_baseline_status: String::new(),
        test_chat: None,
    };
    if let Some(baseline) = &state.quota_baseline {
        out.quota_baseline_status = baseline.status.to_string();
    }
    if let Some(test_chat) = &state.test_chat {
        if test_chat.status != TestChatStatus::COMPLETED {
            out.test_chat = Some(test_chat.clone());
        }
    }
    Some(out)
}

pub(crate) fn readiness_item_ids_for_state(state: &State) -> Vec<String> {
    state
        .readiness_items
        .iter()
        .filter(|item| item.status == ReadinessStatus::BLOCKED || !item.reason_code.is_empty())
        .map(|item| item.item_id.clone())
        .collect()
}

fn stage_for_reason(reason: &ReasonCode) -> FailureStage {
    match reason.as_str() {
        ReasonCode::QUOTA_BASELINE_UNAVAILABLE => FailureStage::QUOTA_BASELINE.into(),
        ReasonCode::TENANT_ACCESS_REVOKED => FailureStage::AUTHORIZATION.into(),
        ReasonCode::PRINCIPAL_DENIED | ReasonCode::PRINCIPAL_DISABLED => {
            FailureStage::ELIGIBILITY.into()
        }
        ReasonCode::TEST_CHAT_FAILED | ReasonCode::TEST_CHAT_UNAVAILABLE => {
            FailureStage::TEST_CHAT.into()
        }
        _ => FailureStage::UNEXPECTED.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::Dependencies;
    use crate::GetInput;
    use crate::Service;
    use crate::StateStore;
    use crate::readiness::ready_readiness_item;
    use crate::testutil::*;
    use crate::types::ReadinessKind;
    use crate::types::STEP_COMPLETED;
    use crate::types::STEP_QUOTA_BASELINE_READY;
    use crate::types::STEP_TENANT_RESOLVED;
    use crate::types::STEP_TEST_CHAT_COMPLETED;
    use crate::types::Status;
    use crate::types::TestChatMetadata;
    use crate::types::default_test_chat_first_action;

    #[tokio::test]
    async fn diagnostics_does_not_report_completed_test_chat_as_failure() {
        let now = test_now();
        let state_store = Arc::new(MemoryStateStore::default());
        let state = State {
            activation_id: "act_completed".to_string(),
            principal_id: "prn_completed".to_string(),
            tenant_id: "ten_completed".to_string(),
            environment_scope: "test".to_string(),
            status: Status::FIRST_ACTION_COMPLETED.into(),
            current_step_id: STEP_COMPLETED.to_string(),
            completed_step_ids: vec![
                STEP_TENANT_RESOLVED.to_string(),
                STEP_QUOTA_BASELINE_READY.to_string(),
                STEP_TEST_CHAT_COMPLETED.to_string(),
            ],
            blocking_reason_codes: Vec::new(),
            readiness_items: vec![ready_readiness_item(
                "tenant-access",
                ReadinessKind::TENANT_ACCESS.into(),
                "Tenant access",
                now,
            )],
            quota_baseline: None,
            first_action: default_test_chat_first_action(true, Vec::new()),
            test_chat: Some(TestChatMetadata {
                activation_id: "act_completed".to_string(),
                tenant_id: "ten_completed".to_string(),
                dispatch_id: "dispatch_completed".to_string(),
                status: TestChatStatus::COMPLETED.into(),
                provider: "test".to_string(),
                model: "test-chat".to_string(),
                usage: serde_json::Map::from_iter([(
                    "totalTokens".to_string(),
                    serde_json::json!(2),
                )]),
                finish_reason: "stop".to_string(),
                reason_code: ReasonCode::default(),
                completed_at: Some(now),
            }),
            failure_reason: None,
            created_at: now,
            updated_at: now,
            first_action_completed_at: None,
            last_evaluated_at: now,
            last_transition_audit_event: String::new(),
            metadata: None,
        };
        state_store
            .upsert_activation_state(state)
            .await
            .expect("upsert state");
        let svc = Service::new(Dependencies {
            state_store: Some(state_store),
            identity: None,
            billing: None,
            chat: None,
            audit: None,
            now: Some(Box::new(move || now)),
            environment_scope: "test".to_string(),
            hosted: true,
        });

        let items = svc
            .diagnostics(GetInput {
                token: active_token("tok_completed", "prn_completed"),
                tenant_context: tenant_context("prn_completed", "ten_completed", "tok_completed"),
            })
            .await
            .expect("diagnostics");
        assert!(
            items.is_empty(),
            "completed activation should not produce failure diagnostics: {items:?}"
        );
    }
}
