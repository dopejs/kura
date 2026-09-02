//! Builtin plugin definitions: every non-kernel subsystem of the daemon,
//! expressed as a [`BuiltinPlugin`] with a declared dependency edge set and a
//! build function over the shared [`Assembly`].
//!
//! The kernel (store, event bus, session router, runtime, checkpoints,
//! policy, auth, identity, secrets, audit — the trust boundary) is built in
//! `App::with_profile` before any plugin runs; it is deliberately *not* a
//! plugin and cannot be disabled. Everything else assembles here in declared
//! order. Disabling a plugin leaves its `AppState` field `None`, which the
//! API layer already answers with not-wired errors; dependents are
//! transitively disabled by `kura_plugin::resolve`.
//!
//! The channel plugins (`channel-*`) build nothing at assembly time — their
//! runtimes are constructed in `App::serve` — but their enablement gates the
//! runtime construction and their `requires` edges keep them honest about
//! the managers the message loop dereferences (runtime, chat).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use kura_activation::{
    BillingProjectorAdapter as ActivationBillingProjectorAdapter,
    ChatRunnerAdapter as ActivationChatRunnerAdapter, Service as ActivationService,
    SqliteActivationStore,
};
use kura_api::AppState;
use kura_billing::Manager as BillingManager;
use kura_calendar::Manager as CalendarManager;
use kura_capabilities::Supervisor as CapabilitiesSupervisor;
use kura_chat::{ChatStore, Service as ChatService};
use kura_computeruse::{
    Dependencies as ComputerUseDependencies, Manager as ComputerUseManager, SqliteArtifactRecorder,
};
use kura_config::Config;
use kura_connectors::Supervisor as ConnectorsSupervisor;
use kura_delivery::{ConnectorAdapter, Manager as DeliveryManager, TestSinkAdapter};
use kura_evaluation::{Dependencies as EvaluationDependencies, Manager as EvaluationManager};
use kura_events::Bus;
use kura_evidence::{Manager as EvidenceManager, RoutineCollector};
use kura_execprofile::{Manager as ExecProfileManager, SandboxHealthChecker};
use kura_integrations::Manager as IntegrationsManager;
use kura_livevalidation::{
    Dependencies as LiveValidationDependencies, Manager as LiveValidationManager,
};
use kura_llm::Dispatcher;
use kura_mail::Manager as MailManager;
use kura_mcp::Manager as McpManager;
use kura_memory::Manager as MemoryManager;
use kura_plugin::{PluginDescriptor, SeamMap};
use kura_policy::Engine as PolicyEngine;
use kura_reminders::{Dependencies as RemindersDependencies, Manager as RemindersManager};
use kura_sandbox::Manager as SandboxManager;
use kura_scheduler::{Dependencies as SchedulerDependencies, Scheduler};
use kura_secrets::Manager as SecretsManager;
use kura_setupwizard::{
    ServiceDependencies as SetupWizardDependencies, new_service as new_setup_wizard,
};
use kura_skills::Registry as SkillsRegistry;
use kura_store::{
    BillingRepositoryHandle, ComputerUseStoreHandle, EvaluationStoreHandle,
    LiveValidationStoreHandle, SQLiteStore, SecretStoreHandle, SetupWizardStoreHandle,
};
use kura_triage::Manager as TriageManager;
use kura_webhook::Manager as WebhookManager;

use crate::adapters;
use crate::AppError;

// ---------------------------------------------------------------------------
// Seams shared between the kernel and plugins during assembly
// ---------------------------------------------------------------------------

/// Managed-provider registry, provided by `llm`, consumed by `providers`.
#[derive(Clone)]
pub(crate) struct ManagedRegistrySeam(pub Arc<dyn kura_providers::ManagedRegistry>);

/// Secret metadata store handle (kernel-provided; `sandbox` builds its own
/// secret-manager instance over it because `set_secret_manager` takes
/// ownership).
#[derive(Clone)]
pub(crate) struct SecretStoreSeam(pub Arc<SecretStoreHandle>);

/// Secret value backend (kernel-provided, same consumer as the store seam).
#[derive(Clone)]
pub(crate) struct SecretBackendSeam(pub Arc<kura_secrets::LocalBackend>);

/// Workflow launcher over the runtime manager (kernel-provided; consumed by
/// `scheduler` and `webhooks`).
#[derive(Clone)]
pub(crate) struct WorkflowLauncherSeam(pub Arc<adapters::WorkflowLauncherImpl>);

// ---------------------------------------------------------------------------
// Assembly context
// ---------------------------------------------------------------------------

/// One lifecycle callback registered by a plugin during its build.
pub(crate) type LifecycleFn = Box<dyn Fn(&AppState) + Send + Sync>;

/// Plugin-owned lifecycle registrations. `starts` run in registration order
/// when the daemon begins serving; `closes` run in registration order during
/// shutdown (each close is best-effort and idempotent by convention).
#[derive(Default)]
pub(crate) struct Lifecycle {
    pub starts: Vec<(String, LifecycleFn)>,
    pub closes: Vec<(String, LifecycleFn)>,
}

impl Lifecycle {
    pub fn on_start(&mut self, plugin_id: &str, f: LifecycleFn) {
        self.starts.push((plugin_id.to_string(), f));
    }

    pub fn on_close(&mut self, plugin_id: &str, f: LifecycleFn) {
        self.closes.push((plugin_id.to_string(), f));
    }
}

/// Mutable assembly context threaded through the plugin build functions.
/// Kernel-built handles live as named fields; cross-plugin intermediates go
/// through [`SeamMap`]; managers land on `state` (the same `Option` fields
/// the API reads).
pub(crate) struct Assembly {
    pub cfg: Config,
    /// The resolved plugin profile: build functions read their entry's
    /// `config` object through it.
    pub profile: kura_plugin::PluginProfile,
    pub env_scope: &'static str,
    pub hosted: bool,
    pub store: Arc<parking_lot::Mutex<SQLiteStore>>,
    /// std::sync::Mutex handle required by the sandbox/mcp/chat constructors.
    pub secondary: Arc<std::sync::Mutex<SQLiteStore>>,
    pub event_bus: Arc<Bus>,
    pub state: AppState,
    pub seams: SeamMap,
    /// Plugin-owned start/close callbacks (run by App serve/close).
    pub lifecycle: Lifecycle,
    /// The fully assembled AppState, set by the kernel after restore. Hooks
    /// built during assembly capture this instead of a stale partial clone.
    pub late_state: Arc<std::sync::OnceLock<AppState>>,
    #[cfg(test)]
    pub wiring: crate::AppWiring,
}

impl Assembly {
    /// Opens an additional connection to the shared WAL database (the
    /// pattern the store-backed handles use).
    fn open_store(&self) -> Result<SQLiteStore, AppError> {
        SQLiteStore::new(&self.cfg.data_dir).map_err(AppError::Store)
    }
}

/// One builtin plugin: static descriptor + build function.
pub(crate) struct BuiltinPlugin {
    pub descriptor: PluginDescriptor,
    pub build: fn(&mut Assembly) -> Result<(), AppError>,
}

