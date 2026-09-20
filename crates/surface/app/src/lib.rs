//! kura-app — the daemon application assembly. The trust-boundary kernel
//! (store, event bus, identity, auth, policy, secrets, audit) is built
//! directly; every other subsystem assembles as a builtin plugin
//! (`plugins::BUILTINS`) whose enablement is resolved from the per-data-dir
//! profile `<data_dir>/plugins.json`. The resolved [`kura_plugin::AssemblyReport`]
//! lands on `AppState.plugins` for `/v1/plugins` introspection.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use kura_api::AppState;
use kura_audit::Emitter as AuditEmitter;
use kura_checkpoints::Manager as CheckpointsManager;
use kura_config::{Config, Environment};
use kura_discord::{
    GatewayTransport as DiscordGatewayTransport, new_runtime as new_discord_runtime,
};
use kura_events::Bus;
use kura_identity::auth::Manager as AuthManager;
use kura_identity::{Manager as IdentityManager, Store as IdentityStore};
use kura_im::MessageLoop;
use kura_matrix::{
    ClientTransportConfig as MatrixClientTransportConfig,
    new_client_transport as new_matrix_transport, new_runtime as new_matrix_runtime,
};
use kura_policy::Engine as PolicyEngine;
use kura_router::SessionRouter;
use kura_runtime::Manager as RuntimeManager;
use kura_secrets::Manager as SecretsManager;
use kura_slack::{
    WebApiTransport as SlackWebApiTransport, WebApiTransportConfig as SlackWebApiTransportConfig,
    new_runtime as new_slack_runtime,
};
use kura_store::{SQLiteStore, SecretStoreHandle};
use kura_telegram::{
    BotApiTransport as TelegramBotApiTransport,
    BotApiTransportConfig as TelegramBotApiTransportConfig, Runtime as TelegramRuntime,
};

mod adapters;
mod error;
mod external;
mod plugins;
pub mod rehearsal;
mod restore;
pub mod tool_host;

pub use error::AppError;
pub use rehearsal::{RehearsalReport, rehearse_upgrade};

/// Port of Go `environmentScope` for `config.Environment`.
pub fn environment_scope(environment: Environment) -> &'static str {
    match environment {
        Environment::Test => "test",
        Environment::Prod => "prod",
        // Embedded shares the non-production isolation scope with test, so
        // every persisted `environmentScope` stays within the test|prod enum
        // the API schemas declare.
        Environment::Embedded => "test",
    }
}

/// Identifiers for the embedded deployment's single local identity. They are
/// stable so a restart reuses the same tenant and its data stays visible.
const LOCAL_TENANT_ID: &str = "ten_local";
const LOCAL_PRINCIPAL_ID: &str = "prn_local_operator";

/// Ensure the embedded workspace has an active tenant and operator, returning
/// the identity that pairing should stamp onto issued tokens.
///
/// Idempotent: an existing row is left in place rather than reset, so a
/// restart does not disturb state already associated with the tenant.
fn bootstrap_local_identity(
    store: &Arc<parking_lot::Mutex<SQLiteStore>>,
) -> Result<kura_identity::auth::PairingIdentity, AppError> {
    use kura_identity::{LifecycleStatus, Principal, PrincipalKind, Tenant, TenantKind};

    let now = chrono::Utc::now();
    let guard = store.lock();
    if guard
        .get_tenant(LOCAL_TENANT_ID)
        .map_err(AppError::Store)?
        .is_none()
    {
        guard
            .upsert_tenant(&Tenant {
                tenant_id: LOCAL_TENANT_ID.to_string(),
                tenant_kind: TenantKind::Personal,
                display_name: "Local workspace".to_string(),
                status: LifecycleStatus::Active,
                created_at: now,
                updated_at: now,
                created_by_principal_id: LOCAL_PRINCIPAL_ID.to_string(),
                default_owner_principal_id: LOCAL_PRINCIPAL_ID.to_string(),
                caller_membership_role: None,
                caller_membership_status: None,
                caller_permissions: Vec::new(),
                default_for_current_token: false,
                default_for_current_principal: false,
            })
            .map_err(AppError::Store)?;
    }
    if guard
        .get_principal(LOCAL_PRINCIPAL_ID)
        .map_err(AppError::Store)?
        .is_none()
    {
        guard
            .upsert_principal(&Principal {
                principal_id: LOCAL_PRINCIPAL_ID.to_string(),
                principal_kind: PrincipalKind::LocalOperator,
                display_name: "Local operator".to_string(),
                status: LifecycleStatus::Active,
                default_tenant_id: LOCAL_TENANT_ID.to_string(),
                created_at: now,
                updated_at: now,
                disabled_at: None,
                removed_at: None,
            })
            .map_err(AppError::Store)?;
    }
    // Permissions come from the membership's role, not from the principal, so
    // without this the operator resolves to a tenant with an empty permission
    // set and every managed surface denies it. Owner is correct here: the sole
    // local operator of a personal workspace is its owner.
    let memberships = guard
        .list_memberships(&kura_identity::MembershipFilter {
            tenant_id: LOCAL_TENANT_ID.to_string(),
            ..Default::default()
        })
        .map_err(AppError::Store)?;
    if !memberships
        .iter()
        .any(|m| m.principal_id == LOCAL_PRINCIPAL_ID && m.status == LifecycleStatus::Active)
    {
        guard
            .upsert_membership(&kura_identity::Membership {
                membership_id: "mem_local_operator".to_string(),
                tenant_id: LOCAL_TENANT_ID.to_string(),
                principal_id: LOCAL_PRINCIPAL_ID.to_string(),
                role: kura_identity::Role::Owner,
                status: LifecycleStatus::Active,
                invitation_id: String::new(),
                created_at: now,
                updated_at: now,
                accepted_at: Some(now),
                removed_at: None,
            })
            .map_err(AppError::Store)?;
    }

    Ok(kura_identity::auth::PairingIdentity {
        principal_id: LOCAL_PRINCIPAL_ID.to_string(),
        default_tenant_id: LOCAL_TENANT_ID.to_string(),
    })
}

/// The daemon application. The manager instances live inside
/// [`kura_api::AppState`]; lifecycle-bearing managers (scheduler, reminders,
/// sandboxes) are read back from the state in `serve`/`close`.
pub struct App {
    pub config: Config,
    pub state: AppState,
    event_bus: Arc<Bus>,
    /// Dedicated SQLite connection shared by the connector message loops and
    /// the four channel-connector runtimes (the runtimes take
    /// Arc<SQLiteStore>, which cannot be derived from the Mutex-wrapped
    /// primary handle).
    connector_store: Arc<SQLiteStore>,
    /// Constructed connector runtimes, filled by App::serve and closed by
    /// App::close. None until serve runs (or when all connectors are disabled).
    connector_runtimes: parking_lot::Mutex<Option<ConnectorRuntimes>>,
    /// External plugin process hosts (tier 2): spawned lazily on first hook
    /// call, killed in App::close.
    external_plugins: Vec<Arc<external::ExternalProcessHost>>,
    /// Plugin-owned lifecycle callbacks (scheduler/reminders/memory starts,
    /// sandbox/scheduler/reminders closes) registered during assembly.
    lifecycle: plugins::Lifecycle,
    closed: AtomicBool,
    /// Test-only visibility into the seam adapters wired by the builtin
    /// plugins: each manager keeps its hooks behind private fields, so the
    /// wiring tests assert through these captured clones instead.
    #[cfg(test)]
    wiring: AppWiring,
}

/// The seam adapters wired into the managers by the builtin plugins,
/// captured for the wiring tests. Every field is Some when the corresponding
/// adapter is wired (activation store/billing/chat, computeruse artifact
/// recorder, execprofile sandbox health checker, evidence routine collector,
/// delivery connector adapter, mcp execution starter + secret resolver).
#[cfg(test)]
#[derive(Default)]
pub struct AppWiring {
    pub activation_store: Option<Arc<kura_activation::SqliteActivationStore>>,
    pub activation_billing: Option<Arc<kura_activation::BillingProjectorAdapter>>,
    pub activation_chat: Option<Arc<kura_activation::ChatRunnerAdapter>>,
    pub computeruse_recorder: Option<Arc<kura_computeruse::SqliteArtifactRecorder>>,
    pub execprofile_health: Option<Arc<kura_execprofile::SandboxHealthChecker>>,
    pub evidence_collector: Option<Arc<kura_evidence::RoutineCollector>>,
    pub delivery_connector: Option<Arc<kura_delivery::ConnectorAdapter>>,
    pub mcp_starter: Option<Arc<adapters::McpExecutionStarter>>,
    pub mcp_secret_resolver: Option<Arc<adapters::McpSecretResolver>>,
}

/// The four channel-connector runtimes (Go app.App fields discordRuntime,
/// telegramRuntime, slackRuntime, matrixRuntime). Each is None when the
/// connector is disabled in config or its channel plugin is disabled in the
/// profile. The telegram/matrix runtimes drive a blocking transport loop
/// from a dedicated thread (start runs the loop on the calling thread), so
/// they are stored as Arc to share with the thread; discord/slack start
/// non-blocking and stay inline.
struct ConnectorRuntimes {
    discord: Option<kura_discord::Runtime>,
    telegram: Option<Arc<TelegramRuntime>>,
    slack: Option<kura_slack::Runtime>,
    matrix: Option<Arc<kura_matrix::Runtime>>,
    telegram_thread: Option<std::thread::JoinHandle<()>>,
}

impl App {
    /// Builds the full application under the profile at
    /// `<data_dir>/plugins.json` (default profile when the file is absent; a
    /// malformed profile fails the boot loudly).
    pub fn new(cfg: Config) -> Result<Self, AppError> {
        plugins::ensure_data_dir(&cfg.data_dir);
        let profile = kura_plugin::PluginProfile::load(&cfg.data_dir)
            .map_err(|err| AppError::PluginProfile(err.to_string()))?;
        Self::with_profile(cfg, profile)
    }

    /// Builds the application under an explicit plugin profile: kernel
    /// first, then every enabled builtin plugin in declared order, then the
    /// persisted-state restore.
    pub fn with_profile(
        cfg: Config,
        profile: kura_plugin::PluginProfile,
    ) -> Result<Self, AppError> {
        let data_dir = cfg.data_dir.clone();
        let env_scope = environment_scope(cfg.environment);
        // Hosted billing quotas are a multi-tenant production concern; an
        // embedded daemon is a single local host process, like test.
        let hosted = cfg.environment == Environment::Prod;

        // --- kernel: SQLite store (migrations run inside SQLiteStore::new).
        // The primary handle is shared by the API state and the
        // parking_lot::Mutex-based managers; sandbox/mcp/chat require a
        // std::sync::Mutex handle (their concrete constructor types), so a
        // second connection to the same WAL database is opened for them.
        let store = Arc::new(parking_lot::Mutex::new(
            SQLiteStore::new(&data_dir).map_err(AppError::Store)?,
        ));
        let secondary = Arc::new(std::sync::Mutex::new(
            SQLiteStore::new(&data_dir).map_err(AppError::Store)?,
        ));
        // The connector message loops + runtimes take a plain Arc<SQLiteStore>
        // (the runtimes are single-threaded, non-Send store owners), so a
        // dedicated connection to the same WAL database is opened for them.
        #[allow(clippy::arc_with_non_send_sync)] // port-mandated Arc<SQLiteStore>
        let connector_store = Arc::new(SQLiteStore::new(&data_dir).map_err(AppError::Store)?);

        // --- kernel: event bus + core managers ---
        let event_bus = Arc::new(Bus::new());
        let session_router = Arc::new(SessionRouter::new());
        let runtime = Arc::new(RuntimeManager::new());
        let checkpoints = Arc::new(CheckpointsManager::new(store.clone(), runtime.clone()));
        let policy_engine = Arc::new(PolicyEngine::new());
        // An embedded daemon is one local workspace with no way to provision
        // identities interactively, yet several surfaces (the setup wizard
        // among them) legitimately require a tenant. Bootstrapping a local
        // operator lets pairing issue a resolvable token instead of an
        // unusable one, without weakening authentication: a caller still has
        // to complete the pairing exchange.
        let auth_manager = if cfg.environment == Environment::Embedded {
            Arc::new(AuthManager::with_pairing_identity(
                bootstrap_local_identity(&store)?,
            ))
        } else {
            Arc::new(AuthManager::new())
        };
        let identity_manager: Arc<IdentityManager<dyn IdentityStore + Send + Sync>> = {
            let erased: Arc<dyn IdentityStore + Send + Sync> =
                Arc::new(adapters::IdentityStoreHandle(store.clone()));
            Arc::new(IdentityManager::new(erased))
        };

        // --- kernel: secrets (tenant secret lifecycle + local value backend).
        // Trust-boundary decision: identity, auth, policy, secrets and audit
        // stay in the kernel and are not disableable plugins.
        let secret_backend = Arc::new(
            kura_secrets::LocalBackend::new(Path::new(&data_dir).join("tenant-secret-values"))
                .map_err(|err| AppError::Secrets(err.to_string()))?,
        );
        let secret_store = Arc::new(SecretStoreHandle::new(
            SQLiteStore::new(&data_dir).map_err(AppError::Store)?,
        ));
        let secret_manager = Arc::new(SecretsManager::new(
            secret_store.clone(),
            secret_backend.clone(),
        ));

        // --- kernel: audit emitter ---
        let audit_emitter = Arc::new(AuditEmitter::new(event_bus.clone()));

        // --- kernel state population ---
        let mut state = AppState::new(cfg.clone(), event_bus.clone(), store.clone());
        // Stage 10.2: reader connections beside the writer. `readers = 0`
        // (the default) is exactly the pre-pool single-connection behaviour.
        state.store_pool = Arc::new(
            kura_store::StorePool::new(store.clone(), cfg.store.readers)
                .map_err(AppError::Store)?,
        );
        state.policy = Some(policy_engine);
        state.auth = Some(auth_manager);
        state.identity = Some(identity_manager);
        state.router = Some(session_router);
        state.runtime = Some(runtime.clone());
        state.secrets = Some(secret_manager);
        state.checkpoints = Some(checkpoints);
        state.audit_emitter = Some(audit_emitter);
        state.tenant_migration_status = Some(Arc::new(adapters::NoMigrationInProgress));
        // The hook bus is kernel infrastructure: plugins register handlers on
        // it during assembly, consumers (chat) run the hook points.
        let hook_bus = Arc::new(kura_plugin::HookBus::new());
        state.hooks = Some(hook_bus);

        // --- kernel seams consumed by plugins ---
        let mut seams = kura_plugin::SeamMap::new();
        seams.put(plugins::SecretStoreSeam(secret_store));
        seams.put(plugins::SecretBackendSeam(secret_backend));
        seams.put(plugins::WorkflowLauncherSeam(Arc::new(
            adapters::WorkflowLauncherImpl::new(runtime),
        )));

        // --- plugin assembly: builtins first, then discovered externals
        // (tier 2). Third-party manifest problems are warnings, not boot
        // failures.
        let (external_plugins_found, external_warnings) = kura_plugin::discover_external(&data_dir);
        let mut specs: Vec<kura_plugin::PluginSpec> = plugins::descriptors()
            .iter()
            .map(kura_plugin::PluginSpec::from_descriptor)
            .collect();
        specs.extend(
            external_plugins_found
                .iter()
                .map(|plugin| kura_plugin::PluginSpec::from_manifest(&plugin.manifest)),
        );
        let mut report = kura_plugin::resolve_specs(&specs, &profile);
        report.warnings.extend(external_warnings);
        for warning in &report.warnings {
            eprintln!("[kura] plugin profile: {warning}");
        }
        let late_state: Arc<std::sync::OnceLock<AppState>> = Arc::new(std::sync::OnceLock::new());
        let mut asm = plugins::Assembly {
            cfg: cfg.clone(),
            profile: profile.clone(),
            env_scope,
            hosted,
            store,
            secondary,
            event_bus: event_bus.clone(),
            state,
            seams,
            lifecycle: plugins::Lifecycle::default(),
            late_state: late_state.clone(),
            #[cfg(test)]
            wiring: AppWiring::default(),
        };
        for plugin in plugins::BUILTINS {
            if report.enabled(plugin.descriptor.id) {
                (plugin.build)(&mut asm)?;
            }
        }
        // Enabled external plugins get a lazy process host; their declared
        // hooks proxy over stdio line-JSON with the manifest's error policy.
        let mut external_hosts = Vec::new();
        for plugin in &external_plugins_found {
            if !report.enabled(plugin.manifest.id.trim()) {
                continue;
            }
            let host = external::ExternalProcessHost::new(plugin);
            if let Some(bus) = &asm.state.hooks {
                for hook in &plugin.manifest.hooks {
                    if hook.point.trim().is_empty() {
                        continue;
                    }
                    bus.register(
                        &hook.point,
                        &host.id,
                        Arc::new(external::ExternalHook::new(
                            host.clone(),
                            &hook.point,
                            hook.on_error,
                        )),
                    );
                }
            }
            // Seam providers: the first enabled external plugin declaring
            // `context.embedder` serves the embedding seam (later
            // declarations warn and are ignored — deterministic assembly).
            if plugin
                .manifest
                .seams
                .iter()
                .any(|s| s == "context.embedder")
            {
                if asm.state.embedder.is_none() {
                    asm.state.embedder =
                        Some(Arc::new(external::ExternalEmbedder::new(host.clone())));
                } else {
                    eprintln!(
                        "[kura] plugin {}: context.embedder already served; ignoring",
                        host.id
                    );
                }
            }
            external_hosts.push(host);
        }
        asm.state.plugins = Some(Arc::new(report));

        // Restore persisted in-memory state from SQLite (Go
        // recoverPersistedStateWithSecrets). Idempotent: every Restore
        // replaces the in-memory registry wholesale; disabled plugins are
        // skipped by restore's per-manager Some guards.
        restore::recover_persisted_state(&asm.state, &event_bus, cfg.environment)?;

        // Hooks and lifecycle callbacks built during assembly see the final
        // state through this late binding (a clone taken mid-assembly would
        // miss later-built managers).
        let _ = late_state.set(asm.state.clone());

        Ok(App {
            config: cfg,
            state: asm.state,
            event_bus,
            connector_store,
            connector_runtimes: parking_lot::Mutex::new(None),
            external_plugins: external_hosts,
            lifecycle: asm.lifecycle,
            closed: AtomicBool::new(false),
            #[cfg(test)]
            wiring: asm.wiring,
        })
    }

