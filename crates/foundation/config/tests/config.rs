//! Behavioral tests ported from `daemon/internal/config/config_test.go` and
//! `discord_projection_test.go`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use kura_config::{
    Config, DiscordConnectorConfig, Environment, ModelRole, load, managed_provider_home_dir,
};
use parking_lot::Mutex;

/// Serializes every test in this binary: they all mutate process-wide env
/// vars and `HOME`.
static ENV_LOCK: Mutex<()> = Mutex::new(());

const KURA_ENV_KEYS: &[&str] = &[
    "KURA_ENV",
    "KURA_DATA_DIR",
    "KURA_BIND_ADDR",
    "KURA_LOG_LEVEL",
    "KURA_VERSION",
    "KURA_LLM_DEFAULT_PROVIDER",
    "KURA_LLM_DEFAULT_MODEL",
    "KURA_LLM_DEFAULT_TIMEOUT_MS",
    "KURA_LLM_DEFAULT_MAX_RETRIES",
    "KURA_LLM_OPENAI_COMPATIBLE_BASE_URL",
    "KURA_LLM_OPENAI_COMPATIBLE_API_KEY",
    "KURA_LLM_OPENAI_COMPATIBLE_API_KEY_ENV",
    "KURA_LLM_OPENAI_COMPATIBLE_MODEL",
    "KURA_LLM_OPENAI_COMPATIBLE_TIMEOUT_MS",
    "KURA_LLM_OPENAI_COMPATIBLE_STREAM_FIRST_CHUNK_TIMEOUT_MS",
    "KURA_LLM_OPENAI_COMPATIBLE_STREAM_IDLE_TIMEOUT_MS",
    "KURA_LLM_OPENAI_COMPATIBLE_STREAM_MAX_DURATION_MS",
    "KURA_LLM_CLAUDE_CLI_PATH",
    "KURA_LLM_CLAUDE_MODEL",
    "KURA_LLM_CLAUDE_WORKDIR",
    "KURA_LLM_CODEX_CLI_PATH",
    "KURA_LLM_CODEX_MODEL",
    "KURA_LLM_CODEX_WORKDIR",
    "KURA_CONNECTORS_DISCORD_ENABLED",
    "KURA_CONNECTORS_DISCORD_CONNECTOR_ID",
    "KURA_CONNECTORS_DISCORD_DISPLAY_NAME",
    "KURA_CONNECTORS_DISCORD_DELIVERY_MODE",
    "KURA_CONNECTORS_DISCORD_BOT_TOKEN",
    "KURA_CONNECTORS_DISCORD_BOT_TOKEN_ENV",
    "KURA_CONNECTORS_DISCORD_REQUIRE_MENTION",
    "KURA_CONNECTORS_DISCORD_RESPOND_IN_DM",
    "KURA_CONNECTORS_DISCORD_ALLOWED_GUILD_IDS",
    "KURA_CONNECTORS_DISCORD_ALLOWED_CHANNEL_IDS",
    "KURA_CONNECTORS_TELEGRAM_ENABLED",
    "KURA_CONNECTORS_TELEGRAM_CONNECTOR_ID",
    "KURA_CONNECTORS_TELEGRAM_DISPLAY_NAME",
    "KURA_CONNECTORS_TELEGRAM_BOT_TOKEN",
    "KURA_CONNECTORS_TELEGRAM_BOT_TOKEN_ENV",
    "KURA_CONNECTORS_TELEGRAM_BOT_API_BASE_URL",
    "KURA_CONNECTORS_TELEGRAM_BOT_USERNAME",
    "KURA_CONNECTORS_TELEGRAM_ALLOWED_USER_IDS",
    "KURA_CONNECTORS_TELEGRAM_ALLOWED_DIRECT_CHAT_IDS",
    "KURA_CONNECTORS_TELEGRAM_ALLOWED_GROUP_IDS",
    "KURA_CONNECTORS_SLACK_ENABLED",
    "KURA_CONNECTORS_SLACK_CONNECTOR_ID",
    "KURA_CONNECTORS_SLACK_DISPLAY_NAME",
    "KURA_CONNECTORS_SLACK_API_BASE_URL",
    "KURA_CONNECTORS_SLACK_BOT_TOKEN_SECRET_REF",
    "KURA_CONNECTORS_SLACK_OAUTH_CLIENT_ID",
    "KURA_CONNECTORS_SLACK_OAUTH_CLIENT_SECRET",
    "KURA_CONNECTORS_SLACK_OAUTH_CLIENT_SECRET_ENV",
    "KURA_CONNECTORS_SLACK_OAUTH_API_BASE_URL",
    "KURA_CONNECTORS_SLACK_WORKSPACE_BINDING_ID",
    "KURA_CONNECTORS_SLACK_WORKSPACE_ID",
    "KURA_CONNECTORS_SLACK_BOT_USER_ID",
    "KURA_CONNECTORS_SLACK_ALLOWED_CHANNEL_IDS",
    "KURA_CONNECTORS_SLACK_ALLOWED_DM_USER_IDS",
    "KURA_CONNECTORS_SLACK_ALLOWED_DM_USER_GROUPS",
    "KURA_CONNECTORS_MATRIX_ENABLED",
    "KURA_CONNECTORS_MATRIX_CONNECTOR_ID",
    "KURA_CONNECTORS_MATRIX_DISPLAY_NAME",
    "KURA_CONNECTORS_MATRIX_HOMESERVER_URL",
    "KURA_CONNECTORS_MATRIX_HOMESERVER_ID",
    "KURA_CONNECTORS_MATRIX_BOT_USER_ID",
    "KURA_CONNECTORS_MATRIX_BOT_ACCESS_TOKEN",
    "KURA_CONNECTORS_MATRIX_BOT_ACCESS_TOKEN_ENV",
    "KURA_CONNECTORS_MATRIX_SELECTED_ROOM_IDS",
    "KURA_CONNECTORS_MATRIX_ALLOWED_DIRECT_USER_IDS",
    "KURA_CONNECTORS_MATRIX_CONFIGURED_COMMANDS",
    "KURA_LLM_OPENAI_COMPATIBLE_TEMPERATURE",
    "KURA_LLM_OPENAI_COMPATIBLE_TOP_P",
    "KURA_LLM_ROLE_PRIMARY",
    "KURA_LLM_ROLE_VISION",
    "KURA_LLM_ROLE_IMAGE",
    "KURA_LLM_ROLE_VIDEO",
    "KURA_LLM_ROLE_EMBED",
    "OPENAI_TEST_KEY",
    "DISCORD_TEST_TOKEN",
    "TELEGRAM_TEST_TOKEN",
    "SLACK_CLIENT_SECRET",
    "MATRIX_BOT_TOKEN",
];

