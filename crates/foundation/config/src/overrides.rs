//! Environment variable overrides and secret-reference resolution.

use crate::types::{Config, ModelRole, ModelRoleBinding, resolve_environment};

/// Apply `KURA_*` environment variable overrides onto `cfg`
/// (Go `applyEnvOverrides`). Env vars win over file config and defaults.
pub(crate) fn apply_env_overrides(cfg: &mut Config) {
    // Go resolves this unconditionally: `resolveEnvironment` never returns an
    // empty environment, so the env-derived resolution always replaces the
    // file-config environment. Uses `cfg.version` before the version override
    // below, matching Go statement order.
    cfg.environment = resolve_environment(&getenv("KURA_ENV", ""), &cfg.version);
    cfg.bind_addr = getenv("KURA_BIND_ADDR", &cfg.bind_addr);
    cfg.data_dir = getenv("KURA_DATA_DIR", &cfg.data_dir);
    cfg.log_level = getenv("KURA_LOG_LEVEL", &cfg.log_level);
    cfg.version = getenv("KURA_VERSION", &cfg.version);

    cfg.llm.default_provider = getenv("KURA_LLM_DEFAULT_PROVIDER", &cfg.llm.default_provider);
    cfg.llm.default_model = getenv("KURA_LLM_DEFAULT_MODEL", &cfg.llm.default_model);
    cfg.llm.default_timeout_ms = getenv_int("KURA_LLM_DEFAULT_TIMEOUT_MS", cfg.llm.default_timeout_ms);
    cfg.llm.default_max_retries = getenv_int("KURA_LLM_DEFAULT_MAX_RETRIES", cfg.llm.default_max_retries);
    // Signed-in subscriptions, as JSON, because the set is whatever the user
    // signed into rather than a fixed number of slots. Passed in the
    // environment for the same reason the API key is: it keeps the credential
    // in one place on disk instead of copying it into a config file.
    //
    // Unparseable input leaves the configured accounts alone. Replacing them
    // with nothing would silently drop every subscription the user has.
    if let Ok(raw) = std::env::var("KURA_LLM_ACCOUNTS") {
        if !raw.trim().is_empty() {
            match serde_json::from_str::<Vec<crate::types::AccountProviderConfig>>(&raw) {
                Ok(accounts) => cfg.llm.accounts = accounts,
                Err(error) => {
                    eprintln!("[kura] ignoring KURA_LLM_ACCOUNTS: {error}");
                }
            }
        }
    }

    cfg.llm.openai_compatible.base_url = getenv(
        "KURA_LLM_OPENAI_COMPATIBLE_BASE_URL",
        &cfg.llm.openai_compatible.base_url,
    );
    cfg.llm.openai_compatible.api_key = getenv(
        "KURA_LLM_OPENAI_COMPATIBLE_API_KEY",
        &cfg.llm.openai_compatible.api_key,
    );
    cfg.llm.openai_compatible.api_key_env = getenv(
        "KURA_LLM_OPENAI_COMPATIBLE_API_KEY_ENV",
        &cfg.llm.openai_compatible.api_key_env,
    );
    cfg.llm.openai_compatible.model = getenv(
        "KURA_LLM_OPENAI_COMPATIBLE_MODEL",
        &cfg.llm.openai_compatible.model,
    );
    cfg.llm.openai_compatible.timeout_ms = getenv_int(
        "KURA_LLM_OPENAI_COMPATIBLE_TIMEOUT_MS",
        cfg.llm.openai_compatible.timeout_ms,
    );
    cfg.llm.openai_compatible.stream_first_chunk_timeout_ms = getenv_int(
        "KURA_LLM_OPENAI_COMPATIBLE_STREAM_FIRST_CHUNK_TIMEOUT_MS",
        cfg.llm.openai_compatible.stream_first_chunk_timeout_ms,
    );
    cfg.llm.openai_compatible.stream_idle_timeout_ms = getenv_int(
        "KURA_LLM_OPENAI_COMPATIBLE_STREAM_IDLE_TIMEOUT_MS",
        cfg.llm.openai_compatible.stream_idle_timeout_ms,
    );
    cfg.llm.openai_compatible.stream_max_duration_ms = getenv_int(
        "KURA_LLM_OPENAI_COMPATIBLE_STREAM_MAX_DURATION_MS",
        cfg.llm.openai_compatible.stream_max_duration_ms,
    );
    cfg.llm.openai_compatible.sampling.temperature = getenv_opt_f64(
        "KURA_LLM_OPENAI_COMPATIBLE_TEMPERATURE",
        cfg.llm.openai_compatible.sampling.temperature,
    );
    cfg.llm.openai_compatible.sampling.top_p = getenv_opt_f64(
        "KURA_LLM_OPENAI_COMPATIBLE_TOP_P",
        cfg.llm.openai_compatible.sampling.top_p,
    );

    // Roles are overridden as `provider[:model]`, e.g. `KURA_LLM_ROLE_IMAGE=studio:sd3-medium`.
    for role in ModelRole::ALL {
        let key = format!("KURA_LLM_ROLE_{}", role.as_str().to_uppercase());
        let raw = getenv(&key, "");
        if raw.trim().is_empty() {
            continue;
        }
        cfg.llm.roles.set(role, parse_role_binding(&raw));
    }

    cfg.llm.claude.cli_path = getenv("KURA_LLM_CLAUDE_CLI_PATH", &cfg.llm.claude.cli_path);
    cfg.llm.claude.default_model = getenv("KURA_LLM_CLAUDE_MODEL", &cfg.llm.claude.default_model);
    cfg.llm.claude.work_dir = getenv("KURA_LLM_CLAUDE_WORKDIR", &cfg.llm.claude.work_dir);
    cfg.llm.codex.cli_path = getenv("KURA_LLM_CODEX_CLI_PATH", &cfg.llm.codex.cli_path);
    cfg.llm.codex.default_model = getenv("KURA_LLM_CODEX_MODEL", &cfg.llm.codex.default_model);
    cfg.llm.codex.work_dir = getenv("KURA_LLM_CODEX_WORKDIR", &cfg.llm.codex.work_dir);
    cfg.llm.claude.cli_path = getenv("KURA_LLM_CLAUDE_CLI_PATH", &cfg.llm.claude.cli_path);
    cfg.llm.claude.default_model = getenv("KURA_LLM_CLAUDE_MODEL", &cfg.llm.claude.default_model);
    cfg.llm.claude.work_dir = getenv("KURA_LLM_CLAUDE_WORKDIR", &cfg.llm.claude.work_dir);
    cfg.llm.codex.cli_path = getenv("KURA_LLM_CODEX_CLI_PATH", &cfg.llm.codex.cli_path);
    cfg.llm.codex.default_model = getenv("KURA_LLM_CODEX_MODEL", &cfg.llm.codex.default_model);
    cfg.llm.codex.work_dir = getenv("KURA_LLM_CODEX_WORKDIR", &cfg.llm.codex.work_dir);

    cfg.connectors.discord.enabled =
        getenv_bool("KURA_CONNECTORS_DISCORD_ENABLED", cfg.connectors.discord.enabled);
    cfg.connectors.discord.connector_id = getenv(
        "KURA_CONNECTORS_DISCORD_CONNECTOR_ID",
        &cfg.connectors.discord.connector_id,
    );
    cfg.connectors.discord.display_name = getenv(
        "KURA_CONNECTORS_DISCORD_DISPLAY_NAME",
        &cfg.connectors.discord.display_name,
    );
    cfg.connectors.discord.delivery_mode = getenv(
        "KURA_CONNECTORS_DISCORD_DELIVERY_MODE",
        &cfg.connectors.discord.delivery_mode,
    );
    cfg.connectors.discord.bot_token = getenv(
        "KURA_CONNECTORS_DISCORD_BOT_TOKEN",
        &cfg.connectors.discord.bot_token,
    );
    cfg.connectors.discord.bot_token_env = getenv(
        "KURA_CONNECTORS_DISCORD_BOT_TOKEN_ENV",
        &cfg.connectors.discord.bot_token_env,
    );
    cfg.connectors.discord.require_mention = getenv_bool(
        "KURA_CONNECTORS_DISCORD_REQUIRE_MENTION",
        cfg.connectors.discord.require_mention,
    );
    cfg.connectors.discord.respond_in_dm = getenv_bool(
        "KURA_CONNECTORS_DISCORD_RESPOND_IN_DM",
        cfg.connectors.discord.respond_in_dm,
    );
    cfg.connectors.discord.allowed_guild_ids = getenv_csv(
        "KURA_CONNECTORS_DISCORD_ALLOWED_GUILD_IDS",
        &cfg.connectors.discord.allowed_guild_ids,
    );
    cfg.connectors.discord.allowed_channel_ids = getenv_csv(
        "KURA_CONNECTORS_DISCORD_ALLOWED_CHANNEL_IDS",
        &cfg.connectors.discord.allowed_channel_ids,
    );

    cfg.connectors.telegram.enabled =
        getenv_bool("KURA_CONNECTORS_TELEGRAM_ENABLED", cfg.connectors.telegram.enabled);
    cfg.connectors.telegram.connector_id = getenv(
        "KURA_CONNECTORS_TELEGRAM_CONNECTOR_ID",
        &cfg.connectors.telegram.connector_id,
    );
    cfg.connectors.telegram.display_name = getenv(
        "KURA_CONNECTORS_TELEGRAM_DISPLAY_NAME",
        &cfg.connectors.telegram.display_name,
    );
    cfg.connectors.telegram.bot_token = getenv(
        "KURA_CONNECTORS_TELEGRAM_BOT_TOKEN",
        &cfg.connectors.telegram.bot_token,
    );
    cfg.connectors.telegram.bot_token_env = getenv(
        "KURA_CONNECTORS_TELEGRAM_BOT_TOKEN_ENV",
        &cfg.connectors.telegram.bot_token_env,
    );
    cfg.connectors.telegram.bot_api_base_url = getenv(
        "KURA_CONNECTORS_TELEGRAM_BOT_API_BASE_URL",
        &cfg.connectors.telegram.bot_api_base_url,
    );
    cfg.connectors.telegram.bot_username = getenv(
        "KURA_CONNECTORS_TELEGRAM_BOT_USERNAME",
        &cfg.connectors.telegram.bot_username,
    );
    cfg.connectors.telegram.allowed_user_ids = getenv_csv(
        "KURA_CONNECTORS_TELEGRAM_ALLOWED_USER_IDS",
        &cfg.connectors.telegram.allowed_user_ids,
    );
    cfg.connectors.telegram.allowed_direct_chat_ids = getenv_csv(
        "KURA_CONNECTORS_TELEGRAM_ALLOWED_DIRECT_CHAT_IDS",
        &cfg.connectors.telegram.allowed_direct_chat_ids,
    );
    cfg.connectors.telegram.allowed_group_ids = getenv_csv(
        "KURA_CONNECTORS_TELEGRAM_ALLOWED_GROUP_IDS",
        &cfg.connectors.telegram.allowed_group_ids,
    );

    cfg.connectors.slack.enabled =
        getenv_bool("KURA_CONNECTORS_SLACK_ENABLED", cfg.connectors.slack.enabled);
    cfg.connectors.slack.connector_id = getenv(
        "KURA_CONNECTORS_SLACK_CONNECTOR_ID",
        &cfg.connectors.slack.connector_id,
    );
    cfg.connectors.slack.display_name = getenv(
        "KURA_CONNECTORS_SLACK_DISPLAY_NAME",
        &cfg.connectors.slack.display_name,
    );
    cfg.connectors.slack.api_base_url = getenv(
        "KURA_CONNECTORS_SLACK_API_BASE_URL",
        &cfg.connectors.slack.api_base_url,
    );
    cfg.connectors.slack.bot_token_secret_ref = getenv(
        "KURA_CONNECTORS_SLACK_BOT_TOKEN_SECRET_REF",
        &cfg.connectors.slack.bot_token_secret_ref,
    );
    cfg.connectors.slack.oauth_client_id = getenv(
        "KURA_CONNECTORS_SLACK_OAUTH_CLIENT_ID",
        &cfg.connectors.slack.oauth_client_id,
    );
    cfg.connectors.slack.oauth_client_secret = getenv(
        "KURA_CONNECTORS_SLACK_OAUTH_CLIENT_SECRET",
        &cfg.connectors.slack.oauth_client_secret,
    );
    cfg.connectors.slack.oauth_client_secret_env = getenv(
        "KURA_CONNECTORS_SLACK_OAUTH_CLIENT_SECRET_ENV",
        &cfg.connectors.slack.oauth_client_secret_env,
    );
    cfg.connectors.slack.oauth_api_base_url = getenv(
        "KURA_CONNECTORS_SLACK_OAUTH_API_BASE_URL",
        &cfg.connectors.slack.oauth_api_base_url,
    );
    cfg.connectors.slack.workspace_binding_id = getenv(
        "KURA_CONNECTORS_SLACK_WORKSPACE_BINDING_ID",
        &cfg.connectors.slack.workspace_binding_id,
    );
    cfg.connectors.slack.workspace_id = getenv(
        "KURA_CONNECTORS_SLACK_WORKSPACE_ID",
        &cfg.connectors.slack.workspace_id,
    );
    cfg.connectors.slack.bot_user_id = getenv(
        "KURA_CONNECTORS_SLACK_BOT_USER_ID",
        &cfg.connectors.slack.bot_user_id,
    );
    cfg.connectors.slack.allowed_channel_ids = getenv_csv(
        "KURA_CONNECTORS_SLACK_ALLOWED_CHANNEL_IDS",
        &cfg.connectors.slack.allowed_channel_ids,
    );
    cfg.connectors.slack.allowed_dm_user_ids = getenv_csv(
        "KURA_CONNECTORS_SLACK_ALLOWED_DM_USER_IDS",
        &cfg.connectors.slack.allowed_dm_user_ids,
    );
    cfg.connectors.slack.allowed_dm_user_groups = getenv_csv(
        "KURA_CONNECTORS_SLACK_ALLOWED_DM_USER_GROUPS",
        &cfg.connectors.slack.allowed_dm_user_groups,
    );

    cfg.connectors.matrix.enabled =
        getenv_bool("KURA_CONNECTORS_MATRIX_ENABLED", cfg.connectors.matrix.enabled);
    cfg.connectors.matrix.connector_id = getenv(
        "KURA_CONNECTORS_MATRIX_CONNECTOR_ID",
        &cfg.connectors.matrix.connector_id,
    );
    cfg.connectors.matrix.display_name = getenv(
        "KURA_CONNECTORS_MATRIX_DISPLAY_NAME",
        &cfg.connectors.matrix.display_name,
    );
    cfg.connectors.matrix.homeserver_url = getenv(
        "KURA_CONNECTORS_MATRIX_HOMESERVER_URL",
        &cfg.connectors.matrix.homeserver_url,
    );
    cfg.connectors.matrix.homeserver_id = getenv(
        "KURA_CONNECTORS_MATRIX_HOMESERVER_ID",
        &cfg.connectors.matrix.homeserver_id,
    );
    cfg.connectors.matrix.bot_user_id = getenv(
        "KURA_CONNECTORS_MATRIX_BOT_USER_ID",
        &cfg.connectors.matrix.bot_user_id,
    );
    cfg.connectors.matrix.bot_access_token = getenv(
        "KURA_CONNECTORS_MATRIX_BOT_ACCESS_TOKEN",
        &cfg.connectors.matrix.bot_access_token,
    );
    cfg.connectors.matrix.bot_access_token_env = getenv(
        "KURA_CONNECTORS_MATRIX_BOT_ACCESS_TOKEN_ENV",
        &cfg.connectors.matrix.bot_access_token_env,
    );
    cfg.connectors.matrix.selected_room_ids = getenv_csv(
        "KURA_CONNECTORS_MATRIX_SELECTED_ROOM_IDS",
        &cfg.connectors.matrix.selected_room_ids,
    );
    cfg.connectors.matrix.allowed_direct_user_ids = getenv_csv(
        "KURA_CONNECTORS_MATRIX_ALLOWED_DIRECT_USER_IDS",
        &cfg.connectors.matrix.allowed_direct_user_ids,
    );
    cfg.connectors.matrix.configured_commands = getenv_csv(
        "KURA_CONNECTORS_MATRIX_CONFIGURED_COMMANDS",
        &cfg.connectors.matrix.configured_commands,
    );
}

