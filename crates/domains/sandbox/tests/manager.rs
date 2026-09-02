//! Manager-behavior tests ported from daemon/internal/sandbox/manager_test.go:
//! explain/evaluate decisions, builtin profile capability metadata, subprocess
//! execution lifecycle (complete/persist, approval gate, cancel, redaction,
//! managed-provider finalization recovery, secret scope), plus wait/close.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use kura_events::{Bus, Filter};
use kura_policy::Engine;
use kura_sandbox::{
    AccessRequest, ApprovalMode, BackendAvailabilityStatus, BackendCapabilityProfile,
    BackendHostStatus, BackendKind, BackendSelectionOutcome, CaptureBuffer, ConsumerContractView,
    ConsumerKind, ConsumerPolicyRecord, ConsumerRequirementDeclaration, DecisionResolution,
    ExecutionMode, ExecutionRequest, ExecutionStatus, Manager, NetworkMode,
    PROFILE_ID_MANAGED_PROVIDER_CLAUDE, PROFILE_ID_MANAGED_PROVIDER_CODEX,
    PROFILE_ID_SUBPROCESS_DEFAULT, PolicyRecordStatus, Profile, SandboxError, SecretDefaultSource,
    SecretEnvironmentScope, SecretResolution, SecretScopeOutcome, Source,
    approval_matches_execution, awaits_managed_provider_finalization, clean_path,
    derived_secret_variants, evaluate_access_decision, first_non_empty, hex_encode, is_terminal,
    redact_secret_text, within_any,
};
use kura_store::SQLiteStore;

