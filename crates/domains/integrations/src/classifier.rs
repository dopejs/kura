//! Provider diagnostics classifier (port of `diagnostics_classifier.go` +
//! `diagnostics_remediation.go`): evidence -> reason-code classification, the default
//! reason-code catalog lookup, remediation hints, and the failure projections the calendar
//! and mail managers record on their operation ledgers.

use chrono::{DateTime, SecondsFormat, Utc};
use sha2::{Digest, Sha256};

use crate::{
    DiagnosticFailureProjection, DiagnosticReasonCode, DiagnosticStatus, FreshnessState,
    ProviderErrorClassification, RedactionStatus, RemediationOwner, RetrySafety,
    default_diagnostic_reason_code_catalog, first_non_empty,
};

/// Evidence the classifier consumes. Mirrors `ProviderDiagnosticEvidence`: it carries only
/// redacted, non-secret provider evidence.
#[derive(Debug, Clone, Default)]
pub struct ProviderDiagnosticEvidence {
    pub provider_kind: String,
    pub domain_kind: String,
    pub integration_id: String,
    pub operation_class: String,
    pub provider_error_class: String,
    pub provider_status_code: String,
    pub redacted_provider_code: String,
    pub message: String,
    pub redaction_confidence: String,
    pub side_effecting: bool,
    pub commit_ambiguous: bool,
    pub created_at: DateTime<Utc>,
}

/// Deterministic, opaque diagnostic id (mirrors Go `diagnosticID`): a sha256 of the parts
/// joined by a NUL, truncated to 24 hex chars.
#[must_use]
pub fn diagnostic_id(prefix: &str, parts: &[&str]) -> String {
    let sep = char::from(0).to_string();
    let joined = parts.join(&sep);
    let digest = Sha256::digest(joined.as_bytes());
    let hex = format!("{digest:x}");
    format!("{prefix}_{}", &hex[..24])
}

/// Feishu/Lark-specific reason vocabulary (mirrors `classifyFeishuLarkReason`). Returns the
/// reason when the combined evidence matches a provider-specific token.
#[must_use]
fn classify_feishu_lark_reason(combined: &str) -> Option<DiagnosticReasonCode> {
    let c = combined;
    let reason = if c.contains("99991663")
        || (c.contains("tenant_access_token_invalid") && c.contains("approval"))
    {
        DiagnosticReasonCode::TenantApprovalPending
    } else if c.contains("99991664")
        || c.contains("app_access_token_invalid")
        || c.contains("app_ticket_invalid")
    {
        DiagnosticReasonCode::AppAuthorizationMissing
    } else if c.contains("99991665") || c.contains("bot_not_installed") || c.contains("bot_auth") {
        DiagnosticReasonCode::BotAuthorizationMissing
    } else if c.contains("99991668")
        || c.contains("user_access_token_invalid")
        || c.contains("user_auth")
    {
        DiagnosticReasonCode::UserAuthorizationMissing
    } else if c.contains("99991669")
        || c.contains("scope_not_granted")
        || c.contains("missing_scope")
    {
        DiagnosticReasonCode::ScopeMissing
    } else if c.contains("refresh_token_missing") || c.contains("refresh_credentials_missing") {
        DiagnosticReasonCode::RefreshCredentialsMissing
    } else if c.contains("refresh_token_failed") || c.contains("refresh_failed") {
        DiagnosticReasonCode::TokenRefreshFailed
    } else if c.contains("token_missing") || c.contains("access_token_missing") {
        DiagnosticReasonCode::TokenMissing
    } else if c.contains("token_expired") || c.contains("access_token_expired") {
        DiagnosticReasonCode::TokenExpired
    } else if c.contains("token_revoked") || c.contains("user_token_revoked") {
        DiagnosticReasonCode::TokenRevoked
    } else if c.contains("tenant_mismatch") {
        DiagnosticReasonCode::TenantMismatch
    } else if c.contains("rate_limited") || c.contains("too_many_requests") {
        DiagnosticReasonCode::RateLimited
    } else if c.contains("provider_unavailable") || c.contains("service_unavailable") {
        DiagnosticReasonCode::ProviderUnavailable
    } else if c.contains("network_failed")
        || c.contains("dns_failed")
        || c.contains("connect_timeout")
    {
        DiagnosticReasonCode::NetworkFailed
    } else if c.contains("transient_provider_failure") || c.contains("temporary_failure") {
        DiagnosticReasonCode::TransientProviderFailure
    } else {
        return None;
    };
    Some(reason)
}