fn set_env(key: &str, value: &str) {
    // SAFETY: every test in this binary holds `ENV_LOCK` for its entire
    // duration, so env mutations are serialized.
    unsafe { std::env::set_var(key, value) }
}

/// Equivalent of Go `setBaseEnv`: point HOME at the temp dir and clear every
/// KURA_* knob (Go sets them to the empty string, which the loader treats as
/// unset).
fn set_base_env(home_dir: &Path) {
    set_env("HOME", &home_dir.to_string_lossy());
    for key in KURA_ENV_KEYS {
        set_env(key, "");
    }
}

/// Unique temp directory removed on drop (stand-in for Go `t.TempDir`).
struct TempHome(PathBuf);

impl TempHome {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "kura-config-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("create temp home");
        TempHome(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write_config(data_dir: &Path, contents: &str) {
    std::fs::create_dir_all(data_dir).expect("create data dir");
    std::fs::write(data_dir.join("config.json"), contents).expect("write config.json");
}

#[test]
fn load_initializes_default_kura_dir() {
    let _guard = ENV_LOCK.lock();
    let home = TempHome::new();
    set_base_env(home.path());

    let cfg = load().expect("load");

    let expected_data_dir = home.path().join(".kura-test");
    assert_eq!(cfg.data_dir, expected_data_dir.to_string_lossy());
    assert_eq!(cfg.environment, Environment::Test);
    assert!(expected_data_dir.is_dir(), "expected data dir to exist");
    assert_eq!(cfg.llm.default_timeout_ms, 30000);
}

#[test]
fn load_reads_config_file_from_kura_dir() {
    let _guard = ENV_LOCK.lock();
    let home = TempHome::new();
    let data_dir = home.path().join(".kura-test");
    write_config(
        &data_dir,
        r#"{
        "bindAddr": "127.0.0.1:19000",
        "logLevel": "debug",
        "llm": {
            "defaultProvider": "openai_compatible",
            "defaultModel": "gpt-test",
            "defaultTimeoutMs": 45000,
            "openaiCompatible": {
                "baseURL": "https://api.example.com/v1",
                "apiKeyEnv": "OPENAI_TEST_KEY",
                "model": "gpt-provider"
            }
        }
    }"#,
    );

    set_base_env(home.path());
    set_env("OPENAI_TEST_KEY", "secret-from-env");

    let cfg = load().expect("load");

    assert_eq!(cfg.bind_addr, "127.0.0.1:19000");
    assert_eq!(cfg.log_level, "debug");
    assert_eq!(cfg.llm.default_provider, "openai_compatible");
    assert_eq!(cfg.llm.default_model, "gpt-test");
    assert_eq!(cfg.llm.default_timeout_ms, 45000);
    assert_eq!(cfg.llm.openai_compatible.base_url, "https://api.example.com/v1");
    assert_eq!(cfg.llm.openai_compatible.api_key, "secret-from-env");
}

#[test]
fn managed_provider_home_dir_uses_isolated_test_root() {
    let cfg = Config {
        project_root: String::new(),
        environment: Environment::Test,
        data_dir: "/tmp/kura-test".to_string(),
        bind_addr: String::new(),
        log_level: String::new(),
        version: String::new(),
        llm: Default::default(),
        connectors: Default::default(),
    };
    let expected = Path::new("/tmp/kura-test").join("managed-provider-home");
    assert_eq!(managed_provider_home_dir(&cfg), expected.to_string_lossy());
}

#[test]
fn managed_provider_home_dir_isolates_embedded_like_test() {
    // A host runs one embedded daemon per workspace, so managed CLI
    // credentials must stay inside the data dir rather than the user's home.
    let cfg = Config {
        project_root: String::new(),
        environment: Environment::Embedded,
        data_dir: "/tmp/dope-embedded".to_string(),
        bind_addr: String::new(),
        log_level: String::new(),
        version: String::new(),
        llm: Default::default(),
        connectors: Default::default(),
    };
    let expected = Path::new("/tmp/dope-embedded").join("managed-provider-home");
    assert_eq!(managed_provider_home_dir(&cfg), expected.to_string_lossy());
}

#[test]
fn embedded_environment_is_selected_by_dope_env() {
    let _guard = ENV_LOCK.lock();
    let home = TempHome::new();
    set_base_env(home.path());
    let data_dir = home.path().join("workspace-data");
    set_env("KURA_ENV", "embedded");
    set_env("KURA_DATA_DIR", &data_dir.to_string_lossy());

    let cfg = load().expect("embedded config loads");

    assert_eq!(cfg.environment, Environment::Embedded);
    // The host supplies the data dir; embedded must not fall back to a shared
    // prod or test root.
    assert_eq!(cfg.data_dir, data_dir.to_string_lossy());
}

#[test]
fn embedded_defaults_avoid_the_prod_and_test_daemons() {
    // Defaults only matter for a misconfigured host, but an embedded daemon
    // must never land on the shared daemons' data dir or port.
    let _guard = ENV_LOCK.lock();
    let home = TempHome::new();
    set_base_env(home.path());
    set_env("KURA_ENV", "embedded");

    let cfg = load().expect("embedded config loads");

    assert_eq!(cfg.environment, Environment::Embedded);
    assert_ne!(cfg.bind_addr, "127.0.0.1:19191");
    assert_ne!(cfg.bind_addr, "127.0.0.1:19192");
    assert!(!cfg.data_dir.ends_with(".dope"));
    assert!(!cfg.data_dir.ends_with(".dope-test"));
}

#[test]
fn model_roles_default_to_unrouted() {
    let _guard = ENV_LOCK.lock();
    let home = TempHome::new();
    set_base_env(home.path());

    let cfg = load().expect("config loads");

    // Only primary has a fallback (default_provider); the rest must stay
    // unrouted so a missing image model is visible instead of silently
    // resolving to a text model.
    for role in [
        ModelRole::Vision,
        ModelRole::Image,
        ModelRole::Video,
        ModelRole::Embed,
    ] {
        assert!(!cfg.llm.roles.get(role).is_routed(), "{role:?} should be unrouted");
    }
}

#[test]
fn model_roles_are_overridden_by_env() {
    let _guard = ENV_LOCK.lock();
    let home = TempHome::new();
    set_base_env(home.path());
    set_env("KURA_LLM_ROLE_IMAGE", "studio:sd3-medium");
    // A bare provider leaves the model empty so the provider default applies.
    set_env("KURA_LLM_ROLE_EMBED", "ollama");

    let cfg = load().expect("config loads");

    let image = cfg.llm.roles.get(ModelRole::Image);
    assert_eq!(image.provider, "studio");
    assert_eq!(image.model, "sd3-medium");
    assert!(image.is_routed());

    let embed = cfg.llm.roles.get(ModelRole::Embed);
    assert_eq!(embed.provider, "ollama");
    assert_eq!(embed.model, "");
    assert!(embed.is_routed());

    set_env("KURA_LLM_ROLE_IMAGE", "");
    set_env("KURA_LLM_ROLE_EMBED", "");
}

#[test]
fn model_role_parsing_rejects_unknown_names() {
    assert_eq!(ModelRole::parse("image"), Some(ModelRole::Image));
    assert_eq!(ModelRole::parse("  VIDEO "), Some(ModelRole::Video));
    assert_eq!(ModelRole::parse("embeddings"), Some(ModelRole::Embed));
    assert_eq!(ModelRole::parse("fast"), None);
    assert_eq!(ModelRole::parse(""), None);
}

#[test]
fn sampling_is_unset_by_default_and_readable_from_env() {
    let _guard = ENV_LOCK.lock();
    let home = TempHome::new();
    set_base_env(home.path());

    let bare = load().expect("config loads");
    // Unset must stay distinguishable from zero: some gateways reject an
    // explicit temperature on reasoning models.
    assert_eq!(bare.llm.openai_compatible.sampling.temperature, None);
    assert_eq!(bare.llm.openai_compatible.sampling.top_p, None);

    set_env("KURA_LLM_OPENAI_COMPATIBLE_TEMPERATURE", "0");
    let tuned = load().expect("config loads");
    assert_eq!(tuned.llm.openai_compatible.sampling.temperature, Some(0.0));
    set_env("KURA_LLM_OPENAI_COMPATIBLE_TEMPERATURE", "");
}

#[test]
fn load_environment_overrides_config_file() {
    let _guard = ENV_LOCK.lock();
    let home = TempHome::new();
    let data_dir = home.path().join(".kura-test");
    write_config(
        &data_dir,
        r#"{
        "bindAddr": "127.0.0.1:19000",
        "logLevel": "debug",
        "llm": {
            "defaultProvider": "openai_compatible",
            "defaultModel": "gpt-file",
            "openaiCompatible": {
                "baseURL": "https://api.file.example/v1",
                "apiKey": "file-secret",
                "model": "gpt-file-provider"
            }
        }
    }"#,
    );

