//! Audit recording (port of `audit.go`).

use std::sync::Arc;

use kura_identity::{
    AUDIT_OUTCOME_DENIED, AUDIT_OUTCOME_FAILED_CLOSED, AUDIT_OUTCOME_SUCCEEDED, AuditStore,
    TenantAuditEvent,
};

use crate::helpers::{audit_event_suffix, audit_outcome, first_non_empty};
use crate::service::{AuditRecorder, BoxFuture};
use crate::types::*;

/// Recorder that maps setup transitions onto the tenant audit stream.
pub struct TenantAuditRecorder {
    store: Option<Arc<dyn AuditStore + Send + Sync>>,
}

impl TenantAuditRecorder {
    #[must_use]
    pub fn new(store: Option<Arc<dyn AuditStore + Send + Sync>>) -> Self {
        TenantAuditRecorder { store }
    }
}

impl AuditRecorder for TenantAuditRecorder {
    fn record_setup_audit(
        &self,
        record: SetupAuditRecord,
    ) -> BoxFuture<'_, Result<String, SetupError>> {
        let result = (|| -> Result<String, SetupError> {
            let Some(store) = &self.store else {
                return Ok(String::new());
            };
            let document = document_map(&record);
            let event = TenantAuditEvent {
                event_kind: record.event_kind,
                tenant_id: record.tenant_id,
                principal_id: record.principal_id,
                outcome: tenant_audit_outcome(&record.outcome),
                reason_code: first_non_empty(&[&record.reason_code, "setup_transition"]),
                created_at: record.created_at,
                document: Some(document),
                ..TenantAuditEvent::default()
            };
            let written = store
                .append_tenant_audit_event(event)
                .map_err(|e| SetupError::Store(e.to_string()))?;
            Ok(written.audit_event_id)
        })();
        Box::pin(async move { result })
    }
}

fn document_map(record: &SetupAuditRecord) -> serde_json::Map<String, serde_json::Value> {
    use serde_json::{Map, Value, json};
    let mut document: Map<String, Value> = Map::new();
    document.insert("setupSessionId".to_string(), json!(record.setup_session_id));
    document.insert("targetId".to_string(), json!(record.target_id));
    document.insert("targetKind".to_string(), json!(record.target_kind.as_str()));
    document.insert("setupStyle".to_string(), json!(record.setup_style.as_str()));
    document.insert("operation".to_string(), json!(record.operation.as_str()));
    document.insert("fromState".to_string(), json!(record.from_state));
    document.insert("toState".to_string(), json!(record.to_state));
    document.insert("retryable".to_string(), json!(record.retryable));
    document.insert(
        "remediationOwner".to_string(),
        json!(record.remediation_owner.as_str()),
    );
    document.insert(
        "safeUseMode".to_string(),
        json!(record.safe_use_mode.as_str()),
    );
    document.insert(
        "diagnosticResultId".to_string(),
        json!(record.diagnostic_result_id),
    );
    document.insert(
        "redactionStatus".to_string(),
        json!(record.redaction_status.as_str()),
    );
    document.insert("resourceRefs".to_string(), json!(record.resource_refs));
    document
}

/// Builds the audit record for a completed transition attempt.
#[must_use]
pub fn audit_record_for_attempt(
    session: &SetupSession,
    attempt: &SetupAttempt,
) -> SetupAuditRecord {
    SetupAuditRecord {
        event_kind: format!(
            "credential_setup.{}",
            audit_event_suffix(attempt.operation, attempt.to_state)
        ),
        tenant_id: session.tenant_id.clone(),
        principal_id: first_non_empty(&[&attempt.actor_principal_id, &session.actor_principal_id]),
        setup_session_id: session.setup_session_id.clone(),
        target_id: session.target_id.clone(),
        target_kind: session.target_kind,
        setup_style: session.setup_style,
        operation: attempt.operation,
        from_state: attempt.from_state.clone(),
        to_state: attempt.to_state.clone(),
        reason_code: attempt.reason_code.clone(),
        retryable: session.retryable,
        remediation_owner: session.remediation_owner,
        safe_use_mode: session.safe_use_mode,
        diagnostic_result_id: session.diagnostic_result_id.clone(),
        resource_refs: session.resource_refs.clone(),
        redaction_status: attempt.redaction_status,
        outcome: audit_outcome(attempt.to_state, attempt.redaction_status),
        created_at: attempt.created_at,
    }
}

fn tenant_audit_outcome(outcome: &str) -> String {
    match outcome {
        "failed_closed" => AUDIT_OUTCOME_FAILED_CLOSED.to_string(),
        "denied" => AUDIT_OUTCOME_DENIED.to_string(),
        _ => AUDIT_OUTCOME_SUCCEEDED.to_string(),
    }
}