/// The builtin plugin set in build order (dependencies before dependents,
/// matching the pre-pluginization `App::new` construction order).
pub(crate) const BUILTINS: &[BuiltinPlugin] = &[
    BuiltinPlugin {
        descriptor: PluginDescriptor {
            id: "llm",
            summary: "LLM dispatcher with managed CLI providers and the echo fallback",
            provides: &["llm.dispatcher", "llm.managed-registry"],
            requires: &[],
        },
        build: build_llm,
    },
    BuiltinPlugin {
        descriptor: PluginDescriptor {
            id: "skills",
            summary: "Skill registry over <data_dir>/skills",
            provides: &["skills.registry"],
            requires: &[],
        },
        build: build_skills,
    },
    BuiltinPlugin {
        descriptor: PluginDescriptor {
            id: "sandbox",
            summary: "Sandboxed execution plane",
            provides: &["sandbox.manager"],
            requires: &[],
        },
        build: build_sandbox,
    },
    BuiltinPlugin {
        descriptor: PluginDescriptor {
            id: "mcp",
            summary: "MCP server registry and attached executions",
            provides: &["mcp.manager"],
            requires: &["sandbox"],
        },
        build: build_mcp,
    },
    BuiltinPlugin {
        descriptor: PluginDescriptor {
            id: "integrations",
            summary: "Integration account registry (adapter RPC plane)",
            provides: &["integrations.manager"],
            requires: &[],
        },
        build: build_integrations,
    },
    BuiltinPlugin {
        descriptor: PluginDescriptor {
            id: "calendar",
            summary: "Calendar accounts, events and scheduling intents",
            provides: &["calendar.manager"],
            requires: &[],
        },
        build: build_calendar,
    },
    BuiltinPlugin {
        descriptor: PluginDescriptor {
            id: "mail",
            summary: "Mail accounts, messages and drafts",
            provides: &["mail.manager"],
            requires: &[],
        },
        build: build_mail,
    },
    BuiltinPlugin {
        descriptor: PluginDescriptor {
            id: "providers",
            summary: "Provider registry (managed auth, models, checks)",
            provides: &["providers.manager"],
            requires: &["llm"],
        },
        build: build_providers,
    },
    BuiltinPlugin {
        descriptor: PluginDescriptor {
            id: "connectors",
            summary: "Channel connector supervisor",
            provides: &["connectors.supervisor"],
            requires: &[],
        },
        build: build_connectors,
    },
    BuiltinPlugin {
        descriptor: PluginDescriptor {
            id: "capabilities",
            summary: "Supervised capability process registry",
            provides: &["capabilities.supervisor"],
            requires: &[],
        },
        build: build_capabilities,
    },
    BuiltinPlugin {
        // Declared before chat so the context/session hooks that consume it
        // can require it (build order among independent managers is free).
        descriptor: PluginDescriptor {
            id: "memory",
            summary: "Layered memory plane (L0-L3) with LLM consolidation",
            provides: &["memory.manager"],
            requires: &["llm"],
        },
        build: build_memory,
    },
    BuiltinPlugin {
        descriptor: PluginDescriptor {
            id: "chat",
            summary: "Chat query service over the LLM dispatcher",
            provides: &["chat.service"],
            // `mcp` because the tools the model may call come from there, and
            // a registry read before that plugin built would be empty.
            requires: &["llm", "mcp"],
        },
        build: build_chat,
    },
    BuiltinPlugin {
        // Registered at chat/pre-dispatch BEFORE session-strategy: context
        // injects the memory bootstrap, then session-strategy shapes the
        // window (bootstrap messages are system-frame and survive elision).
        descriptor: PluginDescriptor {
            id: "context",
            summary: "Default context assembly: memory bootstrap injection under budget",
            provides: &["context.assembler"],
            requires: &["chat", "memory"],
        },
        build: build_context,
    },
    BuiltinPlugin {
        descriptor: PluginDescriptor {
            id: "session-strategy",
            summary: "Session window policy: personal long-session and IM thread budgets",
            provides: &["session.window-policy"],
            requires: &["chat"],
        },
        build: build_session_strategy,
    },
    BuiltinPlugin {
        descriptor: PluginDescriptor {
            id: "billing",
            summary: "Billing plans, usage ledgers and quota reservations",
            provides: &["billing.manager"],
            requires: &[],
        },
        build: build_billing,
    },
    BuiltinPlugin {
        descriptor: PluginDescriptor {
            id: "activation",
            summary: "Activation/onboarding state machine",
            provides: &["activation.service"],
            requires: &["billing"],
        },
        build: build_activation,
    },
    BuiltinPlugin {
        descriptor: PluginDescriptor {
            id: "computer-use",
            summary: "Computer-use sessions with artifact recording",
            provides: &["computeruse.manager"],
            requires: &[],
        },
        build: build_computer_use,
    },
    BuiltinPlugin {
        descriptor: PluginDescriptor {
            id: "delivery",
            summary: "Outbound delivery with connector and test sinks",
            provides: &["delivery.manager"],
            requires: &[],
        },
        build: build_delivery,
    },
    BuiltinPlugin {
        descriptor: PluginDescriptor {
            id: "scheduler",
            summary: "Scheduled workflow launches",
            provides: &["scheduler"],
            requires: &[],
        },
        build: build_scheduler,
    },
    BuiltinPlugin {
        descriptor: PluginDescriptor {
            id: "reminders",
            summary: "Reminders with delivery escalation",
            provides: &["reminders.manager"],
            requires: &[],
        },
        build: build_reminders,
    },
    BuiltinPlugin {
        descriptor: PluginDescriptor {
            id: "routines",
            summary: "Routines compiled onto the scheduler",
            provides: &["routines.manager"],
            requires: &["scheduler"],
        },
        build: build_routines,
    },
    BuiltinPlugin {
        descriptor: PluginDescriptor {
            id: "triage",
            summary: "Inbound triage queue",
            provides: &["triage.manager"],
            requires: &[],
        },
        build: build_triage,
    },
    BuiltinPlugin {
        descriptor: PluginDescriptor {
            id: "webhooks",
            summary: "Webhook ingress with quota-gated workflow launches",
            provides: &["webhooks.manager"],
            // The quota gate is billing-backed and fail-closed (Roadmap 75);
            // running webhooks without billing would drop that enforcement.
            requires: &["billing"],
        },
        build: build_webhooks,
    },
    BuiltinPlugin {
        descriptor: PluginDescriptor {
            id: "catalog",
            summary: "Install catalog (skills, MCP servers, capabilities)",
            provides: &["catalog.manager"],
            requires: &["sandbox"],
        },
        build: build_catalog,
    },
    BuiltinPlugin {
        descriptor: PluginDescriptor {
            id: "exec-profiles",
            summary: "Execution profiles with sandbox-backed health checks",
            provides: &["execprofile.manager"],
            requires: &["sandbox"],
        },
        build: build_exec_profiles,
    },
    BuiltinPlugin {
        descriptor: PluginDescriptor {
            id: "evidence",
            summary: "Support evidence bundles",
            provides: &["evidence.manager"],
            requires: &["routines"],
        },
        build: build_evidence,
    },
    BuiltinPlugin {
        descriptor: PluginDescriptor {
            id: "evaluation",
            summary: "Evaluation harness with billing enforcement",
            provides: &["evaluation.manager"],
            requires: &["billing"],
        },
        build: build_evaluation,
    },
    BuiltinPlugin {
        descriptor: PluginDescriptor {
            id: "live-validation",
            summary: "Live validation ledger with billing enforcement",
            provides: &["livevalidation.manager"],
            requires: &["billing"],
        },
        build: build_live_validation,
    },
    BuiltinPlugin {
        descriptor: PluginDescriptor {
            id: "setup-wizard",
            summary: "First-run setup wizard",
            provides: &["setupwizard.service"],
            requires: &[],
        },
        build: build_setup_wizard,
    },
    BuiltinPlugin {
        descriptor: PluginDescriptor {
            id: "self-improve",
            summary: "Audited self-improvement proposals over the plugin profile",
            provides: &["improvement.manager"],
            requires: &[],
        },
        build: build_self_improve,
    },
    BuiltinPlugin {
        descriptor: PluginDescriptor {
            id: "channel-discord",
            summary: "Discord channel runtime (built at serve time)",
            provides: &["channel.discord"],
            requires: &["connectors", "chat"],
        },
        build: build_channel_noop,
    },
    BuiltinPlugin {
        descriptor: PluginDescriptor {
            id: "channel-telegram",
            summary: "Telegram channel runtime (built at serve time)",
            provides: &["channel.telegram"],
            requires: &["connectors", "chat"],
        },
        build: build_channel_noop,
    },
    BuiltinPlugin {
        descriptor: PluginDescriptor {
            id: "channel-slack",
            summary: "Slack channel runtime (built at serve time)",
            provides: &["channel.slack"],
            requires: &["connectors", "chat"],
        },
        build: build_channel_noop,
    },
    BuiltinPlugin {
        descriptor: PluginDescriptor {
            id: "channel-matrix",
            summary: "Matrix channel runtime (built at serve time)",
            provides: &["channel.matrix"],
            requires: &["connectors", "chat"],
        },
        build: build_channel_noop,
    },
];