/// The provider-agnostic reason classifier (mirrors `classifyReason`).
#[must_use]
fn classify_reason(evidence: &ProviderDiagnosticEvidence) -> DiagnosticReasonCode {
    let raw = format!(
        "{} {} {} {}",
        evidence.provider_error_class,
        evidence.provider_status_code,
        evidence.redacted_provider_code,
        evidence.message
    );
    let combined: String = raw
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();

    let provider_lc = evidence.provider_kind.to_lowercase();
    if provider_lc.contains("feishu") || provider_lc.contains("lark") {
        if let Some(reason) = classify_feishu_lark_reason(&combined) {
            return reason;
        }
    }
    if evidence.commit_ambiguous {
        return DiagnosticReasonCode::AmbiguousDownstreamCommit;
    }
    if evidence.side_effecting && combined.contains("send_permission_required") {
        return DiagnosticReasonCode::UnsafeToRetry;
    }
    if evidence.side_effecting && combined.contains("retry") && combined.contains("unsafe") {
        return DiagnosticReasonCode::UnsafeToRetry;
    }
    if combined.is_empty()
        || combined == "ok"
        || combined == "healthy"
        || combined.contains("status:ok")
    {
        DiagnosticReasonCode::Healthy
    } else if combined.contains("ambiguous")
        && (combined.contains("permission")
            || combined.contains("authorization")
            || combined.contains("scope"))
    {
        DiagnosticReasonCode::UnknownProviderError
    } else if combined.contains("transient") {
        DiagnosticReasonCode::TransientProviderFailure
    } else if combined.contains("app_auth") || combined.contains("app authorization") {
        DiagnosticReasonCode::AppAuthorizationMissing
    } else if combined.contains("bot_auth") || combined.contains("bot authorization") {
        DiagnosticReasonCode::BotAuthorizationMissing
    } else if combined.contains("user_auth")
        || combined.contains("user authorization")
        || combined.contains("auth missing")
    {
        DiagnosticReasonCode::UserAuthorizationMissing
    } else if combined.contains("tenant") && combined.contains("approval") {
        DiagnosticReasonCode::TenantApprovalPending
    } else if combined.contains("scope") {
        DiagnosticReasonCode::ScopeMissing
    } else if combined.contains("refresh") && combined.contains("missing") {
        DiagnosticReasonCode::RefreshCredentialsMissing
    } else if combined.contains("refresh") {
        DiagnosticReasonCode::TokenRefreshFailed
    } else if combined.contains("token") && combined.contains("missing") {
        DiagnosticReasonCode::TokenMissing
    } else if combined.contains("expired")
        || combined.contains("expiry")
        || combined.contains("auth_expiry")
    {
        DiagnosticReasonCode::TokenExpired
    } else if combined.contains("revoked") {
        DiagnosticReasonCode::TokenRevoked
    } else if combined.contains("tenant_mismatch") {
        DiagnosticReasonCode::TenantMismatch
    } else if combined.contains("rate") || combined.contains("429") {
        DiagnosticReasonCode::RateLimited
    } else if combined.contains("network")
        || combined.contains("timeout")
        || combined.contains("slow_response")
    {
        DiagnosticReasonCode::NetworkFailed
    } else if combined.contains("5xx") || combined.contains("unavailable") {
        DiagnosticReasonCode::ProviderUnavailable
    } else if combined.contains("operator_action") {
        DiagnosticReasonCode::OperatorActionNeeded
    } else if combined.contains("unsupported") {
        DiagnosticReasonCode::UnsupportedDiagnostic
    } else {
        DiagnosticReasonCode::UnknownProviderError
    }
}

