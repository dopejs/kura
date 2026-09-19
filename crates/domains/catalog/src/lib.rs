//! Port of `daemon/internal/catalog` (Roadmap 68): the operator-managed skill and capability
//! catalog. Operator-curated catalog items (skills, MCP servers, supervised capabilities) can be
//! enabled, disabled, permissioned, versioned, inspected, and rolled back per tenant. The agent
//! does NOT generate or promote its own skills here; hosted install policy is explicit and fails
//! closed (unmet requirements or denied permissions block enablement before execution).

use std::collections::HashMap;
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

// The Go manager normalizes the empty wire value (Community / disabled state); the enum variant
// order picks the `#[default]` used by `#[serde(default)]` on the struct fields.
string_enum!(ItemKind {
    Skill => "skill",
    McpServer => "mcp_server",
    Capability => "capability",
    Plugin => "plugin",
});

string_enum!(TrustTier {
    Community => "community",
    Official => "official",
    Verified => "verified",
    Untrusted => "untrusted",
});

string_enum!(EnablementState {
    Disabled => "disabled",
    Enabled => "enabled",
});

/// Manager validation/lookup failures (Go sentinel errors).
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum CatalogError {
    #[error("catalog item not found")]
    ItemNotFound,
    #[error("catalog item version not found")]
    VersionNotFound,
    #[error("catalog item requirements are not met")]
    RequirementsUnmet,
    #[error("tenant is not permitted to enable this catalog item")]
    PermissionDenied,
    #[error("no prior enabled version to roll back to")]
    NoRollbackTarget,
    #[error("catalog item definition is invalid")]
    InvalidCatalogItem,
}

/// A declared prerequisite for a catalog item version (e.g. a sandbox backend, a resolved
/// secret, or network access). It is checked before enable/execution (FR-003).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Requirement {
    pub key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

/// One published version of a catalog item with its source, requirements, and checksum.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Version {
    pub version: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub checksum: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requirements: Vec<Requirement>,
    pub published_at: DateTime<Utc>,
}

/// An operator-curated catalog entry. Versions are ordered oldest-first.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogItem {
    pub item_id: String,
    pub kind: ItemKind,
    pub name: String,
    /// Go normalizes the empty trust tier to Community on register; the typed enum defaults to
    /// Community so a wire document without the field degrades the same way.
    #[serde(default)]
    pub trust_tier: TrustTier,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub permissions: Vec<String>,
    pub versions: Vec<Version>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl CatalogItem {
    /// Go `CatalogItem.version`: the published version matching `version`.
    #[must_use]
    pub fn version(&self, version: &str) -> Option<Version> {
        self.versions.iter().find(|v| v.version == version).cloned()
    }

    /// Go `CatalogItem.latest`: the last (newest) published version.
    #[must_use]
    pub fn latest(&self) -> Option<Version> {
        self.versions.last().cloned()
    }
}

/// One auditable enablement transition.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnablementEvent {
    pub action: String, // enabled | disabled | rolled_back
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub version: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub actor: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    pub occurred_at: DateTime<Utc>,
}

/// The per-tenant enablement record for a catalog item, including the active version and an
/// audit history of transitions.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Enablement {
    pub tenant_id: String,
    pub item_id: String,
    /// Missing state on load degrades to Disabled (safe default).
    #[serde(default)]
    pub state: EnablementState,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub active_version: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub version_stack: Vec<String>, // enabled-version stack for deterministic rollback
    pub history: Vec<EnablementEvent>,
    pub updated_at: DateTime<Utc>,
}

/// Projects a catalog item plus a tenant's enablement and any unmet requirements for the
/// active/target version, so a user can see why a skill is unavailable (FR-005, US3).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Inspection {
    pub item: CatalogItem,
    pub enablement: Enablement,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unmet_requirements: Vec<Requirement>,
    pub permission_satisfied: bool,
}

/// Reports which of an item version's declared requirements are unmet in the given
/// tenant/environment. The policy hook that fails closed before install/execution.
pub trait RequirementChecker: Send + Sync {
    fn unmet(&self, tenant_id: &str, requirements: &[Requirement]) -> Vec<Requirement>;
}

/// Reports whether a tenant may enable an item requiring the given permissions.
pub trait PermissionGate: Send + Sync {
    fn allow(&self, tenant_id: &str, permissions: &[String]) -> bool;
}