/// The descriptor list in build order, for `kura_plugin::resolve`.
pub(crate) fn descriptors() -> Vec<PluginDescriptor> {
    BUILTINS.iter().map(|plugin| plugin.descriptor).collect()
}

// ---------------------------------------------------------------------------
// Build functions (each a verbatim port of its pre-pluginization App::new
// block; comments carried over where they record decisions)
// ---------------------------------------------------------------------------

fn build_llm(asm: &mut Assembly) -> Result<(), AppError> {
    let llm = Arc::new(Dispatcher::new());
    if asm.cfg.llm.default_timeout_ms > 0 {
        llm.set_default_timeout(Duration::from_millis(asm.cfg.llm.default_timeout_ms as u64));
    }
    if asm.cfg.llm.default_max_retries > 0 {
        llm.set_default_retries(asm.cfg.llm.default_max_retries);
    }
    if !asm.cfg.llm.default_model.trim().is_empty() {
        llm.set_default_model(&asm.cfg.llm.default_model);
    }
    let managed_registry: Arc<dyn kura_providers::ManagedRegistry> =
        Arc::new(kura_managedproviders::Registry::new(&asm.cfg, None));
    for bridge in managed_registry.list() {
        llm.register_provider(bridge.provider());
    }
    // The OpenAI-compatible endpoint is registered only when it has somewhere
    // to send requests. Registering it unconfigured would put a provider in the
    // dispatcher that fails every dispatch, which reads as a broken daemon
    // rather than an unconfigured one.
    let openai = &asm.cfg.llm.openai_compatible;
    if !openai.base_url.trim().is_empty() {
        let api_key = openai.api_key.trim();
        let client = kura_model_provider::OpenAiCompatibleClient::new(
            openai.base_url.trim(),
            openai.model.trim(),
            (!api_key.is_empty()).then(|| api_key.to_string()),
        )
        .with_headers(openai.headers.clone())
        .with_sampling(kura_model_provider::Sampling {
            temperature: openai.sampling.temperature,
            top_p: openai.sampling.top_p,
        });
        // Kept so the credential can be replaced while the daemon runs: an
        // OAuth token expires in about an hour, and a restart to pick up a
        // refreshed one would drop whatever run is in flight.
        asm.state.openai_credential = Some(client.credential());
        llm.register_provider(Arc::new(
            kura_model_provider::OpenAiCompatibleProvider::new("openai_compatible", client),
        ));
    }

    // Providers backed by a subscription the user signed into.
    //
    // One per account rather than a slot per vendor, and the protocol is named
    // rather than guessed from the URL: most of these speak the
    // OpenAI-compatible shape and need no wire of their own, while Anthropic
    // and Codex each need theirs. Every one keeps a credential handle, because
    // an access token lasts about an hour and the daemon far longer.
    for account in &asm.cfg.llm.accounts {
        let id = account.id.trim();
        if id.is_empty() || account.base_url.trim().is_empty() {
            continue;
        }
        let token = {
            let value = account.access_token.trim();
            (!value.is_empty()).then(|| value.to_string())
        };
        let credential = match account.protocol {
            kura_config::AccountProtocol::AnthropicMessages => {
                let client = kura_model_provider::AnthropicClient::new(
                    account.base_url.trim(),
                    account.model.trim(),
                    token,
                )
                .with_headers(account.headers.clone());
                let handle = client.credential();
                llm.register_provider(Arc::new(
                    kura_model_provider::ModelProviderBridge::new(id, client),
                ));
                handle
            }
            kura_config::AccountProtocol::OpenAiResponses => {
                let client = kura_model_provider::ResponsesClient::new(
                    account.base_url.trim(),
                    account.model.trim(),
                    token,
                )
                .with_headers(account.headers.clone());
                let handle = client.credential();
                llm.register_provider(Arc::new(
                    kura_model_provider::ModelProviderBridge::new(id, client),
                ));
                handle
            }
            kura_config::AccountProtocol::OpenAiCompatible => {
                let client = kura_model_provider::OpenAiCompatibleClient::new(
                    account.base_url.trim(),
                    account.model.trim(),
                    token,
                )
                .with_headers(account.headers.clone());
                let handle = client.credential();
                llm.register_provider(Arc::new(
                    kura_model_provider::OpenAiCompatibleProvider::new(id, client),
                ));
                handle
            }
        };
        asm.state.account_credentials.insert(id.to_string(), credential);
    }

    // Deterministic in-process fallback so the daemon always has a default
    // provider (Go registers echo in dispatcher.go).
    llm.register_provider(Arc::new(kura_llm::EchoProvider::new()));
    let default_provider = if asm.cfg.llm.default_provider.trim().is_empty() {
        "echo".to_string()
    } else {
        asm.cfg.llm.default_provider.clone()
    };
    let _ = llm.set_default_provider(&default_provider);

    asm.seams.put(ManagedRegistrySeam(managed_registry));
    asm.state.llm = Some(llm);
    Ok(())
}

