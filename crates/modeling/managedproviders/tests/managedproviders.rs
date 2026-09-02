//! Behavioral tests ported from the Go managedproviders package
//! (managedproviders_test.go) plus manager/store round-trips. Live CLI
//! execution is exercised through stub runners; sandbox routing is exercised
//! through a stub SandboxManager.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use kura_llm::{CancelToken, Message, MessageRole, Provider as _, ProviderRequest};
use kura_managedproviders::{
    Bridge, ClaudeBridge, ClaudeCLIProvider, CodexBridge, Manager, CLAUDE_PROVIDER_ID,
    CODEX_PROVIDER_ID, ExecRunner, ManagedProviderOperationPlan, Registry, RunError, RunResult,
    Runner, SandboxManager, SandboxRunner, build_managed_provider_consumer_view,
    classify_cli_error, new_managed_provider_operation_id,
};
use kura_providers::{AuthStatus, ManagedRegistry, ManagedBridge};
use kura_sandbox::{
    ApprovalMode, BackendKind, ConsumerContractView, ConsumerKind, Decision,
    DecisionApprovalStatus, DecisionResolution, ExecutionRequest, ExecutionStatus,
    LocalStateAccessMode, ManagedProviderActionKind, Profile, Result as SandboxResult,
    SensitiveLocalStateAccessSummary, SecretResolution,
};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn test_cfg(data_dir: &str) -> kura_config::Config {
    kura_config::Config {
        project_root: String::new(),
        environment: kura_config::Environment::Test,
        bind_addr: String::new(),
        data_dir: data_dir.to_string(),
        log_level: "info".to_string(),
        version: "test".to_string(),
        llm: kura_config::LlmConfig {
            claude: kura_config::ManagedCliProviderConfig {
                cli_path: "/usr/bin/claude".to_string(),
                ..Default::default()
            },
            codex: kura_config::ManagedCliProviderConfig {
                cli_path: "/usr/bin/codex".to_string(),
                ..Default::default()
            },
            ..Default::default()
        },
        connectors: kura_config::ConnectorConfig::default(),
    }
}

/// Go `runnerStub`.
struct RunnerStub {
    handler: Arc<dyn Fn(&str, &[String]) -> (RunResult, Option<RunError>) + Send + Sync>,
}

impl RunnerStub {
    fn new<F>(handler: F) -> Self
    where
        F: Fn(&str, &[String]) -> (RunResult, Option<RunError>) + Send + Sync + 'static,
    {
        RunnerStub { handler: Arc::new(handler) }
    }
}

impl Runner for RunnerStub {
    fn run(
        &self,
        _cancel: &CancelToken,
        cmd: &str,
        args: &[String],
        _workdir: &str,
        _operation: Option<&ManagedProviderOperationPlan>,
    ) -> (RunResult, Option<RunError>) {
        (self.handler)(cmd, args)
    }
}

fn ok_result(stdout: &str) -> (RunResult, Option<RunError>) {
    (RunResult { stdout: stdout.to_string(), ..RunResult::default() }, None)
}

/// A recorded sandbox execution request (the fields the Go tests assert on).
#[derive(Clone, Debug)]
struct RecordedExecution {
    profile_id: String,
    metadata: HashMap<String, String>,
    consumer: Option<ConsumerContractView>,
}

/// In-memory `SandboxManager` stub: records start_execution requests and
/// completes them with the configured stdout.
struct SandboxStub {
    stdout: String,
    allow: bool,
    executions: Mutex<Vec<RecordedExecution>>,
}

impl SandboxStub {
    fn new(stdout: &str, allow: bool) -> Self {
        SandboxStub {
            stdout: stdout.to_string(),
            allow,
            executions: Mutex::new(Vec::new()),
        }
    }

    fn recorded(&self) -> Vec<RecordedExecution> {
        self.executions.lock().unwrap().clone()
    }

