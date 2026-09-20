use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use kura_execprofile::{
    BackendKind, DenialExplanation, ExecProfileError, ExecutionProfile, HealthChecker,
    HealthStatus, Manager, PermissionGate, RequirementChecker, RiskTier, Selection,
};
use kura_store::SQLiteStore;
use serde_json::json;

fn temp_dir(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("kura_execprofile_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().to_string()
}

fn go_zero_time() -> chrono::DateTime<Utc> {
    chrono::DateTime::<Utc>::MIN_UTC
}

struct DegradedHealth;

impl HealthChecker for DegradedHealth {
    fn health(&self, _profile: &ExecutionProfile) -> (HealthStatus, String) {
        (HealthStatus::Degraded, "backend down".to_string())
    }
}

struct DenyAll;

impl PermissionGate for DenyAll {
    fn allow(&self, _tenant_id: &str, _profile_id: &str) -> bool {
        false
    }
}

struct UnmetAll;

impl RequirementChecker for UnmetAll {
    fn unmet(&self, requirements: &[String]) -> Vec<String> {
        requirements.to_vec()
    }
}

fn sample_profile(profile_id: &str, provides: &[&str], requirements: &[&str]) -> ExecutionProfile {
    ExecutionProfile {
        profile_id: profile_id.to_string(),
        name: format!("{profile_id} name"),
        backend_kind: BackendKind::Docker,
        risk_tier: RiskTier::Low,
        provides: provides.iter().map(|s| s.to_string()).collect(),
        requirements: requirements.iter().map(|s| s.to_string()).collect(),
        description: "sample".to_string(),
        created_at: Utc::now(),
    }
}

#[test]
fn register_profile_generates_id_and_preserves_explicit_tier() {
    let manager = Manager::new("test", None, None, None);
    let profile = manager
        .register_profile(ExecutionProfile {
            profile_id: String::new(),
            name: "Sandbox".to_string(),
            backend_kind: BackendKind::Subprocess,
            risk_tier: RiskTier::High,
            provides: vec![],
            requirements: vec![],
            description: String::new(),
            created_at: go_zero_time(), // Go zero time -> stamped now
        })
        .unwrap();
    assert!(profile.profile_id.starts_with("exec_profile_"));
    assert_eq!(profile.risk_tier, RiskTier::High); // explicit tier preserved
    assert_ne!(profile.created_at, go_zero_time());
    assert_eq!(manager.list_profiles().len(), 1);
}

#[test]
fn register_invalid_profile_rejected() {
    let manager = Manager::new("test", None, None, None);
    let err = manager
        .register_profile(ExecutionProfile {
            name: "  ".to_string(),
            ..sample_profile("p_1", &[], &[])
        })
        .unwrap_err();
    assert!(matches!(err, ExecProfileError::InvalidProfile));
}

#[test]
fn list_profiles_sorted_with_live_status() {
    let manager = Manager::new("test", None, None, None);
    manager
        .register_profile(sample_profile("p_z", &["docker"], &[]))
        .unwrap();
    manager
        .register_profile(sample_profile("p_a", &["network"], &[]))
        .unwrap();
    let listed = manager.list_profiles();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].profile.profile_id, "p_a");
    assert_eq!(listed[1].profile.profile_id, "p_z");
    for proj in &listed {
        assert_eq!(proj.status.health, HealthStatus::Ready);
        assert!(proj.status.available);
        assert!(proj.status.reason.is_empty());
    }
}

#[test]
fn get_profile_lookup() {
    let manager = Manager::new("test", None, None, None);
    manager
        .register_profile(sample_profile("p_a", &["docker"], &[]))
        .unwrap();
    let proj = manager.get_profile("p_a").unwrap();
    assert_eq!(proj.profile.name, "p_a name");
    assert!(matches!(
        manager.get_profile("missing").unwrap_err(),
        ExecProfileError::ProfileNotFound
    ));
}

