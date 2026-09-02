//! Sandbox-backed health checker tests (wave 8 parity): availability mapping,
//! unknown-kind fallback, nil-source fallback, and a real sandbox-manager
//! integration through the execprofile manager.

use std::sync::Arc;

use chrono::Utc;
use kura_execprofile::{
    BackendKind, ExecutionProfile, HealthChecker, HealthStatus, Manager, RiskTier,
    SandboxCapabilitySource, SandboxHealthChecker,
};
use kura_sandbox::{BackendAvailabilityStatus, BackendCapabilityProfile, BackendKind as SandboxBackend};

fn temp_dir(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("kura_execprofile_sandbox_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().to_string()
}

fn profile(backend: BackendKind) -> ExecutionProfile {
    ExecutionProfile {
        profile_id: "p_1".to_string(),
        name: "p_1 name".to_string(),
        backend_kind: backend,
        risk_tier: RiskTier::Low,
        provides: vec!["local_fs".to_string()],
        requirements: vec![],
        description: "sample".to_string(),
        created_at: Utc::now(),
    }
}

struct FakeCapabilities(Vec<BackendCapabilityProfile>);

impl SandboxCapabilitySource for FakeCapabilities {
    fn backend_capabilities(&self) -> Vec<BackendCapabilityProfile> {
        self.0.clone()
    }
}

fn capability(backend: SandboxBackend, availability: BackendAvailabilityStatus, reason: &str) -> BackendCapabilityProfile {
    BackendCapabilityProfile {
        backend_kind: backend,
        display_name: String::new(),
        filesystem_enforcement: String::new(),
        network_enforcement: String::new(),
        env_injection_mode: String::new(),
        approval_behavior: String::new(),
        restart_behavior: String::new(),
        host_prerequisites: vec![],
        availability_status: availability,
        availability_reason: reason.to_string(),
    }
}

#[test]
fn maps_sandbox_availability_to_health() {
    let source = FakeCapabilities(vec![
        capability(SandboxBackend::Subprocess, BackendAvailabilityStatus::Available, ""),
        capability(SandboxBackend::Docker, BackendAvailabilityStatus::Degraded, "docker daemon slow"),
        capability(SandboxBackend::Ssh, BackendAvailabilityStatus::Unavailable, "ssh host unreachable"),
    ]);
    let checker = SandboxHealthChecker::new(Some(Arc::new(source)));

    assert_eq!(
        checker.health(&profile(BackendKind::Subprocess)),
        (HealthStatus::Ready, String::new())
    );
    assert_eq!(
        checker.health(&profile(BackendKind::Docker)),
        (HealthStatus::Degraded, "docker daemon slow".to_string())
    );
    assert_eq!(
        checker.health(&profile(BackendKind::Ssh)),
        (HealthStatus::Unavailable, "ssh host unreachable".to_string())
    );
}

#[test]
fn unknown_backend_kind_stays_ready() {
    // local_shell has no sandbox capability counterpart (Go: unknown backend
    // kinds are not falsely marked unavailable).
    let source = FakeCapabilities(vec![capability(SandboxBackend::Subprocess, BackendAvailabilityStatus::Unavailable, "down")]);
    let checker = SandboxHealthChecker::new(Some(Arc::new(source)));
    assert_eq!(
        checker.health(&profile(BackendKind::LocalShell)),
        (HealthStatus::Ready, String::new())
    );
}

#[test]
fn matching_is_case_insensitive() {
    // Go strings.EqualFold: sandbox "Docker" matches profile "docker".
    let source = FakeCapabilities(vec![capability(SandboxBackend::Docker, BackendAvailabilityStatus::Available, "")]);
    let checker = SandboxHealthChecker::new(Some(Arc::new(source)));
    assert_eq!(
        checker.health(&profile(BackendKind::Docker)),
        (HealthStatus::Ready, String::new())
    );
}

#[test]
fn nil_sandbox_source_stays_ready() {
    let checker = SandboxHealthChecker::new(None);
    assert_eq!(
        checker.health(&profile(BackendKind::Docker)),
        (HealthStatus::Ready, String::new())
    );
}

#[test]
fn manager_projects_sandbox_health() {
    // The fake source drives the manager's live status (Go app wiring: the
    // execprofile manager is constructed with sandboxExecHealth).
    let source = FakeCapabilities(vec![capability(SandboxBackend::Subprocess, BackendAvailabilityStatus::Degraded, "subprocess backend down")]);
    let manager = Manager::new(
        "test",
        Some(Box::new(SandboxHealthChecker::new(Some(Arc::new(source))))),
        None,
        None,
    );
    manager.register_profile(profile(BackendKind::Subprocess)).unwrap();
    let proj = manager.get_profile("p_1").unwrap();
    assert_eq!(proj.status.health, HealthStatus::Degraded);
    assert_eq!(proj.status.reason, "subprocess backend down");
    assert!(!proj.status.available);
}

#[test]
fn manager_projects_real_sandbox_capabilities() {
    // Real kura_sandbox::Manager integration: the subprocess backend is always
    // detected as available, so a subprocess profile reports ready.
    let dir = temp_dir("real");
    let cfg = kura_config::Config {
        project_root: String::new(),
        environment: kura_config::Environment::Test,
        bind_addr: "127.0.0.1:19192".to_string(),
        data_dir: dir.clone(),
        log_level: "info".to_string(),
        version: "dev".to_string(),
        llm: Default::default(),
        connectors: Default::default(),
    };
    let sandbox = kura_sandbox::Manager::new(cfg, None, kura_events::Bus::new(), kura_policy::Engine::new());
    let manager = Manager::new(
        "test",
        Some(Box::new(SandboxHealthChecker::new(Some(Arc::new(sandbox))))),
        None,
        None,
    );
    manager.register_profile(profile(BackendKind::Subprocess)).unwrap();
    let proj = manager.get_profile("p_1").unwrap();
    assert_eq!(proj.status.health, HealthStatus::Ready);
    assert!(proj.status.available);
}
