//! Provider manager (port of `manager.go`).

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use kura_config::{AccountProtocol, AccountProviderConfig, LlmConfig};
use kura_llm::{CancelToken, CreateDispatchInput, Dispatcher, Message, MessageRole, PrepareError};
use kura_setupwizard::{Service, ServiceDependencies, SetupSession, new_service};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::types::*;

const OPENAI_COMPATIBLE_PROVIDER_NAME: &str = "openai_compatible";

#[derive(Debug, Clone, Default)]
pub struct ResolvedDispatch {
    pub provider_id: String,
    pub model: String,
    pub timeout_ms: i64,
    pub max_retries: i64,
    pub endpoint: String,
    pub profile: Profile,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub check_id: String,
    pub provider_id: String,
    pub family: Family,
    pub auth_mode: AuthMode,
    pub status: CheckStatus,
    pub model: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub endpoint: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error_class: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error_code: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error_message: String,
    pub usage: kura_llm::Usage,
    pub created_at: chrono::DateTime<Utc>,
    pub completed_at: chrono::DateTime<Utc>,
}

// serde(default): Go decodes the check request into the zero value, so a
// partial body (e.g. only `model`) must not be rejected at the API boundary.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CheckInput {
    pub model: String,
    pub prompt: String,
}

#[derive(Debug, Clone)]
pub struct SyncResult {
    pub state: AuthState,
    pub models: Vec<Model>,
}

#[derive(Clone, Copy)]
enum ManagedAction {
    Start,
    Complete,
    Refresh,
    Revoke,
}

#[derive(Default)]
struct Inner {
    /// Accounts registered since the daemon started.
    ///
    /// Held here rather than in the immutable config because they change while
    /// it runs: a provider added mid-session must take effect without a
    /// restart, and restarting to pick one up would end whatever run is in
    /// flight -- which is the whole reason a user configures one.
    accounts: Vec<AccountProviderConfig>,
    profiles: HashMap<String, Profile>,
    order: Vec<String>,
    auth_states: HashMap<String, AuthState>,
    models: HashMap<String, Vec<Model>>,
    preferences: HashMap<String, Preference>,
}

pub struct Manager {
    cfg: LlmConfig,
    dispatcher: Option<Arc<Dispatcher>>,
    registry: Option<Arc<dyn ManagedRegistry>>,
    inner: RwLock<Inner>,
}

#[must_use]
pub fn new_manager(
    cfg: LlmConfig,
    dispatcher: Option<Arc<Dispatcher>>,
    registries: Vec<Arc<dyn ManagedRegistry>>,
) -> Manager {
    let registry = registries.into_iter().next();
    // Seeded from the configuration, then owned by `Inner`.
    //
    // The accounts a user configured before the daemon started arrive in the
    // config; the ones they add while it runs arrive through `upsert_account`.
    // Both have to live in the same list, because `load_profiles` rebuilds the
    // inventory from one place. Leaving this empty meant the configured ones
    // were dispatchable -- the clients are registered at boot -- and yet
    // invisible: `/v1/providers` answered with nothing while a request still
    // reached the vendor. A settings page showing no providers next to a
    // default provider it had never heard of is that bug, not two.
    let accounts = cfg.accounts.clone();
    let manager = Manager {
        cfg,
        dispatcher,
        registry,
        inner: RwLock::new(Inner { accounts, ..Inner::default() }),
    };
    manager.load_profiles();
    manager
}

impl Manager {
    // -- read accessors -------------------------------------------------------

    #[must_use]
    pub fn list_profiles(&self) -> Vec<Profile> {
        let inner = self.inner.read();
        inner.order.iter().map(|id| inner.profiles[id].clone()).collect()
    }

    #[must_use]
    pub fn get_profile(&self, provider_id: &str) -> Option<Profile> {
        self.inner.read().profiles.get(provider_id.trim()).cloned()
    }

    #[must_use]
    pub fn get_profile_for_tenant(&self, provider_id: &str, tenant_id: &str) -> Option<Profile> {
        let trimmed = provider_id.trim();
        if trimmed.is_empty() {
            return None;
        }
        if let Some(registry) = &self.registry {
            if let Some(bridge) = registry.get(trimmed) {
                return Some(self.build_managed_profile_for_tenant(&bridge, tenant_id));
            }
        }
        self.get_profile(trimmed)
    }

    #[must_use]
    pub fn get_auth_state(&self, provider_id: &str) -> Option<AuthState> {
        self.inner.read().auth_states.get(provider_id.trim()).cloned()
    }

