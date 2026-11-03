//! Pairing codes and device tokens (docs/harness/remote-computer-use.md §4).
//!
//! A signed-in user asks for a code; the device trades it, once and within
//! ten minutes, for a token it keeps. Both are `<kind>.<tenant>.<secret>`:
//! the tenant part lets a gateway route the device to its cell without
//! knowing the secret, and the cell checks it against the row anyway. Only
//! SHA-256 hashes of the whole string are stored.

use chrono::{DateTime, Duration, Utc};
use kura_store::{DevicePairingRecord, DeviceRecord, SQLiteStore};
use sha2::{Digest, Sha256};

const CODE_KIND: &str = "kpair";
const TOKEN_KIND: &str = "kdev";
/// 128 bits for a code that lives ten minutes and works once; 256 for a
/// token that lives until revoked.
const CODE_BYTES: usize = 16;
const TOKEN_BYTES: usize = 32;
pub const PAIRING_TTL: Duration = Duration::minutes(10);
const MAX_NAME_CHARS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingCode {
    pub code: String,
    pub expires_at: DateTime<Utc>,
}

/// A device that was just paired, with the only copy of its token.
#[derive(Debug, Clone, PartialEq)]
pub struct Paired {
    pub device: DeviceRecord,
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    /// The code is malformed, unknown, spent or expired: one answer for
    /// all, so a client learns nothing from trying.
    InvalidCode,
    /// The token is malformed or unknown.
    InvalidToken,
    Revoked,
    Invalid(String),
    Store(String),
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidCode => f.write_str("the pairing code is invalid or has expired"),
            Self::InvalidToken => f.write_str("the device token is invalid"),
            Self::Revoked => f.write_str("the device was revoked"),
            Self::Invalid(message) => f.write_str(message),
            Self::Store(message) => write!(f, "store: {message}"),
        }
    }
}

impl std::error::Error for RegistryError {}

/// A new single-use code for `tenant_id`, on behalf of `principal_id`.
pub fn start_pairing(
    store: &SQLiteStore,
    tenant_id: &str,
    principal_id: &str,
    now: DateTime<Utc>,
) -> Result<PairingCode, RegistryError> {
    let tenant_id = routable_tenant(tenant_id)?;
    if principal_id.trim().is_empty() {
        return Err(RegistryError::Invalid(
            "pairing a device needs a signed-in principal".into(),
        ));
    }
    let code = secret(CODE_KIND, tenant_id, CODE_BYTES);
    let expires_at = now + PAIRING_TTL;
    store
        .insert_device_pairing(&DevicePairingRecord {
            code_hash: hash(&code),
            tenant_id: tenant_id.to_string(),
            principal_id: principal_id.trim().to_string(),
            created_at: now,
            expires_at,
        })
        .map_err(RegistryError::Store)?;
    Ok(PairingCode { code, expires_at })
}

/// Trades a code for a device and its token.
pub fn redeem(
    store: &SQLiteStore,
    code: &str,
    name: &str,
    platform: &str,
    now: DateTime<Utc>,
) -> Result<Paired, RegistryError> {
    let Some(tenant_id) = tenant_of(code.trim(), CODE_KIND) else {
        return Err(RegistryError::InvalidCode);
    };
    let name = bounded("name", name)?;
    let platform = bounded("platform", platform)?;
    let token = secret(TOKEN_KIND, tenant_id, TOKEN_BYTES);
    let candidate = DeviceRecord {
        device_id: format!("dev_{}", uuid::Uuid::new_v4().simple()),
        tenant_id: String::new(),
        principal_id: String::new(),
        name,
        platform,
        token_hash: hash(&token),
        created_at: now,
        last_seen_at: None,
        revoked_at: None,
    };
    let device = store
        .redeem_device_pairing(&hash(code.trim()), &now, &candidate)
        .map_err(RegistryError::Store)?
        .ok_or(RegistryError::InvalidCode)?;
    // The hash covers the whole code, tenant part included, so the row it
    // found is the tenant the code names.
    debug_assert_eq!(device.tenant_id, tenant_id);
    Ok(Paired { device, token })
}

/// The live device a token belongs to.
pub fn authenticate(store: &SQLiteStore, token: &str) -> Result<DeviceRecord, RegistryError> {
    let token = token.trim();
    let Some(tenant_id) = tenant_of(token, TOKEN_KIND) else {
        return Err(RegistryError::InvalidToken);
    };
    let device = store
        .device_by_token_hash(&hash(token))
        .map_err(RegistryError::Store)?
        .ok_or(RegistryError::InvalidToken)?;
    if device.tenant_id != tenant_id {
        return Err(RegistryError::InvalidToken);
    }
    if device.revoked_at.is_some() {
        return Err(RegistryError::Revoked);
    }
    Ok(device)
}

/// The tenant part a gateway routes on, without checking anything else.
#[must_use]
pub fn route_of(credential: &str) -> Option<&str> {
    tenant_of(credential.trim(), CODE_KIND).or_else(|| tenant_of(credential.trim(), TOKEN_KIND))
}

fn tenant_of<'a>(value: &'a str, kind: &str) -> Option<&'a str> {
    let rest = value.strip_prefix(kind)?.strip_prefix('.')?;
    let (tenant, secret) = rest.rsplit_once('.')?;
    (!tenant.is_empty() && !secret.is_empty() && secret.bytes().all(|b| b.is_ascii_hexdigit()))
        .then_some(tenant)
}

