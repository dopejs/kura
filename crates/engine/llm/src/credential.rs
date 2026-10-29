//! Which credential a dispatch goes out with.
//!
//! A provider is registered once, with the credential its operator configured.
//! On a shared daemon that credential belongs to the platform, not to the
//! tenant whose turn is dispatching: billing every tenant's tokens to it, or
//! letting a tenant spend the operator's subscription, is exactly what a
//! shared host must not do (shared-hosting.md §7). The dispatcher therefore
//! asks a [`CredentialPolicy`] once per dispatch, before the first attempt.
//! Without a policy every dispatch uses the provider's own credential, which
//! is today's single-owner behavior.

use std::fmt;

use futures::future::BoxFuture;

/// An API key supplied for one dispatch. Never printed: `Debug` is redacted,
/// because a `ProviderRequest` is `Debug` and ends up in logs.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl ApiKey {
    /// `None` for an empty or whitespace-only value: a blank key would go out
    /// as an anonymous request, which is never what the caller meant.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(Self(trimmed.to_string()))
    }

    /// The raw key, for the one place that puts it on the wire.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(<redacted>)")
    }
}

/// What a dispatch goes out with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialDecision {
    /// The provider's own, configured credential.
    ProviderDefault,
    /// This key instead. A provider that cannot take one fails the dispatch;
    /// it never falls back to its own credential.
    Key(ApiKey),
    /// Do not dispatch. `code` becomes the dispatch's `errorCode`; `message`
    /// is shown to the caller, so it must not carry secret material.
    Refuse { code: String, message: String },
}

/// Decides the credential for a dispatch from the tenant it runs for and the
/// provider it targets. An empty `tenant_id` is the daemon's own work (an
/// operator route, a probe), not a tenant's.
pub trait CredentialPolicy: Send + Sync {
    fn decide<'a>(
        &'a self,
        tenant_id: &'a str,
        provider: &'a str,
    ) -> BoxFuture<'a, CredentialDecision>;
}