    #[must_use]
    pub fn get_auth_state_for_tenant(&self, provider_id: &str, tenant_id: &str) -> Option<AuthState> {
        self.inner
            .read()
            .auth_states
            .get(&tenant_auth_key(tenant_id, provider_id))
            .cloned()
    }

    #[must_use]
    pub fn list_models(&self, provider_id: &str) -> Option<Vec<Model>> {
        let inner = self.inner.read();
        let trimmed = provider_id.trim();
        if let Some(items) = inner.models.get(trimmed) {
            return Some(items.clone());
        }
        let profile = inner.profiles.get(trimmed)?;
        if profile.known_models.is_empty() {
            return None;
        }
        Some(
            profile
                .known_models
                .iter()
                .map(|model_id| Model {
                    provider_id: profile.provider_id.clone(),
                    model_id: model_id.clone(),
                    display_name: model_id.clone(),
                    default: *model_id == profile.default_model || *model_id == profile.effective_model,
                    available: profile.ready,
                    source: profile.source.as_str().to_string(),
                    chat: profile.capabilities.chat,
                    stream: profile.capabilities.stream,
                    coding: profile.capabilities.coding,
                    tool_use: profile.capabilities.tool_use,
                    ..Model::default()
                })
                .collect(),
        )
    }

    #[must_use]
    pub fn get_preference(&self, provider_id: &str) -> Option<Preference> {
        self.inner.read().preferences.get(provider_id.trim()).cloned()
    }

    // -- restore --------------------------------------------------------------

    pub fn restore_managed_auth_states(&self, states: Vec<AuthState>) {
        {
            let mut inner = self.inner.write();
            for state in states {
                if state.provider_id.trim().is_empty() {
                    continue;
                }
                let key = tenant_auth_key(&state.tenant_id, &state.provider_id);
                inner.auth_states.insert(key, state);
            }
        }
        self.load_profiles();
    }

    pub fn restore_managed_auth_states_for_tenant(&self, tenant_id: &str, states: Vec<AuthState>) {
        {
            let mut inner = self.inner.write();
            for mut state in states {
                if state.provider_id.trim().is_empty() {
                    continue;
                }
                state.tenant_id = tenant_id.trim().to_string();
                let key = tenant_auth_key(&state.tenant_id, &state.provider_id);
                inner.auth_states.insert(key, state);
            }
        }
        self.load_profiles();
    }

    pub fn restore_provider_models(&self, items: Vec<Model>) {
        {
            let mut inner = self.inner.write();
            let mut grouped: HashMap<String, Vec<Model>> = HashMap::new();
            for item in items {
                let provider_id = item.provider_id.trim().to_string();
                if provider_id.is_empty() {
                    continue;
                }
                grouped.entry(provider_id).or_default().push(item);
            }
            for (provider_id, models) in grouped {
                inner.models.insert(provider_id, models);
            }
        }
        self.load_profiles();
    }

    pub fn restore_provider_preferences(&self, items: Vec<Preference>) {
        {
            let mut inner = self.inner.write();
            for item in items {
                if item.provider_id.trim().is_empty() {
                    continue;
                }
                inner.preferences.insert(item.provider_id.clone(), item);
            }
        }
        self.load_profiles();
    }

    // -- managed auth ---------------------------------------------------------

    pub async fn sync_managed_providers(&self) -> Result<Vec<SyncResult>, ProvidersError> {
        let Some(registry) = &self.registry else {
            return Ok(Vec::new());
        };
        let bridges = registry.list();
        let mut results = Vec::with_capacity(bridges.len());
        for bridge in bridges {
            let (state, models) = bridge.detect().await.map_err(|err| {
                ProvidersError::ProviderAuthUnavailable(format!(
                    "detect provider {}: {err}",
                    bridge.provider_id()
                ))
            })?;
            self.apply_managed_state(state.clone(), models.clone());
            results.push(SyncResult { state, models });
        }
        self.load_profiles();
        Ok(results)
    }

    pub async fn start_managed_auth(&self, provider_id: &str) -> Result<(AuthState, Vec<Model>), ProvidersError> {
        self.run_managed_action(provider_id, ManagedAction::Start).await
    }

    pub async fn complete_managed_auth(&self, provider_id: &str) -> Result<(AuthState, Vec<Model>), ProvidersError> {
        self.run_managed_action(provider_id, ManagedAction::Complete).await
    }