fn build_skills(asm: &mut Assembly) -> Result<(), AppError> {
    let skills = Arc::new(
        SkillsRegistry::new(&asm.cfg.data_dir).map_err(|err| AppError::Skills(err.to_string()))?,
    );
    asm.state.skills = Some(skills);
    Ok(())
}

fn build_sandbox(asm: &mut Assembly) -> Result<(), AppError> {
    let sandboxes = Arc::new(SandboxManager::new(
        asm.cfg.clone(),
        Some(asm.secondary.clone()),
        (*asm.event_bus).clone(),
        PolicyEngine::new(),
    ));
    // The sandbox secret manager is a second instance sharing the same
    // store/backend because set_secret_manager takes ownership.
    let secret_store = asm.seams.get::<SecretStoreSeam>().expect("kernel secret store").0;
    let secret_backend = asm.seams.get::<SecretBackendSeam>().expect("kernel secret backend").0;
    sandboxes.set_secret_manager(SecretsManager::new(secret_store, secret_backend));
    asm.state.sandboxes = Some(sandboxes);
    asm.lifecycle.on_close(
        "sandbox",
        Box::new(|state| {
            if let Some(sandboxes) = &state.sandboxes {
                let _ = sandboxes.close();
            }
        }),
    );
    Ok(())
}

fn build_mcp(asm: &mut Assembly) -> Result<(), AppError> {
    let sandboxes = asm.state.sandboxes.clone().expect("sandbox plugin built");
    let secret_manager = asm.state.secrets.clone().expect("kernel secrets");
    let mcp_starter = Arc::new(adapters::McpExecutionStarter::new(sandboxes));
    let mcp_secret_resolver =
        Arc::new(adapters::McpSecretResolver::new(asm.store.clone(), secret_manager));
    let mcp = Arc::new(McpManager::new(
        asm.cfg.clone(),
        Some(asm.secondary.clone()),
        Some((*asm.event_bus).clone()),
        Some(mcp_starter.clone()),
        // The daemon's engine, not one of its own. An approval is answered
        // through the policy API, which reads this one; a private engine made
        // every approval a tool call raised unlistable and unanswerable, so a
        // tool marked `approval_required` could be asked for by the model and
        // granted by nobody.
        asm.state.policy.clone(),
        None, // concrete MCP transports attach lazily (restore path)
    ));
    mcp.set_secret_manager(mcp_secret_resolver.clone());
    asm.state.mcp = Some(mcp);
    #[cfg(test)]
    {
        asm.wiring.mcp_starter = Some(mcp_starter);
        asm.wiring.mcp_secret_resolver = Some(mcp_secret_resolver);
    }
    Ok(())
}

fn build_integrations(asm: &mut Assembly) -> Result<(), AppError> {
    asm.state.integrations = Some(Arc::new(IntegrationsManager::new(asm.env_scope)));
    Ok(())
}

fn build_calendar(asm: &mut Assembly) -> Result<(), AppError> {
    asm.state.calendar = Some(Arc::new(CalendarManager::new(asm.env_scope)));
    Ok(())
}

fn build_mail(asm: &mut Assembly) -> Result<(), AppError> {
    asm.state.mail = Some(Arc::new(MailManager::new(asm.env_scope)));
    Ok(())
}

fn build_providers(asm: &mut Assembly) -> Result<(), AppError> {
    let llm = asm.state.llm.clone().expect("llm plugin built");
    let managed_registry = asm.seams.get::<ManagedRegistrySeam>().expect("llm registry seam").0;
    asm.state.providers = Some(Arc::new(kura_providers::new_manager(
        asm.cfg.llm.clone(),
        Some(llm),
        vec![managed_registry],
    )));
    Ok(())
}

fn build_connectors(asm: &mut Assembly) -> Result<(), AppError> {
    asm.state.connectors = Some(Arc::new(ConnectorsSupervisor::new()));
    Ok(())
}

fn build_capabilities(asm: &mut Assembly) -> Result<(), AppError> {
    asm.state.capabilities = Some(Arc::new(CapabilitiesSupervisor::new()));
    Ok(())
}

fn build_chat(asm: &mut Assembly) -> Result<(), AppError> {
    let llm = asm.state.llm.clone().expect("llm plugin built");
    let mut chat = ChatService::new_service(
        llm,
        asm.state.providers.clone(),
        asm.state.skills.clone(),
        Some((*asm.event_bus).clone()),
        Some(asm.secondary.clone() as Arc<dyn ChatStore>),
    );
    if let Some(hooks) = asm.state.hooks.clone() {
        chat.set_hooks(hooks);
    }
    // Whatever the connected MCP servers publish, as tools the agent loop may
    // call. Read per turn rather than snapshotted here: servers are connected
    // and stopped while the daemon runs, and a registry built at assembly
    // would be whatever existed at boot -- which is nothing.
    //
    // Each one still goes through `authorize_tool` when it is called: being
    // offered is not being permitted, and a tool with no exposure rule is
    // refused however the model asks for it.
    if let Some(mcp) = asm.state.mcp.clone() {
        chat.set_tools(Arc::new(McpTools { mcp }));
    }
    asm.state.chat = Some(Arc::new(chat));
    Ok(())
}

/// The surface exposure rules are written against for chat turns.
const CHAT_RUNTIME_SURFACE: &str = "chat";

/// The connected MCP servers, as the tools a chat turn may call.
struct McpTools {
    mcp: Arc<kura_mcp::Manager>,
}

impl kura_chat::ToolSource for McpTools {
    fn registry(&self) -> Arc<kura_core::ToolRegistry> {
        let mut registry = kura_core::ToolRegistry::new();
        for tool in kura_mcp::tools_for_surface(&self.mcp, CHAT_RUNTIME_SURFACE) {
            registry.register(tool);
        }
        Arc::new(registry)
    }
}

/// The default context-assembly hook: injects the tenant's Ready L3/L2
/// memory bootstrap into the system frame under a budget, with citations
/// inline, and records the decision as a `context.assembled` event. Runs
/// before session-strategy in the pre-dispatch waterfall; later hooks
/// (builtin or external) may rewrite or veto the result.
struct ContextHook {
    state: Arc<std::sync::OnceLock<AppState>>,
    config: kura_context::ContextConfig,
}

