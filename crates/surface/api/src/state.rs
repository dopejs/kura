//! Application state shared by every route family.
//!
//! Port of Go's api.Dependencies struct (daemon/internal/api/server.go).
//! Every manager is an Option<Arc<T>> so the foundation compiles and serves
//! health/version/system-info even before app wiring populates the managers;
//! route families read their manager lazily and return 503/500 when the
//! manager they need is absent. store/config/event_bus are required
//! because every route family and the middleware layer need them.

use std::sync::Arc;

use parking_lot::Mutex;

use kura_activation::Service as ActivationService;
use kura_billing::Manager as BillingManager;
use kura_calendar::Manager as CalendarManager;
use kura_capabilities::Supervisor as CapabilitiesSupervisor;
use kura_catalog::Manager as CatalogManager;
use kura_chat::Service as ChatService;
use kura_checkpoints::Manager as CheckpointsManager;
use kura_computeruse::Manager as ComputerUseManager;
use kura_config::Config;
use kura_connectors::Supervisor as ConnectorsSupervisor;
use kura_delivery::Manager as DeliveryManager;
use kura_evaluation::Manager as EvaluationManager;
use kura_events::Bus;
use kura_evidence::Manager as EvidenceManager;
use kura_execprofile::Manager as ExecProfileManager;
use kura_identity::auth::Manager as AuthManager;
use kura_identity::{Manager as IdentityManager, Store as IdentityStore};
use kura_integrations::Manager as IntegrationsManager;
use kura_livevalidation::Manager as LiveValidationManager;
use kura_llm::Dispatcher;
use kura_mail::Manager as MailManager;
use kura_mcp::Manager as McpManager;
use kura_memory::Manager as MemoryManager;
use kura_policy::Engine;
use kura_providers::Manager as ProvidersManager;
use kura_reminders::Manager as RemindersManager;
use kura_router::SessionRouter;
use kura_routine::Manager as RoutineManager;
use kura_runtime::Manager as RuntimeManager;
use kura_sandbox::Manager as SandboxManager;
use kura_scheduler::Scheduler;
use kura_secrets::Manager as SecretsManager;
use kura_setupwizard::Service as SetupWizardService;
use kura_skills::Registry;
use kura_store::SQLiteStore;
use kura_triage::Manager as TriageManager;
use kura_webhook::Manager as WebhookManager;

/// Read-only view of the tenant-backfill migration gate the API needs.
/// Port of Go's api.MigrationStatus interface; the app layer supplies an
/// implementation wrapping the migration runner.
pub trait MigrationStatus: Send + Sync {
    fn in_progress(&self) -> bool;
    fn pending_steps(&self) -> Vec<String>;
}

