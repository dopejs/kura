//! The bridge registry and execution runners (port of `bridges.go`).
//!
//! Go's `Bridge` interface is ported as the synchronous `Bridge` trait
//! (`context.Context` becomes `&kura_llm::CancelToken`). The Go
//! `sandbox.Manager` dependency is ported as the `SandboxManager` trait so
//! the sandbox-backed runner and preflight evaluation stay testable while the
//! concrete `kura-sandbox` manager is still being ported. The `Registry`
//! additionally implements `kura_providers::ManagedRegistry` through
//! `ManagedBridgeAdapter`, so it plugs straight into
//! `kura_providers::Manager`.

use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::Arc;

use kura_llm::CancelToken;
use kura_providers::{AuthMode, AuthState, Family, Model};
use futures::future::BoxFuture;

use crate::evaluate::{
    ManagedProviderOperationPlan, REQUESTED_BY_PREFIX, clone_access_request,
    build_managed_provider_consumer_view, operation_metadata_from_plan,
};
use crate::helpers::{clone_roots, first_non_empty};

/// Go `RunResult`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunResult {
    pub execution_id: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// Go `RunError`: a structured runner failure. An empty `code` denotes a
/// generic (unstructured) failure, which is how the Go port distinguishes
/// `*RunError` values from plain errors in `classify_cli_error`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Go: Error() returns Message when non-empty, otherwise Code.
        if !self.message.trim().is_empty() {
            f.write_str(&self.message)
        } else {
            f.write_str(&self.code)
        }
    }
}

impl std::error::Error for RunError {}

/// Go `Bridge` interface, synchronous.
pub trait Bridge: Send + Sync {
    fn provider_id(&self) -> String;
    fn display_name(&self) -> String;
    fn family(&self) -> Family;
    fn auth_mode(&self) -> AuthMode;
    /// Whether this bridge has the command-line tool it borrows.
    ///
    /// Answered from the resolved path rather than by running anything: it is
    /// consulted while building the provider inventory, and an inventory that
    /// spawned a process per entry would make listing providers as slow as
    /// using one.
    fn available(&self) -> bool;
    fn detect(&self, cancel: &CancelToken) -> Result<(AuthState, Vec<Model>), crate::error::Error>;
    fn start(&self, cancel: &CancelToken) -> Result<(AuthState, Vec<Model>), crate::error::Error>;
    fn complete(&self, cancel: &CancelToken) -> Result<(AuthState, Vec<Model>), crate::error::Error>;
    fn refresh(&self, cancel: &CancelToken) -> Result<(AuthState, Vec<Model>), crate::error::Error>;
    fn revoke(&self, cancel: &CancelToken) -> Result<(AuthState, Vec<Model>), crate::error::Error>;
    fn provider(&self) -> Arc<dyn kura_llm::Provider>;

    /// The bridge's effective default model (configured value, then local
    /// settings/cache), or "" when unknown. Used by the manager for checks and
    /// model validation (the Go manager resolves this through its profile
    /// state; the port surfaces it on the trait so the manager stays
    /// registry-only).
    fn default_model(&self) -> String {
        String::new()
    }

    /// The bridge's known model catalog (Go `claudeBridge.models` /
    /// `codexBridge.models`). Managed providers are fixed-selection, so the
    /// manager validates default-model choices against this list.
    fn models(&self, available: bool) -> Vec<Model> {
        let _ = available;
        Vec::new()
    }
}

/// Go `Runner` interface. The operation plan that Go threads through
/// `context.Context` is passed explicitly (the port is synchronous).
pub trait Runner: Send + Sync {
    fn run(
        &self,
        cancel: &CancelToken,
        cmd: &str,
        args: &[String],
        workdir: &str,
        operation: Option<&ManagedProviderOperationPlan>,
    ) -> (RunResult, Option<RunError>);
}

