//! Pure helpers and redaction (port of `helpers.go` + `redaction.go`).

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::types::*;

// ---------------------------------------------------------------------------
// ID construction
// ---------------------------------------------------------------------------

#[must_use]
pub fn session_id(tenant_id: &str, target_id: &str, style: SetupStyle) -> String {
    format!(
        "setup_{}_{}_{}",
        sanitize_id(tenant_id),
        sanitize_id(target_id),
        sanitize_id(style.as_str())
    )
}

#[must_use]
pub fn attempt_id(session_id: &str, operation: SetupOperation, at: DateTime<Utc>) -> String {
    format!(
        "attempt_{}_{}_{}",
        sanitize_id(session_id),
        sanitize_id(operation.as_str()),
        at.timestamp_nanos_opt().unwrap_or(0)
    )
}

#[must_use]
pub fn oauth_state_ref(session: &SetupSession) -> String {
    format!(
        "oauth_state_{}_{}",
        sanitize_id(&session.tenant_id),
        sanitize_id(&session.setup_session_id)
    )
}

#[must_use]
pub fn sanitize_id(value: &str) -> String {
    let mut out: String = value
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = out.trim_matches('_');
    out = if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed.to_string()
    };
    out
}

#[must_use]
pub fn ensure_diagnostic_id(session: &SetupSession) -> String {
    if !session.diagnostic_result_id.is_empty() {
        return session.diagnostic_result_id.clone();
    }
    format!("diag_{}", sanitize_id(&session.setup_session_id))
}

// ---------------------------------------------------------------------------
// State derivation
// ---------------------------------------------------------------------------

#[must_use]
pub fn safe_use_for_state(session: &SetupSession) -> SafeUseMode {
    match session.state {
        SetupState::Ready => SafeUseMode::Normal,
        SetupState::Degraded => {
            if !session.allowed_capabilities.is_empty()
                && !session.diagnostic_allowed_use.is_empty()
            {
                SafeUseMode::LimitedSafe
            } else {
                SafeUseMode::Blocked
            }
        }
        _ => SafeUseMode::Blocked,
    }
}

#[must_use]
pub fn retryable_for_state(state: SetupState) -> bool {
    matches!(
        state,
        SetupState::ActionRequired
            | SetupState::Unavailable
            | SetupState::Cancelled
            | SetupState::Disabled
            | SetupState::InProgress
    )
}

#[must_use]
pub fn retry_safety_for_state(state: SetupState) -> RetrySafety {
    match state {
        SetupState::Ready => RetrySafety::NoActionNeeded,
        SetupState::Degraded
        | SetupState::ActionRequired
        | SetupState::Unavailable
        | SetupState::Cancelled
        | SetupState::Disabled => RetrySafety::Retryable,
        _ => RetrySafety::Blocked,
    }
}

