//! Codex CLI bridge (port of `codex.go`).

use std::sync::Arc;

use chrono::Utc;
use kura_llm::{CancelToken, ProviderError, ProviderRequest, ProviderResponse, StreamChunk};
use kura_providers::{AuthMode, AuthState, AuthStatus, Family, Model};
use kura_sandbox::{
    AccessRequest, DecisionResolution, LocalStateAccessMode, ManagedProviderActionKind,
    NetworkMode, SensitiveLocalStateAccessSummary,
};
use futures::future::BoxFuture;

use crate::bridge::{Bridge, RunError, Runner, SandboxManager};
use crate::error::Error;
use crate::evaluate::{
    ManagedProviderOperationEvaluation, ManagedProviderOperationPlan, REQUESTED_BY_PREFIX,
    clone_local_state_summaries, consumer_view_json, denied_evaluation,
    evaluate_managed_provider_operation, finalize_managed_provider_execution_failure,
    finalize_managed_provider_execution_success, local_state_class_list, local_state_summary,
    new_managed_provider_operation_id, operation_metadata_from_plan,
};
use crate::helpers::{
    base_name, clone_roots, decode_jwt_payload, filepath_join, first_non_empty, latest_user_message,
    merge_string_maps, now_ptr,
};

pub const CODEX_PROVIDER_ID: &str = "codex_managed";

/// Go `codexBridge`.
pub struct CodexBridge {
    pub home_dir: String,
    pub cli_path: String,
    pub default_model: String,
    pub work_dir: String,
    pub runner: Arc<dyn Runner>,
    pub auth_path: String,
    pub models_cache_path: String,
    pub sandboxes: Option<Arc<dyn SandboxManager>>,
}

impl CodexBridge {
    /// Go `newCodexBridge`.
    pub fn new(
        home_dir: &str,
        cfg: &kura_config::Config,
        runner: Arc<dyn Runner>,
        sandboxes: Option<Arc<dyn SandboxManager>>,
    ) -> Self {
        CodexBridge {
            home_dir: home_dir.to_string(),
            cli_path: crate::helpers::first_available_path(&cfg.llm.codex.cli_path, &["codex"]),
            default_model: cfg.llm.codex.default_model.trim().to_string(),
            work_dir: crate::helpers::resolve_path(home_dir, &cfg.llm.codex.work_dir),
            runner,
            auth_path: filepath_join(&[home_dir, ".codex", "auth.json"]),
            models_cache_path: filepath_join(&[home_dir, ".codex", "models_cache.json"]),
            sandboxes,
        }
    }

    /// The effective default model (configured value, `config.toml`, or the
    /// first known model).
    #[must_use]
    pub fn default_model(&self) -> String {
        self.resolve_default_model(&[])
    }

    /// Go `codexBridge.models`.
    #[must_use]
    pub fn models(&self, available: bool) -> Vec<Model> {
        let mut items: Vec<Model> = Vec::new();
        if let Ok(raw) = std::fs::read(&self.models_cache_path) {
            #[derive(serde::Deserialize)]
            #[serde(rename_all = "snake_case")]
            struct ModelsCachePayload {
                models: Vec<CachedModel>,
            }
            #[derive(serde::Deserialize)]
            struct CachedModel {
                slug: String,
                display_name: String,
                description: String,
                supported_reasoning_levels: Vec<ReasoningLevel>,
            }
            #[derive(serde::Deserialize)]
            struct ReasoningLevel {
                effort: String,
            }
            if let Ok(payload) = serde_json::from_slice::<ModelsCachePayload>(&raw) {
                for cached in payload.models {
                    if cached.slug.trim().is_empty() {
                        continue;
                    }
                    let mut model = Model {
                        provider_id: self.provider_id(),
                        model_id: cached.slug.clone(),
                        display_name: first_non_empty(&[&cached.display_name, &cached.slug]),
                        description: cached.description,
                        source: "cache".to_string(),
                        available,
                        chat: true,
                        stream: true,
                        coding: true,
                        tool_use: false,
                        ..Model::default()
                    };
                    for level in cached.supported_reasoning_levels {
                        if !level.effort.trim().is_empty() {
                            model.reasoning_levels.push(level.effort);
                        }
                    }
                    items.push(model);
                }
            }
        }
        if items.is_empty() {
            let fallback = first_non_empty(&[&self.default_model, "gpt-5.4"]);
            items.push(Model {
                provider_id: self.provider_id(),
                model_id: fallback.clone(),
                display_name: fallback,
                source: "fallback".to_string(),
                available,
                chat: true,
                stream: true,
                coding: true,
                tool_use: false,
                ..Model::default()
            });
        }
        let default_model = self.resolve_default_model(&items);
        for item in &mut items {
            item.default = item.model_id == default_model;
        }
        items
    }

