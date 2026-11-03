//! Where the device keeps what it knows: the server and device id in a
//! small JSON file, the token in the macOS Keychain (never on disk), and
//! the pid of the running `kura-device run` for `stop` and `status`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Overrides the directory, for a second profile or tests.
pub const HOME_ENV: &str = "KURA_DEVICE_HOME";
/// The Keychain service the token is stored under; the account is the
/// server URL.
pub const KEYCHAIN_SERVICE: &str = "dev.kura.device";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub server: String,
    pub device_id: String,
    pub name: String,
}

#[must_use]
pub fn home() -> PathBuf {
    if let Some(dir) = std::env::var_os(HOME_ENV) {
        return PathBuf::from(dir);
    }
    let user = std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from);
    if cfg!(target_os = "macos") {
        user.join("Library/Application Support/kura-device")
    } else {
        user.join(".config/kura-device")
    }
}

#[must_use]
pub fn log_file() -> PathBuf {
    if std::env::var_os(HOME_ENV).is_none() && cfg!(target_os = "macos") {
        if let Some(user) = std::env::var_os("HOME") {
            return PathBuf::from(user).join("Library/Logs/kura-device.log");
        }
    }
    home().join("kura-device.log")
}

#[must_use]
pub fn pid_file(home: &Path) -> PathBuf {
    home.join("kura-device.pid")
}

pub fn load(home: &Path) -> Result<Option<Config>, String> {
    let path = home.join("config.json");
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|err| format!("{}: {err}", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!("{}: {err}", path.display())),
    }
}

pub fn save(home: &Path, config: &Config) -> Result<(), String> {
    std::fs::create_dir_all(home).map_err(|err| format!("{}: {err}", home.display()))?;
    let path = home.join("config.json");
    let tmp = home.join("config.json.tmp");
    let bytes = serde_json::to_vec_pretty(config).map_err(|err| err.to_string())?;
    std::fs::write(&tmp, bytes).map_err(|err| format!("{}: {err}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, &path).map_err(|err| format!("{}: {err}", path.display()))
}

pub fn remove(home: &Path) -> Result<(), String> {
    match std::fs::remove_file(home.join("config.json")) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.to_string()),
    }
}

/// The pid in the pid file, if that process is still alive.
#[must_use]
pub fn running_pid(home: &Path) -> Option<i32> {
    let pid: i32 = std::fs::read_to_string(pid_file(home))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    // Signal 0 checks the process exists without touching it.
    (pid > 0 && unsafe { libc::kill(pid, 0) } == 0).then_some(pid)
}

/// The device token, kept in the platform's credential store.
pub trait TokenStore {
    fn get(&self, server: &str) -> Result<Option<String>, String>;
    fn set(&self, server: &str, token: &str) -> Result<(), String>;
    fn delete(&self, server: &str) -> Result<(), String>;
}

#[cfg(target_os = "macos")]
pub struct Keychain;

#[cfg(target_os = "macos")]
impl TokenStore for Keychain {
    fn get(&self, server: &str) -> Result<Option<String>, String> {
        use security_framework::base::Error;
        // errSecItemNotFound
        const NOT_FOUND: i32 = -25300;
        match security_framework::passwords::get_generic_password(KEYCHAIN_SERVICE, server) {
            Ok(bytes) => String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| "the stored token is not text".into()),
            Err(err) if Error::code(err) == NOT_FOUND => Ok(None),
            Err(err) => Err(format!("keychain: {err}")),
        }
    }

    fn set(&self, server: &str, token: &str) -> Result<(), String> {
        security_framework::passwords::set_generic_password(
            KEYCHAIN_SERVICE,
            server,
            token.as_bytes(),
        )
        .map_err(|err| format!("keychain: {err}"))
    }

    fn delete(&self, server: &str) -> Result<(), String> {
        use security_framework::base::Error;
        const NOT_FOUND: i32 = -25300;
        match security_framework::passwords::delete_generic_password(KEYCHAIN_SERVICE, server) {
            Ok(()) => Ok(()),
            Err(err) if Error::code(err) == NOT_FOUND => Ok(()),
            Err(err) => Err(format!("keychain: {err}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trips_and_a_missing_file_is_none() {
        let dir = std::env::temp_dir().join(format!("kura-device-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(load(&dir).unwrap(), None);
        let config = Config {
            server: "https://kura.example.com".into(),
            device_id: "dev_1".into(),
            name: "work mac".into(),
        };
        save(&dir, &config).unwrap();
        assert_eq!(load(&dir).unwrap(), Some(config));
        let text = std::fs::read_to_string(dir.join("config.json")).unwrap();
        assert!(text.contains("\"deviceId\""), "{text}");
        assert!(!text.contains("kdev."), "the token never goes to disk");
        remove(&dir).unwrap();
        remove(&dir).unwrap();
        assert_eq!(load(&dir).unwrap(), None);
        assert_eq!(running_pid(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