#[must_use]
pub fn remediation_owner_for_state(state: SetupState, reason: &str) -> RemediationOwner {
    if reason == REASON_PROVIDER_UNAVAILABLE
        || reason == REASON_NETWORK_FAILED
        || reason == REASON_RATE_LIMITED
    {
        return RemediationOwner::Provider;
    }
    if reason == REASON_REDACTION_FAILED_CLOSED || reason == REASON_UNSUPPORTED_TARGET {
        return RemediationOwner::Operator;
    }
    match state {
        SetupState::Ready => RemediationOwner::NoneRequired,
        SetupState::Unavailable => RemediationOwner::Provider,
        _ => RemediationOwner::ProductUser,
    }
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

#[must_use]
pub fn first_non_empty(values: &[&str]) -> String {
    for value in values {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    String::new()
}

#[must_use]
pub fn first_redaction(value: RedactionStatus) -> RedactionStatus {
    if value == RedactionStatus::Redacted
        || value == RedactionStatus::Suppressed
        || value == RedactionStatus::FailedClosed
    {
        value
    } else {
        RedactionStatus::Redacted
    }
}

pub fn upsert_resource_ref(items: &mut Vec<ResourceRef>, next: ResourceRef) {
    for item in items.iter_mut() {
        if item.kind == next.kind && item.id == next.id {
            *item = next.clone();
            return;
        }
    }
    items.push(next);
}

#[must_use]
pub fn contains(items: &[String], value: &str) -> bool {
    let value = value.trim();
    items.iter().any(|item| item.trim() == value)
}

#[must_use]
pub fn audit_event_suffix(operation: SetupOperation, state: SetupState) -> String {
    match operation {
        SetupOperation::Start => "started".to_string(),
        SetupOperation::SubmitSecret => "secret_submitted".to_string(),
        SetupOperation::OAuthStart => "oauth_started".to_string(),
        SetupOperation::OAuthCallback => {
            if state == SetupState::Ready {
                "oauth_completed".to_string()
            } else {
                state.as_str().to_string()
            }
        }
        SetupOperation::Retry => "retried".to_string(),
        SetupOperation::Replace => "replaced".to_string(),
        SetupOperation::Cancel => "cancelled".to_string(),
        SetupOperation::Disable => "disabled".to_string(),
        _ => state.as_str().to_string(),
    }
}

#[must_use]
pub fn audit_outcome(state: SetupState, redaction: RedactionStatus) -> String {
    if redaction == RedactionStatus::FailedClosed {
        return "failed_closed".to_string();
    }
    match state {
        SetupState::Ready | SetupState::Degraded => "succeeded".to_string(),
        SetupState::Cancelled => "cancelled".to_string(),
        _ => "blocked".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Redaction
// ---------------------------------------------------------------------------

const FORBIDDEN_EVIDENCE_FIELD_NAMES: [&str; 10] = [
    "value",
    "authorizationCode",
    "accessToken",
    "refreshToken",
    "providerToken",
    "callbackPayload",
    "Authorization",
    "clientSecret",
    "providerSecret",
    "",
];

#[must_use]
pub fn redacted_secret_evidence(
    secret_ref: &str,
    display_name: &str,
) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    out.insert(
        "redactionRule".to_string(),
        "secret_metadata_only".to_string(),
    );
    out.insert("secretRef".to_string(), secret_ref.trim().to_string());
    let trimmed = display_name.trim();
    if !trimmed.is_empty() {
        out.insert("displayName".to_string(), trimmed.to_string());
    }
    out
}

#[must_use]
pub fn redacted_oauth_evidence(
    result: OAuthResult,
    account_label: &str,
) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    out.insert(
        "redactionRule".to_string(),
        "oauth_metadata_only".to_string(),
    );
    out.insert(
        "authorizationStatus".to_string(),
        result.as_str().to_string(),
    );
    let trimmed = account_label.trim();
    if !trimmed.is_empty() {
        out.insert("accountLabel".to_string(), trimmed.to_string());
    }
    out
}

#[must_use]
pub fn contains_forbidden_evidence<T: Serialize>(value: &T, forbidden_values: &[String]) -> bool {
    let raw = match serde_json::to_string(value) {
        Ok(raw) => raw,
        Err(_) => return true,
    };
    for field in FORBIDDEN_EVIDENCE_FIELD_NAMES {
        if !field.is_empty() && raw.contains(&format!("\"{field}\"")) {
            return true;
        }
    }
    for value in forbidden_values {
        let trimmed = value.trim();
        if !trimmed.is_empty() && raw.contains(trimmed) {
            return true;
        }
    }
    false
}

#[must_use]
pub fn fail_closed(mut session: SetupSession, reason: &str) -> SetupSession {
    session.state = SetupState::ActionRequired;
    session.safe_use_mode = SafeUseMode::Blocked;
    session.reason_code = first_non_empty(&[reason, REASON_REDACTION_FAILED_CLOSED]);
    session.retryable = false;
    session.remediation_owner = RemediationOwner::Operator;
    session.redaction_status = RedactionStatus::FailedClosed;
    session.diagnostic_result_id = ensure_diagnostic_id(&session);
    session
}