struct AllMet;

impl RequirementChecker for AllMet {
    fn unmet(&self, _tenant_id: &str, _requirements: &[Requirement]) -> Vec<Requirement> {
        Vec::new()
    }
}

struct AllowAllPermissions;

impl PermissionGate for AllowAllPermissions {
    fn allow(&self, _tenant_id: &str, _permissions: &[String]) -> bool {
        true
    }
}

const DOC_KIND_CATALOG_ITEM: &str = "catalog_item";
const DOC_KIND_CATALOG_ENABLEMENT: &str = "catalog_enablement";

#[derive(Default)]
struct ManagerInner {
    items: HashMap<String, CatalogItem>,
    enablements: HashMap<String, Enablement>, // key: tenantID + NUL + itemID
}

/// Owns the operator catalog and per-tenant enablement. Items + enablements are in-memory with
/// `restore`; a `kura_store::SQLiteStore` installed via `with_store` is used for
/// write-through persistence (None skips persistence, mirroring the Go nil store).
pub struct Manager {
    inner: parking_lot::RwLock<ManagerInner>,
    env: String,
    checker: Box<dyn RequirementChecker>,
    permissions: Box<dyn PermissionGate>,
    docs: Option<Arc<parking_lot::Mutex<SQLiteStore>>>,
}

impl Default for Manager {
    fn default() -> Self {
        Self::new("", None, None)
    }
}

impl Manager {
    /// Go `NewManager`: nil hooks fall back to all-pass defaults.
    pub fn new(
        environment_scope: &str,
        checker: Option<Box<dyn RequirementChecker>>,
        permissions: Option<Box<dyn PermissionGate>>,
    ) -> Self {
        Manager {
            inner: parking_lot::RwLock::new(ManagerInner::default()),
            env: environment_scope.trim().to_string(),
            checker: checker.unwrap_or_else(|| Box::new(AllMet)),
            permissions: permissions.unwrap_or_else(|| Box::new(AllowAllPermissions)),
            docs: None,
        }
    }

    /// Go `WithStore`: installs durable persistence for catalog items + enablements and
    /// returns the manager.
    pub fn with_store(&mut self, store: Arc<parking_lot::Mutex<SQLiteStore>>) -> &mut Self {
        self.docs = Some(store);
        self
    }

    /// Go `LoadFromStore`: reloads persisted catalog items + enablements on startup.
    pub fn load_from_store(&self) -> Result<(), String> {
        let Some(store) = &self.docs else {
            return Ok(());
        };
        let items: Vec<CatalogItem> = list_documents(&store.lock(), DOC_KIND_CATALOG_ITEM)?;
        let enablements: Vec<Enablement> =
            list_documents(&store.lock(), DOC_KIND_CATALOG_ENABLEMENT)?;
        self.restore(items, enablements);
        Ok(())
    }

    /// Go `RegisterItem`: inserts or replaces an operator-curated catalog item (a projection an
    /// operator curates; the agent never authors items here).
    pub fn register_item(&self, mut item: CatalogItem) -> Result<CatalogItem, CatalogError> {
        if item.name.trim().is_empty() || !valid_kind(item.kind) || item.versions.is_empty() {
            return Err(CatalogError::InvalidCatalogItem);
        }
        let now = Utc::now();
        if item.item_id.trim().is_empty() {
            item.item_id = new_id("catalog_item");
            item.created_at = now;
        }
        // Go normalizes an empty TrustTier to Community; the typed enum defaults to Community
        // (see #[serde(default)] on CatalogItem::trust_tier), so no normalization is needed.
        item.updated_at = now;
        let mut inner = self.inner.write();
        if let Some(existing) = inner.items.get(&item.item_id) {
            if !is_zero_time(existing.created_at) {
                item.created_at = existing.created_at;
            }
        }
        inner.items.insert(item.item_id.clone(), item.clone());
        drop(inner);
        self.persist(DOC_KIND_CATALOG_ITEM, &item.item_id, "", &item);
        Ok(item)
    }

    /// Go `GetItem`.
    pub fn get_item(&self, item_id: &str) -> Option<CatalogItem> {
        self.inner.read().items.get(item_id.trim()).cloned()
    }