    /// Loads the effective config (env + config.json) and builds the app.
    pub fn from_env() -> Result<Self, AppError> {
        Self::new(kura_config::load()?)
    }

    /// True when `id` resolved enabled in the plugin assembly (states built
    /// without a report — tests — behave as all-enabled).
    fn plugin_enabled(&self, id: &str) -> bool {
        self.state
            .plugins
            .as_ref()
            .is_none_or(|report| report.enabled(id))
    }

    /// The axum router over the populated state (port of Go
    /// `api.NewServer(...).Handler()`).
    #[allow(clippy::needless_pass_by_value)]
    #[must_use]
    pub fn router(&self) -> axum::Router {
        kura_api::routes::router(self.state.clone())
    }

    /// Starts background loops (scheduler tick, reminders tick) best-effort,
    /// binds the HTTP listener, serves until a shutdown signal, then closes
    /// the application. Port of Go `App.Run`.
    pub async fn serve(self: Arc<Self>) -> Result<(), AppError> {
        // Stage 10.3: the access log and every instrumented crate share
        // one process-global logger at the configured level.
        kura_telemetry::install_global_logger(kura_telemetry::Logger::new(&self.config.log_level));
        let bind_addr = self.config.bind_addr.clone();

        self.start_background_loops();

        // Construct + start the enabled channel-connector runtimes before
        // serving (Go Run: discord -> telegram -> slack -> matrix).
        let mut runtimes = self.build_connector_runtimes().await?;
        self.start_connector_runtimes(&mut runtimes)?;
        *self.connector_runtimes.lock() = Some(runtimes);

        // Publish system.started (best effort; the store/bus carry it).
        let _ = self.publish_system_event(
            "system.started",
            serde_json::json!({ "service": "kura", "version": self.config.version }),
        );

        let app = self.router();
        let listener = tokio::net::TcpListener::bind(&bind_addr)
            .await
            .map_err(|source| AppError::Bind {
                addr: bind_addr.clone(),
                source,
            })?;
        eprintln!("[kura] listening on http://{bind_addr}");

        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal())
            .await
            .map_err(AppError::Serve)?;

        let _ = self.publish_system_event(
            "system.stopped",
            serde_json::json!({ "service": "kura", "reason": "shutdown" }),
        );
        self.close();
        Ok(())
    }

    /// Runs the plugin-registered start callbacks (scheduler tick, reminders
    /// tick, memory 60s consolidation/retention tick, ...) in registration
    /// order. Each start is best-effort: failures are the plugin's to log.
    fn start_background_loops(&self) {
        for (_plugin_id, start) in &self.lifecycle.starts {
            start(&self.state);
        }
    }

    /// Stops background loops and closes the bus (port of Go `App.Close`).
    /// Idempotent.
    pub fn close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        // Connector runtimes close first (Go Close order: discord -> telegram
        // -> slack -> matrix); the telegram poll thread is joined once the
        // transport close drops the channel sender and unblocks its loop.
        let mut first_err: Option<String> = None;
        if let Some(runtimes) = self.connector_runtimes.lock().as_mut() {
            if let Some(discord) = &runtimes.discord {
                if let Err(err) = discord.close() {
                    first_err.get_or_insert_with(|| format!("discord: {err}"));
                }
            }
            if let Some(telegram) = &runtimes.telegram {
                if let Err(err) = telegram.close() {
                    first_err.get_or_insert_with(|| format!("telegram: {err}"));
                }
                if let Some(handle) = runtimes.telegram_thread.take() {
                    let _ = handle.join();
                }
            }
            if let Some(slack) = &runtimes.slack {
                if let Err(err) = slack.close() {
                    first_err.get_or_insert_with(|| format!("slack: {err}"));
                }
            }
            if let Some(matrix) = &runtimes.matrix {
                // The matrix client sync loop has no cancellation seam (the
                // transport close is a no-op), so its thread is detached and
                // ends with the process; only the close is reported here.
                if let Err(err) = matrix.close() {
                    first_err.get_or_insert_with(|| format!("matrix: {err}"));
                }
                // Leak a clone so the detached matrix thread never observes a
                // freed runtime after the App is dropped.
                std::mem::forget(matrix.clone());
            }
        }
        if let Some(err) = first_err {
            eprintln!("[kura] connector close error: {err}");
        }
        // Plugin-registered closes in registration order (sandbox, scheduler,
        // reminders under the default assembly); each is idempotent.
        for (_plugin_id, close) in &self.lifecycle.closes {
            close(&self.state);
        }
        for host in &self.external_plugins {
            host.close();
        }
        self.event_bus.close();
    }

    // ---------------------------------------------------------------------
    // Channel-connector runtimes
    // ---------------------------------------------------------------------

    /// Builds one connector message loop over the shared managers (port of Go
    /// im.NewMessageLoop). kura-im::MessageLoop and kura_router::SessionRouter
    /// are not Clone, so each connector runtime receives its own loop + router
    /// over the same runtime/checkpoints/bus/store/chat deps — matching Go's
    /// per-runtime im.NewMessageLoop(sessionRouter, ...) construction.
    fn connector_message_loop(&self) -> MessageLoop {
        let runtime = self.state.runtime.clone().expect("runtime wired in kernel");
        let chat = self
            .state
            .chat
            .clone()
            .expect("chat plugin enabled (channel requires)");
        MessageLoop::new(
            SessionRouter::new(),
            runtime.clone(),
            Some(CheckpointsManager::new(self.state.store.clone(), runtime)),
            Some((*self.event_bus).clone()),
            self.connector_store.clone(),
            (*chat).clone(),
        )
    }

    /// Resolves a connector secret value through the secret manager (Go
    /// slackBotTokenProvider/matrixBotAccessTokenProvider resolve via
    /// secrets.Resolve). None when no tenant or secret is available; the
    /// caller falls back to the raw config token.
    async fn resolve_connector_secret(&self, secret_ref: &str) -> Option<String> {
        let tenant_id = self
            .state
            .store
            .lock()
            .resolve_default_personal_tenant_id()
            .ok()?;
        let manager = self.state.secrets.clone()?;
        let resolved = manager
            .resolve(kura_secrets::ResolveInput {
                tenant_id,
                secret_ref: secret_ref.trim().to_string(),
            })
            .await
            .ok()?;
        Some(resolved.value)
    }

    /// Constructs the four connector runtimes for the enabled connectors in
    /// config (Go app.New connector block). A connector builds only when its
    /// config flag AND its channel plugin are enabled, so no network or
    /// credential is touched unless both agree.
    async fn build_connector_runtimes(&self) -> Result<ConnectorRuntimes, AppError> {
        let connector_cfg = &self.config.connectors;
        let supervisor = self
            .state
            .connectors
            .clone()
            .expect("connectors supervisor plugin enabled (channel requires)");
        let store = Some(self.connector_store.clone());
        let bus = Some((*self.event_bus).clone());

        // --- discord ---
        let discord_enabled =
            connector_cfg.discord.enabled && self.plugin_enabled("channel-discord");
        let discord_cfg = kura_discord::Config {
            enabled: discord_enabled,
            connector_id: connector_cfg.discord.connector_id.clone(),
            display_name: connector_cfg.discord.display_name.clone(),
            delivery_mode: connector_cfg.discord.delivery_mode.clone(),
            bot_token: connector_cfg.discord.bot_token.clone(),
            require_mention: connector_cfg.discord.require_mention,
            respond_in_dm: connector_cfg.discord.respond_in_dm,
            allowed_guild_ids: connector_cfg.discord.allowed_guild_ids.clone(),
            allowed_channel_ids: connector_cfg.discord.allowed_channel_ids.clone(),
            tenant_id: String::new(),
        };
        let discord_transport: Option<Box<dyn kura_discord::Transport>> = if discord_enabled {
            Some(Box::new(
                DiscordGatewayTransport::new(discord_cfg.clone()).map_err(|err| {
                    AppError::ConnectorRuntime(format!("discord transport: {err}"))
                })?,
            ))
        } else {
            None
        };
        let discord = new_discord_runtime(
            discord_cfg,
            Some(kura_telemetry::Logger::new(&self.config.log_level)),
            supervisor.clone(),
            // The discord runtime takes Arc<MessageLoop>; the loop is !Send
            // (single-threaded store), matching the runtime's design.
            #[allow(clippy::arc_with_non_send_sync)]
            Arc::new(self.connector_message_loop()),
            store.clone(),
            bus.clone(),
            discord_transport,
        )
        .map_err(|err| AppError::ConnectorRuntime(format!("discord runtime: {err}")))?;

        // --- telegram ---
        let telegram_enabled =
            connector_cfg.telegram.enabled && self.plugin_enabled("channel-telegram");
        let telegram_cfg = kura_telegram::Config {
            enabled: telegram_enabled,
            connector_id: connector_cfg.telegram.connector_id.clone(),
            display_name: connector_cfg.telegram.display_name.clone(),
            bot_token: connector_cfg.telegram.bot_token.clone(),
            bot_username: connector_cfg.telegram.bot_username.clone(),
            tenant_id: String::new(),
            allowments: restore::telegram_allowments_from_config(&connector_cfg.telegram),
        };
        let telegram_transport: Option<Arc<dyn kura_telegram::Transport>> = if telegram_enabled {
            Some(Arc::new(
                TelegramBotApiTransport::new(TelegramBotApiTransportConfig {
                    connector_id: connector_cfg.telegram.connector_id.clone(),
                    bot_token: connector_cfg.telegram.bot_token.clone(),
                    bot_username: connector_cfg.telegram.bot_username.clone(),
                    base_url: connector_cfg.telegram.bot_api_base_url.clone(),
                    ..Default::default()
                })
                .map_err(|err| AppError::ConnectorRuntime(format!("telegram transport: {err}")))?,
            ))
        } else {
            None
        };
        let telegram = TelegramRuntime::new(
            telegram_cfg,
            supervisor.clone(),
            Some(self.connector_message_loop()),
            store.clone(),
            bus.clone(),
            telegram_transport,
        )
        .map_err(|err| AppError::ConnectorRuntime(format!("telegram runtime: {err}")))?
        .map(Arc::new);

        // --- slack ---
        let slack_enabled = connector_cfg.slack.enabled && self.plugin_enabled("channel-slack");
        let slack_cfg = kura_slack::Config {
            enabled: slack_enabled,
            connector_id: connector_cfg.slack.connector_id.clone(),
            display_name: connector_cfg.slack.display_name.clone(),
            workspace_binding_id: connector_cfg.slack.workspace_binding_id.clone(),
            workspace_id: connector_cfg.slack.workspace_id.clone(),
            bot_user_id: connector_cfg.slack.bot_user_id.clone(),
            allowed_channel_ids: connector_cfg.slack.allowed_channel_ids.clone(),
            allowed_dm_user_ids: connector_cfg.slack.allowed_dm_user_ids.clone(),
            allowed_dm_user_groups: connector_cfg.slack.allowed_dm_user_groups.clone(),
            tenant_id: String::new(),
        };
        let slack_token = if connector_cfg.slack.bot_token_secret_ref.trim().is_empty() {
            String::new()
        } else {
            self.resolve_connector_secret(&connector_cfg.slack.bot_token_secret_ref)
                .await
                .unwrap_or_default()
        };
        let slack_transport: Option<Arc<dyn kura_slack::Transport>> = if slack_enabled {
            Some(Arc::new(SlackWebApiTransport::new(
                SlackWebApiTransportConfig {
                    connector_id: connector_cfg.slack.connector_id.clone(),
                    base_url: connector_cfg.slack.api_base_url.clone(),
                    bot_token: slack_token,
                    ..Default::default()
                },
            )))
        } else {
            None
        };
        let slack = new_slack_runtime(
            slack_cfg,
            supervisor.clone(),
            self.connector_message_loop(),
            store.clone(),
            bus.clone(),
            slack_transport,
        )
        .map_err(|err| AppError::ConnectorRuntime(format!("slack runtime: {err}")))?;

        // --- matrix ---
        let matrix_enabled = connector_cfg.matrix.enabled && self.plugin_enabled("channel-matrix");
        let matrix_cfg = kura_matrix::types::Config {
            enabled: matrix_enabled,
            connector_id: connector_cfg.matrix.connector_id.clone(),
            display_name: connector_cfg.matrix.display_name.clone(),
            homeserver_url: connector_cfg.matrix.homeserver_url.clone(),
            homeserver_id: connector_cfg.matrix.homeserver_id.clone(),
            bot_user_id: connector_cfg.matrix.bot_user_id.clone(),
            selected_room_ids: connector_cfg.matrix.selected_room_ids.clone(),
            allowed_direct_user_ids: connector_cfg.matrix.allowed_direct_user_ids.clone(),
            configured_commands: connector_cfg.matrix.configured_commands.clone(),
        };
        let matrix_transport: Option<Box<dyn kura_matrix::Transport>> = if matrix_enabled {
            Some(Box::new(
                new_matrix_transport(MatrixClientTransportConfig {
                    connector_id: connector_cfg.matrix.connector_id.clone(),
                    homeserver_url: connector_cfg.matrix.homeserver_url.clone(),
                    bot_access_token: connector_cfg.matrix.bot_access_token.clone(),
                    selected_room_ids: connector_cfg.matrix.selected_room_ids.clone(),
                    allowed_direct_user_ids: connector_cfg.matrix.allowed_direct_user_ids.clone(),
                    ..Default::default()
                })
                .map_err(|err| AppError::ConnectorRuntime(format!("matrix transport: {err}")))?,
            ))
        } else {
            None
        };
        let matrix = new_matrix_runtime(
            matrix_cfg,
            supervisor.clone(),
            self.connector_message_loop(),
            store,
            bus,
            matrix_transport,
        )
        .map_err(|err| AppError::ConnectorRuntime(format!("matrix runtime: {err}")))?
        .map(Arc::new);

        Ok(ConnectorRuntimes {
            discord,
            telegram,
            slack,
            matrix,
            telegram_thread: None,
        })
    }

    /// Starts the constructed connector runtimes (Go App.Run connector
    /// block). Discord/slack start non-blocking on the calling thread; the
    /// telegram transport loop blocks until close, so it runs on a dedicated
    /// thread that is joined in App::close.
    fn start_connector_runtimes(&self, runtimes: &mut ConnectorRuntimes) -> Result<(), AppError> {
        if let Some(discord) = &runtimes.discord {
            discord
                .start()
                .map_err(|err| AppError::ConnectorStart(format!("discord: {err}")))?;
        }
        if let Some(telegram) = &runtimes.telegram {
            // SAFETY: kura_telegram::Runtime is !Send (its inner store
            // connection is single-threaded), so the thread receives a raw
            // pointer instead of moving the runtime. The runtime is kept alive
            // by the App's Arc for the whole process, and every runtime method
            // accesses inner state through the parking_lot::Mutex, so
            // concurrent start() (this thread) and close() (the app thread) are
            // serialized. close() closes the transport, dropping the poll
            // thread's channel sender, which unblocks start() so the thread
            // returns and can be joined before the App is dropped.
            let raw = Arc::as_ptr(telegram) as usize;
            let handle = std::thread::Builder::new()
                .name("telegram-connector".to_string())
                .spawn(move || {
                    // SAFETY: raw is the address of the App-owned runtime,
                    // which stays valid until this thread is joined in close().
                    let rt = unsafe { &*(raw as *const TelegramRuntime) };
                    let _ = rt.start();
                })
                .map_err(|err| AppError::ConnectorStart(format!("telegram thread: {err}")))?;
            runtimes.telegram_thread = Some(handle);
        }
        if let Some(slack) = &runtimes.slack {
            slack
                .start()
                .map_err(|err| AppError::ConnectorStart(format!("slack: {err}")))?;
        }
        if let Some(matrix) = &runtimes.matrix {
            // SAFETY: same serialized-inner rationale as the telegram thread.
            // The matrix client sync loop has no cancellation seam (the
            // transport close is a no-op), so the thread is detached and runs
            // until the process exits; close() leaks a clone of the App's Arc
            // so the runtime is never freed under the running thread.
            let raw = Arc::as_ptr(matrix) as usize;
            let tenant_id = self
                .state
                .store
                .lock()
                .resolve_default_personal_tenant_id()
                .unwrap_or_default();
            std::thread::Builder::new()
                .name("matrix-connector".to_string())
                .spawn(move || {
                    // SAFETY: raw is the address of the App-owned runtime;
                    // close() leaks a clone so it stays valid for the process.
                    let rt = unsafe { &*(raw as *const kura_matrix::Runtime) };
                    let _ = rt.start(&tenant_id);
                })
                .map_err(|err| AppError::ConnectorStart(format!("matrix thread: {err}")))?;
        }
        Ok(())
    }

    /// Persists a system event on the store and publishes it on the bus
    /// (port of Go `publishSystemEvent`).
    fn publish_system_event(&self, name: &str, payload: serde_json::Value) -> Result<(), AppError> {
        let mut event = kura_events::Event::default();
        event.environment_scope = environment_scope(self.config.environment).to_string();
        event.category = "system".to_string();
        event.name = name.to_string();
        event.resource = kura_events::Resource {
            kind: "system".to_string(),
            id: "kura".to_string(),
        };
        if let Some(object) = payload.as_object() {
            event.payload = object.clone();
        }
        let persisted = self
            .state
            .store
            .lock()
            .append_event(&event)
            .map_err(AppError::SystemEvent)?;
        self.event_bus.publish(persisted);
        Ok(())
    }
}

