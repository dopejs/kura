use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use kura_evidence::{
    Bundle, Collector, DEFAULT_RETENTION, EvidenceError, Manager, PermissionGate, RedactionStatus,
    Scope, ScopeKind, Section, redact_sections,
};
use kura_store::SQLiteStore;
use serde_json::json;

fn temp_dir(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("kura_evidence_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().to_string()
}

struct SectionsCollector(Vec<Section>);

impl Collector for SectionsCollector {
    fn collect(&self, _tenant_id: &str, _scope: &Scope) -> Result<Vec<Section>, String> {
        Ok(self.0.clone())
    }
}

struct FailingCollector;

impl Collector for FailingCollector {
    fn collect(&self, _tenant_id: &str, _scope: &Scope) -> Result<Vec<Section>, String> {
        Err("collector exploded".to_string())
    }
}

struct DenyAll;

impl PermissionGate for DenyAll {
    fn allow_support(&self, _actor: &str, _tenant_id: &str) -> bool {
        false
    }
}

fn run_scope(ref_value: &str) -> Scope {
    Scope {
        kind: ScopeKind::Run,
        r#ref: ref_value.to_string(),
        window_start: None,
        window_end: None,
    }
}

#[test]
fn generate_creates_redacted_audited_bundle() {
    let manager = Manager::new("test", None, None);
    let bundle = manager
        .generate("  tenant-a  ", "  alice  ", run_scope("run_1"))
        .unwrap();
    assert!(bundle.bundle_id.starts_with("evidence_bundle_"));
    assert_eq!(bundle.tenant_id, "tenant-a"); // trimmed
    assert_eq!(bundle.actor, "alice"); // trimmed
    assert_eq!(bundle.scope.kind, ScopeKind::Run);
    assert_eq!(bundle.scope.r#ref, "run_1");
    assert!(bundle.sections.is_empty());
    assert_eq!(bundle.redaction_status, RedactionStatus::Redacted);
    assert_eq!(
        bundle.retention_expires_at - bundle.created_at,
        DEFAULT_RETENTION
    );
    assert_eq!(manager.audit_trail(&bundle.bundle_id).len(), 1);
    assert_eq!(
        manager.audit_trail(&bundle.bundle_id)[0].action,
        "generated"
    );
}

#[test]
fn generate_rejects_invalid_scope_or_tenant() {
    let manager = Manager::new("test", None, None);
    // Empty ref for a run scope.
    assert!(matches!(
        manager
            .generate("tenant-a", "alice", run_scope("  "))
            .unwrap_err(),
        EvidenceError::InvalidScope
    ));
    // Time window requires both bounds.
    let window = Scope {
        kind: ScopeKind::TimeWindow,
        r#ref: String::new(),
        window_start: Some(Utc::now()),
        window_end: None,
    };
    assert!(matches!(
        manager.generate("tenant-a", "alice", window).unwrap_err(),
        EvidenceError::InvalidScope
    ));
    // Empty tenant.
    assert!(matches!(
        manager
            .generate("  ", "alice", run_scope("run_1"))
            .unwrap_err(),
        EvidenceError::InvalidScope
    ));
}

#[test]
fn generate_is_permission_gated() {
    let manager = Manager::new("test", None, Some(Box::new(DenyAll)));
    let err = manager
        .generate("tenant-a", "alice", run_scope("run_1"))
        .unwrap_err();
    assert!(matches!(err, EvidenceError::PermissionDenied));

    // Get and ListForTenant are permission-gated once a bundle exists: generate with an
    // allow-all manager + store, then access through a DenyAll manager reloaded from the store.
    let dir = temp_dir("perm");
    let store = Arc::new(parking_lot::Mutex::new(SQLiteStore::new(&dir).unwrap()));
    let mut producer = Manager::new("test", None, None);
    producer.with_store(Arc::clone(&store));
    let bundle = producer
        .generate("tenant-a", "alice", run_scope("run_1"))
        .unwrap();
    let mut consumer = Manager::new("test", None, Some(Box::new(DenyAll)));
    consumer.with_store(Arc::clone(&store));
    consumer.load_from_store().unwrap();
    assert!(matches!(
        consumer
            .get("tenant-a", "alice", &bundle.bundle_id)
            .unwrap_err(),
        EvidenceError::PermissionDenied
    ));
    assert!(matches!(
        consumer.list_for_tenant("tenant-a", "alice").unwrap_err(),
        EvidenceError::PermissionDenied
    ));
}

#[test]
fn generate_surfaces_collector_errors() {
    let manager = Manager::new("test", Some(Box::new(FailingCollector)), None);
    let err = manager
        .generate("tenant-a", "alice", run_scope("run_1"))
        .unwrap_err();
    assert!(matches!(err, EvidenceError::Collect(ref msg) if msg == "collector exploded"));
}

#[test]
fn generate_fails_closed_on_secret_material() {
    let collector = SectionsCollector(vec![Section {
        kind: "audit".to_string(),
        resource_refs: vec![],
        summary: HashMap::from([(
            "url".to_string(),
            "https://x/?token=eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.".to_string(),
        )]),
        links: vec![],
    }]);
    let manager = Manager::new("test", Some(Box::new(collector)), None);
    let err = manager
        .generate("tenant-a", "alice", run_scope("run_1"))
        .unwrap_err();
    assert!(matches!(err, EvidenceError::RedactionFailed));
    // Nothing was stored for the failed bundle.
    assert!(
        manager
            .list_for_tenant("tenant-a", "alice")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn get_enforces_tenant_boundary_and_audits_access() {
    let manager = Manager::new("test", None, None);
    let bundle = manager
        .generate("tenant-a", "alice", run_scope("run_1"))
        .unwrap();

    // Unknown bundle.
    assert!(matches!(
        manager
            .get("tenant-a", "alice", "evidence_bundle_nope")
            .unwrap_err(),
        EvidenceError::BundleNotFound
    ));
    // Cross-tenant access denied.
    assert!(matches!(
        manager
            .get("tenant-b", "alice", &bundle.bundle_id)
            .unwrap_err(),
        EvidenceError::CrossTenantAccess
    ));
    // Authorized access returns the bundle and records an audit event.
    let got = manager.get("tenant-a", "alice", &bundle.bundle_id).unwrap();
    assert_eq!(got, bundle);
    let trail = manager.audit_trail(&bundle.bundle_id);
    assert_eq!(trail.len(), 2);
    assert_eq!(trail[1].action, "accessed");
}

#[test]
fn list_for_tenant_is_tenant_scoped() {
    let manager = Manager::new("test", None, None);
    let a = manager
        .generate("tenant-a", "alice", run_scope("run_a"))
        .unwrap();
    let _b = manager
        .generate("tenant-b", "bob", run_scope("run_b"))
        .unwrap();
    let listed = manager.list_for_tenant("tenant-a", "alice").unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].bundle_id, a.bundle_id);
}

#[test]
fn redact_sections_redacts_and_fails_closed() {
    // Sensitive keys are replaced with the placeholder; normal values pass through.
    let (out, ok) = redact_sections(vec![Section {
        kind: "audit".to_string(),
        resource_refs: vec![],
        summary: HashMap::from([
            ("access_token".to_string(), "sk-abcdefgh".to_string()),
            ("count".to_string(), "42".to_string()),
        ]),
        links: vec!["https://ok.example".to_string()],
    }]);
    assert!(ok);
    assert_eq!(out[0].summary["access_token"], "[redacted]");
    assert_eq!(out[0].summary["count"], "42");

    // Secret material in a link fails the bundle closed.
    let (_, ok) = redact_sections(vec![Section {
        kind: "audit".to_string(),
        resource_refs: vec![],
        summary: HashMap::new(),
        links: vec!["https://x/?t=sk-abcdefgh".to_string()],
    }]);
    assert!(!ok);

    // Secret material in a non-sensitive key fails the bundle closed.
    let (_, ok) = redact_sections(vec![Section {
        kind: "audit".to_string(),
        resource_refs: vec![],
        summary: HashMap::from([("url".to_string(), "Bearer abcdef123456".to_string())]),
        links: vec![],
    }]);
    assert!(!ok);
}

#[test]
fn wire_round_trip() {
    let bundle = Bundle {
        bundle_id: "evidence_bundle_abc".to_string(),
        tenant_id: "tenant-a".to_string(),
        actor: "alice".to_string(),
        scope: run_scope("run_1"),
        sections: vec![Section {
            kind: "audit".to_string(),
            resource_refs: vec!["res_1".to_string()],
            summary: HashMap::new(),
            links: vec![],
        }],
        redaction_status: RedactionStatus::Redacted,
        created_at: Utc::now(),
        retention_expires_at: Utc::now(),
    };
    let value = serde_json::to_value(&bundle).unwrap();
    assert_eq!(value["bundleId"], "evidence_bundle_abc");
    assert_eq!(value["scope"]["kind"], "run");
    assert_eq!(value["scope"]["ref"], "run_1");
    assert!(value["scope"].get("windowStart").is_none());
    assert!(value["sections"][0].get("summary").is_none());
    assert_eq!(value["redactionStatus"], "redacted");

    assert_eq!(ScopeKind::QuotaDenial.as_str(), "quota_denial");
    assert_eq!(ScopeKind::TimeWindow.as_str(), "time_window");
    assert_eq!(RedactionStatus::FailedClosed.as_str(), "failed_closed");
    assert_eq!(
        serde_json::to_value(ScopeKind::TimeWindow).unwrap(),
        json!("time_window")
    );

    let back: Bundle = serde_json::from_value(value).unwrap();
    assert_eq!(back, bundle);
}

#[test]
fn persistence_round_trip() {
    let dir = temp_dir("persist");
    let store = Arc::new(parking_lot::Mutex::new(SQLiteStore::new(&dir).unwrap()));
    let mut manager = Manager::new("test", None, None);
    manager.with_store(Arc::clone(&store));
    let bundle = manager
        .generate("tenant-a", "alice", run_scope("run_1"))
        .unwrap();

    // A fresh manager recovers bundles from the store; audit events are not persisted.
    let mut fresh = Manager::new("test", None, None);
    fresh.with_store(Arc::clone(&store));
    fresh.load_from_store().unwrap();
    assert!(fresh.audit_trail(&bundle.bundle_id).is_empty()); // generation audit is not persisted
    assert_eq!(fresh.list_for_tenant("tenant-a", "alice").unwrap().len(), 1);
    assert_eq!(
        fresh.get("tenant-a", "alice", &bundle.bundle_id).unwrap(),
        bundle
    );
    assert_eq!(fresh.audit_trail(&bundle.bundle_id).len(), 1); // only the access event just recorded
}
/// Compile-time guard: this manager must be usable from axum `AppState` (Send + Sync).
#[test]
fn manager_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<kura_evidence::Manager>();
}
