//! Audit recording for activation transitions (port of `audit.go`). Audit
//! writes fail closed: a sink error aborts the transition with the stable
//! retryable `activation_failed:audit_write` reason.

use kura_identity::TenantAuditEvent;
use serde_json::Map;
use serde_json::Value;

use crate::error::ActivationError;
use crate::error::activation_error;
use crate::service::Service;
use crate::types::FailureStage;
use crate::types::ReasonCode;
use crate::types::RemediationOwner;
use crate::types::Status;
use crate::types::TestChatMetadata;

/// One activation audit transition to record.
#[derive(Debug, Default)]
pub(crate) struct AuditRecord {
    pub event_kind: String,
    pub activation_id: String,
    pub tenant_id: String,
    pub principal_id: String,
    pub token_id: String,
    pub outcome: String,
    pub reason_code: ReasonCode,
    pub stage: FailureStage,
    pub from_status: Status,
    pub to_status: Status,
    pub retryable: bool,
    pub remediation_owner: RemediationOwner,
    pub test_chat: Option<TestChatMetadata>,
    pub completed_step_ids: Vec<String>,
    pub readiness_item_ids: Vec<String>,
    pub quota_baseline_status: String,
}

impl Service {
    /// Appends one tenant audit event describing the transition. Without a
    /// configured sink this is a no-op (Go: `s.audit == nil`).
    pub(crate) async fn record_audit(&self, record: AuditRecord) -> Result<(), ActivationError> {
        let Some(audit) = &self.audit else {
            return Ok(());
        };
        let now = self.now();
        let mut document = Map::new();
        document.insert(
            "activationId".to_string(),
            Value::String(record.activation_id),
        );
        document.insert(
            "environmentScope".to_string(),
            Value::String(self.environment_scope.clone()),
        );
        document.insert("stage".to_string(), Value::String(record.stage.to_string()));
        document.insert(
            "fromStatus".to_string(),
            Value::String(record.from_status.to_string()),
        );
        document.insert(
            "toStatus".to_string(),
            Value::String(record.to_status.to_string()),
        );
        document.insert(
            "reasonCode".to_string(),
            Value::String(record.reason_code.to_string()),
        );
        document.insert("retryable".to_string(), Value::Bool(record.retryable));
        document.insert(
            "transitionedAt".to_string(),
            serde_json::to_value(now).unwrap_or(Value::Null),
        );
        if !record.remediation_owner.is_empty() {
            document.insert(
                "remediationOwner".to_string(),
                Value::String(record.remediation_owner.to_string()),
            );
        }
        if let Some(test_chat) = &record.test_chat {
            if let Ok(value) = serde_json::to_value(test_chat) {
                document.insert("testChat".to_string(), value);
            }
        }
        if !record.completed_step_ids.is_empty() {
            document.insert(
                "completedStepIds".to_string(),
                serde_json::to_value(&record.completed_step_ids).unwrap_or(Value::Null),
            );
        }
        if !record.readiness_item_ids.is_empty() {
            document.insert(
                "readinessItemIds".to_string(),
                serde_json::to_value(&record.readiness_item_ids).unwrap_or(Value::Null),
            );
        }
        if !record.quota_baseline_status.is_empty() {
            document.insert(
                "quotaBaselineStatus".to_string(),
                Value::String(record.quota_baseline_status),
            );
        }
        let event = TenantAuditEvent {
            audit_event_id: random_activation_audit_id(),
            event_kind: record.event_kind,
            tenant_id: record.tenant_id,
            principal_id: record.principal_id,
            token_id: record.token_id,
            outcome: record.outcome,
            reason_code: record.reason_code.to_string(),
            created_at: now,
            document: Some(document),
            ..TenantAuditEvent::default()
        };
        audit
            .append_tenant_audit_event(event)
            .await
            .map_err(|err| {
                activation_error(
                    ReasonCode::AUDIT_WRITE_FAILED.into(),
                    FailureStage::AUDIT.into(),
                    true,
                    RemediationOwner::OPERATOR.into(),
                    err.to_string(),
                )
            })?;
        Ok(())
    }
}