    fn stub_profile(profile_id: &str) -> Profile {
        Profile {
            profile_id: profile_id.to_string(),
            backend_kind: BackendKind::Subprocess,
            approval_policy: kura_sandbox::ApprovalPolicy {
                mode: ApprovalMode::Allow,
                ..Default::default()
            },
            network_policy: kura_sandbox::NetworkPolicy {
                enforcement_mode: "declared_only".to_string(),
                ..Default::default()
            },
            ..Default::default()
        }
    }
}

impl SandboxManager for SandboxStub {
    fn start_execution(&self, request: ExecutionRequest) -> Result<kura_sandbox::Execution, String> {
        self.executions.lock().unwrap().push(RecordedExecution {
            profile_id: request.profile_id.clone(),
            metadata: request.metadata.clone(),
            consumer: request.consumer.clone(),
        });
        Ok(kura_sandbox::Execution {
            execution_id: "exec-1".to_string(),
            profile_id: request.profile_id,
            backend_kind: BackendKind::Subprocess,
            command: request.command,
            args: request.args,
            cwd: request.cwd,
            requested_by: request.requested_by,
            resource_kind: request.resource_kind,
            resource_id: request.resource_id,
            scope: request.scope,
            reason: request.reason,
            metadata: request.metadata,
            access: request.access,
            status: ExecutionStatus::Running,
            decision: Decision {
                decision_id: "decision-1".to_string(),
                resolution: if self.allow { DecisionResolution::Allow } else { DecisionResolution::Deny },
                approval_status: DecisionApprovalStatus::NotApplicable,
                effective_profile_id: SandboxStub::stub_profile("p").profile_id,
                effective_backend_kind: BackendKind::Subprocess,
                ..Default::default()
            },
            consumer: request.consumer,
            ..Default::default()
        })
    }

    fn wait_execution(&self, execution_id: &str) -> Result<kura_sandbox::Execution, String> {
        let (profile_id, metadata, consumer) = {
            let recorded = self.executions.lock().unwrap();
            let Some(entry) = recorded.first() else {
                return Err("no executions recorded".to_string());
            };
            (entry.profile_id.clone(), entry.metadata.clone(), entry.consumer.clone())
        };
        Ok(kura_sandbox::Execution {
            execution_id: execution_id.to_string(),
            profile_id: profile_id.clone(),
            backend_kind: BackendKind::Subprocess,
            status: ExecutionStatus::Completed,
            decision: Decision {
                decision_id: "decision-1".to_string(),
                resolution: if self.allow { DecisionResolution::Allow } else { DecisionResolution::Deny },
                approval_status: DecisionApprovalStatus::NotApplicable,
                effective_profile_id: profile_id.clone(),
                effective_backend_kind: BackendKind::Subprocess,
                ..Default::default()
            },
            result: SandboxResult {
                execution_id: execution_id.to_string(),
                status: ExecutionStatus::Completed,
                exit_code: Some(0),
                stdout: self.stdout.clone(),
                ..Default::default()
            },
            metadata,
            consumer,
            ..Default::default()
        })
    }

    fn finalize_execution(
        &self,
        _execution_id: &str,
        _finalization: kura_sandbox::ExecutionFinalization,
    ) -> Result<(), String> {
        Ok(())
    }

    fn evaluate_access(
        &self,
        profile_id: &str,
        _execution_id: &str,
        _access: &kura_sandbox::AccessRequest,
    ) -> Result<Decision, String> {
        Ok(Decision {
            decision_id: "decision-1".to_string(),
            resolution: if self.allow { DecisionResolution::Allow } else { DecisionResolution::Deny },
            approval_status: DecisionApprovalStatus::NotApplicable,
            effective_profile_id: profile_id.to_string(),
            effective_backend_kind: BackendKind::Subprocess,
            ..Default::default()
        })
    }

    fn get_profile(&self, profile_id: &str) -> Option<Profile> {
        Some(Self::stub_profile(profile_id))
    }

