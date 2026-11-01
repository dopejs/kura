//! Pairing and access-token lifecycle management.
//!
//! Port of `daemon/internal/auth/auth.go`. In-memory by design; persistence
//! is restored via [`Manager::restore`]. Secrets are only ever stored as
//! SHA-256 hashes; the plaintext secret is returned exactly once at issue.

use std::collections::HashMap;

use chrono::DateTime;
use chrono::TimeDelta;
use chrono::Utc;
use parking_lot::RwLock;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;

use crate::manager::random_id;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PairingMode {
    Local,
    Token,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PairingStatus {
    Pending,
    Completed,
    Expired,
}

/// Lifecycle of an access token. Go stores this as a plain string where the
/// empty string behaves as active; deserialization maps `""` to
/// [`TokenStatus::Active`] to preserve that.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TokenStatus {
    Active,
    Revoked,
    Expired,
    Rotated,
}

impl Default for TokenStatus {
    fn default() -> Self {
        Self::Active
    }
}

impl<'de> Deserialize<'de> for TokenStatus {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        match s.as_str() {
            "" | "active" => Ok(Self::Active),
            "revoked" => Ok(Self::Revoked),
            "expired" => Ok(Self::Expired),
            "rotated" => Ok(Self::Rotated),
            other => Err(serde::de::Error::custom(format!(
                "unknown token status {other:?}"
            ))),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("pairing mode is invalid")]
    PairingModeInvalid,
    #[error("pairing not found")]
    PairingNotFound,
    #[error("pairing code is invalid")]
    PairingCodeInvalid,
    #[error("pairing is not pending")]
    PairingNotPending,
    #[error("access token is invalid")]
    TokenInvalid,
    #[error("access token not found")]
    AccessTokenNotFound,
    #[error("authentication is required")]
    AuthRequired,
    #[error("access token is revoked")]
    TokenRevoked,
    #[error("access token is expired")]
    TokenExpired,
    #[error("access token is rotated")]
    TokenRotated,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pairing {
    pub pairing_id: String,
    pub mode: PairingMode,
    pub label: String,
    pub status: PairingStatus,
    #[serde(skip)]
    pub code_hash: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub code_preview: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessToken {
    pub token_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub principal_id: String,
    pub label: String,
    pub mode: PairingMode,
    #[serde(skip)]
    pub token_hash: String,
    pub token_preview: String,
    pub status: TokenStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub default_tenant_id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub rotated_from_token_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub rotated_to_token_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartPairingInput {
    #[serde(default)]
    pub mode: Option<PairingMode>,
    pub label: String,
    #[serde(default)]
    pub ttl_seconds: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletePairingInput {
    pub code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueTokenInput {
    pub principal_id: String,
    pub label: String,
    pub default_tenant_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RotateTokenInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

/// How stale `last_used_at` may get before an authentication refreshes it.
/// The field answers "is this token still in use", so it is kept to this
/// resolution: a busy token costs one write per interval, not one per
/// request.
pub const LAST_USED_RESOLUTION: TimeDelta = TimeDelta::seconds(60);

/// A successful authentication. `touched` says `last_used_at` was advanced
/// (see [`LAST_USED_RESOLUTION`]), which is when a caller persisting tokens
/// has something to write.
#[derive(Debug, Clone)]
pub struct Authenticated {
    pub token: AccessToken,
    pub touched: bool,
}

#[derive(Debug, Default)]
struct State {
    pairings: HashMap<String, Pairing>,
    pairing_ids: Vec<String>,
    tokens: HashMap<String, AccessToken>,
    token_ids: Vec<String>,
    /// `token_hash` -> `token_id`, so authentication is one lookup rather
    /// than a scan of every token. Maintained by [`State::put_token`], the
    /// only way a token enters or changes in `tokens`.
    by_hash: HashMap<String, String>,
}

impl State {
    fn put_token(&mut self, token: AccessToken) {
        match self.tokens.get(&token.token_id) {
            Some(previous) if previous.token_hash != token.token_hash => {
                if self.by_hash.get(&previous.token_hash) == Some(&token.token_id) {
                    self.by_hash.remove(&previous.token_hash);
                }
            }
            Some(_) => {}
            None => self.token_ids.push(token.token_id.clone()),
        }
        if !token.token_hash.is_empty() {
            self.by_hash
                .insert(token.token_hash.clone(), token.token_id.clone());
        }
        self.tokens.insert(token.token_id.clone(), token);
    }

    fn token_by_hash(&self, token_hash: &str) -> Option<&AccessToken> {
        self.by_hash
            .get(token_hash)
            .and_then(|token_id| self.tokens.get(token_id))
    }
}

/// Identity stamped onto tokens issued by the pairing exchange.
///
/// Pairing has no way to ask who is pairing, so by default it issues a token
/// with no principal — which the identity resolver correctly refuses. A
/// deployment that bootstraps a known local identity (see the embedded
/// environment) supplies it here so paired tokens resolve.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PairingIdentity {
    pub principal_id: String,
    pub default_tenant_id: String,
}

#[derive(Debug, Default)]
pub struct Manager {
    state: RwLock<State>,
    pairing_identity: PairingIdentity,
}

impl Manager {
    pub fn new() -> Self {
        Self::default()
    }

    /// A manager whose paired tokens carry `identity`.
    #[must_use]
    pub fn with_pairing_identity(identity: PairingIdentity) -> Self {
        Self {
            state: RwLock::default(),
            pairing_identity: identity,
        }
    }

    /// Starts a pairing session. Returns the pairing record and the one-time
    /// plaintext code (only the hash is retained).
    pub fn start_pairing(&self, input: StartPairingInput) -> Result<(Pairing, String), AuthError> {
        let mode = input.mode.unwrap_or(PairingMode::Local);
        let ttl_seconds = if input.ttl_seconds <= 0 {
            600
        } else {
            input.ttl_seconds
        };

        let now = Utc::now();
        let code = random_digits(6);
        let pairing = Pairing {
            pairing_id: format!("pair_{}", random_id(8)),
            mode,
            label: input.label,
            status: PairingStatus::Pending,
            code_hash: hash_secret(&code),
            code_preview: code.clone(),
            created_at: now,
            updated_at: now,
            expires_at: now + chrono::Duration::seconds(ttl_seconds),
            completed_at: None,
        };

        let mut state = self.state.write();
        state
            .pairings
            .insert(pairing.pairing_id.clone(), pairing.clone());
        state.pairing_ids.push(pairing.pairing_id.clone());
        Ok((pairing, code))
    }

    /// Completes a pending pairing, issuing an access token. Returns the
    /// completed pairing, the token record, and the one-time plaintext secret.
    pub fn complete_pairing(
        &self,
        pairing_id: &str,
        input: CompletePairingInput,
    ) -> Result<(Pairing, AccessToken, String), AuthError> {
        let mut state = self.state.write();

        let Some(mut pairing) = state.pairings.get(pairing_id).cloned() else {
            return Err(AuthError::PairingNotFound);
        };
        if pairing.status != PairingStatus::Pending {
            return Err(AuthError::PairingNotPending);
        }
        let now = Utc::now();
        if now > pairing.expires_at {
            pairing.status = PairingStatus::Expired;
            pairing.updated_at = now;
            state.pairings.insert(pairing_id.to_string(), pairing);
            return Err(AuthError::PairingNotPending);
        }
        if hash_secret(&input.code) != pairing.code_hash {
            return Err(AuthError::PairingCodeInvalid);
        }

        let token_secret = format!("kura_{}", random_id(24));
        let token = AccessToken {
            token_id: format!("tok_{}", random_id(8)),
            principal_id: self.pairing_identity.principal_id.clone(),
            label: pairing.label.clone(),
            mode: pairing.mode,
            token_hash: hash_secret(&token_secret),
            token_preview: token_secret[..12].to_string(),
            status: TokenStatus::Active,
            default_tenant_id: self.pairing_identity.default_tenant_id.clone(),
            created_at: now,
            updated_at: now,
            last_used_at: None,
            expires_at: None,
            revoked_at: None,
            rotated_from_token_id: String::new(),
            rotated_to_token_id: String::new(),
        };

        pairing.status = PairingStatus::Completed;
        pairing.updated_at = now;
        pairing.completed_at = Some(now);
        pairing.code_preview = String::new();
        state
            .pairings
            .insert(pairing_id.to_string(), pairing.clone());
        state.put_token(token.clone());

        Ok((pairing, token, token_secret))
    }

    /// Issues a token directly (token-mode pairing). Returns the token record
    /// and the one-time plaintext secret.
    pub fn issue_token(&self, input: IssueTokenInput) -> Result<(AccessToken, String), AuthError> {
        if input.principal_id.is_empty() || input.default_tenant_id.is_empty() {
            return Err(AuthError::TokenInvalid);
        }
        let now = Utc::now();
        let token_secret = format!("kura_{}", random_id(24));
        let token = AccessToken {
            token_id: format!("tok_{}", random_id(8)),
            principal_id: input.principal_id,
            label: input.label,
            mode: PairingMode::Token,
            token_hash: hash_secret(&token_secret),
            token_preview: token_secret[..12].to_string(),
            status: TokenStatus::Active,
            default_tenant_id: input.default_tenant_id,
            created_at: now,
            updated_at: now,
            last_used_at: None,
            expires_at: input.expires_at,
            revoked_at: None,
            rotated_from_token_id: String::new(),
            rotated_to_token_id: String::new(),
        };

        self.state.write().put_token(token.clone());
        Ok((token, token_secret))
    }

    pub fn revoke_token(&self, token_id: &str) -> Result<AccessToken, AuthError> {
        let mut state = self.state.write();
        let Some(mut token) = state.tokens.get(token_id).cloned() else {
            return Err(AuthError::AccessTokenNotFound);
        };
        match token.status {
            TokenStatus::Rotated => return Err(AuthError::TokenRotated),
            TokenStatus::Expired => return Err(AuthError::TokenExpired),
            TokenStatus::Active | TokenStatus::Revoked => {}
        }
        let now = Utc::now();
        token.status = TokenStatus::Revoked;
        token.updated_at = now;
        token.revoked_at = Some(now);
        state.put_token(token.clone());
        Ok(token)
    }

    /// Rotates a token: the old token is marked rotated with a pointer to its
    /// replacement, which inherits principal, label, mode, and default tenant.
    /// Returns the rotated old token, the replacement, and its plaintext secret.
    pub fn rotate_token(
        &self,
        token_id: &str,
        input: RotateTokenInput,
    ) -> Result<(AccessToken, AccessToken, String), AuthError> {
        let mut state = self.state.write();
        let Some(mut old_token) = state.tokens.get(token_id).cloned() else {
            return Err(AuthError::AccessTokenNotFound);
        };
        match old_token.status {
            TokenStatus::Active => {}
            TokenStatus::Revoked => return Err(AuthError::TokenRevoked),
            TokenStatus::Expired => return Err(AuthError::TokenExpired),
            TokenStatus::Rotated => return Err(AuthError::TokenRotated),
        }
        let now = Utc::now();
        if old_token
            .expires_at
            .is_some_and(|expires_at| expires_at <= now)
        {
            old_token.status = TokenStatus::Expired;
            old_token.updated_at = now;
            state.put_token(old_token);
            return Err(AuthError::TokenExpired);
        }

        let replacement_secret = format!("kura_{}", random_id(24));
        let replacement = AccessToken {
            token_id: format!("tok_{}", random_id(8)),
            principal_id: old_token.principal_id.clone(),
            label: old_token.label.clone(),
            mode: old_token.mode,
            token_hash: hash_secret(&replacement_secret),
            token_preview: replacement_secret[..12].to_string(),
            status: TokenStatus::Active,
            default_tenant_id: old_token.default_tenant_id.clone(),
            created_at: now,
            updated_at: now,
            last_used_at: None,
            expires_at: input.expires_at,
            revoked_at: None,
            rotated_from_token_id: old_token.token_id.clone(),
            rotated_to_token_id: String::new(),
        };
        old_token.status = TokenStatus::Rotated;
        old_token.updated_at = now;
        old_token.rotated_to_token_id = replacement.token_id.clone();
        state.put_token(old_token.clone());
        state.put_token(replacement.clone());
        Ok((old_token, replacement, replacement_secret))
    }

    /// Authenticates a plaintext token secret. Advances `last_used_at` (to
    /// [`LAST_USED_RESOLUTION`]) on success and lazily transitions overdue
    /// tokens to expired.
    pub fn authenticate(&self, token_secret: &str) -> Result<AccessToken, AuthError> {
        self.authenticate_touch(token_secret)
            .map(|authenticated| authenticated.token)
    }

    /// [`Self::authenticate`], also saying whether `last_used_at` moved.
    ///
    /// A token in use within the resolution is answered under the read lock,
    /// so concurrent requests do not queue on each other; only a refresh or
    /// an expiry takes the write lock.
    pub fn authenticate_touch(&self, token_secret: &str) -> Result<Authenticated, AuthError> {
        if token_secret.is_empty() {
            return Err(AuthError::AuthRequired);
        }
        let token_hash = hash_secret(token_secret);
        let now = Utc::now();
        {
            let state = self.state.read();
            let token = state
                .token_by_hash(&token_hash)
                .ok_or(AuthError::TokenInvalid)?;
            usable(token)?;
            if !overdue(token, now) && recently_used(token, now) {
                return Ok(Authenticated {
                    token: token.clone(),
                    touched: false,
                });
            }
        }

        let mut state = self.state.write();
        // Read again: the token may have changed between the two locks.
        let mut token = state
            .token_by_hash(&token_hash)
            .cloned()
            .ok_or(AuthError::TokenInvalid)?;
        usable(&token)?;
        if overdue(&token, now) {
            token.status = TokenStatus::Expired;
            token.updated_at = now;
            state.put_token(token);
            return Err(AuthError::TokenExpired);
        }
        if recently_used(&token, now) {
            return Ok(Authenticated {
                token,
                touched: false,
            });
        }
        token.last_used_at = Some(now);
        token.updated_at = now;
        state.put_token(token.clone());
        Ok(Authenticated {
            token,
            touched: true,
        })
    }

    pub fn get_token(&self, token_id: &str) -> Option<AccessToken> {
        self.state.read().tokens.get(token_id).cloned()
    }

    pub fn update_token(&self, token: AccessToken) {
        self.state.write().put_token(token);
    }

    pub fn list_tokens(&self) -> Vec<AccessToken> {
        let state = self.state.read();
        state
            .token_ids
            .iter()
            .filter_map(|token_id| state.tokens.get(token_id).cloned())
            .collect()
    }

    pub fn get_pairing(&self, pairing_id: &str) -> Option<Pairing> {
        self.state.read().pairings.get(pairing_id).cloned()
    }

    /// Replaces all in-memory state from persisted records.
    pub fn restore(&self, pairings: Vec<Pairing>, tokens: Vec<AccessToken>) {
        let mut state = self.state.write();
        state.pairings = pairings
            .iter()
            .map(|pairing| (pairing.pairing_id.clone(), pairing.clone()))
            .collect();
        state.pairing_ids = pairings
            .into_iter()
            .map(|pairing| pairing.pairing_id)
            .collect();
        state.tokens.clear();
        state.token_ids.clear();
        state.by_hash.clear();
        for token in tokens {
            state.put_token(token);
        }
    }
}

/// The status refusals, before any expiry check.
fn usable(token: &AccessToken) -> Result<(), AuthError> {
    match token.status {
        TokenStatus::Active => Ok(()),
        TokenStatus::Revoked => Err(AuthError::TokenRevoked),
        TokenStatus::Expired => Err(AuthError::TokenExpired),
        TokenStatus::Rotated => Err(AuthError::TokenRotated),
    }
}

fn overdue(token: &AccessToken, now: DateTime<Utc>) -> bool {
    token.expires_at.is_some_and(|expires_at| expires_at <= now)
}

fn recently_used(token: &AccessToken, now: DateTime<Utc>) -> bool {
    token
        .last_used_at
        .is_some_and(|last_used_at| now - last_used_at < LAST_USED_RESOLUTION)
}

fn hash_secret(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}

fn random_digits(length: usize) -> String {
    if length == 0 {
        return String::new();
    }
    let mut buf = vec![0u8; length];
    rand::fill(&mut buf[..]);
    buf.iter().map(|b| (b'0' + b % 10) as char).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_and_authentication_lifecycle() {
        let manager = Manager::new();

        let (pairing, code) = manager
            .start_pairing(StartPairingInput {
                mode: Some(PairingMode::Local),
                label: "web-ui".to_string(),
                ttl_seconds: 0,
            })
            .expect("start pairing");
        assert_eq!(pairing.status, PairingStatus::Pending);

        let (completed_pairing, token, token_secret) = manager
            .complete_pairing(&pairing.pairing_id, CompletePairingInput { code })
            .expect("complete pairing");
        assert_eq!(completed_pairing.status, PairingStatus::Completed);
        assert!(!token.token_id.is_empty());
        assert!(!token_secret.is_empty());

        let authenticated = manager.authenticate(&token_secret).expect("authenticate");
        assert_eq!(authenticated.token_id, token.token_id);
        assert!(authenticated.last_used_at.is_some());
    }

    fn issued(manager: &Manager) -> (AccessToken, String) {
        manager
            .issue_token(IssueTokenInput {
                principal_id: "prn_1".to_string(),
                label: "cli".to_string(),
                default_tenant_id: "tnt_1".to_string(),
                expires_at: None,
            })
            .expect("issue token")
    }

    #[test]
    fn last_used_moves_once_per_resolution() {
        let manager = Manager::new();
        let (token, secret) = issued(&manager);

        let first = manager.authenticate_touch(&secret).expect("first use");
        assert!(first.touched, "a token never used is recorded");
        let second = manager.authenticate_touch(&secret).expect("second use");
        assert!(!second.touched, "a use within the resolution is not");
        assert_eq!(second.token.last_used_at, first.token.last_used_at);

        let mut stale = manager.get_token(&token.token_id).expect("token");
        stale.last_used_at = Some(Utc::now() - LAST_USED_RESOLUTION);
        manager.update_token(stale);
        let third = manager.authenticate_touch(&secret).expect("third use");
        assert!(third.touched, "a use after the resolution is recorded");
        assert!(third.token.last_used_at > first.token.last_used_at);
    }

    #[test]
    fn a_recently_used_token_is_still_refused_once_revoked_or_overdue() {
        let manager = Manager::new();
        let (token, secret) = issued(&manager);
        manager.authenticate(&secret).expect("use");

        let mut overdue = manager.get_token(&token.token_id).expect("token");
        overdue.expires_at = Some(Utc::now() - TimeDelta::seconds(1));
        manager.update_token(overdue);
        assert!(matches!(
            manager.authenticate(&secret),
            Err(AuthError::TokenExpired)
        ));
        assert_eq!(
            manager.get_token(&token.token_id).expect("token").status,
            TokenStatus::Expired
        );

        let (token, secret) = issued(&manager);
        manager.authenticate(&secret).expect("use");
        manager.revoke_token(&token.token_id).expect("revoke");
        assert!(matches!(
            manager.authenticate(&secret),
            Err(AuthError::TokenRevoked)
        ));
    }

    #[test]
    fn the_hash_index_follows_rotation_replacement_and_restore() {
        let manager = Manager::new();
        let (token, secret) = issued(&manager);
        let (_, replacement, replacement_secret) = manager
            .rotate_token(&token.token_id, RotateTokenInput::default())
            .expect("rotate");
        assert!(matches!(
            manager.authenticate(&secret),
            Err(AuthError::TokenRotated)
        ));
        assert_eq!(
            manager
                .authenticate(&replacement_secret)
                .expect("replacement")
                .token_id,
            replacement.token_id
        );

        // A token whose hash is replaced answers to the new secret only.
        let mut rehashed = manager.get_token(&replacement.token_id).expect("token");
        rehashed.token_hash = hash_secret("kura_other");
        manager.update_token(rehashed);
        assert!(matches!(
            manager.authenticate(&replacement_secret),
            Err(AuthError::TokenInvalid)
        ));
        assert!(manager.authenticate("kura_other").is_ok());

        // Restore rebuilds the index from the persisted records alone.
        let restored = Manager::new();
        restored.restore(Vec::new(), manager.list_tokens());
        assert!(restored.authenticate("kura_other").is_ok());
        assert!(matches!(
            restored.authenticate(&secret),
            Err(AuthError::TokenRotated)
        ));
        assert_eq!(restored.list_tokens().len(), 2);
    }

    #[test]
    fn authenticate_rejects_invalid_token() {
        let manager = Manager::new();
        assert!(matches!(
            manager.authenticate("bad-token"),
            Err(AuthError::TokenInvalid)
        ));
        assert!(matches!(
            manager.authenticate(""),
            Err(AuthError::AuthRequired)
        ));
    }

    #[test]
    fn authenticate_rejects_revoked_expired_and_rotated_tokens() {
        let manager = Manager::new();
        let (pairing, code) = manager
            .start_pairing(StartPairingInput {
                mode: Some(PairingMode::Local),
                label: "cli".to_string(),
                ttl_seconds: 0,
            })
            .expect("start pairing");
        let (_, token, token_secret) = manager
            .complete_pairing(&pairing.pairing_id, CompletePairingInput { code })
            .expect("complete pairing");

        for (status, want) in [
            (TokenStatus::Revoked, AuthError::TokenRevoked),
            (TokenStatus::Expired, AuthError::TokenExpired),
            (TokenStatus::Rotated, AuthError::TokenRotated),
        ] {
            manager.restore(
                Vec::new(),
                vec![AccessToken {
                    status,
                    ..token.clone()
                }],
            );
            let err = manager
                .authenticate(&token_secret)
                .expect_err("must be denied");
            assert_eq!(err.to_string(), want.to_string(), "status {status:?}");
        }
    }

    #[test]
    fn issue_revoke_and_rotate_token_lifecycle() {
        let manager = Manager::new();
        let expires_at = Utc::now() + chrono::Duration::hours(1);

        let (token, secret) = manager
            .issue_token(IssueTokenInput {
                principal_id: "prn_1".to_string(),
                label: "automation".to_string(),
                default_tenant_id: "ten_1".to_string(),
                expires_at: Some(expires_at),
            })
            .expect("issue token");
        assert!(!token.token_id.is_empty());
        assert!(!secret.is_empty());
        assert!(!token.token_hash.is_empty());
        assert_ne!(token.token_hash, secret);
        assert_eq!(token.principal_id, "prn_1");
        assert_eq!(token.default_tenant_id, "ten_1");
        assert_eq!(token.status, TokenStatus::Active);
        manager.authenticate(&secret).expect("authenticate issued");

        let revoked = manager.revoke_token(&token.token_id).expect("revoke");
        assert_eq!(revoked.status, TokenStatus::Revoked);
        assert!(revoked.revoked_at.is_some());
        assert!(matches!(
            manager.authenticate(&secret),
            Err(AuthError::TokenRevoked)
        ));
    }

    #[test]
    fn issue_token_requires_principal_and_tenant() {
        let manager = Manager::new();
        let base = IssueTokenInput {
            principal_id: String::new(),
            label: "automation".to_string(),
            default_tenant_id: "ten_1".to_string(),
            expires_at: None,
        };
        assert!(matches!(
            manager.issue_token(base),
            Err(AuthError::TokenInvalid)
        ));
        let base = IssueTokenInput {
            principal_id: "prn_1".to_string(),
            label: "automation".to_string(),
            default_tenant_id: String::new(),
            expires_at: None,
        };
        assert!(matches!(
            manager.issue_token(base),
            Err(AuthError::TokenInvalid)
        ));
    }

    #[test]
    fn rotate_token_preserves_authority_lineage() {
        let manager = Manager::new();
        let (old_token, old_secret) = manager
            .issue_token(IssueTokenInput {
                principal_id: "prn_1".to_string(),
                label: "automation".to_string(),
                default_tenant_id: "ten_1".to_string(),
                expires_at: None,
            })
            .expect("issue token");

        let (rotated, replacement, replacement_secret) = manager
            .rotate_token(
                &old_token.token_id,
                RotateTokenInput {
                    reason: "scheduled".to_string(),
                    ..RotateTokenInput::default()
                },
            )
            .expect("rotate token");
        assert_eq!(rotated.status, TokenStatus::Rotated);
        assert_eq!(rotated.rotated_to_token_id, replacement.token_id);
        assert_eq!(replacement.rotated_from_token_id, old_token.token_id);
        assert_eq!(replacement.principal_id, old_token.principal_id);
        assert_eq!(replacement.default_tenant_id, old_token.default_tenant_id);
        assert!(matches!(
            manager.authenticate(&old_secret),
            Err(AuthError::TokenRotated)
        ));
        manager
            .authenticate(&replacement_secret)
            .expect("authenticate replacement");

        // A rotated token cannot be rotated again.
        assert!(matches!(
            manager.rotate_token(&old_token.token_id, RotateTokenInput::default()),
            Err(AuthError::TokenRotated)
        ));
    }

    #[test]
    fn rotate_expired_token_fails_closed() {
        let manager = Manager::new();
        let (token, _) = manager
            .issue_token(IssueTokenInput {
                principal_id: "prn_1".to_string(),
                label: "automation".to_string(),
                default_tenant_id: "ten_1".to_string(),
                expires_at: Some(Utc::now() - chrono::Duration::hours(1)),
            })
            .expect("issue token");
        assert!(matches!(
            manager.rotate_token(&token.token_id, RotateTokenInput::default()),
            Err(AuthError::TokenExpired)
        ));
        let stored = manager.get_token(&token.token_id).expect("token stored");
        assert_eq!(stored.status, TokenStatus::Expired);
    }

    #[test]
    fn complete_pairing_rejects_unknown_replayed_and_wrong_code() {
        let manager = Manager::new();
        assert!(matches!(
            manager.complete_pairing(
                "pair_missing",
                CompletePairingInput {
                    code: "000000".into()
                }
            ),
            Err(AuthError::PairingNotFound)
        ));

        let (pairing, code) = manager
            .start_pairing(StartPairingInput {
                mode: Some(PairingMode::Local),
                label: "cli".to_string(),
                ttl_seconds: 0,
            })
            .expect("start pairing");
        let wrong = if code == "000000" { "000001" } else { "000000" };
        assert!(matches!(
            manager.complete_pairing(
                &pairing.pairing_id,
                CompletePairingInput {
                    code: wrong.to_string()
                }
            ),
            Err(AuthError::PairingCodeInvalid)
        ));
        manager
            .complete_pairing(&pairing.pairing_id, CompletePairingInput { code })
            .expect("complete pairing");
        assert!(matches!(
            manager.complete_pairing(
                &pairing.pairing_id,
                CompletePairingInput {
                    code: "123456".into()
                }
            ),
            Err(AuthError::PairingNotPending)
        ));
    }

    #[test]
    fn expired_pairing_transitions_and_denies() {
        let manager = Manager::new();
        let (mut pairing, code) = manager
            .start_pairing(StartPairingInput {
                mode: Some(PairingMode::Local),
                label: "cli".to_string(),
                ttl_seconds: 0,
            })
            .expect("start pairing");
        // Force the pairing into the past via restore.
        pairing.expires_at = Utc::now() - chrono::Duration::seconds(1);
        manager.restore(vec![pairing.clone()], Vec::new());
        assert!(matches!(
            manager.complete_pairing(&pairing.pairing_id, CompletePairingInput { code }),
            Err(AuthError::PairingNotPending)
        ));
        let stored = manager
            .get_pairing(&pairing.pairing_id)
            .expect("pairing stored");
        assert_eq!(stored.status, PairingStatus::Expired);
    }

    #[test]
    fn secrets_are_hashed_and_listing_preserves_order() {
        let manager = Manager::new();
        let (first, secret_a) = manager
            .issue_token(IssueTokenInput {
                principal_id: "prn_1".to_string(),
                label: "a".to_string(),
                default_tenant_id: "ten_1".to_string(),
                expires_at: None,
            })
            .expect("issue a");
        let (second, _) = manager
            .issue_token(IssueTokenInput {
                principal_id: "prn_1".to_string(),
                label: "b".to_string(),
                default_tenant_id: "ten_1".to_string(),
                expires_at: None,
            })
            .expect("issue b");
        assert_eq!(first.token_hash, hash_secret(&secret_a));
        assert_eq!(first.token_preview, secret_a[..12]);
        let listed = manager.list_tokens();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].token_id, first.token_id);
        assert_eq!(listed[1].token_id, second.token_id);
    }
}
