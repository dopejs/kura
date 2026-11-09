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

use crate::AppError;
use crate::adapters;

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
            // Stage 9.4: the subprocess browser driver reports to the
            // capability supervisor.
            requires: &["capabilities"],
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
            id: "tools",
            summary: "Tool provider profiles (web search, image generation, browser)",
            provides: &["tools.manager"],
            // Every tool call is third-party spend, so the plane does not
            // assemble without the quota plane it reserves against.
            requires: &["billing"],
        },
        build: build_tools,
    },
    BuiltinPlugin {
        descriptor: PluginDescriptor {
            id: "swarm",
            summary: "Bounded concurrent sub-agents (opt-in via swarm.config.enabled)",
            provides: &["swarm.manager"],
            // A fan-out multiplies spend and runs turns: it does not assemble
            // without the quota plane it reserves against or the chat service
            // its children run through.
            requires: &["billing", "chat"],
        },
        build: build_swarm,
    },
    BuiltinPlugin {
        descriptor: PluginDescriptor {
            id: "work-items",
            summary: "Long-lived work items (opt-in via work-items.config.enabled)",
            provides: &["workitems.manager"],
            // Every round is a chat turn.
            requires: &["chat"],
        },
        build: build_work_items,
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
                llm.register_provider(Arc::new(kura_model_provider::ModelProviderBridge::new(
                    id, client,
                )));
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
                llm.register_provider(Arc::new(kura_model_provider::ModelProviderBridge::new(
                    id, client,
                )));
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
        asm.state
            .account_credentials
            .insert(id.to_string(), credential);
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

    crate::llm_credentials::install(&llm, &asm.state.hosting, asm.state.secrets.as_ref())?;

    asm.seams.put(ManagedRegistrySeam(managed_registry));
    asm.state.llm = Some(llm);
    Ok(())
}

/// Stage 3.4: counts an invocation for every skill a turn selected.
struct SkillUsageHook {
    state: Arc<std::sync::OnceLock<AppState>>,
}
impl kura_plugin::Hook for SkillUsageHook {
    fn handle(&self, payload: &mut serde_json::Value) -> kura_plugin::HookOutcome {
        if let (Some(state), Some(skills)) = (
            self.state.get(),
            payload.get("skills").and_then(serde_json::Value::as_array),
        ) {
            for id in skills.iter().filter_map(serde_json::Value::as_str) {
                kura_api::routes::skill_proposals::record_invocation(state, id);
            }
        }
        kura_plugin::HookOutcome::Continue
    }
}

fn build_skills(asm: &mut Assembly) -> Result<(), AppError> {
    let skills = Arc::new(
        SkillsRegistry::new(&asm.cfg.data_dir).map_err(|err| AppError::Skills(err.to_string()))?,
    );
    asm.state.skills = Some(skills);
    if let Some(bus) = asm.state.hooks.clone() {
        bus.register(
            kura_plugin::points::CHAT_TURN_END,
            "skills",
            Arc::new(SkillUsageHook {
                state: asm.late_state.clone(),
            }),
        );
    }
    Ok(())
}

/// Stage 3.2: a shared daemon gives tenants no plain subprocess path.
pub(crate) fn apply_hosting_floor(
    sandboxes: &SandboxManager,
    hosting: &kura_api::hosting::Hosting,
) {
    if let kura_api::hosting::Hosting::Shared { operator_tenant_id } = hosting {
        sandboxes.require_tenant_isolation(operator_tenant_id);
    }
}

/// Stage 3.5: the tenant egress path, from `KURA_SANDBOX_TENANT_NETWORK`
/// (a docker `--internal` network) and `KURA_SANDBOX_EGRESS_PROXY` (the
/// `kura egress-proxy` URL on it). Any problem leaves isolated runs offline
/// and says why; it never fails the boot, and never opens network around
/// the proxy.
fn configure_tenant_egress(sandboxes: &SandboxManager) {
    let read = |key: &str| {
        std::env::var(key)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };
    match (
        read("KURA_SANDBOX_TENANT_NETWORK"),
        read("KURA_SANDBOX_EGRESS_PROXY"),
    ) {
        (None, None) => {}
        (Some(network), Some(proxy)) => {
            if let Err(err) = sandboxes.configure_tenant_egress(&network, &proxy) {
                eprintln!("[kura] sandbox: tenant egress disabled: {err}");
                return;
            }
            // 3.6: with the key the proxy also holds, each tenant's requests
            // carry its own credential. A key that cannot be used turns
            // egress off rather than running without attribution the
            // operator asked for (the proxy would refuse every request).
            let attribution = match read("KURA_SANDBOX_EGRESS_KEY_FILE") {
                None => "unattributed (no KURA_SANDBOX_EGRESS_KEY_FILE)",
                Some(path) => match kura_egress::EgressKey::from_file(std::path::Path::new(&path))
                    .and_then(|key| sandboxes.configure_tenant_egress_key(key))
                {
                    Ok(()) => "each tenant with its own proxy credential",
                    Err(err) => {
                        sandboxes.disable_tenant_egress();
                        eprintln!("[kura] sandbox: tenant egress disabled: {err}");
                        return;
                    }
                },
            };
            eprintln!(
                "[kura] sandbox: tenant egress through {proxy} on internal network {network}, \
                 {attribution}"
            );
        }
        _ => eprintln!(
            "[kura] sandbox: tenant egress disabled: set both KURA_SANDBOX_TENANT_NETWORK \
             and KURA_SANDBOX_EGRESS_PROXY"
        ),
    }
}

/// Stage 3.4: `KURA_SANDBOX_DOCKER_RUNTIME` names the OCI runtime isolated
/// runs use (`runsc` for gVisor, measured in agent-runtime-program.md 3.4).
/// Unset keeps docker's default. A runtime docker does not have stops the
/// boot: falling back to the default would quietly drop isolation the
/// operator asked for.
fn configure_tenant_runtime(sandboxes: &SandboxManager) -> Result<(), AppError> {
    let Some(runtime) = std::env::var("KURA_SANDBOX_DOCKER_RUNTIME")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };
    sandboxes
        .configure_tenant_runtime(&runtime)
        .map_err(|err| AppError::Sandbox(format!("KURA_SANDBOX_DOCKER_RUNTIME: {err}")))?;
    eprintln!("[kura] sandbox: isolated runs use the {runtime} container runtime");
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
    let secret_store = asm
        .seams
        .get::<SecretStoreSeam>()
        .expect("kernel secret store")
        .0;
    let secret_backend = asm
        .seams
        .get::<SecretBackendSeam>()
        .expect("kernel secret backend")
        .0;
    sandboxes.set_secret_manager(SecretsManager::new(secret_store, secret_backend));
    apply_hosting_floor(&sandboxes, &asm.state.hosting);
    configure_tenant_egress(&sandboxes);
    configure_tenant_runtime(&sandboxes)?;
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
    let mcp_secret_resolver = Arc::new(adapters::McpSecretResolver::new(
        asm.store.clone(),
        secret_manager,
    ));
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
    let managed_registry = asm
        .seams
        .get::<ManagedRegistrySeam>()
        .expect("llm registry seam")
        .0;
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
        // Reads on the store pool's readers, writes on the chat's own
        // connection: with one connection for both, every turn's reads
        // queued behind every other turn's writes (4.5).
        Some(Arc::new(kura_chat::PooledChatStore::new(
            asm.secondary.clone(),
            asm.state.store_pool.clone(),
        )) as Arc<dyn ChatStore>),
    );
    if let Some(hooks) = asm.state.hooks.clone() {
        chat.set_hooks(hooks);
    }
    // Which tools a turn may call is resolved per turn from the late state
    // (tools/memory/mcp build after chat): the connected MCP servers' tools,
    // `memory.lookup`, and one tool per Ready tool profile. Each is wrapped so
    // a call runs the `chat/tool-call` hook, is evented and counted.
    chat.set_tools(Arc::new(crate::tool_host::AppTools::new(
        asm.late_state.clone(),
    )));
    let config_object = asm.profile.config_for("chat");
    let config: ChatPluginConfig = serde_json::from_value(serde_json::Value::Object(config_object))
        .map_err(|err| AppError::PluginProfile(format!("chat config: {err}")))?;
    if config.tool_max_rounds > 0 {
        chat.set_max_tool_rounds(config.tool_max_rounds);
    }
    if config.tool_max_parallel > 0 {
        chat.set_max_parallel_tool_calls(config.tool_max_parallel);
    }
    if config.turn_resume == Some(false) {
        chat.set_turn_resume(false);
    }
    if config.tool_output_framing == Some(false) {
        chat.set_frame_tool_output(false);
    }
    if let Some(limits) = turn_limits(&config.limits, &asm.state.hosting)? {
        eprintln!(
            "[kura] turn admission: global={} tenant={} turns/min={} tokens/min={} queue={}/{} timeout={}s",
            limits.global_concurrency,
            limits.tenant_concurrency,
            limits.turns_per_minute,
            limits.tokens_per_minute,
            limits.tenant_queue_depth,
            limits.global_queue_depth,
            limits.queue_timeout.as_secs()
        );
        chat.set_turn_limits(limits);
    }
    asm.state.chat = Some(Arc::new(chat));
    Ok(())
}