/// The subset of the Go `sandbox.Manager` the managed-provider runners and
/// preflight evaluation need. Implemented by the concrete `kura-sandbox`
/// manager once it is ported; tests use an in-memory stub.
pub trait SandboxManager: Send + Sync {
    fn start_execution(
        &self,
        request: kura_sandbox::ExecutionRequest,
    ) -> Result<kura_sandbox::Execution, String>;
    fn wait_execution(&self, execution_id: &str) -> Result<kura_sandbox::Execution, String>;
    fn finalize_execution(
        &self,
        execution_id: &str,
        finalization: kura_sandbox::ExecutionFinalization,
    ) -> Result<(), String>;
    fn evaluate_access(
        &self,
        profile_id: &str,
        execution_id: &str,
        access: &kura_sandbox::AccessRequest,
    ) -> Result<kura_sandbox::Decision, String>;
    fn get_profile(&self, profile_id: &str) -> Option<kura_sandbox::Profile>;
    fn persist_consumer_view(
        &self,
        view: &kura_sandbox::ConsumerContractView,
    ) -> Result<(), String>;
}

/// Go `execRunner`: runs the CLI with `std::process::Command` and returns
/// the combined output. Cancellation is honored before spawning and reported
/// if the token is already cancelled; killing an in-flight process on
/// cancellation is deferred (the port is synchronous, matching the
/// `context.Context -> sync` porting rule).
pub struct ExecRunner;

impl Runner for ExecRunner {
    fn run(
        &self,
        cancel: &CancelToken,
        cmd: &str,
        args: &[String],
        workdir: &str,
        _operation: Option<&ManagedProviderOperationPlan>,
    ) -> (RunResult, Option<RunError>) {
        if cancel.is_cancelled() {
            return (
                RunResult::default(),
                Some(RunError {
                    code: "cancelled".to_string(),
                    message: "context canceled".to_string(),
                    retryable: false,
                }),
            );
        }
        let mut command = Command::new(cmd);
        command.args(args);
        if !workdir.trim().is_empty() {
            command.current_dir(workdir);
        }
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let output = match command.output() {
            Ok(output) => output,
            Err(err) => {
                // Go returns the result plus the raw error when the process
                // cannot start at all.
                return (
                    RunResult::default(),
                    Some(RunError {
                        code: String::new(),
                        message: err.to_string(),
                        retryable: false,
                    }),
                );
            }
        };
        // Go CombinedOutput: stdout and stderr share one buffer.
        let mut combined = output.stdout;
        combined.extend_from_slice(&output.stderr);
        let combined = String::from_utf8_lossy(&combined).trim().to_string();
        let mut result = RunResult {
            execution_id: String::new(),
            stdout: combined.clone(),
            stderr: combined,
            exit_code: 0,
        };
        if output.status.success() {
            (result, None)
        } else {
            result.exit_code = output.status.code().unwrap_or(-1);
            let exit_code = result.exit_code;
            (
                result,
                Some(RunError {
                    code: String::new(),
                    message: format!("exit status {}", exit_code),
                    retryable: false,
                }),
            )
        }
    }
}

/// Go `sandboxRunner`: routes the CLI invocation through the sandbox manager
/// and maps execution outcomes onto `RunResult`/`RunError`. When no manager
/// is attached it falls back to `ExecRunner`, exactly like Go.
pub struct SandboxRunner {
    pub manager: Option<Arc<dyn SandboxManager>>,
    pub profile_id: String,
    pub provider_id: String,
    pub roots: Vec<String>,
}