    fn persist_consumer_view(&self, _view: &ConsumerContractView) -> Result<(), String> {
        Ok(())
    }
}

fn write_text_file(path: &std::path::Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent dir");
    }
    std::fs::write(path, content).expect("write file");
}

// ---------------------------------------------------------------------------
// Bridge behavior (ported from the Go tests)
// ---------------------------------------------------------------------------

#[test]
fn claude_detect_login_required() {
    let home = std::env::temp_dir().join(format!("kura-mp-claude-{}", uuid_suffix()));
    let cfg = test_cfg(home.to_str().unwrap());
    let runner: Arc<dyn Runner> = Arc::new(RunnerStub::new(|_cmd, _args| {
        ok_result(r#"{"loggedIn":false,"authMethod":"none","apiProvider":"firstParty"}"#)
    }));
    let bridge = ClaudeBridge::new(home.to_str().unwrap(), &cfg, runner, None);
    let (state, models) = bridge.detect(&CancelToken::new()).expect("detect");
    assert_eq!(state.status, AuthStatus::LoginRequired);
    assert_eq!(models.len(), 2);
}

#[test]
fn claude_provider_maps_auth_failure() {
    let home = std::env::temp_dir().join(format!("kura-mp-claude-auth-{}", uuid_suffix()));
    let cfg = test_cfg(home.to_str().unwrap());
    let runner: Arc<dyn Runner> = Arc::new(RunnerStub::new(|_cmd, _args| {
        ok_result(r#"{"is_error":true,"result":"Not logged in · Please run /login"}"#)
    }));
    let bridge = Arc::new(ClaudeBridge::new(home.to_str().unwrap(), &cfg, runner, None));
    let provider = ClaudeCLIProvider { bridge };
    let request = ProviderRequest {
        model: "claude-opus-4-6".to_string(),
        messages: vec![Message { role: MessageRole::User, content: "hello".to_string(), ..Default::default() }],
        ..ProviderRequest::default()
    };
    let err = futures::executor::block_on(provider.complete(request)).unwrap_err();
    assert_eq!(err.code(), "upstream_auth_failed");
}

#[test]
fn codex_detect_and_model_catalog() {
    let home = std::env::temp_dir().join(format!("kura-mp-codex-{}", uuid_suffix()));
    write_text_file(&home.join(".codex/auth.json"), &serde_json::json!({
        "auth_mode": "chatgpt",
        "tokens": { "account_id": "acct_1", "access_token": "header.payload.sig" },
        "last_refresh": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
    }).to_string());
    write_text_file(&home.join(".codex/models_cache.json"), &serde_json::json!({
        "models": [{
            "slug": "gpt-5.4",
            "display_name": "GPT-5.4",
            "description": "Primary coding model",
            "supported_reasoning_levels": [{"effort": "medium"}, {"effort": "high"}],
        }],
    }).to_string());
    write_text_file(&home.join(".codex/config.toml"), "model = \"gpt-5.4\"");
    let cfg = test_cfg(home.to_str().unwrap());
    let runner: Arc<dyn Runner> = Arc::new(RunnerStub::new(|_cmd, _args| (RunResult::default(), None)));
    let bridge = CodexBridge::new(home.to_str().unwrap(), &cfg, runner, None);
    let (state, models) = bridge.detect(&CancelToken::new()).expect("detect");
    assert_eq!(state.status, AuthStatus::Authenticated);
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].model_id, "gpt-5.4");
    assert!(models[0].default);
    assert_eq!(models[0].reasoning_levels, vec!["medium".to_string(), "high".to_string()]);
}

