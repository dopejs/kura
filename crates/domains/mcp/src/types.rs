//! Port of `daemon/internal/mcp/types.go`: the MCP server / server-state / tool /
//! tool-exposure-rule / catalog data model and their input types. Wire values
//! (camelCase field names, explicit enum literals) match the Go json tags exactly.
//! Enums are closed string enums following the workspace convention; the Go zero
//! value "" is represented by the `#[default]` variant (noted per field where it
//! matters).

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{clone_backend_kinds, clone_strings, clean_strings, string_enum};

/// Go marshals nil slices/maps as `null`; Go-era persisted documents carry it
/// where Rust expects a sequence/map. Deserialize null as the default.
pub fn null_default<'de, D, T>(deserializer: D) -> std::result::Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de> + Default,
{
    Ok(<Option<T> as serde::Deserialize>::deserialize(deserializer)?.unwrap_or_default())
}


string_enum!(Source {
    Api => "api",
    Config => "config",
    Builtin => "builtin",
});

string_enum!(TransportKind {
    Stdio => "stdio",
    StreamableHTTP => "streamable-http",
    Websocket => "websocket",
});

string_enum!(OriginKind {
    Manual => "manual",
    Catalog => "catalog",
});

string_enum!(InstallMethod {
    Api => "api",
    Script => "script",
});

string_enum!(AvailabilityStatus {
    Ready => "ready",
    Blocked => "blocked",
    Unavailable => "unavailable",
    Unsupported => "unsupported",
});

string_enum!(LifecycleStatus {
    Disabled => "disabled",
    Stopped => "stopped",
    Starting => "starting",
    Healthy => "healthy",
    Degraded => "degraded",
    BackingOff => "backing_off",
    Failed => "failed",
    Stopping => "stopping",
    Denied => "denied",
    Unsupported => "unsupported",
});

string_enum!(DiscoveryStatus {
    Discovered => "discovered",
    Stale => "stale",
    Unavailable => "unavailable",
});

string_enum!(ExposureMode {
    Blocked => "blocked",
    Allow => "allow",
    ApprovalRequired => "approval_required",
});

string_enum!(TransportHealthStatus {
    Healthy => "healthy",
    Degraded => "degraded",
});

string_enum!(WebsocketAuthMode {
    BearerHeader => "bearer_header",
    Header => "header",
});

string_enum!(ToolAuthorizationStatus {
    Allowed => "allowed",
    Pending => "pending",
    Rejected => "rejected",
    Blocked => "blocked",
});

string_enum!(LifecycleAction {
    Start => "start",
    Stop => "stop",
    Restart => "restart",
    Cancel => "cancel",
});

string_enum!(CatalogDriftStatus {
    InSync => "in_sync",
    CatalogUpdated => "catalog_updated",
    LocallyModified => "locally_modified",
    MissingEntry => "missing_entry",
    Conflicting => "conflicting",
});

string_enum!(CatalogAction {
    Install => "install",
    Refresh => "refresh",
    Reinstall => "reinstall",
    Uninstall => "uninstall",
    Revalidate => "revalidate",
});

string_enum!(CatalogActionStatus {
    Completed => "completed",
    Blocked => "blocked",
    Failed => "failed",
});

string_enum!(RevalidationClassification {
    Healthy => "healthy",
    PrerequisiteLost => "prerequisite_lost",
    CatalogDrift => "catalog_drift",
    LocallyModified => "locally_modified",
    RuntimeUnhealthy => "runtime_unhealthy",
    MissingEntry => "missing_entry",
});

string_enum!(RevalidationIssueStatus {
    Blocked => "blocked",
    Unavailable => "unavailable",
    Unsupported => "unsupported",
    Warning => "warning",
});

/// A declared MCP server (Go Server). `resolved_websocket_headers` is runtime-only
/// and never serialized (`json:"-"`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]

