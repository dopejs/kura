#![allow(unreachable_patterns)]
//! Port of the Go `daemon/internal/mcp` package: the MCP server registry, runtime
//! state, tool catalog, and tool-exposure-rule management (transport kind selection,
//! lifecycle, catalog install/refresh/reinstall/uninstall/revalidate, tool
//! authorization, secret-scope projection, and event fan-out).
//!
//! The manager is a synchronous port: Go's context.Context plumbing is dropped and the
//! in-memory registry is guarded by parking_lot::RwLock with insertion-ordered server
//! ids, mirroring the kura-runtime / kura-orchestration manager pattern. SQLite
//! persistence goes through kura-store's MCP CRUD (servers, server states, tools,
//! tool exposure rules) and events fan out through kura-events' Bus plus the store
//! event ledger.
//!
//! Deferred parts (documented at each site):
//! - The sandbox execution starter (AttachedExecutionStarter) is a trait with no
//!   workspace implementation yet; the manager behaves exactly like the Go manager with a
//!   nil sandbox manager (ErrSandboxManagerMissing for stdio lifecycle).
//! - Tenant-context resolution (tenantctx) and the async kura-secrets manager bridge
//!   are not ported; secret resolution falls back to the mcp-secrets.json file in the
//!   data dir (the Go nil-secret-manager path) unless a SecretResolver is injected.
//! - Approval/decision SQLite persistence (store.UpsertApproval / UpsertDecision) and
//!   store.HasActiveMCPToolCalls are not yet in kura-store; the corresponding Go calls
//!   are no-ops / skipped.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use chrono::SecondsFormat;

pub mod catalog;
mod agent_tool;
pub mod manager;
pub mod transport;
pub mod types;

/// String enum with explicit per-variant wire literals (Go `type X string`). Every
/// variant's serde representation is exactly the literal, and as_str/Display agree
/// with it. Used instead of the snake_case macro because MCP literals contain hyphens
/// (e.g. streamable-http).
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

/// Manager validation/lookup failures (Go sentinel errors).
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum McpError {
    #[error("mcp server id is required")]
    ServerIDRequired,
    #[error("mcp declaration id is required")]
    DeclarationIDRequired,
    #[error("mcp sandbox profile id is required")]
    ProfileIDRequired,
    #[error("mcp command is required")]
    CommandRequired,
    #[error("mcp transport kind is unsupported")]
    UnsupportedTransport,
    #[error("mcp auto-restart requires enabled server")]
    AutoRestartRequiresOn,
    #[error("mcp server not found")]
    ServerNotFound,
    #[error("mcp tool name is required")]
    ToolNameRequired,
    #[error("mcp runtime surface is required")]
    RuntimeSurfaceRequired,
    #[error("mcp approval does not authorize this tool use")]
    ApprovalIDInvalid,
    #[error("mcp transport is not configured")]
    TransportNotConfigured,
    #[error("mcp sandbox manager is not configured")]
    SandboxManagerMissing,
    #[error("mcp transport is unavailable")]
    TransportUnavailable,
    #[error("mcp transport is closed")]
    TransportClosed,
    #[error("policy engine is not configured")]
    PolicyNotConfigured,
    #[error("approval not found")]
    ApprovalNotFound,
    #[error("{0}")]
    Other(String),
    #[error("store: {0}")]
    Store(String),
}

pub use agent_tool::{McpTool, qualified_name, tools_for_surface};
pub use manager::{
    AttachedExecution, AttachedExecutionStarter, Manager, SecretResolver, SessionState,
};
pub use types::{
    AuthorizeToolInput, AvailabilityStatus, CatalogAction, CatalogActionStatus,
    CatalogDriftStatus, CatalogEntry, CatalogInstallInput, CatalogInstallResult,
    CatalogInstallSnapshot, CatalogInstallSupport, CatalogLifecycleResult,
    CatalogManagement, CatalogPrerequisite, CatalogRevalidationResult,
    CatalogSecretRequirement, CreateServerInput, Declaration, DiscoveryStatus,
    ExposureMode, InstallMethod, LifecycleAction, LifecycleResponse, LifecycleStatus,
    OriginKind, RevalidationClassification, RevalidationIssue, RevalidationIssueStatus,
    RevalidationSnapshot, SecretSummary, Server, ServerResource, ServerState, Source,
    Tool, ToolAuthorizationResponse, ToolAuthorizationStatus, ToolExposureRule,
    ToolInvocationResult, ToolResource, TransportCapability, TransportHealthStatus,
    TransportKind, UpdateExposureInput, UpdateServerInput, WebsocketAuthConfig,
    WebsocketAuthMode, WebsocketAuthSummary, WebsocketConfig,
};
pub use transport::{
    Session, SessionPipes, StdioTransport, StreamableHTTPTransport, Transport, TransportMux,
    WebsocketTransport,
};

/// Go resourceKindServer.
pub const RESOURCE_KIND_SERVER: &str = "mcp_server";
/// Go resourceKindTool.
pub const RESOURCE_KIND_TOOL: &str = "mcp_tool";

/// Go isTerminalStatus.
#[must_use]
pub fn is_terminal_status(status: LifecycleStatus) -> bool {
    matches!(
        status,
        LifecycleStatus::Disabled
            | LifecycleStatus::Stopped
            | LifecycleStatus::Failed
            | LifecycleStatus::Denied
            | LifecycleStatus::Unsupported
    )
}

/// Go string(Environment) — "test" / "prod".
#[must_use]
pub fn environment_scope(environment: kura_config::Environment) -> String {
    match environment {
        kura_config::Environment::Prod => "prod".to_string(),
        kura_config::Environment::Test => "test".to_string(),
        // Embedded shares the non-production isolation scope with test.
        kura_config::Environment::Embedded => "test".to_string(),
    }
}

