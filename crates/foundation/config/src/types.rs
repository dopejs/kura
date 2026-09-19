//! Public configuration types and environment-aware defaults.

use serde::{Deserialize, Serialize};

/// Daemon runtime environment, selected by `KURA_ENV` / `KURA_VERSION`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Environment {
    /// Production environment: `~/.kura`, bind `127.0.0.1:19191`.
    Prod,
    /// Test environment: `~/.kura-test`, bind `127.0.0.1:19192`.
    Test,
}

/// Root daemon configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// Effective runtime environment.
    pub environment: Environment,
    /// HTTP bind address.
    pub bind_addr: String,
    /// Effective data directory (fully resolved, `~` expanded).
    pub data_dir: String,
    /// Log level filter string.
    pub log_level: String,
    /// Daemon version string.
    pub version: String,
    /// LLM provider configuration.
    pub llm: LlmConfig,
    /// IM connector configuration.
    pub connectors: ConnectorConfig,
    /// Outbound-request policy applied at every egress point.
    #[serde(default)]
    pub egress: EgressConfig,
    /// Persistence settings (Stage 10.2: the connection pool).
    #[serde(default)]
    pub store: StoreConfig,
}

/// Persistence settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StoreConfig {
    /// Query-only reader connections opened beside the single writer
    /// (Stage 10.2). `0` keeps the pre-pool behaviour: every read goes
    /// through the writer. Reads on a reader connection run concurrently
    /// with writes under WAL.
    pub readers: usize,
}

impl Config {
    /// Environment-aware defaults matching Go `Load`, before file config and
    /// env overrides are applied.
    pub(crate) fn defaults(environment: Environment, version: String, data_dir: String) -> Self {
        Config {
            store: Default::default(),
            environment,
            bind_addr: default_bind_addr(environment).to_string(),
            data_dir,
            log_level: "info".to_string(),
            version,
            llm: LlmConfig {
                default_timeout_ms: 30000,
                default_max_retries: 0,
                openai_compatible: OpenAiCompatibleProviderConfig {
                    timeout_ms: 30000,
                    stream_first_chunk_timeout_ms: 30000,
                    stream_idle_timeout_ms: 30000,
                    ..Default::default()
                },
                claude: ManagedCliProviderConfig {
                    work_dir: "~".to_string(),
                    ..Default::default()
                },
                codex: ManagedCliProviderConfig {
                    work_dir: "~".to_string(),
                    ..Default::default()
                },
                ..Default::default()
            },
            connectors: ConnectorConfig {
                discord: DiscordConnectorConfig {
                    connector_id: "discord-main".to_string(),
                    display_name: "Discord Main".to_string(),
                    delivery_mode: "gateway".to_string(),
                    require_mention: true,
                    respond_in_dm: true,
                    ..Default::default()
                },
                telegram: TelegramConnectorConfig {
                    connector_id: "telegram-main".to_string(),
                    display_name: "Telegram Main".to_string(),
                    ..Default::default()
                },
                slack: SlackConnectorConfig {
                    connector_id: "slack-main".to_string(),
                    display_name: "Slack Main".to_string(),
                    ..Default::default()
                },
                matrix: MatrixConnectorConfig {
                    connector_id: "matrix-main".to_string(),
                    display_name: "Matrix Main".to_string(),
                    ..Default::default()
                },
            },
            // Deny-by-default: loopback, private ranges, and unconditionally
            // the link-local/metadata range. An operator who needs a local
            // model server widens this explicitly.
            egress: EgressConfig::default(),
        }
    }
}

/// LLM provider selection and per-provider settings.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmConfig {
    /// Name of the default provider.
    pub default_provider: String,
    /// Default model requested from the provider.
    pub default_model: String,
    /// Default request timeout in milliseconds.
    pub default_timeout_ms: i64,
    /// Default retry count for failed requests.
    pub default_max_retries: i64,
    /// OpenAI-compatible HTTP provider settings.
    pub openai_compatible: OpenAiCompatibleProviderConfig,
    /// Managed Claude CLI provider settings.
    pub claude: ManagedCliProviderConfig,
    /// Managed Codex CLI provider settings.
    pub codex: ManagedCliProviderConfig,
}

/// Egress policy, surfaced at the top level of the config because it applies
/// to every outbound point rather than to one subsystem.

/// Settings for an OpenAI-compatible HTTP provider.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAiCompatibleProviderConfig {
    /// Provider base URL.
    #[serde(rename = "baseURL")]
    pub base_url: String,
    /// API key material (after secret-ref resolution).
    pub api_key: String,
    /// Name of the environment variable holding the API key.
    pub api_key_env: String,
    /// Model to request from this provider.
    pub model: String,
    /// Request timeout in milliseconds.
    pub timeout_ms: i64,
    /// Timeout for the first streamed chunk in milliseconds.
    pub stream_first_chunk_timeout_ms: i64,
    /// Idle timeout between streamed chunks in milliseconds.
    pub stream_idle_timeout_ms: i64,
    /// Maximum total stream duration in milliseconds.
    pub stream_max_duration_ms: i64,
}

/// Settings for a managed CLI-backed provider (Claude, Codex).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedCliProviderConfig {
    /// Path to the CLI binary.
    pub cli_path: String,
    /// Default model passed to the CLI.
    pub default_model: String,
    /// Working directory for CLI invocations.
    pub work_dir: String,
}

/// Outbound-request policy shared by every egress point (Stage 7.3).
///
/// Defaults deny loopback, private ranges and — unconditionally —
/// link-local/cloud-metadata addresses. Operators widen this explicitly rather
/// than by omission.
pub type EgressConfig = kura_egress::EgressPolicy;

