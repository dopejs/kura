//! Activation test chat execution (port of `test_chat.go`), including the
//! evidence-redaction rules for usage metadata.

use chrono::DateTime;
use chrono::Utc;
use kura_identity::AUDIT_OUTCOME_SUCCEEDED;
use serde_json::Map;
use serde_json::Value;

use crate::audit::AuditRecord;
use crate::diagnostics::readiness_item_ids_for_state;
use crate::error::ActivationError;
use crate::error::activation_error;
use crate::readiness::first_non_empty;
use crate::service::Service;
use crate::types::FailureReason;
use crate::types::FailureStage;
use crate::types::ReasonCode;
use crate::types::RemediationOwner;
use crate::types::STEP_COMPLETED;
use crate::types::STEP_TEST_CHAT;
use crate::types::STEP_TEST_CHAT_COMPLETED;
use crate::types::State;
use crate::types::Status;
use crate::types::TestChatMetadata;
use crate::types::TestChatStatus;
use crate::types::default_test_chat_first_action;

const DEFAULT_ACTIVATION_TEST_CHAT_MESSAGE: &str = "Run a safe hosted activation test.";

/// Input handed to the [`crate::ChatRunner`] for one activation test chat.
#[derive(Debug, Clone)]
pub struct TestChatInput {
    pub activation_id: String,
    pub principal_id: String,
    pub tenant_id: String,
    pub environment_scope: String,
    pub message: String,
}

/// Outcome reported by the [`crate::ChatRunner`].
#[derive(Debug, Clone, Default)]
pub struct TestChatResult {
    pub dispatch_id: String,
    pub status: TestChatStatus,
    pub provider: String,
    pub model: String,
    pub usage: Map<String, Value>,
    pub finish_reason: String,
    pub completed_at: Option<DateTime<Utc>>,
}

/// Chat runner failure carrying the partial result, mirroring Go's
/// `(TestChatResult, error)` multi-return where both may be populated.
#[derive(Debug)]
pub struct ChatRunFailure {
    pub result: TestChatResult,
    pub message: String,
}

impl std::fmt::Display for ChatRunFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ChatRunFailure {}

/// [`Service::run_test_chat`] failure: the (possibly defaulted) test chat
/// metadata plus the stable activation error. Go returns all three values.
#[derive(Debug)]
pub struct RunTestChatFailure {
    pub metadata: TestChatMetadata,
    pub source: ActivationError,
}

impl std::fmt::Display for RunTestChatFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.source, f)
    }
}

impl std::error::Error for RunTestChatFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

impl Service {
    /// Runs the hosted activation test chat against the persisted activation
    /// state, persisting either the completed first action or a recoverable
    /// failure, and auditing the transition with metadata-only evidence.
    pub async fn run_test_chat(
        &self,
        input: crate::RunTestChatInput,
    ) -> Result<(State, TestChatMetadata), RunTestChatFailure> {
        self.run_test_chat_inner(input)
            .await
            .map_err(|(metadata, source)| RunTestChatFailure { metadata, source })
    }

