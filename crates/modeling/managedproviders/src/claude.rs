//! Claude Code CLI bridge (port of `claude.go`).

use std::sync::Arc;

use chrono::Utc;
use kura_llm::{CancelToken, ProviderError, ProviderRequest, ProviderResponse, StreamChunk, Usage};
use kura_providers::{AuthMode, AuthState, AuthStatus, Family, Model};
use kura_sandbox::{
    AccessRequest, DecisionResolution, LocalStateAccessMode, ManagedProviderActionKind,
    NetworkMode, SensitiveLocalStateAccessSummary,
};
use futures::future::BoxFuture;

use crate::bridge::{Bridge, RunError, Runner, SandboxManager};
use crate::codex::classify_cli_error;
use crate::error::Error;
use crate::evaluate::{
    ManagedProviderOperationEvaluation, ManagedProviderOperationPlan, REQUESTED_BY_PREFIX,
    clone_local_state_summaries, consumer_view_json, evaluate_managed_provider_operation,
    finalize_managed_provider_execution_failure, finalize_managed_provider_execution_success,
    local_state_class_list, local_state_summary, new_managed_provider_operation_id,
    operation_metadata_from_plan,
};
use crate::helpers::{
    clone_roots, clone_strings, filepath_join, latest_user_message, merge_string_maps, now_ptr,
};

pub const CLAUDE_PROVIDER_ID: &str = "claude_managed";

/// Outcome of `ClaudeBridge::settings_evaluation`. Mirrors Go's
/// `(evaluation, ok, err)` triple: `Skipped` = (zero, false, nil),
/// `Allowed` = (evaluation, true, nil), `Denied` = (evaluation, true,
/// error) — the denied variant carries the finalized evaluation because the
/// callers still surface its metadata and consumer view.
#[derive(Debug, Clone)]
pub enum SettingsEvaluation {
    /// The bridge has a configured default model, so no settings inspection is
    /// needed.
    Skipped,
    /// The sandbox approved the settings-file access.
    Allowed(ManagedProviderOperationEvaluation),
    /// The sandbox denied the settings-file access; the evaluation carries the
    /// policy-denied metadata.
    Denied(ManagedProviderOperationEvaluation),
}

/// Go `claudeBridge`.
pub struct ClaudeBridge {
    pub home_dir: String,
    pub cli_path: String,
    pub default_model: String,
    pub work_dir: String,
    pub runner: Arc<dyn Runner>,
    pub settings_path: String,
    pub sandboxes: Option<Arc<dyn SandboxManager>>,
}

impl ClaudeBridge {
    /// Go `newClaudeBridge`.
    pub fn new(
        home_dir: &str,
        cfg: &kura_config::Config,
        runner: Arc<dyn Runner>,
        sandboxes: Option<Arc<dyn SandboxManager>>,
    ) -> Self {
        ClaudeBridge {
            home_dir: home_dir.to_string(),
            cli_path: crate::helpers::first_available_path(&cfg.llm.claude.cli_path, &["claude"]),
            default_model: cfg.llm.claude.default_model.trim().to_string(),
            work_dir: crate::helpers::resolve_path(home_dir, &cfg.llm.claude.work_dir),
            runner,
            settings_path: filepath_join(&[home_dir, ".claude", "settings.json"]),
            sandboxes,
        }
    }

    /// The effective default model: configured value, else the model in
    /// `settings.json`, else the first known model.
    #[must_use]
    pub fn default_model(&self) -> String {
        self.resolve_default_model(&[])
    }

    /// Go `claudeBridge.models`: the two built-in Claude models.
    #[must_use]
    pub fn models(&self, available: bool) -> Vec<Model> {
        let mut items = vec![
            Model {
                provider_id: self.provider_id(),
                model_id: "claude-opus-4-6".to_string(),
                display_name: "claude-opus-4-6".to_string(),
                description: "Claude flagship coding model".to_string(),
                source: "builtin".to_string(),
                available,
                chat: true,
                stream: true,
                coding: true,
                tool_use: false,
                ..Model::default()
            },
            Model {
                provider_id: self.provider_id(),
                model_id: "claude-sonnet-4-6".to_string(),
                display_name: "claude-sonnet-4-6".to_string(),
                description: "Claude balanced coding model".to_string(),
                source: "builtin".to_string(),
                available,
                chat: true,
                stream: true,
                coding: true,
                tool_use: false,
                ..Model::default()
            },
        ];
        let default_model = self.resolve_default_model(&items);
        for item in &mut items {
            item.default = item.model_id == default_model;
        }
        items
    }

