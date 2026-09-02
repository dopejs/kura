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
    /// Embedded environment: the daemon is supervised by a host application
    /// that owns the process lifecycle and supplies `KURA_DATA_DIR` and
    /// `KURA_BIND_ADDR` explicitly, typically one instance per host workspace.
    ///
    /// Isolation matches [`Environment::Test`] — managed provider homes stay
    /// inside the data directory and hosted billing quotas are not enforced —
    /// but it is a supported deployment shape with its own scope rather than a
    /// developer convenience, so hosts no longer have to claim to be `test`.
    Embedded,
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
    /// The project this daemon serves, when it serves one.
    ///
    /// Empty for a daemon that is not scoped to a directory, which is the
    /// ordinary case. When set, it is what the `project_tools` sandbox profile
    /// grants access to -- a tool server that has to read the project cannot
    /// run under a profile scoped to the daemon's own data directory, and the
    /// alternative to naming the directory is declaring that the process needs
    /// nothing and handing it the path anyway.
    pub project_root: String,
    /// Log level filter string.
    pub log_level: String,
    /// Daemon version string.
    pub version: String,
    /// LLM provider configuration.
    pub llm: LlmConfig,
    /// IM connector configuration.
    pub connectors: ConnectorConfig,
}

impl Config {
    /// Environment-aware defaults matching Go `Load`, before file config and
    /// env overrides are applied.
    pub(crate) fn defaults(environment: Environment, version: String, data_dir: String) -> Self {
        Config {
            project_root: String::new(),
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
        }
    }
}

/// Sampling parameters sent with each completion request.
///
/// Both fields are optional: `None` omits the key from the request body so a
/// provider applies its own default. This matters for compatibility — some
/// OpenAI-compatible gateways reject an explicit `temperature` on reasoning
/// models — so "unset" and "set to zero" must stay distinguishable.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SamplingConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(rename = "topP", skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
}

/// A modality a model can be routed to.
///
/// Roles are modalities rather than performance tiers: any agent runtime needs
/// to answer "which model handles text, which handles vision, which generates
/// images", and that answer belongs to the runtime rather than to each tool
/// that wants a picture. A tool asks the runtime for the `Image` role instead
/// of carrying its own provider, key and retry policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelRole {
    /// Text reasoning and tool use; the default for chat dispatch.
    Primary,
    /// Image understanding (screenshots, frames, photographs).
    Vision,
    /// Image generation.
    Image,
    /// Video generation.
    Video,
    /// Vector embeddings for indexing and retrieval.
    Embed,
}

impl ModelRole {
    /// Every role, in declaration order.
    pub const ALL: [ModelRole; 5] = [
        ModelRole::Primary,
        ModelRole::Vision,
        ModelRole::Image,
        ModelRole::Video,
        ModelRole::Embed,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ModelRole::Primary => "primary",
            ModelRole::Vision => "vision",
            ModelRole::Image => "image",
            ModelRole::Video => "video",
            ModelRole::Embed => "embed",
        }
    }

    /// Parse a role name, returning `None` when unrecognized.
    #[must_use]
    pub fn parse(raw: &str) -> Option<ModelRole> {
        match raw.trim().to_lowercase().as_str() {
            "primary" => Some(ModelRole::Primary),
            "vision" => Some(ModelRole::Vision),
            "image" => Some(ModelRole::Image),
            "video" => Some(ModelRole::Video),
            "embed" | "embedding" | "embeddings" => Some(ModelRole::Embed),
            _ => None,
        }
    }
}

/// Which provider and model serve one role. An empty `provider` means the role
/// is unrouted; callers must treat that as "capability unavailable" rather than
/// falling back to the default provider, so a missing image model surfaces as
/// a clear error instead of a text model being asked for a picture.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRoleBinding {
    /// Provider id serving this role, or empty when unrouted.
    pub provider: String,
    /// Model id requested from that provider, or empty to use its default.
    pub model: String,
}

impl ModelRoleBinding {
    /// A binding is only usable once a provider has been chosen.
    #[must_use]
    pub fn is_routed(&self) -> bool {
        !self.provider.trim().is_empty()
    }
}