impl ContextHook {
    /// Ready assets of one layer, newest first, visibility-filtered
    /// (private/team inject; restricted/agent wait for binding-aware
    /// loadouts and are recorded as excluded).
    /// Binding-aware loadout: `agent`-visibility assets inject only when
    /// their bindings contain the turn's active agent profile id.
    fn bootstrap_layer(
        memory: &MemoryManager,
        tenant_id: &str,
        agent_profile_id: &str,
        layer: kura_memory::MemoryLayer,
        excluded: &mut Vec<kura_context::ExcludedItem>,
    ) -> Vec<kura_context::BootstrapAsset> {
        let mut assets = memory.list(tenant_id, Some(layer), Some(kura_memory::AssetStatus::Ready));
        assets.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        let mut views = Vec::with_capacity(assets.len());
        for asset in assets {
            let admitted = match asset.visibility {
                kura_memory::Visibility::Private | kura_memory::Visibility::Team => true,
                kura_memory::Visibility::Agent => {
                    !agent_profile_id.trim().is_empty()
                        && asset.bindings.iter().any(|b| b == agent_profile_id.trim())
                }
                _ => false,
            };
            if admitted {
                views.push(kura_context::BootstrapAsset {
                    asset_id: asset.asset_id,
                    layer: asset.layer.as_str().to_string(),
                    title: asset.title,
                    content: asset.content,
                });
            } else {
                excluded.push(kura_context::ExcludedItem {
                    asset_id: asset.asset_id,
                    layer: asset.layer.as_str().to_string(),
                    reason: "visibility".to_string(),
                    source: "bootstrap".to_string(),
                });
            }
        }
        views
    }
}