/// Waits for SIGINT (ctrl-c) or SIGTERM.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler");
        sigterm.recv().await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::to_bytes;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    /// A test config pointing at a fresh temp data dir.
    use kura_config::LlmConfig;

    fn test_config() -> Config {
        let dir = std::env::temp_dir().join(format!("kura-app-smoke-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("create temp data dir");
        Config {
            project_root: String::new(),
            store: Default::default(),
            environment: Environment::Test,
            bind_addr: "127.0.0.1:0".to_string(),
            data_dir: dir.to_string_lossy().into_owned(),
            log_level: "info".to_string(),
            version: "dev".to_string(),
            // These tests dispatch at `echo`, which is a fixture rather than a
            // provider the daemon ships: it appears only where something names
            // it. Asking for it here is what these tests were relying on the
            // inventory to do for them.
            llm: LlmConfig {
                default_provider: "echo".to_string(),
                ..Default::default()
            },
            connectors: Default::default(),
            egress: Default::default(),
        }
    }

    #[tokio::test]
    async fn embedded_bootstraps_a_resolvable_local_identity() {
        // Without this, pairing issues a token with an empty principal that the
        // resolver refuses, so every /v1 route answers 403 after a successful
        // pairing -- and surfaces that legitimately require a tenant (the setup
        // wizard) can never be reached.
        let mut config = test_config();
        config.environment = Environment::Embedded;
        let app = App::new(config).expect("build embedded app");

        assert!(
            app.state.auth.is_some(),
            "embedded still requires a bearer token"
        );
        assert!(
            app.state.identity.is_some(),
            "tenant resolution stays enabled"
        );

        let store = app.state.store.lock();
        let tenant = store.get_tenant(LOCAL_TENANT_ID).expect("tenant lookup");
        assert!(tenant.is_some(), "a local tenant exists to resolve against");
        let principal = store
            .get_principal(LOCAL_PRINCIPAL_ID)
            .expect("principal lookup")
            .expect("a local operator exists");
        assert_eq!(principal.default_tenant_id, LOCAL_TENANT_ID);
        assert_eq!(principal.status, kura_identity::LifecycleStatus::Active);
    }

    #[tokio::test]
    async fn bootstrapping_the_local_identity_is_idempotent() {
        // A restart must reuse the tenant rather than reset it, or state
        // associated with the workspace would be orphaned.
        let mut config = test_config();
        config.environment = Environment::Embedded;
        let first = App::new(config.clone()).expect("first boot");
        let created_at = {
            let store = first.state.store.lock();
            store
                .get_tenant(LOCAL_TENANT_ID)
                .expect("tenant")
                .expect("tenant exists")
                .created_at
        };
        drop(first);

        let second = App::new(config).expect("second boot");
        let store = second.state.store.lock();
        let reused = store
            .get_tenant(LOCAL_TENANT_ID)
            .expect("tenant")
            .expect("tenant");
        assert_eq!(
            reused.created_at, created_at,
            "tenant reused, not recreated"
        );
    }

    #[tokio::test]
    async fn non_embedded_environments_do_not_bootstrap_a_local_identity() {
        let app = App::new(test_config()).expect("build app");
        assert!(app.state.identity.is_some());
        let store = app.state.store.lock();
        assert!(
            store
                .get_principal(LOCAL_PRINCIPAL_ID)
                .expect("lookup")
                .is_none(),
            "only embedded provisions a local operator"
        );
    }

    #[tokio::test]
    async fn an_unconfigured_openai_endpoint_is_not_registered() {
        // Registering it without a base URL would put a provider in the
        // dispatcher that fails every dispatch, which reads as a broken daemon
        // rather than an unconfigured one.
        let app = App::new(test_config()).expect("build app");
        let dispatcher = app.state.llm.as_ref().expect("dispatcher");
        assert!(!dispatcher.has_provider("openai_compatible"));
        // The built-in fallback is always present.
        assert!(dispatcher.has_provider("echo"));
    }

    #[tokio::test]
    async fn a_configured_openai_endpoint_becomes_dispatchable() {
        let mut config = test_config();
        config.llm.openai_compatible.base_url = "https://api.example.test/v1".to_string();
        config.llm.openai_compatible.model = "test-model".to_string();
        let app = App::new(config).expect("build app");

        let dispatcher = app.state.llm.as_ref().expect("dispatcher");
        assert!(
            dispatcher.has_provider("openai_compatible"),
            "a configured endpoint is registered so it can actually answer"
        );
    }

    /// The full embedded authorization chain, which failed in four distinct
    /// ways while being built: pairing must issue a token carrying the local
    /// principal, that token must be granted its tenant, the principal must
    /// hold an owner membership, and only then does a protected route resolve.
    /// Each link was individually plausible and individually wrong.
    #[tokio::test]
    async fn embedded_pairing_yields_a_token_that_resolves_and_authorizes() {
        let mut config = test_config();
        config.environment = Environment::Embedded;
        let app = App::new(config).expect("build embedded app");
        let router = app.router();

        // Unauthenticated access stays closed.
        let denied = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/auth/me")
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("oneshot");
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

        let started = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/auth/pairings/start")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        r#"{"mode":"local","label":"test","ttlSeconds":300}"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("oneshot");
        assert_eq!(started.status(), StatusCode::CREATED);
        let started: serde_json::Value = serde_json::from_slice(
            &to_bytes(started.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("json");
        let pairing_id = started["pairing"]["pairingId"]
            .as_str()
            .expect("pairing id");
        let code = started["pairingCode"].as_str().expect("code");

        let completed = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/auth/pairings/{pairing_id}/complete"))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(format!(r#"{{"code":"{code}"}}"#)))
                    .expect("request"),
            )
            .await
            .expect("oneshot");
        assert_eq!(completed.status(), StatusCode::OK);
        let completed: serde_json::Value = serde_json::from_slice(
            &to_bytes(completed.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("json");
        let secret = completed["accessToken"].as_str().expect("access token");

        let me = router
            .oneshot(
                Request::builder()
                    .uri("/v1/auth/me")
                    .header("authorization", format!("Bearer {secret}"))
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("oneshot");
        // Not merely 200: a token that authenticates but cannot resolve a
        // tenant returns 403 here, which is the failure this chain guards.
        assert_eq!(me.status(), StatusCode::OK);
        let me: serde_json::Value =
            serde_json::from_slice(&to_bytes(me.into_body(), usize::MAX).await.expect("body"))
                .expect("json");
        assert_eq!(me["principal"]["principalId"], LOCAL_PRINCIPAL_ID);
        assert_eq!(me["defaultTenant"]["tenantId"], LOCAL_TENANT_ID);

        app.close();
    }

    /// End-to-end wiring proof: build the full App against a temp-dir store,
    /// serve the router in-process, and hit the introspection routes.
    #[tokio::test]
    async fn healthz_returns_ok_with_full_wiring() {
        let config = test_config();
        let app = App::new(config).expect("build app");

        // The store migrated to head schema and all managers are populated.
        let schema_version = app
            .state
            .store
            .lock()
            .schema_version()
            .expect("schema version");
        assert_eq!(schema_version, kura_store::CURRENT_SCHEMA_VERSION);
        assert!(app.state.policy.is_some());
        assert!(app.state.identity.is_some());
        assert!(app.state.chat.is_some());
        assert!(app.state.scheduler.is_some());
        assert!(app.state.evaluation.is_some());
        assert!(app.state.live_validation.is_some());

        // The default profile resolves every builtin plugin enabled.
        let report = app.state.plugins.as_ref().expect("assembly report");
        assert!(
            report.plugins.iter().all(|p| p.enabled),
            "all builtins enabled"
        );
        assert!(report.warnings.is_empty());

        let router = app.router();
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("oneshot");
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json body");
        assert_eq!(json, serde_json::json!({ "ok": true, "service": "kura" }));

        // /version and /v1/system/info also route through the populated state.
        let version_response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/version")
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("oneshot");
        assert_eq!(version_response.status(), StatusCode::OK);
        let version_bytes = to_bytes(version_response.into_body(), usize::MAX)
            .await
            .expect("body");
        let version_json: serde_json::Value =
            serde_json::from_slice(&version_bytes).expect("json body");
        assert_eq!(version_json, serde_json::json!({ "version": "dev" }));

        let info_response = router
            .oneshot(
                Request::builder()
                    .uri("/v1/system/info")
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("oneshot");
        assert_eq!(info_response.status(), StatusCode::OK);

        // Close is idempotent and stops the background loops.
        app.close();
        app.close();
    }

    /// The app can be built twice against the same data dir (restart path);
    /// migrations are idempotent.
    #[tokio::test]
    async fn rebuild_on_existing_store_is_idempotent() {
        let config = test_config();
        let app = App::new(config.clone()).expect("first build");
        app.close();
        let app2 = App::new(config).expect("second build");
        assert_eq!(
            app2.state
                .store
                .lock()
                .schema_version()
                .expect("schema version"),
            kura_store::CURRENT_SCHEMA_VERSION
        );
        app2.close();
    }

    /// Disabling a leaf plugin leaves its manager unwired (the API answers
    /// not-wired) while the rest of the assembly is unaffected, and the
    /// report records the decision.
    #[tokio::test]
    async fn disabled_leaf_plugin_leaves_manager_unwired() {
        let config = test_config();
        let profile = kura_plugin::PluginProfile {
            disabled: vec!["triage".to_string()],
            ..Default::default()
        };
        let app = App::with_profile(config, profile).expect("build app");
        assert!(app.state.triage.is_none(), "triage unwired");
        assert!(app.state.chat.is_some(), "unrelated plugins unaffected");
        let report = app.state.plugins.as_ref().expect("report");
        assert!(!report.enabled("triage"));
        let triage = report
            .plugins
            .iter()
            .find(|p| p.id == "triage")
            .expect("entry");
        assert_eq!(triage.reason.as_deref(), Some("disabled by profile"));
        app.close();
    }

    /// Disabling a dependency transitively disables its dependents — the
    /// fail-closed gates (webhook quota, billing enforcement) can never run
    /// half-wired.
    #[tokio::test]
    async fn disabling_billing_transitively_disables_dependents() {
        let config = test_config();
        let profile = kura_plugin::PluginProfile {
            disabled: vec!["billing".to_string()],
            ..Default::default()
        };
        let app = App::with_profile(config, profile).expect("build app");
        assert!(app.state.billing.is_none());
        assert!(
            app.state.activation.is_none(),
            "activation requires billing"
        );
        assert!(app.state.webhooks.is_none(), "webhooks require billing");
        assert!(
            app.state.evaluation.is_none(),
            "evaluation requires billing"
        );
        assert!(
            app.state.live_validation.is_none(),
            "live-validation requires billing"
        );
        let report = app.state.plugins.as_ref().expect("report");
        let webhooks = report
            .plugins
            .iter()
            .find(|p| p.id == "webhooks")
            .expect("entry");
        assert_eq!(
            webhooks.reason.as_deref(),
            Some("requires disabled plugin `billing`")
        );
        app.close();
    }

    /// A profile written to <data_dir>/plugins.json is picked up by App::new.
    #[tokio::test]
    async fn profile_file_in_data_dir_is_loaded() {
        let config = test_config();
        std::fs::write(
            std::path::Path::new(&config.data_dir).join(kura_plugin::PROFILE_FILE_NAME),
            serde_json::json!({ "disabled": ["memory"] }).to_string(),
        )
        .expect("write profile");
        let app = App::new(config).expect("build app");
        assert!(
            app.state.memory.is_none(),
            "memory disabled via plugins.json"
        );
        app.close();
    }

    /// Connector runtimes are skipped when every connector is disabled (no
    /// network, no credentials), and are constructed when a connector is
    /// enabled with a token. Restore is idempotent: rebuilding the app against
    /// the same data dir runs recoverPersistedState again without error.
    #[tokio::test]
    async fn connector_runtimes_constructed_or_skipped_and_restore_idempotent() {
        // All connectors disabled: the four runtimes are None (skipped).
        let config = test_config();
        let app = App::new(config.clone()).expect("build app");
        let runtimes = app
            .build_connector_runtimes()
            .await
            .expect("build connector runtimes");
        assert!(runtimes.discord.is_none(), "discord disabled -> skipped");
        assert!(runtimes.telegram.is_none(), "telegram disabled -> skipped");
        assert!(runtimes.slack.is_none(), "slack disabled -> skipped");
        assert!(runtimes.matrix.is_none(), "matrix disabled -> skipped");
        // Starting the (empty) runtime set is a no-op.
        let mut runtimes = runtimes;
        app.start_connector_runtimes(&mut runtimes)
            .expect("start no runtimes");
        app.close();

        // Discord enabled with a token: the runtime is constructed. Nothing is
        // started, so no network is touched.
        let mut enabled = config.clone();
        enabled.connectors.discord.enabled = true;
        enabled.connectors.discord.bot_token = "test-token".to_string();
        enabled.connectors.discord.connector_id = "discord-test".to_string();
        enabled.connectors.discord.display_name = "Discord Test".to_string();
        let app2 = App::new(enabled.clone()).expect("build app with discord enabled");
        let runtimes2 = app2
            .build_connector_runtimes()
            .await
            .expect("build runtimes with discord enabled");
        assert!(
            runtimes2.discord.is_some(),
            "discord enabled -> runtime constructed"
        );
        assert!(
            app2.state
                .connectors
                .as_ref()
                .expect("connectors supervisor")
                .list()
                .is_empty(),
            "no connector registered until start"
        );
        app2.close();

        // Restore idempotence: rebuild against the same data dir (the store now
        // holds whatever bootstrap wrote) and recover again.
        let app3 = App::new(enabled).expect("rebuild app on existing store");
        app3.close();
    }

    /// Disabling a channel plugin gates the runtime even when the connector
    /// config flag is on: profile wins, no network or credential is touched.
    #[tokio::test]
    async fn disabled_channel_plugin_gates_connector_runtime() {
        let mut config = test_config();
        config.connectors.discord.enabled = true;
        config.connectors.discord.bot_token = "test-token".to_string();
        config.connectors.discord.connector_id = "discord-test".to_string();
        config.connectors.discord.display_name = "Discord Test".to_string();
        let profile = kura_plugin::PluginProfile {
            disabled: vec!["channel-discord".to_string()],
            ..Default::default()
        };
        let app = App::with_profile(config, profile).expect("build app");
        let runtimes = app
            .build_connector_runtimes()
            .await
            .expect("build connector runtimes");
        assert!(
            runtimes.discord.is_none(),
            "channel-discord disabled -> runtime skipped despite config flag"
        );
        app.close();
    }

    /// The kernel hook bus reaches the chat pipeline: a pre-dispatch hook
    /// registered on the app's bus mutates what the echo provider sees, and
    /// the persisted dispatch record carries the same mutation
    /// (model-visible = logged), end to end through the real assembly.
    #[test]
    fn chat_hook_bus_is_wired_end_to_end() {
        let config = test_config();
        let app = App::new(config).expect("build app");
        let hooks = app.state.hooks.clone().expect("hook bus wired");
        struct Inject;
        impl kura_plugin::Hook for Inject {
            fn handle(&self, payload: &mut serde_json::Value) -> kura_plugin::HookOutcome {
                let messages = payload["messages"].as_array_mut().expect("messages");
                messages.insert(
                    0,
                    serde_json::json!({ "role": "system", "content": "hook window" }),
                );
                kura_plugin::HookOutcome::Continue
            }
        }
        hooks.register(
            kura_plugin::points::CHAT_PRE_DISPATCH,
            "test-plugin",
            Arc::new(Inject),
        );

        let chat = app.state.chat.clone().expect("chat wired");
        let execution = chat
            .query(
                kura_chat::QueryInput {
                    query: "hello".to_string(),
                    provider: "echo".to_string(),
                    ..Default::default()
                },
                &kura_chat::CancellationToken::new(),
            )
            .expect("query through the assembled app");
        assert!(
            execution
                .result
                .dispatch
                .messages
                .iter()
                .any(|message| message.content == "hook window"),
            "persisted dispatch logs the hook-injected message"
        );
        app.close();
    }

    /// Behavioral pluginization: the memory plugin's chat/turn-end hook
    /// captures the settled turn as an L0 asset through the real assembly
    /// (no hardcoded API-layer call), and lifecycle callbacks are
    /// plugin-registered.
    #[test]
    fn memory_capture_rides_the_turn_end_hook() {
        let config = test_config();
        let app = App::new(config).expect("build app");
        let hooks = app.state.hooks.as_ref().expect("hook bus");
        assert!(
            hooks
                .registrations()
                .contains(&("chat/turn-end".to_string(), "memory".to_string())),
            "memory capture hook registered"
        );
        // Lifecycle registrations are plugin-owned.
        let start_ids: Vec<&str> = app
            .lifecycle
            .starts
            .iter()
            .map(|(id, _)| id.as_str())
            .collect();
        assert_eq!(start_ids, ["memory", "scheduler", "reminders"]);
        let close_ids: Vec<&str> = app
            .lifecycle
            .closes
            .iter()
            .map(|(id, _)| id.as_str())
            .collect();
        assert_eq!(close_ids, ["sandbox", "scheduler", "reminders"]);

        let chat = app.state.chat.clone().expect("chat wired");
        let execution = chat
            .query(
                kura_chat::QueryInput {
                    query: "remember this fact".to_string(),
                    provider: "echo".to_string(),
                    // Tenant-scoped chat exercises the full ChatStore
                    // delegation (profile selection auto-seeds a default).
                    tenant_id: "ten_hook".to_string(),
                    ..Default::default()
                },
                &kura_chat::CancellationToken::new(),
            )
            .expect("query");
        assert!(execution.exec_error.is_none());

        let assets = app
            .state
            .store
            .lock()
            .list_all_memory_assets()
            .expect("list memory assets");
        let captured = assets
            .iter()
            .find(|asset| asset.title == "chat_turn")
            .expect("turn captured as L0 asset via the hook");
        assert!(captured.content.contains("remember this fact"));

        // Channel-source turns are captured too (the only capture point for
        // gateway-driven IM traffic) and carry the message source link.
        let mut channel_payload = serde_json::json!({
            "dispatchId": "disp_chan",
            "tenantId": "",
            "threadId": "thr_chan",
            "query": "im question",
            "output": "im answer",
            "status": "completed",
            "sourceKind": "channel",
            "sourceMessageId": "msg_chan_1",
        });
        let outcome = hooks.run(kura_plugin::points::CHAT_TURN_END, &mut channel_payload);
        assert!(outcome.allowed());
        let assets = app
            .state
            .store
            .lock()
            .list_all_memory_assets()
            .expect("list memory assets");
        let channel_turn = assets
            .iter()
            .find(|asset| asset.content.contains("im question"))
            .expect("channel turn captured");
        assert!(
            channel_turn
                .source_links
                .iter()
                .any(|link| link.id == "msg_chan_1"),
            "message source link carried"
        );
        assert!(
            channel_turn
                .source_links
                .iter()
                .any(|link| link.id == "thr_chan"),
            "thread source link carried"
        );
        app.close();
    }

    /// The default context plugin injects the tenant's Ready memory
    /// bootstrap at chat/pre-dispatch — before session-strategy in the
    /// waterfall — with the citation inline, and records the decision as a
    /// context.assembled event. Later hooks still reshape the result
    /// (composition proof: a tiny session budget elides history but the
    /// injected bootstrap survives as frame).
    #[tokio::test]
    async fn context_plugin_injects_memory_bootstrap_and_composes() {
        let config = test_config();
        std::fs::write(
            std::path::Path::new(&config.data_dir).join(kura_plugin::PROFILE_FILE_NAME),
            serde_json::json!({
                "entries": {
                    "session-strategy": { "config": { "personalBudgetChars": 40, "keepRecent": 1 } }
                }
            })
            .to_string(),
        )
        .expect("write profile");
        let app = App::new(config).expect("build app");
        let hooks = app.state.hooks.as_ref().expect("hook bus");
        // Waterfall order: context injects before session-strategy shapes.
        let pre_dispatch: Vec<String> = hooks
            .registrations()
            .into_iter()
            .filter(|(point, _)| point == "chat/pre-dispatch")
            .map(|(_, plugin)| plugin)
            .collect();
        assert_eq!(pre_dispatch, ["context", "session-strategy"]);

        // Seed a Ready L3 persona in the local operator scope through the
        // real governed chain: L1 atom (with source links) -> L2 scenario
        // (members) -> L3 persona (members).
        let memory = app.state.memory.as_deref().expect("memory wired");
        let operator = kura_memory::Actor {
            kind: kura_memory::ActorKind::Operator,
            id: "op".to_string(),
        };
        let (l1, _) = memory
            .create(kura_memory::CreateAssetInput {
                kind: kura_memory::AssetKind::ChatMemory,
                layer: kura_memory::MemoryLayer::L1,
                owner: operator.clone(),
                visibility: kura_memory::Visibility::Private,
                atom_type: Some(kura_memory::AtomType::Preference),
                title: "language".to_string(),
                content: "replies in Chinese".to_string(),
                source_links: vec![kura_memory::SourceLink {
                    kind: kura_memory::SourceKind::Thread,
                    id: "thr_seed".to_string(),
                    ..kura_memory::SourceLink::default()
                }],
                ..kura_memory::CreateAssetInput::default()
            })
            .expect("seed L1 atom");
        let (l2, _) = memory
            .create(kura_memory::CreateAssetInput {
                kind: kura_memory::AssetKind::ChatMemory,
                layer: kura_memory::MemoryLayer::L2,
                owner: operator.clone(),
                visibility: kura_memory::Visibility::Private,
                title: "workflow".to_string(),
                content: "communication preferences".to_string(),
                member_asset_ids: vec![l1.asset_id.clone()],
                ..kura_memory::CreateAssetInput::default()
            })
            .expect("seed L2 scenario");
        let (asset, _) = memory
            .create(kura_memory::CreateAssetInput {
                kind: kura_memory::AssetKind::ChatMemory,
                layer: kura_memory::MemoryLayer::L3,
                owner: operator,
                visibility: kura_memory::Visibility::Private,
                title: "persona".to_string(),
                content: "Chinese-speaking operator".to_string(),
                member_asset_ids: vec![l2.asset_id.clone()],
                ..kura_memory::CreateAssetInput::default()
            })
            .expect("seed L3 persona");

        let mut payload = serde_json::json!({
            "tenantId": "",
            "sourceKind": "chat",
            "provider": "echo",
            "model": "",
            "messages": [
                { "role": "system", "content": "frame" },
                { "role": "user", "content": "old history ".repeat(20) },
                { "role": "user", "content": "what language should replies use" }
            ]
        });
        let outcome = hooks.run(kura_plugin::points::CHAT_PRE_DISPATCH, &mut payload);
        assert!(outcome.allowed());
        let messages = payload["messages"].as_array().expect("messages");
        let bootstrap = messages
            .iter()
            .find(|m| {
                m["content"]
                    .as_str()
                    .is_some_and(|c| c.contains(&format!("Memory[l3 {}]", asset.asset_id)))
            })
            .expect("bootstrap injected with citation");
        assert_eq!(bootstrap["role"], "system");
        assert!(
            bootstrap["content"]
                .as_str()
                .unwrap()
                .contains("Chinese-speaking operator"),
            "content injected"
        );
        // Session-strategy still shaped the window afterwards: history
        // elided, bootstrap (system frame) survived, current query kept.
        assert!(messages.iter().any(|m| {
            m["content"]
                .as_str()
                .is_some_and(|c| c.contains("elided by the session-strategy"))
        }));
        assert_eq!(
            messages.last().unwrap()["content"],
            "what language should replies use"
        );
        // Query-time recall: the L1 atom lexically matching the query is
        // injected with its citation and the (recalled) marker.
        let recalled = messages
            .iter()
            .find(|m| {
                m["content"]
                    .as_str()
                    .is_some_and(|c| c.contains(&format!("Memory[l1 {}]", l1.asset_id)))
            })
            .unwrap_or_else(|| panic!("matching L1 atom recalled; messages: {messages:?}"));
        assert!(recalled["content"].as_str().unwrap().contains("(recalled)"));

        // The assembly decision is recorded as a context.assembled event.
        let events = app
            .state
            .store
            .lock()
            .list_events(&kura_events::Filter::default())
            .expect("list events");
        let assembled = events
            .iter()
            .find(|e| e.name == "context.assembled")
            .expect("context.assembled event recorded");
        assert_eq!(
            assembled.payload["record"]["included"][0]["assetId"],
            asset.asset_id
        );
        let record_included = assembled.payload["record"]["included"]
            .as_array()
            .expect("included array");
        assert!(
            record_included
                .iter()
                .any(|item| item["source"] == "retrieval" && item["assetId"] == l1.asset_id),
            "retrieval inclusion recorded: {record_included:?}"
        );
        app.close();
    }

    /// Stage 1.1 + 2.3: corpus vectors are computed once and persisted, so a
    /// turn embeds only the query; and a revoked asset's cached vector is
    /// cleared, because persisting the index removed the free guarantee that
    /// forgetting also clears derived state.
    #[tokio::test]
    async fn retrieval_caches_corpus_embeddings_and_revocation_clears_them() {
        let app = App::new(test_config()).expect("build app");
        let hooks = app.state.hooks.as_ref().expect("hook bus");
        let memory = app.state.memory.as_deref().expect("memory wired");
        let operator = kura_memory::Actor {
            kind: kura_memory::ActorKind::Operator,
            id: "op".to_string(),
        };
        let (atom, _) = memory
            .create(kura_memory::CreateAssetInput {
                kind: kura_memory::AssetKind::ChatMemory,
                layer: kura_memory::MemoryLayer::L1,
                owner: operator.clone(),
                visibility: kura_memory::Visibility::Private,
                atom_type: Some(kura_memory::AtomType::Preference),
                title: "language".to_string(),
                content: "replies in Chinese".to_string(),
                source_links: vec![kura_memory::SourceLink {
                    kind: kura_memory::SourceKind::Thread,
                    id: "thr_seed".to_string(),
                    ..kura_memory::SourceLink::default()
                }],
                ..kura_memory::CreateAssetInput::default()
            })
            .expect("seed L1 atom");
        assert_eq!(atom.status, kura_memory::AssetStatus::Ready);
        // The derived index has a foreign key onto memory_assets (that FK is
        // what makes deletion cascade), so the asset must be persisted the way
        // every production write path persists it.
        kura_api::routes::memory::persist_capture(&app.state, &atom);

        let fingerprint =
            kura_context::Embedder::fingerprint(&kura_context::HashedNgramEmbedder::default());
        let ids = vec![atom.asset_id.clone()];
        let cached = |app: &App| {
            app.state
                .store
                .lock()
                .get_memory_asset_embeddings(&fingerprint, &ids)
                .expect("read derived index")
        };

        // Nothing is embedded until a retrieval actually needs it.
        assert!(cached(&app).is_empty(), "index starts empty");

        let mut payload = serde_json::json!({
            "tenantId": "",
            "sourceKind": "chat",
            "provider": "echo",
            "model": "",
            "messages": [
                { "role": "user", "content": "what language should replies use" }
            ]
        });
        let outcome = hooks.run(kura_plugin::points::CHAT_PRE_DISPATCH, &mut payload);
        assert!(outcome.allowed());

        // One turn populated the derived index for the corpus.
        let after_first = cached(&app);
        assert_eq!(
            after_first.len(),
            1,
            "the corpus vector is persisted after the first turn"
        );
        let vector = after_first.get(&atom.asset_id).expect("vector").clone();
        assert!(!vector.is_empty());

        // A second turn reuses it rather than recomputing: the stored vector
        // is byte-identical and the recall still happens.
        let mut payload = serde_json::json!({
            "tenantId": "",
            "sourceKind": "chat",
            "provider": "echo",
            "model": "",
            "messages": [
                { "role": "user", "content": "what language should replies use" }
            ]
        });
        assert!(
            hooks
                .run(kura_plugin::points::CHAT_PRE_DISPATCH, &mut payload)
                .allowed()
        );
        let messages = payload["messages"].as_array().expect("messages");
        assert!(
            messages.iter().any(|m| m["content"]
                .as_str()
                .is_some_and(|c| c.contains(&format!("Memory[l1 {}]", atom.asset_id)))),
            "the cached vector still serves recall: {messages:?}"
        );
        assert_eq!(cached(&app).get(&atom.asset_id), Some(&vector));

        // ("a turn embeds exactly one text" is proved at the mechanism level
        // in kura-context: `retrieve_fused_with_vectors` never embeds a doc.
        // It cannot be proved by swapping `state.embedder` here, because the
        // hooks capture the AppState during `App::with_profile`.)

        // 2.3: revoking clears the derived row. Without this the persisted
        // vector would outlive the asset's recallability.
        let revoked = memory
            .revoke(&atom.asset_id, "operator forgot it")
            .expect("revoke");
        assert_eq!(revoked.status, kura_memory::AssetStatus::Revoked);
        kura_api::routes::memory::persist_capture(&app.state, &revoked);
        assert!(
            cached(&app).is_empty(),
            "revocation must clear the derived index"
        );
        app.close();
    }

    /// Stage 4.1 (tool plane): the production assembly wires the real
    /// billing-backed quota gate, not a permissive default.
    ///
    /// Roadmap 75 recorded that constructing the webhook manager with
    /// `quota: None -> AllowAllQuota` was "a real exposure, not just
    /// defense-in-depth". The tool plane spends money on third-party APIs, so
    /// this asserts the equivalent mistake is not present: the plane declares
    /// `billing` as a dependency and the runtime is built.
    #[tokio::test]
    async fn the_tool_plane_assembles_with_a_real_quota_gate() {
        let app = App::new(test_config()).expect("build app");

        assert!(app.state.tools.is_some(), "profile manager wired");
        assert!(
            app.state.tool_runtime.is_some(),
            "the guarded call path is wired, so no family can be invoked outside it"
        );

        let report = app.state.plugins.as_ref().expect("assembly report");
        let tools = report
            .plugins
            .iter()
            .find(|p| p.id == "tools")
            .expect("tools plugin in the assembly");
        assert!(tools.enabled, "{tools:?}");
        assert!(
            tools.requires.iter().any(|r| r == "billing"),
            "the tool plane must not assemble without the quota plane: {tools:?}"
        );
        app.close();
    }

    /// Stage 1.4: an L2/L3 asset that the bootstrap budget could not fit is
    /// still recallable when the query matches it, and an asset bootstrap
    /// already injected is not injected a second time.
    #[tokio::test]
    async fn retrieval_reaches_l2_beyond_the_bootstrap_budget_without_double_injecting() {
        let config = test_config();
        // A bootstrap budget small enough that only the newest asset fits.
        std::fs::write(
            std::path::Path::new(&config.data_dir).join(kura_plugin::PROFILE_FILE_NAME),
            serde_json::json!({
                "entries": { "context": { "config": { "memoryBudgetChars": 40 } } }
            })
            .to_string(),
        )
        .expect("write profile");
        let app = App::new(config).expect("build app");
        let memory = app.state.memory.as_deref().expect("memory wired");
        let operator = kura_memory::Actor {
            kind: kura_memory::ActorKind::Operator,
            id: "op".to_string(),
        };
        // An L2 scenario aggregates L1 atoms (the governance rule), so each
        // one is seeded through the real chain rather than fabricated.
        let mut scenario = |title: &str, content: &str| {
            let (atom, _) = memory
                .create(kura_memory::CreateAssetInput {
                    kind: kura_memory::AssetKind::ChatMemory,
                    layer: kura_memory::MemoryLayer::L1,
                    owner: operator.clone(),
                    visibility: kura_memory::Visibility::Private,
                    atom_type: Some(kura_memory::AtomType::Preference),
                    title: title.to_string(),
                    content: content.to_string(),
                    source_links: vec![kura_memory::SourceLink {
                        kind: kura_memory::SourceKind::Thread,
                        id: format!("thr_{title}"),
                        ..kura_memory::SourceLink::default()
                    }],
                    ..kura_memory::CreateAssetInput::default()
                })
                .expect("seed L1");
            memory
                .create(kura_memory::CreateAssetInput {
                    kind: kura_memory::AssetKind::ChatMemory,
                    layer: kura_memory::MemoryLayer::L2,
                    owner: operator.clone(),
                    visibility: kura_memory::Visibility::Private,
                    title: title.to_string(),
                    content: content.to_string(),
                    member_asset_ids: vec![atom.asset_id],
                    ..kura_memory::CreateAssetInput::default()
                })
                .expect("seed L2")
                .0
        };
        // Older, and the one the query matches — it will not fit the budget.
        let matching = scenario("deployment", "the operator deploys on Fridays");
        // Newer, unrelated: it takes the whole bootstrap budget.
        let newest = scenario("unrelated", "kitchen renovation notes");

        let mut payload = serde_json::json!({
            "tenantId": "",
            "sourceKind": "chat",
            "provider": "echo",
            "model": "",
            "messages": [
                { "role": "user", "content": "when does the operator deploy on Fridays" }
            ]
        });
        let hooks = app.state.hooks.as_ref().expect("hook bus");
        assert!(
            hooks
                .run(kura_plugin::points::CHAT_PRE_DISPATCH, &mut payload)
                .allowed()
        );
        let messages = payload["messages"].as_array().expect("messages");
        let rendered = messages
            .iter()
            .filter_map(|m| m["content"].as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            rendered.contains(&format!("Memory[l2 {}]", matching.asset_id)),
            "the budget-excluded L2 scenario is recalled by the query: {rendered}"
        );
        // The newest one fit the bootstrap budget; it must appear once, not
        // once from bootstrap and again from retrieval.
        let occurrences = rendered
            .matches(&format!("Memory[l2 {}]", newest.asset_id))
            .count();
        assert_eq!(occurrences, 1, "no double injection: {rendered}");

        let events = app
            .state
            .store
            .lock()
            .list_events(&kura_events::Filter::default())
            .expect("list events");
        let assembled = events
            .iter()
            .find(|e| e.name == "context.assembled")
            .expect("context.assembled recorded");
        let excluded = assembled.payload["record"]["excluded"]
            .as_array()
            .expect("excluded array");
        assert!(
            excluded
                .iter()
                .any(|item| item["assetId"] == newest.asset_id.as_str()
                    && item["reason"] == "already_injected"),
            "the skip is recorded, not silent: {excluded:?}"
        );
        app.close();
    }

    /// Stage 1's own verification requirement (audit F1): the retrieval
    /// benchmark at 10k Ready atoms. Cold turn embeds the whole corpus once;
    /// warm turn must reuse the persisted index and be no slower. Numbers are
    /// printed so the plan can record them; the assertion is the ordering.
    #[tokio::test]
    async fn retrieval_benchmark_10k_atoms_warm_turn_is_not_slower_than_cold() {
        let app = App::new(test_config()).expect("build app");
        let memory = app.state.memory.as_deref().expect("memory");
        let operator = kura_memory::Actor {
            kind: kura_memory::ActorKind::Operator,
            id: "op".into(),
        };
        let store = app.state.store.clone();
        for i in 0..10_000 {
            let (atom, _) = memory
                .create(kura_memory::CreateAssetInput {
                    kind: kura_memory::AssetKind::ChatMemory,
                    layer: kura_memory::MemoryLayer::L1,
                    owner: operator.clone(),
                    visibility: kura_memory::Visibility::Private,
                    atom_type: Some(kura_memory::AtomType::Preference),
                    title: format!("fact {i}"),
                    content: format!("the operator noted item number {i} about topic {}", i % 97),
                    source_links: vec![kura_memory::SourceLink {
                        kind: kura_memory::SourceKind::Thread,
                        id: "thr_bench".into(),
                        ..kura_memory::SourceLink::default()
                    }],
                    ..kura_memory::CreateAssetInput::default()
                })
                .expect("seed");
            store.lock().upsert_memory_asset(&atom).expect("persist");
        }
        let hooks = app.state.hooks.as_ref().expect("hooks");
        let turn = || {
            let mut payload = serde_json::json!({
                "tenantId": "", "sourceKind": "chat", "provider": "echo", "model": "",
                "messages": [{ "role": "user", "content": "what did the operator note about topic 42" }]
            });
            let start = std::time::Instant::now();
            assert!(
                hooks
                    .run(kura_plugin::points::CHAT_PRE_DISPATCH, &mut payload)
                    .allowed()
            );
            start.elapsed()
        };
        let cold = turn();
        let warm = turn();
        let warm2 = turn();
        let warm_best = warm.min(warm2);
        eprintln!("[bench] 10k atoms: cold={cold:?} warm={warm:?} warm2={warm2:?}");
        // The corpus is bounded to RETRIEVAL_MAX_CORPUS (2000) newest atoms,
        // so the index holds 2000 vectors after the cold turn.
        let fp = kura_context::Embedder::fingerprint(&kura_context::HashedNgramEmbedder::default());
        let n = store
            .lock()
            .count_memory_asset_embeddings_for_tenant("")
            .expect("count");
        assert_eq!(
            n,
            kura_context::RETRIEVAL_MAX_CORPUS as i64,
            "index size, fingerprint {fp}"
        );
        assert!(
            warm_best <= cold,
            "warm {warm_best:?} must not be slower than cold {cold:?}"
        );
        app.close();
    }

    /// Stage 5.1: a thread's frame is injected first and survives elision.
    #[tokio::test]
    async fn session_frame_is_injected_first_and_survives_a_tight_window() {
        let config = test_config();
        std::fs::write(
            std::path::Path::new(&config.data_dir).join(kura_plugin::PROFILE_FILE_NAME),
            serde_json::json!({
                "entries": { "session-strategy": { "config": { "personalBudgetChars": 120, "keepRecent": 1 } } }
            })
            .to_string(),
        )
        .expect("write profile");
        let app = App::new(config).expect("build app");
        {
            let store = app.state.store.lock();
            let frame = kura_session::SessionFrame {
                thread_id: "thr_frame".into(),
                goal: "ship the Q4 report".into(),
                constraints: vec!["cite sources".into()],
                ..kura_session::SessionFrame::default()
            };
            kura_store::put_document(
                &store,
                kura_session::DOC_KIND_SESSION_FRAME,
                "thr_frame",
                "",
                "",
                &frame,
            )
            .expect("persist frame");
        }
        let mut payload = serde_json::json!({
            "tenantId": "", "threadId": "thr_frame", "sourceKind": "chat", "provider": "echo", "model": "",
            "messages": [
                { "role": "user", "content": "old history ".repeat(30) },
                { "role": "assistant", "content": "old reply ".repeat(30) },
                { "role": "user", "content": "what next" }
            ]
        });
        let hooks = app.state.hooks.as_ref().expect("hooks");
        assert!(
            hooks
                .run(kura_plugin::points::CHAT_PRE_DISPATCH, &mut payload)
                .allowed()
        );
        let messages = payload["messages"].as_array().expect("messages");
        assert!(
            messages[0]["content"]
                .as_str()
                .unwrap()
                .starts_with("[session frame] goal: ship the Q4 report"),
            "frame first: {messages:?}"
        );
        assert!(
            messages.iter().any(|m| m["content"]
                .as_str()
                .is_some_and(|c| c.contains("elided by the session-strategy"))),
            "history was elided around it: {messages:?}"
        );
        assert_eq!(messages.last().unwrap()["content"], "what next");
        app.close();
    }

    /// Stage 5.2: an idle channel thread opens a new session segment through
    /// the real lifecycle reset; an active one does not.
    #[tokio::test]
    async fn idle_channel_thread_opens_a_new_segment_at_turn_start() {
        let config = test_config();
        std::fs::write(
            std::path::Path::new(&config.data_dir).join(kura_plugin::PROFILE_FILE_NAME),
            serde_json::json!({
                "entries": { "session-strategy": { "config": {
                    "channelSegmentation": { "*": { "idleGapSeconds": 3600 } }
                } } }
            })
            .to_string(),
        )
        .expect("write profile");
        let app = App::new(config).expect("build app");
        let now = chrono::Utc::now();
        let seed = |thread_id: &str, last_turn_age_seconds: i64| {
            let store = app.state.store.lock();
            store
                .upsert_thread(&kura_threads::Thread {
                    thread_id: thread_id.into(),
                    tenant_id: "ten_seg".into(),
                    lifecycle_state: kura_threads::LifecycleState::Active,
                    current_session_segment_id: format!("{thread_id}_seg1"),
                    source_kind: kura_threads::SourceKind::Channel,
                    source_summary: "Discord / #general".into(),
                    last_activity_at: now,
                    created_at: now,
                    updated_at: now,
                    retention_expires_at: Some(now + chrono::Duration::days(90)),
                    redaction_status: kura_threads::RedactionStatus::Redacted,
                })
                .expect("thread");
            store
                .upsert_thread_session_segment(&kura_threads::SessionSegment {
                    session_segment_id: format!("{thread_id}_seg1"),
                    thread_id: thread_id.into(),
                    tenant_id: "ten_seg".into(),
                    session_id: String::new(),
                    generation: 1,
                    state: "active".into(),
                    started_at: now,
                    ended_at: None,
                    last_active_at: now,
                    reset_from_session_segment_id: String::new(),
                    partial_evidence: false,
                })
                .expect("segment");
            let recorded = now - chrono::Duration::seconds(last_turn_age_seconds);
            store
                .save_continuity_turn(&kura_threads::ContinuityTurn {
                    continuity_turn_id: format!("{thread_id}_turn1"),
                    tenant_id: "ten_seg".into(),
                    thread_id: thread_id.into(),
                    session_segment_id: format!("{thread_id}_seg1"),
                    acceptance_sequence: 1,
                    role: kura_threads::ContinuityRole::User,
                    source_kind: kura_threads::SourceKind::Channel,
                    source_linkage_id: String::new(),
                    source_message_id: String::new(),
                    source_timestamp: Some(recorded),
                    dispatch_id: String::new(),
                    response_to_turn_id: String::new(),
                    safe_content: "earlier message".into(),
                    content_redaction_status: kura_threads::RedactionStatus::Redacted,
                    artifact_excerpt_refs: Vec::new(),
                    recorded_at: recorded,
                    retention_expires_at: Some(now + chrono::Duration::days(90)),
                    source_event_key: String::new(),
                })
                .expect("turn");
        };
        seed("thr_idle", 7200);
        seed("thr_busy", 60);

        let hooks = app.state.hooks.as_ref().expect("hooks");
        let run = |thread_id: &str| {
            let mut payload = serde_json::json!({
                "tenantId": "ten_seg", "threadId": thread_id, "query": "hello again",
                "sourceKind": "channel", "channelScopeRef": "discord-main:general",
                "sourceTimestamp": now.to_rfc3339(),
            });
            assert!(
                hooks
                    .run(kura_plugin::points::CHAT_TURN_START, &mut payload)
                    .allowed()
            );
            app.state
                .store
                .lock()
                .get_thread_for_tenant("ten_seg", thread_id)
                .expect("get")
                .expect("thread")
        };
        let idle = run("thr_idle");
        assert_ne!(
            idle.current_session_segment_id, "thr_idle_seg1",
            "idle thread opened a new segment"
        );
        assert_eq!(idle.lifecycle_state, kura_threads::LifecycleState::Reset);
        let busy = run("thr_busy");
        assert_eq!(
            busy.current_session_segment_id, "thr_busy_seg1",
            "active thread keeps its segment"
        );

        // The boundary is an audited lifecycle action, same as an operator reset.
        let actions = app
            .state
            .store
            .lock()
            .list_thread_lifecycle_actions("ten_seg", "thr_idle", now)
            .expect("actions");
        assert!(
            actions
                .iter()
                .any(|a| a.reason_code == "idle_gap_segmentation"),
            "{actions:?}"
        );
        app.close();
    }

    /// Stage 4: a fan-out runs each goal as a real chat turn on its own
    /// thread, records a structured result per child, and publishes progress
    /// events. Verified against the real assembly with the echo provider.
    #[tokio::test]
    async fn swarm_runs_children_concurrently_with_structured_results_and_events() {
        let config = test_config();
        std::fs::write(
            std::path::Path::new(&config.data_dir).join(kura_plugin::PROFILE_FILE_NAME),
            serde_json::json!({ "entries": { "swarm": { "config": { "enabled": true, "maxConcurrentChildren": 2 } } } }).to_string(),
        ).expect("write profile");
        // Single-user assembly for this test: no bearer auth, no tenant, so
        // the per-child gate takes its recorded "nothing to meter" path and
        // the children run as ordinary local turns.
        let mut app = App::new(config).expect("build app");
        assert!(
            app.state.swarm_quota.is_some(),
            "billing-backed child quota gate wired"
        );
        app.state.auth = None;
        // Proof that each child is a hooked turn: a counting pre-dispatch
        // hook registered before the launch must fire once per child.
        struct Count(std::sync::atomic::AtomicUsize);
        impl kura_plugin::Hook for Count {
            fn handle(&self, _p: &mut serde_json::Value) -> kura_plugin::HookOutcome {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                kura_plugin::HookOutcome::Continue
            }
        }
        let counter = Arc::new(Count(std::sync::atomic::AtomicUsize::new(0)));
        app.state.hooks.as_ref().expect("hooks").register(
            kura_plugin::points::CHAT_PRE_DISPATCH,
            "swarm-test",
            counter.clone(),
        );
        let app = Arc::new(app);

        let request = Request::builder()
            .method("POST")
            .uri("/v1/swarm/runs")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                serde_json::json!({
                    "goals": ["summarise A", "summarise B", "summarise C"], "provider": "echo"
                })
                .to_string(),
            ))
            .expect("request");
        let response = app.router().oneshot(request).await.expect("oneshot");
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let run: serde_json::Value = serde_json::from_slice(&body).expect("json");
        let run_id = run["runId"].as_str().expect("runId").to_string();
        assert_eq!(run["status"], "queued");
        assert_eq!(run["children"].as_array().unwrap().len(), 3);

        let manager = app.state.swarm.clone().expect("swarm");
        let mut done = None;
        for _ in 0..200 {
            if let Some(r) = manager.get(&run_id) {
                if !matches!(
                    r.status,
                    kura_swarm::RunStatus::Queued | kura_swarm::RunStatus::Running
                ) {
                    done = Some(r);
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let run = done.expect("run finished within 10s");
        assert_eq!(run.status, kura_swarm::RunStatus::Completed, "{run:?}");
        for child in &run.children {
            assert_eq!(
                child.status,
                kura_swarm::ChildStatus::Completed,
                "{child:?}"
            );
            assert!(
                !child.dispatch_id.is_empty(),
                "each child persisted a dispatch"
            );
            assert!(
                child.output_preview.contains(&child.goal),
                "echo output carries the goal: {child:?}"
            );
            assert_eq!(
                child.thread_id,
                format!("swarm:{run_id}:{}", child.index),
                "own thread per child"
            );
        }

        let events = app
            .state
            .store
            .lock()
            .list_events(&kura_events::Filter::default())
            .expect("events");
        let names: Vec<&str> = events
            .iter()
            .filter(|e| e.category == "swarm")
            .map(|e| e.name.as_str())
            .collect();
        assert!(
            names.contains(&"swarm.run_queued") && names.contains(&"swarm.run_started"),
            "{names:?}"
        );
        assert_eq!(
            names
                .iter()
                .filter(|n| **n == "swarm.child_completed")
                .count(),
            3,
            "{names:?}"
        );
        assert!(names.contains(&"swarm.run_completed"), "{names:?}");

        // Each child went through the chat hooks (model-visible = logged per
        // child): the pre-dispatch hook fired once per child.
        assert_eq!(counter.0.load(std::sync::atomic::Ordering::SeqCst), 3);

        let request = Request::builder()
            .uri(format!("/v1/swarm/runs/{run_id}"))
            .body(axum::body::Body::empty())
            .expect("request");
        let response = app.router().oneshot(request).await.expect("oneshot");
        assert_eq!(response.status(), StatusCode::OK);
        app.close();
    }

    /// Opt-in, not default-on: with no profile entry every launch is refused.
    #[tokio::test]
    async fn swarm_is_refused_until_opted_in() {
        let mut app = App::new(test_config()).expect("build app");
        app.state.auth = None;
        let request = Request::builder()
            .method("POST")
            .uri("/v1/swarm/runs")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(r#"{"goals":["a"]}"#))
            .expect("request");
        let response = app.router().oneshot(request).await.expect("oneshot");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        app.close();
    }

    /// Stage 3.1: distillation runs as a visible chat turn on its own thread
    /// and lands a Pending proposal in the review queue. A canned provider
    /// stands in for the model so the success path is exercised.
    #[tokio::test]
    async fn distillation_runs_as_a_visible_turn_and_lands_a_pending_proposal() {
        // The echo provider returns the prompt, and guidance is placed first in
        // it — so a corrected draft given as guidance is exactly what the model
        // 'answers', which is also the real steering path.
        let mut app = App::new(test_config()).expect("build app");
        app.state.auth = None;
        let app = Arc::new(app);

        let request = Request::builder()
            .method("POST").uri("/v1/skills/proposals/distill").header("content-type", "application/json")
            .body(axum::body::Body::from(serde_json::json!({ "threadId": "thr_source", "provider": "echo", "guidance": "{\"name\": \"deploy-check\", \"description\": \"pre-deploy checklist\", \"body\": \"1. run tests\\n2. check CI\"}" }).to_string()))
            .expect("request");
        let response = app.router().oneshot(request).await.expect("oneshot");
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(json["distillThreadId"], "skill-distill:thr_source");
        assert!(
            !json["dispatchId"].as_str().unwrap().is_empty(),
            "the distillation is a persisted dispatch"
        );
        assert_eq!(json["proposal"]["asset"]["status"], "pending", "{json}");
        assert_eq!(json["proposal"]["asset"]["title"], "deploy-check");
        let links = json["proposal"]["asset"]["sourceLinks"].as_array().unwrap();
        assert!(
            links.iter().any(|l| l["id"] == "thr_source"),
            "evidence links the source thread: {links:?}"
        );
        assert!(
            links.iter().any(|l| l["id"] == "skill-distill:thr_source"),
            "and the distill thread"
        );

        // The distill thread is an ordinary conversation: its dispatch carries
        // the operator's guidance, so replying there steers a second pass.
        let dispatches = app
            .state
            .store
            .lock()
            .list_llm_dispatches()
            .expect("dispatches");
        let mine = dispatches
            .iter()
            .find(|d| d.dispatch_id == json["dispatchId"].as_str().unwrap())
            .expect("dispatch");
        assert!(
            mine.messages
                .iter()
                .any(|m| m.content.contains("Operator guidance: {"))
        );
        app.close();
    }

    /// Symbolic compression: an oversized non-frame message externalizes to
    /// an L0 memory ref (full content preserved) and the window keeps a
    /// preview plus the citation; the current query is never externalized.
    #[tokio::test]
    async fn context_plugin_externalizes_oversized_content() {
        let config = test_config();
        let app = App::new(config).expect("build app");
        let hooks = app.state.hooks.as_ref().expect("hook bus");
        let big = "log line ".repeat(1200); // > 8000 chars
        let mut payload = serde_json::json!({
            "tenantId": "",
            "threadId": "thr_refs",
            "sourceKind": "chat",
            "provider": "echo",
            "model": "",
            "messages": [
                { "role": "system", "content": "frame" },
                { "role": "user", "content": big },
                { "role": "user", "content": "current query" }
            ]
        });
        let outcome = hooks.run(kura_plugin::points::CHAT_PRE_DISPATCH, &mut payload);
        assert!(outcome.allowed());
        let messages = payload["messages"].as_array().expect("messages");
        let externalized = messages
            .iter()
            .find(|m| {
                m["content"]
                    .as_str()
                    .is_some_and(|c| c.contains("[externalized:"))
            })
            .expect("oversized message externalized");
        assert!(
            externalized["content"].as_str().unwrap().len() < 500,
            "window keeps only the preview + citation"
        );
        assert_eq!(
            messages.last().unwrap()["content"],
            "current query",
            "current query untouched"
        );
        let assets = app
            .state
            .store
            .lock()
            .list_all_memory_assets()
            .expect("list assets");
        let asset = assets
            .iter()
            .find(|asset| asset.title == "context_ref")
            .expect("ref asset persisted");
        assert!(
            asset.content.len() > 8000,
            "full content preserved in the ref"
        );
        assert!(asset.source_links.iter().any(|l| l.id == "thr_refs"));
        app.close();
    }

    /// The session-strategy builtin registers at chat/pre-dispatch and
    /// shapes an over-budget window with the operator's plugins.json
    /// config: frame preserved, oldest history elided behind a marker,
    /// current query kept.
    #[tokio::test]
    async fn session_strategy_shapes_window_with_profile_config() {
        let config = test_config();
        std::fs::write(
            std::path::Path::new(&config.data_dir).join(kura_plugin::PROFILE_FILE_NAME),
            serde_json::json!({
                "entries": {
                    "session-strategy": {
                        "config": { "personalBudgetChars": 60, "keepRecent": 1 }
                    }
                }
            })
            .to_string(),
        )
        .expect("write profile");
        let app = App::new(config).expect("build app");
        let report = app.state.plugins.as_ref().expect("report");
        assert!(report.enabled("session-strategy"));
        let hooks = app.state.hooks.as_ref().expect("hook bus");
        assert!(hooks.registrations().contains(&(
            "chat/pre-dispatch".to_string(),
            "session-strategy".to_string()
        )));

        let mut payload = serde_json::json!({
            "sourceKind": "chat",
            "threadId": "thr_evict",
            "provider": "echo",
            "model": "",
            "messages": [
                { "role": "system", "content": "persona frame" },
                { "role": "user", "content": "old ".repeat(30) },
                { "role": "assistant", "content": "older answer ".repeat(10) },
                { "role": "user", "content": "current query" }
            ]
        });
        let outcome = hooks.run(kura_plugin::points::CHAT_PRE_DISPATCH, &mut payload);
        assert!(outcome.allowed());
        let messages = payload["messages"].as_array().expect("messages");
        assert!(
            messages.iter().any(|m| m["content"] == "persona frame"),
            "frame preserved"
        );
        assert_eq!(
            messages.last().expect("last")["content"],
            "current query",
            "current query kept by the keep-recent floor"
        );
        assert!(
            messages
                .iter()
                .any(|m| m["content"].as_str().is_some_and(|c| c.contains("elided"))),
            "elision marker present: {messages:?}"
        );
        // Compression-to-memory: the elided span was captured as an L0 ref
        // linked to the thread, and the marker cites the captured asset.
        let marker = messages
            .iter()
            .find(|m| m["content"].as_str().is_some_and(|c| c.contains("elided")))
            .expect("marker");
        assert!(
            marker["content"]
                .as_str()
                .unwrap()
                .contains("captured as Memory[l0_ref mem_"),
            "marker cites the captured span: {marker:?}"
        );
        let assets = app
            .state
            .store
            .lock()
            .list_all_memory_assets()
            .expect("list memory assets");
        let span = assets
            .iter()
            .find(|asset| asset.title == "session_eviction")
            .expect("elided span captured to the memory plane");
        assert!(
            span.content.contains("old old"),
            "span holds the elided history"
        );
        assert!(
            span.source_links.iter().any(|link| link.id == "thr_evict"),
            "span links back to the thread"
        );
        app.close();
    }

    /// A malformed session-strategy config fails the boot loudly instead of
    /// silently running default budgets.
    #[tokio::test]
    async fn malformed_session_strategy_config_fails_boot() {
        let config = test_config();
        let mut entries = std::collections::BTreeMap::new();
        entries.insert(
            "session-strategy".to_string(),
            kura_plugin::PluginEntry {
                enabled: None,
                config: serde_json::json!({ "personalBudgetChars": "lots" })
                    .as_object()
                    .expect("object")
                    .clone(),
            },
        );
        let profile = kura_plugin::PluginProfile {
            disabled: vec![],
            entries,
        };
        match App::with_profile(config, profile) {
            Err(AppError::PluginProfile(message)) => {
                assert!(message.contains("session-strategy"), "{message}");
            }
            Err(other) => panic!("expected PluginProfile error, got {other}"),
            Ok(app) => {
                app.close();
                panic!("malformed config must fail the boot");
            }
        }
    }

    /// Seam-RPC slice 1: an external plugin declaring the
    /// `context.embedder` seam serves the embedding provider — embed calls
    /// round-trip over the process protocol, and failures fall back to the
    /// in-process default instead of breaking retrieval.
    #[test]
    fn external_plugin_serves_the_embedder_seam() {
        let config = test_config();
        let plugin_dir = std::path::Path::new(&config.data_dir).join("plugins/embedder");
        std::fs::create_dir_all(&plugin_dir).expect("mkdir plugin");
        std::fs::write(
            plugin_dir.join("run.sh"),
            concat!(
                "while read line; do printf '%s\\n' '{\"outcome\":\"continue\",",
                "\"payload\":{\"vector\":[0.5,0.5,0.0]}}'; done\n"
            ),
        )
        .expect("write script");
        std::fs::write(
            plugin_dir.join("manifest.json"),
            serde_json::json!({
                "id": "embedder",
                "summary": "external embedding provider",
                "seams": ["context.embedder"],
                "entry": {
                    "kind": "process",
                    "command": "/bin/sh",
                    "args": ["run.sh"],
                    "timeoutMs": 3000
                }
            })
            .to_string(),
        )
        .expect("write manifest");

        let app = App::new(config).expect("build app");
        let embedder = app.state.embedder.as_deref().expect("embedder seam served");
        assert_eq!(embedder.name(), "embedder");
        assert_eq!(embedder.embed("anything"), vec![0.5, 0.5, 0.0]);
        app.close();
    }

    /// Tier-2 end to end: an external plugin installed under
    /// <data_dir>/plugins/ appears in the assembly report as
    /// source=external, its hooks register on the bus, and its process
    /// rewrites the chat context through the real assembly.
    #[test]
    fn external_plugin_rewrites_chat_context_end_to_end() {
        let config = test_config();
        let plugin_dir = std::path::Path::new(&config.data_dir).join("plugins/external-window");
        std::fs::create_dir_all(&plugin_dir).expect("mkdir plugin");
        std::fs::write(
            plugin_dir.join("run.sh"),
            concat!(
                "while read line; do printf '%s\\n' '{\"outcome\":\"continue\",",
                "\"payload\":{\"query\":\"rewritten by external\"}}'; done\n"
            ),
        )
        .expect("write script");
        std::fs::write(
            plugin_dir.join("manifest.json"),
            serde_json::json!({
                "id": "external-window",
                "summary": "external session window",
                "requires": ["chat"],
                "hooks": [{ "point": "chat/turn-start", "onError": "veto" }],
                "entry": {
                    "kind": "process",
                    "command": "/bin/sh",
                    "args": ["run.sh"],
                    "timeoutMs": 3000
                }
            })
            .to_string(),
        )
        .expect("write manifest");

        let app = App::new(config).expect("build app");
        let report = app.state.plugins.as_ref().expect("report");
        let external = report
            .plugins
            .iter()
            .find(|p| p.id == "external-window")
            .expect("external plugin in report");
        assert!(external.enabled);
        assert_eq!(external.source, "external");
        let hooks = app.state.hooks.as_ref().expect("hook bus");
        assert!(
            hooks
                .registrations()
                .contains(&("chat/turn-start".to_string(), "external-window".to_string())),
            "external hook registered"
        );

        let chat = app.state.chat.clone().expect("chat wired");
        let execution = chat
            .query(
                kura_chat::QueryInput {
                    query: "original".to_string(),
                    provider: "echo".to_string(),
                    ..Default::default()
                },
                &kura_chat::CancellationToken::new(),
            )
            .expect("query through external plugin");
        assert_eq!(
            execution.result.query, "rewritten by external",
            "the external process rewrote the turn's query"
        );
        app.close();
    }

    /// Every seam adapter is wired into its manager by the builtin plugins
    /// (activation store/billing/chat, computeruse artifact recorder,
    /// execprofile sandbox health checker, evidence routine collector,
    /// delivery connector adapter, mcp execution starter + secret resolver).
    #[test]
    fn wired_seam_adapters_are_non_none() {
        let config = test_config();
        let app = App::new(config).expect("build app");
        let wiring = &app.wiring;
        assert!(
            wiring.activation_store.is_some(),
            "activation StateStore/IdentityRepository/AuditSink (SqliteActivationStore)"
        );
        assert!(
            wiring.activation_billing.is_some(),
            "activation BillingProjector"
        );
        assert!(wiring.activation_chat.is_some(), "activation ChatRunner");
        assert!(
            wiring.computeruse_recorder.is_some(),
            "computeruse ArtifactRecorder"
        );
        assert!(
            wiring.execprofile_health.is_some(),
            "execprofile HealthChecker"
        );
        assert!(wiring.evidence_collector.is_some(), "evidence Collector");
        assert!(
            wiring.delivery_connector.is_some(),
            "delivery ConnectorAdapter"
        );
        assert!(wiring.mcp_starter.is_some(), "mcp AttachedExecutionStarter");
        assert!(wiring.mcp_secret_resolver.is_some(), "mcp SecretResolver");
        app.close();
    }
}

#[cfg(test)]
mod tool_host_tests {
    //! Stage 9.0b/9.2 against the real assembly: the model asks for a tool,
    //! the assembly's tool source answers from the real memory / tool planes
    //! through the `kura-core` loop.

    use std::sync::Arc;

    use futures::future::BoxFuture;
    use kura_chat::{CancellationToken, QueryInput, ToolSource as _, ToolTurn};
    use kura_config::{Config, Environment};
    use kura_core::ToolInvocation;
    use kura_llm::{
        MessageRole, Provider, ProviderError, ProviderRequest, ProviderResponse, StreamEmitter,
        Usage,
    };

    use crate::App;

    fn test_config() -> Config {
        let dir = std::env::temp_dir().join(format!("kura-app-tools-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("create temp data dir");
        Config {
            environment: Environment::Test,
            bind_addr: "127.0.0.1:0".to_string(),
            data_dir: dir.to_string_lossy().into_owned(),
            log_level: "info".to_string(),
            version: "dev".to_string(),
            // A configured openai_compatible profile so the providers manager
            // resolves the name; the test then swaps the registered provider
            // for a scripted one under that same name.
            llm: kura_config::LlmConfig {
                default_provider: "openai_compatible".to_string(),
                default_model: "m".to_string(),
                openai_compatible: kura_config::OpenAiCompatibleProviderConfig {
                    base_url: "https://llm.example.org/v1".to_string(),
                    api_key: "test-key".to_string(),
                    model: "m".to_string(),
                    ..Default::default()
                },
                ..Default::default()
            },
            connectors: Default::default(),
            egress: Default::default(),
            store: Default::default(),
            project_root: String::new(),
        }
    }

    /// Asks for `tool` with `arguments` once, then answers with the tool
    /// result it was shown.
    struct AskOnce {
        tool: String,
        arguments: String,
        requests: parking_lot::RwLock<Vec<ProviderRequest>>,
    }

    impl AskOnce {
        fn respond(&self, request: &ProviderRequest) -> ProviderResponse {
            self.requests.write().push(request.clone());
            let offered: Vec<&str> = request.tools.iter().map(|t| t.name.as_str()).collect();
            let answered = request.messages.iter().any(|m| m.role == MessageRole::Tool);
            if offered.contains(&self.tool.as_str()) && !answered {
                return ProviderResponse {
                    output: String::new(),
                    finish_reason: "tool_calls".into(),
                    usage: Usage::default(),
                    tool_calls: vec![kura_llm::ToolCall {
                        call_id: "c1".into(),
                        name: self.tool.clone(),
                        arguments: self.arguments.clone(),
                    }],
                };
            }
            let seen = request
                .messages
                .iter()
                .rev()
                .find(|m| m.role == MessageRole::Tool)
                .map(|m| m.content.clone())
                .unwrap_or_else(|| format!("no tool; offered {offered:?}"));
            ProviderResponse {
                output: format!("final: {seen}"),
                finish_reason: "stop".into(),
                usage: Usage::default(),
                tool_calls: Vec::new(),
            }
        }
    }

    impl Provider for AskOnce {
        fn name(&self) -> &str {
            "openai_compatible"
        }
        fn complete<'a>(
            &'a self,
            request: ProviderRequest,
        ) -> BoxFuture<'a, Result<ProviderResponse, ProviderError>> {
            Box::pin(async move { Ok(self.respond(&request)) })
        }
        fn stream<'a>(
            &'a self,
            request: ProviderRequest,
            _emit: StreamEmitter<'a>,
        ) -> BoxFuture<'a, Result<ProviderResponse, ProviderError>> {
            Box::pin(async move { Ok(self.respond(&request)) })
        }
    }

    fn app_with_provider(tool: &str, arguments: &str) -> (App, Arc<AskOnce>) {
        let app = App::new(test_config()).expect("build app");
        let provider = Arc::new(AskOnce {
            tool: tool.to_string(),
            arguments: arguments.to_string(),
            requests: parking_lot::RwLock::new(Vec::new()),
        });
        app.state
            .llm
            .as_ref()
            .expect("llm")
            .register_provider(provider.clone());
        (app, provider)
    }

    fn query(text: &str) -> QueryInput {
        QueryInput {
            query: text.to_string(),
            provider: "openai_compatible".to_string(),
            model: "m".to_string(),
            ..QueryInput::default()
        }
    }

    fn source_for(app: &App) -> crate::tool_host::AppTools {
        let late = Arc::new(std::sync::OnceLock::new());
        let _ = late.set(app.state.clone());
        crate::tool_host::AppTools::new(late)
    }

    fn call(app: &App, name: &str, arguments: &str) -> kura_core::ToolOutput {
        let registry = source_for(app).registry(&ToolTurn::default());
        let tool = registry
            .get(name)
            .unwrap_or_else(|| panic!("tool {name} offered"));
        futures::executor::block_on(tool.call(&ToolInvocation {
            call_id: "c".into(),
            name: name.into(),
            arguments: arguments.into(),
        }))
        .expect("tool call returns an output, never an error")
    }

    fn offered(app: &App) -> Vec<String> {
        source_for(app)
            .registry(&ToolTurn::default())
            .specs()
            .into_iter()
            .map(|s| s.name)
            .collect()
    }

    /// 9.0b: the model recalls memory mid-turn through the same governed
    /// corpus the retrieval API exposes, and the reply cites it.
    #[test]
    fn memory_lookup_tool_recalls_ready_memory_mid_turn() {
        let (app, provider) =
            app_with_provider("memory.lookup", "{\"query\":\"package manager for web\"}");
        let memory = app.state.memory.as_deref().expect("memory");
        let operator = kura_memory::Actor {
            kind: kura_memory::ActorKind::Operator,
            id: "op".to_string(),
        };
        let (atom, _) = memory
            .create(kura_memory::CreateAssetInput {
                kind: kura_memory::AssetKind::ChatMemory,
                layer: kura_memory::MemoryLayer::L1,
                owner: operator.clone(),
                visibility: kura_memory::Visibility::Private,
                atom_type: Some(kura_memory::AtomType::Preference),
                title: "package manager".to_string(),
                content: "pnpm is the package manager for web projects".to_string(),
                source_links: vec![kura_memory::SourceLink {
                    kind: kura_memory::SourceKind::Thread,
                    id: "thr_seed".to_string(),
                    ..kura_memory::SourceLink::default()
                }],
                ..kura_memory::CreateAssetInput::default()
            })
            .expect("seed atom");
        assert_eq!(atom.status, kura_memory::AssetStatus::Ready);
        // A restricted asset must not be recallable by the tool.
        memory
            .create(kura_memory::CreateAssetInput {
                kind: kura_memory::AssetKind::ChatMemory,
                layer: kura_memory::MemoryLayer::L1,
                owner: operator,
                visibility: kura_memory::Visibility::Restricted,
                atom_type: Some(kura_memory::AtomType::Fact),
                title: "secret".to_string(),
                content: "restricted package manager note".to_string(),
                source_links: vec![kura_memory::SourceLink {
                    kind: kura_memory::SourceKind::Thread,
                    id: "thr_seed".to_string(),
                    ..kura_memory::SourceLink::default()
                }],
                ..kura_memory::CreateAssetInput::default()
            })
            .expect("seed restricted");

        let chat = app.state.chat.clone().expect("chat");
        let execution = chat
            .query(query("which package manager?"), &CancellationToken::new())
            .expect("query");
        assert!(execution.exec_error.is_none(), "{:?}", execution.exec_error);
        let output = &execution.result.dispatch.output;
        assert!(output.starts_with("final: [1] Memory[l1"), "{output}");
        assert!(
            output.contains(&atom.asset_id) && output.contains("pnpm"),
            "{output}"
        );
        assert!(!output.contains("restricted"), "{output}");
        // The first round offered the memory tool; the assembly built it.
        let first: Vec<String> = provider.requests.read()[0]
            .tools
            .iter()
            .map(|t| t.name.clone())
            .collect();
        assert!(first.contains(&"memory.lookup".to_string()), "{first:?}");
        assert!(
            !first.contains(&"web.search".to_string()),
            "no search profile configured"
        );
        // The call was audited.
        let events = app.state.event_bus.list(&kura_events::Filter {
            category: "chat".to_string(),
            ..kura_events::Filter::default()
        });
        assert!(
            events
                .iter()
                .any(|e| e.name == "chat.tool.called" && e.payload["name"] == "memory.lookup")
        );
    }

    /// 9.2 mechanism through the real source: a Ready `web.search` profile is
    /// offered by capability name and runs through `ToolRuntime`; the stub
    /// family proves the path with no vendor account.
    #[test]
    fn a_ready_search_profile_is_offered_and_called_through_the_runtime() {
        let (app, provider) = app_with_provider("web.search", "{\"query\":\"kura daemon\"}");
        let tools = app.state.tools.clone().expect("tools");
        tools
            .create(
                "",
                kura_tools::CreateProfileInput {
                    title: "stub search".into(),
                    capability: kura_tools::Capability::WebSearch,
                    family: kura_tools::Family::BuiltinStub,
                    auth_mode: kura_tools::AuthMode::None,
                    is_default: Some(true),
                    ..kura_tools::CreateProfileInput::default()
                },
            )
            .expect("create profile");
        let chat = app.state.chat.clone().expect("chat");
        let execution = chat
            .query(query("search for kura"), &CancellationToken::new())
            .expect("query");
        assert!(execution.exec_error.is_none());
        let output = &execution.result.dispatch.output;
        assert!(output.contains("stub result 1 for kura daemon"), "{output}");
        let first: Vec<String> = provider.requests.read()[0]
            .tools
            .iter()
            .map(|t| t.name.clone())
            .collect();
        assert!(first.contains(&"web.search".to_string()), "{first:?}");
    }

    /// The Stage 9 verification bar: a tool call meets a quota denial and a
    /// policy denial, and both reach the model as failed calls — never as a
    /// silent skip, never as a call that ran anyway.
    #[test]
    fn a_tool_call_meets_quota_and_policy_denials_as_failed_calls() {
        struct DenyAll;
        impl kura_tools::QuotaGate for DenyAll {
            fn reserve(
                &self,
                _tenant_id: &str,
                _profile: &kura_tools::ToolProfile,
            ) -> kura_tools::QuotaDecision {
                kura_tools::QuotaDecision::Denied {
                    reason_code: "quota_exhausted".to_string(),
                    detail: "daily tool calls used".to_string(),
                }
            }
            fn commit(&self, _tenant_id: &str, _operation_key: &str) {}
            fn release(&self, _tenant_id: &str, _operation_key: &str, _reason: &str) {}
        }
        let mut app = App::new(test_config()).expect("build app");
        app.state.tool_runtime = Some(Arc::new(kura_tools::ToolRuntime::new(
            Arc::new(DenyAll),
            kura_egress::EgressPolicy::default(),
        )));
        app.state
            .tools
            .clone()
            .expect("tools")
            .create(
                "",
                kura_tools::CreateProfileInput {
                    title: "stub search".into(),
                    capability: kura_tools::Capability::WebSearch,
                    family: kura_tools::Family::BuiltinStub,
                    auth_mode: kura_tools::AuthMode::None,
                    is_default: Some(true),
                    ..kura_tools::CreateProfileInput::default()
                },
            )
            .expect("create profile");
        // Quota denial: the runtime refuses before the provider runs.
        let outcome = call(&app, "web.search", "{\"query\":\"x\"}");
        assert!(!outcome.success);
        assert!(
            outcome.content.contains("quota denied: quota_exhausted"),
            "{}",
            outcome.content
        );

        // Policy denial: a `chat/tool-call` hook vetoes; the inner tool never runs.
        struct Veto;
        impl kura_plugin::Hook for Veto {
            fn handle(&self, payload: &mut serde_json::Value) -> kura_plugin::HookOutcome {
                assert_eq!(payload["name"], "memory.lookup");
                kura_plugin::HookOutcome::Halt("no recall in this workspace".to_string())
            }
        }
        app.state.hooks.clone().expect("hooks").register(
            kura_plugin::points::CHAT_TOOL_CALL,
            "policy-test",
            Arc::new(Veto),
        );
        let outcome = call(&app, "memory.lookup", "{\"query\":\"anything\"}");
        assert!(!outcome.success);
        assert!(
            outcome.content.contains("vetoed by plugin policy-test"),
            "{}",
            outcome.content
        );
    }

    /// An `mcp_backed` profile whose server is unknown is not offered: the
    /// model must not be shown a tool that cannot be called.
    #[test]
    fn an_mcp_backed_profile_without_a_live_server_is_not_offered() {
        let app = App::new(test_config()).expect("build app");
        app.state
            .tools
            .clone()
            .expect("tools")
            .create(
                "",
                kura_tools::CreateProfileInput {
                    title: "my search".into(),
                    capability: kura_tools::Capability::WebSearch,
                    family: kura_tools::Family::McpBacked,
                    auth_mode: kura_tools::AuthMode::None,
                    mcp_server_id: "mcp_missing".into(),
                    mcp_tool_name: "search".into(),
                    is_default: Some(true),
                    ..kura_tools::CreateProfileInput::default()
                },
            )
            .expect("create profile");
        let names = offered(&app);
        assert!(!names.contains(&"web.search".to_string()), "{names:?}");
        assert!(names.contains(&"memory.lookup".to_string()), "{names:?}");
    }
}

#[cfg(test)]
mod browser_driver_tests {
    //! Stage 9.4 wiring: `entries.computer-use.config.driver = "subprocess"`
    //! puts a supervised worker behind the computer-use manager and its
    //! health lands in the capability supervisor.

    use kura_config::{Config, Environment};

    use crate::App;

    fn test_config() -> Config {
        let dir = std::env::temp_dir().join(format!("kura-app-browser-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("create temp data dir");
        Config {
            project_root: Default::default(),
            environment: Environment::Test,
            bind_addr: "127.0.0.1:0".to_string(),
            data_dir: dir.to_string_lossy().into_owned(),
            log_level: "info".to_string(),
            version: "dev".to_string(),
            llm: Default::default(),
            connectors: Default::default(),
            egress: Default::default(),
            store: kura_config::StoreConfig { readers: 2 },
        }
    }

    /// A protocol-conformant worker in POSIX sh: answers every request
    /// `ok:true` with the session echoed back as active. Enough to prove the
    /// wiring; the driver's own supervision tests use the Rust fake worker.
    fn write_sh_worker(dir: &std::path::Path) -> std::path::PathBuf {
        let path = dir.join("worker.sh");
        std::fs::write(
            &path,
            r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  session=$(printf '%s' "$line" | sed -n 's/.*"session":\({[^}]*}\).*/\1/p')
  session=$(printf '%s' "$session" | sed 's/"status":"[a-z_]*"/"status":"active"/')
  printf '{"id":%s,"ok":true,"session":%s}\n' "$id" "$session"
done
"#,
        )
        .expect("write worker");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        path
    }

    #[test]
    fn subprocess_driver_is_wired_from_plugin_config_and_reports_to_the_supervisor() {
        let config = test_config();
        let worker = write_sh_worker(std::path::Path::new(&config.data_dir));
        std::fs::write(
            std::path::Path::new(&config.data_dir).join(kura_plugin::PROFILE_FILE_NAME),
            serde_json::json!({ "entries": { "computer-use": { "config": {
                "driver": "subprocess",
                "command": "/bin/sh",
                "args": [worker.to_string_lossy()],
                "timeoutMs": 5000
            } } } })
            .to_string(),
        )
        .expect("write profile");
        let app = App::new(config).expect("build app");

        let capabilities = app.state.capabilities.clone().expect("supervisor");

        let runtime = app.state.runtime.clone().expect("runtime");
        let run = runtime
            .create_run(kura_runtime::CreateRunInput {
                entrypoint: "chat".to_string(),
                ..kura_runtime::CreateRunInput::default()
            })
            .expect("create run");
        // Sessions reference runs by foreign key; the API persists runs, the
        // in-memory runtime alone does not.
        app.state
            .store
            .lock()
            .upsert_run(&run)
            .expect("persist run");
        let manager = app.state.computer_use.clone().expect("computer use");
        let session = manager
            .create_session(
                &run.run_id,
                &kura_computeruse::CreateSessionInput {
                    initial_url: "https://example.org".to_string(),
                    ..kura_computeruse::CreateSessionInput::default()
                },
            )
            .expect("session through the worker");
        assert_eq!(session.status, kura_computeruse::SessionStatus::Active);
        // Boot-time restore replaces the supervisor registry, so the worker
        // registers on its first report; the healthy answer lands there.
        let browser = capabilities
            .get("browser")
            .expect("browser worker registered");
        assert_eq!(browser.kind, "browser_worker");
        assert_eq!(
            browser.status,
            kura_capabilities::Status::Healthy,
            "a healthy answer is reported to the supervisor"
        );
    }

    #[test]
    fn an_unknown_driver_name_fails_the_build_loudly() {
        let config = test_config();
        std::fs::write(
            std::path::Path::new(&config.data_dir).join(kura_plugin::PROFILE_FILE_NAME),
            serde_json::json!({ "entries": { "computer-use": { "config": { "driver": "chrome" } } } })
                .to_string(),
        )
        .expect("write profile");
        let err = match App::new(config) {
            Ok(_) => panic!("unknown driver must fail the build"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("unknown driver"), "{err}");
    }
}