fn temp_dir(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("kura_sandbox_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir.to_string_lossy().to_string()
}

fn test_config(data_dir: &str) -> kura_config::Config {
    kura_config::Config {
        project_root: String::new(),
        environment: kura_config::Environment::Test,
        bind_addr: "127.0.0.1:19192".to_string(),
        data_dir: data_dir.to_string(),
        log_level: "info".to_string(),
        version: "dev".to_string(),
        llm: kura_config::LlmConfig {
            claude: kura_config::ManagedCliProviderConfig {
                work_dir: "~".to_string(),
                ..Default::default()
            },
            codex: kura_config::ManagedCliProviderConfig {
                work_dir: "~".to_string(),
                ..Default::default()
            },
            ..Default::default()
        },
        connectors: kura_config::ConnectorConfig::default(),
    }
}

fn test_manager(data_dir: &str) -> Manager {
    let store = Arc::new(Mutex::new(SQLiteStore::new(data_dir).expect("open store")));
    Manager::new(
        test_config(data_dir),
        Some(store),
        Bus::new(),
        Engine::new(),
    )
}

fn wait_for_terminal(manager: &Manager, execution_id: &str) -> kura_sandbox::Execution {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let execution = manager
            .get_execution(execution_id)
            .expect("execution present");
        if is_terminal(execution.status) {
            return execution;
        }
        if std::time::Instant::now() >= deadline {
            panic!("execution {execution_id} did not reach terminal state");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn test_shell() -> &'static str {
    if cfg!(windows) { "cmd" } else { "/bin/sh" }
}

fn test_shell_args(script: &str) -> Vec<String> {
    if cfg!(windows) {
        vec!["/c".to_string(), script.to_string()]
    } else {
        vec!["-c".to_string(), script.to_string()]
    }
}

fn test_consumer_view(
    consumer_id: &str,
    secret_scope: Vec<SecretScopeOutcome>,
) -> ConsumerContractView {
    ConsumerContractView {
        declaration: Some(ConsumerRequirementDeclaration {
            declaration_id: format!("managed_provider:{consumer_id}:prompt_execution"),
            consumer_kind: ConsumerKind::ManagedProvider,
            consumer_id: consumer_id.to_string(),
            operation_kind: "prompt_execution".to_string(),
            profile_id: PROFILE_ID_SUBPROCESS_DEFAULT.to_string(),
            execution_mode: ExecutionMode::Subprocess,
            allowed_backend_kinds: vec![BackendKind::Subprocess],
            network_mode: Some(NetworkMode::Deny),
            approval_mode: Some(ApprovalMode::Allow),
            required_enforcement_strength: "declared_only".to_string(),
            active: true,
            source: Source::Builtin,
            ..Default::default()
        }),
        secret_scope,
        policy_record: Some(ConsumerPolicyRecord {
            policy_record_id: format!("policy_{consumer_id}"),
            consumer_kind: ConsumerKind::ManagedProvider,
            consumer_id: consumer_id.to_string(),
            operation_kind: "prompt_execution".to_string(),
            decision: DecisionResolution::Allow,
            approval_status: kura_sandbox::DecisionApprovalStatus::NotApplicable,
            secret_resolution: SecretResolution::NotApplicable,
            started_at: Utc::now(),
            status: PolicyRecordStatus::PreflightAllowed,
            ..Default::default()
        }),
    }
}

fn test_secret_scope_outcome(
    secret_ref: &str,
    scope: SecretEnvironmentScope,
    resolution: SecretResolution,
) -> SecretScopeOutcome {
    SecretScopeOutcome {
        consumer_kind: ConsumerKind::ManagedProvider,
        consumer_id: "test_consumer".to_string(),
        secret_ref: secret_ref.to_string(),
        environment_scope: scope,
        default_source: Some(SecretDefaultSource::KindDefault),
        default_rule_id: "test:default".to_string(),
        delivery_kind: "environment_variable".to_string(),
        redaction_rule: "value_redacted".to_string(),
        resolution,
    }
}

#[test]
fn explain_requires_approval_for_network() {
    let manager = test_manager(&temp_dir("explain_network"));
    let decision = manager
        .explain(ExecutionRequest {
            command: "echo".to_string(),
            args: vec!["hello".to_string()],
            access: AccessRequest {
                network_mode: Some(NetworkMode::Full),
                ..Default::default()
            },
            ..ExecutionRequest::default()
        })
        .expect("explain");
    assert_eq!(decision.resolution, DecisionResolution::Ask);
    assert!(decision.approval_required);
}

#[test]
fn list_profiles_includes_backend_capability_and_docker_availability() {
    let manager = test_manager(&temp_dir("list_profiles"));
    let profiles = manager.list_profiles();
    assert!(!profiles.is_empty(), "expected builtin sandbox profiles");
    let mut found_docker = false;
    for profile in &profiles {
        assert_eq!(
            profile.backend_capability.backend_kind,
            profile.backend_kind
        );
        if profile.backend_kind == BackendKind::Docker {
            found_docker = true;
            assert!(!profile.backend_capability.display_name.is_empty());
            assert!(!profile.backend_capability.host_prerequisites.is_empty());
        }
    }
    assert!(found_docker, "expected docker builtin profile");
}

#[test]
fn evaluate_access_decision_unsupported_docker_backend() {
    let profile = Profile {
        profile_id: "docker_test".to_string(),
        backend_kind: BackendKind::Docker,
        backend_capability: BackendCapabilityProfile {
            backend_kind: BackendKind::Docker,
            availability_status: BackendAvailabilityStatus::Unavailable,
            availability_reason: "docker CLI is not available on PATH".to_string(),
            ..Default::default()
        },
        ..Default::default()
    };
    let decision = evaluate_access_decision(&profile, "/tmp", &AccessRequest::default());
    assert_eq!(
        decision.selection_outcome,
        Some(BackendSelectionOutcome::Unsupported)
    );
    assert_eq!(
        decision.host_status,
        Some(BackendHostStatus::MissingPrerequisite)
    );
    assert_eq!(decision.mismatch_reason, "backend_unavailable");
    assert!(decision.explanation.contains("docker CLI is not available"));
}

#[test]
fn evaluate_access_decision_docker_access_rule_mismatch() {
    let profile = Profile {
        profile_id: "docker_test".to_string(),
        backend_kind: BackendKind::Docker,
        backend_capability: BackendCapabilityProfile {
            backend_kind: BackendKind::Docker,
            availability_status: BackendAvailabilityStatus::Available,
            ..Default::default()
        },
        ..Default::default()
    };
    let decision = evaluate_access_decision(
        &profile,
        "/tmp",
        &AccessRequest {
            network_mode: Some(NetworkMode::AllowList),
            allowed_hosts: vec!["example.com".to_string()],
            ..Default::default()
        },
    );
    assert_eq!(
        decision.selection_outcome,
        Some(BackendSelectionOutcome::Unsupported)
    );
    assert_eq!(decision.mismatch_reason, "backend_capability_mismatch");
}

#[test]
fn start_execution_completes_and_persists() {
    let dir = temp_dir("start_completes");
    let store = Arc::new(Mutex::new(SQLiteStore::new(&dir).expect("open store")));
    let manager = Manager::new(
        test_config(&dir),
        Some(Arc::clone(&store)),
        Bus::new(),
        Engine::new(),
    );
    let cwd = temp_dir("start_completes_cwd");

    let execution = manager
        .start_execution(ExecutionRequest {
            command: test_shell().to_string(),
            args: test_shell_args("printf 'hello sandbox'"),
            cwd: cwd.clone(),
            access: AccessRequest {
                read_roots: vec![cwd.clone()],
                write_roots: vec![cwd.clone()],
                ..Default::default()
            },
            ..ExecutionRequest::default()
        })
        .expect("start execution");

    let execution = wait_for_terminal(&manager, &execution.execution_id);
    assert_eq!(execution.status, ExecutionStatus::Completed);
    assert_eq!(execution.result.stdout, "hello sandbox");

    let records = store
        .lock()
        .unwrap()
        .list_sandbox_executions()
        .expect("list executions");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].execution_id, execution.execution_id);
}