    async fn run_test_chat_inner(
        &self,
        input: crate::RunTestChatInput,
    ) -> Result<(State, TestChatMetadata), (TestChatMetadata, ActivationError)> {
        let fail = |source: ActivationError| (TestChatMetadata::default(), source);
        let Some(state_store) = &self.state_store else {
            return Err(fail(Self::not_configured()));
        };
        let mut principal_id = input.tenant_context.principal_id.trim().to_string();
        if principal_id.is_empty() {
            principal_id = input.token.principal_id.trim().to_string();
        }
        let tenant_id = input.tenant_context.tenant_id.trim().to_string();
        if principal_id.is_empty() || tenant_id.is_empty() {
            return Err(fail(activation_error(
                ReasonCode::TENANT_ACCESS_REVOKED.into(),
                FailureStage::AUTHORIZATION.into(),
                false,
                RemediationOwner::PRODUCT_USER.into(),
                "activation tenant context is required",
            )));
        }
        let state = state_store
            .get_activation_state_for_principal_tenant(&principal_id, &tenant_id)
            .await
            .map_err(|err| fail(ActivationError::dependency(err)))?;
        let Some(mut state) = state else {
            return Err(fail(activation_error(
                ReasonCode::TENANT_ACCESS_REVOKED.into(),
                FailureStage::AUTHORIZATION.into(),
                false,
                RemediationOwner::PRODUCT_USER.into(),
                "activation state is not available for this tenant",
            )));
        };
        if state.status == Status::BLOCKED
            || !state.blocking_reason_codes.is_empty()
            || !state.first_action.available
        {
            let (reason, stage) =
                if has_blocking_reason(&state, ReasonCode::QUOTA_BASELINE_UNAVAILABLE) {
                    (
                        ReasonCode::QUOTA_BASELINE_UNAVAILABLE,
                        FailureStage::QUOTA_BASELINE,
                    )
                } else {
                    (ReasonCode::TEST_CHAT_UNAVAILABLE, FailureStage::TEST_CHAT)
                };
            return Err(fail(activation_error(
                reason.into(),
                stage.into(),
                true,
                RemediationOwner::OPERATOR.into(),
                "activation readiness blocks test chat",
            )));
        }
        let Some(chat) = &self.chat else {
            return Err(fail(activation_error(
                ReasonCode::TEST_CHAT_UNAVAILABLE.into(),
                FailureStage::TEST_CHAT.into(),
                true,
                RemediationOwner::OPERATOR.into(),
                "activation test chat runner is not configured",
            )));
        };
        let now = self.now();
        let mut message = input.message.trim().to_string();
        if message.is_empty() {
            message = DEFAULT_ACTIVATION_TEST_CHAT_MESSAGE.to_string();
        }
        let (result, run_error) = match chat
            .run_activation_test_chat(TestChatInput {
                activation_id: state.activation_id.clone(),
                principal_id: principal_id.clone(),
                tenant_id: tenant_id.clone(),
                environment_scope: state.environment_scope.clone(),
                message,
            })
            .await
        {
            Ok(result) => (result, None),
            Err(failure) => (failure.result, Some(failure.message)),
        };
        let completed_at = result.completed_at.unwrap_or(now);
        let mut metadata = TestChatMetadata {
            activation_id: state.activation_id.clone(),
            tenant_id: tenant_id.clone(),
            dispatch_id: result.dispatch_id.trim().to_string(),
            status: result.status,
            provider: result.provider.trim().to_string(),
            model: result.model.trim().to_string(),
            usage: sanitize_usage_metadata(&result.usage),
            finish_reason: result.finish_reason.trim().to_string(),
            reason_code: ReasonCode::default(),
            completed_at: Some(completed_at),
        };
        if metadata.status.is_empty() {
            metadata.status = TestChatStatus::COMPLETED.into();
        }
        if run_error.is_some()
            || metadata.status == TestChatStatus::FAILED
            || metadata.status == TestChatStatus::CANCELLED
        {
            metadata.reason_code = ReasonCode::TEST_CHAT_FAILED.into();
            let message = first_non_empty(&[
                run_error.as_deref().unwrap_or_default(),
                "activation test chat failed",
            ]);
            state.status = Status::ACTIVE.into();
            state.current_step_id = STEP_TEST_CHAT.to_string();
            state.blocking_reason_codes = Vec::new();
            state.first_action = default_test_chat_first_action(true, Vec::new());
            state.test_chat = Some(metadata.clone());
            state.failure_reason = Some(FailureReason {
                reason_code: ReasonCode::TEST_CHAT_FAILED.into(),
                stage: FailureStage::TEST_CHAT.into(),
                retryable: true,
                remediation_owner: RemediationOwner::OPERATOR.into(),
                message: message.clone(),
            });
            state.updated_at = now;
            state.last_evaluated_at = now;
            state_store
                .upsert_activation_state(state.clone())
                .await
                .map_err(|err| {
                    (
                        metadata.clone(),
                        activation_error(
                            ReasonCode::PERSISTENCE_FAILED.into(),
                            FailureStage::PERSISTENCE.into(),
                            true,
                            RemediationOwner::OPERATOR.into(),
                            err.to_string(),
                        ),
                    )
                })?;
            self.record_audit(AuditRecord {
                event_kind: "tenant.activation_failed".to_string(),
                activation_id: state.activation_id.clone(),
                tenant_id: tenant_id.clone(),
                principal_id: principal_id.clone(),
                token_id: input.token.token_id.clone(),
                outcome: "failed".to_string(),
                reason_code: ReasonCode::TEST_CHAT_FAILED.into(),
                stage: FailureStage::TEST_CHAT.into(),
                from_status: Status::ACTIVE.into(),
                to_status: state.status.clone(),
                retryable: true,
                remediation_owner: RemediationOwner::OPERATOR.into(),
                test_chat: Some(metadata.clone()),
                completed_step_ids: state.completed_step_ids.clone(),
                readiness_item_ids: readiness_item_ids_for_state(&state),
                quota_baseline_status: quota_baseline_status_for_audit(&state),
                ..AuditRecord::default()
            })
            .await
            .map_err(|err| (metadata.clone(), err))?;
            return Err((
                metadata,
                activation_error(
                    ReasonCode::TEST_CHAT_FAILED.into(),
                    FailureStage::TEST_CHAT.into(),
                    true,
                    RemediationOwner::OPERATOR.into(),
                    message,
                ),
            ));
        }

        state.status = Status::FIRST_ACTION_COMPLETED.into();
        state.current_step_id = STEP_COMPLETED.to_string();
        state.completed_step_ids =
            append_unique_step(&state.completed_step_ids, STEP_TEST_CHAT_COMPLETED);
        state.blocking_reason_codes = Vec::new();
        state.test_chat = Some(metadata.clone());
        state.failure_reason = None;
        state.first_action_completed_at = Some(completed_at);
        state.updated_at = now;
        state.last_evaluated_at = now;
        state_store
            .upsert_activation_state(state.clone())
            .await
            .map_err(|err| (metadata.clone(), ActivationError::dependency(err)))?;
        self.record_audit(AuditRecord {
            event_kind: "tenant.activation_test_chat_completed".to_string(),
            activation_id: state.activation_id.clone(),
            tenant_id: tenant_id.clone(),
            principal_id: principal_id.clone(),
            token_id: input.token.token_id.clone(),
            outcome: AUDIT_OUTCOME_SUCCEEDED.to_string(),
            stage: FailureStage::TEST_CHAT.into(),
            from_status: Status::ACTIVE.into(),
            to_status: state.status.clone(),
            remediation_owner: RemediationOwner::NONE_REQUIRED.into(),
            test_chat: Some(metadata.clone()),
            completed_step_ids: state.completed_step_ids.clone(),
            readiness_item_ids: readiness_item_ids_for_state(&state),
            quota_baseline_status: quota_baseline_status_for_audit(&state),
            ..AuditRecord::default()
        })
        .await
        .map_err(|err| (metadata.clone(), err))?;
        Ok((state, metadata))
    }
}