#[test]
fn codex_provider_reads_cli_output_file() {
    let home = std::env::temp_dir().join(format!("kura-mp-codex-out-{}", uuid_suffix()));
    let cfg = test_cfg(home.to_str().unwrap());
    let runner: Arc<dyn Runner> = Arc::new(RunnerStub::new(|_cmd, args| {
        let mut output_path = String::new();
        for (index, arg) in args.iter().enumerate() {
            if arg == "-o" && index + 1 < args.len() {
                output_path = args[index + 1].clone();
            }
        }
        assert!(!output_path.is_empty(), "expected output path arg");
        std::fs::write(&output_path, "codex reply").expect("write output file");
        ok_result("ok")
    }));
    let bridge = CodexBridge::new(home.to_str().unwrap(), &cfg, runner, None);
    let provider = kura_managedproviders::CodexCLIProvider { bridge: Arc::new(bridge) };
    let request = ProviderRequest {
        model: "gpt-5.4".to_string(),
        messages: vec![Message { role: MessageRole::User, content: "hello".to_string(), ..Default::default() }],
        ..ProviderRequest::default()
    };
    let response = futures::executor::block_on(provider.complete(request)).expect("complete");
    assert_eq!(response.output, "codex reply");
}

#[test]
fn consumer_view_scopes_secrets_per_consumer_instance() {
    let evaluation = kura_managedproviders::ManagedProviderOperationEvaluation {
        declaration: kura_sandbox::ManagedProviderRequirementDeclaration {
            approval_mode: ApprovalMode::Allow,
            enforcement_strength: "declared_only".to_string(),
            sensitive_state_classes: vec!["settings_file".to_string()],
            active: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let sensitive = |provider_id: &str, state_class: &str, path: &str| SensitiveLocalStateAccessSummary {
        provider_id: provider_id.to_string(),
        action_kind: ManagedProviderActionKind::PromptExecution,
        state_class: state_class.to_string(),
        access_mode: LocalStateAccessMode::Read,
        path_summary: path.to_string(),
        declared: true,
        sensitive: true,
        redaction_rule: "class_summary_only".to_string(),
    };
    let claude = build_managed_provider_consumer_view(&ManagedProviderOperationPlan {
        operation_id: "operation_claude".to_string(),
        provider_id: CLAUDE_PROVIDER_ID.to_string(),
        action: ManagedProviderActionKind::PromptExecution,
        profile_id: kura_sandbox::PROFILE_ID_MANAGED_PROVIDER_CLAUDE.to_string(),
        requested_by: "test".to_string(),
        local_state: vec![sensitive(CLAUDE_PROVIDER_ID, "settings_file", "settings.json")],
        ..Default::default()
    }, Some(&evaluation));
    let codex = build_managed_provider_consumer_view(&ManagedProviderOperationPlan {
        operation_id: "operation_codex".to_string(),
        provider_id: CODEX_PROVIDER_ID.to_string(),
        action: ManagedProviderActionKind::PromptExecution,
        profile_id: kura_sandbox::PROFILE_ID_MANAGED_PROVIDER_CODEX.to_string(),
        requested_by: "test".to_string(),
        local_state: vec![sensitive(CODEX_PROVIDER_ID, "settings_file", "config.toml")],
        ..Default::default()
    }, Some(&evaluation));
    assert_eq!(claude.secret_scope.len(), 1);
    assert_eq!(codex.secret_scope.len(), 1);
    assert_eq!(claude.secret_scope[0].consumer_id, CLAUDE_PROVIDER_ID);
    assert_eq!(codex.secret_scope[0].consumer_id, CODEX_PROVIDER_ID);
    assert_ne!(claude.secret_scope[0].default_rule_id, codex.secret_scope[0].default_rule_id);
    assert_eq!(claude.policy_record.as_ref().unwrap().secret_resolution, SecretResolution::Resolved);
    assert_eq!(codex.policy_record.as_ref().unwrap().secret_resolution, SecretResolution::Resolved);
}

#[test]
fn registry_uses_managed_provider_home_under_data_dir_in_test_environment() {
    let base = std::env::temp_dir().join(format!("kura-mp-home-{}", uuid_suffix()));
    let data_dir = base.join("kura-data");
    let cfg = test_cfg(data_dir.to_str().unwrap());
    let registry = Registry::new(&cfg, None);
    assert_eq!(registry.home_dir(), kura_config::managed_provider_home_dir(&cfg));
    let claude = registry.claude_bridge().expect("claude bridge");
    assert_eq!(claude.home_dir, registry.home_dir());
    assert!(registry.get(CLAUDE_PROVIDER_ID).is_some());
    assert!(registry.get(CODEX_PROVIDER_ID).is_some());
}

#[test]
fn codex_detect_fails_closed_when_local_state_escapes_declaration() {
    let home = std::env::temp_dir().join(format!("kura-mp-codex-escape-{}", uuid_suffix()));
    write_text_file(&home.join(".codex/models_cache.json"), &serde_json::json!({
        "models": [{"slug": "gpt-5.4"}],
    }).to_string());
    write_text_file(&home.join(".codex/config.toml"), "model = \"gpt-5.4\"");
    let outside_path = std::env::temp_dir().join(format!("kura-outside-auth-{}.json", uuid_suffix()));
    write_text_file(&outside_path, &serde_json::json!({
        "auth_mode": "chatgpt",
        "tokens": { "account_id": "acct_1", "access_token": "secret-token" },
    }).to_string());
    let sandbox = Arc::new(SandboxStub::new("", true));
    let cfg = test_cfg(home.to_str().unwrap());
    let runner: Arc<dyn Runner> = Arc::new(RunnerStub::new(|_cmd, _args| (RunResult::default(), None)));
    let mut bridge = CodexBridge::new(home.to_str().unwrap(), &cfg, runner, Some(sandbox));
    bridge.auth_path = outside_path.to_string_lossy().into_owned();
    let (state, _models) = bridge.detect(&CancelToken::new()).expect("detect");
    assert_eq!(state.status, AuthStatus::Error);
    assert_eq!(
        state.metadata.get("failureClass").map(String::as_str),
        Some("policy_denied"),
    );
    let access_summary = state.metadata.get("localStateAccesses").map(String::as_str).unwrap_or("");
    assert!(!access_summary.contains("secret-token"), "metadata must be redacted: {access_summary}");
}

#[test]
fn registry_routes_claude_detect_through_sandbox() {
    let home = std::env::temp_dir().join(format!("kura-mp-sandbox-claude-{}", uuid_suffix()));
    let sandbox = Arc::new(SandboxStub::new(
        r#"{"loggedIn":false,"authMethod":"none","apiProvider":"firstParty"}"#,
        true,
    ));
    let cfg = test_cfg(home.to_str().unwrap());
    let runner: Arc<dyn Runner> = Arc::new(SandboxRunner {
        manager: Some(sandbox.clone()),
        profile_id: kura_sandbox::PROFILE_ID_MANAGED_PROVIDER_CLAUDE.to_string(),
        provider_id: CLAUDE_PROVIDER_ID.to_string(),
        roots: vec![
            home.join("work").to_string_lossy().into_owned(),
            home.join(".claude").to_string_lossy().into_owned(),
            std::env::temp_dir().to_string_lossy().into_owned(),
        ],
    });
    let bridge = Arc::new(ClaudeBridge::new(
        home.to_str().unwrap(),
        &cfg,
        runner,
        Some(sandbox.clone()),
    ));
    let registry = Registry::from_bridges(vec![bridge]);
    let bridge = registry.get(CLAUDE_PROVIDER_ID).expect("claude bridge");
    let (state, _models) = bridge.detect(&CancelToken::new()).expect("detect");
    assert_eq!(state.status, AuthStatus::LoginRequired);
    let recorded = sandbox.recorded();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].profile_id, kura_sandbox::PROFILE_ID_MANAGED_PROVIDER_CLAUDE);
    assert_eq!(
        recorded[0].metadata.get("managedProviderAction").map(String::as_str),
        Some("auth_status"),
    );
    let declaration = recorded[0].consumer.as_ref().and_then(|c| c.declaration.as_ref());
    let declaration = declaration.expect("consumer declaration");
    assert_eq!(declaration.consumer_kind, ConsumerKind::ManagedProvider);
    assert_eq!(declaration.consumer_id, CLAUDE_PROVIDER_ID);
}

