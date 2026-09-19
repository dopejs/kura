//! Port of `daemon/internal/evidence` (Roadmap 71): the support diagnostics + redacted
//! evidence bundle. A permission-gated, tenant-scoped, redacted-by-default bundle that collects
//! resource summaries + links (never raw secrets or unbounded logs) for support/incident triage.
//! Bundle generation and access are audited; redaction failure fails closed.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use kura_store::{SQLiteStore, list_documents, put_document};
use regex::Regex;
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

string_enum!(ScopeKind {
    Run => "run",
    Workflow => "workflow",
    Thread => "thread",
    Connector => "connector",
    Provider => "provider",
    Routine => "routine",
    QuotaDenial => "quota_denial",
    TimeWindow => "time_window",
});

string_enum!(RedactionStatus {
    Redacted => "redacted",
    FailedClosed => "failed_closed",
});

/// The bundle target: a kind + a ref, or a time window.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Scope {
    pub kind: ScopeKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub r#ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_start: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_end: Option<DateTime<Utc>>,
}

/// One collected evidence section: redacted resource summaries + links, never raw secrets or
/// full logs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Section {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resource_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub summary: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<String>,
}

/// A generated, redacted evidence bundle.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bundle {
    pub bundle_id: String,
    pub tenant_id: String,
    pub actor: String,
    pub scope: Scope,
    pub sections: Vec<Section>,
    pub redaction_status: RedactionStatus,
    pub created_at: DateTime<Utc>,
    pub retention_expires_at: DateTime<Utc>,
}

/// An audit record for bundle generation or access.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessEvent {
    pub bundle_id: String,
    pub tenant_id: String,
    pub actor: String,
    pub action: String, // generated | accessed
    pub occurred_at: DateTime<Utc>,
}

/// Manager validation/lookup failures (Go sentinel errors).
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum EvidenceError {
    #[error("evidence bundle not found")]
    BundleNotFound,
    #[error("support permission required for evidence bundle")]
    PermissionDenied,
    #[error("evidence bundle scope is invalid")]
    InvalidScope,
    #[error("evidence bundle redaction failed closed")]
    RedactionFailed,
    #[error("evidence bundle belongs to another tenant")]
    CrossTenantAccess,
    #[error("evidence collection failed: {0}")]
    Collect(String),
}

/// How long a generated bundle is retained.
pub const DEFAULT_RETENTION: Duration = Duration::days(14);

const DOC_KIND_BUNDLE: &str = "evidence_bundle";

/// Gathers redaction-candidate sections for a scope. Implementations reuse existing
/// diagnostic/evaluation/audit/event records (never raw secrets or unbounded logs).
pub trait Collector: Send + Sync {
    fn collect(&self, tenant_id: &str, scope: &Scope) -> Result<Vec<Section>, String>;
}

/// Authorizes support-role bundle generation/access for a tenant.
pub trait PermissionGate: Send + Sync {
    fn allow_support(&self, actor: &str, tenant_id: &str) -> bool;
}

struct AllowAll;

impl PermissionGate for AllowAll {
    fn allow_support(&self, _actor: &str, _tenant_id: &str) -> bool {
        true
    }
}

struct EmptyCollector;

impl Collector for EmptyCollector {
    fn collect(&self, _tenant_id: &str, _scope: &Scope) -> Result<Vec<Section>, String> {
        Ok(Vec::new())
    }
}

#[derive(Default)]
struct ManagerInner {
    bundles: HashMap<String, Bundle>,
    audit: Vec<AccessEvent>,
}

/// Generates and serves redacted evidence bundles. Bundles + audit are in-memory for this
/// slice; bundle content reuses existing records via the collector and is redacted by default.
pub struct Manager {
    inner: parking_lot::RwLock<ManagerInner>,
    env: String,
    collector: Box<dyn Collector>,
    perms: Box<dyn PermissionGate>,
    docs: Option<Arc<parking_lot::Mutex<SQLiteStore>>>,
}

impl Default for Manager {
    fn default() -> Self {
        Self::new("", None, None)
    }
}

impl Manager {
    /// Go `NewManager`: nil hooks fall back to an empty collector + all-pass permission gate.
    pub fn new(
        environment_scope: &str,
        collector: Option<Box<dyn Collector>>,
        perms: Option<Box<dyn PermissionGate>>,
    ) -> Self {
        Manager {
            inner: parking_lot::RwLock::new(ManagerInner::default()),
            env: environment_scope.trim().to_string(),
            collector: collector.unwrap_or_else(|| Box::new(EmptyCollector)),
            perms: perms.unwrap_or_else(|| Box::new(AllowAll)),
            docs: None,
        }
    }

