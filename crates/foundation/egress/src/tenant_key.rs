//! Which tenant a request through the egress proxy belongs to.
//!
//! Every tenant container reaches the world through the same proxy on the
//! same internal network, so the proxy cannot tell tenants apart by address.
//! The daemon and the proxy share a key. The daemon gives each tenant's
//! containers a proxy credential derived from it:
//!
//! ```text
//! username = tenant id, password = hex(HMAC-SHA256(key, "kura-egress-tenant:v1:" + tenant id))
//! ```
//!
//! The proxy recomputes the password and attributes the request to that
//! tenant. A tenant can read its own credential (it is in its containers'
//! environment) but cannot make another's without the key.

use hmac::{Hmac, Mac};
use sha2::Sha256;

const CONTEXT: &[u8] = b"kura-egress-tenant:v1:";

/// The shared key. `Debug` never shows it.
#[derive(Clone, PartialEq, Eq)]
pub struct EgressKey([u8; 32]);

impl std::fmt::Debug for EgressKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EgressKey(<redacted>)")
    }
}

impl EgressKey {
    /// 32 raw bytes, or 64 hex characters (surrounding whitespace ignored).
    pub fn from_bytes(raw: &[u8]) -> Result<Self, String> {
        if let Ok(key) = <[u8; 32]>::try_from(raw) {
            return Ok(Self(key));
        }
        let text = std::str::from_utf8(raw)
            .map_err(|_| "egress tenant key is neither 32 bytes nor hex".to_string())?
            .trim();
        if text.len() != 64 {
            return Err(format!(
                "egress tenant key must be 32 bytes or 64 hex characters (got {} characters)",
                text.len()
            ));
        }
        let mut key = [0u8; 32];
        for (index, byte) in key.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16)
                .map_err(|_| "egress tenant key is not valid hex".to_string())?;
        }
        Ok(Self(key))
    }

    pub fn from_file(path: &std::path::Path) -> Result<Self, String> {
        let raw = std::fs::read(path)
            .map_err(|err| format!("read egress tenant key {}: {err}", path.display()))?;
        Self::from_bytes(&raw)
    }

    fn mac(&self, tenant_id: &str) -> Hmac<Sha256> {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.0).expect("any key length works");
        mac.update(CONTEXT);
        mac.update(tenant_id.as_bytes());
        mac
    }

    /// The proxy password for `tenant_id`.
    #[must_use]
    pub fn credential(&self, tenant_id: &str) -> String {
        self.mac(tenant_id)
            .finalize()
            .into_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// Whether `presented` is `tenant_id`'s credential. Constant-time in the
    /// comparison.
    #[must_use]
    pub fn verify(&self, tenant_id: &str, presented: &str) -> bool {
        if presented.len() != 64 {
            return false;
        }
        let mut bytes = [0u8; 32];
        for (index, byte) in bytes.iter_mut().enumerate() {
            match u8::from_str_radix(&presented[index * 2..index * 2 + 2], 16) {
                Ok(value) => *byte = value,
                Err(_) => return false,
            }
        }
        self.mac(tenant_id).verify_slice(&bytes).is_ok()
    }

    /// `proxy_url` with `tenant_id` and its credential as userinfo, which
    /// curl, pip, git and Playwright send as `Proxy-Authorization: Basic`.
    pub fn proxy_url_for(&self, proxy_url: &str, tenant_id: &str) -> Result<String, String> {
        let mut url = url::Url::parse(proxy_url)
            .map_err(|err| format!("egress proxy URL {proxy_url:?}: {err}"))?;
        url.set_username(tenant_id)
            .and_then(|()| url.set_password(Some(&self.credential(tenant_id))))
            .map_err(|()| format!("egress proxy URL {proxy_url:?} cannot carry a credential"))?;
        Ok(url.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> EgressKey {
        EgressKey::from_bytes(&[7u8; 32]).unwrap()
    }

    #[test]
    fn a_tenants_credential_verifies_for_it_and_no_other() {
        let key = key();
        let a = key.credential("ten_a");
        assert_eq!(a.len(), 64);
        assert!(key.verify("ten_a", &a));
        assert!(
            !key.verify("ten_b", &a),
            "another tenant's name with a's password"
        );
        assert!(!key.verify("ten_a", &key.credential("ten_b")));
        assert!(!key.verify("ten_a", "not-hex"));
        let other = EgressKey::from_bytes(&[8u8; 32]).unwrap();
        assert!(!other.verify("ten_a", &a), "a different key");
    }

    #[test]
    fn keys_load_as_raw_bytes_or_hex_and_never_print() {
        let hex = "07".repeat(32);
        assert_eq!(
            EgressKey::from_bytes(format!("{hex}\n").as_bytes()).unwrap(),
            key()
        );
        assert!(EgressKey::from_bytes(b"short").is_err());
        assert!(EgressKey::from_bytes("zz".repeat(32).as_bytes()).is_err());
        assert_eq!(format!("{:?}", key()), "EgressKey(<redacted>)");
    }

    #[test]
    fn the_proxy_url_carries_the_credential() {
        let url = key()
            .proxy_url_for("http://172.31.0.2:3128", "ten_a")
            .unwrap();
        let parsed = url::Url::parse(&url).unwrap();
        assert_eq!(parsed.username(), "ten_a");
        assert_eq!(parsed.password(), Some(key().credential("ten_a").as_str()));
        assert_eq!(parsed.host_str(), Some("172.31.0.2"));
        assert_eq!(parsed.port(), Some(3128));
    }
}
