//! Port of `daemon/internal/config`. See `rs/MIGRATION.md` for conventions.
//!
//! Loads the daemon configuration from (in increasing precedence) built-in
//! environment-aware defaults, `<dataDir>/config.json`, and `KURA_*`
//! environment variables, then resolves `*Env` secret references. The test
//! environment (`KURA_ENV=test`, or a `dev` version) targets `~/.kura-test`
//! and bind addr `127.0.0.1:19192`; production targets `~/.kura` and
//! `127.0.0.1:19191`.

mod error;
mod file;
mod load;
mod overrides;
mod projection;
mod types;

pub use error::ConfigError;
pub use load::{
    DEFAULT_CONFIG_FILE_NAME, config_file_path, load, managed_provider_home_dir, resolve_dir,
};
pub use projection::{
    DiscordHostedReadinessProjection, MatrixHostedReadinessProjection,
    SlackHostedReadinessProjection, TelegramHostedReadinessProjection,
};
pub use types::{
    AccountProtocol, AccountProviderConfig,
    Config, ConnectorConfig, DiscordConnectorConfig, Environment, LlmConfig,
    ManagedCliProviderConfig, MatrixConnectorConfig, ModelRole, ModelRoleBinding,
    ModelRoutingConfig, OpenAiCompatibleProviderConfig, SamplingConfig, SlackConnectorConfig,
    TelegramConnectorConfig,
};