impl kura_plugin::Hook for ContextHook {
    fn handle(&self, payload: &mut serde_json::Value) -> kura_plugin::HookOutcome {
        use serde_json::Value;
        let Some(state) = self.state.get() else {
            return kura_plugin::HookOutcome::Continue;
        };
        let Some(memory) = state.memory.as_deref() else {
            return kura_plugin::HookOutcome::Continue;
        };
        let tenant_id = payload
            .get("tenantId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        // Symbolic compression: oversized non-frame messages externalize to
        // an L0 memory ref (full content preserved, thread source link) and
        // the window keeps a preview plus the citation — token cost drops,
        // the evidence path stays (GET /v1/memory/assets/{id}). The last
        // user message (the current query) is never externalized, and
        // without a thread there is no evidence link, so nothing changes.
        let thread_id_for_refs = payload
            .get("threadId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if !thread_id_for_refs.is_empty() {
            let threshold = self.config.ref_threshold();
            if let Some(messages) = payload.get_mut("messages").and_then(Value::as_array_mut) {
                let last_user_idx = messages
                    .iter()
                    .rposition(|m| m.get("role").and_then(Value::as_str) == Some("user"));
                for (idx, message) in messages.iter_mut().enumerate() {
                    if Some(idx) == last_user_idx {
                        continue;
                    }
                    let role = message.get("role").and_then(Value::as_str).unwrap_or_default();
                    if role == "system" {
                        continue;
                    }
                    let Some(content) = message.get("content").and_then(Value::as_str) else {
                        continue;
                    };
                    if content.len() <= threshold {
                        continue;
                    }
                    let created = memory.create(kura_memory::CreateAssetInput {
                        kind: kura_memory::AssetKind::ChatMemory,
                        layer: kura_memory::MemoryLayer::L0Ref,
                        tenant_id: tenant_id.clone(),
                        owner: kura_memory::Actor {
                            kind: kura_memory::ActorKind::System,
                            id: "context".to_string(),
                        },
                        visibility: kura_memory::Visibility::Private,
                        title: "context_ref".to_string(),
                        content: content.to_string(),
                        source_links: vec![kura_memory::SourceLink {
                            kind: kura_memory::SourceKind::Thread,
                            id: thread_id_for_refs.clone(),
                            ..kura_memory::SourceLink::default()
                        }],
                        ..kura_memory::CreateAssetInput::default()
                    });
                    if let Ok((asset, _)) = created {
                        kura_api::routes::memory::persist_capture(state, &asset);
                        let preview: String = content.chars().take(200).collect();
                        message["content"] = Value::String(format!(
                            "{preview}… [externalized: full content at Memory[l0_ref {}]]",
                            asset.asset_id
                        ));
                    }
                }
            }
        }

        let agent_profile_id = payload
            .get("agentProfileId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let mut visibility_excluded = Vec::new();
        let mut candidates = Self::bootstrap_layer(
            memory,
            &tenant_id,
            &agent_profile_id,
            kura_memory::MemoryLayer::L3,
            &mut visibility_excluded,
        );
        candidates.extend(Self::bootstrap_layer(
            memory,
            &tenant_id,
            &agent_profile_id,
            kura_memory::MemoryLayer::L2,
            &mut visibility_excluded,
        ));
        let (mut injected, mut record) = kura_context::assemble(&candidates, self.config.budget());
        record.excluded.extend(visibility_excluded);

        // Query-time recall of L1 atoms (BM25 + recency, RRF fusion) over
        // the retrieval budget. The query is the turn's last user message;
        // atoms with no lexical overlap are never recalled.
        let query = payload
            .get("messages")
            .and_then(Value::as_array)
            .and_then(|messages| {
                messages
                    .iter()
                    .rev()
                    .find(|m| m.get("role").and_then(Value::as_str) == Some("user"))
            })
            .and_then(|m| m.get("content").and_then(Value::as_str))
            .unwrap_or_default()
            .to_string();
        if !query.trim().is_empty() {
            let mut atom_excluded = Vec::new();
            let atoms = Self::bootstrap_layer(
                memory,
                &tenant_id,
                &agent_profile_id,
                kura_memory::MemoryLayer::L1,
                &mut atom_excluded,
            );
            // Visibility-excluded atoms are only recorded when retrieval
            // actually runs (they were candidates for this query's corpus).
            record.excluded.extend(atom_excluded.into_iter().map(|mut item| {
                item.source = "retrieval".to_string();
                item
            }));
            let docs: Vec<kura_context::RetrievalDoc> = atoms
                .into_iter()
                .map(|asset| kura_context::RetrievalDoc {
                    asset_id: asset.asset_id,
                    title: asset.title,
                    content: asset.content,
                })
                .collect();
            // The vector ranker joins the fusion through the Embedder seam:
            // an installed external provider (state.embedder) wins, else the
            // deterministic hashed-ngram default.
            let default_embedder = kura_context::HashedNgramEmbedder::default();
            let embedder: &dyn kura_context::Embedder = match state.embedder.as_deref() {
                Some(external) => external,
                None => &default_embedder,
            };
            kura_context::retrieve_and_assemble(
                &query,
                &docs,
                Some(embedder),
                self.config.retrieval_budget(),
                &mut injected,
                &mut record,
            );
        }

        if record.is_empty() {
            return kura_plugin::HookOutcome::Continue;
        }

        // Splice the bootstrap in front of the first non-system message so
        // it joins the frame (persona/skills) rather than the history.
        if let Some(messages) = payload.get_mut("messages").and_then(Value::as_array_mut) {
            let insert_at = messages
                .iter()
                .position(|m| m.get("role").and_then(Value::as_str) != Some("system"))
                .unwrap_or(messages.len());
            for (offset, message) in injected.iter().enumerate() {
                messages.insert(
                    insert_at + offset,
                    serde_json::json!({ "role": message.role, "content": message.content }),
                );
            }
        }

        // Inspectability: the assembly decision is an event (best effort).
        let mut event_payload = serde_json::Map::new();
        event_payload.insert(
            "record".to_string(),
            serde_json::to_value(&record).unwrap_or(Value::Null),
        );
        event_payload.insert("tenantId".to_string(), Value::String(tenant_id));
        if let Some(thread_id) = payload.get("threadId").and_then(Value::as_str) {
            event_payload.insert("threadId".to_string(), Value::String(thread_id.to_string()));
        }
        let event = kura_events::Event {
            category: "context".to_string(),
            name: "context.assembled".to_string(),
            resource: kura_events::Resource {
                kind: "context_assembly".to_string(),
                id: uuid::Uuid::now_v7().to_string(),
            },
            payload: event_payload,
            ..kura_events::Event::default()
        };
        let event = state
            .store
            .lock()
            .append_event(&event)
            .unwrap_or(event);
        state.event_bus.publish(event);
        kura_plugin::HookOutcome::Continue
    }
}

fn build_context(asm: &mut Assembly) -> Result<(), AppError> {
    let Some(bus) = asm.state.hooks.clone() else {
        return Ok(());
    };
    let config_object = asm.profile.config_for("context");
    let config: kura_context::ContextConfig =
        serde_json::from_value(serde_json::Value::Object(config_object))
            .map_err(|err| AppError::PluginProfile(format!("context config: {err}")))?;
    bus.register(
        kura_plugin::points::CHAT_PRE_DISPATCH,
        "context",
        Arc::new(ContextHook { state: asm.late_state.clone(), config }),
    );
    Ok(())
}

/// The session-strategy hook: deterministic frame-preserving window shaping
/// at `chat/pre-dispatch` (see the `kura-session` crate). Runs before any
/// external plugin hooks (builtins register first).
///
/// Compression-to-memory: an elided span is never plain-dropped — when the
/// turn has a thread (the evidence link), the span is captured as an L0 ref
/// through the governed memory pipeline (the consolidator distills it into
/// L1/L2 asynchronously, write policy intact) and the elision marker cites
/// the captured asset so the model can drill back.
struct SessionStrategyHook {
    config: kura_session::SessionStrategyConfig,
    state: Arc<std::sync::OnceLock<AppState>>,
}

impl kura_plugin::Hook for SessionStrategyHook {
    fn handle(&self, payload: &mut serde_json::Value) -> kura_plugin::HookOutcome {
        use serde_json::Value;
        let source_kind = payload
            .get("sourceKind")
            .and_then(Value::as_str)
            .map(str::to_string);
        let Some(messages_value) = payload.get("messages") else {
            return kura_plugin::HookOutcome::Continue;
        };
        let Ok(messages) =
            serde_json::from_value::<Vec<kura_session::WindowMessage>>(messages_value.clone())
        else {
            return kura_plugin::HookOutcome::Continue;
        };
        let mut shaped = kura_session::shape_window(
            &messages,
            self.config.budget_for(source_kind.as_deref()),
            self.config.keep_recent_floor(),
        );
        if shaped.elided == 0 {
            return kura_plugin::HookOutcome::Continue;
        }

        // Capture the elided span before it leaves the window. Without a
        // thread there is no evidence link to hang the L0 ref on; the span
        // then remains reachable through the dispatch records only.
        let thread_id = payload
            .get("threadId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if !thread_id.is_empty() {
            if let Some(state) = self.state.get() {
                let tenant_id = payload
                    .get("tenantId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let span = shaped
                    .elided_messages
                    .iter()
                    .map(|m| format!("{}: {}", m.role, m.content))
                    .collect::<Vec<_>>()
                    .join("\n");
                let captured = kura_api::routes::memory::capture_l0(
                    state,
                    &tenant_id,
                    kura_memory::Actor {
                        kind: kura_memory::ActorKind::System,
                        id: "session-strategy".to_string(),
                    },
                    "session_eviction",
                    &span,
                    vec![kura_memory::SourceLink {
                        kind: kura_memory::SourceKind::Thread,
                        id: thread_id,
                        ..kura_memory::SourceLink::default()
                    }],
                );
                if let Some((asset_id, due)) = captured {
                    // The marker cites the captured span: eviction leaves a
                    // drill-down path, not a hole.
                    if let Some(marker) = shaped
                        .messages
                        .iter_mut()
                        .find(|m| m.content.contains("elided by the session-strategy"))
                    {
                        marker.content = format!(
                            "{}; elided span captured as Memory[l0_ref {asset_id}]",
                            marker.content.trim_end_matches(']'),
                        );
                        marker.content.push(']');
                    }
                    if due {
                        let state = state.clone();
                        let tenant_id = tenant_id.clone();
                        std::thread::spawn(move || {
                            if let Err(err) = kura_api::routes::memory::execute_consolidation(
                                &state, &tenant_id, "turns", None,
                            ) {
                                eprintln!(
                                    "memory: eviction-trigger consolidation failed: {err:?}"
                                );
                            }
                        });
                    }
                }
            }
        }

        if let Ok(new_messages) = serde_json::to_value(&shaped.messages) {
            payload["messages"] = new_messages;
        }
        kura_plugin::HookOutcome::Continue
    }
}

fn build_session_strategy(asm: &mut Assembly) -> Result<(), AppError> {
    let Some(bus) = asm.state.hooks.clone() else {
        return Ok(());
    };
    // A malformed operator config fails the boot loudly (same posture as a
    // malformed profile) instead of silently running default budgets.
    let config_object = asm.profile.config_for("session-strategy");
    let config: kura_session::SessionStrategyConfig =
        serde_json::from_value(serde_json::Value::Object(config_object)).map_err(|err| {
            AppError::PluginProfile(format!("session-strategy config: {err}"))
        })?;
    bus.register(
        kura_plugin::points::CHAT_PRE_DISPATCH,
        "session-strategy",
        Arc::new(SessionStrategyHook { config, state: asm.late_state.clone() }),
    );
    Ok(())
}

fn build_billing(asm: &mut Assembly) -> Result<(), AppError> {
    let billing_repo = Arc::new(BillingRepositoryHandle::new(asm.open_store()?));
    asm.state.billing = Some(Arc::new(BillingManager::new(billing_repo)));
    Ok(())
}

fn build_activation(asm: &mut Assembly) -> Result<(), AppError> {
    let billing = asm.state.billing.clone().expect("billing plugin built");
    let activation_store = Arc::new(
        SqliteActivationStore::new(asm.open_store()?).map_err(AppError::Store)?,
    );
    let activation_billing = Arc::new(ActivationBillingProjectorAdapter::new(billing));
    let activation_chat = Arc::new(ActivationChatRunnerAdapter::new(asm.state.chat.clone()));
    asm.state.activation = Some(Arc::new(ActivationService::with_sqlite(
        activation_store.clone(),
        Some(activation_billing.clone()),
        Some(activation_chat.clone()),
        asm.env_scope,
        asm.hosted,
    )));
    #[cfg(test)]
    {
        asm.wiring.activation_store = Some(activation_store);
        asm.wiring.activation_billing = Some(activation_billing);
        asm.wiring.activation_chat = Some(activation_chat);
    }
    Ok(())
}

fn build_computer_use(asm: &mut Assembly) -> Result<(), AppError> {
    let computeruse_store = Arc::new(ComputerUseStoreHandle::new(asm.open_store()?));
    let computeruse_recorder = Arc::new(SqliteArtifactRecorder::new(
        computeruse_store.clone() as Arc<dyn kura_computeruse::Store>,
        &asm.cfg.data_dir,
        asm.env_scope,
    ));
    asm.state.computer_use = Some(Arc::new(ComputerUseManager::new(ComputerUseDependencies {
        environment_scope: asm.env_scope.to_string(),
        runtime: asm.state.runtime.clone(),
        policy: asm.state.policy.clone(),
        store: computeruse_store,
        driver: None,
        artifacts: Some(computeruse_recorder.clone()),
    })));
    #[cfg(test)]
    {
        asm.wiring.computeruse_recorder = Some(computeruse_recorder);
    }
    Ok(())
}

fn build_delivery(asm: &mut Assembly) -> Result<(), AppError> {
    let delivery_connector = Arc::new(ConnectorAdapter::new(asm.store.clone()));
    asm.state.delivery = Some(Arc::new(DeliveryManager::new(
        asm.env_scope,
        (*asm.event_bus).clone(),
        asm.store.clone(),
        vec![Arc::new(TestSinkAdapter::new()), delivery_connector.clone()],
    )));
    #[cfg(test)]
    {
        asm.wiring.delivery_connector = Some(delivery_connector);
    }
    Ok(())
}

fn build_scheduler(asm: &mut Assembly) -> Result<(), AppError> {
    let runtime = asm.state.runtime.clone().expect("kernel runtime");
    let workflow_launcher = asm.seams.get::<WorkflowLauncherSeam>().expect("kernel launcher").0;
    asm.state.scheduler = Some(Arc::new(Scheduler::new(SchedulerDependencies {
        environment: asm.cfg.environment,
        runtime,
        event_bus: Some((*asm.event_bus).clone()),
        store: asm.store.clone(),
        workflow_launcher: Some(workflow_launcher),
        clock: None,
        tick_interval: Duration::ZERO,
    })));
    asm.lifecycle.on_start(
        "scheduler",
        Box::new(|state| {
            if let Some(scheduler) = &state.scheduler {
                if let Err(err) = scheduler.start() {
                    eprintln!("[kura] scheduler start failed: {err}");
                }
            }
        }),
    );
    asm.lifecycle.on_close(
        "scheduler",
        Box::new(|state| {
            if let Some(scheduler) = &state.scheduler {
                let _ = scheduler.close();
            }
        }),
    );
    Ok(())
}

fn build_reminders(asm: &mut Assembly) -> Result<(), AppError> {
    let workflow_launcher = asm.seams.get::<WorkflowLauncherSeam>().expect("kernel launcher").0;
    asm.state.reminders = Some(Arc::new(RemindersManager::new(RemindersDependencies {
        environment_scope: asm.env_scope.to_string(),
        store: asm.store.clone(),
        event_bus: Some((*asm.event_bus).clone()),
        delivery: asm.state.delivery.as_ref().map(|delivery| (**delivery).clone()),
        workflow_launcher: Some(workflow_launcher),
        clock: None,
        tick_interval: Duration::ZERO,
    })));
    asm.lifecycle.on_start(
        "reminders",
        Box::new(|state| {
            if let Some(reminders) = &state.reminders {
                if let Err(err) = reminders.start() {
                    eprintln!("[kura] reminders start failed: {err}");
                }
            }
        }),
    );
    asm.lifecycle.on_close(
        "reminders",
        Box::new(|state| {
            if let Some(reminders) = &state.reminders {
                reminders.close();
            }
        }),
    );
    Ok(())
}

fn build_routines(asm: &mut Assembly) -> Result<(), AppError> {
    let scheduler = asm.state.scheduler.clone().expect("scheduler plugin built");
    let mut routine_manager = kura_routine::Manager::new(
        asm.env_scope,
        Box::new(adapters::RoutineSchedulerAdapter::new(scheduler)),
    );
    routine_manager.with_store(asm.store.clone());
    asm.state.routines = Some(Arc::new(routine_manager));
    Ok(())
}

/// Memory's chat-turn capture (spec 058 phase 2 W1), owned by the memory
/// plugin as a `chat/turn-end` observer instead of a hardcoded API-layer
/// call. Channel-source turns are captured too — gateway-driven IM traffic
/// reaches chat without touching the HTTP ingress pipeline, so this hook is
/// its only capture point. (HTTP-pipeline messages that also dispatch chat
/// produce both an `inbound_message` and a `chat_turn` L0; accepted — L0s
/// are excerpt evidence and consolidation extracts through citations.)
struct MemoryCaptureHook {
    state: Arc<std::sync::OnceLock<AppState>>,
}

impl kura_plugin::Hook for MemoryCaptureHook {
    fn handle(&self, payload: &mut serde_json::Value) -> kura_plugin::HookOutcome {
        use serde_json::Value;
        let text_of = |key: &str| -> String {
            payload.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
        };
        let Some(state) = self.state.get() else {
            return kura_plugin::HookOutcome::Continue;
        };
        let tenant_id = text_of("tenantId");
        let thread_id = text_of("threadId");
        let dispatch_id = text_of("dispatchId");
        let source_message_id = text_of("sourceMessageId");
        let mut links = Vec::new();
        if !source_message_id.trim().is_empty() {
            links.push(kura_memory::SourceLink {
                kind: kura_memory::SourceKind::Message,
                id: source_message_id,
                ..kura_memory::SourceLink::default()
            });
        }
        if !thread_id.trim().is_empty() {
            links.push(kura_memory::SourceLink {
                kind: kura_memory::SourceKind::Thread,
                id: thread_id,
                ..kura_memory::SourceLink::default()
            });
        }
        if !dispatch_id.trim().is_empty() {
            links.push(kura_memory::SourceLink {
                kind: kura_memory::SourceKind::Run,
                id: dispatch_id,
                ..kura_memory::SourceLink::default()
            });
        }
        if links.is_empty() {
            return kura_plugin::HookOutcome::Continue;
        }
        let text = format!("user: {}\nassistant: {}", text_of("query").trim(), text_of("output"));
        let captured = kura_api::routes::memory::capture_l0(
            state,
            &tenant_id,
            kura_memory::Actor {
                kind: kura_memory::ActorKind::System,
                id: "chat".to_string(),
            },
            "chat_turn",
            &text,
            links,
        );
        if captured.is_some_and(|(_, due)| due) {
            // Consolidation is blocking LLM + store work; run it off the
            // reply path on a plain thread (hooks run on service threads
            // that may have no tokio context, e.g. the IM message loops).
            let state = state.clone();
            std::thread::spawn(move || {
                if let Err(err) = kura_api::routes::memory::execute_consolidation(
                    &state, &tenant_id, "turns", None,
                ) {
                    eprintln!("memory: turn-trigger consolidation failed: {err:?}");
                }
            });
        }
        kura_plugin::HookOutcome::Continue
    }
}

fn build_memory(asm: &mut Assembly) -> Result<(), AppError> {
    let llm = asm.state.llm.clone().expect("llm plugin built");
    asm.state.memory = Some(Arc::new(MemoryManager::new(
        asm.env_scope,
        None,
        Some(Arc::new(adapters::LlmConsolidator::new(llm))),
        None,
    )));
    // Chat-turn capture rides the hook plane (fires for query and stream).
    if let Some(bus) = &asm.state.hooks {
        bus.register(
            kura_plugin::points::CHAT_TURN_END,
            "memory",
            Arc::new(MemoryCaptureHook { state: asm.late_state.clone() }),
        );
    }
    // The 60s consolidation/retention tick is plugin-owned lifecycle: idle
    // triggers and retention sweep on the blocking pool.
    asm.lifecycle.on_start(
        "memory",
        Box::new(|state| {
            let state = state.clone();
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(Duration::from_secs(60));
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    ticker.tick().await;
                    let tick_state = state.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        kura_api::routes::memory::memory_tick(&tick_state);
                    })
                    .await;
                }
            });
        }),
    );
    Ok(())
}