#[test]
fn registry_implements_kura_providers_managed_registry() {
    let home = std::env::temp_dir().join(format!("kura-mp-mreg-{}", uuid_suffix()));
    let cfg = test_cfg(home.to_str().unwrap());
    let claude_runner: Arc<dyn Runner> = Arc::new(RunnerStub::new(|_cmd, _args| {
        ok_result(r#"{"loggedIn":false,"authMethod":"none","apiProvider":"firstParty"}"#)
    }));
    let codex_runner: Arc<dyn Runner> = Arc::new(RunnerStub::new(|_cmd, _args| {
        (RunResult::default(), None)
    }));
    let registry = Registry::from_bridges(vec![
        Arc::new(ClaudeBridge::new(home.to_str().unwrap(), &cfg, claude_runner, None)),
        Arc::new(CodexBridge::new(home.to_str().unwrap(), &cfg, codex_runner, None)),
    ]);
    let bridges = ManagedRegistry::list(&registry);
    assert_eq!(bridges.len(), 2);
    let claude = ManagedRegistry::get(&registry, CLAUDE_PROVIDER_ID).expect("claude");
    assert_eq!(claude.provider_id(), CLAUDE_PROVIDER_ID);
    assert_eq!(claude.family(), kura_providers::Family::ClaudeCodeCLI);
    let (state, models) = futures::executor::block_on(claude.detect()).expect("detect");
    assert_eq!(state.provider_id, CLAUDE_PROVIDER_ID);
    assert_eq!(models.len(), 2);
}

#[test]
fn exec_runner_runs_command_and_reports_exit_code() {
    #[cfg(unix)]
    {
        let runner = ExecRunner;
        let (result, err) = runner.run(
            &CancelToken::new(),
            "/bin/sh",
            &["-c".to_string(), "printf ok".to_string()],
            "",
            None,
        );
        assert!(err.is_none(), "expected success: {err:?}");
        assert_eq!(result.stdout, "ok");

        let (result, err) = runner.run(
            &CancelToken::new(),
            "/bin/sh",
            &["-c".to_string(), "exit 3".to_string()],
            "",
            None,
        );
        let err = err.expect("expected failure");
        assert_eq!(result.exit_code, 3);
        assert!(err.code.is_empty(), "generic error has empty code");
    }
}

#[test]
fn classify_cli_error_structured_vs_heuristic() {
    let structured = classify_cli_error(
        &RunError { code: "sandbox_policy_denied".to_string(), message: "denied".to_string(), retryable: false },
        "",
    );
    assert_eq!(structured.code(), "sandbox_policy_denied");
    let heuristic = classify_cli_error(
        &RunError { code: String::new(), message: String::new(), retryable: false },
        "please run /login to authenticate",
    );
    assert_eq!(heuristic.code(), "upstream_auth_failed");
    assert!(!heuristic.is_retryable());
    let transport = classify_cli_error(
        &RunError { code: String::new(), message: String::new(), retryable: false },
        "permission denied",
    );
    assert_eq!(transport.code(), "upstream_transport_error");
    assert!(transport.is_retryable());
}

#[test]
fn operation_id_is_timestamp_based() {
    let id = new_managed_provider_operation_id();
    assert!(id.starts_with("managed_provider_op_"), "id: {id}");
    assert!(!id.contains('.'), "id must not contain separators: {id}");
}

fn uuid_suffix() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
}