    /// Go `resolveDefaultModel`.
    #[must_use]
    pub fn resolve_default_model(&self, items: &[Model]) -> String {
        if !self.default_model.trim().is_empty() {
            return self.default_model.trim().to_string();
        }
        if let Ok(raw) = std::fs::read(&self.settings_path) {
            #[derive(serde::Deserialize, Default)]
            #[serde(default)]
            struct Settings {
                model: String,
            }
            if let Ok(settings) = serde_json::from_slice::<Settings>(&raw) {
                if !settings.model.trim().is_empty() {
                    return settings.model.trim().to_string();
                }
            }
        }
        items.first().map(|item| item.model_id.clone()).unwrap_or_default()
    }

    /// Go `baseState`.
    #[must_use]
    pub fn base_state(&self) -> AuthState {
        let cli_available = !self.cli_path.trim().is_empty();
        AuthState {
            provider_id: self.provider_id(),
            family: self.family(),
            auth_mode: self.auth_mode(),
            status: AuthStatus::Unknown,
            cli_path: self.cli_path.clone(),
            cli_available,
            login_command: vec![
                crate::helpers::base_name(&self.cli_path),
                "auth".to_string(),
                "login".to_string(),
                "--claudeai".to_string(),
            ],
            logout_command: vec![
                crate::helpers::base_name(&self.cli_path),
                "auth".to_string(),
                "logout".to_string(),
            ],
            ..AuthState::default()
        }
    }

    /// Go `settingsEvaluation`: only inspects `settings.json` when no
    /// default model is configured.
    pub fn settings_evaluation(
        &self,
        action: ManagedProviderActionKind,
    ) -> Result<SettingsEvaluation, Error> {
        if !self.default_model.trim().is_empty() {
            return Ok(SettingsEvaluation::Skipped);
        }
        let plan = ManagedProviderOperationPlan {
            provider_id: self.provider_id(),
            action,
            profile_id: kura_sandbox::PROFILE_ID_MANAGED_PROVIDER_CLAUDE.to_string(),
            requested_by: format!("{REQUESTED_BY_PREFIX}{}", self.provider_id()),
            reason: "managed provider local state inspection".to_string(),
            declared_read: vec![filepath_join(&[&self.home_dir, ".claude", "settings.json"])],
            access: AccessRequest {
                read_roots: vec![self.settings_path.clone()],
                write_roots: Vec::new(),
                network_mode: Some(NetworkMode::Deny),
                allowed_hosts: Vec::new(),
                allowed_ports: Vec::new(),
                allow_loopback: false,
            },
            local_state: vec![local_state_summary(
                &self.provider_id(),
                action,
                "settings_file",
                LocalStateAccessMode::Read,
                &self.settings_path,
                false,
            )],
            sensitive_kinds: vec!["settings_file".to_string()],
            ..ManagedProviderOperationPlan::default()
        };
        let evaluation = evaluate_managed_provider_operation(self.sandboxes.as_deref(), &plan)?;
        if evaluation.operation.decision != DecisionResolution::Allow {
            return Ok(SettingsEvaluation::Denied(ManagedProviderOperationEvaluation {
                metadata: crate::evaluate::finalize_managed_provider_metadata(
                    &evaluation.metadata,
                    kura_sandbox::ErrorClass::PolicyDenied.as_str(),
                ),
                ..evaluation
            }));
        }
        Ok(SettingsEvaluation::Allowed(evaluation))
    }

    /// Go `cliOperationPlan`.
    #[must_use]
    pub fn cli_operation_plan(
        &self,
        action: ManagedProviderActionKind,
        local_state: &[SensitiveLocalStateAccessSummary],
    ) -> ManagedProviderOperationPlan {
        ManagedProviderOperationPlan {
            operation_id: new_managed_provider_operation_id(),
            provider_id: self.provider_id(),
            action,
            profile_id: kura_sandbox::PROFILE_ID_MANAGED_PROVIDER_CLAUDE.to_string(),
            requested_by: format!("{REQUESTED_BY_PREFIX}{}", self.provider_id()),
            reason: "managed provider bridge execution".to_string(),
            access: AccessRequest {
                read_roots: clone_roots(&[
                    self.work_dir.clone(),
                    filepath_join(&[&self.home_dir, ".claude"]),
                ]),
                write_roots: clone_roots(&[
                    self.work_dir.clone(),
                    filepath_join(&[&self.home_dir, ".claude"]),
                ]),
                network_mode: Some(NetworkMode::Full),
                allowed_hosts: Vec::new(),
                allowed_ports: Vec::new(),
                allow_loopback: true,
            },
            local_state: clone_local_state_summaries(local_state),
            sensitive_kinds: local_state_class_list(local_state),
            ..Default::default()
        }
    }
}