    let override_dir = home.path().join("custom-kura");
    set_base_env(home.path());
    set_env("KURA_DATA_DIR", &override_dir.to_string_lossy());
    set_env("KURA_BIND_ADDR", "127.0.0.1:19999");
    set_env("KURA_LOG_LEVEL", "warn");
    set_env("KURA_VERSION", "test");
    set_env("KURA_LLM_DEFAULT_MODEL", "gpt-env");
    set_env("KURA_LLM_OPENAI_COMPATIBLE_BASE_URL", "https://api.env.example/v1");
    set_env("KURA_LLM_OPENAI_COMPATIBLE_API_KEY", "env-secret");
    set_env("KURA_LLM_OPENAI_COMPATIBLE_MODEL", "gpt-env-provider");

    let cfg = load().expect("load");

    assert_eq!(cfg.data_dir, override_dir.to_string_lossy());
    assert_eq!(cfg.bind_addr, "127.0.0.1:19999");
    assert_eq!(cfg.log_level, "warn");
    assert_eq!(cfg.version, "test");
    assert_eq!(cfg.llm.default_model, "gpt-env");
    assert_eq!(cfg.llm.openai_compatible.base_url, "https://api.env.example/v1");
    assert_eq!(cfg.llm.openai_compatible.api_key, "env-secret");
    assert!(override_dir.is_dir(), "expected overridden data dir to exist");
}