fn build_triage(asm: &mut Assembly) -> Result<(), AppError> {
    let mut triage_manager = TriageManager::new(asm.env_scope);
    triage_manager.with_store(asm.store.clone());
    asm.state.triage = Some(Arc::new(triage_manager));
    Ok(())
}

fn build_webhooks(asm: &mut Assembly) -> Result<(), AppError> {
    let billing = asm.state.billing.clone().expect("billing plugin built");
    let workflow_launcher = asm.seams.get::<WorkflowLauncherSeam>().expect("kernel launcher").0;
    let mut webhook_manager = WebhookManager::new(
        asm.env_scope,
        Some(Box::new(adapters::WebhookFirerImpl::new(workflow_launcher))),
        Some(Box::new(adapters::WebhookQuotaGateImpl::new(
            billing,
            asm.store.clone(),
            asm.event_bus.clone(),
            asm.env_scope,
        ))),
    );
    webhook_manager.with_store(asm.store.clone());
    asm.state.webhooks = Some(Arc::new(webhook_manager));
    Ok(())
}

fn build_catalog(asm: &mut Assembly) -> Result<(), AppError> {
    let sandboxes = asm.state.sandboxes.clone().expect("sandbox plugin built");
    let mut catalog_manager = kura_catalog::Manager::new(
        asm.env_scope,
        Some(Box::new(adapters::CatalogSandboxRequirementChecker::new(sandboxes))),
        Some(Box::new(adapters::CatalogTenantPermissionGate::new(asm.store.clone()))),
    );
    catalog_manager.with_store(asm.store.clone());
    asm.state.catalog = Some(Arc::new(catalog_manager));
    Ok(())
}