#[test]
fn start_execution_creates_approval_and_denies_until_approved() {
    let dir = temp_dir("start_approval");
    let store = Arc::new(Mutex::new(SQLiteStore::new(&dir).expect("open store")));
    let manager = Manager::new(
        test_config(&dir),
        Some(Arc::clone(&store)),
        Bus::new(),
        Engine::new(),
    );
    let cwd = temp_dir("start_approval_cwd");

    let execution = manager
        .start_execution(ExecutionRequest {
            command: "echo".to_string(),
            args: vec!["needs-approval".to_string()],
            cwd: cwd.clone(),
            access: AccessRequest {
                read_roots: vec![cwd.clone()],
                network_mode: Some(NetworkMode::Full),
                ..Default::default()
            },
            reason: "network access requested".to_string(),
            ..ExecutionRequest::default()
        })
        .expect("start execution");

    assert_eq!(execution.status, ExecutionStatus::Denied);
    assert!(
        !execution.approval_id.is_empty(),
        "expected approval id on denied execution"
    );
    assert_eq!(execution.result.error_class, "approval_required");

    let approvals = store
        .lock()
        .unwrap()
        .list_approvals()
        .expect("list approvals");
    assert_eq!(approvals.len(), 1);
    assert_eq!(approvals[0].approval_id, execution.approval_id);
}

#[test]
fn cancel_execution_transitions_to_cancelled() {
    let manager = test_manager(&temp_dir("cancel_exec"));
    let cwd = temp_dir("cancel_exec_cwd");

    let execution = manager
        .start_execution(ExecutionRequest {
            command: test_shell().to_string(),
            args: test_shell_args("sleep 5"),
            cwd: cwd.clone(),
            access: AccessRequest {
                read_roots: vec![cwd.clone()],
                write_roots: vec![cwd.clone()],
                ..Default::default()
            },
            ..ExecutionRequest::default()
        })
        .expect("start execution");

    let (_, already_terminal) = manager
        .cancel_execution(&execution.execution_id)
        .expect("cancel");
    assert!(!already_terminal);

    let execution = wait_for_terminal(&manager, &execution.execution_id);
    assert_eq!(execution.status, ExecutionStatus::Cancelled);
    assert_eq!(execution.result.error_class, "cancelled");
}

#[test]
fn wait_execution_times_out() {
    let manager = test_manager(&temp_dir("wait_timeout"));
    let cwd = temp_dir("wait_timeout_cwd");
    let execution = manager
        .start_execution(ExecutionRequest {
            command: test_shell().to_string(),
            args: test_shell_args("sleep 5"),
            cwd: cwd.clone(),
            access: AccessRequest {
                read_roots: vec![cwd.clone()],
                write_roots: vec![cwd.clone()],
                ..Default::default()
            },
            ..ExecutionRequest::default()
        })
        .expect("start execution");

    let err = manager
        .wait_execution(&execution.execution_id, Duration::from_millis(150))
        .expect_err("timeout");
    assert_eq!(err, SandboxError::WaitTimeout);

    // Clean up the running process.
    manager
        .cancel_execution(&execution.execution_id)
        .expect("cancel");
    let terminal = wait_for_terminal(&manager, &execution.execution_id);
    assert_eq!(terminal.status, ExecutionStatus::Cancelled);
}

#[test]
fn close_cancels_active_executions() {
    let manager = test_manager(&temp_dir("close"));
    let cwd = temp_dir("close_cwd");
    let execution = manager
        .start_execution(ExecutionRequest {
            command: test_shell().to_string(),
            args: test_shell_args("sleep 5"),
            cwd: cwd.clone(),
            access: AccessRequest {
                read_roots: vec![cwd.clone()],
                write_roots: vec![cwd.clone()],
                ..Default::default()
            },
            ..ExecutionRequest::default()
        })
        .expect("start execution");

    // close() cancels everything but only waits ~2s best-effort (Go parity)
    // and returns Ok regardless; on a loaded host the runner thread may
    // settle the record slightly later, so assert through the test's own
    // terminal wait instead of immediately after close.
    manager.close().expect("close");
    let terminal = wait_for_terminal(&manager, &execution.execution_id);
    assert_eq!(terminal.status, ExecutionStatus::Cancelled);
    assert_eq!(manager.list_executions().len(), 1);
}