#[test]
fn load_managed_cli_provider_config() {
    let _guard = ENV_LOCK.lock();
    let home = TempHome::new();
    let data_dir = home.path().join(".kura-test");
    write_config(
        &data_dir,
        r#"{
        "llm": {
            "claude": {
                "cliPath": "/usr/local/bin/claude",
                "defaultModel": "claude-opus-4-6",
                "workDir": "~/workspaces/claude"
            },
            "codex": {
                "cliPath": "/opt/homebrew/bin/codex",
                "defaultModel": "gpt-5.4",
                "workDir": "~/workspaces/codex"
            }
        }
    }"#,
    );

    set_base_env(home.path());
    set_env("KURA_LLM_CLAUDE_MODEL", "claude-sonnet-4-6");
    set_env("KURA_LLM_CODEX_WORKDIR", "~/projects/codex");

    let cfg = load().expect("load");

    assert_eq!(cfg.llm.claude.cli_path, "/usr/local/bin/claude");
    assert_eq!(cfg.llm.claude.default_model, "claude-sonnet-4-6");
    assert_eq!(cfg.llm.codex.cli_path, "/opt/homebrew/bin/codex");
    assert_eq!(cfg.llm.codex.work_dir, "~/projects/codex");
}

#[test]
fn load_discord_connector_config() {
    let _guard = ENV_LOCK.lock();
    let home = TempHome::new();
    let data_dir = home.path().join(".kura-test");
    write_config(
        &data_dir,
        r#"{
        "connectors": {
            "discord": {
                "enabled": true,
                "connectorId": "discord-bot",
                "displayName": "Discord Bot",
                "deliveryMode": "gateway",
                "botTokenEnv": "DISCORD_TEST_TOKEN",
                "requireMention": false,
                "respondInDM": true,
                "allowedGuildIds": ["guild_1"],
                "allowedChannelIds": ["channel_1", "channel_2"]
            }
        }
    }"#,
    );

    set_base_env(home.path());
    set_env("DISCORD_TEST_TOKEN", "discord-secret");
    set_env("KURA_CONNECTORS_DISCORD_ALLOWED_CHANNEL_IDS", "channel_3,channel_4");

    let cfg = load().expect("load");

    assert!(cfg.connectors.discord.enabled);
    assert_eq!(cfg.connectors.discord.connector_id, "discord-bot");
    assert_eq!(cfg.connectors.discord.bot_token, "discord-secret");
    assert_eq!(cfg.connectors.discord.delivery_mode, "gateway");
    assert!(!cfg.connectors.discord.require_mention);
    assert_eq!(cfg.connectors.discord.allowed_guild_ids, ["guild_1"]);
    assert_eq!(
        cfg.connectors.discord.allowed_channel_ids,
        ["channel_3", "channel_4"]
    );
}