impl Runner for SandboxRunner {
    fn run(
        &self,
        cancel: &CancelToken,
        cmd: &str,
        args: &[String],
        workdir: &str,
        operation: Option<&ManagedProviderOperationPlan>,
    ) -> (RunResult, Option<RunError>) {
        let Some(manager) = &self.manager else {
            return ExecRunner.run(cancel, cmd, args, workdir, operation);
        };
        let mut requested_by = format!("{REQUESTED_BY_PREFIX}{}", self.provider_id);
        let mut access = kura_sandbox::AccessRequest {
            read_roots: clone_roots(&self.roots),
            write_roots: clone_roots(&self.roots),
            network_mode: Some(kura_sandbox::NetworkMode::Full),
            allowed_hosts: Vec::new(),
            allowed_ports: Vec::new(),
            allow_loopback: true,
        };
        let mut metadata: HashMap<String, String> = HashMap::new();
        let mut consumer: Option<kura_sandbox::ConsumerContractView> = None;
        if let Some(operation) = operation {
            requested_by = first_non_empty(&[&operation.requested_by, &requested_by]);
            access = clone_access_request(&operation.access);
            metadata = operation_metadata_from_plan(operation);
            consumer = Some(build_managed_provider_consumer_view(operation, None));
        }
        let request = kura_sandbox::ExecutionRequest {
            profile_id: self.profile_id.clone(),
            command: cmd.to_string(),
            args: args.to_vec(),
            cwd: workdir.to_string(),
            requested_by,
            resource_kind: "provider".to_string(),
            resource_id: self.provider_id.clone(),
            scope: "managed_provider".to_string(),
            reason: "managed provider bridge execution".to_string(),
            metadata,
            access,
            consumer,
            ..kura_sandbox::ExecutionRequest::default()
        };
        let execution = match manager.start_execution(request) {
            Ok(execution) => execution,
            Err(err) => {
                return (
                    RunResult::default(),
                    Some(RunError {
                        code: String::new(),
                        message: err,
                        retryable: false,
                    }),
                );
            }
        };
        let execution = match manager.wait_execution(&execution.execution_id) {
            Ok(execution) => execution,
            Err(err) => {
                return (
                    RunResult::default(),
                    Some(RunError {
                        code: String::new(),
                        message: err,
                        retryable: false,
                    }),
                );
            }
        };
        let mut result = RunResult {
            execution_id: execution.execution_id.clone(),
            stdout: execution.result.stdout.trim().to_string(),
            stderr: execution.result.stderr.trim().to_string(),
            exit_code: 0,
        };
        if let Some(exit_code) = execution.result.exit_code {
            result.exit_code = exit_code as i32;
        }
        match execution.status {
            kura_sandbox::ExecutionStatus::Completed => (result, None),
            kura_sandbox::ExecutionStatus::Failed => {
                if execution.result.error_class == kura_sandbox::ErrorClass::ProcessFailed.as_str() {
                    (
                        result,
                        Some(RunError {
                            code: String::new(),
                            message: first_non_empty(&[
                                &execution.result.stderr,
                                &execution.result.stdout,
                                &execution.result.error,
                                "sandbox process failed",
                            ]),
                            retryable: false,
                        }),
                    )
                } else {
                    (
                        result,
                        Some(RunError {
                            code: first_non_empty(&[
                                &execution.result.error_code,
                                "sandbox_execution_failed",
                            ]),
                            message: first_non_empty(&[
                                &execution.result.error,
                                &execution.result.stderr,
                                &execution.result.stdout,
                                "sandbox execution failed",
                            ]),
                            retryable: execution.result.error_class
                                == kura_sandbox::ErrorClass::Timeout.as_str(),
                        }),
                    )
                }
            }
            kura_sandbox::ExecutionStatus::Cancelled => (
                result,
                Some(RunError {
                    code: first_non_empty(&[&execution.result.error_code, "sandbox_cancelled"]),
                    message: first_non_empty(&[
                        &execution.result.error,
                        "sandbox execution was cancelled",
                    ]),
                    retryable: false,
                }),
            ),
            kura_sandbox::ExecutionStatus::Denied => (
                result,
                Some(RunError {
                    code: first_non_empty(&[&execution.result.error_code, "sandbox_policy_denied"]),
                    message: first_non_empty(&[
                        &execution.result.error,
                        "sandbox execution was denied",
                    ]),
                    retryable: false,
                }),
            ),
            _ => (
                result,
                Some(RunError {
                    code: "sandbox_unknown_status".to_string(),
                    message: "sandbox execution returned unexpected status".to_string(),
                    retryable: false,
                }),
            ),
        }
    }
}

/// Go `Registry`: managed provider bridges in insertion order (Claude, then
/// Codex). Also remembers the resolved managed-provider home directory and
/// keeps concrete handles to the built-in bridges so callers (and tests, which
/// in Go type-assert `bridge.(*claudeBridge)`) can reach their fields.
pub struct Registry {
    bridges: HashMap<String, Arc<dyn Bridge>>,
    order: Vec<String>,
    home_dir: String,
    claude: Option<Arc<crate::claude::ClaudeBridge>>,
    codex: Option<Arc<crate::codex::CodexBridge>>,
}

impl Default for Registry {
    fn default() -> Self {
        Registry {
            bridges: HashMap::new(),
            order: Vec::new(),
            home_dir: String::new(),
            claude: None,
            codex: None,
        }
    }
}