    pub async fn refresh_managed_auth(&self, provider_id: &str) -> Result<(AuthState, Vec<Model>), ProvidersError> {
        self.run_managed_action(provider_id, ManagedAction::Refresh).await
    }

    pub async fn revoke_managed_auth(&self, provider_id: &str) -> Result<(AuthState, Vec<Model>), ProvidersError> {
        self.run_managed_action(provider_id, ManagedAction::Revoke).await
    }

    async fn run_managed_action(
        &self,
        provider_id: &str,
        action: ManagedAction,
    ) -> Result<(AuthState, Vec<Model>), ProvidersError> {
        self.run_managed_action_for_tenant(provider_id, "", action).await
    }

    /// Go runManagedActionForTenant: run the bridge action and bind the
    /// resulting auth state to the tenant (empty = the local operator scope).
    async fn run_managed_action_for_tenant(
        &self,
        provider_id: &str,
        tenant_id: &str,
        action: ManagedAction,
    ) -> Result<(AuthState, Vec<Model>), ProvidersError> {
        let Some(bridge) = self.managed_bridge(provider_id) else {
            if self.get_profile(provider_id).is_some() {
                return Err(ProvidersError::ManagedAuthUnsupported);
            }
            return Err(ProvidersError::Prepare(PrepareError::ProviderNotFound(
                provider_id.trim().to_string(),
            )));
        };
        let (mut state, models) = match action {
            ManagedAction::Start => bridge.start().await?,
            ManagedAction::Complete => bridge.complete().await?,
            ManagedAction::Refresh => bridge.refresh().await?,
            ManagedAction::Revoke => bridge.revoke().await?,
        };
        state.tenant_id = tenant_id.trim().to_string();
        self.apply_managed_state(state.clone(), models.clone());
        self.load_profiles();
        Ok((state, models))
    }

    /// Go StartManagedAuthForTenant / Complete / Refresh / Revoke.
    pub async fn start_managed_auth_for_tenant(
        &self,
        provider_id: &str,
        tenant_id: &str,
    ) -> Result<(AuthState, Vec<Model>), ProvidersError> {
        self.run_managed_action_for_tenant(provider_id, tenant_id, ManagedAction::Start).await
    }

    pub async fn complete_managed_auth_for_tenant(
        &self,
        provider_id: &str,
        tenant_id: &str,
    ) -> Result<(AuthState, Vec<Model>), ProvidersError> {
        self.run_managed_action_for_tenant(provider_id, tenant_id, ManagedAction::Complete).await
    }

    pub async fn refresh_managed_auth_for_tenant(
        &self,
        provider_id: &str,
        tenant_id: &str,
    ) -> Result<(AuthState, Vec<Model>), ProvidersError> {
        self.run_managed_action_for_tenant(provider_id, tenant_id, ManagedAction::Refresh).await
    }

    pub async fn revoke_managed_auth_for_tenant(
        &self,
        provider_id: &str,
        tenant_id: &str,
    ) -> Result<(AuthState, Vec<Model>), ProvidersError> {
        self.run_managed_action_for_tenant(provider_id, tenant_id, ManagedAction::Revoke).await
    }

    fn managed_bridge(&self, provider_id: &str) -> Option<Arc<dyn ManagedBridge>> {
        self.registry.as_ref()?.get(provider_id.trim())
    }

    fn apply_managed_state(&self, state: AuthState, models: Vec<Model>) {
        let provider_id = state.provider_id.trim().to_string();
        if provider_id.is_empty() {
            return;
        }
        let mut inner = self.inner.write();
        let key = tenant_auth_key(&state.tenant_id, &provider_id);
        inner.auth_states.insert(key, state);
        inner.models.insert(provider_id, models);
    }

    // -- preferences ----------------------------------------------------------

    pub fn set_default_model(&self, provider_id: &str, model: &str) -> Result<Preference, ProvidersError> {
        let trimmed_provider_id = provider_id.trim();
        let trimmed_model = model.trim();
        if trimmed_provider_id.is_empty() {
            return Err(ProvidersError::Prepare(PrepareError::ProviderRequired));
        }
        if trimmed_model.is_empty() {
            return Err(ProvidersError::Prepare(PrepareError::ModelRequired));
        }
        let profile = self.get_profile(trimmed_provider_id).ok_or_else(|| {
            ProvidersError::Prepare(PrepareError::ProviderNotFound(trimmed_provider_id.to_string()))
        })?;
        validate_model(&profile, trimmed_model)?;
        let preference = Preference {
            provider_id: trimmed_provider_id.to_string(),
            default_model: trimmed_model.to_string(),
            updated_at: Utc::now(),
        };
        self.inner
            .write()
            .preferences
            .insert(trimmed_provider_id.to_string(), preference.clone());
        self.load_profiles();
        Ok(preference)
    }