/// Go LiveValidationMatrixRows (live_validation.go): MCP tool calls are unsupported by
/// the default live-validation support matrix.
#[must_use]
pub fn live_validation_matrix_rows() -> Vec<kura_livevalidation::MatrixRow> {
    let tool_class = kura_livevalidation::ToolClass::from(kura_livevalidation::ToolClass::MCP_TOOL_CALL);
    match kura_livevalidation::default_matrix_row(&tool_class) {
        Some(row) => vec![row],
        None => Vec::new(),
    }
}

/// Formats a timestamp like Go's time.RFC3339Nano.
#[must_use]
pub(crate) fn rfc3339_nano(value: chrono::DateTime<chrono::Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Nanos, true)
}

/// Go firstNonEmpty: the first value whose trimmed form is non-empty (the trimmed
/// value itself), else "".
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

/// Go cleanStrings: trims and drops empty entries.
#[must_use]
pub fn clean_strings(items: &[String]) -> Vec<String> {
    items
        .iter()
        .map(|item| item.trim())
        .filter(|trimmed| !trimmed.is_empty())
        .map(str::to_string)
        .collect()
}

/// Go cloneStrings.
#[must_use]
pub fn clone_strings(items: &[String]) -> Vec<String> {
    items.to_vec()
}

/// Go cloneStringMap.
#[must_use]
pub fn clone_string_map(items: &std::collections::HashMap<String, String>) -> std::collections::HashMap<String, String> {
    items.clone()
}

/// Go cloneInts.
#[must_use]
pub fn clone_ints(items: &[i64]) -> Vec<i64> {
    items.to_vec()
}

/// Go cloneBackendKinds.
#[must_use]
pub fn clone_backend_kinds(items: &[kura_sandbox::BackendKind]) -> Vec<kura_sandbox::BackendKind> {
    items.to_vec()
}

#[must_use]
pub(crate) fn is_false(value: &bool) -> bool {
    !*value
}

#[must_use]
pub(crate) fn is_zero_i64(value: &i64) -> bool {
    *value == 0
}

/// Go errorString.
#[must_use]
pub(crate) fn error_string(err: Option<&String>) -> String {
    match err {
        Some(err) => err.clone(),
        None => String::new(),
    }
}

/// Go package var mcpBackoffDelay with the test override hook
/// (SetReconnectBackoffDelayForTest). A non-zero override wins.
static MCP_BACKOFF_OVERRIDE_NANOS: AtomicU64 = AtomicU64::new(0);

/// Restores the previous backoff delay on drop (Go SetReconnectBackoffDelayForTest
/// returns a restore func).
pub struct ReconnectBackoffDelayGuard {
    previous: u64,
}

impl Drop for ReconnectBackoffDelayGuard {
    fn drop(&mut self) {
        MCP_BACKOFF_OVERRIDE_NANOS.store(self.previous, Ordering::Relaxed);
    }
}

/// Go SetReconnectBackoffDelayForTest.
pub fn set_reconnect_backoff_delay_for_test(delay: Duration) -> ReconnectBackoffDelayGuard {
    let previous = MCP_BACKOFF_OVERRIDE_NANOS.swap(delay.as_nanos() as u64, Ordering::Relaxed);
    ReconnectBackoffDelayGuard { previous }
}

/// Go package var mcpSessionStartTimeout (10s default) with the test override hook
/// (SetSessionStartTimeoutForTest).
static MCP_SESSION_START_TIMEOUT_NANOS: AtomicU64 = AtomicU64::new(10_000_000_000);

/// Restores the previous session start timeout on drop.
pub struct SessionStartTimeoutGuard {
    previous: u64,
}

impl Drop for SessionStartTimeoutGuard {
    fn drop(&mut self) {
        MCP_SESSION_START_TIMEOUT_NANOS.store(self.previous, Ordering::Relaxed);
    }
}

/// Go SetSessionStartTimeoutForTest.
pub fn set_session_start_timeout_for_test(timeout: Duration) -> SessionStartTimeoutGuard {
    let previous = MCP_SESSION_START_TIMEOUT_NANOS.swap(timeout.as_nanos() as u64, Ordering::Relaxed);
    SessionStartTimeoutGuard { previous }
}

#[must_use]
pub(crate) fn session_start_timeout() -> Duration {
    Duration::from_nanos(MCP_SESSION_START_TIMEOUT_NANOS.load(Ordering::Relaxed))
}

#[must_use]
pub(crate) fn mcp_backoff_delay(failure_count: i64) -> Duration {
    let override_nanos = MCP_BACKOFF_OVERRIDE_NANOS.load(Ordering::Relaxed);
    if override_nanos > 0 {
        return Duration::from_nanos(override_nanos);
    }
    restart_backoff_delay(failure_count)
}

/// Go restartBackoffDelay: 5s doubling backoff capped at 5 minutes.
#[must_use]
pub fn restart_backoff_delay(failure_count: i64) -> Duration {
    const FIVE_SECONDS: i64 = 5_000_000_000;
    const FIVE_MINUTES: i64 = 300 * 1_000_000_000;
    if failure_count <= 0 {
        return Duration::from_nanos(FIVE_SECONDS as u64);
    }
    let mut delay = FIVE_SECONDS;
    let mut i = 1;
    while i < failure_count {
        delay *= 2;
        if delay >= FIVE_MINUTES {
            return Duration::from_nanos(FIVE_MINUTES as u64);
        }
        i += 1;
    }
    if delay > FIVE_MINUTES {
        return Duration::from_nanos(FIVE_MINUTES as u64);
    }
    Duration::from_nanos(delay as u64)
}