pub struct Server {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    #[serde(default)]
    pub server_id: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub source: Source,
    #[serde(default)]
    pub origin_kind: OriginKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub catalog_entry_id: String,
    #[serde(default)]
    pub install_method: InstallMethod,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub environment_scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_management: Option<CatalogManagement>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub sandbox_profile_id: String,
    #[serde(default)]
    pub declaration_id: String,
    #[serde(default)]
    pub declaration: Declaration,
    #[serde(default)]
    pub transport_kind: TransportKind,
    #[serde(default)]
    pub command: String,
    #[serde(default, deserialize_with = "null_default")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub endpoint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub websocket_config: Option<WebsocketConfig>,
    #[serde(skip)]
    #[serde(default, deserialize_with = "null_default")]
    pub resolved_websocket_headers: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub working_dir: String,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub secret_refs: Vec<String>,
    pub auto_restart: bool,
    #[serde(default, skip_serializing_if = "crate::is_false")]
    pub operator_modified: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Per-server runtime lifecycle state (Go ServerState).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerState {
    #[serde(default)]
    pub server_id: String,
    #[serde(default)]
    pub status: LifecycleStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub health_reason: String,
    #[serde(default)]
    pub failure_count: i64,
    #[serde(default)]
    pub restart_count: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_recovery_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_recovery_class: String,
    #[serde(default, skip_serializing_if = "crate::is_zero_i64")]
    pub reconnect_attempt_count: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_started_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_stopped_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_heartbeat_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_restart_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_reconnect_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_execution_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_policy_record_id: String,
    pub updated_at: DateTime<Utc>,
}

/// Transport capability projection used by `ListTransportCapabilities`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransportCapability {
    pub transport_kind: TransportKind,
    pub availability_status: AvailabilityStatus,
    pub health_status: TransportHealthStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub prerequisites: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub environment_scope: String,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub supported_auth_kinds: Vec<String>,
    pub daemon_managed_reconnect: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub recovery_summary: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebsocketAuthConfig {
    pub mode: WebsocketAuthMode,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub header_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub scheme: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub secret_ref: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebsocketConfig {
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub subprotocols: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<WebsocketAuthConfig>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebsocketAuthSummary {
    #[serde(default)]
    pub mode: WebsocketAuthMode,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub header_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub scheme: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub secret_ref: String,
    pub configured: bool,
    pub resolved: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub blocked_reason: String,
}

/// A discovered MCP tool (Go Tool).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    #[serde(default)]
    pub server_id: String,
    #[serde(default)]
    pub tool_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub schema_fingerprint: String,
    /// The tool's declared argument schema, as the server published it.
    ///
    /// Kept alongside the fingerprint rather than replaced by it. A hash
    /// answers "did this change"; it cannot answer "what does this take",
    /// which is the only thing a model needs in order to call the tool. Empty
    /// for rows discovered before this was retained, and for a server that
    /// publishes no schema.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub input_schema: serde_json::Value,
    pub discovery_status: DiscoveryStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_discovered_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

/// A per (server, tool, runtime surface) exposure rule (Go ToolExposureRule).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolExposureRule {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    #[serde(default)]
    pub server_id: String,
    #[serde(default)]
    pub tool_name: String,
    #[serde(default)]
    pub runtime_surface: String,
    #[serde(default)]
    pub exposure_mode: ExposureMode,
    #[serde(default)]
    pub active: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretSummary {
    pub consumer_id: String,
    pub secret_ref: String,
    pub environment_scope: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub default_rule_id: String,
    pub resolution: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub delivery_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub redaction_rule: String,
}

/// The sandbox declaration for an MCP server (Go Declaration). Enums come from
/// kura-sandbox; serde defaults fill the Go normalizeDeclaration defaults when a wire
/// document omits fields.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Declaration {
    pub execution_mode: kura_sandbox::ExecutionMode,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub allowed_backend_kinds: Vec<kura_sandbox::BackendKind>,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub read_roots: Vec<String>,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub write_roots: Vec<String>,
    #[serde(default)]
    pub network_mode: kura_sandbox::NetworkMode,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub allowed_hosts: Vec<String>,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub allowed_ports: Vec<i64>,
    #[serde(default, skip_serializing_if = "crate::is_false")]
    pub allow_loopback: bool,
    #[serde(default)]
    pub approval_mode: kura_sandbox::ApprovalMode,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub required_enforcement_strength: String,
    #[serde(default)]
    pub active: bool,
}

/// Server + state + tools projection returned by list/get endpoints (Go
/// ServerResource; the embedded Server is flattened on the wire).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerResource {
    #[serde(flatten)]
    pub server: Server,
    pub state: ServerState,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub secret_summary: Vec<SecretSummary>,
    pub tool_count: i64,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolResource>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub transport_config_summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub websocket_auth_summary: Option<WebsocketAuthSummary>,
    #[serde(default)]
    pub availability_status: AvailabilityStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub availability_reason: String,
}

/// Tool + exposure rules projection (Go ToolResource; the embedded Tool is flattened).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResource {
    #[serde(flatten)]
    pub tool: Tool,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub exposure: Vec<ToolExposureRule>,
    pub effective_availability: String,
    pub approval_required: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub unavailable_reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateServerInput {
    /// Whose server this is. Filled by the route from the request's context,
    /// not by the caller: every read filters by it, so a server stored under
    /// the wrong tenant is a server nobody can see.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub server_id: String,
    pub display_name: String,
    #[serde(default)]
    pub origin_kind: OriginKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub catalog_entry_id: String,
    #[serde(default)]
    pub install_method: InstallMethod,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub environment_scope: String,
    pub enabled: bool,
    pub sandbox_profile_id: String,
    pub declaration_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declaration: Option<Declaration>,
    pub transport_kind: TransportKind,
    pub command: String,
    #[serde(default, deserialize_with = "null_default")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub endpoint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub websocket_config: Option<WebsocketConfig>,
    pub working_dir: String,
    #[serde(default, deserialize_with = "null_default")]
    pub secret_refs: Vec<String>,
    pub auto_restart: bool,
    #[serde(default, skip_serializing_if = "crate::is_false")]
    pub operator_modified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_management: Option<CatalogManagement>,
}

/// Partial server update; every field is optional (Go pointer fields).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateServerInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declaration_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declaration: Option<Declaration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport_kind: Option<TransportKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub websocket_config: Option<WebsocketConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_refs: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_restart: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateExposureInput {
    pub runtime_surface: String,
    pub exposure_mode: ExposureMode,
    pub active: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleResponse {
    pub action: LifecycleAction,
    pub server: ServerResource,
    pub idempotent: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub execution_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub failure_class: String,
    pub blocked: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub blocked_reason: String,
    pub preflight_ms: i64,
}

/// Catalog install snapshot captured at install time (Go CatalogInstallSnapshot).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogInstallSnapshot {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub server_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sandbox_profile_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub command: String,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub endpoint: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub working_dir: String,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub secret_refs: Vec<String>,
    #[serde(default)]
    pub install_method: InstallMethod,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevalidationIssue {
    pub kind: String,
    pub name: String,
    pub status: RevalidationIssueStatus,
    pub reason: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub environment_scope: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevalidationSnapshot {
    pub checked_at: DateTime<Utc>,
    pub status: AvailabilityStatus,
    pub classification: RevalidationClassification,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<RevalidationIssue>,
}

/// Catalog-managed install provenance and drift tracking (Go CatalogManagement).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogManagement {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub installed_revision: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub current_revision: String,
    #[serde(default)]
    pub drift_status: CatalogDriftStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub drift_reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_maintained_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_action_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_action: Option<CatalogAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_action_status: Option<CatalogActionStatus>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_action_failure_class: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_action_reason: String,
    #[serde(default, skip_serializing_if = "crate::types::is_default_catalog_install_snapshot")]
    pub install_input_snapshot: CatalogInstallSnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_revalidation: Option<RevalidationSnapshot>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogLifecycleResult {
    pub action_id: String,
    pub action: CatalogAction,
    pub status: CatalogActionStatus,
    pub server_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub catalog_entry_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub failure_class: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub audit_event_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "crate::is_false")]
    pub removed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<ServerResource>,
    #[serde(default, skip_serializing_if = "crate::is_zero_i64")]
    pub preflight_ms: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogRevalidationResult {
    pub action_id: String,
    pub action: CatalogAction,
    pub server_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub catalog_entry_id: String,
    pub status: AvailabilityStatus,
    pub classification: RevalidationClassification,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<RevalidationIssue>,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub audit_event_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<ServerResource>,
    #[serde(default, skip_serializing_if = "crate::is_zero_i64")]
    pub preflight_ms: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorizeToolInput {
    pub runtime_surface: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub approval_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub requested_by: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolAuthorizationResponse {
    pub status: ToolAuthorizationStatus,
    pub tool: ToolResource,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<kura_policy::Approval>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<kura_policy::Decision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<kura_sandbox::ConsumerContractView>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogInstallSupport {
    pub script_supported: bool,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub script_args: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogPrerequisite {
    pub kind: String,
    pub name: String,
    pub required: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSecretRequirement {
    pub secret_ref: String,
    pub required: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEntry {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub transport_kind: TransportKind,
    pub source_kind: String,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    pub immediate_use: bool,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub prerequisites: Vec<CatalogPrerequisite>,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub secret_requirements: Vec<CatalogSecretRequirement>,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub environment_eligibility: Vec<String>,
    pub availability_status: AvailabilityStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub availability_reason: String,
    pub install_support: CatalogInstallSupport,
    pub default_install_spec: CreateServerInput,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogInstallInput {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub server_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sandbox_profile_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub command: String,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub endpoint: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub working_dir: String,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub secret_refs: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogInstallResult {
    pub install_id: String,
    pub status: String,
    pub catalog_entry_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub server_id: String,
    pub availability_status: AvailabilityStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub availability_reason: String,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub audit_event_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<ServerResource>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInvocationResult {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub failure_class: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
}

/// Go normalizeDeclaration: fills the empty defaults (subprocess execution, subprocess
/// backend, network deny, approval allow, declared_only enforcement). Enum defaults are
/// already applied by serde `#[serde(default)]`; this fills the slice/string fields.
#[must_use]
pub fn normalize_declaration(mut declaration: Declaration) -> Declaration {
    if declaration.allowed_backend_kinds.is_empty() {
        declaration.allowed_backend_kinds = vec![kura_sandbox::BackendKind::Subprocess];
    }
    if declaration.required_enforcement_strength.trim().is_empty() {
        declaration.required_enforcement_strength = "declared_only".to_string();
    }
    clone_declaration(declaration)
}

/// Go defaultDeclaration.
#[must_use]
pub fn default_declaration() -> Declaration {
    Declaration {
        execution_mode: kura_sandbox::ExecutionMode::Subprocess,
        allowed_backend_kinds: vec![kura_sandbox::BackendKind::Subprocess],
        network_mode: kura_sandbox::NetworkMode::Deny,
        approval_mode: kura_sandbox::ApprovalMode::Allow,
        required_enforcement_strength: "declared_only".to_string(),
        active: true,
        ..Declaration::default()
    }
}

/// Go cloneDeclaration (deep copy of nested slices).
#[must_use]
pub fn clone_declaration(declaration: Declaration) -> Declaration {
    Declaration {
        allowed_backend_kinds: clone_backend_kinds(&declaration.allowed_backend_kinds),
        read_roots: clone_strings(&declaration.read_roots),
        write_roots: clone_strings(&declaration.write_roots),
        allowed_hosts: clone_strings(&declaration.allowed_hosts),
        allowed_ports: declaration.allowed_ports.clone(),
        ..declaration
    }
}

/// Go cloneDeclarationPtr.
#[must_use]
pub fn clone_declaration_ptr(declaration: Declaration) -> Option<Declaration> {
    Some(clone_declaration(declaration))
}

/// Go cloneCatalogManagement.
#[must_use]
pub fn clone_catalog_management(management: &Option<CatalogManagement>) -> Option<CatalogManagement> {
    management.as_ref().map(|m| {
        let mut cloned = m.clone();
        cloned.install_input_snapshot = clone_catalog_install_snapshot(&m.install_input_snapshot);
        cloned.last_revalidation = clone_revalidation_snapshot(m.last_revalidation.as_ref());
        cloned
    })
}

/// Go cloneCatalogInstallSnapshot.
#[must_use]
pub fn clone_catalog_install_snapshot(snapshot: &CatalogInstallSnapshot) -> CatalogInstallSnapshot {
    CatalogInstallSnapshot {
        args: clone_strings(&snapshot.args),
        secret_refs: clean_strings(&snapshot.secret_refs),
        enabled: snapshot.enabled,
        ..snapshot.clone()
    }
}

/// Go cloneWebsocketConfig.
#[must_use]
pub fn clone_websocket_config(config: &Option<WebsocketConfig>) -> Option<WebsocketConfig> {
    config.as_ref().map(|c| WebsocketConfig {
        subprotocols: clone_strings(&c.subprotocols),
        auth: c.auth.clone(),
    })
}

/// Go cloneWebsocketAuthConfig.
#[must_use]
pub fn clone_websocket_auth_config(config: &Option<WebsocketAuthConfig>) -> Option<WebsocketAuthConfig> {
    config.clone()
}

/// Go cloneRevalidationSnapshot.
#[must_use]
pub fn clone_revalidation_snapshot(snapshot: Option<&RevalidationSnapshot>) -> Option<RevalidationSnapshot> {
    snapshot.cloned()
}

/// Go cloneToolMap (map iteration order).
#[must_use]
pub fn clone_tool_map(items: &HashMap<String, Tool>) -> Vec<Tool> {
    items.values().cloned().collect()
}

#[must_use]
pub(crate) fn is_default_catalog_install_snapshot(snapshot: &CatalogInstallSnapshot) -> bool {
    *snapshot == CatalogInstallSnapshot::default()
}

/// Go optionalSingleRoot.
#[must_use]
pub fn optional_single_root(root: &str) -> Vec<String> {
    let root = root.trim();
    if root.is_empty() {
        Vec::new()
    } else {
        vec![root.to_string()]
    }
}
