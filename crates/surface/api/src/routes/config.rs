//! config route family (port of the /v1/config handler and
//! buildConfigResponse in Go daemon/internal/api).
//!
//! Route: GET /v1/config — the redacted configuration inspection projection:
//! effective environment, LLM provider settings (secrets replaced by
//! `configured` booleans and a redactedFields list), connector projections
//! with hosted-readiness, the MCP server/catalog/transport inventory, and the
//! sandbox backend capability profiles.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use kura_config as config;
use kura_mcp as mcp;
use kura_sandbox as sandbox;

use crate::error::ApiError;
use crate::state::AppState;

/// Route family router.
#[must_use]
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/config", get(get_config))
        .route("/v1/config/file", get(get_config_file).put(put_config_file))
}

// ---------------------------------------------------------------------------
// Response DTOs (Go ConfigResponse and friends; json tags preserved)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfigResponse {
    environment: String,
    bind_addr: String,
    data_dir: String,
    config_file_path: String,
    log_level: String,
    version: String,
    llm: ConfigLlmResponse,
    connectors: ConfigConnectorsResponse,
    mcp: ConfigMcpResponse,
    sandbox: ConfigSandboxResponse,
    redacted_fields: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfigLlmResponse {
    default_provider: String,
    default_model: String,
    default_timeout_ms: i64,
    default_max_retries: i64,
    #[serde(rename = "openaiCompatible")]
    openai_compatible: ConfigOpenAiCompatibleProviderResponse,
    claude: ConfigManagedCliProviderResponse,
    codex: ConfigManagedCliProviderResponse,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfigOpenAiCompatibleProviderResponse {
    configured: bool,
    #[serde(rename = "baseURL")]
    base_url: String,
    model: String,
    timeout_ms: i64,
    stream_first_chunk_timeout_ms: i64,
    stream_idle_timeout_ms: i64,
    stream_max_duration_ms: i64,
    api_key_configured: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    api_key_env: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfigManagedCliProviderResponse {
    configured: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    cli_path: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    default_model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    work_dir: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sandbox: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfigConnectorsResponse {
    discord: ConfigDiscordConnectorResponse,
    telegram: ConfigTelegramConnectorResponse,
    slack: ConfigSlackConnectorResponse,
    matrix: ConfigMatrixConnectorResponse,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfigMcpResponse {
    servers: Vec<mcp::ServerResource>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    catalog: Vec<mcp::CatalogEntry>,
    transports: Vec<mcp::TransportCapability>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfigSandboxResponse {
    backends: Vec<sandbox::BackendCapabilityProfile>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfigDiscordConnectorResponse {
    enabled: bool,
    configured: bool,
    connector_id: String,
    display_name: String,
    delivery_mode: String,
    require_mention: bool,
    #[serde(rename = "respondInDM")]
    respond_in_dm: bool,
    allowed_guild_ids: Vec<String>,
    allowed_channel_ids: Vec<String>,
    bot_token_configured: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    bot_token_env: String,
    hosted_readiness: config::DiscordHostedReadinessProjection,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfigTelegramConnectorResponse {
    enabled: bool,
    configured: bool,
    connector_id: String,
    display_name: String,
    bot_token_configured: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    bot_token_env: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    bot_username: String,
    allowed_user_ids: Vec<String>,
    allowed_direct_chat_ids: Vec<String>,
    allowed_group_ids: Vec<String>,
    hosted_readiness: config::TelegramHostedReadinessProjection,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfigSlackConnectorResponse {
    enabled: bool,
    configured: bool,
    connector_id: String,
    display_name: String,
    #[serde(rename = "apiBaseURL", skip_serializing_if = "String::is_empty")]
    api_base_url: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    bot_token_secret_ref: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    workspace_binding_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    workspace_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    bot_user_id: String,
    allowed_channel_ids: Vec<String>,
    #[serde(rename = "allowedDMUserIds")]
    allowed_dm_user_ids: Vec<String>,
    #[serde(rename = "allowedDMUserGroups")]
    allowed_dm_user_groups: Vec<String>,
    hosted_readiness: config::SlackHostedReadinessProjection,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfigMatrixConnectorResponse {
    enabled: bool,
    configured: bool,
    connector_id: String,
    display_name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    homeserver_url: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    homeserver_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    bot_user_id: String,
    bot_access_token_set: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    bot_access_token_env: String,
    selected_room_ids: Vec<String>,
    allowed_direct_user_ids: Vec<String>,
    configured_commands: Vec<String>,
    hosted_readiness: config::MatrixHostedReadinessProjection,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// GET /v1/config (Go /v1/config handler + buildConfigResponse).
async fn get_config(State(state): State<AppState>) -> Json<ConfigResponse> {
    Json(build_config_response(&state))
}

fn build_config_response(state: &AppState) -> ConfigResponse {
    let cfg = &state.config;
    let mut redacted_fields = Vec::new();
    if !cfg.llm.openai_compatible.api_key.is_empty() {
        redacted_fields.push("llm.openaiCompatible.apiKey".to_string());
    }
    if !cfg.connectors.discord.bot_token.is_empty() {
        redacted_fields.push("connectors.discord.botToken".to_string());
    }
    if !cfg.connectors.telegram.bot_token.is_empty() {
        redacted_fields.push("connectors.telegram.botToken".to_string());
    }
    if !cfg.connectors.slack.oauth_client_secret.is_empty() {
        redacted_fields.push("connectors.slack.oauthClientSecret".to_string());
    }
    if !cfg.connectors.matrix.bot_access_token.is_empty() {
        redacted_fields.push("connectors.matrix.botAccessToken".to_string());
    }

    let default_timeout_ms = if cfg.llm.default_timeout_ms > 0 {
        cfg.llm.default_timeout_ms
    } else {
        30000
    };
    let openai_timeout_ms = if cfg.llm.openai_compatible.timeout_ms > 0 {
        cfg.llm.openai_compatible.timeout_ms
    } else {
        default_timeout_ms
    };
    let first_chunk_timeout_ms = if cfg.llm.openai_compatible.stream_first_chunk_timeout_ms > 0 {
        cfg.llm.openai_compatible.stream_first_chunk_timeout_ms
    } else {
        openai_timeout_ms
    };
    let idle_timeout_ms = if cfg.llm.openai_compatible.stream_idle_timeout_ms > 0 {
        cfg.llm.openai_compatible.stream_idle_timeout_ms
    } else {
        first_chunk_timeout_ms
    };
    let discord_delivery_mode = if cfg.connectors.discord.delivery_mode.is_empty() {
        "gateway".to_string()
    } else {
        cfg.connectors.discord.delivery_mode.clone()
    };

    let openai = &cfg.llm.openai_compatible;
    let discord = &cfg.connectors.discord;
    let telegram = &cfg.connectors.telegram;
    let slack = &cfg.connectors.slack;
    let matrix = &cfg.connectors.matrix;

    ConfigResponse {
        environment: match cfg.environment {
            config::Environment::Prod => "prod".to_string(),
            config::Environment::Test => "test".to_string(),
        },
        bind_addr: cfg.bind_addr.clone(),
        data_dir: cfg.data_dir.clone(),
        config_file_path: config::config_file_path(&cfg.data_dir)
            .to_string_lossy()
            .to_string(),
        log_level: cfg.log_level.clone(),
        version: cfg.version.clone(),
        llm: ConfigLlmResponse {
            default_provider: cfg.llm.default_provider.clone(),
            default_model: cfg.llm.default_model.clone(),
            default_timeout_ms,
            default_max_retries: cfg.llm.default_max_retries,
            openai_compatible: ConfigOpenAiCompatibleProviderResponse {
                configured: !openai.base_url.is_empty()
                    || !openai.api_key.is_empty()
                    || !openai.model.is_empty(),
                base_url: openai.base_url.clone(),
                model: openai.model.clone(),
                timeout_ms: openai_timeout_ms,
                stream_first_chunk_timeout_ms: first_chunk_timeout_ms,
                stream_idle_timeout_ms: idle_timeout_ms,
                stream_max_duration_ms: openai.stream_max_duration_ms,
                api_key_configured: !openai.api_key.is_empty(),
                api_key_env: openai.api_key_env.clone(),
            },
            claude: managed_cli_response("claude_managed", &cfg.llm.claude),
            codex: managed_cli_response("codex_managed", &cfg.llm.codex),
        },
        connectors: ConfigConnectorsResponse {
            discord: ConfigDiscordConnectorResponse {
                enabled: discord.enabled,
                configured: !discord.bot_token.is_empty(),
                connector_id: discord.connector_id.clone(),
                display_name: discord.display_name.clone(),
                delivery_mode: discord_delivery_mode,
                require_mention: discord.require_mention,
                respond_in_dm: discord.respond_in_dm,
                allowed_guild_ids: discord.allowed_guild_ids.clone(),
                allowed_channel_ids: discord.allowed_channel_ids.clone(),
                bot_token_configured: !discord.bot_token.is_empty(),
                bot_token_env: discord.bot_token_env.clone(),
                hosted_readiness: discord.project_hosted_readiness(""),
            },
            telegram: ConfigTelegramConnectorResponse {
                enabled: telegram.enabled,
                configured: !telegram.bot_token.is_empty(),
                connector_id: telegram.connector_id.clone(),
                display_name: telegram.display_name.clone(),
                bot_token_configured: !telegram.bot_token.is_empty(),
                bot_token_env: telegram.bot_token_env.clone(),
                bot_username: telegram.bot_username.clone(),
                allowed_user_ids: telegram.allowed_user_ids.clone(),
                allowed_direct_chat_ids: telegram.allowed_direct_chat_ids.clone(),
                allowed_group_ids: telegram.allowed_group_ids.clone(),
                hosted_readiness: telegram.project_hosted_readiness(""),
            },
            slack: ConfigSlackConnectorResponse {
                enabled: slack.enabled,
                configured: !slack.workspace_id.is_empty()
                    || !slack.allowed_channel_ids.is_empty()
                    || !slack.allowed_dm_user_ids.is_empty()
                    || !slack.allowed_dm_user_groups.is_empty(),
                connector_id: slack.connector_id.clone(),
                display_name: slack.display_name.clone(),
                api_base_url: slack.api_base_url.clone(),
                bot_token_secret_ref: slack.bot_token_secret_ref.clone(),
                workspace_binding_id: slack.workspace_binding_id.clone(),
                workspace_id: slack.workspace_id.clone(),
                bot_user_id: slack.bot_user_id.clone(),
                allowed_channel_ids: slack.allowed_channel_ids.clone(),
                allowed_dm_user_ids: slack.allowed_dm_user_ids.clone(),
                allowed_dm_user_groups: slack.allowed_dm_user_groups.clone(),
                hosted_readiness: slack.project_hosted_readiness(""),
            },
            matrix: ConfigMatrixConnectorResponse {
                enabled: matrix.enabled,
                configured: !matrix.homeserver_url.is_empty()
                    || !matrix.bot_access_token.is_empty()
                    || !matrix.selected_room_ids.is_empty()
                    || !matrix.allowed_direct_user_ids.is_empty(),
                connector_id: matrix.connector_id.clone(),
                display_name: matrix.display_name.clone(),
                homeserver_url: matrix.homeserver_url.clone(),
                homeserver_id: matrix.homeserver_id.clone(),
                bot_user_id: matrix.bot_user_id.clone(),
                bot_access_token_set: !matrix.bot_access_token.is_empty(),
                bot_access_token_env: matrix.bot_access_token_env.clone(),
                selected_room_ids: matrix.selected_room_ids.clone(),
                allowed_direct_user_ids: matrix.allowed_direct_user_ids.clone(),
                configured_commands: matrix.configured_commands.clone(),
                hosted_readiness: matrix.project_hosted_readiness(""),
            },
        },
        mcp: ConfigMcpResponse {
            servers: state
                .mcp
                .as_deref()
                .map(mcp::Manager::list_servers)
                .unwrap_or_default(),
            catalog: state
                .mcp
                .as_deref()
                .map(mcp::Manager::list_catalog)
                .unwrap_or_default(),
            transports: state
                .mcp
                .as_deref()
                .map(mcp::Manager::list_transport_capabilities)
                .unwrap_or_default(),
        },
        sandbox: ConfigSandboxResponse {
            backends: state
                .sandboxes
                .as_deref()
                .map(sandbox::Manager::backend_capabilities)
                .unwrap_or_default(),
        },
        redacted_fields,
    }
}

/// Go buildManagedProviderConfigSandbox: the declaration-only sandbox
/// contract view a managed CLI provider would run under for config
/// inspection.
fn managed_provider_config_sandbox(provider_id: &str, work_dir: &str) -> Option<serde_json::Value> {
    let work_dir = work_dir.trim();
    let read_roots: Vec<String> = if work_dir.is_empty() {
        Vec::new()
    } else {
        vec![work_dir.to_string()]
    };
    let view = sandbox::ConsumerContractView {
        declaration: Some(sandbox::ConsumerRequirementDeclaration {
            declaration_id: format!("managed_provider:{}:config", provider_id.trim()),
            consumer_kind: sandbox::ConsumerKind::ManagedProvider,
            consumer_id: provider_id.trim().to_string(),
            operation_kind: "config_inspect".to_string(),
            profile_id: sandbox::PROFILE_ID_SUBPROCESS_DEFAULT.to_string(),
            execution_mode: sandbox::ExecutionMode::DeclarationOnly,
            allowed_backend_kinds: vec![sandbox::BackendKind::Subprocess],
            read_roots,
            write_roots: Vec::new(),
            network_mode: Some(sandbox::NetworkMode::Deny),
            secret_refs: Vec::new(),
            approval_mode: Some(sandbox::ApprovalMode::Allow),
            required_enforcement_strength: "declared_only".to_string(),
            active: true,
            source: sandbox::Source::Builtin,
            ..sandbox::ConsumerRequirementDeclaration::default()
        }),
        ..sandbox::ConsumerContractView::default()
    };
    serde_json::to_value(&view).ok()
}

fn managed_cli_response(
    provider_id: &str,
    provider: &config::ManagedCliProviderConfig,
) -> ConfigManagedCliProviderResponse {
    ConfigManagedCliProviderResponse {
        configured: !provider.cli_path.is_empty()
            || !provider.default_model.is_empty()
            || (!provider.work_dir.is_empty() && provider.work_dir != "~"),
        cli_path: provider.cli_path.clone(),
        default_model: provider.default_model.clone(),
        work_dir: provider.work_dir.clone(),
        sandbox: managed_provider_config_sandbox(provider_id, &provider.work_dir),
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests_support::{request_json, test_state};
    use axum::http::StatusCode;

    #[tokio::test]
    async fn config_projection_reports_environment_and_redactions() {
        let mut state = test_state();
        state.config.llm.openai_compatible.api_key = "sk-secret".to_string();
        state.config.llm.openai_compatible.api_key_env = "OPENAI_API_KEY".to_string();

        let (status, body) = request_json(state, "GET", "/v1/config", None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["environment"], "test");
        assert_eq!(body["bindAddr"], "127.0.0.1:19192");
        assert_eq!(body["llm"]["openaiCompatible"]["apiKeyConfigured"], true);
        assert!(
            body["llm"]["openaiCompatible"]["apiKey"].is_null(),
            "{body}"
        );
        assert!(
            body["redactedFields"]
                .as_array()
                .expect("redactedFields")
                .iter()
                .any(|field| field == "llm.openaiCompatible.apiKey")
        );
        assert_eq!(body["llm"]["defaultTimeoutMs"], 30000);
        assert_eq!(body["connectors"]["discord"]["deliveryMode"], "gateway");
        assert!(
            body["mcp"]["servers"]
                .as_array()
                .expect("servers")
                .is_empty()
        );
        assert!(
            body["sandbox"]["backends"]
                .as_array()
                .expect("backends")
                .is_empty()
        );
    }
}

// ---------------------------------------------------------------------------
// Config file write path (Stage 7.1)
// ---------------------------------------------------------------------------
//
// `/v1/config` above is the *effective* configuration: defaults + file + env
// overrides + resolved secrets, redacted. It is deliberately not writable —
// writing an effective config back to disk would persist env overrides and
// resolved secret values into `config.json`.
//
// `/v1/config/file` is the **file** itself: the operator-owned document that
// `kura_config::load` merges over the defaults. It carries indirections
// (`apiKeyEnv`, secret refs), never values, and the write path refuses inline
// secrets so the daemon can never be the thing that puts a key on disk.
//
// **Hot-apply boundary, published rather than implied:** nothing hot-applies.
// Every accepted write reports `restartRequired: true` and names the top-level
// sections that changed. A partial hot-apply with an undocumented boundary is
// worse than a full restart; this is the full restart, said out loud, until a
// reload path exists and can name what it covers.

/// Settings that take effect without a restart. Empty today — the constant
/// exists so the answer is a published list rather than folklore, and so the
/// response can carry it.
pub const HOT_APPLY_SETTINGS: &[&str] = &[];

const REDACTED: &str = "[REDACTED]";

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfigFileResponse {
    path: String,
    exists: bool,
    /// The file's JSON with inline secret values redacted. Absent values and
    /// indirections are shown as written.
    config: serde_json::Value,
    /// SHA-256 of the raw bytes on disk. Send it back as `expectSha256` to
    /// write compare-and-set.
    sha256: String,
    hot_apply_settings: Vec<String>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct ConfigFileWriteRequest {
    config: serde_json::Value,
    /// Compare-and-set: refuse unless the file on disk still hashes to this.
    expect_sha256: Option<String>,
    /// Validate and report without writing.
    dry_run: bool,
    /// Refuse keys the file format does not know. Off by default because
    /// `kura_config::load` ignores them; on, a typo is an error instead of a
    /// silent no-op.
    strict: bool,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfigFileWriteResponse {
    applied: bool,
    dry_run: bool,
    sha256: String,
    /// Always true today; see `HOT_APPLY_SETTINGS`.
    restart_required: bool,
    hot_applied: Vec<String>,
    /// Top-level sections whose content changed.
    restart_required_for: Vec<String>,
    /// Unknown keys in non-strict mode: accepted, ignored on load, reported.
    warnings: Vec<String>,
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn read_config_file(state: &AppState) -> Result<(std::path::PathBuf, Option<Vec<u8>>), ApiError> {
    let path = kura_config::config_file_path(&state.config.data_dir);
    match std::fs::read(&path) {
        Ok(bytes) => Ok((path, Some(bytes))),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok((path, None)),
        Err(err) => Err(ApiError::internal(&format!(
            "read {}: {err}",
            path.display()
        ))),
    }
}

fn redact_inline_secrets(value: &mut serde_json::Value) {
    for path in kura_config::INLINE_SECRET_PATHS {
        // A dotted config path is a JSON pointer with the separator swapped;
        // `pointer_mut` walks it without a hand-rolled borrow dance.
        let pointer = format!("/{}", path.replace('.', "/"));
        if let Some(slot) = value.pointer_mut(&pointer) {
            if slot.as_str().is_some_and(|s| !s.trim().is_empty()) {
                *slot = serde_json::Value::String(REDACTED.to_string());
            }
        }
    }
}

/// GET /v1/config/file — the operator-owned `config.json`, redacted, with
/// the hash a compare-and-set write must present.
async fn get_config_file(
    State(state): State<AppState>,
) -> Result<Json<ConfigFileResponse>, ApiError> {
    let (path, bytes) = read_config_file(&state)?;
    let (exists, mut config, sha256) = match bytes {
        Some(bytes) => {
            let parsed: serde_json::Value = serde_json::from_slice(&bytes).map_err(|err| {
                ApiError::internal(&format!("{} is not valid JSON: {err}", path.display()))
            })?;
            (true, parsed, sha256_hex(&bytes))
        }
        None => (false, serde_json::json!({}), sha256_hex(b"")),
    };
    redact_inline_secrets(&mut config);
    Ok(Json(ConfigFileResponse {
        path: path.to_string_lossy().into_owned(),
        exists,
        config,
        sha256,
        hot_apply_settings: HOT_APPLY_SETTINGS
            .iter()
            .map(|s| (*s).to_string())
            .collect(),
    }))
}

/// PUT /v1/config/file — validate, compare-and-set, write atomically.
///
/// Order matters: validation and the inline-secret refusal run **before** the
/// CAS check, so a caller learns their document is wrong without first having
/// to win the race for the file.
async fn put_config_file(
    State(state): State<AppState>,
    tenant: Option<axum::extract::Extension<crate::middleware::TenantContext>>,
    body: axum::body::Bytes,
) -> Result<Json<ConfigFileWriteResponse>, ApiError> {
    crate::middleware::require_daemon_global_operator(
        tenant.as_ref().map(|e| &e.0),
        "PUT /v1/config/file",
    )?;
    let request: ConfigFileWriteRequest = super::decode_json_required(&body)?;
    if !request.config.is_object() {
        return Err(ApiError::BadRequest(
            "config must be a JSON object".to_string(),
        ));
    }
    let candidate = serde_json::to_vec_pretty(&request.config)
        .map_err(|err| ApiError::internal(&format!("encode config: {err}")))?;

    let validation = kura_config::validate_file_config_json(&candidate)
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    if !validation.inline_secrets.is_empty() {
        return Err(ApiError::BadRequest(format!(
            "config.json must not carry inline secret values at {}; use the *Env indirection or a secret reference",
            validation.inline_secrets.join(", ")
        )));
    }
    if request.strict && !validation.unknown_keys.is_empty() {
        return Err(ApiError::BadRequest(format!(
            "unknown keys (strict): {}",
            validation.unknown_keys.join(", ")
        )));
    }

    let (path, current) = read_config_file(&state)?;
    let current_sha = current
        .as_deref()
        .map_or_else(|| sha256_hex(b""), sha256_hex);
    if let Some(expected) = request
        .expect_sha256
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if !expected.eq_ignore_ascii_case(&current_sha) {
            return Err(ApiError::Conflict(format!(
                "config.json changed since it was read (expected sha256 {expected}, current {current_sha}); re-read and retry"
            )));
        }
    }

    // Which sections changed — the restart-required list.
    let previous: serde_json::Value = current
        .as_deref()
        .and_then(|bytes| serde_json::from_slice(bytes).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let mut restart_required_for: Vec<String> = request
        .config
        .as_object()
        .into_iter()
        .flat_map(|m| m.iter())
        .filter(|(key, value)| previous.get(*key) != Some(*value))
        .map(|(key, _)| key.clone())
        .collect();
    if let Some(prev) = previous.as_object() {
        for key in prev.keys() {
            if request.config.get(key).is_none() && !restart_required_for.contains(key) {
                restart_required_for.push(key.clone());
            }
        }
    }
    restart_required_for.sort_unstable();

    let warnings: Vec<String> = validation
        .unknown_keys
        .iter()
        .map(|k| format!("unknown key `{k}` is ignored on load"))
        .collect();

    let new_sha = sha256_hex(&candidate);
    if request.dry_run {
        return Ok(Json(ConfigFileWriteResponse {
            applied: false,
            dry_run: true,
            sha256: new_sha,
            restart_required: !restart_required_for.is_empty(),
            hot_applied: Vec::new(),
            restart_required_for,
            warnings,
        }));
    }

    // Atomic replace, the same shape `kura config set` uses: a crash mid-write
    // must never leave a truncated file that then fails the next boot.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| ApiError::internal(&format!("create {}: {err}", parent.display())))?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &candidate)
        .map_err(|err| ApiError::internal(&format!("write {}: {err}", tmp.display())))?;
    std::fs::rename(&tmp, &path)
        .map_err(|err| ApiError::internal(&format!("replace {}: {err}", path.display())))?;

    Ok(Json(ConfigFileWriteResponse {
        applied: true,
        dry_run: false,
        sha256: new_sha,
        restart_required: !restart_required_for.is_empty(),
        hot_applied: Vec::new(),
        restart_required_for,
        warnings,
    }))
}

#[cfg(test)]
mod file_write_tests {
    use super::super::tests_support::{request_json, request_json_as_role, test_state};
    use axum::http::StatusCode;

    fn state_with_own_data_dir() -> crate::state::AppState {
        let mut state = test_state();
        let dir = std::env::temp_dir().join(format!("kura-config-file-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        state.config.data_dir = dir.to_string_lossy().into_owned();
        state
    }

    fn body(config: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "config": config })
    }

    #[tokio::test]
    async fn write_then_read_round_trips_with_a_cas_hash() {
        let state = state_with_own_data_dir();

        let (status, before) = request_json(state.clone(), "GET", "/v1/config/file", None).await;
        assert_eq!(status, StatusCode::OK, "{before}");
        assert_eq!(before["exists"], false);
        let empty_sha = before["sha256"].as_str().expect("sha").to_string();
        // The boundary is published, not implied.
        assert!(
            before["hotApplySettings"]
                .as_array()
                .expect("list")
                .is_empty()
        );

        let (status, written) = request_json(
            state.clone(),
            "PUT",
            "/v1/config/file",
            Some(serde_json::json!({
                "config": { "logLevel": "debug", "llm": { "defaultModel": "m1" } },
                "expectSha256": empty_sha
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{written}");
        assert_eq!(written["applied"], true);
        assert_eq!(written["restartRequired"], true);
        assert_eq!(written["hotApplied"].as_array().expect("list").len(), 0);
        let changed: Vec<&str> = written["restartRequiredFor"]
            .as_array()
            .expect("list")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(changed, ["llm", "logLevel"]);
        let sha = written["sha256"].as_str().expect("sha").to_string();

        let (status, after) = request_json(state.clone(), "GET", "/v1/config/file", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(after["exists"], true);
        assert_eq!(after["sha256"], sha.as_str());
        assert_eq!(after["config"]["logLevel"], "debug");

        // The file on disk is what `kura_config::load` would read.
        let on_disk =
            std::fs::read(kura_config::config_file_path(&state.config.data_dir)).expect("file");
        let parsed: serde_json::Value = serde_json::from_slice(&on_disk).expect("json");
        assert_eq!(parsed["llm"]["defaultModel"], "m1");
    }

    /// Compare-and-set: a stale hash is refused with 409 and the current hash,
    /// so two operators cannot silently overwrite each other.
    #[tokio::test]
    async fn a_stale_hash_is_a_409_and_nothing_is_written() {
        let state = state_with_own_data_dir();
        let (_, first) = request_json(
            state.clone(),
            "PUT",
            "/v1/config/file",
            Some(body(serde_json::json!({ "logLevel": "info" }))),
        )
        .await;
        let live_sha = first["sha256"].as_str().expect("sha").to_string();

        let (status, refused) = request_json(
            state.clone(),
            "PUT",
            "/v1/config/file",
            Some(serde_json::json!({
                "config": { "logLevel": "warn" },
                "expectSha256": "0000000000000000000000000000000000000000000000000000000000000000"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{refused}");
        assert!(
            refused.to_string().contains(&live_sha),
            "the current hash is reported: {refused}"
        );

        let (_, current) = request_json(state.clone(), "GET", "/v1/config/file", None).await;
        assert_eq!(
            current["config"]["logLevel"], "info",
            "the stale write did not land"
        );
    }

    /// The daemon must never be the thing that puts a key on disk.
    #[tokio::test]
    async fn inline_secret_values_are_refused() {
        let state = state_with_own_data_dir();
        let (status, refused) = request_json(
            state.clone(),
            "PUT",
            "/v1/config/file",
            Some(body(serde_json::json!({
                "llm": { "openaiCompatible": { "apiKey": "sk-live-leaked" } }
            }))),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
        assert!(refused.to_string().contains("llm.openaiCompatible.apiKey"));
        assert!(
            !kura_config::config_file_path(&state.config.data_dir).exists(),
            "nothing was written"
        );

        // The indirection is the supported form.
        let (status, _) = request_json(
            state.clone(),
            "PUT",
            "/v1/config/file",
            Some(body(serde_json::json!({
                "llm": { "openaiCompatible": { "apiKeyEnv": "OPENAI_API_KEY" } }
            }))),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    /// A hand-edited file carrying an inline secret is still read — but never
    /// echoed.
    #[tokio::test]
    async fn a_hand_written_inline_secret_is_redacted_on_read() {
        let state = state_with_own_data_dir();
        let path = kura_config::config_file_path(&state.config.data_dir);
        std::fs::write(
            &path,
            serde_json::json!({ "connectors": { "discord": { "botToken": "hand-edited-token" } } })
                .to_string(),
        )
        .expect("write");

        let (status, read) = request_json(state.clone(), "GET", "/v1/config/file", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            read["config"]["connectors"]["discord"]["botToken"],
            "[REDACTED]"
        );
        assert!(!read.to_string().contains("hand-edited-token"));
    }

    /// Unknown keys: warned by default (load ignores them), refused in strict
    /// mode (a typo is an error instead of a silent no-op).
    #[tokio::test]
    async fn unknown_keys_warn_by_default_and_fail_strict() {
        let state = state_with_own_data_dir();
        let doc = serde_json::json!({ "logLevel": "info", "llm": { "defaultModle": "typo" } });

        let (status, lenient) = request_json(
            state.clone(),
            "PUT",
            "/v1/config/file",
            Some(serde_json::json!({ "config": doc, "dryRun": true })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{lenient}");
        assert!(
            lenient.to_string().contains("llm.defaultModle"),
            "{lenient}"
        );

        let (status, strict) = request_json(
            state.clone(),
            "PUT",
            "/v1/config/file",
            Some(serde_json::json!({ "config": doc, "strict": true })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{strict}");
    }

    #[tokio::test]
    async fn dry_run_reports_without_writing() {
        let state = state_with_own_data_dir();
        let (status, report) = request_json(
            state.clone(),
            "PUT",
            "/v1/config/file",
            Some(serde_json::json!({ "config": { "logLevel": "debug" }, "dryRun": true })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{report}");
        assert_eq!(report["applied"], false);
        assert_eq!(report["dryRun"], true);
        assert!(!kura_config::config_file_path(&state.config.data_dir).exists());
    }

    /// Writing the daemon's configuration is a daemon-global mutation.
    #[tokio::test]
    async fn only_the_owner_role_may_write() {
        let state = state_with_own_data_dir();
        let (status, _) = request_json_as_role(
            state.clone(),
            "tnt_a",
            kura_identity::Role::Admin,
            "PUT",
            "/v1/config/file",
            Some(body(serde_json::json!({ "logLevel": "debug" }))),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = request_json_as_role(
            state.clone(),
            "tnt_a",
            kura_identity::Role::Owner,
            "PUT",
            "/v1/config/file",
            Some(body(serde_json::json!({ "logLevel": "debug" }))),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
}
