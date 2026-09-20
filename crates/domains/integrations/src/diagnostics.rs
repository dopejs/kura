//! Diagnostics types (port of `diagnostics_types.go`): reason-code catalog,
//! classification/result/run shapes, and freshness/retention helpers. The
//! classifier/manager/redaction/remediation logic lands in a later increment.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const DIAGNOSTIC_DEFAULT_RETENTION: Duration = Duration::from_secs(90 * 24 * 3600);
pub const DIAGNOSTIC_STALE_AFTER: Duration = Duration::from_secs(15 * 60);

macro_rules! string_enum {
    ($name:ident { $first:ident => $first_s:literal $(, $v:ident => $s:literal)* $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
        pub enum $name {
            #[default]
            #[serde(rename = $first_s)]
            $first,
            $(#[serde(rename = $s)] $v),*
        }
        impl $name {
            #[must_use]
            pub fn as_str(self) -> &'static str {
                match self {
                    $name::$first => $first_s,
                    $( $name::$v => $s ),*
                }
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

string_enum!(DiagnosticStatus {
    Unknown => "unknown",
    Healthy => "healthy",
    Degraded => "degraded",
    Blocked => "blocked",
    Unsupported => "unsupported",
});

string_enum!(DiagnosticReasonCode {
    Healthy => "healthy",
    AppAuthorizationMissing => "app_authorization_missing",
    BotAuthorizationMissing => "bot_authorization_missing",
    UserAuthorizationMissing => "user_authorization_missing",
    TenantApprovalPending => "tenant_approval_pending",
    ScopeMissing => "scope_missing",
    TokenMissing => "token_missing",
    TokenExpired => "token_expired",
    TokenRevoked => "token_revoked",
    RefreshCredentialsMissing => "refresh_credentials_missing",
    TokenRefreshFailed => "token_refresh_failed",
    TenantMismatch => "tenant_mismatch",
    RateLimited => "rate_limited",
    ProviderUnavailable => "provider_unavailable",
    TransientProviderFailure => "transient_provider_failure",
    NetworkFailed => "network_failed",
    AmbiguousDownstreamCommit => "ambiguous_downstream_commit",
    UnsafeToRetry => "unsafe_to_retry",
    OperatorActionNeeded => "operator_action_needed",
    LimitedDiagnostic => "limited_diagnostic",
    UnsupportedDiagnostic => "unsupported_diagnostic",
    RedactionFailedClosed => "redaction_failed_closed",
    UnknownProviderError => "unknown_provider_error",
});

string_enum!(RetrySafety {
    NoActionNeeded => "no_action_needed",
    Retryable => "retryable",
    Blocked => "blocked",
    UnsafeToRetry => "unsafe_to_retry",
    OperatorActionNeeded => "operator_action_needed",
});

string_enum!(RemediationOwner {
    ProductUser => "product_user",
    TenantAdmin => "tenant_admin",
    Operator => "operator",
    Provider => "provider",
    NoneRequired => "none_required",
});

string_enum!(FreshnessState {
    Fresh => "fresh",
    Stale => "stale",
});

string_enum!(RedactionStatus {
    Redacted => "redacted",
    Suppressed => "suppressed",
    FailedClosed => "failed_closed",
});

string_enum!(DiagnosticRunStatus {
    Queued => "queued",
    Running => "running",
    Completed => "completed",
    Failed => "failed",
    Blocked => "blocked",
});

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticResult {
    pub diagnostic_result_id: String,
    pub tenant_id: String,
    pub integration_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub integration_account_id: String,
    pub domain_kind: String,
    pub provider_kind: String,
    pub capability: String,
    pub status: DiagnosticStatus,
    pub reason_code: DiagnosticReasonCode,
    pub remediation_owner: RemediationOwner,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub remediation_hint: String,
    pub retry_safety: RetrySafety,
    pub checked_at: DateTime<Utc>,
    pub stale_after: DateTime<Utc>,
    pub freshness_state: FreshnessState,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub run_id: String,
    pub redaction_status: RedactionStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub evidence_summary: String,
    pub retention_expires_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub smoke_report_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticRun {
    pub diagnostic_run_id: String,
    pub tenant_id: String,
    pub integration_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub integration_account_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub domain_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider_kind: String,
    pub requested_by: String,
    pub trigger: String,
    pub status: DiagnosticRunStatus,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    pub checked_capabilities: Vec<String>,
    pub result_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub failure_reason_code: String,
    pub redaction_status: RedactionStatus,
    pub retention_expires_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticReasonCodeDefinition {
    pub reason_code: DiagnosticReasonCode,
    pub category: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub default_severity: String,
    pub default_retry_safety: RetrySafety,
    pub default_remediation_owner: RemediationOwner,
    pub user_message_key: String,
    pub operator_message_key: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported_domains: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderErrorClassification {
    pub classification_id: String,
    pub tenant_id: String,
    pub provider_kind: String,
    pub domain_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub integration_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub operation_class: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider_error_class: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider_status_code: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub redacted_provider_code: String,
    pub reason_code: DiagnosticReasonCode,
    pub retry_safety: RetrySafety,
    pub remediation_owner: RemediationOwner,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub evidence_confidence: String,
    pub ambiguous: bool,
    pub redaction_status: RedactionStatus,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticFailureProjection {
    pub reason_code: DiagnosticReasonCode,
    pub remediation_owner: RemediationOwner,
    pub remediation_hint: String,
    pub retry_safety: RetrySafety,
    pub freshness_state: FreshnessState,
    pub checked_at: DateTime<Utc>,
    pub redaction_status: RedactionStatus,
}

#[derive(Debug, Clone, Default)]
pub struct DiagnosticResultFilter {
    pub tenant_id: String,
    pub integration_id: String,
    pub provider_kind: String,
    pub domain_kind: String,
    pub status: DiagnosticStatus,
    pub reason_code: DiagnosticReasonCode,
    pub cursor: String,
    pub limit: i64,
    pub include_expired: bool,
}

#[derive(Debug, Clone, Default)]
pub struct DiagnosticRunFilter {
    pub tenant_id: String,
    pub integration_id: String,
    pub provider_kind: String,
    pub domain_kind: String,
    pub status: DiagnosticRunStatus,
    pub reason_code: DiagnosticReasonCode,
    pub cursor: String,
    pub limit: i64,
    pub include_expired: bool,
}

#[derive(Debug, Clone, Default)]
pub struct DiagnosticInspectionInput {
    pub resource: crate::Resource,
    pub capability: String,
    pub run_id: String,
    pub checked_at: DateTime<Utc>,
    pub evidence_text: String,
    pub force_generic: bool,
}

#[derive(Debug, Clone, Default)]
pub struct DiagnosticRunInput {
    pub resource: crate::Resource,
    pub requested_by: String,
    pub client_key: String,
    pub capabilities: Vec<String>,
    pub trigger: String,
    pub started_at: DateTime<Utc>,
}

#[must_use]
pub fn diagnostic_freshness(now: DateTime<Utc>, stale_after: DateTime<Utc>) -> FreshnessState {
    if stale_after == DateTime::<Utc>::default() || now > stale_after {
        FreshnessState::Stale
    } else {
        FreshnessState::Fresh
    }
}

#[must_use]
pub fn diagnostic_retention_expiry(now: DateTime<Utc>) -> DateTime<Utc> {
    let now = if now == DateTime::<Utc>::default() {
        Utc::now()
    } else {
        now
    };
    now + DIAGNOSTIC_DEFAULT_RETENTION
}

#[must_use]
pub fn default_diagnostic_reason_code_catalog() -> Vec<DiagnosticReasonCodeDefinition> {
    let defs: &[(DiagnosticReasonCode, &str, RetrySafety, RemediationOwner)] = &[
        (
            DiagnosticReasonCode::Healthy,
            "healthy",
            RetrySafety::NoActionNeeded,
            RemediationOwner::NoneRequired,
        ),
        (
            DiagnosticReasonCode::AppAuthorizationMissing,
            "authorization",
            RetrySafety::Blocked,
            RemediationOwner::Operator,
        ),
        (
            DiagnosticReasonCode::BotAuthorizationMissing,
            "authorization",
            RetrySafety::Blocked,
            RemediationOwner::Operator,
        ),
        (
            DiagnosticReasonCode::UserAuthorizationMissing,
            "authorization",
            RetrySafety::Blocked,
            RemediationOwner::ProductUser,
        ),
        (
            DiagnosticReasonCode::TenantApprovalPending,
            "tenant_approval",
            RetrySafety::Blocked,
            RemediationOwner::TenantAdmin,
        ),
        (
            DiagnosticReasonCode::ScopeMissing,
            "scope",
            RetrySafety::Blocked,
            RemediationOwner::TenantAdmin,
        ),
        (
            DiagnosticReasonCode::TokenMissing,
            "token",
            RetrySafety::Blocked,
            RemediationOwner::ProductUser,
        ),
        (
            DiagnosticReasonCode::TokenExpired,
            "token",
            RetrySafety::Blocked,
            RemediationOwner::ProductUser,
        ),
        (
            DiagnosticReasonCode::TokenRevoked,
            "token",
            RetrySafety::Blocked,
            RemediationOwner::ProductUser,
        ),
        (
            DiagnosticReasonCode::RefreshCredentialsMissing,
            "token",
            RetrySafety::Blocked,
            RemediationOwner::Operator,
        ),
        (
            DiagnosticReasonCode::TokenRefreshFailed,
            "token",
            RetrySafety::Blocked,
            RemediationOwner::Operator,
        ),
        (
            DiagnosticReasonCode::TenantMismatch,
            "tenant_mismatch",
            RetrySafety::Blocked,
            RemediationOwner::Operator,
        ),
        (
            DiagnosticReasonCode::RateLimited,
            "quota",
            RetrySafety::Retryable,
            RemediationOwner::Provider,
        ),
        (
            DiagnosticReasonCode::ProviderUnavailable,
            "provider",
            RetrySafety::Retryable,
            RemediationOwner::Provider,
        ),
        (
            DiagnosticReasonCode::TransientProviderFailure,
            "provider",
            RetrySafety::Retryable,
            RemediationOwner::Provider,
        ),
        (
            DiagnosticReasonCode::NetworkFailed,
            "network",
            RetrySafety::Retryable,
            RemediationOwner::Operator,
        ),
        (
            DiagnosticReasonCode::AmbiguousDownstreamCommit,
            "retry_safety",
            RetrySafety::UnsafeToRetry,
            RemediationOwner::Operator,
        ),
        (
            DiagnosticReasonCode::UnsafeToRetry,
            "retry_safety",
            RetrySafety::UnsafeToRetry,
            RemediationOwner::Operator,
        ),
        (
            DiagnosticReasonCode::OperatorActionNeeded,
            "retry_safety",
            RetrySafety::OperatorActionNeeded,
            RemediationOwner::Operator,
        ),
        (
            DiagnosticReasonCode::LimitedDiagnostic,
            "unsupported",
            RetrySafety::NoActionNeeded,
            RemediationOwner::Operator,
        ),
        (
            DiagnosticReasonCode::UnsupportedDiagnostic,
            "unsupported",
            RetrySafety::NoActionNeeded,
            RemediationOwner::Operator,
        ),
        (
            DiagnosticReasonCode::RedactionFailedClosed,
            "redaction",
            RetrySafety::Blocked,
            RemediationOwner::Operator,
        ),
        (
            DiagnosticReasonCode::UnknownProviderError,
            "unknown",
            RetrySafety::OperatorActionNeeded,
            RemediationOwner::Operator,
        ),
    ];
    defs.iter()
        .map(
            |(reason, category, retry, owner)| DiagnosticReasonCodeDefinition {
                reason_code: *reason,
                category: (*category).to_string(),
                default_retry_safety: *retry,
                default_remediation_owner: *owner,
                user_message_key: format!("integration.diagnostic.{}", reason.as_str()),
                operator_message_key: format!("integration.diagnostic.{}", reason.as_str()),
                ..DiagnosticReasonCodeDefinition::default()
            },
        )
        .collect()
}
