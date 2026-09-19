//! Diagnostic probing (port of `probe.go` + `diagnostics.go`).

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};

use crate::helpers::{ensure_diagnostic_id, retry_safety_for_state, sanitize_id};
use crate::service::{BoxFuture, DiagnosticProbe, SecretManager};
use crate::types::*;

/// The built-in diagnostic probe: classifies readiness from resource refs and (when a
/// secrets manager is present) verifies a submitted credential exists.
pub struct DefaultDiagnosticProbe {
    pub secrets: Option<Arc<dyn SecretManager>>,
}

impl DefaultDiagnosticProbe {
    #[must_use]
    pub fn new(secrets: Option<Arc<dyn SecretManager>>) -> Self {
        DefaultDiagnosticProbe { secrets }
    }
}

impl DiagnosticProbe for DefaultDiagnosticProbe {
    fn probe_setup(
        &self,
        session: &SetupSession,
        operation: SetupOperation,
    ) -> BoxFuture<'_, Result<SetupDiagnosticProbeResult, SetupError>> {
        let secrets = self.secrets.clone();
        let session = session.clone();
        Box::pin(async move {
            let mut classified = classify(&session, operation);

            // SubmitSecret additionally verifies the submitted credential exists.
            if operation == SetupOperation::SubmitSecret {
                if let Some(secrets) = &secrets {
                    let secret_ref = resource_ref_id(&session.resource_refs, "tenant_secret");
                    if secret_ref.is_empty() {
                        let (st, o, r) = classify_diagnostic_reason(REASON_CREDENTIAL_MISSING);
                        classified = (st, REASON_CREDENTIAL_MISSING.to_string(), r, o);
                    } else {
                        match secrets.get(&session.tenant_id, &secret_ref).await {
                            Ok(_) => {}
                            Err(SetupError::Secrets(
                                kura_secrets::SecretsError::SecretNotFound,
                            )) => {
                                let (st, o, r) =
                                    classify_diagnostic_reason(REASON_CREDENTIAL_MISSING);
                                classified = (st, REASON_CREDENTIAL_MISSING.to_string(), r, o);
                            }
                            Err(_) => {
                                let (st, o, r) =
                                    classify_diagnostic_reason(REASON_PROVIDER_UNAVAILABLE);
                                classified = (st, REASON_PROVIDER_UNAVAILABLE.to_string(), r, o);
                            }
                        }
                    }
                }
            }

            Ok(SetupDiagnosticProbeResult {
                state: classified.0,
                reason_code: classified.1,
                retry_safety: classified.2,
                remediation_owner: classified.3,
                allowed_capabilities: Vec::new(),
                diagnostic_result_id: default_diagnostic_result_id(&session, operation),
                diagnostic_run_id: default_diagnostic_run_id(&session, operation),
                diagnostic_stage: diagnostic_stage_for_operation(operation),
                diagnostic_source: DiagnosticSource {
                    kind: default_diagnostic_source_kind(&session),
                    id: session.target_id.clone(),
                },
            })
        })
    }
}

type Classification = (SetupState, String, RetrySafety, RemediationOwner);

fn classify(session: &SetupSession, operation: SetupOperation) -> Classification {
    match operation {
        SetupOperation::SubmitSecret => {
            if session.target_id == TARGET_DISCORD_CONNECTOR {
                return connector_classification(
                    session,
                    "discord_destination_validation",
                    "discord_destination_invalid",
                    REASON_DISCORD_DESTINATION_INVALID,
                    REASON_DISCORD_DESTINATION_MISSING,
                );
            }
            if session.target_id == TARGET_TELEGRAM_CONNECTOR {
                return connector_classification(
                    session,
                    "telegram_allowment_validation",
                    "telegram_allowment_invalid",
                    REASON_TELEGRAM_ALLOWMENT_INVALID,
                    REASON_TELEGRAM_ALLOWMENT_MISSING,
                );
            }
            healthy()
        }
        SetupOperation::OAuthCallback => {
            if resource_ref_id(&session.resource_refs, "provider_auth_state").is_empty() {
                let (s, o, r) = classify_diagnostic_reason(REASON_TOKEN_MISSING);
                return (s, REASON_TOKEN_MISSING.to_string(), r, o);
            }
            if session.target_id == TARGET_SLACK_CONNECTOR {
                if !resource_ref_id(&session.resource_refs, "slack_route_policy_validation")
                    .is_empty()
                {
                    return healthy();
                }
                if !resource_ref_id(&session.resource_refs, "slack_route_policy_invalid").is_empty()
                {
                    let (s, o, r) = classify_diagnostic_reason(REASON_SLACK_ROUTE_POLICY_INVALID);
                    return (s, REASON_SLACK_ROUTE_POLICY_INVALID.to_string(), r, o);
                }
                let (s, o, r) = classify_diagnostic_reason(REASON_SLACK_ROUTE_POLICY_MISSING);
                return (s, REASON_SLACK_ROUTE_POLICY_MISSING.to_string(), r, o);
            }
            healthy()
        }
        _ => healthy(),
    }
}

fn healthy() -> Classification {
    (
        SetupState::Ready,
        REASON_HEALTHY.to_string(),
        RetrySafety::NoActionNeeded,
        RemediationOwner::NoneRequired,
    )
}

