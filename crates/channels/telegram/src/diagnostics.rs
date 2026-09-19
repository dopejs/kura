//! Telegram diagnostic building and error classification (port of
//! diagnostics.go).

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use kura_connectors::{
    ConnectorDiagnosticState, DiagnosticReasonCode, FreshnessState, LifecycleState,
    RedactionStatus, RemediationOwner, RetrySafety,
};

use crate::transport::TelegramApiError;
use crate::{TelegramError, is_unset_time};

/// Go `DiagnosticReasonForError`: maps a classified Telegram error (by error
/// class) or the raw error text onto a stable diagnostic reason code.
#[must_use]
pub fn diagnostic_reason_for_error(
    err: &(dyn std::error::Error + 'static),
) -> DiagnosticReasonCode {
    if let Some(classified) = err.downcast_ref::<TelegramApiError>() {
        match classified.error_class().as_str() {
            "auth_error" => return DiagnosticReasonCode::AuthMissing,
            "permission_missing" => return DiagnosticReasonCode::PermissionMissing,
            "rate_limited" => return DiagnosticReasonCode::RateLimited,
            "provider_unavailable" => return DiagnosticReasonCode::ProviderUnavailable,
            "network_failed" => return DiagnosticReasonCode::NetworkFailed,
            "unsupported_capability" => return DiagnosticReasonCode::UnsupportedCapability,
            _ => {}
        }
    }
    let message = err.to_string().to_lowercase();
    if message.contains("401") || message.contains("unauthorized") || message.contains("token") {
        return DiagnosticReasonCode::AuthMissing;
    }
    if message.contains("403") || message.contains("forbidden") || message.contains("permission") {
        return DiagnosticReasonCode::PermissionMissing;
    }
    if message.contains("429")
        || message.contains("rate limit")
        || message.contains("too many requests")
    {
        return DiagnosticReasonCode::RateLimited;
    }
    if message.contains("unsupported")
        || message.contains("attachment")
        || message.contains("voice")
        || message.contains("payment")
        || message.contains("mini app")
    {
        return DiagnosticReasonCode::UnsupportedCapability;
    }
    if message.contains("unavailable") || message.contains("5xx") {
        return DiagnosticReasonCode::ProviderUnavailable;
    }
    if message.contains("network")
        || message.contains("connection")
        || message.contains("reconnect")
    {
        return DiagnosticReasonCode::NetworkFailed;
    }
    DiagnosticReasonCode::UnknownConnectorFailure
}

/// Go `BuildDiagnosticState`: builds a `ConnectorDiagnosticState` from a
/// reason and (possibly unsafe) evidence. Evidence containing token/secret
/// material is suppressed and the state is marked `Suppressed`.
pub fn build_diagnostic_state(
    tenant_id: &str,
    connector_id: &str,
    connector_account_id: &str,
    reason: DiagnosticReasonCode,
    evidence: HashMap<String, String>,
    now: DateTime<Utc>,
) -> Result<ConnectorDiagnosticState, TelegramError> {
    let tenant_id = tenant_id.trim().to_string();
    let connector_id = connector_id.trim().to_string();
    let connector_account_id = connector_account_id.trim().to_string();
    if connector_id.is_empty() {
        return Err(TelegramError::DiagnosticConnectorRequired);
    }
    let now = if is_unset_time(&now) { Utc::now() } else { now };
    let (redaction, evidence, redaction_failure_id) = if contains_unsafe_evidence(&evidence) {
        (
            RedactionStatus::Suppressed,
            HashMap::new(),
            format!("redaction_failed_{connector_id}"),
        )
    } else {
        (
            RedactionStatus::Redacted,
            safe_evidence(&evidence),
            String::new(),
        )
    };
    Ok(ConnectorDiagnosticState {
        diagnostic_state_id: format!("diag_{connector_id}_{}", reason.as_str()),
        tenant_id,
        connector_id,
        connector_account_id,
        status: status_for_diagnostic(reason),
        reason_code: reason,
        remediation_owner: remediation_for_diagnostic(reason),
        user_visible_severity: severity_for_diagnostic(reason).to_string(),
        retry_safety: retry_safety_for_diagnostic(reason),
        evidence_timestamp: now,
        freshness_state: FreshnessState::Fresh,
        redaction_status: redaction,
        retention_expires_at: now + chrono::Duration::days(90),
        safe_evidence: evidence,
        redaction_failure_id,
    })
}

/// Go `statusForDiagnostic` (from `ClassifyDiagnostic`).
#[must_use]
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

/// Go `remediationForDiagnostic`.
#[must_use]
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

/// Go `retrySafetyForDiagnostic`.
#[must_use]
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

/// Go `severityForDiagnostic`.
#[must_use]
fn severity_for_diagnostic(reason: DiagnosticReasonCode) -> &'static str {
    match reason {
        DiagnosticReasonCode::DuplicateInbound | DiagnosticReasonCode::UnsupportedCapability => {
            "info"
        }
        DiagnosticReasonCode::BlockedRoute | DiagnosticReasonCode::RateLimited => "warning",
        _ => "error",
    }
}

