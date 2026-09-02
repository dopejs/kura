//! Store-backed state and check management for managed providers.
//!
//! The Go package itself is a registry + bridges; the daemon's app layer
//! (API handlers in `daemon/internal/api/server.go`) persists bridge results
//! into the provider store and runs checks. This manager ports that wiring:
//! sync/action results are persisted with the store provider CRUD
//! (`upsert_provider_auth_state`, `replace_provider_models`,
//! `upsert_provider_preference`, `upsert_provider_check`), restored state is
//! exposed for feeding `kura_providers::Manager`, and the
//! `kura-setupwizard` dependent-use gate guards resolution.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use kura_llm::{Message, MessageRole, ProviderRequest};
use kura_providers::{AuthMode, AuthState, Check, CheckErrorClass, CheckStatus, Family, Model, Preference};
use kura_store::SQLiteStore;
use parking_lot::Mutex;

use crate::bridge::{Bridge, Registry, SandboxManager};
use crate::error::Error;

/// Go `providers.SyncResult`.
#[derive(Debug, Clone, Default)]
pub struct SyncResult {
    pub state: AuthState,
    pub models: Vec<Model>,
}

/// A manager that drives the bridge registry and persists provider state,
/// models, preferences, and checks through the store.
pub struct Manager {
    cfg: kura_config::Config,
    registry: Registry,
    store: Option<Mutex<SQLiteStore>>,
}

impl Manager {
    /// Builds a manager over the given registry. `store` is optional; when
    /// absent, persistence helpers are no-ops (matching Go's nil-store
    /// handling) and the in-memory registry remains fully usable.
    pub fn new(cfg: kura_config::Config, registry: Registry, store: Option<SQLiteStore>) -> Self {
        Manager {
            cfg,
            registry,
            store: store.map(Mutex::new),
        }
    }

    /// The underlying registry handle.
    #[must_use]
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// The optional store handle (for callers that need direct store access).
    #[must_use]
    pub fn store(&self) -> Option<&Mutex<SQLiteStore>> {
        self.store.as_ref()
    }

    // -- registry passthrough ------------------------------------------------

    /// Bridges in insertion order.
    #[must_use]
    pub fn list_bridges(&self) -> Vec<Arc<dyn Bridge>> {
        self.registry.list()
    }

    /// Look up one bridge.
    #[must_use]
    pub fn get_bridge(&self, provider_id: &str) -> Option<Arc<dyn Bridge>> {
        self.registry.get(provider_id)
    }

    /// Whether the provider id names a managed bridge.
    #[must_use]
    pub fn is_managed_provider(&self, provider_id: &str) -> bool {
        self.registry.is_managed_provider(provider_id)
    }

    // -- state management ----------------------------------------------------

    /// Detects every registered bridge and persists the resulting auth state
    /// and model catalog (Go app-layer `persistManagedProviderState`).
    pub fn sync_managed_providers(&self) -> Result<Vec<SyncResult>, Error> {
        let mut results = Vec::new();
        for bridge in self.registry.list() {
            let (state, models) = bridge.detect(&kura_llm::CancelToken::new())?;
            self.persist_managed_state(&state, &models)?;
            results.push(SyncResult { state, models });
        }
        Ok(results)
    }

    /// Starts managed auth for a provider and persists the result.
    pub fn start_managed_auth(&self, provider_id: &str) -> Result<(AuthState, Vec<Model>), Error> {
        let bridge = self.require_bridge(provider_id)?;
        let (state, models) = bridge.start(&kura_llm::CancelToken::new())?;
        self.persist_managed_state(&state, &models)?;
        Ok((state, models))
    }

    /// Completes managed auth for a provider and persists the result.
    pub fn complete_managed_auth(&self, provider_id: &str) -> Result<(AuthState, Vec<Model>), Error> {
        let bridge = self.require_bridge(provider_id)?;
        let (state, models) = bridge.complete(&kura_llm::CancelToken::new())?;
        self.persist_managed_state(&state, &models)?;
        Ok((state, models))
    }