/// A tenant id that can sit in a credential: the gateway splits on `.` from
/// the right, so only the secret must be free of it, but whitespace and
/// control characters would not survive a header or a shell.
fn routable_tenant(tenant_id: &str) -> Result<&str, RegistryError> {
    let tenant_id = tenant_id.trim();
    if tenant_id.is_empty() {
        return Err(RegistryError::Invalid(
            "pairing a device needs a tenant".into(),
        ));
    }
    if !tenant_id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
    {
        return Err(RegistryError::Invalid(format!(
            "tenant id {tenant_id:?} cannot be carried in a device credential"
        )));
    }
    Ok(tenant_id)
}

fn bounded(field: &str, value: &str) -> Result<String, RegistryError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(RegistryError::Invalid(format!("{field} is required")));
    }
    if value.chars().count() > MAX_NAME_CHARS || value.chars().any(char::is_control) {
        return Err(RegistryError::Invalid(format!(
            "{field} must be at most {MAX_NAME_CHARS} printable characters"
        )));
    }
    Ok(value.to_string())
}

fn secret(kind: &str, tenant_id: &str, bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::fill(&mut buf[..]);
    format!("{kind}.{tenant_id}.{}", hex::encode(buf))
}

fn hash(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(name: &str) -> SQLiteStore {
        let dir = std::env::temp_dir().join(format!(
            "kura_devices_registry_{name}_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        SQLiteStore::new(&dir.to_string_lossy()).unwrap()
    }

    #[test]
    fn a_code_becomes_a_device_whose_token_authenticates() {
        let store = store("pair");
        let now = Utc::now();
        let code = start_pairing(&store, "ten_a", "prn_1", now).unwrap();
        assert!(code.code.starts_with("kpair.ten_a."));
        assert_eq!(code.expires_at, now + PAIRING_TTL);
        assert_eq!(route_of(&code.code), Some("ten_a"));

        let paired = redeem(&store, &code.code, "work mac", "macos", now).unwrap();
        assert!(paired.token.starts_with("kdev.ten_a."));
        assert_eq!(route_of(&paired.token), Some("ten_a"));
        assert_eq!(paired.device.tenant_id, "ten_a");
        assert_eq!(paired.device.principal_id, "prn_1");
        // Nothing secret is stored.
        assert_ne!(paired.device.token_hash, paired.token);

        let device = authenticate(&store, &paired.token).unwrap();
        assert_eq!(device.device_id, paired.device.device_id);

        // Once only.
        assert_eq!(
            redeem(&store, &code.code, "again", "macos", now),
            Err(RegistryError::InvalidCode)
        );

        store
            .revoke_device("ten_a", &device.device_id, &now)
            .unwrap();
        assert_eq!(
            authenticate(&store, &paired.token),
            Err(RegistryError::Revoked)
        );
    }

    #[test]
    fn malformed_and_forged_credentials_are_refused_alike() {
        let store = store("forged");
        let now = Utc::now();
        let code = start_pairing(&store, "ten_a", "prn_1", now).unwrap();
        let paired = redeem(&store, &code.code, "mac", "macos", now).unwrap();

        for bad in [
            "",
            "kdev",
            "kdev..ab",
            "kdev.ten_a.",
            "kdev.ten_a.zz",
            "kpair.ten_a.00",
        ] {
            assert_eq!(
                authenticate(&store, bad),
                Err(RegistryError::InvalidToken),
                "{bad}"
            );
        }
        // The right secret under another tenant's name routes elsewhere and
        // is refused there.
        let secret = paired.token.rsplit_once('.').unwrap().1;
        assert_eq!(
            authenticate(&store, &format!("kdev.ten_b.{secret}")),
            Err(RegistryError::InvalidToken)
        );
        assert_eq!(
            redeem(&store, "kpair.ten_a.00", "mac", "macos", now),
            Err(RegistryError::InvalidCode)
        );
    }

    #[test]
    fn an_expired_code_is_refused() {
        let store = store("expired");
        let then = Utc::now() - PAIRING_TTL - Duration::seconds(1);
        let code = start_pairing(&store, "ten_a", "prn_1", then).unwrap();
        assert_eq!(
            redeem(&store, &code.code, "mac", "macos", Utc::now()),
            Err(RegistryError::InvalidCode)
        );
    }

    #[test]
    fn inputs_are_bounded() {
        let store = store("bounds");
        let now = Utc::now();
        assert!(start_pairing(&store, "", "prn_1", now).is_err());
        assert!(start_pairing(&store, "ten a", "prn_1", now).is_err());
        assert!(start_pairing(&store, "ten_a", " ", now).is_err());
        let code = start_pairing(&store, "ten_a", "prn_1", now).unwrap();
        assert!(matches!(
            redeem(&store, &code.code, &"x".repeat(65), "macos", now),
            Err(RegistryError::Invalid(_))
        ));
        assert!(matches!(
            redeem(&store, &code.code, "a\nb", "macos", now),
            Err(RegistryError::Invalid(_))
        ));
        // A refused name does not spend the code.
        assert!(redeem(&store, &code.code, "mac", "macos", now).is_ok());
    }
}