impl Bridge for ClaudeBridge {
    fn provider_id(&self) -> String {
        CLAUDE_PROVIDER_ID.to_string()
    }

    fn display_name(&self) -> String {
        "Claude Code".to_string()
    }

    fn family(&self) -> Family {
        Family::ClaudeCodeCLI
    }

    fn available(&self) -> bool {
        !self.cli_path.trim().is_empty()
    }

    fn auth_mode(&self) -> AuthMode {
        AuthMode::LocalCLIBridge
    }

    fn detect(&self, cancel: &CancelToken) -> Result<(AuthState, Vec<Model>), Error> {
        let mut state = self.base_state();
        if self.cli_path.trim().is_empty() {
            state.status = AuthStatus::Error;
            state.last_error = "claude CLI is not installed".to_string();
            state.last_checked_at = Utc::now();
            return Ok((state, Vec::new()));
        }

        let mut auth_operation =
            self.cli_operation_plan(ManagedProviderActionKind::AuthStatus, &[]);
        match self.settings_evaluation(ManagedProviderActionKind::AuthStatus) {
            Ok(SettingsEvaluation::Skipped) => {}
            Ok(SettingsEvaluation::Denied(evaluation)) => {
                state.status = AuthStatus::Error;
                state.last_error = "sandbox denied managed provider local state access".to_string();
                state.last_checked_at = Utc::now();
                state.metadata = evaluation.metadata.clone();
                state.sandbox = consumer_view_json(evaluation.consumer.as_ref());
                return Ok((state, Vec::new()));
            }
            Ok(SettingsEvaluation::Allowed(evaluation)) => {
                state.metadata = evaluation.metadata.clone();
                state.sandbox = consumer_view_json(evaluation.consumer.as_ref());
                auth_operation.local_state =
                    clone_local_state_summaries(&evaluation.operation.local_state_access_summaries);
                auth_operation.sensitive_kinds =
                    clone_strings(&evaluation.operation.sensitive_state_classes);
                state.metadata = merge_string_maps(
                    &state.metadata,
                    &operation_metadata_from_plan(&auth_operation),
                );
            }
            Err(err) => {
                state.status = AuthStatus::Error;
                state.last_error = err.to_string();
                state.last_checked_at = Utc::now();
                return Ok((state, Vec::new()));
            }
        }

        let models = self.models(false);

        let (result, run_err) = self.runner.run(
            cancel,
            &self.cli_path,
            &["auth".to_string(), "status".to_string()],
            &self.work_dir,
            Some(&auth_operation),
        );
        let now = Utc::now();
        state.last_checked_at = now;
        if let Some(_err) = run_err {
            state.status = AuthStatus::Error;
            state.last_error = result.stdout.clone();
            return Ok((state, models));
        }

        #[derive(serde::Deserialize, Default)]
        #[serde(rename_all = "camelCase", default)]
        struct AuthStatusPayload {
            logged_in: bool,
            auth_method: String,
            api_provider: String,
        }
        let payload = match serde_json::from_str::<AuthStatusPayload>(result.stdout.trim()) {
            Ok(payload) => payload,
            Err(parse_err) => {
                finalize_managed_provider_execution_failure(
                    self.sandboxes.as_deref(),
                    &result,
                    &ProviderError::provider(
                        "provider_error",
                        parse_err.to_string(),
                        false,
                    ),
                );
                state.status = AuthStatus::Error;
                state.last_error = parse_err.to_string();
                return Ok((state, models));
            }
        };
        finalize_managed_provider_execution_success(self.sandboxes.as_deref(), &result);
        state.auth_method = payload.auth_method;
        let mut api_provider = std::collections::HashMap::new();
        api_provider.insert("apiProvider".to_string(), payload.api_provider);
        state.metadata = merge_string_maps(&state.metadata, &api_provider);
        if payload.logged_in {
            state.status = AuthStatus::Authenticated;
            state.account_label = "Anthropic".to_string();
            state.last_authenticated_at = now_ptr(now);
            let default_model = self.resolve_default_model(&models);
            let mut models = models;
            for model in &mut models {
                model.available = true;
                model.default = model.model_id == default_model;
            }
            return Ok((state, models));
        }

        state.status = AuthStatus::LoginRequired;
        state.last_error = String::new();
        Ok((state, models))
    }