#[test]
fn status_derives_unmet_requirement_reason() {
    let manager = Manager::new("test", None, Some(Box::new(UnmetAll)), None);
    manager
        .register_profile(sample_profile("p_a", &["docker"], &["docker_engine"]))
        .unwrap();
    let proj = manager.get_profile("p_a").unwrap();
    assert_eq!(proj.status.health, HealthStatus::Ready);
    assert!(!proj.status.available);
    assert_eq!(proj.status.reason, "unmet requirements: docker_engine");
    assert_eq!(
        proj.status.unmet_requirements,
        vec!["docker_engine".to_string()]
    );
}

#[test]
fn explain_denial_splits_eligible_missing_unavailable() {
    let manager = Manager::new("test", None, None, None);
    manager
        .register_profile(sample_profile("p_docker", &["docker", "network"], &[]))
        .unwrap();
    manager
        .register_profile(sample_profile("p_basic", &["local_fs"], &[]))
        .unwrap();
    let required = vec!["docker".to_string(), "network".to_string()];
    let exp = manager.explain_denial(&required);
    assert_eq!(exp.required_capabilities, required);
    assert_eq!(exp.eligible_profiles, vec!["p_docker".to_string()]);
    assert_eq!(
        exp.missing_capabilities["p_basic"],
        vec!["docker".to_string(), "network".to_string()]
    );
    assert!(exp.unavailable.is_empty());

    // Unavailable profiles land in 'unavailable' with their reason.
    let degraded = Manager::new("test", Some(Box::new(DegradedHealth)), None, None);
    degraded
        .register_profile(sample_profile("p_docker", &["docker"], &[]))
        .unwrap();
    let exp = degraded.explain_denial(&["docker".to_string()]);
    assert!(exp.eligible_profiles.is_empty());
    assert_eq!(exp.unavailable["p_docker"], "backend down");
}

#[test]
fn compatibility_for_ignores_availability() {
    let manager = Manager::new("test", None, None, None);
    manager
        .register_profile(sample_profile("p_docker", &["docker"], &[]))
        .unwrap();
    manager
        .register_profile(sample_profile("p_basic", &["local_fs"], &[]))
        .unwrap();
    let compat = manager.compatibility_for(&["docker".to_string()]);
    assert_eq!(compat.compatible, vec!["p_docker".to_string()]);
    assert_eq!(compat.incompatible, vec!["p_basic".to_string()]);
}

#[test]
fn select_profile_lifecycle_is_audited() {
    let manager = Manager::new("test", None, None, None);
    manager
        .register_profile(sample_profile("p_a", &["docker"], &[]))
        .unwrap();
    let sel = manager.select_profile("tenant-a", "p_a", "alice").unwrap();
    assert_eq!(sel.tenant_id, "tenant-a");
    assert_eq!(sel.profile_id, "p_a");
    assert_eq!(sel.history.len(), 1);
    assert_eq!(sel.history[0].actor, "alice");

    let (sel2, ok) = manager.selection_for_tenant("tenant-a");
    assert!(ok);
    assert_eq!(sel2, sel);
    assert_eq!(
        manager.selection_for_tenant("tenant-b"),
        (Selection::default(), false)
    );

    // A second selection appends to the audit history.
    let sel3 = manager.select_profile("tenant-a", "p_a", "bob").unwrap();
    assert_eq!(sel3.history.len(), 2);
    assert_eq!(sel3.history[1].actor, "bob");
}

#[test]
fn select_profile_fails_closed() {
    // Permission denied.
    let denied = Manager::new("test", None, None, Some(Box::new(DenyAll)));
    denied
        .register_profile(sample_profile("p_a", &["docker"], &[]))
        .unwrap();
    assert!(matches!(
        denied
            .select_profile("tenant-a", "p_a", "alice")
            .unwrap_err(),
        ExecProfileError::PermissionDenied
    ));

    // Unavailable profile.
    let degraded = Manager::new("test", Some(Box::new(DegradedHealth)), None, None);
    degraded
        .register_profile(sample_profile("p_a", &["docker"], &[]))
        .unwrap();
    assert!(matches!(
        degraded
            .select_profile("tenant-a", "p_a", "alice")
            .unwrap_err(),
        ExecProfileError::ProfileUnavailable
    ));

    // Unknown profile.
    let manager = Manager::new("test", None, None, None);
    assert!(matches!(
        manager
            .select_profile("tenant-a", "nope", "alice")
            .unwrap_err(),
        ExecProfileError::ProfileNotFound
    ));
}