/// Classify provider evidence into a typed classification (mirrors `ClassifyProviderEvidence`).
#[must_use]
pub fn classify_provider_evidence(
    evidence: &ProviderDiagnosticEvidence,
) -> ProviderErrorClassification {
    let mut now = evidence.created_at;
    if now == DateTime::<Utc>::default() {
        now = Utc::now();
    }
    let mut reason = classify_reason(evidence);
    let (_, mut owner, mut retry_safety) = diagnostic_defaults(reason);
    let mut redaction_status = RedactionStatus::Redacted;
    let mut ambiguous = false;
    if evidence
        .redaction_confidence
        .eq_ignore_ascii_case("uncertain")
    {
        reason = DiagnosticReasonCode::RedactionFailedClosed;
        let (_, o, r) = diagnostic_defaults(reason);
        owner = o;
        retry_safety = r;
        redaction_status = RedactionStatus::FailedClosed;
    }
    if reason == DiagnosticReasonCode::UnknownProviderError
        || reason == DiagnosticReasonCode::AmbiguousDownstreamCommit
    {
        ambiguous = true;
    }
    let ts = now.to_rfc3339_opts(SecondsFormat::Nanos, true);
    let parts: Vec<&str> = vec![
        &evidence.provider_kind,
        &evidence.domain_kind,
        &evidence.integration_id,
        &evidence.operation_class,
        &evidence.provider_error_class,
        &evidence.provider_status_code,
        reason.as_str(),
        &ts,
    ];
    ProviderErrorClassification {
        classification_id: diagnostic_id("diag_class", &parts),
        tenant_id: String::new(),
        provider_kind: first_non_empty(&[&evidence.provider_kind, "unknown"]),
        domain_kind: evidence.domain_kind.trim().to_string(),
        integration_id: evidence.integration_id.trim().to_string(),
        operation_class: evidence.operation_class.trim().to_string(),
        provider_error_class: evidence.provider_error_class.trim().to_string(),
        provider_status_code: evidence.provider_status_code.trim().to_string(),
        redacted_provider_code: evidence.redacted_provider_code.trim().to_string(),
        reason_code: reason,
        retry_safety,
        remediation_owner: owner,
        evidence_confidence: "high".to_string(),
        ambiguous,
        redaction_status,
        created_at: now,
    }
}

/// Default (status, remediation owner, retry safety) for a reason code, falling back to
/// operator-action-needed for an unmapped reason (mirrors `DiagnosticDefaults`).
#[must_use]
pub fn diagnostic_defaults(
    reason: DiagnosticReasonCode,
) -> (DiagnosticStatus, RemediationOwner, RetrySafety) {
    for def in default_diagnostic_reason_code_catalog() {
        if def.reason_code == reason {
            let status = match reason {
                DiagnosticReasonCode::Healthy => DiagnosticStatus::Healthy,
                DiagnosticReasonCode::LimitedDiagnostic => DiagnosticStatus::Degraded,
                DiagnosticReasonCode::UnsupportedDiagnostic => DiagnosticStatus::Unsupported,
                DiagnosticReasonCode::ProviderUnavailable
                | DiagnosticReasonCode::TransientProviderFailure
                | DiagnosticReasonCode::RateLimited
                | DiagnosticReasonCode::NetworkFailed => DiagnosticStatus::Degraded,
                DiagnosticReasonCode::UnknownProviderError
                | DiagnosticReasonCode::OperatorActionNeeded
                | DiagnosticReasonCode::RedactionFailedClosed => DiagnosticStatus::Unknown,
                _ => DiagnosticStatus::Blocked,
            };
            return (
                status,
                def.default_remediation_owner,
                def.default_retry_safety,
            );
        }
    }
    (
        DiagnosticStatus::Unknown,
        RemediationOwner::Operator,
        RetrySafety::OperatorActionNeeded,
    )
}