/// `plugins.json` → `entries.chat.config`.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct ChatPluginConfig {
    /// Bound on model-tool-model rounds per turn; 0 = the chat crate's default.
    tool_max_rounds: usize,
    /// Parallel-safe calls of one round run at once (2.6); 0 = the default
    /// (4), 1 = strictly serial.
    tool_max_parallel: usize,
    /// Checkpoint tool-calling turns so a retried request resumes from its
    /// last settled call (2.7); absent = on, `false` turns it off.
    turn_resume: Option<bool>,
    /// Frame tool results as untrusted data for the model (2.8, G9);
    /// absent = on, `false` turns it off.
    tool_output_framing: Option<bool>,
    /// Turn admission (4.1).
    limits: TurnLimitsConfig,
}

/// `entries.chat.config.limits`: per-tenant turn limits and the fair queue
/// (`docs/harness/turn-admission.md` §3). An absent key takes the hosting
/// mode's default; `0` is unbounded. Unknown keys fail the boot: a misspelt
/// limit would otherwise be silently unenforced.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
struct TurnLimitsConfig {
    /// `false` removes the gate; the rollback.
    enabled: Option<bool>,
    global_concurrency: Option<usize>,
    tenant_concurrency: Option<usize>,
    turns_per_minute: Option<u64>,
    tokens_per_minute: Option<u64>,
    tenant_queue_depth: Option<usize>,
    global_queue_depth: Option<usize>,
    queue_timeout_seconds: Option<u64>,
}

/// Defaults (turn-admission.md §3). Global: 64 sits between the 32 and 128
/// sessions measured in 4.3; 64 running plus 256 waiting stays under tokio's
/// 512 blocking threads. Shared, per tenant: a 4-child swarm plus
/// interactive turns beside it.
const DEFAULT_GLOBAL_TURNS: usize = 64;
const DEFAULT_GLOBAL_QUEUE_DEPTH: usize = 256;
const DEFAULT_QUEUE_TIMEOUT_SECS: u64 = 120;
const SHARED_TENANT_TURNS: usize = 8;
const SHARED_TURNS_PER_MINUTE: u64 = 30;
const SHARED_TENANT_QUEUE_DEPTH: usize = 16;

/// The limits for this hosting mode, or `None` when the gate is off.
fn turn_limits(
    config: &TurnLimitsConfig,
    hosting: &kura_api::hosting::Hosting,
) -> Result<Option<kura_chat::TurnLimits>, AppError> {
    if config.enabled == Some(false) {
        return Ok(None);
    }
    let (tenant_turns, turns_per_minute, tenant_queue, exempt_tenants) = match hosting {
        kura_api::hosting::Hosting::Shared { operator_tenant_id } => (
            SHARED_TENANT_TURNS,
            SHARED_TURNS_PER_MINUTE,
            SHARED_TENANT_QUEUE_DEPTH,
            vec![operator_tenant_id.clone()],
        ),
        kura_api::hosting::Hosting::Single => (0, 0, 0, Vec::new()),
    };
    let queue_timeout_secs = config
        .queue_timeout_seconds
        .unwrap_or(DEFAULT_QUEUE_TIMEOUT_SECS);
    if queue_timeout_secs == 0 {
        return Err(AppError::PluginProfile(
            "chat config: limits.queueTimeoutSeconds must be at least 1".to_string(),
        ));
    }
    Ok(Some(kura_chat::TurnLimits {
        global_concurrency: config.global_concurrency.unwrap_or(DEFAULT_GLOBAL_TURNS),
        tenant_concurrency: config.tenant_concurrency.unwrap_or(tenant_turns),
        turns_per_minute: config.turns_per_minute.unwrap_or(turns_per_minute),
        tokens_per_minute: config.tokens_per_minute.unwrap_or(0),
        tenant_queue_depth: config.tenant_queue_depth.unwrap_or(tenant_queue),
        global_queue_depth: config
            .global_queue_depth
            .unwrap_or(DEFAULT_GLOBAL_QUEUE_DEPTH),
        queue_timeout: Duration::from_secs(queue_timeout_secs),
        exempt_tenants,
    }))
}

#[cfg(test)]
mod turn_limits_tests {
    use super::*;
    use kura_api::hosting::Hosting;

    fn parse(json: serde_json::Value) -> Result<TurnLimitsConfig, serde_json::Error> {
        serde_json::from_value(json)
    }

    #[test]
    fn single_mode_caps_the_daemon_and_leaves_tenants_unbounded() {
        let limits = turn_limits(&TurnLimitsConfig::default(), &Hosting::Single)
            .unwrap()
            .unwrap();
        assert_eq!(limits.global_concurrency, 64);
        assert_eq!(limits.global_queue_depth, 256);
        assert_eq!(limits.tenant_concurrency, 0);
        assert_eq!(limits.turns_per_minute, 0);
        assert_eq!(limits.tokens_per_minute, 0);
        assert_eq!(limits.queue_timeout, Duration::from_secs(120));
        assert!(limits.exempt_tenants.is_empty());
    }

    #[test]
    fn shared_mode_limits_tenants_and_exempts_the_operator() {
        let hosting = Hosting::Shared {
            operator_tenant_id: "ten_op".to_string(),
        };
        let limits = turn_limits(&TurnLimitsConfig::default(), &hosting)
            .unwrap()
            .unwrap();
        assert_eq!(limits.tenant_concurrency, 8);
        assert_eq!(limits.turns_per_minute, 30);
        assert_eq!(limits.tenant_queue_depth, 16);
        assert_eq!(limits.tokens_per_minute, 0);
        assert_eq!(limits.exempt_tenants, ["ten_op"]);
    }

    #[test]
    fn configured_values_win_and_disabled_removes_the_gate() {
        let config = parse(serde_json::json!({
            "tenantConcurrency": 2,
            "tokensPerMinute": 50000,
            "queueTimeoutSeconds": 10
        }))
        .unwrap();
        let limits = turn_limits(&config, &Hosting::Single).unwrap().unwrap();
        assert_eq!(limits.tenant_concurrency, 2);
        assert_eq!(limits.tokens_per_minute, 50_000);
        assert_eq!(limits.queue_timeout, Duration::from_secs(10));
        let off = parse(serde_json::json!({"enabled": false})).unwrap();
        assert!(turn_limits(&off, &Hosting::Single).unwrap().is_none());
    }