    /// Refreshes managed auth for a provider and persists the result.
    pub fn refresh_managed_auth(&self, provider_id: &str) -> Result<(AuthState, Vec<Model>), Error> {
        let bridge = self.require_bridge(provider_id)?;
        let (state, models) = bridge.refresh(&kura_llm::CancelToken::new())?;
        self.persist_managed_state(&state, &models)?;
        Ok((state, models))
    }

    /// Revokes managed auth for a provider and persists the result.
    pub fn revoke_managed_auth(&self, provider_id: &str) -> Result<(AuthState, Vec<Model>), Error> {
        let bridge = self.require_bridge(provider_id)?;
        let (state, models) = bridge.revoke(&kura_llm::CancelToken::new())?;
        self.persist_managed_state(&state, &models)?;
        Ok((state, models))
    }

    /// Go `persistManagedProviderState`: upsert the auth state and replace the
    /// provider's models in the store. Tenant scoping is not yet ported; the
    /// tenantless write paths are used (the same paths the Go daemon uses when
    /// no tenant context is attached).
    pub fn persist_managed_state(&self, state: &AuthState, models: &[Model]) -> Result<(), Error> {
        let Some(store) = &self.store else { return Ok(()) };
        let store = store.lock();
        store
            .upsert_provider_auth_state(state)
            .map_err(Error::Store)?;
        store
            .replace_provider_models(&state.provider_id, models)
            .map_err(Error::Store)?;
        Ok(())
    }

    // -- restore (store -> caller) -------------------------------------------

    /// Lists persisted provider auth states (to seed `kura_providers::Manager`).
    pub fn restore_auth_states(&self) -> Result<Vec<AuthState>, Error> {
        let Some(store) = &self.store else { return Ok(Vec::new()) };
        store.lock().list_provider_auth_states().map_err(Error::Store)
    }

    /// Lists persisted provider models.
    pub fn restore_models(&self) -> Result<Vec<Model>, Error> {
        let Some(store) = &self.store else { return Ok(Vec::new()) };
        store.lock().list_provider_models().map_err(Error::Store)
    }

    /// Lists models for one provider.
    pub fn restore_models_by_provider(&self, provider_id: &str) -> Result<Vec<Model>, Error> {
        let Some(store) = &self.store else { return Ok(Vec::new()) };
        store
            .lock()
            .list_provider_models_by_provider(provider_id)
            .map_err(Error::Store)
    }

    /// Lists persisted provider preferences.
    pub fn restore_preferences(&self) -> Result<Vec<Preference>, Error> {
        let Some(store) = &self.store else { return Ok(Vec::new()) };
        store.lock().list_provider_preferences().map_err(Error::Store)
    }

    // -- preferences ---------------------------------------------------------

    /// Persists a provider preference (Go API `default-model` handler).
    pub fn upsert_preference(&self, preference: &Preference) -> Result<(), Error> {
        let Some(store) = &self.store else { return Ok(()) };
        store
            .lock()
            .upsert_provider_preference(preference)
            .map_err(Error::Store)
    }

    /// Validates a default-model choice against the bridge's known models and
    /// persists the resulting preference (Go `Manager.SetDefaultModel` for
    /// managed providers, which are fixed-selection).
    pub fn set_default_model(&self, provider_id: &str, model: &str) -> Result<Preference, Error> {
        let provider_id = provider_id.trim();
        let model = model.trim();
        if provider_id.is_empty() {
            return Err(Error::Other("provider is required".to_string()));
        }
        if model.is_empty() {
            return Err(Error::Other("model is required".to_string()));
        }
        let bridge = self.require_bridge(provider_id)?;
        let known = bridge.models(false);
        if !known.iter().any(|item| item.model_id.eq_ignore_ascii_case(model)) {
            return Err(Error::Other(format!(
                "model {model:?} is not supported by provider {provider_id}"
            )));
        }
        let preference = Preference {
            provider_id: provider_id.to_string(),
            default_model: model.to_string(),
            updated_at: Utc::now(),
        };
        self.upsert_preference(&preference)?;
        Ok(preference)
    }