/// Resolve `*Env` secret references: when the material field is empty and the
/// env-ref name is set, read the named environment variable (Go
/// `resolveSecretRefs`). Missing variables resolve to empty strings.
pub(crate) fn resolve_secret_refs(cfg: &mut Config) {
    if cfg.llm.openai_compatible.api_key.is_empty() && !cfg.llm.openai_compatible.api_key_env.is_empty() {
        cfg.llm.openai_compatible.api_key =
            std::env::var(&cfg.llm.openai_compatible.api_key_env).unwrap_or_default();
    }
    if cfg.connectors.discord.bot_token.is_empty() && !cfg.connectors.discord.bot_token_env.is_empty() {
        cfg.connectors.discord.bot_token =
            std::env::var(&cfg.connectors.discord.bot_token_env).unwrap_or_default();
    }
    if cfg.connectors.telegram.bot_token.is_empty() && !cfg.connectors.telegram.bot_token_env.is_empty() {
        cfg.connectors.telegram.bot_token =
            std::env::var(&cfg.connectors.telegram.bot_token_env).unwrap_or_default();
    }
    if cfg.connectors.slack.oauth_client_secret.is_empty()
        && !cfg.connectors.slack.oauth_client_secret_env.is_empty()
    {
        cfg.connectors.slack.oauth_client_secret =
            std::env::var(&cfg.connectors.slack.oauth_client_secret_env).unwrap_or_default();
    }
    if cfg.connectors.matrix.bot_access_token.is_empty()
        && !cfg.connectors.matrix.bot_access_token_env.is_empty()
    {
        cfg.connectors.matrix.bot_access_token =
            std::env::var(&cfg.connectors.matrix.bot_access_token_env).unwrap_or_default();
    }
}

