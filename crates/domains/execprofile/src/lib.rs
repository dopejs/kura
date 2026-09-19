//! Port of `daemon/internal/execprofile` (Roadmap 69): execution backend + sandbox profile
//! availability, requirements, health, risk, and selection projections for product surfaces.
//! The daemon policy/sandbox layer remains authoritative for execution permission — these
//! projections never grant hidden access and never weaken preflight/approval gates; hosted
//! defaults fail closed when a backend is unavailable.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use kura_store::{SQLiteStore, list_documents, put_document};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! string_enum {
    ($name:ident { $first:ident => $first_s:literal $(, $v:ident => $s:literal)* $(,)? }) => {
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
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

string_enum!(BackendKind {
    Subprocess => "subprocess",
    Docker => "docker",
    Ssh => "ssh",
    LocalShell => "local_shell",
});

// The Go manager normalizes the empty risk tier to Medium; the enum variant order picks the
// `#[default]` used by `#[serde(default)]` on the profile field.
string_enum!(RiskTier {
    Medium => "medium",
    Low => "low",
    High => "high",
});

string_enum!(HealthStatus {
    Ready => "ready",
    Degraded => "degraded",
    Unavailable => "unavailable",
});

/// Manager validation/lookup failures (Go sentinel errors).
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ExecProfileError {
    #[error("execution profile not found")]
    ProfileNotFound,
    #[error("execution profile definition is invalid")]
    InvalidProfile,
    #[error("execution profile is unavailable")]
    ProfileUnavailable,
    #[error("profile selection is not permitted")]
    PermissionDenied,
}

/// An execution backend + sandbox profile: what capabilities it provides to tools, what
/// environment prerequisites it needs to be available, and its risk tier.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionProfile {
    pub profile_id: String,
    pub name: String,
    pub backend_kind: BackendKind,
    /// Go normalizes the empty risk tier to Medium on register; the typed enum defaults to
    /// Medium so a wire document without the field degrades the same way.
    #[serde(default)]
    pub risk_tier: RiskTier,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provides: Vec<String>, // capabilities offered to tools (e.g. docker, network, local_fs)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requirements: Vec<String>, // env prerequisites for the profile itself
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub created_at: DateTime<Utc>,
}

/// The live status of a profile: backend health plus any unmet requirements.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileStatus {
    pub profile_id: String,
    pub health: HealthStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unmet_requirements: Vec<String>,
    pub available: bool, // health ready AND requirements met
}

/// A profile with its live status (the list/detail product projection).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileProjection {
    pub profile: ExecutionProfile,
    pub status: ProfileStatus,
}

/// Explains why a tool with the given required capabilities can or cannot run: the eligible
/// profiles, plus per-profile missing capabilities (incompatible) or unavailability reasons
/// (FR-002 denials link to missing requirements / policy).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DenialExplanation {
    pub required_capabilities: Vec<String>,
    pub eligible_profiles: Vec<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub missing_capabilities: HashMap<String, Vec<String>>, // profileId -> missing caps
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub unavailable: HashMap<String, String>, // profileId -> reason
}

/// Reports the profiles compatible/incompatible with a catalog item's capability requirements
/// (FR-003).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Compatibility {
    pub required_capabilities: Vec<String>,
    pub compatible: Vec<String>,
    pub incompatible: Vec<String>,
}

/// One auditable profile-selection transition.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionEvent {
    pub profile_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub actor: String,
    pub occurred_at: DateTime<Utc>,
}

/// A tenant's selected execution profile with an audit history.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    pub tenant_id: String,
    pub profile_id: String,
    pub history: Vec<SelectionEvent>,
    pub updated_at: DateTime<Utc>,
}

/// Reports the live backend health of a profile (injectable; fake in tests).
pub trait HealthChecker: Send + Sync {
    fn health(&self, profile: &ExecutionProfile) -> (HealthStatus, String);
}

/// Reports which of a profile's environment requirements are unmet.
pub trait RequirementChecker: Send + Sync {
    fn unmet(&self, requirements: &[String]) -> Vec<String>;
}