    // -- resolution -----------------------------------------------------------

    pub fn resolve(&self, provider_id: &str, model: &str, timeout_ms: i64, max_retries: i64) -> Result<ResolvedDispatch, ProvidersError> {
        self.resolve_with_profile(provider_id, model, timeout_ms, max_retries, |pid| self.get_profile(pid), false)
    }

    pub fn resolve_for_tenant(
        &self,
        provider_id: &str,
        model: &str,
        timeout_ms: i64,
        max_retries: i64,
        tenant_id: &str,
    ) -> Result<ResolvedDispatch, ProvidersError> {
        self.resolve_with_profile(provider_id, model, timeout_ms, max_retries, |pid| {
            self.get_profile_for_tenant(pid, tenant_id)
        }, true)
    }

    fn resolve_with_profile<F>(
        &self,
        provider_id: &str,
        model: &str,
        timeout_ms: i64,
        max_retries: i64,
        profile_resolver: F,
        require_managed_auth_ready: bool,
    ) -> Result<ResolvedDispatch, ProvidersError>
    where
        F: Fn(&str) -> Option<Profile>,
    {
        let requested_provider = provider_id.trim().to_string();
        let effective_provider = if requested_provider.is_empty() {
            self.default_provider_id()
        } else {
            requested_provider.clone()
        };

        let profile = profile_resolver(&effective_provider).ok_or_else(|| {
            ProvidersError::Prepare(PrepareError::ProviderNotFound(effective_provider.clone()))
        })?;
        if !profile.registered {
            return Err(ProvidersError::Prepare(PrepareError::ProviderNotFound(effective_provider.clone())));
        }
        if require_managed_auth_ready && profile.source == Source::Managed && !profile.ready {
            return Err(ProvidersError::ProviderAuthUnavailable(effective_provider.clone()));
        }

        let mut effective_model = model.trim().to_string();
        if effective_model.is_empty() {
            let configured_default_model = self.cfg.default_model.trim().to_string();
            if !configured_default_model.is_empty() {
                let default_provider = self.cfg.default_provider.trim();
                if requested_provider.is_empty() || default_provider == profile.provider_id {
                    effective_model = configured_default_model;
                }
            }
        }
        if effective_model.is_empty() {
            if let Some(preference) = self.get_preference(&profile.provider_id) {
                effective_model = preference.default_model.trim().to_string();
            }
        }
        if effective_model.is_empty() {
            effective_model = profile.default_model.trim().to_string();
        }
        if effective_model.is_empty() {
            effective_model = profile.effective_model.trim().to_string();
        }
        if effective_model.is_empty() {
            return Err(ProvidersError::Prepare(PrepareError::ModelRequired));
        }
        validate_model(&profile, &effective_model)?;

        let effective_timeout_ms = if timeout_ms > 0 {
            timeout_ms
        } else {
            default_positive(profile.effective_timeout_ms, 30000)
        };
        let effective_max_retries = max_retry_value(max_retries, profile.effective_max_retries);

        Ok(ResolvedDispatch {
            provider_id: effective_provider,
            model: effective_model,
            timeout_ms: effective_timeout_ms,
            max_retries: effective_max_retries,
            endpoint: profile.request_url.clone(),
            profile,
        })
    }

    pub fn resolve_dispatch_input(&self, input: CreateDispatchInput) -> Result<(ResolvedDispatch, CreateDispatchInput), ProvidersError> {
        let resolved = self.resolve(&input.provider, &input.model, input.timeout_ms, input.max_retries)?;
        if input.messages.is_empty() {
            return Err(ProvidersError::Prepare(PrepareError::MessagesRequired));
        }
        let effective = CreateDispatchInput {
            provider: resolved.provider_id.clone(),
            model: resolved.model.clone(),
            messages: input.messages.clone(),
            timeout_ms: resolved.timeout_ms,
            max_retries: resolved.max_retries,
        };
        Ok((resolved, effective))
    }

    // -- setup gate -----------------------------------------------------------

    pub fn setup_dependent_use_decision(&self, session: &SetupSession, capability: &str) -> kura_setupwizard::DependentUseDecision {
        let service: Service = new_service(ServiceDependencies::default());
        service.dependent_use_decision(session, capability)
    }