/// `audit_activation_<16 lowercase hex chars>` from 8 random bytes. Go falls
/// back to a timestamp when `crypto/rand` fails; `Uuid::new_v4` is infallible
/// (its backend panics on OS RNG failure), so no fallback path is needed.
fn random_activation_audit_id() -> String {
    let bytes = uuid::Uuid::new_v4();
    let mut hex = String::with_capacity(16);
    for byte in &bytes.as_bytes()[..8] {
        hex.push_str(&format!("{byte:02x}"));
    }
    format!("audit_activation_{hex}")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use kura_identity::TenantContext;
    use serde_json::json;

    use crate::ActivateInput;
    use crate::Dependencies;
    use crate::RunTestChatInput;
    use crate::Service;
    use crate::TestChatResult;
    use crate::error::ActivationError;
    use crate::error::reason_code_from_error;
    use crate::testutil::*;
    use crate::types::ReasonCode;
    use crate::types::RemediationOwner;
    use crate::types::TestChatStatus;

    #[tokio::test]
    async fn audit_fail_closed_with_stable_retryable_reason() {
        let now = test_now();
        let repo = Arc::new(MemoryIdentityRepository::default());
        repo.principals.lock().insert(
            "prn_audit_fail".to_string(),
            active_principal("prn_audit_fail", now),
        );
        let svc = Service::new(Dependencies {
            state_store: Some(Arc::new(MemoryStateStore::default())),
            identity: Some(repo),
            billing: None,
            chat: None,
            audit: Some(Arc::new(FailingAuditSink)),
            now: Some(Box::new(move || now)),
            environment_scope: "test".to_string(),
            hosted: true,
        });

        let err = svc
            .activate(ActivateInput {
                token: active_token("tok_audit_fail", "prn_audit_fail"),
                tenant_context: TenantContext::default(),
                source: String::new(),
            })
            .await
            .expect_err("audit failure must abort activation");
        assert_eq!(reason_code_from_error(&err), ReasonCode::AUDIT_WRITE_FAILED);
        let ActivationError::Domain(domain) = &err else {
            panic!("expected domain error, got {err:?}");
        };
        assert!(domain.retryable, "expected retryable operator audit error");
        assert_eq!(domain.remediation_owner, RemediationOwner::OPERATOR);
    }

    #[tokio::test]
    async fn audit_records_metadata_only_test_chat_completion() {
        let now = test_now();
        let repo = Arc::new(MemoryIdentityRepository::default());
        repo.principals.lock().insert(
            "prn_audit_metadata".to_string(),
            active_principal("prn_audit_metadata", now),
        );
        let audit_sink = Arc::new(RecordingAuditSink::default());
        let chat = Arc::new(RecordingChatRunner {
            result: TestChatResult {
                dispatch_id: "dispatch_audit".to_string(),
                status: TestChatStatus::COMPLETED.into(),
                provider: "test".to_string(),
                model: "test-chat".to_string(),
                usage: serde_json::Map::from_iter([("totalTokens".to_string(), json!(2))]),
                finish_reason: String::new(),
                completed_at: Some(now),
            },
            last: parking_lot::Mutex::new(None),
        });
        let svc = Service::new(Dependencies {
            state_store: Some(Arc::new(MemoryStateStore::default())),
            identity: Some(repo),
            billing: None,
            chat: Some(chat.clone()),
            audit: Some(audit_sink.clone()),
            now: Some(Box::new(move || now)),
            environment_scope: "test".to_string(),
            hosted: true,
        });
        let state = svc
            .activate(ActivateInput {
                token: active_token("tok_audit_metadata", "prn_audit_metadata"),
                tenant_context: TenantContext::default(),
                source: String::new(),
            })
            .await
            .expect("activate");
        svc.run_test_chat(RunTestChatInput {
            token: active_token("tok_audit_metadata", "prn_audit_metadata"),
            tenant_context: tenant_context(
                "prn_audit_metadata",
                &state.tenant_id,
                "tok_audit_metadata",
            ),
            message: "Do not audit this prompt.".to_string(),
        })
        .await
        .expect("run test chat");

        let payload = audit_sink.payload();
        assert!(
            payload.contains("tenant.activation_test_chat_completed"),
            "expected completion audit event: {payload}"
        );
        assert!(payload.contains("dispatch_audit") && payload.contains("test-chat"));
        for forbidden in [
            "Do not audit this prompt",
            "query",
            "reply",
            "transcript",
            "rawProviderPayload",
            "authorization",
            "accessToken",
            "refreshToken",
        ] {
            assert!(
                !payload.contains(forbidden),
                "audit retained forbidden evidence {forbidden:?}: {payload}"
            );
        }
    }
}