    /// Go `ListItems`.
    pub fn list_items(&self) -> Vec<CatalogItem> {
        let inner = self.inner.read();
        inner.items.values().cloned().collect()
    }

    /// Go `Enable`: enables a catalog item version for a tenant after the permission and
    /// requirement gates pass (fail closed). Enablement is recorded with an audit event.
    pub fn enable(
        &self,
        tenant_id: &str,
        item_id: &str,
        version: &str,
        actor: &str,
    ) -> Result<Enablement, CatalogError> {
        let item = self.get_item(item_id).ok_or(CatalogError::ItemNotFound)?;
        let target = resolve_version(&item, version).ok_or(CatalogError::VersionNotFound)?;
        if !self.permissions.allow(tenant_id, &item.permissions) {
            return Err(CatalogError::PermissionDenied);
        }
        if !self
            .checker
            .unmet(tenant_id, &target.requirements)
            .is_empty()
        {
            return Err(CatalogError::RequirementsUnmet);
        }
        Ok(self.record_transition(
            tenant_id,
            item_id,
            EnablementState::Enabled,
            &target.version,
            "enabled",
            actor,
            "",
        ))
    }

    /// Go `Disable`: disables a catalog item for a tenant (not permission-gated in Go).
    pub fn disable(
        &self,
        tenant_id: &str,
        item_id: &str,
        actor: &str,
    ) -> Result<Enablement, CatalogError> {
        if self.get_item(item_id).is_none() {
            return Err(CatalogError::ItemNotFound);
        }
        Ok(self.record_transition(
            tenant_id,
            item_id,
            EnablementState::Disabled,
            "",
            "disabled",
            actor,
            "",
        ))
    }

    /// Go `Rollback`: restores the prior enabled version from the audit history, or disables
    /// safely when there is no prior version (FR-004).
    pub fn rollback(
        &self,
        tenant_id: &str,
        item_id: &str,
        actor: &str,
    ) -> Result<Enablement, CatalogError> {
        let key = enablement_key(tenant_id, item_id);
        let mut inner = self.inner.write();
        let mut enablement = inner
            .enablements
            .get(&key)
            .cloned()
            .ok_or(CatalogError::NoRollbackTarget)?;
        let now = Utc::now();
        // Pop the current active version off the stack; the new top (if any) is restored.
        if !enablement.version_stack.is_empty() {
            enablement.version_stack.pop();
        }
        if enablement.version_stack.is_empty() {
            enablement.state = EnablementState::Disabled;
            enablement.active_version.clear();
            enablement.history.push(EnablementEvent {
                action: "rolled_back".to_string(),
                actor: actor.to_string(),
                reason: "no prior version; disabled".to_string(),
                occurred_at: now,
                ..EnablementEvent::default()
            });
        } else {
            let prior = enablement.version_stack[enablement.version_stack.len() - 1].clone();
            enablement.state = EnablementState::Enabled;
            enablement.active_version = prior.clone();
            enablement.history.push(EnablementEvent {
                action: "rolled_back".to_string(),
                version: prior,
                actor: actor.to_string(),
                occurred_at: now,
                ..EnablementEvent::default()
            });
        }
        enablement.updated_at = now;
        inner.enablements.insert(key.clone(), enablement.clone());
        drop(inner);
        self.persist(
            DOC_KIND_CATALOG_ENABLEMENT,
            &key,
            &enablement.tenant_id,
            &enablement,
        );
        Ok(enablement)
    }

    /// Go `Inspect`: returns the item, the tenant's enablement, unmet requirements for the
    /// active/latest version, and whether the permission gate is satisfied (FR-005, US3).
    pub fn inspect(&self, tenant_id: &str, item_id: &str) -> Result<Inspection, CatalogError> {
        let item = self.get_item(item_id).ok_or(CatalogError::ItemNotFound)?;
        let enablement = {
            let inner = self.inner.read();
            inner
                .enablements
                .get(&enablement_key(tenant_id, item_id))
                .cloned()
                .unwrap_or_default()
        };
        let version = match resolve_version(&item, &enablement.active_version) {
            Some(version) => version,
            None => item.latest().unwrap_or_default(),
        };
        Ok(Inspection {
            unmet_requirements: self.checker.unmet(tenant_id, &version.requirements),
            permission_satisfied: self.permissions.allow(tenant_id, &item.permissions),
            item,
            enablement,
        })
    }