fn quota_baseline_status_for_audit(state: &State) -> String {
    state
        .quota_baseline
        .as_ref()
        .map(|baseline| baseline.status.to_string())
        .unwrap_or_default()
}

fn has_blocking_reason(state: &State, reason: &str) -> bool {
    if state
        .blocking_reason_codes
        .iter()
        .any(|item| item.as_str() == reason)
    {
        return true;
    }
    state
        .failure_reason
        .as_ref()
        .is_some_and(|failure| failure.reason_code == reason)
}

fn append_unique_step(items: &[String], step: &str) -> Vec<String> {
    if items.iter().any(|item| item == step) {
        return items.to_vec();
    }
    let mut out = items.to_vec();
    out.push(step.to_string());
    out
}

/// Keeps only scalar numeric/boolean usage counters under non-evidence keys;
/// prompts, transcripts, tokens, and secrets never leave the process.
pub(crate) fn sanitize_usage_metadata(input: &Map<String, Value>) -> Map<String, Value> {
    let mut output = Map::new();
    for (key, value) in input {
        if forbidden_activation_evidence_key(key) {
            continue;
        }
        if value.is_number() || value.is_boolean() {
            output.insert(key.clone(), value.clone());
        }
    }
    output
}

fn forbidden_activation_evidence_key(key: &str) -> bool {
    let normalized: String = key.trim().to_lowercase().replace('_', "");
    match normalized.as_str() {
        "query" | "reply" | "transcript" | "delta" | "prompt" | "rawproviderpayload"
        | "authorization" | "accesstoken" | "refreshtoken" | "secret" => true,
        _ => normalized.contains("secret"),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use kura_identity::TenantContext;
    use serde_json::json;

    use super::*;
    use crate::ActivateInput;
    use crate::Dependencies;
    use crate::GetInput;
    use crate::RunTestChatInput;
    use crate::StateStore;
    use crate::error::reason_code_from_error;
    use crate::testutil::*;

    #[tokio::test]
    async fn test_chat_failure_persists_recoverable_state_and_audit() {
        let now = test_now();
        let repo = Arc::new(MemoryIdentityRepository::default());
        repo.principals.lock().insert(
            "prn_chat_fail".to_string(),
            active_principal("prn_chat_fail", now),
        );
        let state_store = Arc::new(MemoryStateStore::default());
        let audit_sink = Arc::new(RecordingAuditSink::default());
        let svc = Service::new(Dependencies {
            state_store: Some(state_store.clone()),
            identity: Some(repo),
            billing: None,
            chat: Some(Arc::new(FailingChatRunner)),
            audit: Some(audit_sink.clone()),
            now: Some(Box::new(move || now)),
            environment_scope: "test".to_string(),
            hosted: true,
        });
        let state = svc
            .activate(ActivateInput {
                token: active_token("tok_chat_fail", "prn_chat_fail"),
                tenant_context: TenantContext::default(),
                source: String::new(),
            })
            .await
            .expect("activate");

        let failure = svc
            .run_test_chat(RunTestChatInput {
                token: active_token("tok_chat_fail", "prn_chat_fail"),
                tenant_context: tenant_context("prn_chat_fail", &state.tenant_id, "tok_chat_fail"),
                message: "Do not persist this failed prompt.".to_string(),
            })
            .await
            .expect_err("failing chat runner must error");
        assert_eq!(
            reason_code_from_error(&failure.source),
            ReasonCode::TEST_CHAT_FAILED,
            "expected test chat failure reason"
        );
        let metadata = &failure.metadata;
        assert_eq!(metadata.status, TestChatStatus::FAILED);
        assert_eq!(metadata.reason_code, ReasonCode::TEST_CHAT_FAILED);

        let persisted = state_store
            .get_activation_state_for_principal_tenant("prn_chat_fail", &state.tenant_id)
            .await
            .expect("store")
            .expect("persisted recoverable activation state");
        assert_eq!(persisted.status, Status::ACTIVE);
        assert_eq!(persisted.current_step_id, STEP_TEST_CHAT);
        assert!(
            persisted.first_action.available,
            "expected active retryable activation state"
        );
        let failure_reason = persisted.failure_reason.as_ref().expect("failure reason");
        assert_eq!(failure_reason.reason_code, ReasonCode::TEST_CHAT_FAILED);
        assert!(failure_reason.retryable);
        let persisted_chat = persisted.test_chat.as_ref().expect("test chat metadata");
        assert_eq!(persisted_chat.status, TestChatStatus::FAILED);

        let diagnostics = svc
            .diagnostics(GetInput {
                token: active_token("", ""),
                tenant_context: tenant_context("prn_chat_fail", &state.tenant_id, "tok_chat_fail"),
            })
            .await
            .expect("diagnostics");
        assert_eq!(
            diagnostics.len(),
            1,
            "expected test chat diagnostic: {diagnostics:?}"
        );
        assert_eq!(diagnostics[0].stage, FailureStage::TEST_CHAT);
        assert_eq!(diagnostics[0].reason_code, ReasonCode::TEST_CHAT_FAILED);
        assert!(diagnostics[0].test_chat.is_some());

        let payload = audit_sink.payload();
        assert!(payload.contains("tenant.activation_failed"));
        assert!(payload.contains("activation_failed:test_chat"));
        assert!(payload.contains("dispatch_failed"));
        assert!(
            !payload.contains("Do not persist this failed prompt."),
            "audit retained failed test chat prompt: {payload}"
        );
    }

    #[tokio::test]
    async fn test_chat_persists_metadata_only() {
        let now = test_now();
        let repo = Arc::new(MemoryIdentityRepository::default());
        repo.principals.lock().insert(
            "prn_redaction".to_string(),
            active_principal("prn_redaction", now),
        );
        let state_store = Arc::new(MemoryStateStore::default());
        let audit_sink = Arc::new(RecordingAuditSink::default());
        let chat = Arc::new(RecordingChatRunner {
            result: TestChatResult {
                dispatch_id: "dispatch_redacted".to_string(),
                status: TestChatStatus::COMPLETED.into(),
                provider: "test".to_string(),
                model: "test-chat".to_string(),
                usage: serde_json::Map::from_iter([
                    ("inputTokens".to_string(), json!(1)),
                    ("query".to_string(), json!("forbidden query")),
                    ("reply".to_string(), json!("forbidden reply")),
                    ("transcript".to_string(), json!("forbidden transcript")),
                    ("delta".to_string(), json!("forbidden delta")),
                    ("prompt".to_string(), json!("forbidden prompt")),
                    (
                        "rawProviderPayload".to_string(),
                        json!("forbidden raw payload"),
                    ),
                    ("authorization".to_string(), json!("Bearer forbidden")),
                    ("accessToken".to_string(), json!("token")),
                    ("refreshToken".to_string(), json!("refresh")),
                    ("secret".to_string(), json!("secret")),
                ]),
                finish_reason: "stop".to_string(),
                completed_at: Some(now),
            },
            last: parking_lot::Mutex::new(None),
        });
        let svc = Service::new(Dependencies {
            state_store: Some(state_store),
            identity: Some(repo),
            billing: None,
            chat: Some(chat),
            audit: Some(audit_sink.clone()),
            now: Some(Box::new(move || now)),
            environment_scope: "test".to_string(),
            hosted: true,
        });

        let started = svc
            .activate(ActivateInput {
                token: active_token("tok_redaction", "prn_redaction"),
                tenant_context: TenantContext::default(),
                source: String::new(),
            })
            .await
            .expect("activate");
        let (completed, metadata) = svc
            .run_test_chat(RunTestChatInput {
                token: active_token("tok_redaction", "prn_redaction"),
                tenant_context: tenant_context(
                    "prn_redaction",
                    &started.tenant_id,
                    "tok_redaction",
                ),
                message: "Never persist this test chat message.".to_string(),
            })
            .await
            .expect("run test chat");

        let mut payload = serde_json::to_string(&completed).unwrap_or_default();
        payload.push_str(&serde_json::to_string(&metadata).unwrap_or_default());
        payload.push_str(&audit_sink.payload());
        for forbidden in [
            "Never persist this test chat message",
            "forbidden query",
            "forbidden reply",
            "forbidden transcript",
            "forbidden delta",
            "forbidden prompt",
            "forbidden raw payload",
            "Bearer forbidden",
            "accessToken",
            "refreshToken",
            "secret",
        ] {
            assert!(
                !payload.contains(forbidden),
                "activation test chat retained forbidden evidence {forbidden:?}"
            );
        }
        let test_chat = completed.test_chat.as_ref().expect("test chat metadata");
        assert_eq!(
            test_chat.usage.get("inputTokens"),
            Some(&json!(1)),
            "expected safe usage metadata to remain"
        );
    }
}
