//! Config loading entry points: [`load`], directory resolution, and helpers.

use std::path::{Path, PathBuf};

use crate::error::ConfigError;
use crate::file::{apply_file_config, load_file_config};
use crate::overrides::{apply_env_overrides, getenv, resolve_secret_refs};
use crate::types::{Config, Environment, default_data_dir, resolve_environment};

/// Name of the config file inside the data directory.
pub const DEFAULT_CONFIG_FILE_NAME: &str = "config.json";

/// Load the effective daemon configuration.
///
/// Order of operations (Go `Load`):
/// 1. Resolve the bootstrap data dir from `KURA_DATA_DIR` or the
///    environment-aware default, creating it.
/// 2. Start from environment-aware defaults.
/// 3. Merge `config.json` from the bootstrap data dir (missing file is fine,
///    invalid JSON is an error).
/// 4. Apply `KURA_*` environment variable overrides.
/// 5. Resolve `*Env` secret references.
/// 6. Re-resolve the effective data dir (file/env may have changed it) and
///    create it.
pub fn load() -> Result<Config, ConfigError> {
    let version = getenv("KURA_VERSION", "dev");
    let env_name = resolve_environment(&getenv("KURA_ENV", ""), &version);

    let bootstrap_dir = resolve_dir(&getenv("KURA_DATA_DIR", default_data_dir(env_name)))
        .map_err(|err| ConfigError::ResolveBootstrapDataDir(Box::new(err)))?;
    ensure_dir(&bootstrap_dir).map_err(ConfigError::InitBootstrapDataDir)?;

    let mut cfg = Config::defaults(env_name, version, bootstrap_dir.clone());

    let file_cfg = load_file_config(&Path::new(&bootstrap_dir).join(DEFAULT_CONFIG_FILE_NAME))?;
    apply_file_config(&mut cfg, file_cfg);
    apply_env_overrides(&mut cfg);
    resolve_secret_refs(&mut cfg);

    cfg.data_dir = resolve_dir(&cfg.data_dir)
        .map_err(|err| ConfigError::ResolveEffectiveDataDir(Box::new(err)))?;
    ensure_dir(&cfg.data_dir).map_err(ConfigError::InitEffectiveDataDir)?;

    Ok(cfg)
}

/// Expand a leading `~` / `~/` against the user home directory
/// (Go `ResolveDir`). Empty paths are an error; other paths pass through
/// unchanged.
pub fn resolve_dir(path: &str) -> Result<String, ConfigError> {
    if path.is_empty() {
        return Err(ConfigError::PathRequired);
    }
    if path == "~" {
        return user_home_dir();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        let home = user_home_dir()?;
        return Ok(Path::new(&home).join(rest).to_string_lossy().into_owned());
    }
    Ok(path.to_string())
}

/// Home directory exposed to managed CLI providers (Go
/// `ManagedProviderHomeDir`): an isolated root under the test data dir, or
/// the real user home otherwise. Returns an empty string when the home
/// directory cannot be determined.
pub fn managed_provider_home_dir(cfg: &Config) -> String {
    if cfg.environment == Environment::Test && !cfg.data_dir.trim().is_empty() {
        return Path::new(cfg.data_dir.trim())
            .join("managed-provider-home")
            .to_string_lossy()
            .into_owned();
    }
    user_home_dir()
        .map(|home| home.trim().to_string())
        .unwrap_or_default()
}

/// Path of the config file inside `data_dir` (Go `ConfigFilePath`).
pub fn config_file_path(data_dir: &str) -> PathBuf {
    Path::new(data_dir).join(DEFAULT_CONFIG_FILE_NAME)
}

fn ensure_dir(path: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}

fn user_home_dir() -> Result<String, ConfigError> {
    #[cfg(unix)]
    let candidate = std::env::var("HOME").ok();
    #[cfg(windows)]
    let candidate = std::env::var("USERPROFILE").ok();
    #[cfg(not(any(unix, windows)))]
    let candidate = std::env::var("HOME").ok();

    candidate
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ConfigError::HomeResolution("user home directory is undefined".to_string()))
}
