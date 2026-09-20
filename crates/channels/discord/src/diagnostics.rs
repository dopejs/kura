//! Diagnostics (port of diagnostics.go and the diagnostics half of runtime.go):
//! error classification into stable class strings, `diagnostic_reason_for_error`
//! mapping onto the connector diagnostic reason codes, and
//! `build_diagnostic_state` producing the persisted diagnostic state.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};

use kura_connectors::{
    ConnectorDiagnosticState, DiagnosticReasonCode, FreshnessState, LifecycleState,
    RedactionStatus, RemediationOwner, RetrySafety,
};

use crate::DiscordError;

/// Go `classifyDiscordError`: stable class strings derived from the
/// lower-cased error message. Order matters and matches Go exactly.
#[must_use]
pub fn classify_discord_error(err: &(dyn std::error::Error + 'static)) -> String {
    classify_discord_error_message(&err.to_string())
}

/// Message-based classification (Go `classifyDiscordError` over
/// `err.Error()`).
#[must_use]
pub fn classify_discord_error_message(message: &str) -> String {
    let message = message.to_lowercase();
    if contains_any(&message, &["401", "unauthorized", "token"]) {
        return "auth_error".to_string();
    }
    if contains_any(
        &message,
        &["403", "forbidden", "permission", "message content"],
    ) {
        return "permission_missing".to_string();
    }
    if contains_any(&message, &["429", "rate limit"]) {
        return "rate_limited".to_string();
    }
    if contains_any(&message, &["unavailable", "5xx"]) {
        return "provider_unavailable".to_string();
    }
    if contains_any(&message, &["gateway", "network", "connection"]) {
        return "network_failed".to_string();
    }
    "transport_error".to_string()
}

/// Go `DiagnosticReasonForError`: classified errors map their class onto the
/// reason code; everything else falls back to the message scan.
#[must_use]
pub fn diagnostic_reason_for_error(
    err: &(dyn std::error::Error + 'static),
) -> DiagnosticReasonCode {
    if let Some(discord) = err.downcast_ref::<DiscordError>() {
        if let DiscordError::Classified { class, .. } = discord {
            match class.as_str() {
                "auth_error" => return DiagnosticReasonCode::AuthMissing,
                "permission_missing" => return DiagnosticReasonCode::PermissionMissing,
                "rate_limited" => return DiagnosticReasonCode::RateLimited,
                "provider_unavailable" => return DiagnosticReasonCode::ProviderUnavailable,
                "network_failed" => return DiagnosticReasonCode::NetworkFailed,
                _ => {}
            }
        }
    }
    diagnostic_reason_for_error_message(&err.to_string())
}

/// Message-based reason mapping (Go `DiagnosticReasonForError` fallback).
/// The scan order differs from `classify_discord_error_message`: network is
/// matched before provider, and `invalid session` is an auth marker.
#[must_use]
pub fn diagnostic_reason_for_error_message(message: &str) -> DiagnosticReasonCode {
    let message = message.to_lowercase();
    if contains_any(
        &message,
        &["401", "unauthorized", "token", "invalid session"],
    ) {
        return DiagnosticReasonCode::AuthMissing;
    }
    if contains_any(
        &message,
        &["403", "forbidden", "permission", "message content"],
    ) {
        return DiagnosticReasonCode::PermissionMissing;
    }
    if contains_any(&message, &["429", "rate limit"]) {
        return DiagnosticReasonCode::RateLimited;
    }
    if contains_any(&message, &["gateway", "network", "connection"]) {
        return DiagnosticReasonCode::NetworkFailed;
    }
    if contains_any(&message, &["unavailable", "5xx"]) {
        return DiagnosticReasonCode::ProviderUnavailable;
    }
    DiagnosticReasonCode::UnknownConnectorFailure
}

fn contains_any(message: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| message.contains(needle))
}

/// Go `BuildDiagnosticState`: classifies the reason into a persisted
/// diagnostic state with fresh timestamps, redacted evidence, and a 90-day
/// retention window.
///
/// Note: the underlying classification (`classify_diagnostic`) mirrors
/// `ClassifyDiagnostic` from daemon/internal/connectors/diagnostics.go,
/// which is not yet ported into `kura-connectors` (reported as a missing
/// dependency; this local mirror should move there once it lands).
pub fn build_diagnostic_state(
    tenant_id: &str,
    connector_id: &str,
    connector_account_id: &str,
    reason: DiagnosticReasonCode,
    evidence: HashMap<String, String>,
    now: DateTime<Utc>,
) -> Result<ConnectorDiagnosticState, DiscordError> {
    let connector_id = connector_id.trim();
    if connector_id.is_empty() {
        return Err(DiscordError::DiagnosticConnectorRequired);
    }
    classify_diagnostic(
        tenant_id.trim().to_string(),
        connector_id.to_string(),
        connector_account_id.trim().to_string(),
        reason,
        now,
        true,
        evidence,
    )
}

/// Mirror of Go `ClassifyDiagnostic` (diagnostics.go) for
/// `RedactionReliable = true` (the only path the Discord package uses).
fn classify_diagnostic(
    tenant_id: String,
    connector_id: String,
    connector_account_id: String,
    reason: DiagnosticReasonCode,
    mut now: DateTime<Utc>,
    redaction_reliable: bool,
    mut evidence: HashMap<String, String>,
) -> Result<ConnectorDiagnosticState, DiscordError> {
    if now == DateTime::<Utc>::default() {
        now = Utc::now();
    }
    let mut redaction = RedactionStatus::Redacted;
    let mut redaction_failure_id = String::new();
    if !redaction_reliable {
        redaction = RedactionStatus::Suppressed;
        evidence = HashMap::new();
        redaction_failure_id = format!("redaction_failed_{connector_id}");
    }
    let diagnostic_state_id = format!("diag_{connector_id}_{}", reason.as_str());
    Ok(ConnectorDiagnosticState {
        diagnostic_state_id,
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
        retention_expires_at: now + Duration::days(90),
        safe_evidence: evidence,
        redaction_failure_id,
    })
}

/// Go `statusForDiagnostic`.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::wrap_discord_error;

    fn ts() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-05-07T10:00:00Z")
            .expect("parse")
            .with_timezone(&Utc)
    }

    // Go TestDiagnosticReasonForErrorMapsDiscordFailureFamilies
    #[test]
    fn diagnostic_reason_for_error_maps_discord_failure_families() {
        let cases = [
            (
                "auth",
                "401 Unauthorized: invalid token",
                DiagnosticReasonCode::AuthMissing,
            ),
            (
                "permission",
                "403 Forbidden: missing send messages permission",
                DiagnosticReasonCode::PermissionMissing,
            ),
            (
                "rate_limit",
                "429 rate limit exceeded",
                DiagnosticReasonCode::RateLimited,
            ),
            (
                "network",
                "gateway connection reset",
                DiagnosticReasonCode::NetworkFailed,
            ),
            (
                "provider",
                "Discord provider unavailable 5xx",
                DiagnosticReasonCode::ProviderUnavailable,
            ),
        ];
        for (name, message, want) in cases {
            assert_eq!(
                diagnostic_reason_for_error_message(message),
                want,
                "case {name}"
            );
        }
    }

    #[test]
    fn diagnostic_reason_for_error_uses_classified_class() {
        let err = wrap_discord_error("send discord reply", "401 Unauthorized: invalid token");
        assert_eq!(
            diagnostic_reason_for_error(&err),
            DiagnosticReasonCode::AuthMissing
        );
        let err = wrap_discord_error("send discord reply", "403 Forbidden: missing permission");
        assert_eq!(
            diagnostic_reason_for_error(&err),
            DiagnosticReasonCode::PermissionMissing
        );
        let err = wrap_discord_error("open discord session", "429 rate limit exceeded");
        assert_eq!(
            diagnostic_reason_for_error(&err),
            DiagnosticReasonCode::RateLimited
        );
        let err = wrap_discord_error("open discord session", "gateway connection reset");
        assert_eq!(
            diagnostic_reason_for_error(&err),
            DiagnosticReasonCode::NetworkFailed
        );
        let err = wrap_discord_error("send discord reply", "provider unavailable 5xx");
        assert_eq!(
            diagnostic_reason_for_error(&err),
            DiagnosticReasonCode::ProviderUnavailable
        );
        // transport_error class falls through to the message scan.
        let err = wrap_discord_error("send discord reply", "something else broke");
        assert_eq!(
            diagnostic_reason_for_error(&err),
            DiagnosticReasonCode::UnknownConnectorFailure
        );
    }

    #[test]
    fn classify_discord_error_class_strings() {
        assert_eq!(
            classify_discord_error_message("401 Unauthorized"),
            "auth_error"
        );
        assert_eq!(
            classify_discord_error_message("invalid token"),
            "auth_error"
        );
        assert_eq!(
            classify_discord_error_message("403 forbidden"),
            "permission_missing"
        );
        assert_eq!(
            classify_discord_error_message("missing send messages permission"),
            "permission_missing"
        );
        assert_eq!(
            classify_discord_error_message("message content intent not enabled"),
            "permission_missing"
        );
        assert_eq!(
            classify_discord_error_message("429 rate limit"),
            "rate_limited"
        );
        assert_eq!(
            classify_discord_error_message("service unavailable 5xx"),
            "provider_unavailable"
        );
        assert_eq!(
            classify_discord_error_message("gateway connection reset"),
            "network_failed"
        );
        assert_eq!(
            classify_discord_error_message("unknown thing"),
            "transport_error"
        );
    }

    // Go TestBuildDiagnosticStateUsesFreshnessRetentionAndRedactedEvidence
    #[test]
    fn build_diagnostic_state_uses_freshness_retention_and_redacted_evidence() {
        let now = ts();
        let state = build_diagnostic_state(
            "ten_discord",
            "discord-main",
            "acct_redacted",
            DiagnosticReasonCode::PermissionMissing,
            HashMap::from([("permission".to_string(), "send_messages".to_string())]),
            now,
        )
        .expect("build diagnostic state");
        assert_eq!(state.freshness_state, FreshnessState::Fresh);
        assert_eq!(state.retention_expires_at - now, Duration::days(90));
        assert_eq!(
            state.safe_evidence.get("permission").map(String::as_str),
            Some("send_messages")
        );
        assert_eq!(state.redaction_status, RedactionStatus::Redacted);
        assert_eq!(state.status, LifecycleState::PermissionBlocked);
        assert_eq!(state.remediation_owner, RemediationOwner::Admin);
        assert_eq!(state.retry_safety, RetrySafety::Blocked);
        assert_eq!(
            state.diagnostic_state_id,
            "diag_discord-main_permission_missing"
        );
    }

    #[test]
    fn build_diagnostic_state_rejects_missing_connector_id() {
        let err = build_diagnostic_state(
            "ten_discord",
            "  ",
            "acct",
            DiagnosticReasonCode::AuthMissing,
            HashMap::new(),
            ts(),
        )
        .expect_err("connector id required");
        assert!(matches!(
            err,
            crate::DiscordError::DiagnosticConnectorRequired
        ));
    }
}
