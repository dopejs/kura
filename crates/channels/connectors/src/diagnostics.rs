//! Port of daemon/internal/connectors/diagnostics.go — the shared
//! connector-diagnostic classifier: maps a diagnostic reason code onto the
//! lifecycle status, remediation owner, retry safety, severity, and freshness
//! fields of a ConnectorDiagnosticState.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};

use crate::{
    ConnectorDiagnosticState, ConnectorsError, DiagnosticReasonCode, FreshnessState,
    LifecycleState, RedactionStatus, RemediationOwner, RetrySafety,
};

/// Input to [classify_diagnostic] (Go DiagnosticInput). Absent/zero values in
/// Go (empty string reason, zero timestamp) are represented as None.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiagnosticInput {
    pub diagnostic_state_id: String,
    pub tenant_id: String,
    pub connector_id: String,
    pub connector_account_id: String,
    pub reason_code: Option<DiagnosticReasonCode>,
    pub evidence_timestamp: Option<DateTime<Utc>>,
    pub redaction_reliable: bool,
    pub safe_evidence: HashMap<String, String>,
}

/// Classifies a diagnostic into a full ConnectorDiagnosticState.
pub fn classify_diagnostic(
    input: DiagnosticInput,
) -> Result<ConnectorDiagnosticState, ConnectorsError> {
    if input.connector_id.trim().is_empty() {
        return Err(ConnectorsError::DiagnosticConnectorRequired);
    }
    let reason = input
        .reason_code
        .ok_or(ConnectorsError::DiagnosticReasonRequired)?;

    let now = input.evidence_timestamp.unwrap_or_else(Utc::now);

    let (redaction_status, safe_evidence, redaction_failure_id) = if input.redaction_reliable {
        (
            RedactionStatus::Redacted,
            input.safe_evidence,
            String::new(),
        )
    } else {
        (
            RedactionStatus::Suppressed,
            HashMap::new(),
            format!("redaction_failed_{}", input.connector_id),
        )
    };

    let diagnostic_state_id = if input.diagnostic_state_id.trim().is_empty() {
        format!("diag_{}_{}", input.connector_id, reason.as_str())
    } else {
        input.diagnostic_state_id
    };

    Ok(ConnectorDiagnosticState {
        diagnostic_state_id,
        tenant_id: input.tenant_id,
        connector_id: input.connector_id,
        connector_account_id: input.connector_account_id,
        status: status_for_diagnostic(reason),
        reason_code: reason,
        remediation_owner: remediation_for_diagnostic(reason),
        user_visible_severity: severity_for_diagnostic(reason).to_string(),
        retry_safety: retry_safety_for_diagnostic(reason),
        evidence_timestamp: now,
        freshness_state: freshness_at(now, now),
        redaction_status,
        retention_expires_at: now + Duration::days(90),
        safe_evidence,
        redaction_failure_id,
    })
}

/// Go FreshnessAt: stale when the evidence is zero or older than 15 minutes.
pub fn freshness_at(evidence_timestamp: DateTime<Utc>, now: DateTime<Utc>) -> FreshnessState {
    let now = if now == DateTime::<Utc>::UNIX_EPOCH {
        Utc::now()
    } else {
        now
    };
    if evidence_timestamp == DateTime::<Utc>::UNIX_EPOCH
        || now - evidence_timestamp > Duration::minutes(15)
    {
        FreshnessState::Stale
    } else {
        FreshnessState::Fresh
    }
}

/// Go CurrentDiagnosticTruth: reclassify with a fresh failure timestamp.
pub fn current_diagnostic_truth(
    mut input: DiagnosticInput,
    failure_time: DateTime<Utc>,
) -> Result<ConnectorDiagnosticState, ConnectorsError> {
    input.evidence_timestamp = Some(failure_time);
    classify_diagnostic(input)
}

fn status_for_diagnostic(reason: DiagnosticReasonCode) -> LifecycleState {
    match reason {
        DiagnosticReasonCode::AuthMissing => LifecycleState::Failed,
        DiagnosticReasonCode::PermissionMissing => LifecycleState::PermissionBlocked,
        DiagnosticReasonCode::RateLimited => LifecycleState::RateLimited,
        DiagnosticReasonCode::ProviderUnavailable | DiagnosticReasonCode::NetworkFailed => {
            LifecycleState::Degraded
        }
        DiagnosticReasonCode::UnsupportedCapability => LifecycleState::UnsupportedCapability,
        DiagnosticReasonCode::BlockedRoute | DiagnosticReasonCode::DuplicateInbound => {
            LifecycleState::Degraded
        }
        DiagnosticReasonCode::ReplyFailed | DiagnosticReasonCode::UnknownConnectorFailure => {
            LifecycleState::Failed
        }
    }
}

