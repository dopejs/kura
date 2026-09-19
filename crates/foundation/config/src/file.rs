//! On-disk config file schema (`config.json`) and its merge into [`Config`].

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::ConfigError;
use crate::types::{
    Config, ConnectorConfig, DiscordConnectorConfig, LlmConfig, ManagedCliProviderConfig,
    MatrixConnectorConfig, OpenAiCompatibleProviderConfig, SlackConnectorConfig,
    TelegramConnectorConfig, normalize_environment,
};

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct FileConfig {
    environment: String,
    bind_addr: String,
    data_dir: String,
    log_level: String,
    llm: Option<FileLlmConfig>,
    connectors: Option<FileConnectorConfig>,
    store: Option<FileStoreConfig>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct FileStoreConfig {
    readers: Option<usize>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct FileLlmConfig {
    default_provider: String,
    default_model: String,
    default_timeout_ms: i64,
    default_max_retries: i64,
    openai_compatible: Option<FileOpenAiCompatibleProviderConfig>,
    claude: Option<FileManagedCliProviderConfig>,
    codex: Option<FileManagedCliProviderConfig>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct FileOpenAiCompatibleProviderConfig {
    #[serde(rename = "baseURL")]
    base_url: String,
    api_key: String,
    api_key_env: String,
    model: String,
    timeout_ms: i64,
    stream_first_chunk_timeout_ms: i64,
    stream_idle_timeout_ms: i64,
    stream_max_duration_ms: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct FileManagedCliProviderConfig {
    cli_path: String,
    default_model: String,
    work_dir: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct FileConnectorConfig {
    discord: Option<FileDiscordConnectorConfig>,
    telegram: Option<FileTelegramConnectorConfig>,
    slack: Option<FileSlackConnectorConfig>,
    matrix: Option<FileMatrixConnectorConfig>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct FileDiscordConnectorConfig {
    enabled: Option<bool>,
    connector_id: String,
    display_name: String,
    delivery_mode: String,
    bot_token: String,
    bot_token_env: String,
    require_mention: Option<bool>,
    respond_in_dm: Option<bool>,
    allowed_guild_ids: Option<Vec<String>>,
    allowed_channel_ids: Option<Vec<String>>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct FileTelegramConnectorConfig {
    enabled: Option<bool>,
    connector_id: String,
    display_name: String,
    bot_token: String,
    bot_token_env: String,
    bot_api_base_url: String,
    bot_username: String,
    allowed_user_ids: Option<Vec<String>>,
    allowed_direct_chat_ids: Option<Vec<String>>,
    allowed_group_ids: Option<Vec<String>>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct FileSlackConnectorConfig {
    enabled: Option<bool>,
    connector_id: String,
    display_name: String,
    api_base_url: String,
    bot_token_secret_ref: String,
    oauth_client_id: String,
    oauth_client_secret: String,
    oauth_client_secret_env: String,
    oauth_api_base_url: String,
    workspace_binding_id: String,
    workspace_id: String,
    bot_user_id: String,
    allowed_channel_ids: Option<Vec<String>>,
    #[serde(rename = "allowedDMUserIds")]
    allowed_dm_user_ids: Option<Vec<String>>,
    #[serde(rename = "allowedDMUserGroups")]
    allowed_dm_user_groups: Option<Vec<String>>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct FileMatrixConnectorConfig {
    enabled: Option<bool>,
    connector_id: String,
    display_name: String,
    homeserver_url: String,
    homeserver_id: String,
    bot_user_id: String,
    bot_access_token: String,
    bot_access_token_env: String,
    selected_room_ids: Option<Vec<String>>,
    allowed_direct_user_ids: Option<Vec<String>>,
    configured_commands: Option<Vec<String>>,
}

/// Read and decode the file config at `path`. A missing file yields the
/// empty (all-default) file config; any other read or decode failure is an
/// error.
pub(crate) fn load_file_config(path: &Path) -> Result<FileConfig, ConfigError> {
    let raw = match fs::read(path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(FileConfig::default());
        }
        Err(err) => {
            return Err(ConfigError::ReadFile {
                path: path.to_path_buf(),
                source: err,
            });
        }
    };
    serde_json::from_slice(&raw).map_err(|source| ConfigError::DecodeFile {
        path: path.to_path_buf(),
        source,
    })
}

/// Merge non-empty file values into `cfg` (Go `applyFileConfig`).
pub(crate) fn apply_file_config(cfg: &mut Config, file: FileConfig) {
    if let Some(env) = normalize_environment(&file.environment) {
        cfg.environment = env;
    }
    if !file.bind_addr.is_empty() {
        cfg.bind_addr = file.bind_addr;
    }
    if !file.data_dir.is_empty() {
        cfg.data_dir = file.data_dir;
    }
    if !file.log_level.is_empty() {
        cfg.log_level = file.log_level;
    }
    if let Some(llm) = file.llm {
        apply_file_llm_config(&mut cfg.llm, llm);
    }
    if let Some(connectors) = file.connectors {
        apply_file_connector_config(&mut cfg.connectors, connectors);
    }
    if let Some(store) = file.store {
        if let Some(readers) = store.readers {
            cfg.store.readers = readers;
        }
    }
}

fn apply_file_llm_config(cfg: &mut LlmConfig, file: FileLlmConfig) {
    if !file.default_provider.is_empty() {
        cfg.default_provider = file.default_provider;
    }
    if !file.default_model.is_empty() {
        cfg.default_model = file.default_model;
    }
    if file.default_timeout_ms > 0 {
        cfg.default_timeout_ms = file.default_timeout_ms;
    }
    // Go checks `>= 0`: a present non-negative value (including the implicit
    // zero of an absent key) replaces the default; negative values are kept
    // out.
    if file.default_max_retries >= 0 {
        cfg.default_max_retries = file.default_max_retries;
    }
    if let Some(openai) = file.openai_compatible {
        apply_file_openai_compatible_config(&mut cfg.openai_compatible, openai);
    }
    if let Some(claude) = file.claude {
        apply_file_managed_cli_config(&mut cfg.claude, claude);
    }
    if let Some(codex) = file.codex {
        apply_file_managed_cli_config(&mut cfg.codex, codex);
    }
}

fn apply_file_openai_compatible_config(
    cfg: &mut OpenAiCompatibleProviderConfig,
    file: FileOpenAiCompatibleProviderConfig,
) {
    if !file.base_url.is_empty() {
        cfg.base_url = file.base_url;
    }
    if !file.api_key.is_empty() {
        cfg.api_key = file.api_key;
    }
    if !file.api_key_env.is_empty() {
        cfg.api_key_env = file.api_key_env;
    }
    if !file.model.is_empty() {
        cfg.model = file.model;
    }
    if file.timeout_ms > 0 {
        cfg.timeout_ms = file.timeout_ms;
    }
    if file.stream_first_chunk_timeout_ms > 0 {
        cfg.stream_first_chunk_timeout_ms = file.stream_first_chunk_timeout_ms;
    }
    if file.stream_idle_timeout_ms > 0 {
        cfg.stream_idle_timeout_ms = file.stream_idle_timeout_ms;
    }
    if file.stream_max_duration_ms > 0 {
        cfg.stream_max_duration_ms = file.stream_max_duration_ms;
    }
}

fn apply_file_managed_cli_config(
    cfg: &mut ManagedCliProviderConfig,
    file: FileManagedCliProviderConfig,
) {
    if !file.cli_path.is_empty() {
        cfg.cli_path = file.cli_path;
    }
    if !file.default_model.is_empty() {
        cfg.default_model = file.default_model;
    }
    if !file.work_dir.is_empty() {
        cfg.work_dir = file.work_dir;
    }
}

fn apply_file_connector_config(cfg: &mut ConnectorConfig, file: FileConnectorConfig) {
    if let Some(discord) = file.discord {
        apply_file_discord_connector_config(&mut cfg.discord, discord);
    }
    if let Some(telegram) = file.telegram {
        apply_file_telegram_connector_config(&mut cfg.telegram, telegram);
    }
    if let Some(slack) = file.slack {
        apply_file_slack_connector_config(&mut cfg.slack, slack);
    }
    if let Some(matrix) = file.matrix {
        apply_file_matrix_connector_config(&mut cfg.matrix, matrix);
    }
}

fn apply_file_discord_connector_config(
    cfg: &mut DiscordConnectorConfig,
    file: FileDiscordConnectorConfig,
) {
    if let Some(enabled) = file.enabled {
        cfg.enabled = enabled;
    }
    if !file.connector_id.is_empty() {
        cfg.connector_id = file.connector_id;
    }
    if !file.display_name.is_empty() {
        cfg.display_name = file.display_name;
    }
    if !file.delivery_mode.is_empty() {
        cfg.delivery_mode = file.delivery_mode;
    }
    if !file.bot_token.is_empty() {
        cfg.bot_token = file.bot_token;
    }
    if !file.bot_token_env.is_empty() {
        cfg.bot_token_env = file.bot_token_env;
    }
    if let Some(require_mention) = file.require_mention {
        cfg.require_mention = require_mention;
    }
    if let Some(respond_in_dm) = file.respond_in_dm {
        cfg.respond_in_dm = respond_in_dm;
    }
    if let Some(ids) = file.allowed_guild_ids {
        cfg.allowed_guild_ids = ids;
    }
    if let Some(ids) = file.allowed_channel_ids {
        cfg.allowed_channel_ids = ids;
    }
}

fn apply_file_telegram_connector_config(
    cfg: &mut TelegramConnectorConfig,
    file: FileTelegramConnectorConfig,
) {
    if let Some(enabled) = file.enabled {
        cfg.enabled = enabled;
    }
    if !file.connector_id.is_empty() {
        cfg.connector_id = file.connector_id;
    }
    if !file.display_name.is_empty() {
        cfg.display_name = file.display_name;
    }
    if !file.bot_token.is_empty() {
        cfg.bot_token = file.bot_token;
    }
    if !file.bot_token_env.is_empty() {
        cfg.bot_token_env = file.bot_token_env;
    }
    if !file.bot_api_base_url.is_empty() {
        cfg.bot_api_base_url = file.bot_api_base_url;
    }
    if !file.bot_username.is_empty() {
        cfg.bot_username = file.bot_username;
    }
    if let Some(ids) = file.allowed_user_ids {
        cfg.allowed_user_ids = ids;
    }
    if let Some(ids) = file.allowed_direct_chat_ids {
        cfg.allowed_direct_chat_ids = ids;
    }
    if let Some(ids) = file.allowed_group_ids {
        cfg.allowed_group_ids = ids;
    }
}

fn apply_file_slack_connector_config(
    cfg: &mut SlackConnectorConfig,
    file: FileSlackConnectorConfig,
) {
    if let Some(enabled) = file.enabled {
        cfg.enabled = enabled;
    }
    if !file.connector_id.is_empty() {
        cfg.connector_id = file.connector_id;
    }
    if !file.display_name.is_empty() {
        cfg.display_name = file.display_name;
    }
    if !file.api_base_url.is_empty() {
        cfg.api_base_url = file.api_base_url;
    }
    if !file.bot_token_secret_ref.is_empty() {
        cfg.bot_token_secret_ref = file.bot_token_secret_ref;
    }
    if !file.oauth_client_id.is_empty() {
        cfg.oauth_client_id = file.oauth_client_id;
    }
    if !file.oauth_client_secret.is_empty() {
        cfg.oauth_client_secret = file.oauth_client_secret;
    }
    if !file.oauth_client_secret_env.is_empty() {
        cfg.oauth_client_secret_env = file.oauth_client_secret_env;
    }
    if !file.oauth_api_base_url.is_empty() {
        cfg.oauth_api_base_url = file.oauth_api_base_url;
    }
    if !file.workspace_binding_id.is_empty() {
        cfg.workspace_binding_id = file.workspace_binding_id;
    }
    if !file.workspace_id.is_empty() {
        cfg.workspace_id = file.workspace_id;
    }
    if !file.bot_user_id.is_empty() {
        cfg.bot_user_id = file.bot_user_id;
    }
    if let Some(ids) = file.allowed_channel_ids {
        cfg.allowed_channel_ids = ids;
    }
    if let Some(ids) = file.allowed_dm_user_ids {
        cfg.allowed_dm_user_ids = ids;
    }
    if let Some(ids) = file.allowed_dm_user_groups {
        cfg.allowed_dm_user_groups = ids;
    }
}

fn apply_file_matrix_connector_config(
    cfg: &mut MatrixConnectorConfig,
    file: FileMatrixConnectorConfig,
) {
    if let Some(enabled) = file.enabled {
        cfg.enabled = enabled;
    }
    if !file.connector_id.is_empty() {
        cfg.connector_id = file.connector_id;
    }
    if !file.display_name.is_empty() {
        cfg.display_name = file.display_name;
    }
    if !file.homeserver_url.is_empty() {
        cfg.homeserver_url = file.homeserver_url;
    }
    if !file.homeserver_id.is_empty() {
        cfg.homeserver_id = file.homeserver_id;
    }
    if !file.bot_user_id.is_empty() {
        cfg.bot_user_id = file.bot_user_id;
    }
    if !file.bot_access_token.is_empty() {
        cfg.bot_access_token = file.bot_access_token;
    }
    if !file.bot_access_token_env.is_empty() {
        cfg.bot_access_token_env = file.bot_access_token_env;
    }
    if let Some(ids) = file.selected_room_ids {
        cfg.selected_room_ids = ids;
    }
    if let Some(ids) = file.allowed_direct_user_ids {
        cfg.allowed_direct_user_ids = ids;
    }
    if let Some(commands) = file.configured_commands {
        cfg.configured_commands = commands;
    }
}

// ---------------------------------------------------------------------------
// Write-path validation (Stage 7.1)
// ---------------------------------------------------------------------------

/// Dotted paths at which the file format can carry a plaintext secret. The
/// daemon's own write path refuses these: the indirections (`*Env`, secret
/// refs) are the supported way, and an operator hand-editing the file is a
/// different trust decision from an API caller writing to disk.
pub const INLINE_SECRET_PATHS: &[&str] = &[
    "llm.openaiCompatible.apiKey",
    "connectors.discord.botToken",
    "connectors.telegram.botToken",
    "connectors.slack.oauthClientSecret",
    "connectors.matrix.botAccessToken",
];

/// Outcome of validating a candidate `config.json` body.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileConfigValidation {
    /// Keys present in the input that the file format does not know. Ignored
    /// on load (every field is `#[serde(default)]`), which is exactly why a
    /// strict write must refuse them: a typo would otherwise be accepted and
    /// silently do nothing.
    pub unknown_keys: Vec<String>,
    /// Paths carrying a non-empty inline secret value.
    pub inline_secrets: Vec<String>,
}

/// Validates a candidate `config.json` without touching disk.
///
/// Parses `raw` as JSON, decodes it as [`FileConfig`] (so a type error
/// surfaces here rather than at the next boot), then round-trips the decoded
/// value and reports any input key absent from the round-trip as unknown.
/// Because every `File*` struct is `#[serde(default)]` with no
/// `skip_serializing_if`, the round-trip carries every known key, so the
/// comparison needs no hand-maintained key list.
pub fn validate_file_config_json(raw: &[u8]) -> Result<FileConfigValidation, ConfigError> {
    let input: serde_json::Value =
        serde_json::from_slice(raw).map_err(|source| ConfigError::DecodeFile {
            path: Path::new("<candidate>").to_path_buf(),
            source,
        })?;
    let decoded: FileConfig =
        serde_json::from_value(input.clone()).map_err(|source| ConfigError::DecodeFile {
            path: Path::new("<candidate>").to_path_buf(),
            source,
        })?;
    let known = serde_json::to_value(&decoded).unwrap_or(serde_json::Value::Null);

    let mut unknown_keys = Vec::new();
    collect_unknown_keys(&input, &known, "", &mut unknown_keys);

    let inline_secrets = INLINE_SECRET_PATHS
        .iter()
        .filter(|path| {
            lookup_path(&input, path)
                .and_then(serde_json::Value::as_str)
                .is_some_and(|v| !v.trim().is_empty())
        })
        .map(|p| (*p).to_string())
        .collect();

    Ok(FileConfigValidation {
        unknown_keys,
        inline_secrets,
    })
}

fn collect_unknown_keys(
    input: &serde_json::Value,
    known: &serde_json::Value,
    prefix: &str,
    out: &mut Vec<String>,
) {
    let (Some(input_map), Some(known_map)) = (input.as_object(), known.as_object()) else {
        return;
    };
    for (key, value) in input_map {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match known_map.get(key) {
            None => out.push(path),
            Some(known_child) => collect_unknown_keys(value, known_child, &path, out),
        }
    }
}

fn lookup_path<'a>(value: &'a serde_json::Value, dotted: &str) -> Option<&'a serde_json::Value> {
    dotted
        .split('.')
        .try_fold(value, |cursor, seg| cursor.get(seg))
}