#[test]
fn evaluate_access_distinguishes_declared_managed_provider_roots() {
    let dir = temp_dir("managed_roots");
    let manager = test_manager(&dir);
    let managed_home = format!("{}/managed-provider-home", dir.trim_end_matches('/'));

    let allowed_path = format!("{managed_home}/.codex/auth.json");
    let allowed = manager.evaluate_access(
        PROFILE_ID_MANAGED_PROVIDER_CODEX,
        "",
        AccessRequest {
            read_roots: vec![allowed_path.clone()],
            ..Default::default()
        },
    );
    assert_eq!(allowed.resolution, DecisionResolution::Allow);

    let denied = manager.evaluate_access(
        PROFILE_ID_MANAGED_PROVIDER_CODEX,
        "",
        AccessRequest {
            read_roots: vec!["/etc/passwd".to_string()],
            ..Default::default()
        },
    );
    assert_eq!(denied.resolution, DecisionResolution::Deny);
}

#[test]
fn managed_provider_profiles_use_isolated_home_in_test_environment() {
    let dir = temp_dir("isolated_home");
    let manager = test_manager(&dir);
    let profile = manager
        .get_profile(PROFILE_ID_MANAGED_PROVIDER_CLAUDE)
        .expect("claude profile");
    let want_home = format!("{}/managed-provider-home", dir.trim_end_matches('/'));
    assert_eq!(profile.default_work_dir, want_home);
    assert!(within_any(
        &format!("{want_home}/.claude"),
        &profile.filesystem_policy.read_roots
    ));
}

#[test]
fn start_execution_redacts_secret_values_from_results() {
    let manager = test_manager(&temp_dir("redaction"));
    let cwd = temp_dir("redaction_cwd");

    let mut consumer = test_consumer_view(
        "redaction-skill",
        vec![SecretScopeOutcome {
            consumer_kind: ConsumerKind::Skill,
            consumer_id: "redaction-skill".to_string(),
            secret_ref: "EXEC_SKILL_TOKEN".to_string(),
            environment_scope: SecretEnvironmentScope::Test,
            default_source: Some(SecretDefaultSource::InstanceOverride),
            default_rule_id: "skill:redaction-skill".to_string(),
            delivery_kind: "environment_variable".to_string(),
            redaction_rule: "value_redacted".to_string(),
            resolution: SecretResolution::Resolved,
        }],
    );
    if let Some(declaration) = &mut consumer.declaration {
        declaration.consumer_kind = ConsumerKind::Skill;
        declaration.declaration_id = "skill:redaction-skill:tool_call.execute".to_string();
        declaration.operation_kind = "tool_call.execute".to_string();
        declaration.execution_mode = ExecutionMode::Subprocess;
    }
    if let Some(record) = &mut consumer.policy_record {
        record.policy_record_id = "policy_skill_redaction".to_string();
        record.consumer_kind = ConsumerKind::Skill;
        record.consumer_id = "redaction-skill".to_string();
        record.operation_kind = "tool_call.execute".to_string();
        record.secret_resolution = SecretResolution::Resolved;
    }

    let execution = manager
        .start_execution(ExecutionRequest {
            command: test_shell().to_string(),
            args: test_shell_args(
                "encoded=$(printf '%s' \"$EXEC_SKILL_TOKEN\" | base64 | tr -d '\n'); printf '%s\n%s' \"$EXEC_SKILL_TOKEN\" \"$encoded\"",
            ),
            cwd: cwd.clone(),
            env: HashMap::from([("EXEC_SKILL_TOKEN".to_string(), "top-secret-token".to_string())]),
            access: AccessRequest {
                read_roots: vec![cwd.clone()],
                write_roots: vec![cwd.clone()],
                ..Default::default()
            },
            consumer: Some(consumer),
            ..ExecutionRequest::default()
        })
        .expect("start execution");

    let execution = wait_for_terminal(&manager, &execution.execution_id);
    assert_eq!(execution.status, ExecutionStatus::Completed);
    assert!(
        !execution.result.stdout.contains("top-secret-token"),
        "raw secret leaked: {:?}",
        execution.result.stdout
    );
    assert!(
        !execution.result.stdout.contains("dG9wLXNlY3JldC10b2tlbg=="),
        "derived base64 leaked: {:?}",
        execution.result.stdout
    );
    assert_eq!(execution.result.stdout, "[REDACTED]\n[REDACTED]");
}