    pub fn resolve_with_setup_gate(
        &self,
        provider_id: &str,
        model: &str,
        timeout_ms: i64,
        max_retries: i64,
        session: &SetupSession,
        capability: &str,
    ) -> Result<(ResolvedDispatch, kura_setupwizard::DependentUseDecision), ProvidersError> {
        let decision = self.setup_dependent_use_decision(session, capability);
        if decision.safe_use_mode == kura_setupwizard::SafeUseMode::Blocked {
            let effective_provider = if provider_id.trim().is_empty() {
                self.default_provider_id()
            } else {
                provider_id.trim().to_string()
            };
            return Err(ProvidersError::ProviderAuthUnavailable(effective_provider));
        }
        let resolved = self.resolve_for_tenant(provider_id, model, timeout_ms, max_retries, &session.tenant_id)?;
        Ok((resolved, decision))
    }

    // -- checks ---------------------------------------------------------------

    pub async fn run_check(&self, provider_id: &str, check_id: &str, input: CheckInput) -> Result<Check, ProvidersError> {
        let started_at = Utc::now();
        let resolved = self.resolve(provider_id, &input.model, 0, 0)?;

        let prompt = if input.prompt.trim().is_empty() {
            "Reply with the single word ok.".to_string()
        } else {
            input.prompt.trim().to_string()
        };

        let dispatch_input = CreateDispatchInput {
            provider: resolved.provider_id.clone(),
            model: resolved.model.clone(),
            messages: vec![Message { role: MessageRole::User, content: prompt }],
            timeout_ms: resolved.timeout_ms,
            max_retries: resolved.max_retries,
        };

        let Some(dispatcher) = &self.dispatcher else {
            return Err(ProvidersError::Prepare(PrepareError::ProviderNotFound(resolved.provider_id)));
        };
        let dispatch = dispatcher.prepare(dispatch_input, false)?;
        let cancel = CancelToken::new();
        let result = dispatcher.dispatch(dispatch, &cancel).await?;

        Ok(Check {
            check_id: check_id.to_string(),
            provider_id: resolved.profile.provider_id.clone(),
            family: resolved.profile.family,
            auth_mode: resolved.profile.auth_mode,
            status: CheckStatus::Passed,
            model: resolved.model,
            endpoint: resolved.endpoint,
            error_class: String::new(),
            error_code: String::new(),
            error_message: String::new(),
            usage: result.usage,
            created_at: started_at,
            completed_at: Utc::now(),
        })
    }

    // -- profile construction -------------------------------------------------

    fn load_profiles(&self) {
        // NOTE: build_managed_profile acquires its own read lock, so the
        // write lock must not be held while managed profiles are built (a
        // parking_lot RwLock is not reentrant and self-deadlocks when a
        // managed registry is present).
        // A provider appears when it can actually be used, and not before.
        //
        // Seeding the inventory with every provider the build knows about
        // meant a daemon nobody had configured still listed several, most of
        // them reporting faults -- so an untouched install read as broken
        // rather than as empty, and "not set up yet" was indistinguishable
        // from "set up and failing".
        let mut items = Vec::new();
        // Echo answers deterministically without reaching anything. It is a
        // test fixture, so it is present only when something asked for it by
        // name; offering it otherwise invites routing real work at a provider
        // that returns its own input.
        if self.cfg.default_provider.trim() == "echo" {
            items.push(self.build_echo_profile());
        }
        if !self.cfg.openai_compatible.base_url.trim().is_empty() {
            items.push(self.build_openai_compatible_profile(&self.inner.read()));
        }
        for account in &self.inner.read().accounts {
            if !account.id.trim().is_empty() && !account.base_url.trim().is_empty() {
                items.push(build_account_profile(account, &self.dispatcher));
            }
        }

        // Managed bridges are not listed as providers.
        //
        // A bridge borrows a vendor's coding agent and drives it with `-p`,
        // which means every request pays for that agent's own system prompt
        // and tool definitions before it reaches a model: measured against a
        // ten-token question, twenty-seven thousand tokens and eight seconds.
        // Loopforge is an agent itself, so wrapping another one is overhead
        // with nothing gained -- a subscription is reached by holding its
        // OAuth grant and calling the vendor's API directly.
        //
        // They remain available for auth inspection through the registry;
        // they are simply not somewhere a model request can be routed.
        let default_provider_id = default_provider_id_for_items(&self.cfg, &items);
        for item in &mut items {
            item.default = item.provider_id == default_provider_id;
        }
        items.sort_by(|a, b| a.provider_id.cmp(&b.provider_id));
        let mut inner = self.inner.write();
        inner.profiles.clear();
        inner.order.clear();
        for item in items {
            inner.order.push(item.provider_id.clone());
            inner.profiles.insert(item.provider_id.clone(), item);
        }
    }

