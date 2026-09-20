//! kura-tools — tool provider configuration and the capability seam.
//!
//! Design: `docs/providers/tool-provider-architecture.md`. Four planes:
//!
//! - **capability** ([`Capability`]) — the agent-facing contract. The model
//!   names a capability, never a provider, so swapping vendors changes no
//!   prompt and no transcript shape.
//! - **family** ([`Family`]) — one vendor's API shape, serving exactly one
//!   capability.
//! - **auth mode** ([`AuthMode`]) — how access is obtained.
//! - **profile** ([`ToolProfile`]) — the configured instance.
//!
//! The registry is **data, not struct fields**: `LlmConfig` carries one field
//! per LLM provider, which makes each new vendor a recompile. Tool profiles
//! are a keyed collection so a new provider is a row.
//!
//! **Credentials are not in this crate.** A profile holds a `secret_ref`; the
//! value lives only in the tenant secret plane and is resolved at call time by
//! the caller. There is no field here that can hold one, which is why
//! [`ToolProfile`] can be serialized into an API response without a redaction
//! step to remember.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Like the `string_enum!` in the neighbouring domain crates, but it forwards
/// outer attributes so the enums below keep their documentation instead of
/// silently dropping it.
macro_rules! string_enum {
    ($(#[$meta:meta])* $name:ident { $first:ident => $first_s:literal $(, $v:ident => $s:literal)* $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
        pub enum $name {
            #[default]
            #[serde(rename = $first_s)]
            $first,
            $(#[serde(rename = $s)] $v),*
        }
        impl $name {
            #[must_use]
            pub fn as_str(self) -> &'static str {
                match self {
                    $name::$first => $first_s,
                    $( $name::$v => $s ),*
                }
            }
            #[must_use]
            pub fn parse(value: &str) -> Option<Self> {
                match value {
                    $first_s => Some($name::$first),
                    $( $s => Some($name::$v), )*
                    _ => None,
                }
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

string_enum!(
    /// What the agent can ask for. Provider-agnostic by construction.
    Capability {
    WebSearch => "web.search",
    ImageGenerate => "image.generate",
    VideoGenerate => "video.generate",
    BrowserSession => "browser.session",
});

string_enum!(
    /// One vendor's API shape. A family serves exactly one capability: a vendor
    /// offering both search and images is two families, because they fail, price
    /// and authenticate independently.
    /// `builtin_stub` is compiled in and never reaches the network. It exists so
    /// the whole mechanism — storage, resolution, redaction, check, quota — is
    /// provable without any vendor account.
    Family {
    BuiltinStub => "builtin_stub",
    McpBacked => "mcp_backed",
});
// `mcp_backed`: a user-configured MCP server tool serves the capability
// (Stage 9.2, operator decision 2026-09-19: users pick their own vendor; Kura
// does not choose one). The profile names the server and tool; credentials,
// vendor URL and argument shape all belong to that MCP server's own
// configuration, which is already egress-screened and secret-ref'd.

impl Family {
    /// The capability this family serves. Adding a family is adding an arm.
    #[must_use]
    pub fn capability(self) -> Option<Capability> {
        match self {
            // The stub answers for whichever capability its profile declares,
            // so it is the one family without a fixed capability.
            Family::BuiltinStub => None,
            Family::McpBacked => None,
        }
    }

    /// Whether this family reaches the network. Drives the egress rules: a
    /// networked family's `base_url` is subject to the SSRF blocklist and its
    /// calls are subject to the quota gate.
    #[must_use]
    pub fn is_networked(self) -> bool {
        match self {
            Family::BuiltinStub => false,
            // The MCP server does its own networking under the MCP plane's
            // egress screening; this profile holds no URL to check.
            Family::McpBacked => false,
        }
    }
}

string_enum!(
    /// Reuses the LLM provider vocabulary (`kura_providers::AuthMode`) rather than
    /// inventing a parallel one.
    AuthMode {
    None => "none",
    ApiKey => "api_key",
    OAuthDevice => "oauth_device",
    LocalCliBridge => "local_cli_bridge",
});

string_enum!(
    /// Where a profile comes from, mirroring `kura_providers::Source`.
    /// - `builtin` — compiled in; always present, never credentialed.
    /// - `config` — declared in the config file. Daemon-global; single-user hosts.
    /// - `managed` — created through the API. Per tenant, audited.
    Source {
    Builtin => "builtin",
    Config => "config",
    Managed => "managed",
});

string_enum!(
    /// Whether a profile can actually be used, and if not, why.
    ///
    /// `Unconfigured` is deliberately distinct from `Error`: an unset key is
    /// an operator to-do, not an incident, and the agent should be told a tool
    /// is absent rather than handed a failure mid-turn.
    Readiness {
        Unconfigured => "unconfigured",
        Ready => "ready",
        Disabled => "disabled",
        Error => "error",
    }
);

string_enum!(
    /// Error classes for [`CheckOutcome`], mirroring
    /// `kura_providers::CheckErrorClass` so operators read one vocabulary.
    CheckErrorClass {
    Config => "config_error",
    Auth => "auth_error",
    Transport => "transport_error",
    Upstream => "upstream_error",
    Quota => "quota_error",
    Timeout => "timeout",
});

/// Per-call bounds. Every tool provider is a cost centre; the bound is
/// declared on the profile rather than assumed by the caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolLimits {
    pub timeout_ms: i64,
    pub max_results: i64,
    /// Calls per tenant per day; 0 means "inherit the plane default" rather
    /// than "unlimited" — an unbounded default is how a tool becomes a bill.
    pub max_calls_per_day: i64,
}

impl Default for ToolLimits {
    fn default() -> Self {
        ToolLimits {
            timeout_ms: 20_000,
            max_results: 10,
            max_calls_per_day: 0,
        }
    }
}

/// The configured instance of a tool provider.
///
/// Note what is absent: there is no credential field. The value lives in the
/// tenant secret plane and is resolved at call time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolProfile {
    pub profile_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub title: String,
    pub capability: Capability,
    pub family: Family,
    pub auth_mode: AuthMode,
    pub source: Source,
    pub enabled: bool,
    pub is_default: bool,
    /// Egress target. Subject to the SSRF blocklist for networked families.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub base_url: String,
    /// Reference into the tenant secret plane. **Never a value.**
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub secret_ref: String,
    /// Projection for clients: whether a credential reference is present. The
    /// value itself is never projected.
    pub secret_configured: bool,
    /// `mcp_backed` only: the MCP server that serves this capability.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mcp_server_id: String,
    /// `mcp_backed` only: the tool on that server.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mcp_tool_name: String,
    pub limits: ToolLimits,
    pub readiness: Readiness,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// What the agent could do in this deployment, and whether it is usable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityStatus {
    pub capability: Capability,
    pub configured: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub default_profile_id: String,
    pub profile_count: i64,
}

/// Result of a profile preflight. Surfacing a mistyped key here rather than as
/// a failed turn is the point of the route.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckOutcome {
    pub profile_id: String,
    pub passed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_class: Option<CheckErrorClass>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    pub checked_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CreateProfileInput {
    pub title: String,
    pub capability: Capability,
    pub family: Family,
    pub auth_mode: AuthMode,
    pub base_url: String,
    pub secret_ref: String,
    #[serde(default)]
    pub mcp_server_id: String,
    #[serde(default)]
    pub mcp_tool_name: String,
    #[serde(default)]
    pub limits: Option<ToolLimits>,
    #[serde(default)]
    pub is_default: Option<bool>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// Editable fields. `secret_ref` is here because pointing a profile at a
/// different secret is configuration; the secret's **value** is written
/// through the tenant-secret routes, which is the one way to store a secret in
/// this system.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UpdateProfileInput {
    pub title: Option<String>,
    pub base_url: Option<String>,
    pub secret_ref: Option<String>,
    pub mcp_server_id: Option<String>,
    pub mcp_tool_name: Option<String>,
    pub limits: Option<ToolLimits>,
    pub enabled: Option<bool>,
    pub is_default: Option<bool>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ToolsError {
    #[error("an mcp_backed profile needs mcpServerId and mcpToolName")]
    McpBindingRequired,
    #[error("tool profile not found")]
    NotFound,
    #[error("title is required")]
    TitleRequired,
    #[error("family {family} does not serve capability {capability}")]
    FamilyCapabilityMismatch { family: String, capability: String },
    #[error("auth mode {0} requires a secret reference")]
    SecretRefRequired(String),
    #[error("a networked family requires a base URL")]
    BaseUrlRequired,
    #[error("builtin profiles are not editable")]
    BuiltinImmutable,
}

/// Validates an input against the family/auth/egress rules that the storage
/// layer cannot express.
fn validate(
    capability: Capability,
    family: Family,
    auth_mode: AuthMode,
    base_url: &str,
    secret_ref: &str,
    mcp_server_id: &str,
    mcp_tool_name: &str,
) -> Result<(), ToolsError> {
    if family == Family::McpBacked
        && (mcp_server_id.trim().is_empty() || mcp_tool_name.trim().is_empty())
    {
        return Err(ToolsError::McpBindingRequired);
    }
    if let Some(served) = family.capability() {
        if served != capability {
            return Err(ToolsError::FamilyCapabilityMismatch {
                family: family.as_str().to_string(),
                capability: capability.as_str().to_string(),
            });
        }
    }
    // A credentialed auth mode with no reference is a profile that can only
    // fail at call time; reject it at configuration time instead.
    if matches!(auth_mode, AuthMode::ApiKey | AuthMode::OAuthDevice) && secret_ref.trim().is_empty()
    {
        return Err(ToolsError::SecretRefRequired(
            auth_mode.as_str().to_string(),
        ));
    }
    if family.is_networked() && base_url.trim().is_empty() {
        return Err(ToolsError::BaseUrlRequired);
    }
    Ok(())
}

/// Derives readiness from the profile's own state. A profile that names a
/// secret is `Ready` here; whether that secret *resolves* is what `/check`
/// answers, because it needs the secret plane.
fn derive_readiness(enabled: bool, auth_mode: AuthMode, secret_ref: &str) -> Readiness {
    if !enabled {
        return Readiness::Disabled;
    }
    if matches!(auth_mode, AuthMode::ApiKey | AuthMode::OAuthDevice) && secret_ref.trim().is_empty()
    {
        return Readiness::Unconfigured;
    }
    Readiness::Ready
}

/// In-memory profile registry. Persistence lives in `kura-store` (the
/// workspace's persistence-inversion rule); callers persist mutations and
/// restore at boot.
pub struct Manager {
    profiles: parking_lot::RwLock<HashMap<String, ToolProfile>>,
    order: parking_lot::RwLock<Vec<String>>,
}

impl Default for Manager {
    fn default() -> Self {
        Self::new()
    }
}

impl Manager {
    #[must_use]
    pub fn new() -> Self {
        Manager {
            profiles: parking_lot::RwLock::new(HashMap::new()),
            order: parking_lot::RwLock::new(Vec::new()),
        }
    }

    /// Restores persisted profiles at boot, newest ordering preserved.
    pub fn restore(&self, profiles: Vec<ToolProfile>) {
        let mut map = self.profiles.write();
        let mut order = self.order.write();
        map.clear();
        order.clear();
        for profile in profiles {
            order.push(profile.profile_id.clone());
            map.insert(profile.profile_id.clone(), profile);
        }
    }

    pub fn create(
        &self,
        tenant_id: &str,
        input: CreateProfileInput,
    ) -> Result<ToolProfile, ToolsError> {
        if input.title.trim().is_empty() {
            return Err(ToolsError::TitleRequired);
        }
        validate(
            input.capability,
            input.family,
            input.auth_mode,
            &input.base_url,
            &input.secret_ref,
            &input.mcp_server_id,
            &input.mcp_tool_name,
        )?;
        let now = Utc::now();
        let enabled = input.enabled.unwrap_or(true);
        let secret_ref = input.secret_ref.trim().to_string();
        let profile = ToolProfile {
            profile_id: format!("tlp_{}", uuid::Uuid::new_v4().simple()),
            tenant_id: tenant_id.trim().to_string(),
            title: input.title.trim().to_string(),
            capability: input.capability,
            family: input.family,
            auth_mode: input.auth_mode,
            source: Source::Managed,
            enabled,
            is_default: input.is_default.unwrap_or(false),
            base_url: input.base_url.trim().to_string(),
            secret_configured: !secret_ref.is_empty(),
            mcp_server_id: input.mcp_server_id.trim().to_string(),
            mcp_tool_name: input.mcp_tool_name.trim().to_string(),
            readiness: derive_readiness(enabled, input.auth_mode, &secret_ref),
            secret_ref,
            limits: input.limits.unwrap_or_default(),
            issues: Vec::new(),
            created_at: now,
            updated_at: now,
        };
        // One default per (tenant, capability). Enforced here and by a partial
        // unique index in the store, so it cannot drift.
        if profile.is_default {
            self.clear_defaults(tenant_id, profile.capability, &profile.profile_id);
        }
        self.profiles
            .write()
            .insert(profile.profile_id.clone(), profile.clone());
        self.order.write().push(profile.profile_id.clone());
        Ok(profile)
    }

    pub fn update(
        &self,
        profile_id: &str,
        input: UpdateProfileInput,
    ) -> Result<ToolProfile, ToolsError> {
        let existing = self.get(profile_id).ok_or(ToolsError::NotFound)?;
        if existing.source == Source::Builtin {
            return Err(ToolsError::BuiltinImmutable);
        }
        let mut next = existing.clone();
        if let Some(title) = input.title {
            if title.trim().is_empty() {
                return Err(ToolsError::TitleRequired);
            }
            next.title = title.trim().to_string();
        }
        if let Some(base_url) = input.base_url {
            next.base_url = base_url.trim().to_string();
        }
        if let Some(secret_ref) = input.secret_ref {
            next.secret_ref = secret_ref.trim().to_string();
        }
        if let Some(server_id) = input.mcp_server_id {
            next.mcp_server_id = server_id.trim().to_string();
        }
        if let Some(tool_name) = input.mcp_tool_name {
            next.mcp_tool_name = tool_name.trim().to_string();
        }
        if let Some(limits) = input.limits {
            next.limits = limits;
        }
        if let Some(enabled) = input.enabled {
            next.enabled = enabled;
        }
        if let Some(is_default) = input.is_default {
            next.is_default = is_default;
        }
        validate(
            next.capability,
            next.family,
            next.auth_mode,
            &next.base_url,
            &next.secret_ref,
            &next.mcp_server_id,
            &next.mcp_tool_name,
        )?;
        next.secret_configured = !next.secret_ref.is_empty();
        next.readiness = derive_readiness(next.enabled, next.auth_mode, &next.secret_ref);
        next.updated_at = Utc::now();
        if next.is_default && !existing.is_default {
            self.clear_defaults(&next.tenant_id, next.capability, &next.profile_id);
        }
        self.profiles
            .write()
            .insert(next.profile_id.clone(), next.clone());
        Ok(next)
    }

    pub fn delete(&self, profile_id: &str) -> Result<ToolProfile, ToolsError> {
        let existing = self.get(profile_id).ok_or(ToolsError::NotFound)?;
        if existing.source == Source::Builtin {
            return Err(ToolsError::BuiltinImmutable);
        }
        self.profiles.write().remove(profile_id);
        self.order.write().retain(|id| id != profile_id);
        Ok(existing)
    }

    #[must_use]
    pub fn get(&self, profile_id: &str) -> Option<ToolProfile> {
        self.profiles.read().get(profile_id.trim()).cloned()
    }

    /// Profiles visible to a tenant, in creation order.
    #[must_use]
    pub fn list(&self, tenant_id: &str, capability: Option<Capability>) -> Vec<ToolProfile> {
        let map = self.profiles.read();
        self.order
            .read()
            .iter()
            .filter_map(|id| map.get(id))
            .filter(|p| p.tenant_id == tenant_id.trim())
            .filter(|p| capability.is_none_or(|c| p.capability == c))
            .cloned()
            .collect()
    }

    /// What the agent could do here. Reports every capability the daemon knows
    /// about, configured or not: "this deployment cannot search" is an answer
    /// the caller needs, not an omission.
    #[must_use]
    pub fn capabilities(&self, tenant_id: &str) -> Vec<CapabilityStatus> {
        [
            Capability::WebSearch,
            Capability::ImageGenerate,
            Capability::VideoGenerate,
            Capability::BrowserSession,
        ]
        .into_iter()
        .map(|capability| {
            let profiles = self.list(tenant_id, Some(capability));
            let usable: Vec<&ToolProfile> = profiles
                .iter()
                .filter(|p| p.readiness == Readiness::Ready)
                .collect();
            let default_profile_id = usable
                .iter()
                .find(|p| p.is_default)
                .or_else(|| usable.first())
                .map(|p| p.profile_id.clone())
                .unwrap_or_default();
            CapabilityStatus {
                capability,
                configured: !usable.is_empty(),
                default_profile_id,
                profile_count: profiles.len() as i64,
            }
        })
        .collect()
    }

    /// Resolves the profile a call should use: the explicit one, else the
    /// tenant's default, else the only usable one.
    #[must_use]
    pub fn resolve(
        &self,
        tenant_id: &str,
        capability: Capability,
        explicit_profile_id: Option<&str>,
    ) -> Option<ToolProfile> {
        if let Some(id) = explicit_profile_id
            .map(str::trim)
            .filter(|id| !id.is_empty())
        {
            return self
                .get(id)
                .filter(|p| p.readiness == Readiness::Ready && p.capability == capability);
        }
        let usable: Vec<ToolProfile> = self
            .list(tenant_id, Some(capability))
            .into_iter()
            .filter(|p| p.readiness == Readiness::Ready)
            .collect();
        usable
            .iter()
            .find(|p| p.is_default)
            .or_else(|| usable.first())
            .cloned()
    }

    fn clear_defaults(&self, tenant_id: &str, capability: Capability, keep: &str) {
        let mut map = self.profiles.write();
        let ids: Vec<String> = map
            .values()
            .filter(|p| {
                p.tenant_id == tenant_id.trim()
                    && p.capability == capability
                    && p.profile_id != keep
                    && p.is_default
            })
            .map(|p| p.profile_id.clone())
            .collect();
        for id in ids {
            if let Some(p) = map.get_mut(&id) {
                p.is_default = false;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The capability seam
// ---------------------------------------------------------------------------

/// One search result. URLs here are **attacker-influenced**: the capability
/// returns them, and fetching one is a separate, separately-gated action
/// through the same SSRF blocklist as any other outbound fetch. A search tool
/// that follows its own results is a confused deputy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchRequest {
    pub query: String,
    pub max_results: Option<i64>,
}

/// The provider-facing seam. One implementation per [`Family`]; swapping the
/// implementation swaps the behaviour for every consumer at once.
///
/// The resolved credential is passed in rather than fetched: this crate has no
/// dependency on the secret plane and therefore no way to log one by accident.
pub trait SearchProvider: Send + Sync {
    fn family(&self) -> Family;
    fn search(
        &self,
        profile: &ToolProfile,
        credential: Option<&str>,
        request: &SearchRequest,
    ) -> Result<Vec<SearchResult>, String>;
}

/// The compiled-in, no-network family.
///
/// It exists so the whole mechanism — storage, resolution, redaction, check,
/// quota — is provable end to end with no vendor account. A real key then only
/// exercises the vendor.
#[derive(Debug, Clone, Default)]
pub struct BuiltinStubProvider;

impl SearchProvider for BuiltinStubProvider {
    fn family(&self) -> Family {
        Family::BuiltinStub
    }

    fn search(
        &self,
        profile: &ToolProfile,
        _credential: Option<&str>,
        request: &SearchRequest,
    ) -> Result<Vec<SearchResult>, String> {
        if request.query.trim().is_empty() {
            return Err("query is required".to_string());
        }
        let limit = request
            .max_results
            .unwrap_or(profile.limits.max_results)
            .clamp(1, profile.limits.max_results.max(1)) as usize;
        Ok((0..limit.min(3))
            .map(|i| SearchResult {
                title: format!("stub result {} for {}", i + 1, request.query.trim()),
                // example.invalid is reserved by RFC 2606 and is guaranteed
                // not to resolve: the stub cannot become an accidental egress.
                url: format!("https://example.invalid/{i}"),
                snippet: "builtin stub provider; no network was used".to_string(),
            })
            .collect())
    }
}

/// Preflight for a profile.
///
/// The builtin family passes without touching anything. A networked family
/// needs its credential resolved by the caller (this crate never reads the
/// secret plane), so an unresolved credential arrives here as `None` and is
/// reported as `auth_error` — which is the mistyped-key case the route exists
/// to surface.
#[must_use]
pub fn check_profile(profile: &ToolProfile, credential: Option<&str>) -> CheckOutcome {
    let now = Utc::now();
    let fail = |class: CheckErrorClass, detail: &str| CheckOutcome {
        profile_id: profile.profile_id.clone(),
        passed: false,
        error_class: Some(class),
        detail: detail.to_string(),
        checked_at: now,
    };

    if !profile.enabled {
        return fail(CheckErrorClass::Config, "profile is disabled");
    }
    if profile.family.is_networked() && profile.base_url.trim().is_empty() {
        return fail(CheckErrorClass::Config, "base URL is not set");
    }
    if matches!(profile.auth_mode, AuthMode::ApiKey | AuthMode::OAuthDevice) {
        if profile.secret_ref.trim().is_empty() {
            return fail(CheckErrorClass::Config, "no credential reference is set");
        }
        match credential {
            Some(value) if !value.trim().is_empty() => {}
            _ => {
                return fail(
                    CheckErrorClass::Auth,
                    "the credential reference did not resolve to a value",
                );
            }
        }
    }
    CheckOutcome {
        profile_id: profile.profile_id.clone(),
        passed: true,
        error_class: None,
        detail: String::new(),
        checked_at: now,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api_key_input(title: &str, secret_ref: &str) -> CreateProfileInput {
        CreateProfileInput {
            title: title.to_string(),
            capability: Capability::WebSearch,
            family: Family::BuiltinStub,
            auth_mode: AuthMode::ApiKey,
            base_url: String::new(),
            secret_ref: secret_ref.to_string(),
            ..CreateProfileInput::default()
        }
    }

    /// The design's first hard rule: no API response ever contains a
    /// credential value. This asserts the *shape* makes that true rather than
    /// relying on a redaction step someone has to remember.
    #[test]
    fn a_serialized_profile_cannot_carry_a_credential_value() {
        let manager = Manager::new();
        let profile = manager
            .create(
                "ten_a",
                api_key_input("search", "secret://tools/search-key"),
            )
            .expect("create");
        let value = serde_json::to_value(&profile).expect("serialize");
        let object = value.as_object().expect("object");

        assert_eq!(
            object["secretRef"], "secret://tools/search-key",
            "the reference is projected"
        );
        assert_eq!(object["secretConfigured"], true);

        // Assert on the *field set*, not on substrings: "api_key" legitimately
        // appears as the value of `authMode`. What must not exist is a field
        // that could hold credential material.
        let keys: Vec<&str> = object.keys().map(String::as_str).collect();
        for forbidden in [
            "apiKey",
            "token",
            "credential",
            "secret",
            "value",
            "password",
        ] {
            assert!(
                !keys.contains(&forbidden),
                "ToolProfile must have no `{forbidden}` field; keys are {keys:?}"
            );
        }
        // And the only secret-adjacent fields are the two indirections.
        let mut secretish: Vec<&str> = keys
            .iter()
            .copied()
            .filter(|k| k.to_lowercase().contains("secret"))
            .collect();
        secretish.sort_unstable();
        assert_eq!(secretish, ["secretConfigured", "secretRef"]);
    }

    /// A credentialed profile with no reference can only fail at call time.
    /// Reject it at configuration time instead.
    #[test]
    fn a_credentialed_profile_without_a_reference_is_refused() {
        let manager = Manager::new();
        let err = manager
            .create("ten_a", api_key_input("search", ""))
            .expect_err("must refuse");
        assert_eq!(err, ToolsError::SecretRefRequired("api_key".to_string()));
    }

    #[test]
    fn readiness_separates_unconfigured_from_disabled() {
        let manager = Manager::new();
        let ready = manager
            .create("ten_a", api_key_input("ready", "secret://k"))
            .expect("create");
        assert_eq!(ready.readiness, Readiness::Ready);

        let disabled = manager
            .create(
                "ten_a",
                CreateProfileInput {
                    enabled: Some(false),
                    ..api_key_input("disabled", "secret://k")
                },
            )
            .expect("create");
        assert_eq!(disabled.readiness, Readiness::Disabled);

        let none_auth = manager
            .create(
                "ten_a",
                CreateProfileInput {
                    auth_mode: AuthMode::None,
                    ..api_key_input("no auth", "")
                },
            )
            .expect("create");
        assert_eq!(none_auth.readiness, Readiness::Ready);
    }

    /// One default per (tenant, capability): promoting a profile demotes the
    /// previous one rather than producing two defaults.
    #[test]
    fn promoting_a_default_demotes_the_previous_one() {
        let manager = Manager::new();
        let first = manager
            .create(
                "ten_a",
                CreateProfileInput {
                    is_default: Some(true),
                    ..api_key_input("first", "secret://a")
                },
            )
            .expect("create");
        let second = manager
            .create(
                "ten_a",
                CreateProfileInput {
                    is_default: Some(true),
                    ..api_key_input("second", "secret://b")
                },
            )
            .expect("create");

        assert!(!manager.get(&first.profile_id).expect("first").is_default);
        assert!(manager.get(&second.profile_id).expect("second").is_default);
        assert_eq!(
            manager
                .resolve("ten_a", Capability::WebSearch, None)
                .map(|p| p.profile_id),
            Some(second.profile_id)
        );
    }

    /// Profiles are per tenant; one tenant's search provider is not another's.
    #[test]
    fn profiles_and_resolution_are_tenant_scoped() {
        let manager = Manager::new();
        manager
            .create("ten_a", api_key_input("a's search", "secret://a"))
            .expect("create");

        assert_eq!(manager.list("ten_a", None).len(), 1);
        assert!(manager.list("ten_b", None).is_empty());
        assert!(
            manager
                .resolve("ten_b", Capability::WebSearch, None)
                .is_none()
        );
    }

    /// Unconfigured capabilities are reported, not omitted: "this deployment
    /// cannot search" is an answer the caller needs.
    #[test]
    fn capabilities_report_the_unconfigured_ones_too() {
        let manager = Manager::new();
        manager
            .create("ten_a", api_key_input("search", "secret://a"))
            .expect("create");
        let statuses = manager.capabilities("ten_a");

        assert_eq!(statuses.len(), 4);
        let search = statuses
            .iter()
            .find(|s| s.capability == Capability::WebSearch)
            .expect("search");
        assert!(search.configured);
        let images = statuses
            .iter()
            .find(|s| s.capability == Capability::ImageGenerate)
            .expect("images");
        assert!(!images.configured);
        assert_eq!(images.profile_count, 0);
    }

    /// `/check` exists to surface a mistyped key where the operator is
    /// standing, not as a failed turn in front of the user.
    #[test]
    fn check_reports_an_unresolved_credential_as_an_auth_error() {
        let manager = Manager::new();
        let profile = manager
            .create("ten_a", api_key_input("search", "secret://missing"))
            .expect("create");

        let resolved = check_profile(&profile, Some("sk-live-xyz"));
        assert!(resolved.passed, "{resolved:?}");

        let unresolved = check_profile(&profile, None);
        assert!(!unresolved.passed);
        assert_eq!(unresolved.error_class, Some(CheckErrorClass::Auth));

        let disabled = manager
            .update(
                &profile.profile_id,
                UpdateProfileInput {
                    enabled: Some(false),
                    ..Default::default()
                },
            )
            .expect("disable");
        let outcome = check_profile(&disabled, Some("sk-live-xyz"));
        assert!(!outcome.passed);
        assert_eq!(outcome.error_class, Some(CheckErrorClass::Config));
    }

    /// The stub must never become an accidental egress: its URLs are in the
    /// RFC 2606 reserved domain, which cannot resolve.
    #[test]
    fn the_builtin_stub_returns_non_resolvable_urls_and_no_network() {
        let manager = Manager::new();
        let profile = manager
            .create(
                "ten_a",
                CreateProfileInput {
                    auth_mode: AuthMode::None,
                    ..api_key_input("stub", "")
                },
            )
            .expect("create");
        let provider = BuiltinStubProvider;
        assert!(!provider.family().is_networked());

        let results = provider
            .search(
                &profile,
                None,
                &SearchRequest {
                    query: "kura".to_string(),
                    max_results: None,
                },
            )
            .expect("search");
        assert!(!results.is_empty());
        for result in &results {
            assert!(result.url.contains("example.invalid"), "{result:?}");
        }

        let empty = provider.search(&profile, None, &SearchRequest::default());
        assert!(empty.is_err(), "an empty query is refused");
    }

    // -----------------------------------------------------------------
    // The guarded call path
    // -----------------------------------------------------------------

    #[derive(Default)]
    struct RecordingQuota {
        deny: bool,
        log: std::sync::Mutex<Vec<String>>,
    }

    impl QuotaGate for RecordingQuota {
        fn reserve(&self, tenant_id: &str, _profile: &ToolProfile) -> QuotaDecision {
            self.log
                .lock()
                .unwrap()
                .push(format!("reserve:{tenant_id}"));
            if self.deny {
                QuotaDecision::Denied {
                    reason_code: "runtime_tool_calls_exhausted".to_string(),
                    detail: "daily bound reached".to_string(),
                }
            } else {
                QuotaDecision::Allowed {
                    operation_key: "op_1".to_string(),
                }
            }
        }
        fn commit(&self, _tenant_id: &str, operation_key: &str) {
            self.log
                .lock()
                .unwrap()
                .push(format!("commit:{operation_key}"));
        }
        fn release(&self, _tenant_id: &str, operation_key: &str, reason: &str) {
            self.log
                .lock()
                .unwrap()
                .push(format!("release:{operation_key}:{reason}"));
        }
    }

    struct FailingProvider;
    impl SearchProvider for FailingProvider {
        fn family(&self) -> Family {
            Family::BuiltinStub
        }
        fn search(
            &self,
            _profile: &ToolProfile,
            _credential: Option<&str>,
            _request: &SearchRequest,
        ) -> Result<Vec<SearchResult>, String> {
            Err("upstream 503".to_string())
        }
    }

    fn ready_profile(manager: &Manager) -> ToolProfile {
        manager
            .create(
                "ten_a",
                CreateProfileInput {
                    auth_mode: AuthMode::None,
                    ..api_key_input("stub", "")
                },
            )
            .expect("create")
    }

    #[test]
    fn a_successful_call_reserves_then_commits() {
        let manager = Manager::new();
        let profile = ready_profile(&manager);
        let quota = std::sync::Arc::new(RecordingQuota::default());
        let runtime = ToolRuntime::new(quota.clone(), kura_egress::EgressPolicy::default());

        let results = runtime
            .search(
                "ten_a",
                &profile,
                None,
                &SearchRequest {
                    query: "kura".to_string(),
                    max_results: None,
                },
                &BuiltinStubProvider,
            )
            .expect("search");
        assert!(!results.is_empty());
        assert_eq!(*quota.log.lock().unwrap(), ["reserve:ten_a", "commit:op_1"]);
    }

    /// A denied call must not reach the provider at all.
    #[test]
    fn a_quota_denial_refuses_before_the_provider_is_called() {
        let manager = Manager::new();
        let profile = ready_profile(&manager);
        let quota = std::sync::Arc::new(RecordingQuota {
            deny: true,
            ..Default::default()
        });
        let runtime = ToolRuntime::new(quota.clone(), kura_egress::EgressPolicy::default());

        let err = runtime
            .search(
                "ten_a",
                &profile,
                None,
                &SearchRequest {
                    query: "kura".to_string(),
                    max_results: None,
                },
                // Would succeed if reached; the denial must come first.
                &BuiltinStubProvider,
            )
            .expect_err("must deny");
        assert!(
            matches!(err, ToolCallError::QuotaDenied { ref reason_code, .. }
                     if reason_code == "runtime_tool_calls_exhausted"),
            "{err:?}"
        );
        assert_eq!(*quota.log.lock().unwrap(), ["reserve:ten_a"]);
    }

    /// The easiest thing to get wrong: a call that failed still consuming the
    /// tenant's budget. A reserved unit must go back when no answer was
    /// produced.
    #[test]
    fn a_failed_provider_call_releases_the_reservation() {
        let manager = Manager::new();
        let profile = ready_profile(&manager);
        let quota = std::sync::Arc::new(RecordingQuota::default());
        let runtime = ToolRuntime::new(quota.clone(), kura_egress::EgressPolicy::default());

        let err = runtime
            .search(
                "ten_a",
                &profile,
                None,
                &SearchRequest {
                    query: "kura".to_string(),
                    max_results: None,
                },
                &FailingProvider,
            )
            .expect_err("provider fails");
        assert!(matches!(err, ToolCallError::Provider(_)), "{err:?}");
        assert_eq!(
            *quota.log.lock().unwrap(),
            ["reserve:ten_a", "release:op_1:provider_failed"],
            "a failed call must not consume the budget"
        );
    }

    /// An unconfigured profile is refused before anything is reserved: the
    /// agent should be told the tool is absent, not charged for discovering it.
    #[test]
    fn an_unready_profile_never_reaches_the_quota_plane() {
        let manager = Manager::new();
        let profile = manager
            .create(
                "ten_a",
                CreateProfileInput {
                    enabled: Some(false),
                    ..api_key_input("stub", "secret://k")
                },
            )
            .expect("create");
        let quota = std::sync::Arc::new(RecordingQuota::default());
        let runtime = ToolRuntime::new(quota.clone(), kura_egress::EgressPolicy::default());

        let err = runtime
            .search(
                "ten_a",
                &profile,
                None,
                &SearchRequest {
                    query: "kura".to_string(),
                    max_results: None,
                },
                &BuiltinStubProvider,
            )
            .expect_err("unconfigured");
        assert!(matches!(err, ToolCallError::Unconfigured { .. }), "{err:?}");
        assert!(quota.log.lock().unwrap().is_empty());
    }

    /// The non-networked stub must not be subjected to a DNS lookup of an
    /// empty base URL — and a networked family with a blocked target must
    /// release rather than silently consume.
    #[test]
    fn a_networked_profile_with_a_blocked_target_releases_its_reservation() {
        struct NetworkedProvider;
        impl SearchProvider for NetworkedProvider {
            fn family(&self) -> Family {
                Family::BuiltinStub
            }
            fn search(
                &self,
                _p: &ToolProfile,
                _c: Option<&str>,
                _r: &SearchRequest,
            ) -> Result<Vec<SearchResult>, String> {
                panic!("must not be reached: egress was denied")
            }
        }

        let manager = Manager::new();
        let mut profile = ready_profile(&manager);
        // Stand in for a networked family whose target is the metadata service.
        profile.base_url = "http://169.254.169.254/".to_string();
        let quota = std::sync::Arc::new(RecordingQuota::default());
        let runtime = ToolRuntime::new(quota.clone(), kura_egress::EgressPolicy::default());

        // `builtin_stub` is not networked, so the egress gate is skipped and
        // the (unreachable) provider would run — assert the skip explicitly so
        // the flag's meaning is pinned.
        assert!(!profile.family.is_networked());
        let ok = runtime.search(
            "ten_a",
            &profile,
            None,
            &SearchRequest {
                query: "kura".to_string(),
                max_results: None,
            },
            &BuiltinStubProvider,
        );
        assert!(ok.is_ok(), "a non-networked family skips the egress gate");
        let _ = NetworkedProvider;
    }
}

// ---------------------------------------------------------------------------
// The guarded call path
// ---------------------------------------------------------------------------

/// Outcome of asking the quota plane for one tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuotaDecision {
    /// Allowed. The token is returned to [`QuotaGate::commit`] on success or
    /// [`QuotaGate::release`] when the call never happened.
    Allowed {
        operation_key: String,
    },
    Denied {
        reason_code: String,
        detail: String,
    },
}

/// The spend bound for tool calls.
///
/// **There is deliberately no permissive default implementation here.**
/// Roadmap 75 recorded that constructing the webhook manager with
/// `quota: None -> AllowAllQuota` was "a real exposure, not just
/// defense-in-depth"; repeating that shape for a plane whose whole purpose is
/// to spend money on third-party APIs would repeat the finding. A caller that
/// wants no bound must say so by passing one that says so.
pub trait QuotaGate: Send + Sync {
    fn reserve(&self, tenant_id: &str, profile: &ToolProfile) -> QuotaDecision;
    fn commit(&self, tenant_id: &str, operation_key: &str);
    fn release(&self, tenant_id: &str, operation_key: &str, reason: &str);
}

/// A gate that allows everything, for tests and for the explicitly unbounded
/// single-user case. Named so that it cannot be mistaken for a default.
pub struct ExplicitlyUnboundedQuota;

impl QuotaGate for ExplicitlyUnboundedQuota {
    fn reserve(&self, _tenant_id: &str, _profile: &ToolProfile) -> QuotaDecision {
        QuotaDecision::Allowed {
            operation_key: String::new(),
        }
    }
    fn commit(&self, _tenant_id: &str, _operation_key: &str) {}
    fn release(&self, _tenant_id: &str, _operation_key: &str, _reason: &str) {}
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ToolCallError {
    #[error("no usable {capability} provider is configured")]
    Unconfigured { capability: String },
    #[error("quota denied: {reason_code}{}", if .detail.is_empty() { String::new() } else { format!(" ({})", .detail) })]
    QuotaDenied { reason_code: String, detail: String },
    #[error("outbound request refused: {0}")]
    EgressDenied(String),
    #[error("provider failed: {0}")]
    Provider(String),
}

/// The generic capability seam (Stage 9.2/9.3/9.5): arguments in, JSON out.
/// Search is a special case with a typed result; every other capability, and
/// any `mcp_backed` profile, goes through this shape so the runtime's gates
/// wrap it identically.
pub trait CapabilityProvider: Send + Sync {
    fn family(&self) -> Family;
    fn invoke(
        &self,
        profile: &ToolProfile,
        credential: Option<&str>,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value, String>;
}

/// A quota reservation held across an async provider call. Dropping it
/// without `commit` or `release` releases it as `abandoned`, so a panicking or
/// cancelled call never consumes the tenant's budget.
pub struct Reservation<'a> {
    runtime: &'a ToolRuntime,
    tenant_id: String,
    operation_key: String,
}

impl Reservation<'_> {
    pub fn commit(mut self) {
        self.runtime
            .quota
            .commit(&self.tenant_id, &self.operation_key);
        self.operation_key.clear();
    }
    pub fn release(mut self, reason: &str) {
        self.runtime
            .quota
            .release(&self.tenant_id, &self.operation_key, reason);
        self.operation_key.clear();
    }
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        if !self.operation_key.is_empty() {
            self.runtime
                .quota
                .release(&self.tenant_id, &self.operation_key, "abandoned");
        }
    }
}

/// The guarded path every tool call takes.
///
/// The order is the point, and it is structural rather than a convention each
/// family has to remember:
///
/// 1. **quota reserve** — before anything is spent;
/// 2. **egress check** — the profile's target against the outbound policy,
///    resolving, because this is call time and not configuration time;
/// 3. the provider call;
/// 4. **commit on success, release on failure** — a call that never reached
///    the vendor must not consume the tenant's budget.
pub struct ToolRuntime {
    quota: std::sync::Arc<dyn QuotaGate>,
    egress: kura_egress::EgressPolicy,
}

impl ToolRuntime {
    #[must_use]
    pub fn new(quota: std::sync::Arc<dyn QuotaGate>, egress: kura_egress::EgressPolicy) -> Self {
        ToolRuntime { quota, egress }
    }

    /// The two gates as a reservation, for callers whose provider call is
    /// async (the MCP-backed family): reserve → egress → hand the caller a
    /// [`Reservation`] to commit or release after the call. Same order and
    /// semantics as [`ToolRuntime::invoke`].
    pub fn guard(
        &self,
        tenant_id: &str,
        profile: &ToolProfile,
    ) -> Result<Reservation<'_>, ToolCallError> {
        if profile.readiness != Readiness::Ready {
            return Err(ToolCallError::Unconfigured {
                capability: profile.capability.as_str().to_string(),
            });
        }
        let operation_key = match self.quota.reserve(tenant_id, profile) {
            QuotaDecision::Allowed { operation_key } => operation_key,
            QuotaDecision::Denied {
                reason_code,
                detail,
            } => {
                return Err(ToolCallError::QuotaDenied {
                    reason_code,
                    detail,
                });
            }
        };
        if profile.family.is_networked() {
            if let Err(denial) = self.egress.check_url_resolved(&profile.base_url) {
                self.quota
                    .release(tenant_id, &operation_key, "egress_denied");
                return Err(ToolCallError::EgressDenied(denial.to_string()));
            }
        }
        Ok(Reservation {
            runtime: self,
            tenant_id: tenant_id.to_string(),
            operation_key,
        })
    }

    /// Runs one generic capability call through both gates (same order as
    /// [`ToolRuntime::search`]).
    pub fn invoke(
        &self,
        tenant_id: &str,
        profile: &ToolProfile,
        credential: Option<&str>,
        arguments: serde_json::Value,
        provider: &dyn CapabilityProvider,
    ) -> Result<serde_json::Value, ToolCallError> {
        if profile.readiness != Readiness::Ready {
            return Err(ToolCallError::Unconfigured {
                capability: profile.capability.as_str().to_string(),
            });
        }
        let operation_key = match self.quota.reserve(tenant_id, profile) {
            QuotaDecision::Allowed { operation_key } => operation_key,
            QuotaDecision::Denied {
                reason_code,
                detail,
            } => {
                return Err(ToolCallError::QuotaDenied {
                    reason_code,
                    detail,
                });
            }
        };
        if profile.family.is_networked() {
            if let Err(denial) = self.egress.check_url_resolved(&profile.base_url) {
                self.quota
                    .release(tenant_id, &operation_key, "egress_denied");
                return Err(ToolCallError::EgressDenied(denial.to_string()));
            }
        }
        match provider.invoke(profile, credential, arguments) {
            Ok(value) => {
                self.quota.commit(tenant_id, &operation_key);
                Ok(value)
            }
            Err(err) => {
                self.quota
                    .release(tenant_id, &operation_key, "provider_failed");
                Err(ToolCallError::Provider(err))
            }
        }
    }

    /// Runs one search through both gates.
    pub fn search(
        &self,
        tenant_id: &str,
        profile: &ToolProfile,
        credential: Option<&str>,
        request: &SearchRequest,
        provider: &dyn SearchProvider,
    ) -> Result<Vec<SearchResult>, ToolCallError> {
        if profile.readiness != Readiness::Ready {
            return Err(ToolCallError::Unconfigured {
                capability: profile.capability.as_str().to_string(),
            });
        }

        let operation_key = match self.quota.reserve(tenant_id, profile) {
            QuotaDecision::Allowed { operation_key } => operation_key,
            QuotaDecision::Denied {
                reason_code,
                detail,
            } => {
                return Err(ToolCallError::QuotaDenied {
                    reason_code,
                    detail,
                });
            }
        };

        // Egress is checked after the reservation so a denied call still
        // releases cleanly, and resolving rather than syntactically because a
        // profile validated at configuration time can point at a name whose
        // address changed since.
        if profile.family.is_networked() {
            if let Err(denial) = self.egress.check_url_resolved(&profile.base_url) {
                self.quota
                    .release(tenant_id, &operation_key, "egress_denied");
                return Err(ToolCallError::EgressDenied(denial.to_string()));
            }
        }

        match provider.search(profile, credential, request) {
            Ok(results) => {
                self.quota.commit(tenant_id, &operation_key);
                Ok(results)
            }
            Err(err) => {
                // The vendor was reached but failed, or was never reached. Either
                // way the tenant did not get an answer, so the unit goes back.
                self.quota
                    .release(tenant_id, &operation_key, "provider_failed");
                Err(ToolCallError::Provider(err))
            }
        }
    }
}