impl Registry {
    /// Builds a registry from bridges in the given (insertion) order. Used by
    /// `Registry::new` and by tests that inject stub runners/sandbox stubs.
    pub fn from_bridges(items: Vec<Arc<dyn Bridge>>) -> Self {
        let mut registry = Registry::default();
        for bridge in items {
            let id = bridge.provider_id();
            registry.bridges.insert(id.clone(), bridge);
            registry.order.push(id);
        }
        registry
    }

    /// The concrete Claude bridge, when this registry was built by
    /// `Registry::new` (or registered a concrete Claude bridge).
    #[must_use]
    pub fn claude_bridge(&self) -> Option<Arc<crate::claude::ClaudeBridge>> {
        self.claude.clone()
    }

    /// The concrete Codex bridge, when this registry was built by
    /// `Registry::new` (or registered a concrete Codex bridge).
    #[must_use]
    pub fn codex_bridge(&self) -> Option<Arc<crate::codex::CodexBridge>> {
        self.codex.clone()
    }

    /// The managed-provider home directory resolved at construction.
    #[must_use]
    pub fn home_dir(&self) -> &str {
        &self.home_dir
    }

    /// Go `NewRegistry`: resolves the managed-provider home directory,
    /// builds the per-provider runners (sandbox-backed when a manager is
    /// attached, otherwise direct exec), and registers the Claude and Codex
    /// bridges in that order.
    pub fn new(cfg: &kura_config::Config, sandboxes: Option<Arc<dyn SandboxManager>>) -> Self {
        let mut home_dir = kura_config::managed_provider_home_dir(cfg);
        if home_dir.trim().is_empty() {
            home_dir = crate::helpers::user_home_dir().unwrap_or_default();
        }
        let mut registry = Registry::default();
        if !home_dir.trim().is_empty() {
            let _ = std::fs::create_dir_all(&home_dir);
        }
        registry.home_dir = home_dir.clone();

        let claude_work_dir = first_non_empty(&[
            &crate::helpers::resolve_path(&home_dir, &cfg.llm.claude.work_dir),
            &crate::helpers::home_fallback_workdir(&home_dir),
        ]);
        let codex_work_dir = first_non_empty(&[
            &crate::helpers::resolve_path(&home_dir, &cfg.llm.codex.work_dir),
            &crate::helpers::home_fallback_workdir(&home_dir),
        ]);

        let claude_runner: Arc<dyn Runner> = Arc::new(ExecRunner);
        let codex_runner: Arc<dyn Runner> = Arc::new(ExecRunner);
        let temp_dir = std::env::temp_dir().to_string_lossy().into_owned();
        let (claude_runner, codex_runner) = if let Some(manager) = &sandboxes {
            (
                Arc::new(SandboxRunner {
                    manager: Some(Arc::clone(manager)),
                    profile_id: kura_sandbox::PROFILE_ID_MANAGED_PROVIDER_CLAUDE.to_string(),
                    provider_id: crate::claude::CLAUDE_PROVIDER_ID.to_string(),
                    roots: vec![
                        claude_work_dir.clone(),
                        crate::helpers::filepath_join(&[&home_dir, ".claude"]),
                        temp_dir.clone(),
                    ],
                }) as Arc<dyn Runner>,
                Arc::new(SandboxRunner {
                    manager: Some(Arc::clone(manager)),
                    profile_id: kura_sandbox::PROFILE_ID_MANAGED_PROVIDER_CODEX.to_string(),
                    provider_id: crate::codex::CODEX_PROVIDER_ID.to_string(),
                    roots: vec![
                        codex_work_dir.clone(),
                        crate::helpers::filepath_join(&[&home_dir, ".codex"]),
                        temp_dir,
                    ],
                }) as Arc<dyn Runner>,
            )
        } else {
            (claude_runner, codex_runner)
        };

        let claude_bridge = Arc::new(crate::claude::ClaudeBridge::new(
            &home_dir,
            cfg,
            claude_runner,
            sandboxes.clone(),
        ));
        let codex_bridge = Arc::new(crate::codex::CodexBridge::new(
            &home_dir,
            cfg,
            codex_runner,
            sandboxes.clone(),
        ));
        let items: Vec<Arc<dyn Bridge>> =
            vec![Arc::clone(&claude_bridge) as Arc<dyn Bridge>, Arc::clone(&codex_bridge) as Arc<dyn Bridge>];
        for bridge in items {
            let id = bridge.provider_id();
            registry.bridges.insert(id.clone(), bridge);
            registry.order.push(id);
        }
        registry.claude = Some(claude_bridge);
        registry.codex = Some(codex_bridge);
        registry
    }