    /// Register or replace a provider backed by a signed-in account.
    ///
    /// Returns once the inventory reflects it. The caller registers the client
    /// with the dispatcher; this is what makes it visible to everything that
    /// resolves a provider by id, which is what dispatch does.
    pub fn upsert_account(&self, account: AccountProviderConfig) {
        {
            let mut inner = self.inner.write();
            inner.accounts.retain(|existing| existing.id != account.id);
            inner.accounts.push(account);
        }
        self.load_profiles();
    }

    /// Forget a provider backed by an account. True when there was one.
    pub fn remove_account(&self, provider_id: &str) -> bool {
        let removed = {
            let mut inner = self.inner.write();
            let before = inner.accounts.len();
            inner.accounts.retain(|existing| existing.id != provider_id.trim());
            inner.accounts.len() != before
        };
        if removed {
            self.load_profiles();
        }
        removed
    }

    fn build_echo_profile(&self) -> Profile {
        let provider_id = "echo".to_string();
        let mut effective_model = "echo-v1".to_string();
        let mut issues = Vec::new();
        let configured_default_model = self.cfg.default_model.trim().to_string();
        let default_provider = self.cfg.default_provider.trim();
        if !configured_default_model.is_empty()
            && (default_provider == provider_id || default_provider.is_empty())
        {
            if configured_default_model.eq_ignore_ascii_case("echo-v1") {
                effective_model = "echo-v1".to_string();
            } else {
                effective_model = String::new();
                issues.push("configured default model is incompatible with provider echo".to_string());
            }
        }
        Profile {
            provider_id: provider_id.clone(),
            title: "Echo".to_string(),
            family: Family::BuiltinEcho,
            auth_mode: AuthMode::None,
            source: Source::Builtin,
            model_selection_mode: ModelSelectionMode::Fixed,
            known_models: vec!["echo-v1".to_string()],
            registered: has_provider(&self.dispatcher, &provider_id),
            configured: true,
            ready: !effective_model.is_empty(),
            default_model: "echo-v1".to_string(),
            effective_model,
            effective_timeout_ms: default_positive(self.cfg.default_timeout_ms, 30000),
            effective_max_retries: max_retry_value(0, self.cfg.default_max_retries),
            capabilities: CapabilityFlags { chat: true, stream: true, ..CapabilityFlags::default() },
            issues,
            ..Profile::default()
        }
    }

    fn build_openai_compatible_profile(&self, inner: &Inner) -> Profile {
        let provider_id = OPENAI_COMPATIBLE_PROVIDER_NAME.to_string();
        let base_url = self.cfg.openai_compatible.base_url.trim().to_string();
        let request_url = base_url.trim_end_matches('/').to_string();
        let mut issues = Vec::new();
        if base_url.is_empty() {
            issues.push("base URL is not configured".to_string());
        }
        let secret_configured = !self.cfg.openai_compatible.api_key.trim().is_empty();
        if !secret_configured {
            issues.push("API key is not configured".to_string());
        }
        let mut profile_default_model = self.cfg.openai_compatible.model.trim().to_string();
        if let Some(preference) = inner.preferences.get(&provider_id) {
            if !preference.default_model.trim().is_empty() {
                profile_default_model = preference.default_model.trim().to_string();
            }
        }
        let mut effective_model = profile_default_model.clone();
        let configured_default_model = self.cfg.default_model.trim().to_string();
        let default_provider = self.cfg.default_provider.trim();
        if effective_model.is_empty()
            && !configured_default_model.is_empty()
            && (default_provider == provider_id || default_provider.is_empty())
        {
            effective_model = configured_default_model;
        }
        if effective_model.is_empty() {
            issues.push("default model is not configured".to_string());
        }
        let timeout_ms = default_positive(
            self.cfg.openai_compatible.timeout_ms,
            default_positive(self.cfg.default_timeout_ms, 30000),
        );
        let max_retries = max_retry_value(0, self.cfg.default_max_retries);
        Profile {
            provider_id: provider_id.clone(),
            title: "OpenAI-Compatible".to_string(),
            family: Family::OpenAICompatible,
            auth_mode: AuthMode::ApiKey,
            source: Source::Config,
            model_selection_mode: ModelSelectionMode::Open,
            registered: has_provider(&self.dispatcher, &provider_id),
            configured: !base_url.is_empty() || secret_configured || !profile_default_model.is_empty() || default_provider == provider_id,
            ready: !request_url.is_empty() && secret_configured && !effective_model.is_empty() && has_provider(&self.dispatcher, &provider_id),
            base_url,
            request_url,
            default_model: profile_default_model,
            effective_model,
            effective_timeout_ms: timeout_ms,
            effective_max_retries: max_retries,
            secret_configured,
            secret_ref: self.cfg.openai_compatible.api_key_env.trim().to_string(),
            capabilities: CapabilityFlags { chat: true, stream: true, ..CapabilityFlags::default() },
            issues,
            ..Profile::default()
        }
    }