#[test]
fn load_telegram_connector_config() {
    let _guard = ENV_LOCK.lock();
    let home = TempHome::new();
    let data_dir = home.path().join(".kura-test");
    write_config(
        &data_dir,
        r#"{
        "connectors": {
            "telegram": {
                "enabled": true,
                "connectorId": "telegram-bot",
                "displayName": "Telegram Bot",
                "botTokenEnv": "TELEGRAM_TEST_TOKEN",
                "botUsername": "kura_test_bot",
                "allowedUserIds": ["user_1"],
                "allowedDirectChatIds": ["chat_1"],
                "allowedGroupIds": ["group_1"]
            }
        }
    }"#,
    );

    set_base_env(home.path());
    set_env("TELEGRAM_TEST_TOKEN", "telegram-secret");
    set_env("KURA_CONNECTORS_TELEGRAM_ALLOWED_GROUP_IDS", "group_2,group_3");

    let cfg = load().expect("load");

    assert!(cfg.connectors.telegram.enabled);
    assert_eq!(cfg.connectors.telegram.connector_id, "telegram-bot");
    assert_eq!(cfg.connectors.telegram.bot_token, "telegram-secret");
    assert_eq!(cfg.connectors.telegram.bot_username, "kura_test_bot");
    assert_eq!(cfg.connectors.telegram.allowed_direct_chat_ids, ["chat_1"]);
    assert_eq!(
        cfg.connectors.telegram.allowed_group_ids,
        ["group_2", "group_3"]
    );
}