#[test]
fn restore_cancels_pending_managed_provider_finalization() {
    let dir = temp_dir("restore_pending");
    let store1 = Arc::new(Mutex::new(SQLiteStore::new(&dir).expect("open store 1")));
    let bus1 = Bus::new();
    let manager1 = Manager::new(
        test_config(&dir),
        Some(Arc::clone(&store1)),
        bus1.clone(),
        Engine::new(),
    );
    let cwd = temp_dir("restore_pending_cwd");

    let execution = manager1
        .start_execution(ExecutionRequest {
            command: test_shell().to_string(),
            args: test_shell_args("printf 'hello managed provider'"),
            cwd: cwd.clone(),
            metadata: HashMap::from([
                (
                    "managedProviderId".to_string(),
                    "claude_managed".to_string(),
                ),
                (
                    "managedProviderAction".to_string(),
                    "prompt_execution".to_string(),
                ),
                (
                    "managedProviderOperationId".to_string(),
                    "managed_provider_op_1".to_string(),
                ),
            ]),
            access: AccessRequest {
                read_roots: vec![cwd.clone()],
                write_roots: vec![cwd.clone()],
                ..Default::default()
            },
            ..ExecutionRequest::default()
        })
        .expect("start execution");

    let execution = wait_for_terminal(&manager1, &execution.execution_id);
    assert_eq!(execution.status, ExecutionStatus::Completed);
    assert!(
        awaits_managed_provider_finalization(&execution),
        "expected pending finalization marker"
    );
    let events1 = bus1.list(&Filter {
        category: "sandbox".to_string(),
        ..Default::default()
    });
    assert!(
        !events1
            .iter()
            .any(|event| event.name == "sandbox.execution_completed"),
        "terminal event must be deferred"
    );

    // The runner thread persists the terminal document after publishing the
    // in-memory state; wait for the marker before closing the store.
    let persist_deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        let records = store1
            .lock()
            .unwrap()
            .list_sandbox_executions()
            .expect("list executions");
        if records.iter().any(|record| {
            record.execution_id == execution.execution_id
                && record
                    .document
                    .contains("managedProviderFinalizationPending")
        }) {
            break;
        }
        if std::time::Instant::now() >= persist_deadline {
            panic!("terminal execution document was not persisted");
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    drop(manager1);
    drop(store1);

    let store2 = Arc::new(Mutex::new(SQLiteStore::new(&dir).expect("reopen store")));
    let bus2 = Bus::new();
    let manager2 = Manager::new(test_config(&dir), Some(store2), bus2.clone(), Engine::new());
    manager2.restore().expect("restore");

    let restored = manager2
        .get_execution(&execution.execution_id)
        .expect("restored execution");
    assert_eq!(restored.status, ExecutionStatus::Cancelled);
    assert_eq!(
        restored.result.error_code,
        "daemon_restarted_before_consumer_finalization"
    );
    assert!(
        !awaits_managed_provider_finalization(&restored),
        "marker must be cleared"
    );
    let events2 = bus2.list(&Filter {
        category: "sandbox".to_string(),
        ..Default::default()
    });
    assert!(
        events2
            .iter()
            .any(|event| event.name == "sandbox.execution_cancelled"),
        "expected recovery cancelled event"
    );
}

/// Minimal in-memory secret metadata store implementing kura_secrets::Store.
#[derive(Default)]
struct FakeSecretStore {
    secrets: Mutex<HashMap<String, kura_secrets::TenantSecret>>,
    versions: Mutex<HashMap<String, kura_secrets::SecretVersion>>,
}

impl FakeSecretStore {
    fn new() -> Self {
        Self::default()
    }
}

impl kura_secrets::Store for FakeSecretStore {
    fn create_secret<'a>(
        &'a self,
        secret: kura_secrets::TenantSecret,
        version: kura_secrets::SecretVersion,
    ) -> kura_secrets::BoxFuture<'a, kura_secrets::Result<()>> {
        Box::pin(async move {
            self.secrets
                .lock()
                .unwrap()
                .insert(secret.secret_ref.clone(), secret);
            self.versions
                .lock()
                .unwrap()
                .insert(version.secret_version_id.clone(), version);
            Ok(())
        })
    }

    fn update_secret_metadata<'a>(
        &'a self,
        secret: kura_secrets::TenantSecret,
    ) -> kura_secrets::BoxFuture<'a, kura_secrets::Result<()>> {
        Box::pin(async move {
            self.secrets
                .lock()
                .unwrap()
                .insert(secret.secret_ref.clone(), secret);
            Ok(())
        })
    }

    fn rotate_secret<'a>(
        &'a self,
        secret: kura_secrets::TenantSecret,
        _previous_version_id: &'a str,
        version: kura_secrets::SecretVersion,
    ) -> kura_secrets::BoxFuture<'a, kura_secrets::Result<()>> {
        Box::pin(async move {
            self.secrets
                .lock()
                .unwrap()
                .insert(secret.secret_ref.clone(), secret);
            self.versions
                .lock()
                .unwrap()
                .insert(version.secret_version_id.clone(), version);
            Ok(())
        })
    }

    fn disable_secret<'a>(
        &'a self,
        secret: kura_secrets::TenantSecret,
    ) -> kura_secrets::BoxFuture<'a, kura_secrets::Result<()>> {
        Box::pin(async move {
            self.secrets
                .lock()
                .unwrap()
                .insert(secret.secret_ref.clone(), secret);
            Ok(())
        })
    }

    fn get_secret_by_ref<'a>(
        &'a self,
        tenant_id: &'a str,
        secret_ref: &'a str,
    ) -> kura_secrets::BoxFuture<'a, kura_secrets::Result<Option<kura_secrets::TenantSecret>>> {
        Box::pin(async move {
            Ok(self
                .secrets
                .lock()
                .unwrap()
                .get(secret_ref)
                .cloned()
                .filter(|secret| secret.tenant_id == tenant_id))
        })
    }

    fn get_secret_version<'a>(
        &'a self,
        tenant_id: &'a str,
        secret_version_id: &'a str,
    ) -> kura_secrets::BoxFuture<'a, kura_secrets::Result<Option<kura_secrets::SecretVersion>>>
    {
        Box::pin(async move {
            Ok(self
                .versions
                .lock()
                .unwrap()
                .get(secret_version_id)
                .cloned()
                .filter(|version| version.tenant_id == tenant_id))
        })
    }

    fn list_secrets<'a>(
        &'a self,
        _tenant_id: &'a str,
    ) -> kura_secrets::BoxFuture<'a, kura_secrets::Result<Vec<kura_secrets::TenantSecret>>> {
        Box::pin(async move { Ok(self.secrets.lock().unwrap().values().cloned().collect()) })
    }
}