// ---------------------------------------------------------------------------
// Manager + store round-trips
// ---------------------------------------------------------------------------

/// Builds a registry with stub-runner bridges and a manager over a fresh store.
fn manager_with_store() -> (Manager, std::path::PathBuf) {
    let base = std::env::temp_dir().join(format!("kura-mp-store-{}", uuid_suffix()));
    let data_dir = base.join("kura-data");
    let cfg = test_cfg(data_dir.to_str().unwrap());
    let claude_runner: Arc<dyn Runner> = Arc::new(RunnerStub::new(|_cmd, _args| {
        ok_result(r#"{"loggedIn":false,"authMethod":"none","apiProvider":"firstParty"}"#)
    }));
    let codex_runner: Arc<dyn Runner> = Arc::new(RunnerStub::new(|_cmd, _args| {
        (RunResult::default(), None)
    }));
    let registry = Registry::from_bridges(vec![
        Arc::new(ClaudeBridge::new(&home(&cfg), &cfg, claude_runner, None)),
        Arc::new(CodexBridge::new(&home(&cfg), &cfg, codex_runner, None)),
    ]);
    let store = kura_store::SQLiteStore::new(data_dir.to_str().unwrap()).expect("open store");
    (Manager::new(cfg, registry, Some(store)), base)
}

fn home(cfg: &kura_config::Config) -> String {
    kura_config::managed_provider_home_dir(cfg)
}

fn setup_session(state: kura_setupwizard::SetupState) -> kura_setupwizard::SetupSession {
    let now = Utc::now();
    kura_setupwizard::SetupSession {
        setup_session_id: "session-1".to_string(),
        tenant_id: "tenant-1".to_string(),
        actor_principal_id: String::new(),
        target_id: kura_setupwizard::TARGET_OPENAI_COMPATIBLE.to_string(),
        target_kind: kura_setupwizard::TargetKind::Provider,
        setup_style: kura_setupwizard::SetupStyle::SubmittedSecret,
        state,
        reason_code: String::new(),
        retryable: false,
        remediation_owner: kura_setupwizard::RemediationOwner::ProductUser,
        safe_use_mode: kura_setupwizard::SafeUseMode::Blocked,
        allowed_capabilities: Vec::new(),
        current_attempt_id: String::new(),
        diagnostic_result_id: String::new(),
        diagnostic_run_id: String::new(),
        diagnostic_stage: String::new(),
        diagnostic_source_kind: String::new(),
        diagnostic_source_id: String::new(),
        diagnostic_allowed_use: Vec::new(),
        redaction_status: kura_setupwizard::RedactionStatus::Redacted,
        resource_refs: Vec::new(),
        redacted_evidence: HashMap::new(),
        oauth_state_ref: String::new(),
        created_at: now,
        updated_at: now,
        last_transition_at: now,
        last_transition_audit_id: String::new(),
        operator_remediation: String::new(),
        user_remediation: String::new(),
        unsupported_reason_code: String::new(),
    }
}

#[test]
fn manager_sync_persists_auth_states_and_models_to_store() {
    let (manager, _base) = manager_with_store();
    let results = manager.sync_managed_providers().expect("sync");
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].state.provider_id, CLAUDE_PROVIDER_ID);
    assert_eq!(results[0].state.status, AuthStatus::LoginRequired);
    assert_eq!(results[0].models.len(), 2);
    assert_eq!(results[1].state.provider_id, CODEX_PROVIDER_ID);

    let states = manager.restore_auth_states().expect("states");
    assert_eq!(states.len(), 2);
    let claude = states.iter().find(|s| s.provider_id == CLAUDE_PROVIDER_ID).expect("claude state");
    assert_eq!(claude.status, AuthStatus::LoginRequired);

    let claude_models = manager.restore_models_by_provider(CLAUDE_PROVIDER_ID).expect("models");
    assert_eq!(claude_models.len(), 2);
    assert!(claude_models.iter().all(|m| m.provider_id == CLAUDE_PROVIDER_ID));

    let all_models = manager.restore_models().expect("all models");
    assert_eq!(all_models.len(), 3); // 2 claude + 1 codex fallback
}