    fn build_managed_profile(&self, bridge: &Arc<dyn ManagedBridge>) -> Profile {
        self.build_managed_profile_for_tenant(bridge, "")
    }

    fn build_managed_profile_for_tenant(&self, bridge: &Arc<dyn ManagedBridge>, tenant_id: &str) -> Profile {
        let inner = self.inner.read();
        let key = tenant_auth_key(tenant_id, &bridge.provider_id());
        let (state, has_state) = match inner.auth_states.get(&key) {
            Some(state) => (state.clone(), true),
            None => (
                AuthState {
                    provider_id: bridge.provider_id(),
                    family: bridge.family(),
                    auth_mode: bridge.auth_mode(),
                    status: AuthStatus::Unknown,
                    ..AuthState::default()
                },
                false,
            ),
        };
        let models = inner.models.get(&bridge.provider_id()).cloned().unwrap_or_default();
        drop(inner);

        let mut issues = Vec::new();
        let known_models: Vec<String> = models.iter().map(|m| m.model_id.clone()).collect();
        if known_models.is_empty() {
            issues.push("model catalog is not available".to_string());
        }
        let default_model = if let Some(preference) = self.get_preference(&bridge.provider_id()) {
            if !preference.default_model.trim().is_empty() {
                preference.default_model.trim().to_string()
            } else {
                default_model_from_models(&models)
            }
        } else {
            default_model_from_models(&models)
        };
        let mut effective_model = default_model.clone();
        if self.cfg.default_provider.trim() == bridge.provider_id() && !self.cfg.default_model.trim().is_empty() {
            effective_model = self.cfg.default_model.trim().to_string();
        }
        if default_model.trim().is_empty() {
            issues.push("default model is not configured".to_string());
        }
        if !state.cli_available {
            issues.push("provider CLI is not available".to_string());
        }
        match state.status {
            AuthStatus::LoginRequired | AuthStatus::PendingLogin | AuthStatus::Revoked => {
                issues.push("provider login is required".to_string());
            }
            AuthStatus::Error => {
                if !state.last_error.trim().is_empty() {
                    issues.push(state.last_error.clone());
                } else {
                    issues.push("provider auth state is in error".to_string());
                }
            }
            _ => {}
        }

        Profile {
            provider_id: bridge.provider_id(),
            title: bridge.display_name(),
            family: bridge.family(),
            auth_mode: bridge.auth_mode(),
            source: Source::Managed,
            model_selection_mode: ModelSelectionMode::Fixed,
            known_models,
            registered: has_provider(&self.dispatcher, &bridge.provider_id()),
            configured: state.cli_available || has_state,
            ready: has_provider(&self.dispatcher, &bridge.provider_id())
                && state.status == AuthStatus::Authenticated
                && !effective_model.trim().is_empty(),
            default_model,
            effective_model,
            effective_timeout_ms: default_positive(self.cfg.default_timeout_ms, 30000),
            effective_max_retries: max_retry_value(0, self.cfg.default_max_retries),
            capabilities: capabilities_from_models(&models),
            auth_status: state.status.as_str().to_string(),
            cli_available: state.cli_available,
            account_label: state.account_label,
            account_id: state.account_id,
            plan: state.plan,
            auth_method: state.auth_method,
            login_command: state.login_command,
            logout_command: state.logout_command,
            available_model_count: available_model_count(&models),
            issues,
            ..Profile::default()
        }
    }

    fn default_provider_id(&self) -> String {
        default_provider_id_for_items(&self.cfg, &self.list_profiles())
    }
}

// ---------------------------------------------------------------------------
// Free helpers
// ---------------------------------------------------------------------------

#[must_use]
pub fn new_check_id() -> String {
    let now = Utc::now().format("%Y%m%d%H%M%S%.6f").to_string().replace('.', "");
    format!("provider_check_{now}")
}