#[test]
fn sandbox_secret_scope_uses_active_tenant_and_fails_closed() {
    let dir = temp_dir("secret_scope");
    let store = Arc::new(Mutex::new(SQLiteStore::new(&dir).expect("open store")));
    let manager = Manager::new(
        test_config(&dir),
        Some(Arc::clone(&store)),
        Bus::new(),
        Engine::new(),
    );

    let backend_dir = temp_dir("secret_scope_backend");
    let backend = kura_secrets::LocalBackend::new(&backend_dir).expect("local backend");
    let secret_store = Arc::new(FakeSecretStore::new());
    let secret_manager = kura_secrets::Manager::new(secret_store.clone(), Arc::new(backend));
    futures::executor::block_on(secret_manager.create(kura_secrets::CreateInput {
        tenant_id: "ten_a".to_string(),
        secret_ref: "SANDBOX_TOKEN".to_string(),
        value: "tenant-a".to_string(),
        ..kura_secrets::CreateInput::default()
    }))
    .expect("create tenant A secret");
    futures::executor::block_on(secret_manager.create(kura_secrets::CreateInput {
        tenant_id: "ten_b".to_string(),
        secret_ref: "SANDBOX_TOKEN".to_string(),
        value: "tenant-b".to_string(),
        ..kura_secrets::CreateInput::default()
    }))
    .expect("create tenant B secret");
    manager.set_secret_manager(secret_manager);

    let cwd = temp_dir("secret_scope_cwd");
    let request = ExecutionRequest {
        command: "echo".to_string(),
        args: vec!["ok".to_string()],
        cwd: cwd.clone(),
        access: AccessRequest {
            read_roots: vec![cwd.clone()],
            write_roots: vec![cwd.clone()],
            ..Default::default()
        },
        consumer: Some(test_consumer_view(
            "sandbox_scope",
            vec![test_secret_scope_outcome(
                "SANDBOX_TOKEN",
                SecretEnvironmentScope::Test,
                SecretResolution::Unavailable,
            )],
        )),
        ..ExecutionRequest::default()
    };

    // Without tenant context the scope must fail closed.
    let denied = manager
        .explain(request.clone())
        .expect("explain without tenant");
    assert_eq!(denied.resolution, DecisionResolution::Deny);
    assert_eq!(
        denied.consumer.as_ref().expect("consumer").secret_scope[0].resolution,
        SecretResolution::Denied
    );

    // With the active tenant the secret resolves and the decision is allowed.
    let context = kura_identity::TenantContext {
        tenant_id: "ten_b".to_string(),
        principal_id: "prn_b".to_string(),
        ..Default::default()
    };
    let allowed = kura_identity::tenantctx::with_context(context, || {
        manager.explain(request).expect("explain with tenant")
    });
    assert_eq!(allowed.resolution, DecisionResolution::Allow);
    assert_eq!(
        allowed.consumer.as_ref().expect("consumer").secret_scope[0].resolution,
        SecretResolution::Resolved
    );
}