#[test]
fn manager_set_default_model_validates_and_persists_preference() {
    let (manager, _base) = manager_with_store();
    let err = manager
        .set_default_model(CLAUDE_PROVIDER_ID, "nonexistent-model")
        .unwrap_err();
    assert!(err.to_string().contains("not supported"), "err: {err}");

    let preference = manager
        .set_default_model(CLAUDE_PROVIDER_ID, "claude-sonnet-4-6")
        .expect("preference");
    assert_eq!(preference.default_model, "claude-sonnet-4-6");

    let prefs = manager.restore_preferences().expect("prefs");
    assert_eq!(prefs.len(), 1);
    assert_eq!(prefs[0].provider_id, CLAUDE_PROVIDER_ID);
    assert_eq!(prefs[0].default_model, "claude-sonnet-4-6");
}

#[test]
fn manager_run_check_persists_passed_check() {
    let (manager, _base) = manager_with_store();
    let check = manager
        .run_check(CLAUDE_PROVIDER_ID, "provider_check_1", "claude-opus-4-6", "ping")
        .expect("check");
    assert_eq!(check.status, kura_providers::CheckStatus::Passed);
    assert_eq!(check.model, "claude-opus-4-6");
    assert_eq!(check.family, kura_providers::Family::ClaudeCodeCLI);

    let stored = manager
        .get_check(CLAUDE_PROVIDER_ID, "provider_check_1")
        .expect("get check")
        .expect("check exists");
    assert_eq!(stored.status, kura_providers::CheckStatus::Passed);

    let listed = manager.list_checks(CLAUDE_PROVIDER_ID).expect("list");
    assert_eq!(listed.len(), 1);
}