fn build_exec_profiles(asm: &mut Assembly) -> Result<(), AppError> {
    let sandboxes = asm.state.sandboxes.clone().expect("sandbox plugin built");
    #[cfg(test)]
    {
        asm.wiring.execprofile_health =
            Some(Arc::new(SandboxHealthChecker::new(Some(sandboxes.clone()))));
    }
    let mut exec_profile_manager = ExecProfileManager::new(
        asm.env_scope,
        Some(Box::new(SandboxHealthChecker::new(Some(sandboxes.clone())))),
        Some(Box::new(adapters::ExecProfileSandboxRequirementChecker::new(sandboxes))),
        Some(Box::new(adapters::ExecProfileTenantPermissionGate::new(asm.store.clone()))),
    );
    exec_profile_manager.with_store(asm.store.clone());
    asm.state.exec_profiles = Some(Arc::new(exec_profile_manager));
    Ok(())
}

fn build_evidence(asm: &mut Assembly) -> Result<(), AppError> {
    let routines = asm.state.routines.clone().expect("routines plugin built");
    #[cfg(test)]
    {
        asm.wiring.evidence_collector =
            Some(Arc::new(RoutineCollector::new(Some(routines.clone()))));
    }
    let mut evidence_manager = EvidenceManager::new(
        asm.env_scope,
        Some(Box::new(RoutineCollector::new(Some(routines)))),
        Some(Box::new(adapters::EvidenceSupportPermissionGate::new(asm.store.clone()))),
    );
    evidence_manager.with_store(asm.store.clone());
    asm.state.evidence = Some(Arc::new(evidence_manager));
    Ok(())
}

fn build_evaluation(asm: &mut Assembly) -> Result<(), AppError> {
    let billing = asm.state.billing.clone().expect("billing plugin built");
    let evaluation_store = Arc::new(EvaluationStoreHandle::new(asm.open_store()?));
    asm.state.evaluation = Some(Arc::new(EvaluationManager::new(EvaluationDependencies {
        environment_scope: asm.env_scope.to_string(),
        store: Some(evaluation_store),
        fixtures_dir: String::new(),
        runtime_recorder: None,
        billing: Some(billing),
        hosted_billing: asm.hosted,
        clock: None,
    })));
    Ok(())
}

fn build_live_validation(asm: &mut Assembly) -> Result<(), AppError> {
    let billing = asm.state.billing.clone().expect("billing plugin built");
    let live_validation_store = Arc::new(LiveValidationStoreHandle::new(asm.open_store()?));
    asm.state.live_validation = Some(Arc::new(LiveValidationManager::new(
        LiveValidationDependencies {
            environment_scope: asm.env_scope.to_string(),
            store: Some(live_validation_store),
            enabled: true,
            billing: Some(billing),
            hosted_billing: asm.hosted,
            clock: None,
            ledger_event_sink: None,
            candidate_tool_class_resolver: None,
        },
    )));
    Ok(())
}

fn build_setup_wizard(asm: &mut Assembly) -> Result<(), AppError> {
    let secrets = asm.state.secrets.clone().expect("kernel secrets");
    let setup_wizard_store = Arc::new(SetupWizardStoreHandle::new(asm.open_store()?));
    asm.state.setup_wizard = Some(Arc::new(new_setup_wizard(SetupWizardDependencies {
        store: Some(setup_wizard_store),
        secrets: Some(secrets),
        ..SetupWizardDependencies::default()
    })));
    Ok(())
}

fn build_self_improve(asm: &mut Assembly) -> Result<(), AppError> {
    let config_object = asm.profile.config_for("self-improve");
    let config: kura_improvement::ImprovementConfig =
        serde_json::from_value(serde_json::Value::Object(config_object))
            .map_err(|err| AppError::PluginProfile(format!("self-improve config: {err}")))?;
    asm.state.improvement =
        Some(Arc::new(kura_improvement::Manager::new(&asm.cfg.data_dir, config)));
    Ok(())
}

/// Channel plugins assemble nothing here; their runtimes are constructed in
/// `App::serve` behind both the connector config flag and the plugin's
/// resolved enablement.
#[allow(clippy::unnecessary_wraps)]
fn build_channel_noop(_asm: &mut Assembly) -> Result<(), AppError> {
    Ok(())
}

/// Ensures the data dir exists before the profile is read (the store creates
/// it later anyway; profile load must not fail on a fresh install).
pub(crate) fn ensure_data_dir(data_dir: &str) {
    let _ = std::fs::create_dir_all(Path::new(data_dir));
}