    #[test]
    fn a_misspelt_key_or_a_zero_timeout_is_refused() {
        assert!(parse(serde_json::json!({"tenantConcurency": 2})).is_err());
        let zero = parse(serde_json::json!({"queueTimeoutSeconds": 0})).unwrap();
        assert!(turn_limits(&zero, &Hosting::Single).is_err());
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
    /// Vectors for the retrieval corpus. Delegates to the API crate's
    /// implementation so this path and `/v1/retrieval/queries` share one
    /// derived index and cannot drift apart.
    fn corpus_vectors(
        state: &kura_api::AppState,
        embedder: &dyn kura_context::Embedder,
        docs: &[kura_context::RetrievalDoc],
    ) -> Vec<Vec<f32>> {
        kura_api::routes::retrieval::corpus_vectors(state, embedder, docs)
    }

    fn bootstrap_layer(
        memory: &MemoryManager,
        tenant_id: &str,
        agent_profile_id: &str,
        layer: kura_memory::MemoryLayer,
        excluded: &mut Vec<kura_context::ExcludedItem>,
    ) -> Vec<kura_context::BootstrapAsset> {
        let mut assets = memory.list(
            tenant_id,
            Some(layer),
            Some(kura_memory::AssetStatus::Ready),
        );
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
                    let role = message
                        .get("role")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
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
        // Framed as data (G9 for memory): what memory holds can come from
        // web pages and tool results captured into it.
        let frame = self.config.frame_memory();
        let (mut injected, mut record) =
            kura_context::assemble_framed(&candidates, self.config.bootstrap_budget(), frame);
        // Everything after this many is query-time recall (below).
        let bootstrapped = injected.len();
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
            // The corpus is L1 atoms **plus** L2/L3. Bootstrap injects L2/L3
            // newest-first under its own budget, so a scenario or persona that
            // did not fit — or simply is not recent — was previously
            // unreachable no matter how well the query matched it. Retrieval
            // is the second chance.
            let mut atoms = Self::bootstrap_layer(
                memory,
                &tenant_id,
                &agent_profile_id,
                kura_memory::MemoryLayer::L1,
                &mut atom_excluded,
            );
            for layer in [kura_memory::MemoryLayer::L2, kura_memory::MemoryLayer::L3] {
                atoms.extend(Self::bootstrap_layer(
                    memory,
                    &tenant_id,
                    &agent_profile_id,
                    layer,
                    &mut atom_excluded,
                ));
            }
            // Anything bootstrap already injected must not be injected twice.
            // Recorded rather than silently dropped: "it was already in the
            // window" is a different fact from "it did not match".
            let already_injected: std::collections::HashSet<String> = record
                .included
                .iter()
                .map(|item| item.asset_id.clone())
                .collect();
            atoms.retain(|asset| {
                if already_injected.contains(&asset.asset_id) {
                    record.excluded.push(kura_context::ExcludedItem {
                        asset_id: asset.asset_id.clone(),
                        layer: asset.layer.clone(),
                        reason: "already_injected".to_string(),
                        source: "retrieval".to_string(),
                    });
                    false
                } else {
                    true
                }
            });
            // Visibility-excluded assets are only recorded when retrieval
            // actually runs (they were candidates for this query's corpus).
            record
                .excluded
                .extend(atom_excluded.into_iter().map(|mut item| {
                    item.source = "retrieval".to_string();
                    item
                }));
            // Bound the corpus before any scoring: it is the tenant's whole
            // Ready-atom set, it grows without limit, and everything below is
            // linear in it. Newest first (bootstrap_layer already sorted), so
            // the bound drops the oldest — and says so.
            let max_corpus = self.config.max_corpus();
            if atoms.len() > max_corpus {
                for asset in atoms.split_off(max_corpus) {
                    record.excluded.push(kura_context::ExcludedItem {
                        asset_id: asset.asset_id,
                        layer: asset.layer,
                        reason: "over_candidate_limit".to_string(),
                        source: "retrieval".to_string(),
                    });
                }
            }
            let docs: Vec<kura_context::RetrievalDoc> = atoms
                .into_iter()
                .map(|asset| kura_context::RetrievalDoc {
                    asset_id: asset.asset_id,
                    layer: asset.layer,
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
            // Corpus vectors come from the persisted derived index, so this
            // turn embeds exactly one text: the query. Misses are computed
            // once and written back. Before this, every turn re-embedded the
            // whole corpus — one RPC per atom per turn for an external
            // provider, synchronously on the reply path.
            let doc_vectors = Self::corpus_vectors(state, embedder, &docs);
            kura_context::retrieve_and_assemble_framed(
                &query,
                &docs,
                Some(embedder),
                Some(&doc_vectors),
                self.config.recall_budget(),
                self.config.min_similarity(),
                frame,
                &mut injected,
                &mut record,
            );
        }

        if record.is_empty() {
            return kura_plugin::HookOutcome::Continue;
        }

        // Two places, so the prompt cache (2.3) reaches across turns:
        // - the bootstrap is the same turn after turn, so it joins the frame
        //   (persona, skills), in front of the history;
        // - recall depends on this question, so it goes right before it,
        //   after the history. In front of the history it changed the prefix
        //   every turn, and nothing after it could be read from the cache.
        // The notice goes once, before whichever comes first.
        let mut recalled = injected.split_off(bootstrapped);
        let mut bootstrap = injected;
        if frame {
            if !bootstrap.is_empty() {
                bootstrap.insert(0, kura_context::memory_notice_message());
            } else if !recalled.is_empty() {
                recalled.insert(0, kura_context::memory_notice_message());
            }
        }
        if let Some(messages) = payload.get_mut("messages").and_then(Value::as_array_mut) {
            let as_json = |message: &kura_context::ContextMessage| serde_json::json!({ "role": message.role, "content": message.content });
            let frame_end = messages
                .iter()
                .position(|m| m.get("role").and_then(Value::as_str) != Some("system"))
                .unwrap_or(messages.len());
            messages.splice(frame_end..frame_end, bootstrap.iter().map(as_json));
            let question_at = messages
                .iter()
                .rposition(|m| m.get("role").and_then(Value::as_str) == Some("user"))
                .unwrap_or(messages.len());
            messages.splice(question_at..question_at, recalled.iter().map(as_json));
        }

        // Inspectability: the assembly decision is an event (best effort).
        publish_context_assembly(
            state,
            &tenant_id,
            payload.get("threadId").and_then(Value::as_str),
            &record,
        );
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
        Arc::new(ContextHook {
            state: asm.late_state.clone(),
            config,
        }),
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
    /// Set when `summaryBudgetTokens` is configured (2.5).
    summarizer: Option<Arc<dyn kura_session::summary::Summarizer>>,
    summary_jobs: Arc<SummaryJobs>,
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
        let Ok(mut messages) =
            serde_json::from_value::<Vec<kura_session::WindowMessage>>(messages_value.clone())
        else {
            return kura_plugin::HookOutcome::Continue;
        };

        // Stage 5.1: the thread's session frame goes in first, as a system
        // message, so the goal survives whatever elision follows and is the
        // first thing the model reads.
        let mut frame_applied = false;
        {
            let thread_id = payload
                .get("threadId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim();
            let tenant_id = payload
                .get("tenantId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim();
            if !thread_id.is_empty() {
                if let Some(state) = self.state.get() {
                    if let Ok(Some(frame)) =
                        kura_api::routes::session_frames::load_frame(state, tenant_id, thread_id)
                    {
                        if !frame.is_empty() {
                            kura_session::apply_frame(&mut messages, &frame);
                            frame_applied = true;
                        }
                    }
                }
            }
        }

        let model = payload
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let mut shaped = shape_by_unit(
            &self.config,
            &messages,
            source_kind.as_deref(),
            &model,
            None,
        );
        if shaped.elided > 0 {
            if let (Some(summarizer), Some(state)) = (&self.summarizer, self.state.get()) {
                self.place_summary(
                    state,
                    summarizer,
                    payload,
                    &messages,
                    source_kind.as_deref(),
                    &model,
                    &mut shaped,
                );
            }
        }
        if shaped.elided == 0 {
            if frame_applied {
                if let Ok(new_messages) = serde_json::to_value(&shaped.messages) {
                    payload["messages"] = new_messages;
                }
            }
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
                    // With a summary covering the whole span there is no
                    // marker; the summary carries the citation instead.
                    let marker_at = shaped
                        .messages
                        .iter()
                        .position(|m| m.content.contains("elided by the session-strategy"))
                        .or_else(|| {
                            shaped.messages.iter().position(|m| {
                                m.content
                                    .starts_with(kura_session::summary::SUMMARY_MESSAGE_PREFIX)
                            })
                        });
                    if let Some(marker) = marker_at.map(|at| &mut shaped.messages[at]) {
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
                                eprintln!("memory: eviction-trigger consolidation failed: {err:?}");
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

impl SessionStrategyHook {
    /// 2.5: puts the thread's summary in place of the elided span and, when
    /// enough uncovered messages have piled up, folds them in off the reply
    /// path. Every failure leaves `shaped` as the plain marker window.
    #[allow(clippy::too_many_arguments)]
    fn place_summary(
        &self,
        state: &AppState,
        summarizer: &Arc<dyn kura_session::summary::Summarizer>,
        payload: &serde_json::Value,
        messages: &[kura_session::WindowMessage],
        source_kind: Option<&str>,
        model: &str,
        shaped: &mut kura_session::ShapedWindow,
    ) {
        use serde_json::Value;
        let field = |name: &str| {
            payload
                .get(name)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string()
        };
        let (tenant_id, thread_id) = (field("tenantId"), field("threadId"));
        if thread_id.is_empty() {
            return;
        }
        // A summary belongs to one session segment: after a reset (manual
        // or idle-gap segmentation) the old conversation's summary must not
        // leak into the new one.
        let segment_id = state
            .store_pool
            .read()
            .get_thread_for_tenant(&tenant_id, &thread_id)
            .ok()
            .flatten()
            .map(|thread| thread.current_session_segment_id)
            .unwrap_or_default();
        let existing = load_session_summary(state, &tenant_id, &thread_id)
            .filter(|summary| summary.segment_id == segment_id);

        // The summary is part of the window, so the history makes room for
        // it rather than the total overflowing the budget.
        if let Some(summary) = &existing {
            let reserve = kura_session::WindowMessage {
                role: "system".to_string(),
                content: kura_session::summary::render_summary_message(&summary.summary),
            };
            *shaped = shape_by_unit(&self.config, messages, source_kind, model, Some(&reserve));
        }
        let placement = kura_session::summary::place_summary(
            messages,
            shaped,
            existing.as_ref().map(|s| s.summary.as_str()),
            existing.as_ref().map_or("", |s| s.watermark.as_str()),
            self.config.summary_refresh_floor(),
        );
        let coverage = if placement.uncovered == 0 {
            "full"
        } else if placement.covered > 0 {
            "partial"
        } else {
            "none"
        };
        kura_telemetry::metrics::registry().inc(
            kura_telemetry::metrics::SESSION_SUMMARY_WINDOWS_TOTAL,
            &[("coverage", coverage)],
        );
        if placement.summary_injected {
            if let Some(message) = placement.messages.iter().find(|m| {
                m.content
                    .starts_with(kura_session::summary::SUMMARY_MESSAGE_PREFIX)
            }) {
                let tokens = kura_session::estimate_message_tokens(message);
                let record = kura_context::AssemblyRecord {
                    included: vec![kura_context::IncludedItem {
                        asset_id: format!(
                            "{}/{thread_id}",
                            kura_session::summary::DOC_KIND_SESSION_SUMMARY
                        ),
                        layer: "summary".to_string(),
                        chars: message.content.len(),
                        tokens,
                        source: "session_summary".to_string(),
                    }],
                    budget_unit: kura_context::BudgetUnit::Tokens,
                    budget_tokens: self.config.summary_budget_tokens,
                    used_tokens: tokens,
                    budget_chars: self.config.summary_budget_tokens * kura_session::CHARS_PER_TOKEN,
                    used_chars: message.content.len(),
                    ..kura_context::AssemblyRecord::default()
                };
                publish_context_assembly(state, &tenant_id, Some(&thread_id), &record);
            }
        }
        shaped.messages = placement.messages;
        if let Some(update) = placement.update {
            self.spawn_summary_update(
                state, summarizer, tenant_id, thread_id, segment_id, existing, update,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_summary_update(
        &self,
        state: &AppState,
        summarizer: &Arc<dyn kura_session::summary::Summarizer>,
        tenant_id: String,
        thread_id: String,
        segment_id: String,
        previous: Option<kura_session::summary::SessionSummary>,
        update: kura_session::summary::SummaryUpdate,
    ) {
        let count = |outcome: &str| {
            kura_telemetry::metrics::registry().inc(
                kura_telemetry::metrics::SESSION_SUMMARY_UPDATES_TOTAL,
                &[("outcome", outcome)],
            );
        };
        let job = match self.summary_jobs.begin(&format!("{tenant_id}/{thread_id}")) {
            Ok(job) => job,
            Err(outcome) => {
                count(outcome);
                return;
            }
        };
        let budget = self.config.summary_budget_tokens;
        let state = state.clone();
        let summarizer = Arc::clone(summarizer);
        let spawned = std::thread::Builder::new()
            .name("session-summary".to_string())
            .spawn(move || {
                let previous_text = previous.as_ref().map_or("", |p| p.summary.as_str());
                let outcome =
                    match summarizer.summarize(&tenant_id, previous_text, &update.batch, budget) {
                        Ok(text) => {
                            let text = kura_session::summary::fit_summary(&text, budget);
                            if text.is_empty() {
                                "empty"
                            } else {
                                let summary = kura_session::summary::SessionSummary {
                                    thread_id: thread_id.clone(),
                                    tenant_id: tenant_id.clone(),
                                    segment_id,
                                    summary: text,
                                    watermark: update.watermark,
                                    covered_messages: previous
                                        .as_ref()
                                        .map_or(0, |p| p.covered_messages)
                                        + update.batch.len(),
                                    updated_at: chrono::Utc::now(),
                                };
                                let stored = kura_store::put_document(
                                    &state.store.lock(),
                                    kura_session::summary::DOC_KIND_SESSION_SUMMARY,
                                    &thread_id,
                                    "",
                                    &tenant_id,
                                    &summary,
                                );
                                match stored {
                                    Ok(()) => "written",
                                    Err(err) => {
                                        kura_telemetry::global_logger().warn(&format!(
                                            "session.summary store failed thread={thread_id}: {err}"
                                        ));
                                        "failed"
                                    }
                                }
                            }
                        }
                        Err(err) => {
                            kura_telemetry::global_logger().warn(&format!(
                                "session.summary update failed thread={thread_id}: {err}"
                            ));
                            "failed"
                        }
                    };
                job.finish(outcome == "written");
                count(outcome);
            });
        if let Err(err) = spawned {
            // The closure (and the job it owns) is dropped, which releases
            // the slot; the next dispatch tries again.
            kura_telemetry::global_logger().warn(&format!("session.summary spawn failed: {err}"));
        }
    }
}

/// A thread's summary, if one exists under this tenant.
fn load_session_summary(
    state: &AppState,
    tenant_id: &str,
    thread_id: &str,
) -> Option<kura_session::summary::SessionSummary> {
    let doc = state
        .store_pool
        .read()
        .get_manager_document(kura_session::summary::DOC_KIND_SESSION_SUMMARY, thread_id)
        .ok()
        .flatten()?;
    // Summaries are new with 2.5, so there are no pre-tenancy rows to admit:
    // the owner must match exactly.
    if doc.tenant_id != tenant_id {
        return None;
    }
    serde_json::from_str(&doc.document_json).ok()
}

/// Background summary updates (2.5): one in flight per thread, a small cap
/// overall, and a pause after a failure, so a provider outage costs one call
/// per thread per pause rather than one per tool round.
#[derive(Default)]
struct SummaryJobs {
    inner: Arc<parking_lot::Mutex<SummaryJobsInner>>,
}

#[derive(Default)]
struct SummaryJobsInner {
    inflight: std::collections::HashSet<String>,
    failed_at: std::collections::HashMap<String, std::time::Instant>,
}

/// Summary updates running at once across all threads.
const SUMMARY_MAX_INFLIGHT: usize = 4;
/// Pause after a failed update before the thread is tried again.
const SUMMARY_RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(300);

impl SummaryJobs {
    /// Claims the thread's slot, or says why not (`busy` | `backoff`).
    fn begin(&self, key: &str) -> Result<SummaryJob, &'static str> {
        let mut inner = self.inner.lock();
        if inner.inflight.contains(key) || inner.inflight.len() >= SUMMARY_MAX_INFLIGHT {
            return Err("busy");
        }
        if inner
            .failed_at
            .get(key)
            .is_some_and(|at| at.elapsed() < SUMMARY_RETRY_AFTER)
        {
            return Err("backoff");
        }
        inner.inflight.insert(key.to_string());
        Ok(SummaryJob {
            key: key.to_string(),
            jobs: Arc::clone(&self.inner),
        })
    }
}

/// A claimed slot; released on drop, so a panicking update cannot wedge
/// the thread.
struct SummaryJob {
    key: String,
    jobs: Arc<parking_lot::Mutex<SummaryJobsInner>>,
}

impl SummaryJob {
    fn finish(&self, ok: bool) {
        let mut inner = self.jobs.lock();
        if ok {
            inner.failed_at.remove(&self.key);
        } else {
            inner
                .failed_at
                .insert(self.key.clone(), std::time::Instant::now());
        }
    }
}

impl Drop for SummaryJob {
    fn drop(&mut self) {
        self.jobs.lock().inflight.remove(&self.key);
    }
}

/// Records one assembly decision as a `context.assembled` event (best
/// effort), read back through `/v1/context/assemblies`.
fn publish_context_assembly(
    state: &AppState,
    tenant_id: &str,
    thread_id: Option<&str>,
    record: &kura_context::AssemblyRecord,
) {
    use serde_json::Value;
    let mut event_payload = serde_json::Map::new();
    event_payload.insert(
        "record".to_string(),
        serde_json::to_value(record).unwrap_or(Value::Null),
    );
    event_payload.insert("tenantId".to_string(), Value::String(tenant_id.to_string()));
    if let Some(thread_id) = thread_id {
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
    let event = state.store.lock().append_event(&event).unwrap_or(event);
    state.event_bus.publish(event);
}

/// Shapes the window in the configured unit (agent-runtime-program.md 2.2).
/// Shadow mode shapes by chars, as before, and counts how a token budget
/// would have differed, so the switch is made on measured traffic.
///
/// `reserve` is a message the caller will add to the window (the 2.5
/// summary); its cost comes out of the budget so the total still fits.
fn shape_by_unit(
    config: &kura_session::SessionStrategyConfig,
    messages: &[kura_session::WindowMessage],
    source_kind: Option<&str>,
    model: &str,
    reserve: Option<&kura_session::WindowMessage>,
) -> kura_session::ShapedWindow {
    let keep_recent = config.keep_recent_floor();
    let reserve_chars = reserve.map_or(0, |m| m.content.len());
    let reserve_tokens = reserve.map_or(0, kura_session::estimate_message_tokens);
    let by_chars = || {
        kura_session::shape_window(
            messages,
            config.budget_for(source_kind).saturating_sub(reserve_chars),
            keep_recent,
        )
    };
    let by_tokens = || {
        kura_session::shape_window_tokens(
            messages,
            config
                .token_budget_for(source_kind, model)
                .saturating_sub(reserve_tokens),
            keep_recent,
        )
    };
    match config.budget_unit {
        kura_session::BudgetUnit::Chars => by_chars(),
        kura_session::BudgetUnit::Tokens => by_tokens(),
        // The 2.5 re-shape (with a reserve) is the same dispatch again;
        // counting it would weigh summarized windows twice.
        kura_session::BudgetUnit::Shadow if reserve.is_some() => by_chars(),
        kura_session::BudgetUnit::Shadow => {
            let chars = by_chars();
            let tokens = by_tokens();
            let agreement = match tokens.elided.cmp(&chars.elided) {
                std::cmp::Ordering::Equal => "same",
                std::cmp::Ordering::Greater => "tokens_elide_more",
                std::cmp::Ordering::Less => "tokens_elide_fewer",
            };
            kura_telemetry::metrics::registry().inc(
                kura_telemetry::metrics::SESSION_WINDOW_SHADOW_TOTAL,
                &[("agreement", agreement)],
            );
            if agreement != "same" {
                kura_telemetry::global_logger().info(&format!(
                    "session.window.shadow model={model} chars_elided={} tokens_elided={} \
                     char_budget={} token_budget={}",
                    chars.elided,
                    tokens.elided,
                    config.budget_for(source_kind),
                    config.token_budget_for(source_kind, model),
                ));
            }
            chars
        }
    }
}

/// Stage 5.2: channel-native thread segmentation, evaluated at
/// `chat/turn-start` — before continuity is assembled, so a freshly opened
/// segment is what the turn sees. The mechanism is the threads engine's own
/// `reset_thread` (through `apply_thread_lifecycle_action`), so an automatic
/// segment boundary leaves the same audited lifecycle action and reset
/// evidence as an operator-requested reset.
struct SessionSegmentationHook {
    config: kura_session::SessionStrategyConfig,
    state: Arc<std::sync::OnceLock<AppState>>,
}

impl kura_plugin::Hook for SessionSegmentationHook {
    fn handle(&self, payload: &mut serde_json::Value) -> kura_plugin::HookOutcome {
        use serde_json::Value;
        if payload.get("sourceKind").and_then(Value::as_str) != Some("channel") {
            return kura_plugin::HookOutcome::Continue;
        }
        let channel_scope_ref = payload
            .get("channelScopeRef")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(policy) = self.config.segmentation_for(channel_scope_ref).cloned() else {
            return kura_plugin::HookOutcome::Continue;
        };
        let tenant_id = payload
            .get("tenantId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        let thread_id = payload
            .get("threadId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if thread_id.is_empty() {
            return kura_plugin::HookOutcome::Continue;
        }
        let Some(state) = self.state.get() else {
            return kura_plugin::HookOutcome::Continue;
        };
        let now = payload
            .get("sourceTimestamp")
            .and_then(Value::as_str)
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.with_timezone(&chrono::Utc))
            .unwrap_or_else(chrono::Utc::now);

        let (segment_id, last_turn_at) = {
            let store = state.store.lock();
            let Ok(Some(thread)) = store.get_thread_for_tenant(&tenant_id, &thread_id) else {
                return kura_plugin::HookOutcome::Continue;
            };
            let segment_id = thread.current_session_segment_id.clone();
            if segment_id.trim().is_empty() {
                return kura_plugin::HookOutcome::Continue;
            }
            let last = store
                .list_continuity_turns(&kura_store::thread_continuity::ContinuityLookupQuery {
                    tenant_id: tenant_id.clone(),
                    thread_id: thread_id.clone(),
                    session_segment_id: segment_id.clone(),
                    limit: 1,
                    now: Some(now),
                })
                .ok()
                .and_then(|turns| turns.into_iter().next())
                .map(|turn| turn.recorded_at);
            (segment_id, last)
        };
        if !kura_session::segment_boundary_due(&policy, last_turn_at, now) {
            return kura_plugin::HookOutcome::Continue;
        }

        let input = kura_threads::LifecycleMutationInput {
            actor_principal_id: "session-strategy".to_string(),
            reason_code: "idle_gap_segmentation".to_string(),
            audit_event_id: format!(
                "audit_thread_segment_{}",
                now.timestamp_nanos_opt().unwrap_or_default()
            ),
            now: Some(now),
            new_segment_id: String::new(),
        };
        let result = state.store.lock().apply_thread_lifecycle_action(
            &tenant_id,
            &thread_id,
            kura_threads::LifecycleActionKind::Reset,
            &input,
        );
        match result {
            Ok(Some(result)) => {
                let _ = kura_api::routes::resources::publish_thread_event(
                    state,
                    &tenant_id,
                    kura_events::thread_lifecycle_event(result.action.clone()),
                );
                eprintln!(
                    "[kura] session-strategy: thread {thread_id} idle past {}s; opened segment {} (was {segment_id})",
                    policy.idle_gap_seconds, result.thread.current_session_segment_id
                );
            }
            Ok(None) => {}
            Err(err) => {
                eprintln!("[kura] session-strategy: segment reset for {thread_id} failed: {err}")
            }
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
        serde_json::from_value(serde_json::Value::Object(config_object))
            .map_err(|err| AppError::PluginProfile(format!("session-strategy config: {err}")))?;
    bus.register(
        kura_plugin::points::CHAT_TURN_START,
        "session-strategy",
        Arc::new(SessionSegmentationHook {
            config: config.clone(),
            state: asm.late_state.clone(),
        }),
    );
    let summarizer: Option<Arc<dyn kura_session::summary::Summarizer>> = if config.summary_enabled()
    {
        let llm = asm.state.llm.clone().expect("llm plugin built");
        Some(Arc::new(adapters::LlmSessionSummarizer::new(llm)))
    } else {
        None
    };
    bus.register(
        kura_plugin::points::CHAT_PRE_DISPATCH,
        "session-strategy",
        Arc::new(SessionStrategyHook {
            config,
            state: asm.late_state.clone(),
            summarizer,
            summary_jobs: Arc::new(SummaryJobs::default()),
        }),
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
    let activation_store =
        Arc::new(SqliteActivationStore::new(asm.open_store()?).map_err(AppError::Store)?);
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
    // Stage 9.4: `entries.computer-use.config.driver = "subprocess"` swaps
    // the in-memory driver for a supervised worker process
    // (`capabilities/browser/PROTOCOL.md`). The manager, policy gating and
    // artifact recorder do not change.
    let config_object = asm.profile.config_for("computer-use");
    let config: ComputerUsePluginConfig =
        serde_json::from_value(serde_json::Value::Object(config_object))
            .map_err(|err| AppError::PluginProfile(format!("computer-use config: {err}")))?;
    let driver: Option<Arc<dyn kura_computeruse::Driver>> = match config.driver.as_str() {
        "" | "memory" => None,
        "subprocess" => {
            if config.command.trim().is_empty() {
                return Err(AppError::PluginProfile(
                    "computer-use config: driver \"subprocess\" needs `command`".to_string(),
                ));
            }
            let capability_id = kura_computeruse::subprocess_driver::DEFAULT_CAPABILITY_ID;
            let supervisor: Option<Arc<dyn kura_computeruse::WorkerSupervisor>> =
                asm.state.capabilities.clone().map(|sup| {
                    let _ = sup.register(kura_capabilities::RegisterInput {
                        capability_id: capability_id.to_string(),
                        kind: kura_computeruse::subprocess_driver::KIND_BROWSER_WORKER.to_string(),
                        display_name: "browser worker".to_string(),
                    });
                    Arc::new(crate::adapters::BrowserWorkerSupervisor(sup))
                        as Arc<dyn kura_computeruse::WorkerSupervisor>
                });
            Some(Arc::new(kura_computeruse::SubprocessDriver::new(
                kura_computeruse::SubprocessDriverConfig {
                    command: config.command.clone(),
                    args: config.args.clone(),
                    env: config
                        .env
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                    timeout_ms: if config.timeout_ms == 0 {
                        30_000
                    } else {
                        config.timeout_ms
                    },
                    capability_id: capability_id.to_string(),
                },
                supervisor,
            )))
        }
        other => {
            return Err(AppError::PluginProfile(format!(
                "computer-use config: unknown driver \"{other}\" (memory | subprocess)"
            )));
        }
    };
    asm.state.computer_use = Some(Arc::new(ComputerUseManager::new(ComputerUseDependencies {
        environment_scope: asm.env_scope.to_string(),
        runtime: asm.state.runtime.clone(),
        policy: asm.state.policy.clone(),
        store: computeruse_store,
        driver,
        artifacts: Some(computeruse_recorder.clone()),
    })));
    #[cfg(test)]
    {
        asm.wiring.computeruse_recorder = Some(computeruse_recorder);
    }
    build_tenant_browsers(asm);
    // Remote computer use (docs/harness/remote-computer-use.md): the hub
    // only holds connections the users' devices open; nothing reaches a
    // device until a tool asks for a session and the user allows it.
    asm.state.device_hub = Some(Arc::new(kura_devices::Hub::default()));
    asm.state.computer_remote = remote_computer_gate(&config.remote, &asm.state.hosting)?;
    match &asm.state.computer_remote {
        kura_devices::RemoteGate::Off => eprintln!("[kura] computer: remote tools disabled"),
        kura_devices::RemoteGate::AllTenants => {
            eprintln!("[kura] computer: remote tools enabled");
        }
        kura_devices::RemoteGate::Tenants(tenants) => eprintln!(
            "[kura] computer: remote tools enabled for {} tenant(s)",
            tenants.len()
        ),
    }
    Ok(())
}

/// 3.3 (`docs/harness/tenant-browser.md`): the tenants' throwaway browsers,
/// offered to turns as `browser.*` tools. Needs the worker image
/// (`KURA_BROWSER_WORKER_IMAGE`) and the tenant egress path: a browser with
/// no network is no use, and the sandbox refuses network without it. Any
/// missing piece leaves the tools off and says which.
fn build_tenant_browsers(asm: &mut Assembly) {
    let read = |key: &str| {
        std::env::var(key)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };
    let sandboxes = asm.state.sandboxes.clone();
    let egress = sandboxes
        .as_ref()
        .is_some_and(|s| s.tenant_egress_configured());
    let (image, limits) = match browser_settings(&read, sandboxes.is_some(), egress) {
        Ok(Some(settings)) => settings,
        Ok(None) => return,
        Err(reason) => {
            eprintln!("[kura] browser: off: {reason}");
            return;
        }
    };
    let sandboxes = sandboxes.expect("checked by browser_settings");
    let container = kura_sandbox::ContainerSpec {
        image: image.clone(),
        limits: kura_sandbox::IsolationLimits {
            cpus: 1.0,
            memory_mb: 1024,
            pids: 512,
            tmpfs_mb: 256,
        },
    };
    let launcher = Arc::new(crate::adapters::SandboxBrowserLauncher::new(
        sandboxes, container,
    ));
    let browsers = kura_computeruse::TenantBrowsers::new(launcher, limits);
    if let Err(err) = browsers.spawn_reaper() {
        eprintln!("[kura] browser: off: cannot start the idle reaper: {err}");
        return;
    }
    eprintln!(
        "[kura] browser: tenant browsers from {image}: up to {} at once, closed after {}s idle",
        limits.max_sessions,
        limits.idle.as_secs()
    );
    asm.state.tenant_browsers = Some(browsers);
    asm.lifecycle.on_close(
        "tenant-browsers",
        Box::new(|state| {
            if let Some(browsers) = &state.tenant_browsers {
                browsers.close_all();
            }
        }),
    );
}

/// The tenant browser's image and limits, `None` when it is not asked for,
/// or why it cannot be offered.
fn browser_settings(
    read: &dyn Fn(&str) -> Option<String>,
    has_sandbox: bool,
    egress_configured: bool,
) -> Result<Option<(String, kura_computeruse::BrowserLimits)>, &'static str> {
    let Some(image) = read("KURA_BROWSER_WORKER_IMAGE") else {
        return Ok(None);
    };
    if !has_sandbox {
        return Err("the sandbox plane is not built");
    }
    if !egress_configured {
        return Err(
            "tenant egress is not configured (KURA_SANDBOX_TENANT_NETWORK + KURA_SANDBOX_EGRESS_PROXY)",
        );
    }
    let number = |key: &str, default: u64| {
        read(key)
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(default)
    };
    Ok(Some((
        image,
        kura_computeruse::BrowserLimits {
            max_sessions: number("KURA_BROWSER_MAX_SESSIONS", DEFAULT_BROWSER_SESSIONS) as usize,
            idle: Duration::from_secs(number(
                "KURA_BROWSER_IDLE_SECONDS",
                DEFAULT_BROWSER_IDLE_SECS,
            )),
        },
    )))
}

#[cfg(test)]
mod browser_settings_tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |key| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    #[test]
    fn the_browser_is_offered_only_with_an_image_a_sandbox_and_egress() {
        assert_eq!(browser_settings(&env(&[]), true, true), Ok(None));
        let image = [(
            "KURA_BROWSER_WORKER_IMAGE",
            "ghcr.io/x/kura-browser-worker@sha256:1",
        )];
        assert!(browser_settings(&env(&image), false, true).is_err());
        assert!(
            browser_settings(&env(&image), true, false)
                .unwrap_err()
                .contains("egress")
        );
        let (got, limits) = browser_settings(&env(&image), true, true).unwrap().unwrap();
        assert_eq!(got, image[0].1);
        assert_eq!(limits.max_sessions, 4);
        assert_eq!(limits.idle, Duration::from_secs(600));
    }

    #[test]
    fn limits_can_be_set_and_nonsense_keeps_the_default() {
        let settings = |extra: &[(&str, &str)]| {
            let mut pairs = vec![("KURA_BROWSER_WORKER_IMAGE", "img")];
            pairs.extend_from_slice(extra);
            browser_settings(&env(&pairs), true, true)
                .unwrap()
                .unwrap()
                .1
        };
        let set = settings(&[
            ("KURA_BROWSER_MAX_SESSIONS", "2"),
            ("KURA_BROWSER_IDLE_SECONDS", "60"),
        ]);
        assert_eq!((set.max_sessions, set.idle), (2, Duration::from_secs(60)));
        let bad = settings(&[
            ("KURA_BROWSER_MAX_SESSIONS", "0"),
            ("KURA_BROWSER_IDLE_SECONDS", "soon"),
        ]);
        assert_eq!((bad.max_sessions, bad.idle), (4, Duration::from_secs(600)));
    }
}

/// Measured on the cell host: 160-210 MiB per idle session (tenant-browser.md §3).
const DEFAULT_BROWSER_SESSIONS: u64 = 4;
const DEFAULT_BROWSER_IDLE_SECS: u64 = 600;

/// `plugins.json` → `entries.computer-use.config`.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct ComputerUsePluginConfig {
    /// `memory` (default) or `subprocess`.
    driver: String,
    command: String,
    args: Vec<String>,
    env: std::collections::BTreeMap<String, String>,
    timeout_ms: u64,
    /// Remote computer use: the `computer.*` tools on the users' own
    /// computers (docs/harness/remote-computer-use.md §8).
    remote: RemoteComputerConfig,
}

/// `entries.computer-use.config.remote`.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
struct RemoteComputerConfig {
    enabled: bool,
    /// The tenants offered the tools. Required on a shared daemon, where
    /// every tenant at once is never the default; on a single-user daemon
    /// empty means its one tenant.
    tenants: Vec<String>,
}

/// The gate `computer.*` is offered through. A shared daemon enabled with
/// no tenants named fails the boot rather than open it to everyone.
fn remote_computer_gate(
    config: &RemoteComputerConfig,
    hosting: &kura_api::hosting::Hosting,
) -> Result<kura_devices::RemoteGate, AppError> {
    if !config.enabled {
        return Ok(kura_devices::RemoteGate::Off);
    }
    let tenants: std::collections::BTreeSet<String> = config
        .tenants
        .iter()
        .map(|tenant| tenant.trim().to_string())
        .filter(|tenant| !tenant.is_empty())
        .collect();
    if !tenants.is_empty() {
        return Ok(kura_devices::RemoteGate::Tenants(tenants));
    }
    if hosting.is_shared() {
        return Err(AppError::PluginProfile(
            "computer-use config: remote.enabled on a shared daemon needs remote.tenants".into(),
        ));
    }
    Ok(kura_devices::RemoteGate::AllTenants)
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
    let workflow_launcher = asm
        .seams
        .get::<WorkflowLauncherSeam>()
        .expect("kernel launcher")
        .0;
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
    let workflow_launcher = asm
        .seams
        .get::<WorkflowLauncherSeam>()
        .expect("kernel launcher")
        .0;
    asm.state.reminders = Some(Arc::new(RemindersManager::new(RemindersDependencies {
        environment_scope: asm.env_scope.to_string(),
        store: asm.store.clone(),
        event_bus: Some((*asm.event_bus).clone()),
        delivery: asm
            .state
            .delivery
            .as_ref()
            .map(|delivery| (**delivery).clone()),
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
            payload
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
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
        let text = format!(
            "user: {}\nassistant: {}",
            text_of("query").trim(),
            text_of("output")
        );
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
            Arc::new(MemoryCaptureHook {
                state: asm.late_state.clone(),
            }),
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
    let workflow_launcher = asm
        .seams
        .get::<WorkflowLauncherSeam>()
        .expect("kernel launcher")
        .0;
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
        Some(Box::new(adapters::CatalogSandboxRequirementChecker::new(
            sandboxes,
        ))),
        Some(Box::new(adapters::CatalogTenantPermissionGate::new(
            asm.store.clone(),
        ))),
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
        Some(Box::new(
            adapters::ExecProfileSandboxRequirementChecker::new(sandboxes),
        )),
        Some(Box::new(adapters::ExecProfileTenantPermissionGate::new(
            asm.store.clone(),
        ))),
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
        Some(Box::new(adapters::EvidenceSupportPermissionGate::new(
            asm.store.clone(),
        ))),
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

/// Tool provider profiles. Restored from the store at build time so a
/// configured search provider survives a restart; credentials are not restored
/// because they were never here — a profile holds only a `secretRef`.
fn build_tools(asm: &mut Assembly) -> Result<(), AppError> {
    let manager = kura_tools::Manager::new();
    match asm.store.lock().list_all_tool_profiles() {
        Ok(profiles) => manager.restore(profiles),
        // A corrupt profile row must not brick the boot: the daemon starts
        // with no tool providers, which is the same state as a fresh install
        // and is visible through /v1/tools/capabilities.
        Err(err) => {
            eprintln!("[kura] tools: restoring profiles failed ({err}); starting with none")
        }
    }
    asm.state.tools = Some(Arc::new(manager));

    // The guarded call path: quota reserve -> egress check -> provider ->
    // commit/release. Built here with the real billing gate so no family can
    // be invoked outside it.
    let billing = asm.state.billing.clone().expect("billing plugin built");
    asm.state.tool_runtime = Some(Arc::new(kura_tools::ToolRuntime::new(
        Arc::new(crate::adapters::ToolQuotaGateImpl::new(billing)),
        asm.cfg.egress.clone(),
    )));
    Ok(())
}

/// Swarm runs are restored from the store at boot; anything a restart
/// interrupted is marked cancelled rather than left pending forever.
fn build_swarm(asm: &mut Assembly) -> Result<(), AppError> {
    let config_object = asm.profile.config_for("swarm");
    let config: kura_swarm::SwarmConfig =
        serde_json::from_value(serde_json::Value::Object(config_object))
            .map_err(|err| AppError::PluginProfile(format!("swarm config: {err}")))?;
    let manager = kura_swarm::Manager::new(config);
    match kura_store::list_documents::<kura_swarm::SwarmRun>(
        &asm.store.lock(),
        kura_swarm::DOC_KIND_SWARM_RUN,
    ) {
        Ok(runs) => manager.restore(runs),
        Err(err) => eprintln!("[kura] swarm: restoring runs failed ({err}); starting with none"),
    }
    let billing = asm.state.billing.clone().expect("billing plugin built");
    asm.state.swarm_quota = Some(Arc::new(crate::adapters::SwarmQuotaGateImpl::new(billing)));
    asm.state.swarm = Some(Arc::new(manager));
    Ok(())
}

/// Work items (docs/harness/work-items.md). Off unless the profile sets
/// `entries.work-items.config.enabled`: otherwise nothing is built, every
/// work-item route answers 404, no chat turn is offered `work_item.open`
/// and the IM loops get no hook, exactly as before the feature existed.
fn build_work_items(asm: &mut Assembly) -> Result<(), AppError> {
    let Some(config) = work_items_config(asm.profile.config_for("work-items"))? else {
        eprintln!("[kura] work items: disabled");
        return Ok(());
    };
    eprintln!(
        "[kura] work items: enabled (maxOpenItems {}, subscriptionIdleDays {}, \
         maxRoundsPerItemPerHour {})",
        config.max_open_items, config.subscription_idle_days, config.max_rounds_per_item_per_hour
    );
    let chat = asm
        .state
        .chat
        .clone()
        .expect("chat plugin built (required)");
    asm.state.work_items = Some(Arc::new(
        kura_workitems::Manager::new(asm.state.store_pool.clone(), asm.event_bus.clone(), config)
            .with_chat(chat),
    ));
    // The outbox sweeper (slice 3): its first pass is the boot sweep of
    // replies a previous run left unsent. Connector senders register when
    // the runtimes are built (`App::serve`).
    asm.lifecycle.on_start(
        "work-items",
        Box::new(|state| {
            if let Some(work_items) = &state.work_items {
                if let Err(err) = work_items.start_outbox_sweeper() {
                    eprintln!("[kura] work item: starting the outbox sweeper failed: {err}");
                }
            }
        }),
    );
    asm.lifecycle.on_close(
        "work-items",
        Box::new(|state| {
            if let Some(work_items) = &state.work_items {
                work_items.stop_outbox_sweeper();
            }
        }),
    );
    Ok(())
}

#[cfg(test)]
mod remote_computer_gate_tests {
    use super::{RemoteComputerConfig, remote_computer_gate};
    use kura_api::hosting::Hosting;
    use kura_devices::RemoteGate;

    fn config(value: serde_json::Value) -> RemoteComputerConfig {
        serde_json::from_value(value).expect("config")
    }

    #[test]
    fn off_unless_enabled_and_never_open_to_every_shared_tenant() {
        let shared = Hosting::Shared {
            operator_tenant_id: "ten_op".into(),
        };
        assert_eq!(
            remote_computer_gate(&config(serde_json::json!({})), &Hosting::Single).unwrap(),
            RemoteGate::Off
        );
        assert_eq!(
            remote_computer_gate(&config(serde_json::json!({"tenants": ["a"]})), &shared).unwrap(),
            RemoteGate::Off
        );
        assert_eq!(
            remote_computer_gate(
                &config(serde_json::json!({"enabled": true})),
                &Hosting::Single
            )
            .unwrap(),
            RemoteGate::AllTenants
        );
        assert!(
            remote_computer_gate(&config(serde_json::json!({"enabled": true})), &shared).is_err()
        );
        assert_eq!(
            remote_computer_gate(
                &config(serde_json::json!({"enabled": true, "tenants": [" a ", ""]})),
                &shared
            )
            .unwrap(),
            RemoteGate::Tenants(["a".to_string()].into())
        );
        assert!(
            serde_json::from_value::<RemoteComputerConfig>(serde_json::json!({"enable": true}))
                .is_err(),
            "a misspelt key fails the boot"
        );
    }
}

/// The work-items config, or `None` when the feature is off. A misspelt
/// key fails the boot rather than silently running a different daemon.
fn work_items_config(
    object: serde_json::Map<String, serde_json::Value>,
) -> Result<Option<kura_workitems::WorkItemsConfig>, AppError> {
    let config: kura_workitems::WorkItemsConfig =
        serde_json::from_value(serde_json::Value::Object(object))
            .map_err(|err| AppError::PluginProfile(format!("work-items config: {err}")))?;
    Ok(config.enabled.then_some(config))
}

#[cfg(test)]
mod work_items_config_tests {
    use super::work_items_config;

    fn object(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        value.as_object().cloned().expect("object")
    }

    #[test]
    fn off_by_default_and_on_only_when_enabled() {
        assert!(
            work_items_config(object(serde_json::json!({})))
                .unwrap()
                .is_none()
        );
        assert!(
            work_items_config(object(
                serde_json::json!({"enabled": false, "maxOpenItems": 3})
            ))
            .unwrap()
            .is_none()
        );
        let on = work_items_config(object(serde_json::json!({"enabled": true})))
            .unwrap()
            .expect("enabled");
        assert_eq!(on.max_open_items, 50);
        let capped = work_items_config(object(
            serde_json::json!({"enabled": true, "maxOpenItems": 3}),
        ))
        .unwrap()
        .expect("enabled");
        assert_eq!(capped.max_open_items, 3);
        // Slice 2's D7 keys are real configuration now.
        assert_eq!(on.subscription_idle_days, 14);
        assert_eq!(on.max_rounds_per_item_per_hour, 20);
        let tuned = work_items_config(object(serde_json::json!({
            "enabled": true,
            "subscriptionIdleDays": 0,
            "maxRoundsPerItemPerHour": 5
        })))
        .unwrap()
        .expect("enabled");
        assert_eq!(tuned.subscription_idle_days, 0);
        assert_eq!(tuned.max_rounds_per_item_per_hour, 5);
    }

    #[test]
    fn a_misspelt_key_is_refused() {
        assert!(work_items_config(object(serde_json::json!({"enable": true}))).is_err());
        // Later slices' keys stay unknown until their slice ships.
        assert!(
            work_items_config(object(
                serde_json::json!({"enabled": true, "recoverOnBoot": true})
            ))
            .is_err()
        );
    }
}

fn build_self_improve(asm: &mut Assembly) -> Result<(), AppError> {
    let config_object = asm.profile.config_for("self-improve");
    let config: kura_improvement::ImprovementConfig =
        serde_json::from_value(serde_json::Value::Object(config_object))
            .map_err(|err| AppError::PluginProfile(format!("self-improve config: {err}")))?;
    asm.state.improvement = Some(Arc::new(kura_improvement::Manager::new(
        &asm.cfg.data_dir,
        config,
    )));
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

#[cfg(test)]
mod session_window_unit_tests {
    use super::*;

    /// Four Chinese history messages of 30 Han characters each: 90 bytes,
    /// 30 + 4 estimated tokens apiece.
    fn chinese_window() -> serde_json::Value {
        let mut messages = vec![serde_json::json!({"role": "system", "content": "frame"})];
        for i in 0..4 {
            messages.push(serde_json::json!({
                "role": if i % 2 == 0 { "user" } else { "assistant" },
                "content": format!("{i}{}", "好".repeat(29)),
            }));
        }
        serde_json::json!({ "model": "local-8k", "messages": messages })
    }

    fn shaped(config: serde_json::Value) -> Vec<String> {
        let hook = SessionStrategyHook {
            config: serde_json::from_value(config).expect("config"),
            state: Arc::new(std::sync::OnceLock::new()),
            summarizer: None,
            summary_jobs: Arc::new(SummaryJobs::default()),
        };
        let mut payload = chinese_window();
        kura_plugin::Hook::handle(&hook, &mut payload);
        payload["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .map(|m| m["content"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    fn elided(messages: &[String]) -> bool {
        messages
            .iter()
            .any(|m| m.contains("elided by the session-strategy"))
    }

    #[test]
    fn chars_stays_the_default_and_behaves_as_before() {
        // 200 "chars" is 200 bytes: two 90-byte Chinese messages fit.
        let messages = shaped(serde_json::json!({"personalBudgetChars": 200, "keepRecent": 1}));
        assert!(elided(&messages));
        assert_eq!(messages.len(), 1 + 1 + 2, "frame, marker, two kept");
    }

    #[test]
    fn tokens_counts_chinese_by_character_not_by_byte() {
        // The same budget converted is 50 tokens; set it explicitly to hold
        // all four 34-token messages.
        let messages = shaped(serde_json::json!({
            "budgetUnit": "tokens",
            "personalBudgetTokens": 6 + 4 * 34,
            "keepRecent": 1,
        }));
        assert!(!elided(&messages), "{messages:?}");
    }

    #[test]
    fn tokens_never_exceeds_the_models_window() {
        let messages = shaped(serde_json::json!({
            "budgetUnit": "tokens",
            "personalBudgetTokens": 100_000,
            "modelContextWindows": { "local-8k": 4096 + 6 + 34 },
            "keepRecent": 1,
        }));
        assert!(elided(&messages));
        assert_eq!(messages.last().unwrap().chars().next(), Some('3'));
    }

    #[test]
    fn shadow_shapes_by_chars_and_counts_the_difference() {
        let registry = kura_telemetry::metrics::registry();
        let labels = [("agreement", "tokens_elide_fewer")];
        let before = registry.counter_value(
            kura_telemetry::metrics::SESSION_WINDOW_SHADOW_TOTAL,
            &labels,
        );

        let shadow = shaped(serde_json::json!({
            "budgetUnit": "shadow",
            "personalBudgetChars": 200,
            "personalBudgetTokens": 1000,
            "keepRecent": 1,
        }));
        let chars = shaped(serde_json::json!({"personalBudgetChars": 200, "keepRecent": 1}));

        assert_eq!(shadow, chars, "shadow does not change what the model sees");
        assert!(
            registry.counter_value(
                kura_telemetry::metrics::SESSION_WINDOW_SHADOW_TOTAL,
                &labels
            ) > before
        );
    }
}

#[cfg(test)]
mod session_summary_tests {
    use super::*;
    use kura_session::summary::{DOC_KIND_SESSION_SUMMARY, SUMMARY_MESSAGE_PREFIX, SessionSummary};

    /// Records every call; replies with `reply`.
    struct FakeSummarizer {
        calls: parking_lot::Mutex<Vec<(String, String, usize)>>,
        reply: Result<String, String>,
    }

    impl kura_session::summary::Summarizer for FakeSummarizer {
        fn summarize(
            &self,
            tenant_id: &str,
            previous: &str,
            batch: &[kura_session::WindowMessage],
            _budget_tokens: usize,
        ) -> Result<String, String> {
            self.calls
                .lock()
                .push((tenant_id.to_string(), previous.to_string(), batch.len()));
            self.reply.clone()
        }
    }

    fn fake(reply: Result<String, String>) -> Arc<FakeSummarizer> {
        Arc::new(FakeSummarizer {
            calls: parking_lot::Mutex::new(Vec::new()),
            reply,
        })
    }

    fn hook(app: &crate::App, summarizer: Arc<FakeSummarizer>) -> SessionStrategyHook {
        let state = std::sync::OnceLock::new();
        let _ = state.set(app.state.clone());
        SessionStrategyHook {
            config: serde_json::from_value(serde_json::json!({
                "personalBudgetChars": 1200,
                "keepRecent": 1,
                "summaryBudgetTokens": 20,
            }))
            .expect("config"),
            state: Arc::new(state),
            summarizer: Some(summarizer),
            summary_jobs: Arc::new(SummaryJobs::default()),
        }
    }

    /// A frame, six 400-char history messages and the current query: the
    /// 1200-char budget elides the oldest four, and still holds the two kept
    /// ones once a summary takes its share.
    fn payload(tenant_id: &str, thread_id: &str) -> serde_json::Value {
        let mut messages = vec![serde_json::json!({"role": "system", "content": "frame"})];
        for i in 0..6 {
            messages.push(serde_json::json!({
                "role": if i % 2 == 0 { "user" } else { "assistant" },
                "content": format!("{i}{}", "h".repeat(399)),
            }));
        }
        messages.push(serde_json::json!({"role": "user", "content": "what next"}));
        serde_json::json!({
            "tenantId": tenant_id, "threadId": thread_id, "sourceKind": "chat",
            "model": "", "messages": messages,
        })
    }

    fn run(hook: &SessionStrategyHook, tenant_id: &str, thread_id: &str) -> Vec<String> {
        let mut payload = payload(tenant_id, thread_id);
        kura_plugin::Hook::handle(hook, &mut payload);
        payload["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .map(|m| m["content"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    fn has_marker(messages: &[String]) -> bool {
        messages
            .iter()
            .any(|m| m.contains("elided by the session-strategy"))
    }

    fn summary_of(messages: &[String]) -> Option<&String> {
        messages
            .iter()
            .find(|m| m.starts_with(SUMMARY_MESSAGE_PREFIX))
    }

    fn wait_for(mut done: impl FnMut() -> bool) {
        for _ in 0..200 {
            if done() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        panic!("condition not reached within 5s");
    }

    fn put_summary(app: &crate::App, summary: &SessionSummary) {
        kura_store::put_document(
            &app.state.store.lock(),
            DOC_KIND_SESSION_SUMMARY,
            &summary.thread_id,
            "",
            &summary.tenant_id,
            summary,
        )
        .expect("put summary");
    }

    #[tokio::test]
    async fn an_elided_span_is_summarized_off_the_reply_path_then_replaces_the_marker() {
        let (_dir, config) = crate::tests::test_config();
        let app = crate::App::new(config).expect("app");
        let summarizer = fake(Ok(format!("they chose plan B {}", "and more ".repeat(40))));
        let hook = hook(&app, Arc::clone(&summarizer));

        // First crossing: the marker as before; the summary is written
        // behind the reply.
        let first = run(&hook, "", "thr_sum");
        assert!(
            has_marker(&first) && summary_of(&first).is_none(),
            "{first:?}"
        );
        wait_for(|| load_session_summary(&app.state, "", "thr_sum").is_some());
        let calls = summarizer.calls.lock().clone();
        assert_eq!(calls, [(String::new(), String::new(), 4)]);
        let stored = load_session_summary(&app.state, "", "thr_sum").expect("stored");
        assert!(
            kura_session::estimate_tokens(&stored.summary) <= 20,
            "the budget is enforced, not trusted to the model: {}",
            stored.summary
        );
        assert_eq!(stored.covered_messages, 4);

        // Next dispatch: the summary stands where the span was; nothing is
        // left uncovered, so no marker and no further model call.
        let second = run(&hook, "", "thr_sum");
        let summary = summary_of(&second).expect("summary injected");
        assert!(summary.contains("they chose plan B"), "{summary}");
        assert!(!has_marker(&second), "{second:?}");
        assert_eq!(second.last().map(String::as_str), Some("what next"));
        // The L0 citation appended after shaping is outside the budget, as
        // it always was for the marker; everything else fits.
        let total: usize = second
            .iter()
            .map(|m| {
                m.split("; elided span captured as")
                    .next()
                    .unwrap_or(m)
                    .len()
            })
            .sum();
        assert!(total <= 1200, "the summary fits the budget: {total}");
        assert!(
            summary.contains("elided span captured as Memory[l0_ref"),
            "{summary}"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert_eq!(summarizer.calls.lock().len(), 1);

        // Recorded like any other inclusion.
        let events = app
            .state
            .store_pool
            .read()
            .list_events(&kura_events::Filter {
                category: "context".to_string(),
                ..kura_events::Filter::default()
            })
            .expect("events");
        let included = events
            .iter()
            .filter(|e| e.name == "context.assembled")
            .flat_map(|e| {
                e.payload["record"]["included"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
            })
            .find(|item| item["source"] == "session_summary")
            .expect("summary recorded");
        assert_eq!(included["assetId"], "session_summary/thr_sum");
        assert_eq!(included["layer"], "summary");
        app.close();
    }

    #[tokio::test]
    async fn another_tenants_or_another_segments_summary_is_never_injected() {
        let (_dir, config) = crate::tests::test_config();
        let app = crate::App::new(config).expect("app");
        let summarizer = fake(Ok("fresh".to_string()));
        let hook = hook(&app, Arc::clone(&summarizer));
        let input: Vec<kura_session::WindowMessage> =
            serde_json::from_value(payload("ten_a", "thr_x")["messages"].clone()).expect("m");
        let watermark = kura_session::summary::watermark_at(&input, 4);

        put_summary(
            &app,
            &SessionSummary {
                thread_id: "thr_x".into(),
                tenant_id: "ten_b".into(),
                summary: "ten_b's secret".into(),
                watermark: watermark.clone(),
                ..SessionSummary::default()
            },
        );
        let messages = run(&hook, "ten_a", "thr_x");
        assert!(summary_of(&messages).is_none(), "{messages:?}");

        // Same tenant, but written in a segment the thread has left.
        put_summary(
            &app,
            &SessionSummary {
                thread_id: "thr_y".into(),
                tenant_id: "ten_a".into(),
                segment_id: "seg_old".into(),
                summary: "the previous conversation".into(),
                watermark,
                ..SessionSummary::default()
            },
        );
        let messages = run(&hook, "ten_a", "thr_y");
        assert!(summary_of(&messages).is_none(), "{messages:?}");
        app.close();
    }

    #[tokio::test]
    async fn a_failed_update_backs_off_instead_of_retrying_every_round() {
        let (_dir, config) = crate::tests::test_config();
        let app = crate::App::new(config).expect("app");
        let summarizer = fake(Err("provider down".to_string()));
        let hook = hook(&app, Arc::clone(&summarizer));

        let first = run(&hook, "", "thr_fail");
        assert!(has_marker(&first), "the marker is the fallback: {first:?}");
        wait_for(|| {
            hook.summary_jobs
                .inner
                .lock()
                .failed_at
                .contains_key("/thr_fail")
        });
        wait_for(|| hook.summary_jobs.inner.lock().inflight.is_empty());

        let registry = kura_telemetry::metrics::registry();
        let labels = [("outcome", "backoff")];
        let before = registry.counter_value(
            kura_telemetry::metrics::SESSION_SUMMARY_UPDATES_TOTAL,
            &labels,
        );
        let second = run(&hook, "", "thr_fail");
        assert!(has_marker(&second));
        assert_eq!(
            summarizer.calls.lock().len(),
            1,
            "no retry inside the pause"
        );
        assert!(
            registry.counter_value(
                kura_telemetry::metrics::SESSION_SUMMARY_UPDATES_TOTAL,
                &labels
            ) > before
        );
        assert!(load_session_summary(&app.state, "", "thr_fail").is_none());
        app.close();
    }
}