#[test]
fn restore_reloads_profiles_and_selections() {
    let manager = Manager::new("test", None, None, None);
    let profile = manager
        .register_profile(sample_profile("p_a", &["docker"], &[]))
        .unwrap();
    let selection = manager
        .select_profile("tenant-a", &profile.profile_id, "alice")
        .unwrap();

    let fresh = Manager::new("test", None, None, None);
    fresh.restore(vec![profile.clone()], vec![selection.clone()]);
    assert_eq!(
        fresh.get_profile(&profile.profile_id).unwrap().profile,
        profile
    );
    assert_eq!(fresh.selection_for_tenant("tenant-a"), (selection, true));
}

#[test]
fn wire_round_trip() {
    let profile = ExecutionProfile {
        description: String::new(),
        ..sample_profile("p_1", &["docker"], &[])
    };
    let value = serde_json::to_value(&profile).unwrap();
    assert_eq!(value["profileId"], "p_1");
    assert_eq!(value["backendKind"], "docker");
    assert_eq!(value["riskTier"], "low");
    assert_eq!(value["provides"], json!(["docker"]));
    assert!(value.get("description").is_none()); // empty description omitted
    let back: ExecutionProfile = serde_json::from_value(value).unwrap();
    assert_eq!(back, profile);

    assert_eq!(BackendKind::Ssh.as_str(), "ssh");
    assert_eq!(BackendKind::LocalShell.as_str(), "local_shell");
    assert_eq!(HealthStatus::Degraded.as_str(), "degraded");
    assert_eq!(
        serde_json::to_value(BackendKind::Ssh).unwrap(),
        json!("ssh")
    );

    let exp = DenialExplanation {
        required_capabilities: vec!["docker".to_string()],
        eligible_profiles: vec!["p_1".to_string()],
        missing_capabilities: HashMap::new(),
        unavailable: HashMap::new(),
    };
    let value = serde_json::to_value(&exp).unwrap();
    assert_eq!(value["requiredCapabilities"], json!(["docker"]));
    assert_eq!(value["eligibleProfiles"], json!(["p_1"]));
    assert!(value.get("missingCapabilities").is_none());
    assert!(value.get("unavailable").is_none());

    let sel = Selection {
        tenant_id: "tenant-a".to_string(),
        profile_id: "p_1".to_string(),
        history: vec![],
        updated_at: Utc::now(),
    };
    let value = serde_json::to_value(&sel).unwrap();
    assert_eq!(value["tenantId"], "tenant-a");
    assert_eq!(value["profileId"], "p_1");
    assert_eq!(value["history"], json!([]));
    let back: Selection = serde_json::from_value(value).unwrap();
    assert_eq!(back, sel);
}

#[test]
fn persistence_round_trip() {
    let dir = temp_dir("persist");
    let store = Arc::new(parking_lot::Mutex::new(SQLiteStore::new(&dir).unwrap()));
    let mut manager = Manager::new("test", None, None, None);
    manager.with_store(Arc::clone(&store));
    let profile = manager
        .register_profile(sample_profile("p_a", &["docker"], &[]))
        .unwrap();
    manager
        .select_profile("tenant-a", &profile.profile_id, "alice")
        .unwrap();

    // A fresh manager recovers profiles + selections from the store.
    let mut fresh = Manager::new("test", None, None, None);
    fresh.with_store(Arc::clone(&store));
    fresh.load_from_store().unwrap();
    assert_eq!(
        fresh.get_profile(&profile.profile_id).unwrap().profile,
        profile
    );
    let (sel, ok) = fresh.selection_for_tenant("tenant-a");
    assert!(ok);
    assert_eq!(sel.profile_id, profile.profile_id);
    assert_eq!(sel.history.len(), 1);
}
/// Compile-time guard: this manager must be usable from axum `AppState` (Send + Sync).
#[test]
fn manager_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<kura_execprofile::Manager>();
}