    // -- check management ----------------------------------------------------

    /// Runs a provider check through the bridge's `kura_llm::Provider` and
    /// persists the resulting `Check` (Go API `checks` POST handler +
    /// `Manager.RunCheck`). Dispatch failures are folded into a failed
    /// `Check` (Go behavior: the check is persisted regardless, with the
    /// failure recorded on it).
    pub fn run_check(
        &self,
        provider_id: &str,
        check_id: &str,
        model: &str,
        prompt: &str,
    ) -> Result<Check, Error> {
        let started_at = Utc::now();
        let Some(bridge) = self.registry.get(provider_id) else {
            let check = failed_check(
                check_id,
                provider_id.trim(),
                Family::BuiltinEcho,
                AuthMode::None,
                model.trim(),
                "",
                CheckErrorClass::Config,
                "provider_check_failed",
                &format!("provider not found: {}", provider_id.trim()),
                started_at,
            );
            return self.persist_check(check);
        };

        let effective_model = if model.trim().is_empty() {
            bridge.default_model()
        } else {
            model.trim().to_string()
        };
        let prompt = if prompt.trim().is_empty() {
            "Reply with the single word ok.".to_string()
        } else {
            prompt.trim().to_string()
        };
        let request = ProviderRequest {
            provider: bridge.provider_id(),
            model: effective_model.clone(),
            messages: vec![Message { role: MessageRole::User, content: prompt, ..Default::default() }],
            ..ProviderRequest::default()
        };
        let response = futures::executor::block_on(bridge.provider().complete(request));
        let check = match response {
            Ok(response) => Check {
                check_id: check_id.to_string(),
                provider_id: bridge.provider_id(),
                family: bridge.family(),
                auth_mode: bridge.auth_mode(),
                status: CheckStatus::Passed,
                model: effective_model,
                usage: response.usage,
                created_at: started_at,
                completed_at: Utc::now(),
                ..Check::default()
            },
            Err(provider_err) => failed_check(
                check_id,
                &bridge.provider_id(),
                bridge.family(),
                bridge.auth_mode(),
                &effective_model,
                "",
                classify_dispatch_failure(provider_err.code()),
                provider_err.code(),
                &provider_err.to_string(),
                started_at,
            ),
        };
        self.persist_check(check)
    }

    /// Lists persisted checks for a provider.
    pub fn list_checks(&self, provider_id: &str) -> Result<Vec<Check>, Error> {
        let Some(store) = &self.store else { return Ok(Vec::new()) };
        store
            .lock()
            .list_provider_checks(provider_id)
            .map_err(Error::Store)
    }

    /// Gets one persisted check.
    pub fn get_check(&self, provider_id: &str, check_id: &str) -> Result<Option<Check>, Error> {
        let Some(store) = &self.store else { return Ok(None) };
        store
            .lock()
            .get_provider_check(provider_id, check_id)
            .map_err(Error::Store)
    }

    // -- setup wizard gate ---------------------------------------------------

    /// `kura-setupwizard` dependent-use decision for a session/capability
    /// (Go `Manager.setupDependentUseDecision`).
    #[must_use]
    pub fn setup_dependent_use_decision(
        &self,
        session: &kura_setupwizard::SetupSession,
        capability: &str,
    ) -> kura_setupwizard::DependentUseDecision {
        let service = kura_setupwizard::new_service(kura_setupwizard::ServiceDependencies::default());
        service.dependent_use_decision(session, capability)
    }