    fn start(&self, cancel: &CancelToken) -> Result<(AuthState, Vec<Model>), Error> {
        let (mut state, models) = self.detect(cancel)?;
        state.status = AuthStatus::PendingLogin;
        state.last_checked_at = Utc::now();
        Ok((state, models))
    }

    fn complete(&self, cancel: &CancelToken) -> Result<(AuthState, Vec<Model>), Error> {
        self.detect(cancel)
    }

    fn refresh(&self, cancel: &CancelToken) -> Result<(AuthState, Vec<Model>), Error> {
        self.detect(cancel)
    }

    fn revoke(&self, cancel: &CancelToken) -> Result<(AuthState, Vec<Model>), Error> {
        let mut state = self.base_state();
        match self.settings_evaluation(ManagedProviderActionKind::Logout) {
            Ok(SettingsEvaluation::Skipped) => {}
            Ok(SettingsEvaluation::Denied(evaluation)) => {
                state.status = AuthStatus::Error;
                state.last_error = "sandbox denied managed provider local state access".to_string();
                state.last_checked_at = Utc::now();
                state.metadata = evaluation.metadata.clone();
                state.sandbox = consumer_view_json(evaluation.consumer.as_ref());
                return Ok((state, Vec::new()));
            }
            Ok(SettingsEvaluation::Allowed(evaluation)) => {
                state.metadata = evaluation.metadata.clone();
                state.sandbox = consumer_view_json(evaluation.consumer.as_ref());
            }
            Err(err) => {
                state.status = AuthStatus::Error;
                state.last_error = err.to_string();
                state.last_checked_at = Utc::now();
                return Ok((state, Vec::new()));
            }
        }
        let models = self.models(false);
        if self.cli_path.trim().is_empty() {
            state.status = AuthStatus::Error;
            state.last_error = "claude CLI is not installed".to_string();
            state.last_checked_at = Utc::now();
            return Ok((state, models));
        }
        let logout_operation = self.cli_operation_plan(ManagedProviderActionKind::Logout, &[]);
        state.metadata = operation_metadata_from_plan(&logout_operation);
        let (result, run_err) = self.runner.run(
            cancel,
            &self.cli_path,
            &["auth".to_string(), "logout".to_string()],
            &self.work_dir,
            Some(&logout_operation),
        );
        if let Some(err) = run_err {
            state.status = AuthStatus::Error;
            state.last_error = err.to_string();
            state.metadata = crate::evaluate::finalize_managed_provider_metadata(
                &state.metadata,
                "process_failed",
            );
            state.last_checked_at = Utc::now();
            return Ok((state, models));
        }
        finalize_managed_provider_execution_success(self.sandboxes.as_deref(), &result);
        state.status = AuthStatus::Revoked;
        state.last_checked_at = Utc::now();
        Ok((state, models))
    }

    fn provider(&self) -> Arc<dyn kura_llm::Provider> {
        Arc::new(ClaudeCLIProvider {
            bridge: Arc::new(self.clone_shallow()),
        })
    }

    fn default_model(&self) -> String {
        ClaudeBridge::default_model(self)
    }

    fn models(&self, available: bool) -> Vec<Model> {
        ClaudeBridge::models(self, available)
    }
}

/// Go `claudeCLIProvider`.
pub struct ClaudeCLIProvider {
    pub bridge: Arc<ClaudeBridge>,
}

impl ClaudeBridge {
    /// A shallow clone for embedding inside the provider handle (the runner and
    /// sandbox manager are shared `Arc`s, so this is cheap and keeps the
    /// provider independent of the registry's bridge handle).
    #[must_use]
    fn clone_shallow(&self) -> ClaudeBridge {
        ClaudeBridge {
            home_dir: self.home_dir.clone(),
            cli_path: self.cli_path.clone(),
            default_model: self.default_model.clone(),
            work_dir: self.work_dir.clone(),
            runner: Arc::clone(&self.runner),
            settings_path: self.settings_path.clone(),
            sandboxes: self.sandboxes.clone(),
        }
    }
}

impl kura_llm::Provider for ClaudeCLIProvider {
    fn name(&self) -> &str {
        CLAUDE_PROVIDER_ID
    }

