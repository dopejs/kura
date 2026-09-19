//! Domain types (port of `types.go`): setup target/session/attempt/diagnostic/audit
//! shapes, string enums, reason codes, and sentinel errors.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

macro_rules! string_enum {
    ($name:ident { $($v:ident => $s:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum $name { $(#[serde(rename = $s)] $v),+ }

        impl $name {
            #[must_use]
            pub fn as_str(self) -> &'static str {
                match self { $( $name::$v => $s ),+ }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

// ---------------------------------------------------------------------------
// Target identifiers
// ---------------------------------------------------------------------------

pub const TARGET_OPENAI_COMPATIBLE: &str = "provider.openai_compatible";
pub const TARGET_FEISHU_LARK: &str = "integration.feishu_lark";
pub const TARGET_DISCORD_CONNECTOR: &str = "connector.discord";
pub const TARGET_TELEGRAM_CONNECTOR: &str = "connector.telegram";
pub const TARGET_SLACK_CONNECTOR: &str = "connector.slack";
pub const TARGET_MATRIX_CONNECTOR: &str = "connector.matrix";

// ---------------------------------------------------------------------------
// String enums
// ---------------------------------------------------------------------------

string_enum!(TargetKind {
    Provider => "provider",
    Integration => "integration",
    Channel => "channel",
    Connector => "connector",
});

string_enum!(SetupStyle {
    SubmittedSecret => "submitted_secret",
    OAuth => "oauth",
    Unsupported => "unsupported",
});

string_enum!(SupportStatus {
    Supported => "supported",
    Unsupported => "unsupported",
    ActionRequired => "action_required",
});

string_enum!(SetupState {
    NotStarted => "not_started",
    InProgress => "in_progress",
    Ready => "ready",
    Degraded => "degraded",
    Unavailable => "unavailable",
    Cancelled => "cancelled",
    ActionRequired => "action_required",
    Disabled => "disabled",
});

string_enum!(SafeUseMode {
    Normal => "normal",
    LimitedSafe => "limited_safe",
    Blocked => "blocked",
});

string_enum!(RemediationOwner {
    ProductUser => "product_user",
    TenantAdmin => "tenant_admin",
    Operator => "operator",
    Provider => "provider",
    NoneRequired => "none_required",
});

string_enum!(RedactionStatus {
    Redacted => "redacted",
    Suppressed => "suppressed",
    FailedClosed => "failed_closed",
});

string_enum!(RetrySafety {
    NoActionNeeded => "no_action_needed",
    Retryable => "retryable",
    Blocked => "blocked",
    UnsafeToRetry => "unsafe_to_retry",
});

string_enum!(OAuthResult {
    Completed => "completed",
    Denied => "denied",
    Abandoned => "abandoned",
    Expired => "expired",
    Replay => "replay",
    TenantMismatch => "tenant_mismatch",
    ProviderError => "provider_error",
});

string_enum!(SetupOperation {
    Start => "start",
    SubmitSecret => "submit_secret",
    OAuthStart => "oauth_start",
    OAuthCallback => "oauth_callback",
    DiagnosticProbe => "diagnostic_probe",
    Retry => "retry",
    Replace => "replace",
    Cancel => "cancel",
    Disable => "disable",
});

// ---------------------------------------------------------------------------
// Reason codes
// ---------------------------------------------------------------------------

pub const REASON_HEALTHY: &str = "healthy";
pub const REASON_CREDENTIAL_MISSING: &str = "credential_missing";
pub const REASON_SCOPE_MISSING: &str = "scope_missing";
pub const REASON_TENANT_APPROVAL_PENDING: &str = "tenant_approval_pending";
pub const REASON_TOKEN_MISSING: &str = "token_missing";
pub const REASON_TOKEN_EXPIRED: &str = "token_expired";
pub const REASON_TOKEN_REVOKED: &str = "token_revoked";
pub const REASON_OAUTH_DENIED: &str = "oauth_denied";
pub const REASON_OAUTH_ABANDONED: &str = "oauth_abandoned";
pub const REASON_OAUTH_EXPIRED: &str = "oauth_expired";
pub const REASON_OAUTH_REPLAY: &str = "oauth_replay";
pub const REASON_TENANT_MISMATCH: &str = "tenant_mismatch";
pub const REASON_PROVIDER_UNAVAILABLE: &str = "provider_unavailable";
pub const REASON_NETWORK_FAILED: &str = "network_failed";
pub const REASON_RATE_LIMITED: &str = "rate_limited";
pub const REASON_UNSUPPORTED_TARGET: &str = "unsupported_target";
pub const REASON_REDACTION_FAILED_CLOSED: &str = "redaction_failed_closed";
pub const REASON_USER_CANCELLED: &str = "user_cancelled";
pub const REASON_DISABLED_BY_USER: &str = "disabled_by_user";
pub const REASON_SETUP_PERSISTENCE_FAILURE: &str = "setup_failed:persistence";
pub const REASON_DISCORD_DESTINATION_MISSING: &str = "discord_destination_missing";
pub const REASON_DISCORD_DESTINATION_INVALID: &str = "discord_destination_invalid";
pub const REASON_TELEGRAM_ALLOWMENT_MISSING: &str = "telegram_allowment_missing";
pub const REASON_TELEGRAM_ALLOWMENT_INVALID: &str = "telegram_allowment_invalid";
pub const REASON_SLACK_ROUTE_POLICY_MISSING: &str = "slack_route_policy_missing";
pub const REASON_SLACK_ROUTE_POLICY_INVALID: &str = "slack_route_policy_invalid";
pub const REASON_MATRIX_ROUTE_POLICY_MISSING: &str = "matrix_route_policy_missing";
pub const REASON_MATRIX_ROUTE_POLICY_INVALID: &str = "matrix_route_policy_invalid";
pub const REASON_MATRIX_OWNERSHIP_MISMATCH: &str = "matrix_ownership_mismatch";

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Setup wizard failures (Go sentinel errors + wrapped secrets/store errors).
#[derive(Debug, Error)]
pub enum SetupError {
    #[error("tenant context is required")]
    TenantRequired,
    #[error("setup permission denied")]
    PermissionDenied,
    #[error("setup target is required")]
    TargetRequired,
    #[error("setup target is unsupported")]
    UnsupportedTarget,
    #[error("setup session id is required")]
    SessionRequired,
    #[error("setup session not found")]
    SessionNotFound,
    #[error("secret ref is required")]
    SecretRefRequired,
    #[error("secret value is required")]
    SecretValueRequired,
    #[error("oauth state is required")]
    OAuthStateRequired,
    #[error("oauth state does not match setup session")]
    OAuthStateMismatch,
    #[error("setup evidence contains forbidden credential material")]
    UnsafeEvidence,
    #[error("ready or degraded setup requires diagnostic linkage")]
    DiagnosticLinkNeeded,
    #[error("setup style {0} does not match target {1}")]
    StyleMismatch(String, String),
    #[error(transparent)]
    Secrets(#[from] kura_secrets::SecretsError),
    #[error("setup store error: {0}")]
    Store(String),
}

// ---------------------------------------------------------------------------
// Domain structs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupTarget {
    pub target_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub target_kind: TargetKind,
    pub setup_style: SetupStyle,
    pub display_name: String,
    pub proof_target: bool,
    pub support_status: SupportStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_permissions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limited_safe_capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub current_session_id: String,
    #[serde(default)]
    pub current_state: SetupState,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub diagnostic_result_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupSession {
    pub setup_session_id: String,
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub actor_principal_id: String,
    pub target_id: String,
    pub target_kind: TargetKind,
    pub setup_style: SetupStyle,
    pub state: SetupState,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
    pub retryable: bool,
    pub remediation_owner: RemediationOwner,
    pub safe_use_mode: SafeUseMode,
    pub allowed_capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub current_attempt_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub diagnostic_result_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub diagnostic_run_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub diagnostic_stage: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub diagnostic_source_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub diagnostic_source_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostic_allowed_use: Vec<String>,
    pub redaction_status: RedactionStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resource_refs: Vec<ResourceRef>,
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub redacted_evidence: std::collections::HashMap<String, String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub oauth_state_ref: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_transition_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_transition_audit_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub operator_remediation: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub user_remediation: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub unsupported_reason_code: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupAttempt {
    pub attempt_id: String,
    pub setup_session_id: String,
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub actor_principal_id: String,
    pub operation: SetupOperation,
    #[serde(default)]
    pub from_state: SetupState,
    pub to_state: SetupState,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub redacted_evidence: std::collections::HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resource_refs: Vec<ResourceRef>,
    pub redaction_status: RedactionStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub diagnostic_result_id: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceRef {
    pub kind: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub route: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupDiagnostic {
    pub setup_session_id: String,
    pub target_id: String,
    pub diagnostic_result_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub diagnostic_run_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub diagnostic_stage: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub diagnostic_source_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub diagnostic_source_id: String,
    pub status: SetupState,
    pub reason_code: String,
    pub retry_safety: RetrySafety,
    pub remediation_owner: RemediationOwner,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_capabilities: Vec<String>,
    pub checked_at: DateTime<Utc>,
    pub stale_after: DateTime<Utc>,
    pub redaction_status: RedactionStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticSource {
    pub kind: String,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupDiagnosticProbeResult {
    pub state: SetupState,
    pub reason_code: String,
    pub retry_safety: RetrySafety,
    pub remediation_owner: RemediationOwner,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_capabilities: Vec<String>,
    pub diagnostic_result_id: String,
    pub diagnostic_run_id: String,
    pub diagnostic_stage: String,
    pub diagnostic_source: DiagnosticSource,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupAuditRecord {
    pub event_kind: String,
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub principal_id: String,
    pub setup_session_id: String,
    pub target_id: String,
    pub target_kind: TargetKind,
    pub setup_style: SetupStyle,
    pub operation: SetupOperation,
    #[serde(default)]
    pub from_state: SetupState,
    pub to_state: SetupState,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
    pub retryable: bool,
    pub remediation_owner: RemediationOwner,
    pub safe_use_mode: SafeUseMode,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub diagnostic_result_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resource_refs: Vec<ResourceRef>,
    pub redaction_status: RedactionStatus,
    pub outcome: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DependentUseDecision {
    pub tenant_id: String,
    pub target_id: String,
    pub setup_state: SetupState,
    pub safe_use_mode: SafeUseMode,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
    pub checked_at: DateTime<Utc>,
}

impl Default for SetupState {
    fn default() -> Self {
        SetupState::NotStarted
    }
}