#[must_use]
fn tenant_auth_key(tenant_id: &str, provider_id: &str) -> String {
    let provider_id = provider_id.trim();
    let tenant_id = tenant_id.trim();
    if tenant_id.is_empty() {
        return provider_id.to_string();
    }
    format!("{tenant_id}{}{provider_id}", char::from(0))
}

#[must_use]
fn validate_model(profile: &Profile, model: &str) -> Result<(), ProvidersError> {
    match profile.model_selection_mode {
        ModelSelectionMode::Fixed => {
            for known in &profile.known_models {
                if known.trim().eq_ignore_ascii_case(model.trim()) {
                    return Ok(());
                }
            }
            Err(ProvidersError::ModelNotSupported {
                model: model.to_string(),
                provider: profile.provider_id.clone(),
            })
        }
        ModelSelectionMode::Open => Ok(()),
    }
}

#[must_use]
fn has_provider(dispatcher: &Option<Arc<Dispatcher>>, provider_id: &str) -> bool {
    dispatcher.as_ref().map_or(false, |d| d.has_provider(provider_id))
}

#[must_use]
/// The inventory entry for an account-backed provider.
fn build_account_profile(
    account: &AccountProviderConfig,
    dispatcher: &Option<Arc<Dispatcher>>,
) -> Profile {
    let model = account.model.trim().to_string();
    let mut issues = Vec::new();
    if model.is_empty() {
        issues.push("default model is not configured".to_string());
    }
    if account.access_token.trim().is_empty() {
        issues.push("the account is not signed in".to_string());
    }
    Profile {
        provider_id: account.id.trim().to_string(),
        title: if account.title.trim().is_empty() {
            account.id.trim().to_string()
        } else {
            account.title.trim().to_string()
        },
        // The wire it actually speaks. Every account reported the OpenAI
        // family regardless, which is the label a surface puts next to the
        // provider's name -- so an Anthropic subscription was shown tagged
        // with a protocol it does not serve.
        family: match account.protocol {
            AccountProtocol::AnthropicMessages => Family::AnthropicMessages,
            AccountProtocol::OpenAiResponses => Family::OpenAIResponses,
            AccountProtocol::OpenAiCompatible => Family::OpenAICompatible,
        },
        auth_mode: AuthMode::ApiKey,
        source: Source::Config,
        model_selection_mode: ModelSelectionMode::Open,
        known_models: if model.is_empty() { Vec::new() } else { vec![model.clone()] },
        registered: has_provider(dispatcher, account.id.trim()),
        configured: true,
        ready: issues.is_empty(),
        default_model: model.clone(),
        effective_model: model,
        capabilities: CapabilityFlags { chat: true, stream: true, ..CapabilityFlags::default() },
        issues,
        ..Profile::default()
    }
}

fn default_provider_id_for_items(cfg: &LlmConfig, items: &[Profile]) -> String {
    let explicit = cfg.default_provider.trim();
    if !explicit.is_empty() {
        return explicit.to_string();
    }
    for item in items {
        if item.provider_id == OPENAI_COMPATIBLE_PROVIDER_NAME && item.ready {
            return item.provider_id.clone();
        }
    }
    for item in items {
        if item.source == Source::Managed && item.ready {
            return item.provider_id.clone();
        }
    }
    "echo".to_string()
}

#[must_use]
fn default_positive(value: i64, fallback: i64) -> i64 {
    if value > 0 { value } else { fallback }
}

#[must_use]
fn max_retry_value(value: i64, fallback: i64) -> i64 {
    let value = if value < 0 { 0 } else { value };
    if value > 0 {
        value
    } else if fallback < 0 {
        0
    } else {
        fallback
    }
}

#[must_use]
fn default_model_from_models(items: &[Model]) -> String {
    for item in items {
        if item.default && !item.model_id.trim().is_empty() {
            return item.model_id.clone();
        }
    }
    if let Some(item) = items.first() {
        return item.model_id.trim().to_string();
    }
    String::new()
}

#[must_use]
fn available_model_count(items: &[Model]) -> i64 {
    items.iter().filter(|m| m.available).count() as i64
}

#[must_use]
fn capabilities_from_models(items: &[Model]) -> CapabilityFlags {
    let mut flags = CapabilityFlags { chat: true, stream: true, ..CapabilityFlags::default() };
    for item in items {
        flags.chat |= item.chat;
        flags.stream |= item.stream;
        flags.coding |= item.coding;
        flags.tool_use |= item.tool_use;
    }
    flags
}