/// Model routing by modality.
///
/// `primary` falls back to [`LlmConfig::default_provider`] and
/// `default_model` when unset, preserving the behaviour of deployments that
/// never configure roles. The other roles have no fallback by design.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRoutingConfig {
    pub primary: ModelRoleBinding,
    pub vision: ModelRoleBinding,
    pub image: ModelRoleBinding,
    pub video: ModelRoleBinding,
    pub embed: ModelRoleBinding,
}

impl ModelRoutingConfig {
    #[must_use]
    pub fn get(&self, role: ModelRole) -> &ModelRoleBinding {
        match role {
            ModelRole::Primary => &self.primary,
            ModelRole::Vision => &self.vision,
            ModelRole::Image => &self.image,
            ModelRole::Video => &self.video,
            ModelRole::Embed => &self.embed,
        }
    }

    pub fn set(&mut self, role: ModelRole, binding: ModelRoleBinding) {
        match role {
            ModelRole::Primary => self.primary = binding,
            ModelRole::Vision => self.vision = binding,
            ModelRole::Image => self.image = binding,
            ModelRole::Video => self.video = binding,
            ModelRole::Embed => self.embed = binding,
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
    /// Providers backed by a subscription the user signed into.
    ///
    /// One entry per account rather than a fixed slot per vendor: a person can
    /// hold several subscriptions, and the set is whatever they signed into
    /// rather than anything this build decides in advance.
    #[serde(default)]
    pub accounts: Vec<AccountProviderConfig>,
    /// Model routing by modality. Defaults to unrouted for every role.
    #[serde(default)]
    pub roles: ModelRoutingConfig,
}

/// Which wire an account's vendor speaks.
///
/// Most of them are OpenAI-compatible and need no protocol of their own; the
/// two that are not each cost a wire, which is why this is named rather than
/// inferred from a URL.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccountProtocol {
    // Named one by one rather than derived. `rename_all = "snake_case"` turns
    // `OpenAiCompatible` into `open_ai_compatible`, which is not what this
    // protocol is called anywhere else in the system -- the config key, the
    // provider id and the Workbench all say `openai_compatible`, and the
    // mismatch surfaced only as a rejected request naming a variant nobody
    // had written.
    #[default]
    #[serde(rename = "openai_compatible")]
    OpenAiCompatible,
    /// Anthropic's Messages API.
    #[serde(rename = "anthropic_messages")]
    AnthropicMessages,
    /// OpenAI's Responses API, as Codex uses it.
    #[serde(rename = "openai_responses")]
    OpenAiResponses,
}

/// One subscription, reached with the grant its owner authorised.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountProviderConfig {
    /// Provider id, unique among providers.
    pub id: String,
    /// Shown in a provider listing.
    pub title: String,
    pub protocol: AccountProtocol,
    #[serde(rename = "baseURL")]
    pub base_url: String,
    pub model: String,
    /// The access token. Replaced in place as it is refreshed, so what is here
    /// is only the value the daemon starts with.
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub headers: std::collections::BTreeMap<String, String>,
}

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
    /// Extra HTTP headers sent with every request to this provider.
    ///
    /// Required by corporate gateways that route on a header. `Authorization`
    /// is reserved for the API key and is ignored here, so a header map cannot
    /// silently replace the configured credential.
    #[serde(default)]
    pub headers: std::collections::BTreeMap<String, String>,
    /// Sampling parameters for this provider.
    #[serde(default)]
    pub sampling: SamplingConfig,
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
        // A host is expected to set KURA_DATA_DIR; this default only keeps an
        // unconfigured embedded daemon away from the prod and test roots.
        Environment::Embedded => "~/.kura-embedded",
    }
}

/// Default bind addr per environment.
pub(crate) fn default_bind_addr(env: Environment) -> &'static str {
    match env {
        Environment::Test => "127.0.0.1:19192",
        Environment::Prod => "127.0.0.1:19191",
        // Hosts allocate a free port and pass KURA_BIND_ADDR; this default only
        // avoids colliding with a prod or test daemon on the same machine.
        Environment::Embedded => "127.0.0.1:19193",
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
        "embedded" | "embed" | "host" | "hosted" => Some(Environment::Embedded),
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