fn remediation_for_diagnostic(reason: DiagnosticReasonCode) -> RemediationOwner {
    match reason {
        DiagnosticReasonCode::AuthMissing => RemediationOwner::User,
        DiagnosticReasonCode::PermissionMissing | DiagnosticReasonCode::BlockedRoute => {
            RemediationOwner::Admin
        }
        DiagnosticReasonCode::RateLimited | DiagnosticReasonCode::ProviderUnavailable => {
            RemediationOwner::Provider
        }
        DiagnosticReasonCode::NetworkFailed
        | DiagnosticReasonCode::ReplyFailed
        | DiagnosticReasonCode::UnknownConnectorFailure => RemediationOwner::Operator,
        DiagnosticReasonCode::UnsupportedCapability | DiagnosticReasonCode::DuplicateInbound => {
            RemediationOwner::NoneRequired
        }
    }
}

fn retry_safety_for_diagnostic(reason: DiagnosticReasonCode) -> RetrySafety {
    match reason {
        DiagnosticReasonCode::RateLimited => RetrySafety::RetryAfter,
        DiagnosticReasonCode::ProviderUnavailable | DiagnosticReasonCode::NetworkFailed => {
            RetrySafety::Retryable
        }
        DiagnosticReasonCode::AuthMissing
        | DiagnosticReasonCode::PermissionMissing
        | DiagnosticReasonCode::BlockedRoute => RetrySafety::Blocked,
        DiagnosticReasonCode::DuplicateInbound | DiagnosticReasonCode::UnsupportedCapability => {
            RetrySafety::NoActionNeeded
        }
        DiagnosticReasonCode::ReplyFailed | DiagnosticReasonCode::UnknownConnectorFailure => {
            RetrySafety::Unsafe
        }
    }
}

fn severity_for_diagnostic(reason: DiagnosticReasonCode) -> &'static str {
    match reason {
        DiagnosticReasonCode::DuplicateInbound | DiagnosticReasonCode::UnsupportedCapability => {
            "info"
        }
        DiagnosticReasonCode::BlockedRoute | DiagnosticReasonCode::RateLimited => "warning",
        _ => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(reason: DiagnosticReasonCode) -> DiagnosticInput {
        DiagnosticInput {
            connector_id: "discord-main".to_string(),
            reason_code: Some(reason),
            redaction_reliable: true,
            safe_evidence: HashMap::from([("stage".to_string(), "transport_start".to_string())]),
            ..DiagnosticInput::default()
        }
    }

    #[test]
    fn requires_connector_and_reason() {
        assert_eq!(
            classify_diagnostic(DiagnosticInput::default()),
            Err(ConnectorsError::DiagnosticConnectorRequired)
        );
        assert_eq!(
            classify_diagnostic(DiagnosticInput {
                connector_id: "c".to_string(),
                ..DiagnosticInput::default()
            }),
            Err(ConnectorsError::DiagnosticReasonRequired)
        );
    }

    #[test]
    fn maps_reason_to_status_owner_retry_severity() {
        let state = classify_diagnostic(input(DiagnosticReasonCode::RateLimited)).unwrap();
        assert_eq!(state.status, LifecycleState::RateLimited);
        assert_eq!(state.remediation_owner, RemediationOwner::Provider);
        assert_eq!(state.retry_safety, RetrySafety::RetryAfter);
        assert_eq!(state.user_visible_severity, "warning");

        let state = classify_diagnostic(input(DiagnosticReasonCode::PermissionMissing)).unwrap();
        assert_eq!(state.status, LifecycleState::PermissionBlocked);
        assert_eq!(state.remediation_owner, RemediationOwner::Admin);
        assert_eq!(state.retry_safety, RetrySafety::Blocked);

        let state = classify_diagnostic(input(DiagnosticReasonCode::AuthMissing)).unwrap();
        assert_eq!(state.status, LifecycleState::Failed);
        assert_eq!(state.remediation_owner, RemediationOwner::User);
        assert_eq!(state.retry_safety, RetrySafety::Blocked);
        assert_eq!(state.user_visible_severity, "error");
    }

    #[test]
    fn defaults_id_timestamp_freshness_retention() {
        let state = classify_diagnostic(input(DiagnosticReasonCode::NetworkFailed)).unwrap();
        assert_eq!(
            state.diagnostic_state_id,
            "diag_discord-main_network_failed"
        );
        assert_eq!(state.freshness_state, FreshnessState::Fresh);
        assert!(state.retention_expires_at > state.evidence_timestamp);
        assert_eq!(state.redaction_status, RedactionStatus::Redacted);
        assert!(!state.safe_evidence.is_empty());
    }

    #[test]
    fn unreliable_redaction_suppresses_evidence() {
        let mut in_ = input(DiagnosticReasonCode::ReplyFailed);
        in_.redaction_reliable = false;
        let state = classify_diagnostic(in_).unwrap();
        assert_eq!(state.redaction_status, RedactionStatus::Suppressed);
        assert!(state.safe_evidence.is_empty());
        assert_eq!(state.redaction_failure_id, "redaction_failed_discord-main");
    }

    #[test]
    fn freshness_at_stale_after_fifteen_minutes() {
        let now = Utc::now();
        assert_eq!(freshness_at(now, now), FreshnessState::Fresh);
        assert_eq!(
            freshness_at(now - Duration::minutes(16), now),
            FreshnessState::Stale
        );
    }
}