/// Go `getenv`: unset or empty env var falls back.
pub(crate) fn getenv(key: &str, fallback: &str) -> String {
    match std::env::var(key) {
        Ok(value) if !value.is_empty() => value,
        _ => fallback.to_string(),
    }
}

/// Go `getenvInt`: unset, empty, or unparsable values fall back.
fn getenv_int(key: &str, fallback: i64) -> i64 {
    match std::env::var(key) {
        Ok(value) if !value.is_empty() => value.parse::<i64>().unwrap_or(fallback),
        _ => fallback,
    }
}

/// Go `getenvBool`: value is trimmed, then parsed with `strconv.ParseBool`
/// semantics (`1/t/T/TRUE/true/True` and `0/f/F/FALSE/false/False`); anything
/// else falls back.
fn getenv_bool(key: &str, fallback: bool) -> bool {
    match std::env::var(key) {
        Ok(value) => match value.trim() {
            "" => fallback,
            "1" | "t" | "T" | "TRUE" | "true" | "True" => true,
            "0" | "f" | "F" | "FALSE" | "false" | "False" => false,
            _ => fallback,
        },
        Err(_) => fallback,
    }
}

/// Go `getenvCSV`: unset/blank falls back; otherwise split on `,`, trim each
/// item, and drop empty items (possibly yielding an empty list).
fn getenv_csv(key: &str, fallback: &[String]) -> Vec<String> {
    let value = match std::env::var(key) {
        Ok(value) => value,
        Err(_) => return fallback.to_vec(),
    };
    let value = value.trim();
    if value.is_empty() {
        return fallback.to_vec();
    }
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

/// Parse a `provider[:model]` role override. A bare provider leaves the model
/// empty so the provider's own default applies.
fn parse_role_binding(raw: &str) -> ModelRoleBinding {
    match raw.split_once(':') {
        Some((provider, model)) => ModelRoleBinding {
            provider: provider.trim().to_string(),
            model: model.trim().to_string(),
        },
        None => ModelRoleBinding {
            provider: raw.trim().to_string(),
            model: String::new(),
        },
    }
}

/// Read an optional float override. An unset or blank value keeps `current`;
/// an unparsable value is ignored rather than silently becoming a default,
/// matching how `getenv_int` treats malformed input.
fn getenv_opt_f64(key: &str, current: Option<f64>) -> Option<f64> {
    match std::env::var(key) {
        Ok(raw) if !raw.trim().is_empty() => raw.trim().parse::<f64>().ok().or(current),
        _ => current,
    }
}