    /// Go `ActiveVersion`: returns the version influencing execution for a tenant's item, plus
    /// whether it is currently enabled and requirements remain met (runtime evidence, FR-005).
    pub fn active_version(&self, tenant_id: &str, item_id: &str) -> (String, bool) {
        let enablement = {
            let inner = self.inner.read();
            inner
                .enablements
                .get(&enablement_key(tenant_id, item_id))
                .cloned()
        };
        let Some(enablement) = enablement else {
            return (String::new(), false);
        };
        if enablement.state != EnablementState::Enabled {
            return (String::new(), false);
        }
        let Some(item) = self.get_item(item_id) else {
            return (String::new(), false);
        };
        let Some(version) = resolve_version(&item, &enablement.active_version) else {
            return (String::new(), false);
        };
        if !self
            .checker
            .unmet(tenant_id, &version.requirements)
            .is_empty()
        {
            return (String::new(), false); // requirements regressed; not safe to execute
        }
        (enablement.active_version, true)
    }

    /// Go `Restore`: reloads persisted items + enablements.
    pub fn restore(&self, items: Vec<CatalogItem>, enablements: Vec<Enablement>) {
        let mut inner = self.inner.write();
        inner.items = items
            .into_iter()
            .map(|item| (item.item_id.clone(), item))
            .collect();
        inner.enablements = enablements
            .into_iter()
            .map(|e| (enablement_key(&e.tenant_id, &e.item_id), e))
            .collect();
    }

    /// Go `recordTransition`: records an enablement transition with an audit event and
    /// write-through persistence.
    fn record_transition(
        &self,
        tenant_id: &str,
        item_id: &str,
        state: EnablementState,
        version: &str,
        action: &str,
        actor: &str,
        reason: &str,
    ) -> Enablement {
        let key = enablement_key(tenant_id, item_id);
        let mut inner = self.inner.write();
        let mut enablement = inner
            .enablements
            .get(&key)
            .cloned()
            .unwrap_or_else(|| Enablement {
                tenant_id: tenant_id.trim().to_string(),
                item_id: item_id.trim().to_string(),
                ..Enablement::default()
            });
        let now = Utc::now();
        enablement.state = state;
        if enablement.state == EnablementState::Enabled {
            enablement.active_version = version.to_string();
            // Push onto the version stack (dedup the current top) for deterministic rollback.
            let stack_len = enablement.version_stack.len();
            if stack_len == 0 || enablement.version_stack[stack_len - 1] != version {
                enablement.version_stack.push(version.to_string());
            }
        } else {
            enablement.active_version.clear();
            enablement.version_stack.clear();
        }
        enablement.history.push(EnablementEvent {
            action: action.to_string(),
            version: version.to_string(),
            actor: actor.to_string(),
            reason: reason.to_string(),
            occurred_at: now,
        });
        enablement.updated_at = now;
        inner.enablements.insert(key.clone(), enablement.clone());
        drop(inner);
        self.persist(
            DOC_KIND_CATALOG_ENABLEMENT,
            &key,
            &enablement.tenant_id,
            &enablement,
        );
        enablement
    }

    /// Write-through persistence; skipped when no store is installed (Go nil store no-ops).
    fn persist<T: serde::Serialize>(&self, kind: &str, id: &str, tenant: &str, value: &T) {
        if let Some(store) = &self.docs {
            let _ = put_document(&store.lock(), kind, id, &self.env, tenant, value);
        }
    }
}

/// Go `enablementKey`: tenantID + NUL + itemID (both trimmed).
fn enablement_key(tenant_id: &str, item_id: &str) -> String {
    format!(
        "{}{}{}",
        tenant_id.trim(),
        char::from_u32(0).expect("NUL is a valid char"),
        item_id.trim()
    )
}

/// Go `resolveVersion`: empty version resolves to the latest.
fn resolve_version(item: &CatalogItem, version: &str) -> Option<Version> {
    if version.trim().is_empty() {
        return item.latest();
    }
    item.version(version)
}

/// Go `validKind` (extended with `plugin` for the pluginization program).
fn valid_kind(kind: ItemKind) -> bool {
    matches!(
        kind,
        ItemKind::Skill | ItemKind::McpServer | ItemKind::Capability | ItemKind::Plugin
    )
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
