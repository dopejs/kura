#![allow(unreachable_patterns)]
//! Port of daemon/internal/sandbox: the sandbox data model (profiles, policies,
//! execution requests/decisions/results, and consumer declarations) plus the
//! manager (profile/policy/execution lifecycle, persistence, and event
//! fan-out), execution (subprocess and docker process execution, cancellation,
//! capture buffers, backend metadata), and redaction (secret-value redaction
//! for result surfaces) layers. Ported from the Go types.go / manager.go.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub mod execution;
pub mod manager;
pub mod redaction;

pub use execution::*;
pub use manager::*;
pub use redaction::*;

/// Go marshals nil slices/maps as `null`; Go-era persisted documents carry it
/// where Rust expects a sequence/map. Deserialize null as the default.
pub fn null_default<'de, D, T>(deserializer: D) -> std::result::Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de> + Default,
{
    Ok(<Option<T> as serde::Deserialize>::deserialize(deserializer)?.unwrap_or_default())
}

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

string_enum!(Source {
    Builtin => "builtin",
});

string_enum!(BackendKind {
    Subprocess => "subprocess",
    Docker => "docker",
    Ssh => "ssh",
    Remote => "remote",
});

string_enum!(FilesystemMode {
    None => "none",
    Scoped => "scoped",
    Full => "full",
});

string_enum!(NetworkMode {
    Deny => "deny",
    AllowList => "allow_list",
    Full => "full",
});

string_enum!(EnvironmentMode {
    Clean => "clean",
    InheritSafe => "inherit_safe",
    InheritAll => "inherit_all",
});

string_enum!(ApprovalMode {
    Allow => "allow",
    Ask => "ask",
    Deny => "deny",
});

string_enum!(DecisionResolution {
    Allow => "allow",
    Ask => "ask",
    Deny => "deny",
});

string_enum!(DecisionApprovalStatus {
    NotApplicable => "not_applicable",
    Pending => "pending",
    Approved => "approved",
    Rejected => "rejected",
});

string_enum!(ExecutionStatus {
    Pending => "pending",
    Running => "running",
    Completed => "completed",
    Failed => "failed",
    Cancelled => "cancelled",
    Denied => "denied",
    Unsupported => "unsupported",
});

string_enum!(ErrorClass {
    None => "",
    PolicyDenied => "policy_denied",
    ApprovalRequired => "approval_required",
    ApprovalRejected => "approval_rejected",
    InvalidProfile => "invalid_profile",
    BackendMissing => "backend_unavailable",
    BackendMismatch => "backend_capability_mismatch",
    LaunchFailed => "launch_failed",
    ProcessFailed => "process_failed",
    ProviderFailed => "provider_error",
    ProviderAuth => "provider_auth_failed",
    Timeout => "timeout",
    Cancelled => "cancelled",
    IoCaptureFailed => "io_capture_failed",
});

string_enum!(ManagedProviderActionKind {
    AuthStatus => "auth_status",
    Logout => "logout",
    PromptExecution => "prompt_execution",
});

string_enum!(ManagedProviderOperationStatus {
    Pending => "pending",
    Denied => "denied",
    LocalStateInspection => "local_state_inspection",
    Running => "running",
    Completed => "completed",
    Failed => "failed",
    Cancelled => "cancelled",
});

string_enum!(LocalStateAccessMode {
    Read => "read",
    Write => "write",
});

string_enum!(ConsumerKind {
    ManagedProvider => "managed_provider",
    Skill => "skill",
    LocalTool => "local_tool",
    McpServer => "mcp_server",
});

string_enum!(ExecutionMode {
    Subprocess => "subprocess",
    AccessOnly => "access_only",
    DeclarationOnly => "declaration_only",
});

string_enum!(SecretDefaultSource {
    KindDefault => "kind_default",
    InstanceOverride => "instance_override",
});

string_enum!(SecretEnvironmentScope {
    Test => "test",
    Prod => "prod",
    Both => "both",
});

string_enum!(SecretResolution {
    Resolved => "resolved",
    Denied => "denied",
    Unavailable => "unavailable",
    NotApplicable => "not_applicable",
});

string_enum!(PolicyRecordStatus {
    PreflightAllowed => "preflight_allowed",
    ApprovalPending => "approval_pending",
    Running => "running",
    Completed => "completed",
    Failed => "failed",
    Cancelled => "cancelled",
    Denied => "denied",
    Unsupported => "unsupported",
});

string_enum!(BackendAvailabilityStatus {
    Available => "available",
    Unavailable => "unavailable",
    Degraded => "degraded",
});

string_enum!(BackendHostStatus {
    Ready => "ready",
    MissingPrerequisite => "missing_prerequisite",
    RuntimeUnavailable => "runtime_unavailable",
});