    /// Go `WithStore`: installs durable persistence for generated evidence bundles and
    /// returns the manager.
    pub fn with_store(&mut self, store: Arc<parking_lot::Mutex<SQLiteStore>>) -> &mut Self {
        self.docs = Some(store);
        self
    }

    /// Go `LoadFromStore`: reloads persisted evidence bundles on startup (merges into the
    /// in-memory map; the audit trail is not persisted).
    pub fn load_from_store(&self) -> Result<(), String> {
        let Some(store) = &self.docs else {
            return Ok(());
        };
        let bundles: Vec<Bundle> = list_documents(&store.lock(), DOC_KIND_BUNDLE)?;
        let mut inner = self.inner.write();
        for bundle in bundles {
            inner.bundles.insert(bundle.bundle_id.clone(), bundle);
        }
        Ok(())
    }

    /// Go `Generate`: produces a redacted, tenant-scoped, audited evidence bundle. It fails
    /// closed when the caller lacks support permission or redaction cannot guarantee secret
    /// removal.
    pub fn generate(
        &self,
        tenant_id: &str,
        actor: &str,
        scope: Scope,
    ) -> Result<Bundle, EvidenceError> {
        if tenant_id.trim().is_empty() || !valid_scope(&scope) {
            return Err(EvidenceError::InvalidScope);
        }
        if !self.perms.allow_support(actor, tenant_id) {
            return Err(EvidenceError::PermissionDenied);
        }
        let collected = self
            .collector
            .collect(tenant_id, &scope)
            .map_err(EvidenceError::Collect)?;
        let (redacted, ok) = redact_sections(collected);
        if !ok {
            // Fail closed: do not persist or return a bundle that could leak secrets.
            return Err(EvidenceError::RedactionFailed);
        }
        let now = Utc::now();
        let bundle = Bundle {
            bundle_id: new_id("evidence_bundle"),
            tenant_id: tenant_id.trim().to_string(),
            actor: actor.trim().to_string(),
            scope,
            sections: redacted,
            redaction_status: RedactionStatus::Redacted,
            created_at: now,
            retention_expires_at: now + DEFAULT_RETENTION,
        };
        let mut inner = self.inner.write();
        inner
            .bundles
            .insert(bundle.bundle_id.clone(), bundle.clone());
        inner.audit.push(AccessEvent {
            bundle_id: bundle.bundle_id.clone(),
            tenant_id: bundle.tenant_id.clone(),
            actor: bundle.actor.clone(),
            action: "generated".to_string(),
            occurred_at: now,
        });
        drop(inner);
        self.persist(
            DOC_KIND_BUNDLE,
            &bundle.bundle_id,
            &bundle.tenant_id,
            &bundle,
        );
        Ok(bundle)
    }

    /// Go `Get`: returns a bundle for an authorized support actor within the owning tenant,
    /// recording an access audit event. Cross-tenant access is denied.
    pub fn get(
        &self,
        tenant_id: &str,
        actor: &str,
        bundle_id: &str,
    ) -> Result<Bundle, EvidenceError> {
        let bundle = {
            let inner = self.inner.read();
            inner.bundles.get(bundle_id.trim()).cloned()
        };
        let Some(bundle) = bundle else {
            return Err(EvidenceError::BundleNotFound);
        };
        if bundle.tenant_id != tenant_id.trim() {
            return Err(EvidenceError::CrossTenantAccess);
        }
        if !self.perms.allow_support(actor, tenant_id) {
            return Err(EvidenceError::PermissionDenied);
        }
        let mut inner = self.inner.write();
        inner.audit.push(AccessEvent {
            bundle_id: bundle.bundle_id.clone(),
            tenant_id: bundle.tenant_id.clone(),
            actor: actor.trim().to_string(),
            action: "accessed".to_string(),
            occurred_at: Utc::now(),
        });
        Ok(bundle)
    }

    /// Go `ListForTenant`: returns bundle metadata for a tenant (permission-gated).
    pub fn list_for_tenant(
        &self,
        tenant_id: &str,
        actor: &str,
    ) -> Result<Vec<Bundle>, EvidenceError> {
        if !self.perms.allow_support(actor, tenant_id) {
            return Err(EvidenceError::PermissionDenied);
        }
        let inner = self.inner.read();
        Ok(inner
            .bundles
            .values()
            .filter(|b| b.tenant_id == tenant_id.trim())
            .cloned()
            .collect())
    }