#[test]
fn load_slack_connector_config() {
    let _guard = ENV_LOCK.lock();
    let home = TempHome::new();
    let data_dir = home.path().join(".kura-test");
    write_config(
        &data_dir,
        r#"{
        "connectors": {
            "slack": {
                "enabled": true,
                "connectorId": "slack-bot",
                "displayName": "Slack Bot",
                "apiBaseUrl": "https://slack.test",
                "botTokenSecretRef": "slack/slack-bot/bot_token",
                "oauthClientId": "client_file",
                "oauthClientSecretEnv": "SLACK_CLIENT_SECRET",
                "oauthApiBaseUrl": "https://slack-oauth.test",
                "workspaceBindingId": "workspace_binding_file",
                "workspaceId": "workspace_file",
                "botUserId": "bot_file",
                "allowedChannelIds": ["channel_file"],
                "allowedDMUserIds": ["user_file"],
                "allowedDMUserGroups": ["group_file"]
            }
        }
    }"#,
    );

    set_base_env(home.path());
    set_env(
        "KURA_CONNECTORS_SLACK_ALLOWED_CHANNEL_IDS",
        "channel_env_1,channel_env_2",
    );
    set_env("KURA_CONNECTORS_SLACK_BOT_USER_ID", "bot_env");
    set_env("SLACK_CLIENT_SECRET", "secret-from-env");

    let cfg = load().expect("load");

    assert!(cfg.connectors.slack.enabled);
    assert_eq!(cfg.connectors.slack.connector_id, "slack-bot");
    assert_eq!(cfg.connectors.slack.workspace_binding_id, "workspace_binding_file");
    assert_eq!(cfg.connectors.slack.workspace_id, "workspace_file");
    assert_eq!(cfg.connectors.slack.api_base_url, "https://slack.test");
    assert_eq!(cfg.connectors.slack.bot_token_secret_ref, "slack/slack-bot/bot_token");
    assert_eq!(cfg.connectors.slack.oauth_client_id, "client_file");
    assert_eq!(cfg.connectors.slack.oauth_client_secret, "secret-from-env");
    assert_eq!(cfg.connectors.slack.oauth_api_base_url, "https://slack-oauth.test");
    assert_eq!(cfg.connectors.slack.bot_user_id, "bot_env");
    assert_eq!(
        cfg.connectors.slack.allowed_channel_ids,
        ["channel_env_1", "channel_env_2"]
    );
    assert_eq!(cfg.connectors.slack.allowed_dm_user_groups, ["group_file"]);
}

