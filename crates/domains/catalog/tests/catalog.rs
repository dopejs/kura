use std::sync::Arc;

use chrono::Utc;
use kura_catalog::{
    CatalogError, CatalogItem, Enablement, EnablementEvent, EnablementState, ItemKind, Manager,
    PermissionGate, Requirement, RequirementChecker, TrustTier, Version,
};
use kura_store::SQLiteStore;
use serde_json::json;

fn temp_dir(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("kura_catalog_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().to_string()
}

struct DenyAll;

impl PermissionGate for DenyAll {
    fn allow(&self, _tenant_id: &str, _permissions: &[String]) -> bool {
        false
    }
}

struct UnmetAll;

impl RequirementChecker for UnmetAll {
    fn unmet(&self, _tenant_id: &str, requirements: &[Requirement]) -> Vec<Requirement> {
        requirements.to_vec()
    }
}

fn sample_item() -> CatalogItem {
    CatalogItem {
        item_id: "skill_search".to_string(),
        kind: ItemKind::Skill,
        name: "Search".to_string(),
        trust_tier: TrustTier::Verified,
        permissions: vec!["network".to_string()],
        versions: vec![
            Version {
                version: "1.0.0".to_string(),
                source: "registry".to_string(),
                checksum: "abc123".to_string(),
                requirements: vec![Requirement {
                    key: "network".to_string(),
                    description: "needs network".to_string(),
                }],
                published_at: Utc::now(),
            },
            Version {
                version: "1.1.0".to_string(),
                source: "registry".to_string(),
                checksum: String::new(),
                requirements: vec![],
                published_at: Utc::now(),
            },
        ],
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

#[test]
fn register_item_generates_id_and_preserves_explicit_tier() {
    let manager = Manager::new("test", None, None);
    let item = manager
        .register_item(CatalogItem {
            item_id: String::new(),
            kind: ItemKind::Skill,
            name: "Search".to_string(),
            trust_tier: TrustTier::Official,
            permissions: vec![],
            versions: vec![Version {
                version: "1.0.0".to_string(),
                source: "s".to_string(),
                checksum: String::new(),
                requirements: vec![],
                published_at: Utc::now(),
            }],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
        .unwrap();
    assert!(item.item_id.starts_with("catalog_item_"));
    assert_eq!(item.trust_tier, TrustTier::Official);
    assert_eq!(manager.list_items().len(), 1);
    assert!(manager.get_item(&item.item_id).is_some());
}

#[test]
fn register_invalid_item_rejected() {
    let manager = Manager::new("test", None, None);
    // Empty name.
    let err = manager
        .register_item(CatalogItem {
            name: String::new(),
            ..sample_item()
        })
        .unwrap_err();
    assert!(matches!(err, CatalogError::InvalidCatalogItem));
    // No versions.
    let err = manager
        .register_item(CatalogItem {
            versions: vec![],
            ..sample_item()
        })
        .unwrap_err();
    assert!(matches!(err, CatalogError::InvalidCatalogItem));
}

#[test]
fn enable_disable_rollback_lifecycle() {
    let manager = Manager::new("test", None, None);
    let item = manager.register_item(sample_item()).unwrap();
    let item_id = item.item_id.clone();

    let e1 = manager
        .enable("tenant-a", &item_id, "1.0.0", "alice")
        .unwrap();
    assert_eq!(e1.state, EnablementState::Enabled);
    assert_eq!(e1.active_version, "1.0.0");
    assert_eq!(e1.version_stack, vec!["1.0.0".to_string()]);
    assert_eq!(e1.history.len(), 1);
    assert_eq!(e1.history[0].action, "enabled");

    let e2 = manager
        .enable("tenant-a", &item_id, "1.1.0", "alice")
        .unwrap();
    assert_eq!(e2.active_version, "1.1.0");
    assert_eq!(
        e2.version_stack,
        vec!["1.0.0".to_string(), "1.1.0".to_string()]
    );

    // Re-enabling the same version does not duplicate the stack top.
    let e3 = manager
        .enable("tenant-a", &item_id, "1.1.0", "alice")
        .unwrap();
    assert_eq!(
        e3.version_stack,
        vec!["1.0.0".to_string(), "1.1.0".to_string()]
    );
    assert_eq!(e3.history.len(), 3);

    let r1 = manager.rollback("tenant-a", &item_id, "alice").unwrap();
    assert_eq!(r1.state, EnablementState::Enabled);
    assert_eq!(r1.active_version, "1.0.0");
    assert_eq!(r1.version_stack, vec!["1.0.0".to_string()]);
    assert_eq!(r1.history[3].action, "rolled_back");
    assert_eq!(r1.history[3].version, "1.0.0");

    // No prior version left: rollback disables safely.
    let r2 = manager.rollback("tenant-a", &item_id, "alice").unwrap();
    assert_eq!(r2.state, EnablementState::Disabled);
    assert!(r2.active_version.is_empty());
    assert!(r2.version_stack.is_empty());
    assert_eq!(r2.history[4].reason, "no prior version; disabled");

    let d = manager.disable("tenant-a", &item_id, "alice").unwrap();
    assert_eq!(d.state, EnablementState::Disabled);
    assert!(d.active_version.is_empty());
    assert!(d.version_stack.is_empty());
    assert_eq!(d.history.len(), 6);

    // Unknown item / version.
    assert!(matches!(
        manager
            .enable("tenant-a", "nope", "1.0.0", "alice")
            .unwrap_err(),
        CatalogError::ItemNotFound
    ));
    assert!(matches!(
        manager
            .enable("tenant-a", &item_id, "9.9.9", "alice")
            .unwrap_err(),
        CatalogError::VersionNotFound
    ));
}

#[test]
fn enable_is_permission_gated_but_disable_is_not() {
    let denied = Manager::new("test", None, Some(Box::new(DenyAll)));
    let item = denied.register_item(sample_item()).unwrap();
    let err = denied
        .enable("tenant-a", &item.item_id, "1.0.0", "alice")
        .unwrap_err();
    assert!(matches!(err, CatalogError::PermissionDenied));
    // Go's Disable is not permission-gated.
    assert!(denied.disable("tenant-a", &item.item_id, "alice").is_ok());
    let insp = denied.inspect("tenant-a", &item.item_id).unwrap();
    assert!(!insp.permission_satisfied);
}

#[test]
fn enable_fails_closed_on_unmet_requirements() {
    let strict = Manager::new("test", Some(Box::new(UnmetAll)), None);
    let item = strict.register_item(sample_item()).unwrap();
    let err = strict
        .enable("tenant-a", &item.item_id, "1.0.0", "alice")
        .unwrap_err();
    assert!(matches!(err, CatalogError::RequirementsUnmet));
    // A version without requirements passes the gate.
    assert!(
        strict
            .enable("tenant-a", &item.item_id, "1.1.0", "alice")
            .is_ok()
    );
}

#[test]
fn inspect_projects_enablement_and_gates() {
    let manager = Manager::new("test", None, None);
    let item = manager.register_item(sample_item()).unwrap();
    let item_id = item.item_id.clone();
    manager
        .enable("tenant-a", &item_id, "1.1.0", "alice")
        .unwrap();

    let insp = manager.inspect("tenant-a", &item_id).unwrap();
    assert_eq!(insp.item.item_id, item_id);
    assert_eq!(insp.enablement.state, EnablementState::Enabled);
    assert_eq!(insp.enablement.active_version, "1.1.0");
    assert!(insp.permission_satisfied);
    assert!(insp.unmet_requirements.is_empty());

    // A tenant without enablement inspects against the latest version.
    let insp2 = manager.inspect("tenant-b", &item_id).unwrap();
    assert_eq!(insp2.enablement.state, EnablementState::Disabled);
    assert!(insp2.unmet_requirements.is_empty());
}

#[test]
fn active_version_fails_closed_when_requirements_regress() {
    let manager = Manager::new("test", None, None);
    let item = manager.register_item(sample_item()).unwrap();
    let item_id = item.item_id.clone();

    assert_eq!(
        manager.active_version("tenant-a", &item_id),
        (String::new(), false)
    );
    manager
        .enable("tenant-a", &item_id, "1.0.0", "alice")
        .unwrap();
    assert_eq!(
        manager.active_version("tenant-a", &item_id),
        ("1.0.0".to_string(), true)
    );

    // A restored enablement whose active version has unmet requirements must not execute.
    let strict = Manager::new("test", Some(Box::new(UnmetAll)), None);
    strict.restore(
        vec![item],
        vec![Enablement {
            tenant_id: "tenant-a".to_string(),
            item_id: item_id.clone(),
            state: EnablementState::Enabled,
            active_version: "1.0.0".to_string(),
            version_stack: vec!["1.0.0".to_string()],
            history: vec![],
            updated_at: Utc::now(),
        }],
    );
    assert_eq!(
        strict.active_version("tenant-a", &item_id),
        (String::new(), false)
    );
}

#[test]
fn restore_reloads_items_and_enablements() {
    let manager = Manager::new("test", None, None);
    let item = manager.register_item(sample_item()).unwrap();
    let enablement = manager
        .enable("tenant-a", &item.item_id, "1.0.0", "alice")
        .unwrap();

    let fresh = Manager::new("test", None, None);
    fresh.restore(vec![item.clone()], vec![enablement.clone()]);
    assert_eq!(fresh.get_item(&item.item_id).unwrap(), item);
    assert_eq!(
        fresh.inspect("tenant-a", &item.item_id).unwrap().enablement,
        enablement
    );
}

#[test]
fn wire_round_trip() {
    let item = sample_item();
    let value = serde_json::to_value(&item).unwrap();
    assert_eq!(value["itemId"], "skill_search");
    assert_eq!(value["kind"], "skill");
    assert_eq!(value["trustTier"], "verified");
    assert_eq!(value["versions"][0]["requirements"][0]["key"], "network");
    let back: CatalogItem = serde_json::from_value(value).unwrap();
    assert_eq!(back, item);

    // Enum wire values are exact snake_case.
    assert_eq!(ItemKind::McpServer.as_str(), "mcp_server");
    assert_eq!(ItemKind::Capability.as_str(), "capability");
    assert_eq!(TrustTier::Untrusted.as_str(), "untrusted");
    assert_eq!(EnablementState::Enabled.as_str(), "enabled");
    assert_eq!(
        serde_json::to_value(ItemKind::McpServer).unwrap(),
        json!("mcp_server")
    );
    assert_eq!(
        serde_json::to_value(EnablementState::Enabled).unwrap(),
        json!("enabled")
    );

    // omitempty fields are skipped.
    let empty_item = CatalogItem {
        permissions: vec![],
        versions: vec![],
        ..sample_item()
    };
    let v = serde_json::to_value(&empty_item).unwrap();
    assert!(v.get("permissions").is_none());
    let e = Enablement {
        active_version: String::new(),
        version_stack: vec![],
        history: vec![],
        ..Enablement::default()
    };
    let v = serde_json::to_value(&e).unwrap();
    assert!(v.get("activeVersion").is_none());
    assert!(v.get("versionStack").is_none());
    assert!(v.get("history").is_some());
    let ev = EnablementEvent {
        action: "enabled".to_string(),
        version: "1.0.0".to_string(),
        actor: "alice".to_string(),
        reason: String::new(),
        occurred_at: Utc::now(),
    };
    let v = serde_json::to_value(&ev).unwrap();
    assert!(v.get("reason").is_none());
}

#[test]
fn persistence_round_trip() {
    let dir = temp_dir("persist");
    let store = Arc::new(parking_lot::Mutex::new(SQLiteStore::new(&dir).unwrap()));
    let mut manager = Manager::new("test", None, None);
    manager.with_store(Arc::clone(&store));
    let registered = manager.register_item(sample_item()).unwrap();
    manager
        .enable("tenant-a", &registered.item_id, "1.0.0", "alice")
        .unwrap();

    // A fresh manager recovers items + enablements from the store.
    let mut fresh = Manager::new("test", None, None);
    fresh.with_store(Arc::clone(&store));
    fresh.load_from_store().unwrap();
    assert_eq!(fresh.get_item(&registered.item_id).unwrap(), registered);
    let insp = fresh.inspect("tenant-a", &registered.item_id).unwrap();
    assert_eq!(insp.enablement.state, EnablementState::Enabled);
    assert_eq!(insp.enablement.active_version, "1.0.0");
}
/// Compile-time guard: this manager must be usable from axum `AppState` (Send + Sync).
#[test]
fn manager_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<kura_catalog::Manager>();
}