    fn complete<'a>(
        &'a self,
        request: ProviderRequest,
    ) -> BoxFuture<'a, Result<ProviderResponse, ProviderError>> {
        let bridge = Arc::clone(&self.bridge);
        Box::pin(async move { claude_complete(&bridge, request) })
    }

    fn stream<'a>(
        &'a self,
        request: ProviderRequest,
        emit: kura_llm::StreamEmitter<'a>,
    ) -> BoxFuture<'a, Result<ProviderResponse, ProviderError>> {
        let bridge = Arc::clone(&self.bridge);
        Box::pin(async move {
            let response = claude_complete(&bridge, request)?;
            if !response.output.trim().is_empty() {
                let chunk = StreamChunk {
                    delta: response.output.clone(),
                    output: response.output.clone(),
                    ..StreamChunk::default()
                };
                emit(chunk)?;
            }
            Ok(response)
        })
    }
}

/// Go `claudeCLIProvider.Complete` body (sync, shared by complete/stream).
fn claude_complete(bridge: &ClaudeBridge, request: ProviderRequest) -> Result<ProviderResponse, ProviderError> {
    let mut model = request.model.trim().to_string();
    let mut local_state: Vec<SensitiveLocalStateAccessSummary> = Vec::new();
    if model.is_empty() {
        match bridge.settings_evaluation(ManagedProviderActionKind::PromptExecution) {
            Ok(SettingsEvaluation::Allowed(evaluation)) => {
                local_state =
                    clone_local_state_summaries(&evaluation.operation.local_state_access_summaries);
            }
            Ok(SettingsEvaluation::Denied(_)) => {
                return Err(classify_cli_error(
                    &RunError {
                        code: "sandbox_policy_denied".to_string(),
                        message: "sandbox denied managed provider local state access".to_string(),
                        retryable: false,
                    },
                    "",
                ));
            }
            Ok(SettingsEvaluation::Skipped) => {}
            Err(err) => {
                return Err(classify_cli_error(
                    &RunError {
                        code: "sandbox_policy_denied".to_string(),
                        message: err.to_string(),
                        retryable: false,
                    },
                    "",
                ));
            }
        }
        model = bridge.resolve_default_model(&[]);
    }
    let operation =
        bridge.cli_operation_plan(ManagedProviderActionKind::PromptExecution, &local_state);
    let args = vec![
        "-p".to_string(),
        "--output-format".to_string(),
        "json".to_string(),
        "--model".to_string(),
        model.clone(),
        "--allowedTools".to_string(),
        String::new(),
        "--permission-mode".to_string(),
        "dontAsk".to_string(),
        latest_user_message(&request.messages),
    ];
    let (result, run_err) =
        bridge.runner.run(&request.cancel, &bridge.cli_path, &args, &bridge.work_dir, Some(&operation));
    let response = parse_claude_result(&result, run_err.as_ref());
    match response {
        Ok(response) => {
            finalize_managed_provider_execution_success(bridge.sandboxes.as_deref(), &result);
            Ok(response)
        }
        Err(provider_err) => {
            finalize_managed_provider_execution_failure(
                bridge.sandboxes.as_deref(),
                &result,
                &provider_err,
            );
            Err(provider_err)
        }
    }
}

/// Go `parseClaudeResult`.
#[must_use]
pub fn parse_claude_result(
    result: &crate::bridge::RunResult,
    run_err: Option<&RunError>,
) -> Result<ProviderResponse, ProviderError> {
    #[derive(serde::Deserialize, Default)]
    #[serde(rename_all = "snake_case", default)]
    struct ClaudePayload {
        is_error: bool,
        result: String,
        usage: ClaudeUsage,
    }
    #[derive(serde::Deserialize, Default)]
    #[serde(rename_all = "snake_case", default)]
    struct ClaudeUsage {
        input_tokens: i64,
        output_tokens: i64,
    }
    if let Ok(payload) = serde_json::from_str::<ClaudePayload>(result.stdout.trim()) {
        if payload.is_error {
            let mut code = "provider_error";
            if payload.result.to_lowercase().contains("not logged in") {
                code = "upstream_auth_failed";
            }
            return Err(ProviderError::provider(code, payload.result, false));
        }
        return Ok(ProviderResponse {
        // A borrowed CLI runs its own tool loop internally and reports only
        // the finished text; there is nothing for a caller to dispatch.
        tool_calls: Vec::new(),
            output: payload.result,
            finish_reason: "stop".to_string(),
            usage: Usage {
                input_tokens: payload.usage.input_tokens,
                output_tokens: payload.usage.output_tokens,
                total_tokens: payload.usage.input_tokens + payload.usage.output_tokens,
            },
        });
    }
    if let Some(run_err) = run_err {
        return Err(classify_cli_error(run_err, &result.stdout));
    }
    Err(ProviderError::provider(
        "provider_error",
        format!("decode claude CLI response: {}", result.stdout.trim()),
        false,
    ))
}