#[test]
fn load_matrix_connector_config() {
    let _guard = ENV_LOCK.lock();
    let home = TempHome::new();
    let data_dir = home.path().join(".kura-test");
    write_config(
        &data_dir,
        r#"{
        "connectors": {
            "matrix": {
                "enabled": true,
                "connectorId": "matrix-bot",
                "displayName": "Matrix Bot",
                "homeserverUrl": "https://matrix.example.org",
                "homeserverId": "example.org",
                "botUserId": "@bot:example.org",
                "botAccessTokenEnv": "MATRIX_BOT_TOKEN",
                "selectedRoomIds": ["!room_file:example.org"],
                "allowedDirectUserIds": ["@alice:example.org"],
                "configuredCommands": ["!kura"]
            }
        }
    }"#,
    );

    set_base_env(home.path());
    set_env("MATRIX_BOT_TOKEN", "matrix-secret");
    set_env(
        "KURA_CONNECTORS_MATRIX_SELECTED_ROOM_IDS",
        "!room_env_1:example.org,!room_env_2:example.org",
    );

    let cfg = load().expect("load");

    assert!(cfg.connectors.matrix.enabled);
    assert_eq!(cfg.connectors.matrix.connector_id, "matrix-bot");
    assert_eq!(cfg.connectors.matrix.display_name, "Matrix Bot");
    assert_eq!(cfg.connectors.matrix.homeserver_url, "https://matrix.example.org");
    assert_eq!(cfg.connectors.matrix.homeserver_id, "example.org");
    assert_eq!(cfg.connectors.matrix.bot_access_token, "matrix-secret");
    assert_eq!(
        cfg.connectors.matrix.selected_room_ids,
        ["!room_env_1:example.org", "!room_env_2:example.org"]
    );

    let readiness = cfg.connectors.matrix.project_hosted_readiness("ten_matrix");
    assert!(!readiness.hosted_ready);
    assert_eq!(readiness.hosted_homeserver_policy, "unsupported");
    assert!(readiness.bot_access_token_set);
}

#[test]
fn load_rejects_invalid_config_file() {
    let _guard = ENV_LOCK.lock();
    let home = TempHome::new();
    let data_dir = home.path().join(".kura-test");
    write_config(&data_dir, r#"{"bindAddr":"#);

    set_base_env(home.path());

    let err = load().expect_err("expected load to fail for invalid config file");
    assert!(
        err.to_string().contains("decode config file"),
        "unexpected error: {err}"
    );
}

#[test]
fn load_uses_prod_defaults_when_environment_is_prod() {
    let _guard = ENV_LOCK.lock();
    let home = TempHome::new();
    set_base_env(home.path());
    set_env("KURA_ENV", "prod");

    let cfg = load().expect("load");

    let expected_data_dir = home.path().join(".kura");
    assert_eq!(cfg.environment, Environment::Prod);
    assert_eq!(cfg.data_dir, expected_data_dir.to_string_lossy());
    assert_eq!(cfg.bind_addr, "127.0.0.1:19191");
}

#[test]
fn discord_local_config_projects_into_hosted_readiness_without_breaking_legacy_use() {
    let cfg = DiscordConnectorConfig {
        enabled: true,
        connector_id: "discord-main".to_string(),
        display_name: "Discord Main".to_string(),
        delivery_mode: "gateway".to_string(),
        bot_token: "local-dev-token".to_string(),
        require_mention: true,
        respond_in_dm: true,
        allowed_guild_ids: vec!["guild_local".to_string()],
        allowed_channel_ids: vec!["channel_local".to_string()],
        ..Default::default()
    };

    let projection = cfg.project_hosted_readiness("ten_local");

    assert!(
        projection.local_compatible,
        "expected local config to remain compatible: {projection:?}"
    );
    assert_eq!(
        projection.readiness_state, "degraded_needs_repair",
        "readiness must stay degraded until explicit hosted destinations validate"
    );
    assert!(
        !projection.hosted_ready,
        "legacy local config must not become hosted-ready without validated destination evidence"
    );
    assert_eq!(projection.reason_code, "destination_validation_required");
    assert!(
        projection.bot_token_configured && projection.bot_token.is_empty(),
        "projection must expose configured flag without token material: {projection:?}"
    );
}