string_enum!(BackendSelectionOutcome {
    Selected => "selected",
    Unsupported => "unsupported",
    Denied => "denied",
});

pub const PROFILE_ID_SUBPROCESS_DEFAULT: &str = "subprocess_default";
/// A subprocess scoped to the project the daemon serves.
///
/// Exists only when `project_root` is configured. A tool server that reads the
/// project cannot run under the default profile, which scopes the filesystem
/// to the daemon's own data directory -- and the alternative to naming the
/// directory is telling the policy engine the process needs nothing while
/// handing it the path on its command line.
pub const PROFILE_ID_PROJECT_TOOLS: &str = "project_tools";
pub const PROFILE_ID_DOCKER_DEFAULT: &str = "docker_default";
pub const PROFILE_ID_MANAGED_PROVIDER_CLAUDE: &str = "managed_provider_claude";
pub const PROFILE_ID_MANAGED_PROVIDER_CODEX: &str = "managed_provider_codex";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendCapabilityProfile {
    pub backend_kind: BackendKind,
    pub display_name: String,
    pub filesystem_enforcement: String,
    pub network_enforcement: String,
    pub env_injection_mode: String,
    pub approval_behavior: String,
    pub restart_behavior: String,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub host_prerequisites: Vec<String>,
    pub availability_status: BackendAvailabilityStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub availability_reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsumerRequirementDeclaration {
    pub declaration_id: String,
    pub consumer_kind: ConsumerKind,
    pub consumer_id: String,
    pub operation_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub profile_id: String,
    pub execution_mode: ExecutionMode,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub allowed_backend_kinds: Vec<BackendKind>,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub read_roots: Vec<String>,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub write_roots: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_mode: Option<NetworkMode>,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub allowed_hosts: Vec<String>,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub allowed_ports: Vec<i64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub allow_loopback: bool,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub secret_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_mode: Option<ApprovalMode>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub required_enforcement_strength: String,
    pub active: bool,
    pub source: Source,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretScopeBinding {
    pub binding_id: String,
    pub consumer_kind: ConsumerKind,
    pub consumer_id: String,
    pub default_source: SecretDefaultSource,
    pub environment_scope: SecretEnvironmentScope,
    pub secret_ref: String,
    pub delivery_kind: String,
    pub redaction_rule: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub default_rule_id: String,
    pub active: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretScopeOutcome {
    pub consumer_kind: ConsumerKind,
    pub consumer_id: String,
    pub secret_ref: String,
    pub environment_scope: SecretEnvironmentScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_source: Option<SecretDefaultSource>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub default_rule_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub delivery_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub redaction_rule: String,
    pub resolution: SecretResolution,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsumerPolicyRecord {
    pub policy_record_id: String,
    pub consumer_kind: ConsumerKind,
    pub consumer_id: String,
    pub operation_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub declaration_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub requested_by: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub approval_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub decision_id: String,
    pub decision: DecisionResolution,
    pub approval_status: DecisionApprovalStatus,
    pub secret_resolution: SecretResolution,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub enforcement_strength: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub failure_class: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sandbox_execution_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool_call_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider_operation_id: String,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    pub status: PolicyRecordStatus,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsumerContractView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declaration: Option<ConsumerRequirementDeclaration>,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub secret_scope: Vec<SecretScopeOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_record: Option<ConsumerPolicyRecord>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedProviderRequirementDeclaration {
    pub provider_id: String,
    pub action_kind: ManagedProviderActionKind,
    pub profile_id: String,
    pub backend_kind: BackendKind,
    #[serde(default, deserialize_with = "null_default")]
    pub read_roots: Vec<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub write_roots: Vec<String>,
    pub network_mode: NetworkMode,
    #[serde(default, deserialize_with = "null_default")]
    pub allowed_hosts: Vec<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub allowed_ports: Vec<i64>,
    pub approval_mode: ApprovalMode,
    #[serde(default, deserialize_with = "null_default")]
    pub sensitive_state_classes: Vec<String>,
    pub enforcement_strength: String,
    pub active: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SensitiveLocalStateAccessSummary {
    pub provider_id: String,
    pub action_kind: ManagedProviderActionKind,
    pub state_class: String,
    pub access_mode: LocalStateAccessMode,
    pub path_summary: String,
    pub declared: bool,
    pub sensitive: bool,
    pub redaction_rule: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedProviderOperation {
    pub operation_id: String,
    pub provider_id: String,
    pub action_kind: ManagedProviderActionKind,
    pub requested_by: String,
    pub requirement_profile_id: String,
    pub decision: DecisionResolution,
    pub approval_status: DecisionApprovalStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub failure_class: String,
    pub enforcement_strength: String,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub sensitive_state_classes: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub execution_id: String,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    pub status: ManagedProviderOperationStatus,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub local_state_access_summaries: Vec<SensitiveLocalStateAccessSummary>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilesystemPolicy {
    pub mode: FilesystemMode,
    #[serde(default, deserialize_with = "null_default")]
    pub read_roots: Vec<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub write_roots: Vec<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub temp_roots: Vec<String>,
    pub allow_data_dir: bool,
    pub allow_user_agents_dir: bool,
    pub allow_home_read: bool,
    pub allow_home_write: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkPolicy {
    pub mode: NetworkMode,
    #[serde(default, deserialize_with = "null_default")]
    pub allowed_hosts: Vec<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub allowed_ports: Vec<i64>,
    pub allow_loopback: bool,
    pub enforcement_mode: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentPolicy {
    pub mode: EnvironmentMode,
    #[serde(default, deserialize_with = "null_default")]
    pub allowed_vars: Vec<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub injected_vars: HashMap<String, String>,
    #[serde(default, deserialize_with = "null_default")]
    pub redacted_vars: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalPolicy {
    pub mode: ApprovalMode,
    #[serde(default, deserialize_with = "null_default")]
    pub required_for_commands: Vec<String>,
    pub required_for_writes_outside_roots: bool,
    pub required_for_network: bool,
    pub required_for_unknown_backends: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessPolicy {
    pub timeout_ms: i64,
    pub max_timeout_ms: i64,
    pub kill_grace_ms: i64,
    pub capture_stdout: bool,
    pub capture_stderr: bool,
    pub max_output_bytes: i64,
    pub allow_streaming: bool,
    pub restart_on_failure: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub profile_id: String,
    pub title: String,
    pub description: String,
    pub backend_kind: BackendKind,
    pub backend_capability: BackendCapabilityProfile,
    pub default_work_dir: String,
    pub filesystem_policy: FilesystemPolicy,
    pub network_policy: NetworkPolicy,
    pub env_policy: EnvironmentPolicy,
    pub approval_policy: ApprovalPolicy,
    pub process_policy: ProcessPolicy,
    pub default_timeout_ms: i64,
    pub max_timeout_ms: i64,
    pub restartable: bool,
    pub source: Source,
    pub active: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessRequest {
    #[serde(default, deserialize_with = "null_default")]
    pub read_roots: Vec<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub write_roots: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_mode: Option<NetworkMode>,
    #[serde(default, deserialize_with = "null_default")]
    pub allowed_hosts: Vec<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub allowed_ports: Vec<i64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub allow_loopback: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub profile_id: String,
    pub command: String,
    #[serde(default, deserialize_with = "null_default")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cwd: String,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stdin: String,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub timeout_ms: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub requested_by: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub resource_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub resource_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub scope: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub approval_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
    pub access: AccessRequest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumer: Option<ConsumerContractView>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Decision {
    pub decision_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub execution_id: String,
    pub resolution: DecisionResolution,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection_outcome: Option<BackendSelectionOutcome>,
    #[serde(default, deserialize_with = "null_default")]
    pub matched_rules: Vec<String>,
    pub approval_required: bool,
    pub approval_status: DecisionApprovalStatus,
    pub effective_profile_id: String,
    pub effective_backend_kind: BackendKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_backend_kind: Option<BackendKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_status: Option<BackendHostStatus>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mismatch_reason: String,
    pub explanation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumer: Option<ConsumerContractView>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Result {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub execution_id: String,
    pub status: ExecutionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub signal: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stdout: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stderr: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub output_truncated: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error_class: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error_code: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub partial: bool,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "serde_json::Map::is_empty")]
    pub backend_metadata: serde_json::Map<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumer: Option<ConsumerContractView>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionFinalization {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ExecutionStatus>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error_class: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error_code: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Execution {
    pub execution_id: String,
    pub profile_id: String,
    pub backend_kind: BackendKind,
    pub command: String,
    #[serde(default, deserialize_with = "null_default")]
    pub args: Vec<String>,
    pub cwd: String,
    #[serde(default, deserialize_with = "null_default")]
    pub env_keys: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stdin_provided: bool,
    pub timeout_ms: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub requested_by: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub resource_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub resource_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub scope: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub approval_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
    pub access: AccessRequest,
    pub status: ExecutionStatus,
    pub decision: Decision,
    pub result: Result,
    pub requested_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumer: Option<ConsumerContractView>,
}

#[must_use]
pub fn is_terminal(status: ExecutionStatus) -> bool {
    matches!(
        status,
        ExecutionStatus::Completed
            | ExecutionStatus::Failed
            | ExecutionStatus::Cancelled
            | ExecutionStatus::Denied
            | ExecutionStatus::Unsupported
    )
}

#[must_use]
fn is_zero_i64(v: &i64) -> bool {
    *v == 0
}