    /// Go `resolveDefaultModel`: configured value, else the `model = "..."`
    /// line in `config.toml`, else the first known model.
    #[must_use]
    pub fn resolve_default_model(&self, items: &[Model]) -> String {
        if !self.default_model.trim().is_empty() {
            return self.default_model.trim().to_string();
        }
        if let Ok(raw) = std::fs::read(filepath_join(&[&self.home_dir, ".codex", "config.toml"])) {
            for line in String::from_utf8_lossy(&raw).lines() {
                let line = line.trim();
                if let Some(rest) = line.strip_prefix("model = ") {
                    let model = rest.trim().trim_matches('"');
                    if !model.trim().is_empty() {
                        return model.to_string();
                    }
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
            login_command: vec![base_name(&self.cli_path), "login".to_string()],
            logout_command: vec![base_name(&self.cli_path), "logout".to_string()],
            ..AuthState::default()
        }
    }

    /// Go `authStatusEvaluation`.
    pub fn auth_status_evaluation(&self) -> Result<ManagedProviderOperationEvaluation, Error> {
        let plan = ManagedProviderOperationPlan {
            provider_id: self.provider_id(),
            action: ManagedProviderActionKind::AuthStatus,
            profile_id: kura_sandbox::PROFILE_ID_MANAGED_PROVIDER_CODEX.to_string(),
            requested_by: format!("{REQUESTED_BY_PREFIX}{}", self.provider_id()),
            reason: "managed provider local state inspection".to_string(),
            declared_read: vec![
                filepath_join(&[&self.home_dir, ".codex", "auth.json"]),
                filepath_join(&[&self.home_dir, ".codex", "models_cache.json"]),
                filepath_join(&[&self.home_dir, ".codex", "config.toml"]),
            ],
            access: AccessRequest {
                read_roots: vec![
                    self.auth_path.clone(),
                    self.models_cache_path.clone(),
                    filepath_join(&[&self.home_dir, ".codex", "config.toml"]),
                ],
                write_roots: Vec::new(),
                network_mode: Some(NetworkMode::Deny),
                allowed_hosts: Vec::new(),
                allowed_ports: Vec::new(),
                allow_loopback: false,
            },
            local_state: vec![
                local_state_summary(
                    &self.provider_id(),
                    ManagedProviderActionKind::AuthStatus,
                    "auth_file",
                    LocalStateAccessMode::Read,
                    &self.auth_path,
                    true,
                ),
                local_state_summary(
                    &self.provider_id(),
                    ManagedProviderActionKind::AuthStatus,
                    "models_cache",
                    LocalStateAccessMode::Read,
                    &self.models_cache_path,
                    false,
                ),
                local_state_summary(
                    &self.provider_id(),
                    ManagedProviderActionKind::AuthStatus,
                    "config_file",
                    LocalStateAccessMode::Read,
                    &filepath_join(&[&self.home_dir, ".codex", "config.toml"]),
                    false,
                ),
            ],
            sensitive_kinds: vec![
                "auth_file".to_string(),
                "models_cache".to_string(),
                "config_file".to_string(),
            ],
            ..ManagedProviderOperationPlan::default()
        };
        let evaluation = evaluate_managed_provider_operation(self.sandboxes.as_deref(), &plan)?;
        if evaluation.operation.decision != DecisionResolution::Allow {
            return Err(denied_evaluation(evaluation));
        }
        Ok(evaluation)
    }

    /// Go `promptExecutionEvaluation`.
    pub fn prompt_execution_evaluation(
        &self,
        include_config: bool,
    ) -> Result<ManagedProviderOperationEvaluation, Error> {
        let temp_dir = std::env::temp_dir().to_string_lossy().into_owned();
        let mut read_roots = vec![temp_dir.clone()];
        let mut local_state = vec![local_state_summary(
            &self.provider_id(),
            ManagedProviderActionKind::PromptExecution,
            "temp_output",
            LocalStateAccessMode::Write,
            &temp_dir,
            false,
        )];
        let mut sensitive_kinds = vec!["temp_output".to_string()];
        let config_path = filepath_join(&[&self.home_dir, ".codex", "config.toml"]);
        if include_config {
            read_roots.push(config_path.clone());
            local_state.push(local_state_summary(
                &self.provider_id(),
                ManagedProviderActionKind::PromptExecution,
                "config_file",
                LocalStateAccessMode::Read,
                &config_path,
                false,
            ));
            sensitive_kinds.push("config_file".to_string());
        }
        let mut declared_read = vec![temp_dir.clone()];
        if include_config {
            declared_read.push(config_path.clone());
        }
        let plan = ManagedProviderOperationPlan {
            provider_id: self.provider_id(),
            action: ManagedProviderActionKind::PromptExecution,
            profile_id: kura_sandbox::PROFILE_ID_MANAGED_PROVIDER_CODEX.to_string(),
            requested_by: format!("{REQUESTED_BY_PREFIX}{}", self.provider_id()),
            reason: "managed provider local state inspection".to_string(),
            declared_read,
            declared_write: vec![temp_dir.clone()],
            access: AccessRequest {
                read_roots,
                write_roots: vec![temp_dir],
                network_mode: Some(NetworkMode::Deny),
                allowed_hosts: Vec::new(),
                allowed_ports: Vec::new(),
                allow_loopback: false,
            },
            local_state,
            sensitive_kinds,
            ..Default::default()
        };
        let evaluation = evaluate_managed_provider_operation(self.sandboxes.as_deref(), &plan)?;
        if evaluation.operation.decision != DecisionResolution::Allow {
            return Err(denied_evaluation(evaluation));
        }
        Ok(evaluation)
    }

    /// Go `logoutEvaluation`.
    pub fn logout_evaluation(&self) -> Result<ManagedProviderOperationEvaluation, Error> {
        let config_path = filepath_join(&[&self.home_dir, ".codex", "config.toml"]);
        let plan = ManagedProviderOperationPlan {
            provider_id: self.provider_id(),
            action: ManagedProviderActionKind::Logout,
            profile_id: kura_sandbox::PROFILE_ID_MANAGED_PROVIDER_CODEX.to_string(),
            requested_by: format!("{REQUESTED_BY_PREFIX}{}", self.provider_id()),
            reason: "managed provider local state inspection".to_string(),
            declared_read: vec![
                filepath_join(&[&self.home_dir, ".codex", "models_cache.json"]),
                config_path.clone(),
            ],
            access: AccessRequest {
                read_roots: vec![self.models_cache_path.clone(), config_path.clone()],
                write_roots: Vec::new(),
                network_mode: Some(NetworkMode::Deny),
                allowed_hosts: Vec::new(),
                allowed_ports: Vec::new(),
                allow_loopback: false,
            },
            local_state: vec![
                local_state_summary(
                    &self.provider_id(),
                    ManagedProviderActionKind::Logout,
                    "models_cache",
                    LocalStateAccessMode::Read,
                    &self.models_cache_path,
                    false,
                ),
                local_state_summary(
                    &self.provider_id(),
                    ManagedProviderActionKind::Logout,
                    "config_file",
                    LocalStateAccessMode::Read,
                    &config_path,
                    false,
                ),
            ],
            sensitive_kinds: vec!["models_cache".to_string(), "config_file".to_string()],
            ..ManagedProviderOperationPlan::default()
        };
        let evaluation = evaluate_managed_provider_operation(self.sandboxes.as_deref(), &plan)?;
        if evaluation.operation.decision != DecisionResolution::Allow {
            return Err(denied_evaluation(evaluation));
        }
        Ok(evaluation)
    }

    /// Go `cliOperationPlan`.
    #[must_use]
    pub fn cli_operation_plan(
        &self,
        action: ManagedProviderActionKind,
        local_state: &[SensitiveLocalStateAccessSummary],
    ) -> ManagedProviderOperationPlan {
        let temp_dir = std::env::temp_dir().to_string_lossy().into_owned();
        ManagedProviderOperationPlan {
            operation_id: new_managed_provider_operation_id(),
            provider_id: self.provider_id(),
            action,
            profile_id: kura_sandbox::PROFILE_ID_MANAGED_PROVIDER_CODEX.to_string(),
            requested_by: format!("{REQUESTED_BY_PREFIX}{}", self.provider_id()),
            reason: "managed provider bridge execution".to_string(),
            access: AccessRequest {
                read_roots: clone_roots(&[
                    self.work_dir.clone(),
                    filepath_join(&[&self.home_dir, ".codex"]),
                    temp_dir.clone(),
                ]),
                write_roots: clone_roots(&[
                    self.work_dir.clone(),
                    filepath_join(&[&self.home_dir, ".codex"]),
                    temp_dir,
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

impl Bridge for CodexBridge {
    fn provider_id(&self) -> String {
        CODEX_PROVIDER_ID.to_string()
    }

    fn display_name(&self) -> String {
        "Codex CLI".to_string()
    }

    fn family(&self) -> Family {
        Family::CodexCLI
    }

    fn available(&self) -> bool {
        !self.cli_path.trim().is_empty()
    }

    fn auth_mode(&self) -> AuthMode {
        AuthMode::LocalCLIBridge
    }

    fn detect(&self, _cancel: &CancelToken) -> Result<(AuthState, Vec<Model>), Error> {
        let mut state = self.base_state();
        let now = Utc::now();
        state.last_checked_at = now;

        if self.cli_path.trim().is_empty() {
            let models = self.models(false);
            state.status = AuthStatus::Error;
            state.last_error = "codex CLI is not installed".to_string();
            return Ok((state, models));
        }

        match self.auth_status_evaluation() {
            Err(Error::Denied(denied)) => {
                state.status = AuthStatus::Error;
                state.last_error = denied.message.clone();
                state.metadata = denied.evaluation.metadata.clone();
                state.sandbox = consumer_view_json(denied.evaluation.consumer.as_ref());
                return Ok((state, Vec::new()));
            }
            Err(err) => {
                state.status = AuthStatus::Error;
                state.last_error = err.to_string();
                return Ok((state, Vec::new()));
            }
            Ok(evaluation) => {
                state.metadata = evaluation.metadata;
                state.sandbox = consumer_view_json(evaluation.consumer.as_ref());
            }
        }
        let mut models = self.models(false);

        let raw = match std::fs::read(&self.auth_path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                state.status = AuthStatus::LoginRequired;
                state.metadata = crate::evaluate::finalize_managed_provider_metadata(
                    &state.metadata,
                    "missing_local_state",
                );
                return Ok((state, models));
            }
            Err(err) => {
                state.status = AuthStatus::Error;
                state.last_error = err.to_string();
                state.metadata = crate::evaluate::finalize_managed_provider_metadata(
                    &state.metadata,
                    "missing_local_state",
                );
                return Ok((state, models));
            }
        };

        #[derive(serde::Deserialize, Default)]
        #[serde(rename_all = "snake_case", default)]
        struct CodexAuthFile {
            auth_mode: String,
            tokens: CodexTokens,
            last_refresh: String,
        }
        #[derive(serde::Deserialize, Default)]
        #[serde(rename_all = "snake_case", default)]
        struct CodexTokens {
            account_id: String,
            access_token: String,
            id_token: String,
            refresh_token: String,
        }
        let auth_file = match serde_json::from_slice::<CodexAuthFile>(&raw) {
            Ok(auth_file) => auth_file,
            Err(err) => {
                state.status = AuthStatus::Error;
                state.last_error = err.to_string();
                state.metadata = crate::evaluate::finalize_managed_provider_metadata(
                    &state.metadata,
                    "provider_auth_failed",
                );
                return Ok((state, models));
            }
        };

        if auth_file.tokens.access_token.trim().is_empty() {
            state.status = AuthStatus::LoginRequired;
            state.metadata = crate::evaluate::finalize_managed_provider_metadata(
                &state.metadata,
                "provider_auth_failed",
            );
            return Ok((state, models));
        }

        state.status = AuthStatus::Authenticated;
        state.auth_method = auth_file.auth_mode;
        state.account_id = auth_file.tokens.account_id;
        state.account_label = "ChatGPT".to_string();
        if let Some(claims) = decode_jwt_payload(&first_non_empty(&[
            &auth_file.tokens.id_token,
            &auth_file.tokens.access_token,
        ])) {
            if let Some(email) = claims.get("email").and_then(|value| value.as_str()) {
                state.account_label = email.to_string();
            }
            if let Some(auth_meta) = claims
                .get("https://api.openai.com/auth")
                .and_then(|value| value.as_object())
            {
                if let Some(plan) = auth_meta
                    .get("chatgpt_plan_type")
                    .and_then(|value| value.as_str())
                {
                    state.plan = plan.to_string();
                }
            }
        }
        if !auth_file.last_refresh.trim().is_empty() {
            if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(&auth_file.last_refresh) {
                state.last_authenticated_at = now_ptr(parsed.with_timezone(&Utc));
            }
        }

        models = self.models(true);
        let default_model = self.resolve_default_model(&models);
        for model in &mut models {
            model.default = model.model_id == default_model;
        }
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
        match self.logout_evaluation() {
            Err(Error::Denied(denied)) => {
                state.status = AuthStatus::Error;
                state.last_error = denied.message.clone();
                state.last_checked_at = Utc::now();
                state.metadata = denied.evaluation.metadata.clone();
                state.sandbox = consumer_view_json(denied.evaluation.consumer.as_ref());
                return Ok((state, Vec::new()));
            }
            Err(err) => {
                state.status = AuthStatus::Error;
                state.last_error = err.to_string();
                state.last_checked_at = Utc::now();
                return Ok((state, Vec::new()));
            }
            Ok(evaluation) => {
                state.metadata = evaluation.metadata;
                state.sandbox = consumer_view_json(evaluation.consumer.as_ref());
            }
        }
        let models = self.models(false);
        if self.cli_path.trim().is_empty() {
            state.status = AuthStatus::Error;
            state.last_error = "codex CLI is not installed".to_string();
            state.last_checked_at = Utc::now();
            return Ok((state, models));
        }
        let logout_operation = self.cli_operation_plan(ManagedProviderActionKind::Logout, &[]);
        state.metadata = merge_string_maps(
            &state.metadata,
            &operation_metadata_from_plan(&logout_operation),
        );
        let (result, run_err) = self.runner.run(
            cancel,
            &self.cli_path,
            &["logout".to_string()],
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
        Arc::new(CodexCLIProvider {
            bridge: Arc::new(self.clone_shallow()),
        })
    }

    fn default_model(&self) -> String {
        CodexBridge::default_model(self)
    }

    fn models(&self, available: bool) -> Vec<Model> {
        CodexBridge::models(self, available)
    }
}

/// Go `codexCLIProvider`.
pub struct CodexCLIProvider {
    pub bridge: Arc<CodexBridge>,
}

impl CodexBridge {
    /// Shallow clone for embedding in the provider handle (see
    /// `ClaudeBridge::clone_shallow`).
    #[must_use]
    fn clone_shallow(&self) -> CodexBridge {
        CodexBridge {
            home_dir: self.home_dir.clone(),
            cli_path: self.cli_path.clone(),
            default_model: self.default_model.clone(),
            work_dir: self.work_dir.clone(),
            runner: Arc::clone(&self.runner),
            auth_path: self.auth_path.clone(),
            models_cache_path: self.models_cache_path.clone(),
            sandboxes: self.sandboxes.clone(),
        }
    }
}

impl kura_llm::Provider for CodexCLIProvider {
    fn name(&self) -> &str {
        CODEX_PROVIDER_ID
    }

    fn complete<'a>(
        &'a self,
        request: ProviderRequest,
    ) -> BoxFuture<'a, Result<ProviderResponse, ProviderError>> {
        let bridge = Arc::clone(&self.bridge);
        Box::pin(async move { codex_complete(&bridge, request) })
    }

    fn stream<'a>(
        &'a self,
        request: ProviderRequest,
        emit: kura_llm::StreamEmitter<'a>,
    ) -> BoxFuture<'a, Result<ProviderResponse, ProviderError>> {
        let bridge = Arc::clone(&self.bridge);
        Box::pin(async move {
            let response = codex_complete(&bridge, request)?;
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

/// Removes the temp output file on drop (Go `defer os.Remove`).
struct TempFileGuard {
    path: std::path::PathBuf,
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Go `codexCLIProvider.Complete` body (sync, shared by complete/stream).
fn codex_complete(
    bridge: &CodexBridge,
    request: ProviderRequest,
) -> Result<ProviderResponse, ProviderError> {
    let model = request.model.trim().to_string();
    let evaluation = bridge
        .prompt_execution_evaluation(model.is_empty())
        .map_err(|err| {
            classify_cli_error(
                &RunError {
                    code: "sandbox_policy_denied".to_string(),
                    message: err.to_string(),
                    retryable: false,
                },
                "",
            )
        })?;
    let local_state =
        clone_local_state_summaries(&evaluation.operation.local_state_access_summaries);
    let model = if model.is_empty() {
        bridge.resolve_default_model(&[])
    } else {
        model
    };
    let mut operation = bridge.cli_operation_plan(ManagedProviderActionKind::PromptExecution, &local_state);

    let unique = uuid::Uuid::new_v4().simple().to_string();
    let temp_path = std::env::temp_dir().join(format!("kura-codex-output-{unique}.txt"));
    if let Err(err) = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)
    {
        return Err(ProviderError::provider("provider_error", err.to_string(), false));
    }
    let _guard = TempFileGuard { path: temp_path.clone() };

    let temp_dir = std::env::temp_dir().to_string_lossy().into_owned();
    let mut write_roots = operation.access.write_roots.clone();
    write_roots.push(temp_dir.clone());
    write_roots.push(temp_path.to_string_lossy().into_owned());
    operation.access.write_roots = clone_roots(&write_roots);
    operation.local_state.push(local_state_summary(
        &bridge.provider_id(),
        ManagedProviderActionKind::PromptExecution,
        "temp_output",
        LocalStateAccessMode::Write,
        &temp_path.to_string_lossy(),
        false,
    ));
    operation.sensitive_kinds = local_state_class_list(&operation.local_state);

    let args = vec![
        "exec".to_string(),
        "--skip-git-repo-check".to_string(),
        "--sandbox".to_string(),
        "read-only".to_string(),
        "--model".to_string(),
        model,
        "-o".to_string(),
        temp_path.to_string_lossy().into_owned(),
        latest_user_message(&request.messages),
    ];
    let (result, run_err) = bridge.runner.run(
        &request.cancel,
        &bridge.cli_path,
        &args,
        &bridge.work_dir,
        Some(&operation),
    );
    if let Some(run_err) = run_err {
        let err = classify_cli_error(&run_err, &result.stdout);
        finalize_managed_provider_execution_failure(bridge.sandboxes.as_deref(), &result, &err);
        return Err(err);
    }

    let raw = match std::fs::read(&temp_path) {
        Ok(raw) => raw,
        Err(err) => {
            let provider_err = ProviderError::provider("provider_error", err.to_string(), false);
            finalize_managed_provider_execution_failure(
                bridge.sandboxes.as_deref(),
                &result,
                &provider_err,
            );
            return Err(provider_err);
        }
    };
    let output = String::from_utf8_lossy(&raw).trim().to_string();
    if output.is_empty() {
        let err = ProviderError::provider(
            "upstream_invalid_response",
            "codex CLI returned empty output",
            false,
        );
        finalize_managed_provider_execution_failure(bridge.sandboxes.as_deref(), &result, &err);
        return Err(err);
    }
    finalize_managed_provider_execution_success(bridge.sandboxes.as_deref(), &result);
    Ok(ProviderResponse {
        // A borrowed CLI runs its own tool loop internally and reports only
        // the finished text; there is nothing for a caller to dispatch.
        tool_calls: Vec::new(),
        output,
        finish_reason: "stop".to_string(),
        usage: kura_llm::Usage::default(),
    })
}

/// Go `classifyCLIError`.
#[must_use]
pub fn classify_cli_error(run_err: &RunError, output: &str) -> ProviderError {
    if !run_err.code.is_empty() {
        return ProviderError::provider(
            first_non_empty(&[&run_err.code, "provider_error"]),
            first_non_empty(&[&run_err.message, &run_err.to_string()]),
            run_err.retryable,
        );
    }
    let mut message = output.trim().to_string();
    if message.is_empty() {
        message = run_err.to_string();
    }
    let lower = message.to_lowercase();
    if lower.contains("not logged in") || lower.contains("please run /login") {
        return ProviderError::provider("upstream_auth_failed", message, false);
    }
    // "logged in using" alone falls through to provider_error (Go behavior).
    if lower.contains("permission denied") {
        return ProviderError::provider("upstream_transport_error", message, true);
    }
    ProviderError::provider("provider_error", message, false)
}