/// Shared application state, mirroring Go's api.Dependencies field set.
///
/// logger is intentionally skipped (the Go field is *slog.Logger; the Rust
/// surface has no equivalent yet — telemetry is out of scope for this wave).
/// reminders is a placeholder because the kura-reminders crate has not been
/// ported yet (see MISSING-MANAGERS note below).
#[derive(Clone)]
pub struct AppState {
    /// Go Dependencies.Config.
    pub config: Config,
    /// Go Dependencies.EventBus.
    pub event_bus: Arc<Bus>,
    /// Go Dependencies.Policy.
    pub policy: Option<Arc<Engine>>,
    /// Go Dependencies.Auth (pairing + access-token lifecycle).
    pub auth: Option<Arc<AuthManager>>,
    /// Go Dependencies.Identity (tenant/principal resolution). The store is
    /// erased behind the object-safe Store trait so the manager can be shared.
    pub identity: Option<Arc<IdentityManager<dyn IdentityStore + Send + Sync>>>,
    /// Go Dependencies.Router.
    pub router: Option<Arc<SessionRouter>>,
    /// Go Dependencies.Runtime.
    pub runtime: Option<Arc<RuntimeManager>>,
    /// Go Dependencies.LLM.
    pub llm: Option<Arc<Dispatcher>>,
    /// The OpenAI-compatible provider's credential, swappable at runtime.
    ///
    /// Present only when that provider is configured. It exists because an
    /// OAuth access token expires roughly hourly while the daemon runs for
    /// far longer: without a way to replace the credential in place, a
    /// refreshed token would need a restart, and a restart drops whatever run
    /// is in flight.
    pub openai_credential: Option<kura_model_provider::Credential>,
    /// Credentials for providers backed by a signed-in subscription, by id.
    ///
    /// One per account, because each holds its own grant and they expire
    /// independently. Whatever refreshes a token hands the new value here
    /// rather than restarting the daemon.
    pub account_credentials: std::collections::BTreeMap<String, kura_model_provider::Credential>,
    /// Go Dependencies.Chat.
    ///
    pub chat: Option<Arc<ChatService>>,
    /// Go Dependencies.Providers.
    pub providers: Option<Arc<ProvidersManager>>,
    /// Go Dependencies.Skills.
    pub skills: Option<Arc<Registry>>,
    /// Go Dependencies.Sandboxes.
    pub sandboxes: Option<Arc<SandboxManager>>,
    /// Go Dependencies.Secrets.
    pub secrets: Option<Arc<SecretsManager>>,
    /// Go Dependencies.MCP.
    pub mcp: Option<Arc<McpManager>>,
    /// Go Dependencies.Integrations.
    pub integrations: Option<Arc<IntegrationsManager>>,
    /// Go Dependencies.Calendar.
    pub calendar: Option<Arc<CalendarManager>>,
    /// Go Dependencies.Mail.
    pub mail: Option<Arc<MailManager>>,
    /// Go Dependencies.Reminders.
    ///
    pub reminders: Option<Arc<RemindersManager>>,
    /// Go Dependencies.Triage.
    ///
    pub triage: Option<Arc<TriageManager>>,
    /// Memory plane manager (Roadmap 78, spec 058).
    pub memory: Option<Arc<MemoryManager>>,
    /// Go Dependencies.Routines.
    ///
    pub routines: Option<Arc<RoutineManager>>,
    /// Go Dependencies.Webhooks.
    ///
    pub webhooks: Option<Arc<WebhookManager>>,
    /// Go Dependencies.Catalog.
    ///
    pub catalog: Option<Arc<CatalogManager>>,
    /// Go Dependencies.ExecProfiles.
    ///
    pub exec_profiles: Option<Arc<ExecProfileManager>>,
    /// Go Dependencies.Evidence.
    ///
    pub evidence: Option<Arc<EvidenceManager>>,
    /// Go Dependencies.Connectors.
    pub connectors: Option<Arc<ConnectorsSupervisor>>,
    /// Go Dependencies.Capabilities.
    pub capabilities: Option<Arc<CapabilitiesSupervisor>>,
    /// Go Dependencies.ComputerUse.
    pub computer_use: Option<Arc<ComputerUseManager>>,
    /// Go Dependencies.Scheduler.
    ///
    pub scheduler: Option<Arc<Scheduler>>,
    /// Go Dependencies.Delivery.
    pub delivery: Option<Arc<DeliveryManager>>,
    /// Go Dependencies.Billing.
    pub billing: Option<Arc<BillingManager>>,
    /// Go Dependencies.Activation.
    pub activation: Option<Arc<ActivationService>>,
    /// Go Dependencies.SetupWizard.
    pub setup_wizard: Option<Arc<SetupWizardService>>,
    /// Go Dependencies.Store.
    ///
    /// Wrapped in a mutex because rusqlite `Connection` is `!Sync`; the Go
    /// daemon shares the store across goroutines behind its own lock, so the
    /// mutex is the Rust equivalent.
    pub store: Arc<Mutex<SQLiteStore>>,
    /// Go Dependencies.Checkpoints.
    ///
    pub checkpoints: Option<Arc<CheckpointsManager>>,
    /// Go Dependencies.Evaluation.
    pub evaluation: Option<Arc<EvaluationManager>>,
    /// Go Dependencies.LiveValidation.
    pub live_validation: Option<Arc<LiveValidationManager>>,
    /// Go Dependencies.AuditEmitter (emits audit.cross_tenant_access_denied).
    pub audit_emitter: Option<Arc<kura_audit::Emitter>>,
    /// Go Dependencies.TenantMigrationStatus. None behaves as if all
    /// backfills are complete.
    pub tenant_migration_status: Option<Arc<dyn MigrationStatus>>,
    /// Plugin assembly report (which plugins resolved enabled/disabled and
    /// why). None only in test states built outside the app assembly.
    pub plugins: Option<Arc<kura_plugin::AssemblyReport>>,
    /// The plugin hook bus (waterfall interception points, pluginization
    /// phase 2). None only in test states built outside the app assembly.
    pub hooks: Option<Arc<kura_plugin::HookBus>>,
    /// The embedding seam provider (an external plugin serving
    /// `context.embedder`, when installed). None = the deterministic
    /// in-process default.
    pub embedder: Option<Arc<dyn kura_context::Embedder>>,
    /// Audited self-improvement proposals (the `self-improve` plugin).
    pub improvement: Option<Arc<kura_improvement::Manager>>,
}

impl AppState {
    /// Builds a state with only the required core (config, event_bus,
    /// store) populated; every manager is None.
    #[must_use]
    pub fn new(config: Config, event_bus: Arc<Bus>, store: Arc<Mutex<SQLiteStore>>) -> Self {
        Self {
            config,
            event_bus,
            policy: None,
            auth: None,
            identity: None,
            router: None,
            runtime: None,
            llm: None,
            openai_credential: None,
            account_credentials: std::collections::BTreeMap::new(),
            chat: None,
            providers: None,
            skills: None,
            sandboxes: None,
            secrets: None,
            mcp: None,
            integrations: None,
            calendar: None,
            mail: None,
            reminders: None,
            triage: None,
            memory: None,
            routines: None,
            webhooks: None,
            catalog: None,
            exec_profiles: None,
            evidence: None,
            connectors: None,
            capabilities: None,
            computer_use: None,
            scheduler: None,
            delivery: None,
            billing: None,
            activation: None,
            setup_wizard: None,
            store,
            checkpoints: None,
            evaluation: None,
            live_validation: None,
            audit_emitter: None,
            tenant_migration_status: None,
            plugins: None,
            hooks: None,
            embedder: None,
            improvement: None,
        }
    }
}

// NOTE: no Default impl. Config has no Default (environment is required) and
// SQLiteStore::new needs a real data dir, so a Default would have to hide an
// unwrap. Tests build state explicitly through AppState::new with a temp-dir
// store (see routes.rs tests).
