//! Port of the daemon/internal/connectors domain types: the connector supervisor
//! model (`Connector`), conformance results (`ConformanceResult`), and per-connector
//! diagnostic state (`ConnectorDiagnosticState`) together with their string enums.
//! Wire values (camelCase field names, snake_case enum literals) match the Go json
//! tags exactly. The supervisor manager (crate::Supervisor) and the conformance
//! helpers (crate::run_matrix_case, crate::CapabilityProfile, ...) live in the
//! supervisor and conformance submodules.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// String enum with explicit per-variant wire literals: every variant's serde
/// representation is exactly the literal, and `as_str`/`Display` agree with it.
macro_rules! string_enum {
    ($name:ident { $first:ident => $first_s:literal $(, $v:ident => $s:literal)* $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
        pub enum $name {
            #[default]
            #[serde(rename = $first_s)]
            $first,
            $(
                #[serde(rename = $s)]
                $v
            ),*
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

pub(crate) use string_enum;

// --- supervisor.go: Status + Connector ------------------------------------

string_enum!(Status {
    Registered => "registered",
    Healthy => "healthy",
    Degraded => "degraded",
    BackingOff => "backing_off",
    Failed => "failed",
    Disabled => "disabled",
});

/// Port of `secrets.RedactedSecretSummary` (wire type referenced by
/// `Connector.SecretSummary`); the secrets package itself is not a dependency
/// of this types-only crate, so the wire shape is declared locally.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactedSecretSummary {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub secret_ref: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub secret_version_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<ResolutionStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<SecretStatus>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub disabled_reason: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub redaction_rule: String,
}

string_enum!(SecretStatus {
    Active => "active",
    Disabled => "disabled",
    PendingRemediation => "pending_remediation",
});

string_enum!(ResolutionStatus {
    Resolved => "resolved",
    Unavailable => "unavailable",
    Denied => "denied",
    NotApplicable => "not_applicable",
});

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connector {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub connector_id: String,
    pub kind: String,
    pub display_name: String,
    pub status: Status,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub disabled_reason: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secret_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secret_summary: Vec<RedactedSecretSummary>,
    pub failure_count: i64,
    pub restart_count: i64,
    pub backoff_seconds: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_restart_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_restart_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_heartbeat_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_failure_reason: String,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub capability_profile: serde_json::Map<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub diagnostic_state: serde_json::Map<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub conformance_result: serde_json::Map<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub account_binding: serde_json::Map<String, serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// --- conformance.go: LifecycleState, ConformanceResultStatus, RedactionStatus,
//     ConformanceResult -----------------------------------------------------

string_enum!(LifecycleState {
    Configured => "configured",
    Disabled => "disabled",
    Starting => "starting",
    Healthy => "healthy",
    Degraded => "degraded",
    Failed => "failed",
    PermissionBlocked => "permission_blocked",
    RateLimited => "rate_limited",
    UnsupportedCapability => "unsupported_capability",
});

string_enum!(ConformanceResultStatus {
    Pass => "pass",
    Fail => "fail",
    Supported => "supported",
    Limited => "limited",
    Unsupported => "unsupported",
});

string_enum!(RedactionStatus {
    Redacted => "redacted",
    Suppressed => "suppressed",
    Failed => "redaction_failed",
});

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConformanceResult {
    pub conformance_result_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub connector_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_id: String,
    pub scenario_id: String,
    pub area: String,
    pub result: ConformanceResultStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
    pub redaction_status: RedactionStatus,
    pub evidence_timestamp: DateTime<Utc>,
    pub retention_expires_at: DateTime<Utc>,
}

// --- diagnostics.go: ConnectorDiagnosticState + its enums ------------------

string_enum!(DiagnosticReasonCode {
    AuthMissing => "auth_missing",
    PermissionMissing => "permission_missing",
    RateLimited => "rate_limited",
    ProviderUnavailable => "provider_unavailable",
    NetworkFailed => "network_failed",
    UnsupportedCapability => "unsupported_capability",
    BlockedRoute => "blocked_route",
    DuplicateInbound => "duplicate_inbound",
    ReplyFailed => "reply_failed",
    UnknownConnectorFailure => "unknown_connector_failure",
});

string_enum!(RemediationOwner {
    User => "product_user",
    Admin => "tenant_admin",
    Operator => "operator",
    Provider => "provider",
    NoneRequired => "none_required",
});

string_enum!(RetrySafety {
    NoActionNeeded => "no_action_needed",
    Retryable => "retryable",
    RetryAfter => "retry_after",
    Blocked => "blocked",
    Unsafe => "unsafe",
});

string_enum!(FreshnessState {
    Fresh => "fresh",
    Stale => "stale",
});

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorDiagnosticState {
    pub diagnostic_state_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub connector_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_account_id: String,
    pub status: LifecycleState,
    pub reason_code: DiagnosticReasonCode,
    pub remediation_owner: RemediationOwner,
    pub user_visible_severity: String,
    pub retry_safety: RetrySafety,
    pub evidence_timestamp: DateTime<Utc>,
    pub freshness_state: FreshnessState,
    pub redaction_status: RedactionStatus,
    pub retention_expires_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub safe_evidence: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub redaction_failure_id: String,
}

// --- supervisor.go manager + conformance.go helpers ------------------------

mod conformance;
mod diagnostics;
mod management;
mod supervisor;

pub use conformance::*;
pub use diagnostics::*;
pub use management::*;
pub use supervisor::*;