fn connector_classification(
    session: &SetupSession,
    ok_kind: &str,
    invalid_kind: &str,
    invalid_reason: &str,
    missing_reason: &str,
) -> Classification {
    if !resource_ref_id(&session.resource_refs, ok_kind).is_empty() {
        return healthy();
    }
    if !resource_ref_id(&session.resource_refs, invalid_kind).is_empty() {
        let (s, o, r) = classify_diagnostic_reason(invalid_reason);
        return (s, invalid_reason.to_string(), r, o);
    }
    let (s, o, r) = classify_diagnostic_reason(missing_reason);
    (s, missing_reason.to_string(), r, o)
}

#[must_use]
pub fn resource_ref_id(items: &[ResourceRef], kind: &str) -> String {
    items
        .iter()
        .find(|item| item.kind == kind)
        .map(|item| item.id.clone())
        .unwrap_or_default()
}

#[must_use]
pub fn diagnostic_stage_for_operation(operation: SetupOperation) -> String {
    match operation {
        SetupOperation::SubmitSecret => "credential_probe".to_string(),
        SetupOperation::OAuthCallback => "oauth_probe".to_string(),
        SetupOperation::DiagnosticProbe => "diagnostic_probe".to_string(),
        _ => operation.as_str().to_string(),
    }
}

#[must_use]
pub fn default_diagnostic_source_kind(session: &SetupSession) -> String {
    match session.target_kind {
        TargetKind::Provider => "provider_check".to_string(),
        TargetKind::Integration => "integration_diagnostic".to_string(),
        _ => "setup_probe".to_string(),
    }
}

#[must_use]
pub fn default_diagnostic_result_id(session: &SetupSession, operation: SetupOperation) -> String {
    format!(
        "diag_{}_{}",
        sanitize_id(&session.setup_session_id),
        sanitize_id(operation.as_str())
    )
}

#[must_use]
pub fn default_diagnostic_run_id(session: &SetupSession, operation: SetupOperation) -> String {
    format!(
        "diag_run_{}_{}",
        sanitize_id(&session.setup_session_id),
        sanitize_id(operation.as_str())
    )
}

#[must_use]
pub fn classify_diagnostic_reason(reason: &str) -> (SetupState, RemediationOwner, RetrySafety) {
    match reason {
        REASON_HEALTHY => (
            SetupState::Ready,
            RemediationOwner::NoneRequired,
            RetrySafety::NoActionNeeded,
        ),
        REASON_SCOPE_MISSING
        | REASON_TENANT_APPROVAL_PENDING
        | REASON_CREDENTIAL_MISSING
        | REASON_TOKEN_MISSING
        | REASON_TOKEN_EXPIRED
        | REASON_TOKEN_REVOKED
        | REASON_OAUTH_DENIED
        | REASON_OAUTH_EXPIRED
        | REASON_OAUTH_REPLAY
        | REASON_TENANT_MISMATCH => (
            SetupState::ActionRequired,
            RemediationOwner::TenantAdmin,
            RetrySafety::Retryable,
        ),
        REASON_DISCORD_DESTINATION_MISSING | REASON_DISCORD_DESTINATION_INVALID => (
            SetupState::Degraded,
            RemediationOwner::TenantAdmin,
            RetrySafety::Retryable,
        ),
        REASON_TELEGRAM_ALLOWMENT_MISSING
        | REASON_TELEGRAM_ALLOWMENT_INVALID
        | REASON_SLACK_ROUTE_POLICY_MISSING
        | REASON_SLACK_ROUTE_POLICY_INVALID
        | REASON_MATRIX_ROUTE_POLICY_MISSING
        | REASON_MATRIX_ROUTE_POLICY_INVALID
        | REASON_MATRIX_OWNERSHIP_MISMATCH => (
            SetupState::ActionRequired,
            RemediationOwner::TenantAdmin,
            RetrySafety::Blocked,
        ),
        REASON_PROVIDER_UNAVAILABLE | REASON_NETWORK_FAILED | REASON_RATE_LIMITED => (
            SetupState::Unavailable,
            RemediationOwner::Provider,
            RetrySafety::Retryable,
        ),
        REASON_UNSUPPORTED_TARGET | REASON_REDACTION_FAILED_CLOSED => (
            SetupState::ActionRequired,
            RemediationOwner::Operator,
            RetrySafety::Blocked,
        ),
        _ => (
            SetupState::Unavailable,
            RemediationOwner::Operator,
            RetrySafety::Retryable,
        ),
    }
}

/// Builds the diagnostic projection for a session.
#[must_use]
pub fn diagnostic_for_session(session: &SetupSession, now: DateTime<Utc>) -> SetupDiagnostic {
    let mut reason = session.reason_code.clone();
    if reason.is_empty() && session.state == SetupState::Ready {
        reason = REASON_HEALTHY.to_string();
    }
    SetupDiagnostic {
        setup_session_id: session.setup_session_id.clone(),
        target_id: session.target_id.clone(),
        diagnostic_result_id: ensure_diagnostic_id(session),
        diagnostic_run_id: session.diagnostic_run_id.clone(),
        diagnostic_stage: session.diagnostic_stage.clone(),
        diagnostic_source_kind: session.diagnostic_source_kind.clone(),
        diagnostic_source_id: session.diagnostic_source_id.clone(),
        status: session.state,
        reason_code: reason,
        retry_safety: retry_safety_for_state(session.state),
        remediation_owner: session.remediation_owner,
        allowed_capabilities: session.diagnostic_allowed_use.clone(),
        checked_at: now,
        stale_after: now + Duration::minutes(15),
        redaction_status: session.redaction_status,
    }
}