/// IM connector configurations keyed by platform.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorConfig {
    /// Discord connector.
    pub discord: DiscordConnectorConfig,
    /// Telegram connector.
    pub telegram: TelegramConnectorConfig,
    /// Slack connector.
    pub slack: SlackConnectorConfig,
    /// Matrix connector.
    pub matrix: MatrixConnectorConfig,
}

/// Discord connector configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscordConnectorConfig {
    /// Whether the connector is active.
    pub enabled: bool,
    /// Stable connector identifier.
    pub connector_id: String,
    /// Human-readable connector name.
    pub display_name: String,
    /// Delivery mode (e.g. `gateway`).
    pub delivery_mode: String,
    /// Bot token material (after secret-ref resolution).
    pub bot_token: String,
    /// Name of the environment variable holding the bot token.
    pub bot_token_env: String,
    /// Whether guild messages must mention the bot.
    pub require_mention: bool,
    /// Whether the bot responds in direct messages.
    pub respond_in_dm: bool,
    /// Allowlisted guild IDs.
    pub allowed_guild_ids: Vec<String>,
    /// Allowlisted channel IDs.
    pub allowed_channel_ids: Vec<String>,
}

/// Telegram connector configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramConnectorConfig {
    /// Whether the connector is active.
    pub enabled: bool,
    /// Stable connector identifier.
    pub connector_id: String,
    /// Human-readable connector name.
    pub display_name: String,
    /// Bot token material (after secret-ref resolution).
    pub bot_token: String,
    /// Name of the environment variable holding the bot token.
    pub bot_token_env: String,
    /// Bot API base URL override.
    pub bot_api_base_url: String,
    /// Bot username (without `@`).
    pub bot_username: String,
    /// Allowlisted user IDs.
    pub allowed_user_ids: Vec<String>,
    /// Allowlisted direct-chat IDs.
    pub allowed_direct_chat_ids: Vec<String>,
    /// Allowlisted group IDs.
    pub allowed_group_ids: Vec<String>,
}

/// Slack connector configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlackConnectorConfig {
    /// Whether the connector is active.
    pub enabled: bool,
    /// Stable connector identifier.
    pub connector_id: String,
    /// Human-readable connector name.
    pub display_name: String,
    /// Slack Web API base URL override.
    pub api_base_url: String,
    /// Reference to the bot token in the secret store.
    pub bot_token_secret_ref: String,
    /// OAuth client ID.
    pub oauth_client_id: String,
    /// OAuth client secret material (after secret-ref resolution).
    pub oauth_client_secret: String,
    /// Name of the environment variable holding the OAuth client secret.
    pub oauth_client_secret_env: String,
    /// Slack OAuth API base URL override.
    pub oauth_api_base_url: String,
    /// Hosted workspace binding identifier.
    pub workspace_binding_id: String,
    /// Slack workspace (team) ID.
    pub workspace_id: String,
    /// Bot user ID inside the workspace.
    pub bot_user_id: String,
    /// Allowlisted channel IDs.
    pub allowed_channel_ids: Vec<String>,
    /// Allowlisted DM user IDs.
    #[serde(rename = "allowedDMUserIds")]
    pub allowed_dm_user_ids: Vec<String>,
    /// Allowlisted DM user groups.
    #[serde(rename = "allowedDMUserGroups")]
    pub allowed_dm_user_groups: Vec<String>,
}

/// Matrix connector configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatrixConnectorConfig {
    /// Whether the connector is active.
    pub enabled: bool,
    /// Stable connector identifier.
    pub connector_id: String,
    /// Human-readable connector name.
    pub display_name: String,
    /// Homeserver base URL.
    pub homeserver_url: String,
    /// Homeserver identifier (server name).
    pub homeserver_id: String,
    /// Bot user ID (e.g. `@bot:example.org`).
    pub bot_user_id: String,
    /// Bot access token material (after secret-ref resolution).
    pub bot_access_token: String,
    /// Name of the environment variable holding the bot access token.
    pub bot_access_token_env: String,
    /// Room IDs the bot operates in.
    pub selected_room_ids: Vec<String>,
    /// Allowlisted direct-message user IDs.
    pub allowed_direct_user_ids: Vec<String>,
    /// Configured bot command prefixes.
    pub configured_commands: Vec<String>,
}

/// Default data dir per environment: `~/.kura-test` / `~/.kura`.
pub(crate) fn default_data_dir(env: Environment) -> &'static str {
    match env {
        Environment::Test => "~/.kura-test",
        Environment::Prod => "~/.kura",
    }
}

/// Default bind addr per environment.
pub(crate) fn default_bind_addr(env: Environment) -> &'static str {
    match env {
        Environment::Test => "127.0.0.1:19192",
        Environment::Prod => "127.0.0.1:19191",
    }
}

/// Normalize a raw environment string, returning `None` when unrecognized.
///
/// Matches Go `normalizeEnvironment`: prod/production, and
/// test/testing/dev/development map onto the two environments.
pub(crate) fn normalize_environment(raw: &str) -> Option<Environment> {
    match raw.trim().to_lowercase().as_str() {
        "prod" | "production" => Some(Environment::Prod),
        "test" | "testing" | "dev" | "development" => Some(Environment::Test),
        _ => None,
    }
}

/// Resolve the effective environment from a raw value and the daemon version.
///
/// An explicit recognized value wins; otherwise a `dev` version implies the
/// test environment and anything else implies production. Never `None`.
pub(crate) fn resolve_environment(raw: &str, version: &str) -> Environment {
    if let Some(env) = normalize_environment(raw) {
        return env;
    }
    if version.trim().eq_ignore_ascii_case("dev") {
        return Environment::Test;
    }
    Environment::Prod
}