/// Gates profile selection (FR-004 permission-gated + auditable).
pub trait PermissionGate: Send + Sync {
    fn allow(&self, tenant_id: &str, profile_id: &str) -> bool;
}

struct ReadyHealth;

impl HealthChecker for ReadyHealth {
    fn health(&self, _profile: &ExecutionProfile) -> (HealthStatus, String) {
        (HealthStatus::Ready, String::new())
    }
}

struct AllMet;

impl RequirementChecker for AllMet {
    fn unmet(&self, _requirements: &[String]) -> Vec<String> {
        Vec::new()
    }
}

struct AllowAll;

impl PermissionGate for AllowAll {
    fn allow(&self, _tenant_id: &str, _profile_id: &str) -> bool {
        true
    }
}

const DOC_KIND_EXEC_PROFILE: &str = "exec_profile";
const DOC_KIND_EXEC_SELECTION: &str = "exec_selection";

#[derive(Default)]
struct ManagerInner {
    profiles: HashMap<String, ExecutionProfile>,
    selections: HashMap<String, Selection>, // tenantID -> selection
}

/// Projects execution profiles and selections. Profiles + selections are in-memory with
/// `restore`; the sandbox/policy layer remains authoritative for actual execution permission.
pub struct Manager {
    inner: parking_lot::RwLock<ManagerInner>,
    env: String,
    health: Box<dyn HealthChecker>,
    reqs: Box<dyn RequirementChecker>,
    perms: Box<dyn PermissionGate>,
    docs: Option<Arc<parking_lot::Mutex<SQLiteStore>>>,
}

impl Default for Manager {
    fn default() -> Self {
        Self::new("", None, None, None)
    }
}

impl Manager {
    /// Go `NewManager`: nil hooks fall back to ready-health / all-met / allow-all defaults.
    pub fn new(
        environment_scope: &str,
        health: Option<Box<dyn HealthChecker>>,
        reqs: Option<Box<dyn RequirementChecker>>,
        perms: Option<Box<dyn PermissionGate>>,
    ) -> Self {
        Manager {
            inner: parking_lot::RwLock::new(ManagerInner::default()),
            env: environment_scope.trim().to_string(),
            health: health.unwrap_or_else(|| Box::new(ReadyHealth)),
            reqs: reqs.unwrap_or_else(|| Box::new(AllMet)),
            perms: perms.unwrap_or_else(|| Box::new(AllowAll)),
            docs: None,
        }
    }

    /// Go `WithStore`: installs durable persistence for profiles + selections and returns
    /// the manager.
    pub fn with_store(&mut self, store: Arc<parking_lot::Mutex<SQLiteStore>>) -> &mut Self {
        self.docs = Some(store);
        self
    }

    /// Go `LoadFromStore`: reloads persisted profiles + selections on startup.
    pub fn load_from_store(&self) -> Result<(), String> {
        let Some(store) = &self.docs else {
            return Ok(());
        };
        let profiles: Vec<ExecutionProfile> = list_documents(&store.lock(), DOC_KIND_EXEC_PROFILE)?;
        let selections: Vec<Selection> = list_documents(&store.lock(), DOC_KIND_EXEC_SELECTION)?;
        self.restore(profiles, selections);
        Ok(())
    }

    /// Go `RegisterProfile`: inserts or replaces an execution profile.
    pub fn register_profile(
        &self,
        mut profile: ExecutionProfile,
    ) -> Result<ExecutionProfile, ExecProfileError> {
        if profile.name.trim().is_empty() || !valid_backend(profile.backend_kind) {
            return Err(ExecProfileError::InvalidProfile);
        }
        // Go normalizes an empty RiskTier to Medium; the typed enum defaults to Medium (see
        // #[serde(default)] on ExecutionProfile::risk_tier), so no normalization is needed.
        if profile.profile_id.trim().is_empty() {
            profile.profile_id = new_id("exec_profile");
        }
        if is_zero_time(profile.created_at) {
            profile.created_at = Utc::now();
        }
        let mut inner = self.inner.write();
        inner
            .profiles
            .insert(profile.profile_id.clone(), profile.clone());
        drop(inner);
        self.persist(DOC_KIND_EXEC_PROFILE, &profile.profile_id, "", &profile);
        Ok(profile)
    }