/// Human-readable remediation hint for a reason code (mirrors `DiagnosticRemediationHint`).
#[must_use]
pub fn diagnostic_remediation_hint(reason: DiagnosticReasonCode) -> String {
    let hint = match reason {
        DiagnosticReasonCode::Healthy => "No operator action is required.",
        DiagnosticReasonCode::AppAuthorizationMissing
        | DiagnosticReasonCode::BotAuthorizationMissing => {
            "Reconnect the provider application or bot credentials."
        }
        DiagnosticReasonCode::UserAuthorizationMissing
        | DiagnosticReasonCode::TokenMissing
        | DiagnosticReasonCode::TokenExpired
        | DiagnosticReasonCode::TokenRevoked => {
            "Ask the affected user to reauthorize the integration account."
        }
        DiagnosticReasonCode::TenantApprovalPending => {
            "Ask a tenant administrator to approve the provider application."
        }
        DiagnosticReasonCode::ScopeMissing => {
            "Ask a tenant administrator to grant the missing provider scope."
        }
        DiagnosticReasonCode::RefreshCredentialsMissing
        | DiagnosticReasonCode::TokenRefreshFailed
        | DiagnosticReasonCode::TenantMismatch => {
            "Review integration credential binding and reconnect the account if needed."
        }
        DiagnosticReasonCode::RateLimited => {
            "Wait for the provider quota window to recover before retrying."
        }
        DiagnosticReasonCode::ProviderUnavailable
        | DiagnosticReasonCode::TransientProviderFailure => "Retry after provider health recovers.",
        DiagnosticReasonCode::NetworkFailed => {
            "Check local network reachability from the daemon environment."
        }
        DiagnosticReasonCode::AmbiguousDownstreamCommit | DiagnosticReasonCode::UnsafeToRetry => {
            "Do not retry automatically; review downstream commit evidence."
        }
        DiagnosticReasonCode::OperatorActionNeeded => {
            "An operator must inspect the integration before retrying."
        }
        DiagnosticReasonCode::LimitedDiagnostic => {
            "Only limited diagnostic dimensions are available for this domain."
        }
        DiagnosticReasonCode::UnsupportedDiagnostic => {
            "Diagnostics are not yet supported for this domain."
        }
        DiagnosticReasonCode::RedactionFailedClosed => {
            "Diagnostic evidence was suppressed because redaction could not be proven."
        }
        DiagnosticReasonCode::UnknownProviderError => {
            "Inspect provider evidence and integration configuration."
        }
    };
    hint.to_string()
}

/// Projection of a diagnostic failure for a single reason code (mirrors `DiagnosticFailureForReason`).
#[must_use]
pub fn diagnostic_failure_for_reason(
    reason: DiagnosticReasonCode,
    checked_at: DateTime<Utc>,
) -> DiagnosticFailureProjection {
    let mut checked_at = checked_at;
    if checked_at == DateTime::<Utc>::default() {
        checked_at = Utc::now();
    }
    let (_, owner, retry_safety) = diagnostic_defaults(reason);
    DiagnosticFailureProjection {
        reason_code: reason,
        remediation_owner: owner,
        remediation_hint: diagnostic_remediation_hint(reason),
        retry_safety,
        freshness_state: FreshnessState::Fresh,
        checked_at,
        redaction_status: RedactionStatus::Redacted,
    }
}

/// Classify an operation failure and project its diagnostic failure (mirrors
/// `DiagnosticFailureForOperationFailure`). The calendar/mail managers call this to record a
/// stable reason on the operation ledger.
#[must_use]
pub fn diagnostic_failure_for_operation_failure(
    domain_kind: &str,
    provider_kind: &str,
    integration_id: &str,
    operation_class: &str,
    failure_class: &str,
    reason: &str,
    side_effecting: bool,
    checked_at: DateTime<Utc>,
) -> DiagnosticFailureProjection {
    let mut checked_at = checked_at;
    if checked_at == DateTime::<Utc>::default() {
        checked_at = Utc::now();
    }
    let classification = classify_provider_evidence(&ProviderDiagnosticEvidence {
        provider_kind: provider_kind.to_string(),
        domain_kind: domain_kind.to_string(),
        integration_id: integration_id.to_string(),
        operation_class: operation_class.to_string(),
        provider_error_class: failure_class.to_string(),
        message: reason.to_string(),
        side_effecting,
        created_at: checked_at,
        ..ProviderDiagnosticEvidence::default()
    });
    diagnostic_failure_for_reason(classification.reason_code, checked_at)
}