#[test]
fn manager_run_check_unknown_provider_persists_failed_check() {
    let (manager, _base) = manager_with_store();
    let check = manager
        .run_check("nonexistent", "provider_check_2", "", "")
        .expect("check");
    assert_eq!(check.status, kura_providers::CheckStatus::Failed);
    assert_eq!(check.error_code, "provider_check_failed");
}

#[test]
fn manager_setup_gate_blocks_unready_session() {
    let (manager, _base) = manager_with_store();
    let blocked = setup_session(kura_setupwizard::SetupState::NotStarted);
    let err = manager
        .resolve_with_setup_gate(CLAUDE_PROVIDER_ID, "claude-opus-4-6", &blocked, "provider.use")
        .unwrap_err();
    assert!(err.to_string().contains("unavailable"), "err: {err}");

    let ready = setup_session(kura_setupwizard::SetupState::Ready);
    let decision = manager
        .resolve_with_setup_gate(CLAUDE_PROVIDER_ID, "claude-opus-4-6", &ready, "provider.use")
        .expect("decision");
    assert_eq!(decision.safe_use_mode, kura_setupwizard::SafeUseMode::Normal);
}

#[test]
fn manager_missing_store_is_noop_for_persistence() {
    let base = std::env::temp_dir().join(format!("kura-mp-nostore-{}", uuid_suffix()));
    let cfg = test_cfg(base.to_str().unwrap());
    let runner: Arc<dyn Runner> = Arc::new(RunnerStub::new(|_cmd, _args| {
        ok_result(r#"{"loggedIn":false,"authMethod":"none","apiProvider":"firstParty"}"#)
    }));
    let registry = Registry::from_bridges(vec![Arc::new(ClaudeBridge::new(&home(&cfg), &cfg, runner, None))]);
    let manager = Manager::new(cfg, registry, None);
    let results = manager.sync_managed_providers().expect("sync");
    assert_eq!(results.len(), 1);
    assert!(manager.restore_auth_states().expect("states").is_empty());
    assert!(manager.list_checks(CLAUDE_PROVIDER_ID).expect("checks").is_empty());
    let check = manager
        .run_check(CLAUDE_PROVIDER_ID, "provider_check_3", "claude-opus-4-6", "ping")
        .expect("check");
    assert_eq!(check.status, kura_providers::CheckStatus::Passed);
}