    /// Go `status`: live status of a profile with derived reason when unavailable.
    fn status(&self, profile: &ExecutionProfile) -> ProfileStatus {
        let (health, mut reason) = self.health.health(profile);
        let unmet = self.reqs.unmet(&profile.requirements);
        let available = health == HealthStatus::Ready && unmet.is_empty();
        if !available && reason.is_empty() {
            if health != HealthStatus::Ready {
                reason = format!("backend {}", health.as_str());
            } else if !unmet.is_empty() {
                reason = format!("unmet requirements: {}", unmet.join(", "));
            }
        }
        ProfileStatus {
            profile_id: profile.profile_id.clone(),
            health,
            reason,
            unmet_requirements: unmet,
            available,
        }
    }

    /// Go `ListProfiles`: all profiles with live status (FR-001), sorted by profile id.
    pub fn list_profiles(&self) -> Vec<ProfileProjection> {
        let mut profiles: Vec<ExecutionProfile> = {
            let inner = self.inner.read();
            inner.profiles.values().cloned().collect()
        };
        profiles.sort_by(|a, b| a.profile_id.cmp(&b.profile_id));
        profiles
            .into_iter()
            .map(|profile| ProfileProjection {
                status: self.status(&profile),
                profile,
            })
            .collect()
    }

    /// Go `GetProfile`.
    pub fn get_profile(&self, profile_id: &str) -> Result<ProfileProjection, ExecProfileError> {
        let profile = {
            let inner = self.inner.read();
            inner.profiles.get(profile_id.trim()).cloned()
        };
        let Some(profile) = profile else {
            return Err(ExecProfileError::ProfileNotFound);
        };
        Ok(ProfileProjection {
            status: self.status(&profile),
            profile,
        })
    }

    /// Go `ExplainDenial`: explains which profiles can run a tool requiring the given
    /// capabilities, and why the others cannot (missing capabilities or unavailability).
    pub fn explain_denial(&self, required: &[String]) -> DenialExplanation {
        let mut exp = DenialExplanation {
            required_capabilities: required.to_vec(),
            eligible_profiles: Vec::new(),
            missing_capabilities: HashMap::new(),
            unavailable: HashMap::new(),
        };
        for proj in self.list_profiles() {
            let missing = missing_capabilities(&proj.profile.provides, required);
            if !missing.is_empty() {
                exp.missing_capabilities
                    .insert(proj.profile.profile_id.clone(), missing);
                continue;
            }
            if !proj.status.available {
                exp.unavailable.insert(
                    proj.profile.profile_id.clone(),
                    first_non_empty(&[proj.status.reason.as_str(), "unavailable"]),
                );
                continue;
            }
            exp.eligible_profiles.push(proj.profile.profile_id.clone());
        }
        exp
    }

    /// Go `CompatibilityFor`: profiles compatible/incompatible with a catalog item's
    /// capability requirements (FR-003), regardless of live availability.
    pub fn compatibility_for(&self, required: &[String]) -> Compatibility {
        let mut out = Compatibility {
            required_capabilities: required.to_vec(),
            compatible: Vec::new(),
            incompatible: Vec::new(),
        };
        for proj in self.list_profiles() {
            if missing_capabilities(&proj.profile.provides, required).is_empty() {
                out.compatible.push(proj.profile.profile_id.clone());
            } else {
                out.incompatible.push(proj.profile.profile_id.clone());
            }
        }
        out
    }