    /// Go `AuditTrail`: the generation/access audit events for a bundle (FR audit).
    pub fn audit_trail(&self, bundle_id: &str) -> Vec<AccessEvent> {
        let inner = self.inner.read();
        inner
            .audit
            .iter()
            .filter(|e| e.bundle_id == bundle_id.trim())
            .cloned()
            .collect()
    }

    /// Write-through persistence; skipped when no store is installed (Go nil store no-ops).
    fn persist<T: serde::Serialize>(&self, kind: &str, id: &str, tenant: &str, value: &T) {
        if let Some(store) = &self.docs {
            let _ = put_document(&store.lock(), kind, id, &self.env, tenant, value);
        }
    }
}

/// Go `validScope`: scoped kinds require a ref; a time window requires both bounds.
fn valid_scope(scope: &Scope) -> bool {
    match scope.kind {
        ScopeKind::Run
        | ScopeKind::Workflow
        | ScopeKind::Thread
        | ScopeKind::Connector
        | ScopeKind::Provider
        | ScopeKind::Routine
        | ScopeKind::QuotaDenial => !scope.r#ref.trim().is_empty(),
        ScopeKind::TimeWindow => scope.window_start.is_some() && scope.window_end.is_some(),
    }
}

/// Go `newID`: 8 random bytes hex-encoded (16 hex chars) with the prefix.
fn new_id(prefix: &str) -> String {
    let hex = Uuid::new_v4().simple().to_string();
    format!("{prefix}_{}", &hex[..16])
}

// ---------------------------------------------------------------------------
// Redaction (Go redaction.go): sensitive summary keys are always redacted; raw secret material
// detected anywhere fails the bundle closed.
// ---------------------------------------------------------------------------

/// The placeholder substituted for sensitive summary values.
pub const REDACTED_PLACEHOLDER: &str = "[redacted]";

/// Summary keys whose values are always redacted (Go sensitiveKeys).
const SENSITIVE_KEYS: &[&str] = &[
    "token",
    "accesstoken",
    "access_token",
    "refreshtoken",
    "secret",
    "password",
    "authorization",
    "apikey",
    "api_key",
    "credential",
    "oauth",
    "clientsecret",
    "client_secret",
    "signingsecret",
];

/// Matches obvious raw credential material that must never appear in a bundle. If a value still
/// matches after redaction, the bundle fails closed (Go secretMarker).
static SECRET_MARKER: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(
        r"(?i)(bearer\s+[a-z0-9._\-]{8,}|sk-[a-z0-9]{8,}|xox[baprs]-[a-z0-9-]{8,}|-----BEGIN [A-Z ]+PRIVATE KEY-----|eyJ[a-zA-Z0-9_\-]{10,}\.)",
    )
    .expect("secret marker regex is valid")
});

/// Go `sensitiveKeys[strings.ToLower(strings.TrimSpace(key))]`.
#[must_use]
pub fn is_sensitive_key(key: &str) -> bool {
    let normalized = key.trim().to_ascii_lowercase();
    SENSITIVE_KEYS.contains(&normalized.as_str())
}

/// Go `redactSections`: redacts sensitive summary values and validates that no raw secret
/// material remains. Returns (redacted sections, ok) where ok=false fails the bundle closed.
#[must_use]
pub fn redact_sections(sections: Vec<Section>) -> (Vec<Section>, bool) {
    let mut out = Vec::with_capacity(sections.len());
    let mut ok = true;
    for section in sections {
        let mut redacted = Section {
            kind: section.kind,
            resource_refs: section.resource_refs,
            summary: HashMap::new(),
            links: section.links,
        };
        if !section.summary.is_empty() {
            redacted.summary.reserve(section.summary.len());
            for (key, value) in &section.summary {
                if is_sensitive_key(key) {
                    redacted
                        .summary
                        .insert(key.clone(), REDACTED_PLACEHOLDER.to_string());
                    continue;
                }
                if SECRET_MARKER.is_match(value) {
                    // A non-sensitive-keyed value carrying raw secret material cannot be safely
                    // redacted in place — fail the whole bundle closed (FR redaction-fail-closed).
                    ok = false;
                    redacted
                        .summary
                        .insert(key.clone(), REDACTED_PLACEHOLDER.to_string());
                    continue;
                }
                redacted.summary.insert(key.clone(), value.clone());
            }
        }
        // Links must not carry secret material either.
        for link in &redacted.links {
            if SECRET_MARKER.is_match(link) {
                ok = false;
            }
        }
        out.push(redacted);
    }
    (out, ok)
}

// ---------------------------------------------------------------------------
// Routine-backed Collector (wave 8 parity)
// ---------------------------------------------------------------------------

mod routine_collector;
pub use routine_collector::RoutineCollector;