    /// Go `Registry.List`: bridges in insertion order.
    #[must_use]
    pub fn list(&self) -> Vec<Arc<dyn Bridge>> {
        self.order
            .iter()
            .filter_map(|id| self.bridges.get(id).cloned())
            .collect()
    }

    /// Go `Registry.Get`.
    #[must_use]
    pub fn get(&self, provider_id: &str) -> Option<Arc<dyn Bridge>> {
        self.bridges.get(provider_id.trim()).cloned()
    }

    /// Whether `provider_id` names a registered managed bridge.
    #[must_use]
    pub fn is_managed_provider(&self, provider_id: &str) -> bool {
        self.bridges.contains_key(provider_id.trim())
    }
}

/// Adapts the crate's synchronous `Bridge` into `kura_providers::ManagedBridge`
/// (async) so the registry plugs into `kura_providers::Manager`.
pub struct ManagedBridgeAdapter {
    bridge: Arc<dyn Bridge>,
}

impl ManagedBridgeAdapter {
    #[must_use]
    pub fn new(bridge: Arc<dyn Bridge>) -> Self {
        ManagedBridgeAdapter { bridge }
    }
}

impl kura_providers::ManagedBridge for ManagedBridgeAdapter {
    fn available(&self) -> bool {
        self.bridge.available()
    }

    fn provider_id(&self) -> String {
        self.bridge.provider_id()
    }

    fn display_name(&self) -> String {
        self.bridge.display_name()
    }

    fn family(&self) -> Family {
        self.bridge.family()
    }

    fn auth_mode(&self) -> AuthMode {
        self.bridge.auth_mode()
    }

    fn detect(&self) -> BoxFuture<'_, Result<(AuthState, Vec<Model>), kura_providers::ProvidersError>> {
        let bridge = Arc::clone(&self.bridge);
        Box::pin(async move {
            let cancel = CancelToken::new();
            bridge.detect(&cancel).map_err(crate::error::Error::map_providers_error)
        })
    }

    fn start(&self) -> BoxFuture<'_, Result<(AuthState, Vec<Model>), kura_providers::ProvidersError>> {
        let bridge = Arc::clone(&self.bridge);
        Box::pin(async move {
            let cancel = CancelToken::new();
            bridge.start(&cancel).map_err(crate::error::Error::map_providers_error)
        })
    }

    fn complete(&self) -> BoxFuture<'_, Result<(AuthState, Vec<Model>), kura_providers::ProvidersError>> {
        let bridge = Arc::clone(&self.bridge);
        Box::pin(async move {
            let cancel = CancelToken::new();
            bridge.complete(&cancel).map_err(crate::error::Error::map_providers_error)
        })
    }

    fn refresh(&self) -> BoxFuture<'_, Result<(AuthState, Vec<Model>), kura_providers::ProvidersError>> {
        let bridge = Arc::clone(&self.bridge);
        Box::pin(async move {
            let cancel = CancelToken::new();
            bridge.refresh(&cancel).map_err(crate::error::Error::map_providers_error)
        })
    }

    fn revoke(&self) -> BoxFuture<'_, Result<(AuthState, Vec<Model>), kura_providers::ProvidersError>> {
        let bridge = Arc::clone(&self.bridge);
        Box::pin(async move {
            let cancel = CancelToken::new();
            bridge.revoke(&cancel).map_err(crate::error::Error::map_providers_error)
        })
    }

    fn provider(&self) -> Arc<dyn kura_llm::Provider> {
        self.bridge.provider()
    }
}

impl kura_providers::ManagedRegistry for Registry {
    fn list(&self) -> Vec<Arc<dyn kura_providers::ManagedBridge>> {
        Registry::list(self)
            .into_iter()
            .map(|bridge| Arc::new(ManagedBridgeAdapter::new(bridge)) as Arc<dyn kura_providers::ManagedBridge>)
            .collect()
    }

    fn get(&self, provider_id: &str) -> Option<Arc<dyn kura_providers::ManagedBridge>> {
        Registry::get(self, provider_id)
            .map(|bridge| Arc::new(ManagedBridgeAdapter::new(bridge)) as Arc<dyn kura_providers::ManagedBridge>)
    }
}