    /// Go `SelectProfile`: sets a tenant's execution profile, permission-gated and audited
    /// (FR-004). Fails closed when the profile is unavailable.
    pub fn select_profile(
        &self,
        tenant_id: &str,
        profile_id: &str,
        actor: &str,
    ) -> Result<Selection, ExecProfileError> {
        let proj = self.get_profile(profile_id)?;
        if !self.perms.allow(tenant_id, profile_id) {
            return Err(ExecProfileError::PermissionDenied);
        }
        if !proj.status.available {
            return Err(ExecProfileError::ProfileUnavailable);
        }
        let mut inner = self.inner.write();
        let mut selection = inner
            .selections
            .get(tenant_id.trim())
            .cloned()
            .unwrap_or_default();
        let now = Utc::now();
        selection.tenant_id = tenant_id.trim().to_string();
        selection.profile_id = profile_id.to_string();
        selection.history.push(SelectionEvent {
            profile_id: profile_id.to_string(),
            actor: actor.to_string(),
            occurred_at: now,
        });
        selection.updated_at = now;
        inner
            .selections
            .insert(selection.tenant_id.clone(), selection.clone());
        drop(inner);
        self.persist(
            DOC_KIND_EXEC_SELECTION,
            &selection.tenant_id,
            &selection.tenant_id,
            &selection,
        );
        Ok(selection)
    }

    /// Go `SelectionForTenant`.
    pub fn selection_for_tenant(&self, tenant_id: &str) -> (Selection, bool) {
        let inner = self.inner.read();
        match inner.selections.get(tenant_id.trim()) {
            Some(selection) => (selection.clone(), true),
            None => (Selection::default(), false),
        }
    }

    /// Go `Restore`: reloads persisted profiles + selections.
    pub fn restore(&self, profiles: Vec<ExecutionProfile>, selections: Vec<Selection>) {
        let mut inner = self.inner.write();
        inner.profiles = profiles
            .into_iter()
            .map(|p| (p.profile_id.clone(), p))
            .collect();
        inner.selections = selections
            .into_iter()
            .map(|s| (s.tenant_id.clone(), s))
            .collect();
    }

    /// Write-through persistence; skipped when no store is installed (Go nil store no-ops).
    fn persist<T: serde::Serialize>(&self, kind: &str, id: &str, tenant: &str, value: &T) {
        if let Some(store) = &self.docs {
            let _ = put_document(&store.lock(), kind, id, &self.env, tenant, value);
        }
    }
}

/// Go `missingCapabilities`: the required capabilities a profile does not provide, compared
/// case-insensitively after trimming, preserving the required order.
#[must_use]
pub fn missing_capabilities(provides: &[String], required: &[String]) -> Vec<String> {
    let have: HashSet<String> = provides
        .iter()
        .map(|p| p.trim().to_ascii_lowercase())
        .collect();
    required
        .iter()
        .filter(|r| !have.contains(&r.trim().to_ascii_lowercase()))
        .cloned()
        .collect()
}

/// Go `validBackend`.
fn valid_backend(kind: BackendKind) -> bool {
    matches!(
        kind,
        BackendKind::Subprocess | BackendKind::Docker | BackendKind::Ssh | BackendKind::LocalShell
    )
}

/// Go `firstNonEmpty`: the first value whose trimmed form is non-empty (the original,
/// untrimmed value), else "".
#[must_use]
pub fn first_non_empty(values: &[&str]) -> String {
    values
        .iter()
        .find(|value| !value.trim().is_empty())
        .map(|value| (*value).to_string())
        .unwrap_or_default()
}

/// Go `time.Time.IsZero`: the Go zero time is 0001-01-01T00:00:00Z.
fn is_zero_time(t: DateTime<Utc>) -> bool {
    t == DateTime::<Utc>::MIN_UTC
}

/// Go `newID`: 8 random bytes hex-encoded (16 hex chars) with the prefix.
fn new_id(prefix: &str) -> String {
    let hex = Uuid::new_v4().simple().to_string();
    format!("{prefix}_{}", &hex[..16])
}

// ---------------------------------------------------------------------------
// Sandbox-backed HealthChecker (wave 8 parity)
// ---------------------------------------------------------------------------

mod sandbox_health;
pub use sandbox_health::SandboxCapabilitySource;
pub use sandbox_health::SandboxHealthChecker;
