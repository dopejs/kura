//! Port of daemon/internal/connectors/matrix/diagnostics.go: Matrix condition
//! diagnostics with freshness and redaction-suppression semantics.
//!
//! The base classification helpers (status/remediation/retry-safety/severity
//! per reason code and the 15-minute freshness rule) are ported locally from
//! daemon/internal/connectors/diagnostics.go because kura-connectors currently
//! exposes only the diagnostic *types*; the shared types are imported.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use kura_connectors::{
    ConnectorDiagnosticState, DiagnosticReasonCode, FreshnessState, LifecycleState,
    RedactionStatus, RemediationOwner, RetrySafety,
};
use serde::{Deserialize, Serialize};

use crate::is_unset_time;
use crate::string_enum;

// Go `MatrixCondition`.
string_enum!(MatrixCondition {
    BotAuthInvalid => "bot_auth_invalid",
    BotAuthRevoked => "bot_auth_revoked",
    RoomPermissionMissing => "room_permission_missing",
    OwnershipMismatch => "ownership_mismatch",
    HomeserverUnsupported => "homeserver_unsupported",
    HomeserverUnreachable => "homeserver_unreachable",
    FederationFailed => "federation_failed",
    RateLimited => "rate_limited",
    ProviderUnavailable => "provider_unavailable",
    NetworkFailed => "network_failed",
    BlockedRoute => "blocked_route",
    DuplicateEvent => "duplicate_event",
    EncryptedRoomUnsupported => "encrypted_room_unsupported",
    UndecryptableEvent => "undecryptable_event",
    UnsupportedSurface => "unsupported_surface",
    ReplyFailed => "reply_failed",
    Unknown => "unknown",
});

/// Go `DiagnosticInput`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticInput {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_id: String,
    pub evidence_timestamp: DateTime<Utc>,
    pub now: DateTime<Utc>,
    pub redaction_reliable: bool,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub safe_evidence: HashMap<String, String>,
}

/// Go `DiagnosticState`: the base connector diagnostic state flattened with
/// the Matrix-specific condition (Go embeds ConnectorDiagnosticState).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticState {
    #[serde(flatten)]
    pub base: ConnectorDiagnosticState,
    pub matrix_condition: MatrixCondition,
}

/// Go `MapCondition`: classifies a Matrix condition into a diagnostic state
/// with the evidence-timestamp freshness, suppressing safe evidence when
/// redaction is not reliable.
#[must_use]
pub fn map_condition(condition: MatrixCondition, input: DiagnosticInput) -> DiagnosticState {
    let mut now = input.now;
    if is_unset_time(&now) {
        now = Utc::now();
    }
    let mut evidence_at = input.evidence_timestamp;
    if is_unset_time(&evidence_at) {
        evidence_at = now;
    }
    let redaction_reliable = input.redaction_reliable;
    let safe_evidence = if redaction_reliable {
        input.safe_evidence
    } else {
        HashMap::new()
    };
    // Go ignores the classification error (zero state) and always applies the
    // freshness override; unwrap_or_default mirrors that.
    let mut base = classify_diagnostic(DiagnosticInputInternal {
        tenant_id: input.tenant_id,
        connector_id: input.connector_id,
        reason_code: reason_for_condition(condition),
        evidence_timestamp: evidence_at,
        redaction_reliable,
        safe_evidence,
    })
    .unwrap_or_default();
    base.freshness_state = freshness_at(evidence_at, now);
    DiagnosticState {
        base,
        matrix_condition: condition,
    }
}

