//! Provider domain types (port of `managed_types.go` + the type declarations in
//! `manager.go`).

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// Each variant serializes as its exact Go wire literal: serde's
// rename_all = "snake_case" mangles acronym variants (ClaudeCodeCLI ->
// claude_code_c_l_i) and Go-era rows/clients carry the Go values.
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

string_enum!(Family {
    BuiltinEcho => "builtin_echo",
    OpenAICompatible => "openai_compatible",
    ClaudeCodeCLI => "claude_code_cli",
    CodexCLI => "codex_cli",
});

string_enum!(AuthMode {
    None => "none",
    ApiKey => "api_key",
    LocalCLIBridge => "local_cli_bridge",
});

string_enum!(Source {
    Builtin => "builtin",
    Config => "config",
    Managed => "managed",
});

string_enum!(ModelSelectionMode {
    Fixed => "fixed",
    Open => "open",
});

string_enum!(AuthStatus {
    Unknown => "unknown",
    LoginRequired => "login_required",
    PendingLogin => "pending_login",
    Authenticated => "authenticated",
    Revoked => "revoked",
    Error => "error",
});

string_enum!(CheckStatus {
    Passed => "passed",
    Failed => "failed",
});

string_enum!(CheckErrorClass {
    Config => "config_error",
    Auth => "auth_error",
    Transport => "transport_error",
    Upstream => "upstream_error",
    Timeout => "timeout",
});

/// Managed-auth state for one tenant/provider pair.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthState {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub provider_id: String,
    pub family: Family,
    pub auth_mode: AuthMode,
    pub status: AuthStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cli_path: String,
    pub cli_available: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub account_label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub account_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub plan: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub auth_method: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub login_command: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub logout_command: Vec<String>,
    pub last_checked_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_authenticated_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_error: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<serde_json::Value>,
}

/// A provider model.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub provider_id: String,
    pub model_id: String,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub default: bool,
    pub available: bool,
    pub source: String,
    pub chat: bool,
    pub stream: bool,
    pub coding: bool,
    pub tool_use: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasoning_levels: Vec<String>,
}

/// A per-provider default-model preference.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preference {
    pub provider_id: String,
    pub default_model: String,
    pub updated_at: DateTime<Utc>,
}

/// Re-exported so consumers of this domain (notably the store) can name roles
/// without taking a direct dependency on the config crate.
pub use kura_config::ModelRole;

/// A persisted model-role assignment.
///
/// Roles are modalities (see [`kura_config::ModelRole`]). Storing the
/// assignment here rather than inside each tool keeps credentials, retries and
/// usage accounting in one place: a tool asks the runtime for a role instead
/// of carrying its own provider configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleBinding {
    pub role: kura_config::ModelRole,
    pub provider_id: String,
    /// Empty means "use the provider's default model".
    pub model: String,
    pub updated_at: DateTime<Utc>,
}

/// Capability flags aggregated across a provider's models.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityFlags {
    pub chat: bool,
    pub stream: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub coding: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tool_use: bool,
}

/// A provider's projected profile.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub provider_id: String,
    pub title: String,
    pub family: Family,
    pub auth_mode: AuthMode,
    pub source: Source,
    pub model_selection_mode: ModelSelectionMode,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub known_models: Vec<String>,
    pub registered: bool,
    pub configured: bool,
    pub ready: bool,
    pub default: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub base_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub request_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub default_model: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub effective_model: String,
    pub effective_timeout_ms: i64,
    pub effective_max_retries: i64,
    pub secret_configured: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub secret_ref: String,
    pub capabilities: CapabilityFlags,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub auth_status: String,
    pub cli_available: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub account_label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub account_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub plan: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub auth_method: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub login_command: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub logout_command: Vec<String>,
    pub available_model_count: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<String>,
}

/// Boxed future for object-safe async traits.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A managed provider bridge: adapts a CLI-backed provider into the registry.
pub trait ManagedBridge: Send + Sync {
    fn provider_id(&self) -> String;
    fn display_name(&self) -> String;
    fn family(&self) -> Family;
    fn auth_mode(&self) -> AuthMode;
    /// Whether the tool this bridge borrows is installed.
    ///
    /// Consulted while building the provider inventory, so it must not run
    /// anything: a bridge with nothing to borrow is left out rather than
    /// listed as a provider that fails every request.
    fn available(&self) -> bool;
    fn detect(&self) -> BoxFuture<'_, Result<(AuthState, Vec<Model>), ProvidersError>>;
    fn start(&self) -> BoxFuture<'_, Result<(AuthState, Vec<Model>), ProvidersError>>;
    fn complete(&self) -> BoxFuture<'_, Result<(AuthState, Vec<Model>), ProvidersError>>;
    fn refresh(&self) -> BoxFuture<'_, Result<(AuthState, Vec<Model>), ProvidersError>>;
    fn revoke(&self) -> BoxFuture<'_, Result<(AuthState, Vec<Model>), ProvidersError>>;
    fn provider(&self) -> Arc<dyn kura_llm::Provider>;
}

/// A registry of managed provider bridges.
pub trait ManagedRegistry: Send + Sync {
    fn list(&self) -> Vec<Arc<dyn ManagedBridge>>;
    fn get(&self, provider_id: &str) -> Option<Arc<dyn ManagedBridge>>;
}

/// Provider manager errors.
#[derive(Debug, thiserror::Error)]
pub enum ProvidersError {
    #[error("model {model:?} is not supported by provider {provider}")]
    ModelNotSupported { model: String, provider: String },
    #[error("managed auth is not supported by provider")]
    ManagedAuthUnsupported,
    #[error("tenant provider auth is unavailable: {0}")]
    ProviderAuthUnavailable(String),
    #[error(transparent)]
    Prepare(#[from] kura_llm::PrepareError),
    #[error(transparent)]
    Dispatch(#[from] kura_llm::FailedDispatch),
}

impl Default for Family {
    fn default() -> Self { Family::BuiltinEcho }
}
impl Default for AuthMode {
    fn default() -> Self { AuthMode::None }
}
impl Default for Source {
    fn default() -> Self { Source::Builtin }
}
impl Default for ModelSelectionMode {
    fn default() -> Self { ModelSelectionMode::Fixed }
}
impl Default for AuthStatus {
    fn default() -> Self { AuthStatus::Unknown }
}
impl Default for CheckStatus {
    fn default() -> Self { CheckStatus::Passed }
}
impl Default for CheckErrorClass {
    fn default() -> Self { CheckErrorClass::Config }
}