    /// Resolves a managed provider + model under the setup gate (Go
    /// `Manager.resolveWithSetupGate`): a blocked session is rejected, then
    /// the managed provider and model are validated.
    pub fn resolve_with_setup_gate(
        &self,
        provider_id: &str,
        model: &str,
        session: &kura_setupwizard::SetupSession,
        capability: &str,
    ) -> Result<kura_setupwizard::DependentUseDecision, Error> {
        let decision = self.setup_dependent_use_decision(session, capability);
        if decision.safe_use_mode == kura_setupwizard::SafeUseMode::Blocked {
            return Err(Error::Other("tenant provider auth is unavailable".to_string()));
        }
        let effective_provider = if provider_id.trim().is_empty() {
            self.default_provider_id()
        } else {
            provider_id.trim().to_string()
        };
        let bridge = self.require_bridge(&effective_provider)?;
        let model = model.trim();
        if !model.is_empty() {
            let known = bridge.models(false);
            if !known.iter().any(|item| item.model_id.eq_ignore_ascii_case(model)) {
                return Err(Error::Other(format!(
                    "model {model:?} is not supported by provider {effective_provider}"
                )));
            }
        }
        Ok(decision)
    }

    /// The default managed provider id: the configured default provider, else
    /// the first registered bridge, else "echo" (a scoped version of Go's
    /// `defaultProviderIDForItems`).
    #[must_use]
    pub fn default_provider_id(&self) -> String {
        let explicit = self.cfg.llm.default_provider.trim();
        if !explicit.is_empty() {
            return explicit.to_string();
        }
        if let Some(bridge) = self.registry.list().into_iter().next() {
            return bridge.provider_id();
        }
        "echo".to_string()
    }

    fn require_bridge(&self, provider_id: &str) -> Result<Arc<dyn Bridge>, Error> {
        self.registry
            .get(provider_id)
            .ok_or_else(|| {
                Error::Other(format!(
                    "managed auth is not supported by provider: {}",
                    provider_id.trim()
                ))
            })
    }

    fn persist_check(&self, check: Check) -> Result<Check, Error> {
        if let Some(store) = &self.store {
            store.lock().upsert_provider_check(&check).map_err(Error::Store)?;
        }
        Ok(check)
    }
}

/// The `kura-sandbox` manager attachment for the registry: a convenience
/// passthrough so callers can construct `Registry::new(cfg, Some(manager))`
/// with the concrete trait object.
#[allow(dead_code)]
pub type SandboxManagerRef = Arc<dyn SandboxManager>;

/// Go `failedCheck`.
#[must_use]
pub fn failed_check(
    check_id: &str,
    provider_id: &str,
    family: Family,
    auth_mode: AuthMode,
    model: &str,
    endpoint: &str,
    class: CheckErrorClass,
    error_code: &str,
    message: &str,
    created_at: DateTime<Utc>,
) -> Check {
    let error_code = if error_code.trim().is_empty() {
        "provider_check_failed".to_string()
    } else {
        error_code.trim().to_string()
    };
    Check {
        check_id: check_id.to_string(),
        provider_id: provider_id.to_string(),
        family,
        auth_mode,
        status: CheckStatus::Failed,
        model: model.to_string(),
        endpoint: endpoint.to_string(),
        error_class: class.as_str().to_string(),
        error_code,
        error_message: message.to_string(),
        created_at,
        completed_at: Utc::now(),
        ..Check::default()
    }
}

/// Go `classifyDispatchFailure`.
#[must_use]
pub fn classify_dispatch_failure(code: &str) -> CheckErrorClass {
    match code {
        "upstream_auth_failed" => CheckErrorClass::Auth,
        "upstream_transport_error" => CheckErrorClass::Transport,
        "timeout" | "connect_timeout" | "first_chunk_timeout" | "idle_timeout"
        | "max_duration_exceeded" => CheckErrorClass::Timeout,
        "upstream_invalid_request" => CheckErrorClass::Config,
        _ => CheckErrorClass::Upstream,
    }
}