/// Go `reasonForCondition`.
#[must_use]
pub fn reason_for_condition(condition: MatrixCondition) -> DiagnosticReasonCode {
    match condition {
        MatrixCondition::BotAuthInvalid | MatrixCondition::BotAuthRevoked => {
            DiagnosticReasonCode::AuthMissing
        }
        MatrixCondition::RoomPermissionMissing | MatrixCondition::OwnershipMismatch => {
            DiagnosticReasonCode::PermissionMissing
        }
        MatrixCondition::HomeserverUnsupported
        | MatrixCondition::EncryptedRoomUnsupported
        | MatrixCondition::UndecryptableEvent
        | MatrixCondition::UnsupportedSurface => DiagnosticReasonCode::UnsupportedCapability,
        MatrixCondition::RateLimited => DiagnosticReasonCode::RateLimited,
        MatrixCondition::ProviderUnavailable => DiagnosticReasonCode::ProviderUnavailable,
        MatrixCondition::HomeserverUnreachable
        | MatrixCondition::FederationFailed
        | MatrixCondition::NetworkFailed => DiagnosticReasonCode::NetworkFailed,
        MatrixCondition::BlockedRoute => DiagnosticReasonCode::BlockedRoute,
        MatrixCondition::DuplicateEvent => DiagnosticReasonCode::DuplicateInbound,
        MatrixCondition::ReplyFailed => DiagnosticReasonCode::ReplyFailed,
        _ => DiagnosticReasonCode::UnknownConnectorFailure,
    }
}

/// Port of daemon/internal/connectors/diagnostics.go `ClassifyDiagnostic`.
fn classify_diagnostic(input: DiagnosticInputInternal) -> Result<ConnectorDiagnosticState, String> {
    if input.connector_id.trim().is_empty() {
        return Err("connector id is required".to_string());
    }
    let mut now = input.evidence_timestamp;
    if is_unset_time(&now) {
        now = Utc::now();
    }
    let (redaction, evidence, redaction_failure_id) = if input.redaction_reliable {
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
    Ok(ConnectorDiagnosticState {
        diagnostic_state_id: format!("diag_{}_{}", input.connector_id, input.reason_code.as_str()),
        tenant_id: input.tenant_id,
        connector_id: input.connector_id,
        connector_account_id: String::new(),
        status: status_for_diagnostic(input.reason_code),
        reason_code: input.reason_code,
        remediation_owner: remediation_for_diagnostic(input.reason_code),
        user_visible_severity: severity_for_diagnostic(input.reason_code).to_string(),
        retry_safety: retry_safety_for_diagnostic(input.reason_code),
        evidence_timestamp: now,
        freshness_state: freshness_at(now, now),
        redaction_status: redaction,
        retention_expires_at: now + Duration::days(90),
        safe_evidence: evidence,
        redaction_failure_id,
    })
}

struct DiagnosticInputInternal {
    tenant_id: String,
    connector_id: String,
    reason_code: DiagnosticReasonCode,
    evidence_timestamp: DateTime<Utc>,
    redaction_reliable: bool,
    safe_evidence: HashMap<String, String>,
}

/// Port of connectors `FreshnessAt`: evidence older than 15 minutes is stale.
#[must_use]
pub fn freshness_at(evidence_timestamp: DateTime<Utc>, now: DateTime<Utc>) -> FreshnessState {
    let now = if is_unset_time(&now) { Utc::now() } else { now };
    if is_unset_time(&evidence_timestamp) || (now - evidence_timestamp) > Duration::minutes(15) {
        FreshnessState::Stale
    } else {
        FreshnessState::Fresh
    }
}

/// Port of connectors `statusForDiagnostic`.
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

/// Port of connectors `remediationForDiagnostic`.
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

/// Port of connectors `retrySafetyForDiagnostic`.
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

/// Port of connectors `severityForDiagnostic`.
fn severity_for_diagnostic(reason: DiagnosticReasonCode) -> &'static str {
    match reason {
        DiagnosticReasonCode::DuplicateInbound | DiagnosticReasonCode::UnsupportedCapability => {
            "info"
        }
        DiagnosticReasonCode::BlockedRoute | DiagnosticReasonCode::RateLimited => "warning",
        DiagnosticReasonCode::AuthMissing
        | DiagnosticReasonCode::PermissionMissing
        | DiagnosticReasonCode::ProviderUnavailable
        | DiagnosticReasonCode::NetworkFailed
        | DiagnosticReasonCode::ReplyFailed
        | DiagnosticReasonCode::UnknownConnectorFailure => "error",
    }
}