#[test]
fn capture_buffer_truncates() {
    use std::io::Write as _;
    let mut buffer = CaptureBuffer::new(16);
    buffer.write_all(b"0123456789abcdefghij").unwrap();
    assert!(buffer.truncated());
    assert_eq!(buffer.as_str(), "0123456789abcdef");

    let no_trunc = CaptureBuffer::new(1024);
    assert!(!no_trunc.truncated());
}

#[test]
fn redaction_covers_raw_and_derived_values() {
    let variants = derived_secret_variants("top-secret-token");
    assert!(
        variants.contains(&"dG9wLXNlY3JldC10b2tlbg==".to_string()),
        "base64 variant missing"
    );
    assert!(
        variants.contains(&hex_encode(b"top-secret-token")),
        "hex variant missing"
    );
    assert!(
        variants.iter().any(|v| v.len() == 32),
        "md5 variant missing"
    );

    let redacted = redact_secret_text("prefix dG9wLXNlY3JldC10b2tlbg== suffix", &variants);
    assert!(!redacted.contains("dG9wLXNlY3JldC10b2tlbg=="));
    assert!(redacted.contains("[REDACTED]"));
}

#[test]
fn approval_matches_execution_matches_sandbox_and_tool_call() {
    let profile = Profile {
        profile_id: "subprocess_default".to_string(),
        ..Default::default()
    };
    let execution = kura_sandbox::Execution {
        resource_kind: "capability".to_string(),
        resource_id: "shell".to_string(),
        ..Default::default()
    };
    let sandbox_approval = kura_policy::Approval {
        approval_id: "approval_1".to_string(),
        action: "sandbox.execute".to_string(),
        resource_kind: "sandbox_profile".to_string(),
        resource_id: "subprocess_default".to_string(),
        ..Default::default()
    };
    assert!(approval_matches_execution(
        &sandbox_approval,
        &execution,
        &profile
    ));

    let tool_approval = kura_policy::Approval {
        approval_id: "approval_2".to_string(),
        action: "tool_call.execute".to_string(),
        resource_kind: "capability".to_string(),
        resource_id: "shell".to_string(),
        ..Default::default()
    };
    assert!(approval_matches_execution(
        &tool_approval,
        &execution,
        &profile
    ));

    let unrelated = kura_policy::Approval {
        approval_id: "approval_3".to_string(),
        action: "calendar.create_event".to_string(),
        resource_kind: "calendar".to_string(),
        resource_id: "cal".to_string(),
        ..Default::default()
    };
    assert!(!approval_matches_execution(
        &unrelated, &execution, &profile
    ));
}

#[test]
fn clean_path_normalizes_like_filepath_clean() {
    assert_eq!(clean_path("/a/b/../c"), "/a/c");
    assert_eq!(clean_path("/a//b/./c"), "/a/b/c");
    assert_eq!(clean_path("a/b/../../c"), "c");
    assert_eq!(first_non_empty(&["", "  ", "value", "other"]), "value");
}

// ---------------------------------------------------------------------------
// The project profile
//
// A tool server that reads the project cannot run under `subprocess_default`,
// which scopes the filesystem to the daemon's own data directory. The way to
// make that work without a profile is to declare that the process needs no
// filesystem at all and hand it the project path on its command line, which
// passes the check by lying to it.
// ---------------------------------------------------------------------------

fn config_for_project(data_dir: &str, project_root: &str) -> kura_config::Config {
    kura_config::Config {
        project_root: project_root.to_string(),
        ..test_config(data_dir)
    }
}

#[test]
fn a_daemon_serving_no_project_has_no_project_profile() {
    // A profile granting access to a project should not exist where there is
    // none: it would be a standing route to the filesystem, waiting for
    // something to name it.
    let dir = temp_dir("noproject");
    let manager = test_manager(&dir);

    assert!(manager.get_profile(kura_sandbox::PROFILE_ID_PROJECT_TOOLS).is_none());
}