/// Go `containsUnsafeEvidence`: any key/value hinting at token or secret
/// material makes redaction unreliable.
#[must_use]
pub(crate) fn contains_unsafe_evidence(evidence: &HashMap<String, String>) -> bool {
    for (key, value) in evidence {
        let lower_key = key.to_lowercase();
        let lower_value = value.to_lowercase();
        if lower_key.contains("token")
            || lower_key.contains("authorization")
            || lower_value.contains("secret")
            || lower_value.contains("authorization")
            || lower_value.contains("bot token")
        {
            return true;
        }
    }
    false
}

/// Go `safeEvidence`: a copy of the evidence map, or empty when any entry is
/// unsafe.
#[must_use]
pub(crate) fn safe_evidence(evidence: &HashMap<String, String>) -> HashMap<String, String> {
    if contains_unsafe_evidence(evidence) {
        return HashMap::new();
    }
    evidence.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, s)
            .single()
            .expect("valid timestamp")
    }

    // Go TestDiagnosticReasonForErrorMapsTelegramFailures.
    #[test]
    fn diagnostic_reason_for_error_maps_telegram_failures() {
        let cases: [(&str, DiagnosticReasonCode); 6] = [
            (
                "401 unauthorized bot token",
                DiagnosticReasonCode::AuthMissing,
            ),
            (
                "403 forbidden missing permission",
                DiagnosticReasonCode::PermissionMissing,
            ),
            (
                "429 too many requests rate limit",
                DiagnosticReasonCode::RateLimited,
            ),
            (
                "telegram provider unavailable 5xx",
                DiagnosticReasonCode::ProviderUnavailable,
            ),
            (
                "network connection reset by peer",
                DiagnosticReasonCode::NetworkFailed,
            ),
            (
                "unsupported attachment voice input",
                DiagnosticReasonCode::UnsupportedCapability,
            ),
        ];
        for (message, want) in cases {
            let err = std::io::Error::new(std::io::ErrorKind::Other, message);
            assert_eq!(diagnostic_reason_for_error(&err), want, "{message}");
        }
    }

    // Go TestBuildDiagnosticStateRedactsUnsafeEvidence.
    #[test]
    fn build_diagnostic_state_redacts_unsafe_evidence() {
        let state = build_diagnostic_state(
            "ten_telegram",
            "telegram-main",
            "bot_redacted",
            DiagnosticReasonCode::AuthMissing,
            HashMap::from([
                ("token".to_string(), "123:SECRET".to_string()),
                ("hint".to_string(), "safe".to_string()),
            ]),
            ts(2026, 5, 8, 10, 0, 0),
        )
        .expect("build diagnostic state");
        assert_eq!(state.redaction_status, RedactionStatus::Suppressed);
        assert!(!state.redaction_failure_id.is_empty());
        for value in state.safe_evidence.values() {
            assert!(!value.contains("SECRET"));
        }
    }
}