#[test]
fn a_configured_project_gets_a_profile_scoped_to_it() {
    let dir = temp_dir("project");
    let project = temp_dir("projectroot");
    let store = Arc::new(Mutex::new(SQLiteStore::new(&dir).expect("open store")));
    let manager = Manager::new(
        config_for_project(&dir, &project),
        Some(store),
        Bus::new(),
        Engine::new(),
    );

    let profile = manager
        .get_profile(kura_sandbox::PROFILE_ID_PROJECT_TOOLS)
        .expect("a configured project must have a profile");
    assert_eq!(profile.default_work_dir, project);
    assert!(profile.filesystem_policy.read_roots.contains(&project));
    assert!(profile.filesystem_policy.write_roots.contains(&project));
}

#[test]
fn the_project_profile_permits_working_in_the_project() {
    // The decision the MCP server start makes. Under the default profile this
    // is a deny, which is what left the tool server registered and never run.
    let dir = temp_dir("projectallow");
    let project = temp_dir("projectallowroot");
    let store = Arc::new(Mutex::new(SQLiteStore::new(&dir).expect("open store")));
    let manager = Manager::new(
        config_for_project(&dir, &project),
        Some(store),
        Bus::new(),
        Engine::new(),
    );

    let scoped = manager.get_profile(kura_sandbox::PROFILE_ID_PROJECT_TOOLS).unwrap();
    let (resolution, _) = kura_sandbox::evaluate_filesystem(
        &scoped,
        &project,
        &AccessRequest {
            read_roots: vec![project.clone()],
            ..AccessRequest::default()
        },
    );
    assert_eq!(resolution, DecisionResolution::Allow);

    // Why the profile is needed at all. Stated with an explicit path rather
    // than the temp directory used above: the default profile's temp root
    // covers anything under it, so a project that happens to live in temp is
    // allowed for a reason that has nothing to do with the project.
    let default_profile = manager.get_profile(PROFILE_ID_SUBPROCESS_DEFAULT).unwrap();
    let (denied, _) = kura_sandbox::evaluate_filesystem(
        &default_profile,
        "/Users/someone/Code/a-game",
        &AccessRequest::default(),
    );
    assert_eq!(denied, DecisionResolution::Deny, "the default profile should still refuse");
}

#[test]
fn the_project_profile_outlives_a_command_timeout() {
    // A tool server is a session, not a command. Under the default thirty
    // seconds it started healthy, was killed on the timeout, and the next tool
    // call found it gone -- the model was told the server was unavailable, with
    // nothing anywhere saying it had been killed for not exiting.
    let dir = temp_dir("projecttimeout");
    let project = temp_dir("projecttimeoutroot");
    let store = Arc::new(Mutex::new(SQLiteStore::new(&dir).expect("open store")));
    let manager = Manager::new(
        config_for_project(&dir, &project),
        Some(store),
        Bus::new(),
        Engine::new(),
    );

    let profile = manager.get_profile(kura_sandbox::PROFILE_ID_PROJECT_TOOLS).unwrap();
    let effective = kura_sandbox::effective_timeout(&profile, 0);
    assert!(effective >= 60 * 60 * 1000, "a session would be killed after {effective}ms");

    // Still bounded: a wedged child that nothing reaps is the other failure.
    assert!(effective <= 24 * 60 * 60 * 1000, "{effective}ms is effectively unlimited");

    let default_profile = manager.get_profile(PROFILE_ID_SUBPROCESS_DEFAULT).unwrap();
    assert!(
        kura_sandbox::effective_timeout(&default_profile, 0) < 60 * 1000,
        "the default profile should still be for commands that finish"
    );
}

#[test]
fn the_project_profile_does_not_widen_beyond_the_project() {
    // Scoped to the project and the data directory, not to the home directory
    // that contains them both.
    let dir = temp_dir("projectnarrow");
    let project = temp_dir("projectnarrowroot");
    let store = Arc::new(Mutex::new(SQLiteStore::new(&dir).expect("open store")));
    let manager = Manager::new(
        config_for_project(&dir, &project),
        Some(store),
        Bus::new(),
        Engine::new(),
    );

    let profile = manager.get_profile(kura_sandbox::PROFILE_ID_PROJECT_TOOLS).unwrap();
    assert!(!profile.filesystem_policy.allow_home_read);
    assert!(!profile.filesystem_policy.allow_home_write);
    assert_eq!(profile.network_policy.mode, NetworkMode::Deny);

    let (elsewhere, _) = kura_sandbox::evaluate_filesystem(
        &profile,
        "/etc",
        &AccessRequest::default(),
    );
    assert_eq!(elsewhere, DecisionResolution::Deny);
}
