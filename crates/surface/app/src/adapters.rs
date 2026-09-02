//! Adapter implementations needed by the app wiring (port of the small adapter
//! structs defined in Go app.go / the api package).

use std::sync::Arc;

use kura_scheduler::WorkflowLauncher as _;
use kura_store::SQLiteStore;

// ---------------------------------------------------------------------------
// Identity store handle (Go store.SQLiteStore is the identity store; the
// Rust SQLiteStore is !Sync, so the API layer erases it behind the
// object-safe kura_identity::Store trait through this mutex handle).
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct IdentityStoreError(String);

fn identity_store_err(message: String) -> kura_identity::IdentityError {
    kura_identity::IdentityError::Store(Box::new(IdentityStoreError(message)))
}

/// Send + Sync handle over the shared SQLite store implementing
/// [`kura_identity::Store`], mirroring the erased-store pattern used by the
/// api test suite (TestIdentityStore).
pub struct IdentityStoreHandle(pub Arc<parking_lot::Mutex<SQLiteStore>>);

impl kura_identity::ResolverStore for IdentityStoreHandle {
    fn get_principal(
        &self,
        principal_id: &str,
    ) -> Result<Option<kura_identity::Principal>, kura_identity::IdentityError> {
        self.0
            .lock()
            .get_principal(principal_id)
            .map_err(identity_store_err)
    }
    fn get_tenant(
        &self,
        tenant_id: &str,
    ) -> Result<Option<kura_identity::Tenant>, kura_identity::IdentityError> {
        self.0
            .lock()
            .get_tenant(tenant_id)
            .map_err(identity_store_err)
    }
    fn list_memberships(
        &self,
        filter: &kura_identity::MembershipFilter,
    ) -> Result<Vec<kura_identity::Membership>, kura_identity::IdentityError> {
        self.0
            .lock()
            .list_memberships(filter)
            .map_err(identity_store_err)
    }
    fn list_token_tenant_grants(
        &self,
        token_id: &str,
    ) -> Result<Vec<kura_identity::TokenTenantGrant>, kura_identity::IdentityError> {
        self.0
            .lock()
            .list_token_tenant_grants(token_id)
            .map_err(identity_store_err)
    }
}

impl kura_identity::AuditStore for IdentityStoreHandle {
    fn append_tenant_audit_event(
        &self,
        event: kura_identity::TenantAuditEvent,
    ) -> Result<kura_identity::TenantAuditEvent, kura_identity::IdentityError> {
        self.0
            .lock()
            .append_tenant_audit_event(&event)
            .map_err(identity_store_err)
    }
}

impl kura_identity::Store for IdentityStoreHandle {
    fn upsert_tenant(
        &self,
        tenant: &kura_identity::Tenant,
    ) -> Result<(), kura_identity::IdentityError> {
        self.0
            .lock()
            .upsert_tenant(tenant)
            .map_err(identity_store_err)
    }
    fn upsert_principal(
        &self,
        principal: &kura_identity::Principal,
    ) -> Result<(), kura_identity::IdentityError> {
        self.0
            .lock()
            .upsert_principal(principal)
            .map_err(identity_store_err)
    }
    fn upsert_membership(
        &self,
        membership: &kura_identity::Membership,
    ) -> Result<(), kura_identity::IdentityError> {
        self.0
            .lock()
            .upsert_membership(membership)
            .map_err(identity_store_err)
    }
    fn upsert_tenant_invitation(
        &self,
        invitation: &kura_identity::TenantInvitation,
    ) -> Result<(), kura_identity::IdentityError> {
        self.0
            .lock()
            .upsert_tenant_invitation(invitation)
            .map_err(identity_store_err)
    }
    fn upsert_token_tenant_grant(
        &self,
        grant: &kura_identity::TokenTenantGrant,
    ) -> Result<(), kura_identity::IdentityError> {
        self.0
            .lock()
            .upsert_token_tenant_grant(grant)
            .map_err(identity_store_err)
    }
    fn list_tenants(
        &self,
        filter: &kura_identity::TenantFilter,
    ) -> Result<Vec<kura_identity::Tenant>, kura_identity::IdentityError> {
        self.0
            .lock()
            .list_tenants(filter)
            .map_err(identity_store_err)
    }
    fn list_principals(
        &self,
        filter: &kura_identity::PrincipalFilter,
    ) -> Result<Vec<kura_identity::Principal>, kura_identity::IdentityError> {
        self.0
            .lock()
            .list_principals(filter)
            .map_err(identity_store_err)
    }
    fn list_tenant_invitations(
        &self,
        filter: &kura_identity::InvitationFilter,
    ) -> Result<Vec<kura_identity::TenantInvitation>, kura_identity::IdentityError> {
        self.0
            .lock()
            .list_tenant_invitations(filter)
            .map_err(identity_store_err)
    }
    fn list_token_authorities(
        &self,
    ) -> Result<Vec<kura_identity::TokenAuthority>, kura_identity::IdentityError> {
        self.0
            .lock()
            .list_token_authorities()
            .map_err(identity_store_err)
    }
}
// ---------------------------------------------------------------------------
// Workflow launcher (Go api.NewScheduleWorkflowLauncher): launches a run in
// the runtime manager for scheduled workflows and reminders.
// ---------------------------------------------------------------------------

pub struct WorkflowLauncherImpl {
    runtime: Arc<kura_runtime::Manager>,
}

impl WorkflowLauncherImpl {
    #[must_use]
    pub fn new(runtime: Arc<kura_runtime::Manager>) -> Self {
        Self { runtime }
    }

    fn launch_run(
        &self,
        entrypoint: &str,
        goal: &str,
        schedule_id: &str,
        schedule_attempt_id: &str,
        reminder_id: &str,
        reminder_occurrence_id: &str,
    ) -> Result<String, String> {
        let run = self
            .runtime
            .create_run(kura_runtime::CreateRunInput {
                run_id: String::new(),
                session_id: String::new(),
                schedule_id: schedule_id.to_string(),
                schedule_attempt_id: schedule_attempt_id.to_string(),
                reminder_id: reminder_id.to_string(),
                reminder_occurrence_id: reminder_occurrence_id.to_string(),
                entrypoint: if entrypoint.trim().is_empty() {
                    "operator".to_string()
                } else {
                    entrypoint.to_string()
                },
                goal: goal.to_string(),
            })
            .map_err(|err| format!("launch run: {err}"))?;
        Ok(run.run_id)
    }
}

impl kura_scheduler::WorkflowLauncher for WorkflowLauncherImpl {
    fn launch_scheduled_workflow(
        &self,
        target: &kura_scheduler::WorkflowTarget,
        schedule_id: &str,
        schedule_attempt_id: &str,
    ) -> Result<kura_scheduler::WorkflowLaunchResult, String> {
        let goal = if target.workflow_goal.trim().is_empty() {
            target.run_goal.clone()
        } else {
            target.workflow_goal.clone()
        };
        let run_id = self.launch_run(
            &target.entrypoint,
            &goal,
            schedule_id,
            schedule_attempt_id,
            "",
            "",
        )?;
        Ok(kura_scheduler::WorkflowLaunchResult {
            run_id,
            workflow_id: String::new(),
            downstream_status: kura_scheduler::DownstreamStatus::Running,
        })
    }
}

impl kura_reminders::WorkflowLauncher for WorkflowLauncherImpl {
    fn launch_reminder_workflow(
        &self,
        cfg: &kura_reminders::WorkflowLaunchConfig,
        reminder_id: &str,
        occurrence_id: &str,
    ) -> Result<kura_reminders::WorkflowLaunchResult, String> {
        let goal = if cfg.workflow_goal.trim().is_empty() {
            cfg.run_goal.clone()
        } else {
            cfg.workflow_goal.clone()
        };
        let run_id = self.launch_run(&cfg.entrypoint, &goal, "", "", reminder_id, occurrence_id)?;
        Ok(kura_reminders::WorkflowLaunchResult {
            run_id,
            workflow_id: String::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// Webhook firer (Go webhookWorkflowFirer): fires a webhook target by
// launching a scheduled workflow through the launcher.
// ---------------------------------------------------------------------------

pub struct WebhookFirerImpl {
    launcher: Arc<WorkflowLauncherImpl>,
}

impl WebhookFirerImpl {
    #[must_use]
    pub fn new(launcher: Arc<WorkflowLauncherImpl>) -> Self {
        Self { launcher }
    }
}

impl kura_webhook::Firer for WebhookFirerImpl {
    fn fire(&self, endpoint: &kura_webhook::Endpoint, _payload: &[u8]) -> Result<String, String> {
        let target = kura_scheduler::WorkflowTarget {
            session_id: String::new(),
            entrypoint: "operator".to_string(),
            run_goal: String::new(),
            workflow_goal: endpoint.target_ref.clone(),
            calendar_action: None,
            mail_action: None,
        };
        let result = self.launcher.launch_scheduled_workflow(
            &target,
            &format!("webhook:{}", endpoint.webhook_id),
            "",
        )?;
        if result.run_id.is_empty() {
            Ok(result.workflow_id)
        } else {
            Ok(result.run_id)
        }
    }
}

// ---------------------------------------------------------------------------
// Routine scheduler adapter (Go *scheduler.Scheduler satisfies the routine
// builder Scheduler interface; the Rust scheduler crate does not implement
// kura_routine::Scheduler, so this local adapter performs the mapping).
// ---------------------------------------------------------------------------

pub struct RoutineSchedulerAdapter {
    inner: Arc<kura_scheduler::Scheduler>,
}

impl RoutineSchedulerAdapter {
    #[must_use]
    pub fn new(inner: Arc<kura_scheduler::Scheduler>) -> Self {
        Self { inner }
    }
}

fn to_scheduler_trigger_kind(
    kind: kura_routine::SchedulerTriggerKind,
) -> kura_scheduler::TriggerKind {
    match kind {
        kura_routine::SchedulerTriggerKind::Once => kura_scheduler::TriggerKind::Once,
        kura_routine::SchedulerTriggerKind::Cron => kura_scheduler::TriggerKind::Cron,
    }
}

fn to_routine_trigger_kind(
    kind: kura_scheduler::TriggerKind,
) -> kura_routine::SchedulerTriggerKind {
    match kind {
        kura_scheduler::TriggerKind::Once => kura_routine::SchedulerTriggerKind::Once,
        kura_scheduler::TriggerKind::Cron => kura_routine::SchedulerTriggerKind::Cron,
    }
}

fn to_scheduler_target_kind(kind: kura_routine::SchedulerTargetKind) -> kura_scheduler::TargetKind {
    match kind {
        kura_routine::SchedulerTargetKind::Run => kura_scheduler::TargetKind::Run,
        kura_routine::SchedulerTargetKind::Workflow => kura_scheduler::TargetKind::Workflow,
    }
}

fn to_routine_target_kind(kind: kura_scheduler::TargetKind) -> kura_routine::SchedulerTargetKind {
    match kind {
        kura_scheduler::TargetKind::Run => kura_routine::SchedulerTargetKind::Run,
        kura_scheduler::TargetKind::Workflow => kura_routine::SchedulerTargetKind::Workflow,
    }
}

fn to_scheduler_backoff(
    kind: kura_routine::SchedulerRetryBackoffKind,
) -> kura_scheduler::RetryBackoffKind {
    match kind {
        kura_routine::SchedulerRetryBackoffKind::Fixed => kura_scheduler::RetryBackoffKind::Fixed,
        kura_routine::SchedulerRetryBackoffKind::Exponential => {
            kura_scheduler::RetryBackoffKind::Exponential
        }
    }
}

fn to_routine_backoff(
    kind: kura_scheduler::RetryBackoffKind,
) -> kura_routine::SchedulerRetryBackoffKind {
    match kind {
        kura_scheduler::RetryBackoffKind::Fixed => kura_routine::SchedulerRetryBackoffKind::Fixed,
        kura_scheduler::RetryBackoffKind::Exponential => {
            kura_routine::SchedulerRetryBackoffKind::Exponential
        }
    }
}

fn to_scheduler_create_input(input: &kura_routine::CreateInput) -> kura_scheduler::CreateInput {
    kura_scheduler::CreateInput {
        trigger: kura_scheduler::Trigger {
            kind: to_scheduler_trigger_kind(input.trigger.kind),
            fire_at: input.trigger.fire_at,
            cron_expr: input.trigger.cron_expr.clone(),
            timezone: input.trigger.timezone.clone(),
            next_due_at: None,
        },
        target: kura_scheduler::Target {
            kind: to_scheduler_target_kind(input.target.kind),
            revision: 0,
            active: input.target.active,
            run: None,
            workflow: input.target.workflow.as_ref().map(|workflow| {
                kura_scheduler::WorkflowTarget {
                    session_id: String::new(),
                    entrypoint: workflow.entrypoint.clone(),
                    run_goal: String::new(),
                    workflow_goal: workflow.workflow_goal.clone(),
                    calendar_action: None,
                    mail_action: None,
                }
            }),
            summary: input.target.summary.clone(),
            updated_at: chrono::Utc::now(),
        },
        retry_policy: kura_scheduler::RetryPolicy {
            max_retries: input.retry_policy.max_retries,
            backoff_kind: to_scheduler_backoff(input.retry_policy.backoff_kind),
            base_delay_seconds: input.retry_policy.base_delay_seconds,
            max_delay_seconds: input.retry_policy.max_delay_seconds,
        },
    }
}

fn to_routine_schedule(schedule: kura_scheduler::Schedule) -> kura_routine::Schedule {
    kura_routine::Schedule {
        schedule_id: schedule.schedule_id,
        environment_scope: schedule.environment_scope,
        tenant_id: schedule.tenant_id,
        kind: match schedule.kind {
            kura_scheduler::ScheduleKind::OneTime => kura_routine::ScheduleKind::OneTime,
            kura_scheduler::ScheduleKind::Recurring => kura_routine::ScheduleKind::Recurring,
        },
        status: match schedule.status {
            kura_scheduler::ScheduleStatus::Scheduled => kura_routine::ScheduleStatus::Scheduled,
            kura_scheduler::ScheduleStatus::Active => kura_routine::ScheduleStatus::Active,
            kura_scheduler::ScheduleStatus::Paused => kura_routine::ScheduleStatus::Paused,
            kura_scheduler::ScheduleStatus::Cancelled => kura_routine::ScheduleStatus::Cancelled,
            kura_scheduler::ScheduleStatus::Completed => kura_routine::ScheduleStatus::Completed,
            kura_scheduler::ScheduleStatus::DispatchFailed => {
                kura_routine::ScheduleStatus::DispatchFailed
            }
        },
        target_ref_id: schedule.target_ref_id,
        trigger: kura_routine::SchedulerTrigger {
            kind: to_routine_trigger_kind(schedule.trigger.kind),
            fire_at: schedule.trigger.fire_at,
            cron_expr: schedule.trigger.cron_expr,
            timezone: schedule.trigger.timezone,
        },
        target: kura_routine::SchedulerTarget {
            kind: to_routine_target_kind(schedule.target.kind),
            active: schedule.target.active,
            workflow: schedule.target.workflow.map(|workflow| {
                kura_routine::SchedulerWorkflowTarget {
                    entrypoint: workflow.entrypoint,
                    workflow_goal: workflow.workflow_goal,
                }
            }),
            summary: schedule.target.summary,
        },
        retry_policy: kura_routine::SchedulerRetryPolicy {
            max_retries: schedule.retry_policy.max_retries,
            backoff_kind: to_routine_backoff(schedule.retry_policy.backoff_kind),
            base_delay_seconds: schedule.retry_policy.base_delay_seconds,
            max_delay_seconds: schedule.retry_policy.max_delay_seconds,
        },
        created_at: schedule.created_at,
        updated_at: schedule.updated_at,
    }
}

impl kura_routine::Scheduler for RoutineSchedulerAdapter {
    fn create(&self, input: &kura_routine::CreateInput) -> Result<kura_routine::Schedule, String> {
        let schedule = self
            .inner
            .create(to_scheduler_create_input(input))
            .map_err(|err| err.to_string())?;
        Ok(to_routine_schedule(schedule))
    }

    fn pause(&self, schedule_id: &str) -> Result<(kura_routine::Schedule, bool), String> {
        match self
            .inner
            .pause(schedule_id)
            .map_err(|err| err.to_string())?
        {
            Some(schedule) => Ok((to_routine_schedule(schedule), true)),
            None => Err("routine schedule not found".to_string()),
        }
    }

    fn resume(&self, schedule_id: &str) -> Result<(kura_routine::Schedule, bool), String> {
        match self
            .inner
            .resume(schedule_id)
            .map_err(|err| err.to_string())?
        {
            Some(schedule) => Ok((to_routine_schedule(schedule), true)),
            None => Err("routine schedule not found".to_string()),
        }
    }

    fn cancel(&self, schedule_id: &str) -> Result<(kura_routine::Schedule, bool), String> {
        match self
            .inner
            .cancel(schedule_id)
            .map_err(|err| err.to_string())?
        {
            Some(schedule) => Ok((to_routine_schedule(schedule), true)),
            None => Err("routine schedule not found".to_string()),
        }
    }

    fn get(&self, schedule_id: &str) -> Result<(kura_routine::Schedule, bool), String> {
        match self.inner.get(schedule_id).map_err(|err| err.to_string())? {
            Some(schedule) => Ok((to_routine_schedule(schedule), true)),
            None => Ok((kura_routine::Schedule::default(), false)),
        }
    }
}

// ---------------------------------------------------------------------------
// Tenant migration gate (Go api.MigrationStatus): the Rust port has no
// migration runner yet, so the gate reports no in-flight migration.
// ---------------------------------------------------------------------------

pub struct NoMigrationInProgress;

impl kura_api::state::MigrationStatus for NoMigrationInProgress {
    fn in_progress(&self) -> bool {
        false
    }

    fn pending_steps(&self) -> Vec<String> {
        Vec::new()
    }
}


// ---------------------------------------------------------------------------
// MCP secret resolver (Go mcp.SetSecretManager with the tenant secret
// manager): kura-mcp's SecretResolver seam is synchronous, while
// kura-secrets::Manager::resolve is async (its store/backend futures are
// plain sync work wrapped in BoxFuture), so the bridge blocks on a resolved
// default personal tenant. Mirrors App::resolve_connector_secret's
// tenant resolution.
// ---------------------------------------------------------------------------

pub struct McpSecretResolver {
    store: Arc<parking_lot::Mutex<SQLiteStore>>,
    secrets: Arc<kura_secrets::Manager>,
}

impl McpSecretResolver {
    #[must_use]
    pub fn new(
        store: Arc<parking_lot::Mutex<SQLiteStore>>,
        secrets: Arc<kura_secrets::Manager>,
    ) -> Self {
        Self { store, secrets }
    }
}

impl kura_mcp::SecretResolver for McpSecretResolver {
    fn resolve(&self, secret_ref: &str) -> Result<Option<String>, String> {
        let tenant_id = self
            .store
            .lock()
            .resolve_default_personal_tenant_id()
            .map_err(|err| format!("resolve default tenant for mcp secret: {err}"))?;
        if tenant_id.trim().is_empty() {
            return Ok(None);
        }
        let resolved = futures::executor::block_on(self.secrets.resolve(kura_secrets::ResolveInput {
            tenant_id,
            secret_ref: secret_ref.trim().to_string(),
        }))
        .map_err(|err| format!("resolve mcp secret {secret_ref}: {err}"))?;
        Ok(Some(resolved.value))
    }
}

// ---------------------------------------------------------------------------
// MCP attached-execution starter (Go attachedExecutionStarter): spawns the
// MCP server command through the kura-sandbox execution plane, which returns
// live stdin/stdout pipes plus a runner thread (stderr is drained into the
// sandbox capture buffer, so the stdio transport gets no stderr pipe — it
// tolerates that by skipping its own drain thread). The remaining trait
// methods forward to the sandbox manager.
// ---------------------------------------------------------------------------

pub struct McpExecutionStarter {
    sandbox: Arc<kura_sandbox::Manager>,
}

impl McpExecutionStarter {
    #[must_use]
    pub fn new(sandbox: Arc<kura_sandbox::Manager>) -> Self {
        Self { sandbox }
    }
}

impl kura_mcp::AttachedExecutionStarter for McpExecutionStarter {
    fn start_attached_execution(
        &self,
        request: &kura_sandbox::ExecutionRequest,
    ) -> Result<(kura_sandbox::Execution, Option<kura_mcp::AttachedExecution>), String> {
        let (execution, attached) = self
            .sandbox
            .start_attached_execution(request.clone())
            .map_err(|err| err.to_string())?;
        let attached = attached.map(|attached| kura_mcp::AttachedExecution {
            execution: attached.execution,
            stdin: attached
                .stdin
                .map(|stdin| Box::new(stdin) as Box<dyn std::io::Write + Send>),
            stdout: attached
                .stdout
                .map(|stdout| Box::new(stdout) as Box<dyn std::io::Read + Send>),
            stderr: None,
        });
        Ok((execution, attached))
    }

    fn cancel_execution(
        &self,
        execution_id: &str,
    ) -> Result<(kura_sandbox::Execution, bool), String> {
        self.sandbox
            .cancel_execution(execution_id)
            .map_err(|err| err.to_string())
    }

    fn get_execution(&self, execution_id: &str) -> Option<kura_sandbox::Execution> {
        self.sandbox.get_execution(execution_id)
    }

    fn persist_consumer_view(
        &self,
        view: &kura_sandbox::ConsumerContractView,
    ) -> Result<(), String> {
        self.sandbox
            .persist_consumer_view(view)
            .map_err(|err| err.to_string())
    }

    fn get_profile(&self, profile_id: &str) -> Option<kura_sandbox::Profile> {
        self.sandbox.get_profile(profile_id)
    }
}

// ---------------------------------------------------------------------------
// Roadmap 75: deferred hook wiring — webhook trigger quota gate and the
// catalog requirement/permission gates. These replace the permissive
// AllowAllQuota / AllMet / AllowAll defaults in the production assembly.
// ---------------------------------------------------------------------------

/// Webhook trigger quota gate backed by the billing plane (Roadmap 75).
///
/// Tenant-scoped triggers reserve one workflow-launch unit and commit it;
/// denials return the billing reason code and publish an auditable
/// `webhook.trigger_quota_denied` event (the billing repo also records the
/// durable QuotaDenial surfaced via /v1/billing/denials). Tenant-less local
/// triggers stay allowed by recorded decision: quota enforcement is a hosted,
/// per-tenant bound, and the local single-operator mode has no plan to charge.
pub struct WebhookQuotaGateImpl {
    billing: Arc<kura_billing::Manager>,
    store: Arc<parking_lot::Mutex<kura_store::SQLiteStore>>,
    events: Arc<kura_events::Bus>,
    environment_scope: String,
}

impl WebhookQuotaGateImpl {
    #[must_use]
    pub fn new(
        billing: Arc<kura_billing::Manager>,
        store: Arc<parking_lot::Mutex<kura_store::SQLiteStore>>,
        events: Arc<kura_events::Bus>,
        environment_scope: &str,
    ) -> Self {
        Self {
            billing,
            store,
            events,
            environment_scope: environment_scope.to_string(),
        }
    }

    fn publish_denied(&self, tenant_id: &str, webhook_id: &str, reason_code: &str) {
        let mut payload = serde_json::Map::new();
        payload.insert("tenantId".to_string(), serde_json::json!(tenant_id));
        payload.insert("webhookId".to_string(), serde_json::json!(webhook_id));
        payload.insert("reasonCode".to_string(), serde_json::json!(reason_code));
        let event = kura_events::Event {
            category: "webhook".to_string(),
            name: "webhook.trigger_quota_denied".to_string(),
            environment_scope: self.environment_scope.clone(),
            resource: kura_events::Resource {
                kind: "webhook".to_string(),
                id: webhook_id.to_string(),
            },
            payload,
            ..kura_events::Event::default()
        };
        if let Ok(stored) = self.store.lock().append_event(&event) {
            self.events.publish(stored);
        }
    }
}

impl kura_webhook::QuotaGate for WebhookQuotaGateImpl {
    fn allow(&self, tenant_id: &str, webhook_id: &str) -> (bool, String) {
        let tenant_id = tenant_id.trim();
        if tenant_id.is_empty() {
            return (true, String::new());
        }
        let operation_key = format!(
            "webhook_trigger:{webhook_id}:{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        );
        let reserve = futures::executor::block_on(self.billing.reserve(
            kura_billing::ReserveInput {
                tenant_id: tenant_id.to_string(),
                category: kura_billing::Category::WORKFLOW_LAUNCHES.into(),
                amount: 1,
                operation_key: operation_key.clone(),
                reservation_point: "webhook_trigger".to_string(),
                guarded_entry_point: "/v1/triggers/webhook".to_string(),
                actor_principal_id: String::new(),
                hosted: true,
            },
        ));
        match reserve {
            Ok(result) if result.allowed => {
                // The accepted trigger consumes the launch: commit the
                // reservation immediately (best-effort; a failed commit
                // resolves through the billing recovery sweep).
                let _ = futures::executor::block_on(self.billing.commit(
                    kura_billing::ResolveInput {
                        tenant_id: tenant_id.to_string(),
                        category: kura_billing::Category::WORKFLOW_LAUNCHES.into(),
                        operation_key,
                        amount: 1,
                        reason_code: "webhook_trigger_fired".to_string(),
                        reason: "webhook trigger accepted".to_string(),
                        actor_principal_id: String::new(),
                    },
                ));
                (true, String::new())
            }
            Ok(result) => {
                let reason_code = result
                    .denial
                    .map(|denial| denial.reason_code)
                    .filter(|reason| !reason.is_empty())
                    .unwrap_or_else(|| "quota_denied".to_string());
                self.publish_denied(tenant_id, webhook_id, &reason_code);
                (false, reason_code)
            }
            Err(err) => {
                // Fail closed for hosted tenants: an unavailable quota plane
                // must not grant unbounded webhook launches.
                self.publish_denied(tenant_id, webhook_id, "quota_state_unavailable");
                (false, format!("quota check failed: {err}"))
            }
        }
    }
}

/// Catalog requirement checker backed by the sandbox execution plane
/// (Roadmap 75, closing the spec 053 follow-on).
///
/// A requirement key (optionally prefixed `backend:`) is met when it names a
/// sandbox backend whose capability profile reports `available`; every other
/// key is unmet — the gate fails closed rather than guessing at
/// prerequisites the sandbox plane cannot attest.
pub struct CatalogSandboxRequirementChecker {
    sandbox: Arc<kura_sandbox::Manager>,
}

impl CatalogSandboxRequirementChecker {
    #[must_use]
    pub fn new(sandbox: Arc<kura_sandbox::Manager>) -> Self {
        Self { sandbox }
    }
}

impl kura_catalog::RequirementChecker for CatalogSandboxRequirementChecker {
    fn unmet(
        &self,
        _tenant_id: &str,
        requirements: &[kura_catalog::Requirement],
    ) -> Vec<kura_catalog::Requirement> {
        let backends = self.sandbox.backend_capabilities();
        requirements
            .iter()
            .filter(|requirement| {
                let key = requirement.key.trim();
                let key = key.strip_prefix("backend:").unwrap_or(key);
                !backends.iter().any(|backend| {
                    backend.backend_kind.as_str().eq_ignore_ascii_case(key)
                        && backend.availability_status
                            == kura_sandbox::BackendAvailabilityStatus::Available
                })
            })
            .cloned()
            .collect()
    }
}

/// Catalog permission gate backed by the identity plane (Roadmap 75).
///
/// Tenant-less local enablement stays allowed (Go single-operator
/// semantics). A tenant-scoped enablement requires the tenant to exist with
/// an Active lifecycle status; unknown or non-active tenants are denied —
/// fail closed. Finer-grained permission-string checks stay with the
/// authoritative protected() route middleware, which resolves the caller's
/// token permissions.
pub struct CatalogTenantPermissionGate {
    store: Arc<parking_lot::Mutex<kura_store::SQLiteStore>>,
}

impl CatalogTenantPermissionGate {
    #[must_use]
    pub fn new(store: Arc<parking_lot::Mutex<kura_store::SQLiteStore>>) -> Self {
        Self { store }
    }
}

impl kura_catalog::PermissionGate for CatalogTenantPermissionGate {
    fn allow(&self, tenant_id: &str, _permissions: &[String]) -> bool {
        let tenant_id = tenant_id.trim();
        if tenant_id.is_empty() {
            return true;
        }
        match self.store.lock().get_tenant(tenant_id) {
            Ok(Some(tenant)) => tenant.status == kura_identity::LifecycleStatus::Active,
            _ => false,
        }
    }
}

#[cfg(test)]
mod hook_wiring_tests {
    use super::*;
    use kura_catalog::{PermissionGate as _, RequirementChecker as _};
    use kura_webhook::QuotaGate as _;

    fn test_store() -> Arc<parking_lot::Mutex<kura_store::SQLiteStore>> {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "kura-hook-wiring-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        Arc::new(parking_lot::Mutex::new(
            kura_store::SQLiteStore::new(dir.to_str().expect("path")).expect("store"),
        ))
    }

    fn test_sandbox(store: &Arc<parking_lot::Mutex<kura_store::SQLiteStore>>) -> Arc<kura_sandbox::Manager> {
        let config = kura_config::Config {
            environment: kura_config::Environment::Test,
            bind_addr: "127.0.0.1:19192".to_string(),
            data_dir: "/tmp/kura-hook-wiring".to_string(),
            log_level: "info".to_string(),
            version: "0.1.0".to_string(),
            llm: kura_config::LlmConfig::default(),
            connectors: kura_config::ConnectorConfig::default(),
        };
        let _ = store;
        Arc::new(kura_sandbox::Manager::new(
            config,
            None,
            kura_events::Bus::new(),
            kura_policy::Engine::new(),
        ))
    }

    #[test]
    fn catalog_requirement_checker_matches_available_backends_and_fails_closed() {
        let store = test_store();
        let checker = CatalogSandboxRequirementChecker::new(test_sandbox(&store));
        let requirements = vec![
            kura_catalog::Requirement { key: "subprocess".to_string(), description: String::new() },
            kura_catalog::Requirement { key: "backend:subprocess".to_string(), description: String::new() },
            kura_catalog::Requirement { key: "warp_drive".to_string(), description: String::new() },
        ];
        let unmet = checker.unmet("ten_a", &requirements);
        assert_eq!(unmet.len(), 1, "{unmet:?}");
        assert_eq!(unmet[0].key, "warp_drive");
    }

    #[test]
    fn catalog_permission_gate_requires_an_active_tenant() {
        let store = test_store();
        let gate = CatalogTenantPermissionGate::new(store.clone());
        // Local (tenant-less) enablement stays allowed.
        assert!(gate.allow("", &[]));
        // Unknown tenant: fail closed.
        assert!(!gate.allow("ten_unknown", &[]));
        // Active tenant: allowed.
        let now = chrono::Utc::now();
        store
            .lock()
            .upsert_tenant(&kura_identity::Tenant {
                tenant_id: "ten_active".to_string(),
                display_name: "Active".to_string(),
                status: kura_identity::LifecycleStatus::Active,
                created_at: now,
                updated_at: now,
                tenant_kind: kura_identity::TenantKind::Organization,
                created_by_principal_id: String::new(),
                default_owner_principal_id: String::new(),
                caller_membership_role: None,
                caller_membership_status: None,
                caller_permissions: Vec::new(),
                default_for_current_principal: false,
                default_for_current_token: false,
            })
            .expect("upsert tenant");
        assert!(gate.allow("ten_active", &[]));
        // Disabled tenant: denied.
        store
            .lock()
            .upsert_tenant(&kura_identity::Tenant {
                tenant_id: "ten_disabled".to_string(),
                display_name: "Disabled".to_string(),
                status: kura_identity::LifecycleStatus::Disabled,
                created_at: now,
                updated_at: now,
                tenant_kind: kura_identity::TenantKind::Organization,
                created_by_principal_id: String::new(),
                default_owner_principal_id: String::new(),
                caller_membership_role: None,
                caller_membership_status: None,
                caller_permissions: Vec::new(),
                default_for_current_principal: false,
                default_for_current_token: false,
            })
            .expect("upsert tenant");
        assert!(!gate.allow("ten_disabled", &[]));
    }

    #[test]
    fn webhook_quota_gate_allows_local_and_fails_closed_without_quota_state() {
        let store = test_store();
        let bus = Arc::new(kura_events::Bus::new());
        // A billing manager without a repository: hosted reservations fail
        // closed with quota_state_unavailable.
        let billing = Arc::new(kura_billing::Manager::without_repo());
        let gate = WebhookQuotaGateImpl::new(billing, store.clone(), bus, "test");

        let (allowed, reason) = gate.allow("", "wh_local");
        assert!(allowed, "{reason}");

        let (allowed, reason) = gate.allow("ten_a", "wh_hosted");
        assert!(!allowed);
        assert!(!reason.is_empty());
        // The deny is auditable: the event ledger holds the denial.
        let events = store
            .lock()
            .list_events(&kura_events::Filter {
                environment_scope: "test".to_string(),
                category: "webhook".to_string(),
                ..kura_events::Filter::default()
            })
            .expect("list events");
        assert!(
            events.iter().any(|event| event.name == "webhook.trigger_quota_denied"),
            "{events:?}"
        );
    }
}

/// Execution-profile environment-requirement checker backed by the sandbox
/// plane (Roadmap 75): a requirement string is met when it names an available
/// sandbox backend (optionally `backend:`-prefixed); anything the sandbox
/// plane cannot attest stays unmet — fail closed.
pub struct ExecProfileSandboxRequirementChecker {
    sandbox: Arc<kura_sandbox::Manager>,
}

impl ExecProfileSandboxRequirementChecker {
    #[must_use]
    pub fn new(sandbox: Arc<kura_sandbox::Manager>) -> Self {
        Self { sandbox }
    }
}

impl kura_execprofile::RequirementChecker for ExecProfileSandboxRequirementChecker {
    fn unmet(&self, requirements: &[String]) -> Vec<String> {
        let backends = self.sandbox.backend_capabilities();
        requirements
            .iter()
            .filter(|requirement| {
                let key = requirement.trim();
                let key = key.strip_prefix("backend:").unwrap_or(key);
                !backends.iter().any(|backend| {
                    backend.backend_kind.as_str().eq_ignore_ascii_case(key)
                        && backend.availability_status
                            == kura_sandbox::BackendAvailabilityStatus::Available
                })
            })
            .cloned()
            .collect()
    }
}

/// Execution-profile selection gate backed by the identity plane
/// (Roadmap 75): tenant-less local selection stays allowed; a tenant-scoped
/// selection requires an Active tenant — fail closed on unknown tenants.
pub struct ExecProfileTenantPermissionGate {
    store: Arc<parking_lot::Mutex<kura_store::SQLiteStore>>,
}

impl ExecProfileTenantPermissionGate {
    #[must_use]
    pub fn new(store: Arc<parking_lot::Mutex<kura_store::SQLiteStore>>) -> Self {
        Self { store }
    }

    fn tenant_active(&self, tenant_id: &str) -> bool {
        let tenant_id = tenant_id.trim();
        if tenant_id.is_empty() {
            return true;
        }
        matches!(
            self.store.lock().get_tenant(tenant_id),
            Ok(Some(tenant)) if tenant.status == kura_identity::LifecycleStatus::Active
        )
    }
}

impl kura_execprofile::PermissionGate for ExecProfileTenantPermissionGate {
    fn allow(&self, tenant_id: &str, _profile_id: &str) -> bool {
        self.tenant_active(tenant_id)
    }
}

/// Support evidence-bundle gate (Roadmap 75): requires a named actor for the
/// audit trail and, when the bundle is tenant-scoped, an Active tenant —
/// fail closed. The authoritative caller authentication stays with the
/// protected() route middleware; this in-manager gate is defense-in-depth.
pub struct EvidenceSupportPermissionGate {
    store: Arc<parking_lot::Mutex<kura_store::SQLiteStore>>,
}

impl EvidenceSupportPermissionGate {
    #[must_use]
    pub fn new(store: Arc<parking_lot::Mutex<kura_store::SQLiteStore>>) -> Self {
        Self { store }
    }
}

impl kura_evidence::PermissionGate for EvidenceSupportPermissionGate {
    fn allow_support(&self, actor: &str, tenant_id: &str) -> bool {
        if actor.trim().is_empty() {
            return false;
        }
        let tenant_id = tenant_id.trim();
        if tenant_id.is_empty() {
            return true;
        }
        matches!(
            self.store.lock().get_tenant(tenant_id),
            Ok(Some(tenant)) if tenant.status == kura_identity::LifecycleStatus::Active
        )
    }
}

// ---------------------------------------------------------------------------
// Roadmap 78 phase 2: the LLM-dispatch-backed memory Consolidator.
// ---------------------------------------------------------------------------

/// Memory consolidator over the LLM dispatch plane (spec 058 phase 2).
///
/// Each trait method issues one internal system dispatch (empty provider/
/// model resolve to the daemon defaults) whose prompt demands a strict JSON
/// reply, then parses it tolerantly (first JSON array/object in the text).
/// Guard: extracted source links must reference ids present in the supplied
/// L0 window — invented citations are dropped and logged, so the extractor
/// can never fabricate evidence.
pub struct LlmConsolidator {
    dispatcher: Arc<kura_llm::Dispatcher>,
}

impl LlmConsolidator {
    #[must_use]
    pub fn new(dispatcher: Arc<kura_llm::Dispatcher>) -> Self {
        Self { dispatcher }
    }

    fn dispatch_json(&self, system: &str, user: String) -> Result<serde_json::Value, String> {
        let input = kura_llm::CreateDispatchInput {
            provider: String::new(),
            model: String::new(),
            messages: vec![
                kura_llm::Message {
                    role: kura_llm::MessageRole::System,
                    content: system.to_string(),
                    ..Default::default()
                },
                kura_llm::Message { role: kura_llm::MessageRole::User, content: user, ..Default::default() },
            ],
            ..kura_llm::CreateDispatchInput::default()
        };
        let dispatch = self
            .dispatcher
            .prepare(input, false)
            .map_err(|err| format!("prepare consolidation dispatch: {err}"))?;
        let cancel = kura_llm::CancelToken::new();
        let dispatcher = Arc::clone(&self.dispatcher);
        let settled = consolidator_runtime()?
            .block_on(async move { dispatcher.dispatch(dispatch, &cancel).await })
            .map_err(|err| format!("consolidation dispatch failed: {err}"))?;
        extract_json(&settled.output)
            .ok_or_else(|| "consolidation reply carried no JSON".to_string())
    }
}

/// The consolidator's IO runtime: consolidation always runs off the main
/// runtime (spawn_blocking / scheduler tick threads), and the dispatcher's
/// futures need a tokio reactor. One shared runtime, never dropped (a
/// per-call runtime dropped from an async-adjacent context panics).
fn consolidator_runtime() -> Result<Arc<tokio::runtime::Runtime>, String> {
    static RUNTIME: std::sync::OnceLock<Arc<tokio::runtime::Runtime>> = std::sync::OnceLock::new();
    if let Some(runtime) = RUNTIME.get() {
        return Ok(Arc::clone(runtime));
    }
    let built = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("memory-consolidator")
        .worker_threads(1)
        .build()
        .map_err(|err| format!("build consolidator runtime: {err}"))?;
    let _ = RUNTIME.set(Arc::new(built));
    Ok(Arc::clone(RUNTIME.get().expect("consolidator runtime initialized")))
}

/// Finds the first JSON array or object embedded in a model reply.
fn extract_json(text: &str) -> Option<serde_json::Value> {
    for open in ['[', '{'] {
        if let Some(start) = text.find(open) {
            let mut depth = 0i64;
            let mut in_string = false;
            let mut escaped = false;
            for (offset, ch) in text[start..].char_indices() {
                if escaped {
                    escaped = false;
                    continue;
                }
                match ch {
                    '\\' if in_string => escaped = true,
                    '"' => in_string = !in_string,
                    '[' | '{' if !in_string => depth += 1,
                    ']' | '}' if !in_string => {
                        depth -= 1;
                        if depth == 0 {
                            let candidate = &text[start..=start + offset];
                            if let Ok(value) = serde_json::from_str(candidate) {
                                return Some(value);
                            }
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    None
}

const EXTRACT_SYSTEM_PROMPT: &str = "You extract atomic memories from a conversation window. \
Reply with ONLY a JSON array; each element: {\"atomType\": one of \
fact|preference|constraint|event|decision|reference, \"title\": short label, \
\"content\": one self-contained sentence, \"sourceIds\": array of the window item ids the \
atom derives from}. Extract only what the window supports; no speculation.";

const SCENARIO_SYSTEM_PROMPT: &str = "You aggregate memory atoms into scenario blocks. \
Reply with ONLY a JSON array; each element: {\"title\": scenario label, \"content\": a concise \
Markdown block summarizing the related atoms}. Merge related atoms; skip singletons that fit \
no scenario.";

const PERSONA_SYSTEM_PROMPT: &str = "You distill scenario blocks into one persona/core profile. \
Reply with ONLY a JSON object: {\"title\": profile label, \"content\": a concise Markdown \
profile of durable preferences, constraints, and patterns}.";

impl kura_memory::Consolidator for LlmConsolidator {
    fn extract_l1(
        &self,
        _tenant_id: &str,
        window: &[kura_memory::L0Item],
    ) -> Result<Vec<kura_memory::AtomDraft>, String> {
        if window.is_empty() {
            return Ok(Vec::new());
        }
        let mut lines = String::new();
        for item in window {
            lines.push_str(&format!(
                "- id={} role={} at={}: {}\n",
                item.source.id,
                item.role,
                item.occurred_at.to_rfc3339(),
                item.text.replace('\n', " "),
            ));
        }
        let value = self.dispatch_json(EXTRACT_SYSTEM_PROMPT, lines)?;
        let Some(items) = value.as_array() else {
            return Err("extraction reply is not a JSON array".to_string());
        };
        let known: std::collections::HashMap<&str, &kura_memory::SourceLink> =
            window.iter().map(|item| (item.source.id.as_str(), &item.source)).collect();
        let mut drafts = Vec::new();
        for item in items {
            let content = item["content"].as_str().unwrap_or("").trim().to_string();
            if content.is_empty() {
                continue;
            }
            let mut source_links = Vec::new();
            if let Some(ids) = item["sourceIds"].as_array() {
                for id in ids {
                    let Some(id) = id.as_str() else { continue };
                    match known.get(id) {
                        Some(link) => source_links.push((*link).clone()),
                        None => eprintln!(
                            "memory: consolidator invented citation {id}; dropped"
                        ),
                    }
                }
            }
            if source_links.is_empty() {
                // No verifiable citation -> the atom is unattributable; drop
                // it entirely (fail closed on evidence).
                eprintln!("memory: dropping atom without verifiable citations: {content}");
                continue;
            }
            let atom_type = item["atomType"]
                .as_str()
                .and_then(|raw| serde_json::from_value(serde_json::json!(raw)).ok());
            drafts.push(kura_memory::AtomDraft {
                atom_type,
                title: item["title"].as_str().unwrap_or("").trim().to_string(),
                content,
                source_links,
            });
        }
        Ok(drafts)
    }

    fn aggregate_l2(
        &self,
        _tenant_id: &str,
        atoms: &[kura_memory::MemoryAsset],
    ) -> Result<Vec<kura_memory::AtomDraft>, String> {
        if atoms.is_empty() {
            return Ok(Vec::new());
        }
        let mut lines = String::new();
        for atom in atoms {
            lines.push_str(&format!(
                "- [{}] {}: {}\n",
                atom.atom_type.map(|a| a.as_str()).unwrap_or("fact"),
                atom.title,
                atom.content.replace('\n', " "),
            ));
        }
        let value = self.dispatch_json(SCENARIO_SYSTEM_PROMPT, lines)?;
        let Some(items) = value.as_array() else {
            return Err("scenario reply is not a JSON array".to_string());
        };
        Ok(items
            .iter()
            .filter_map(|item| {
                let content = item["content"].as_str().unwrap_or("").trim().to_string();
                if content.is_empty() {
                    return None;
                }
                Some(kura_memory::AtomDraft {
                    atom_type: None,
                    title: item["title"].as_str().unwrap_or("").trim().to_string(),
                    content,
                    source_links: Vec::new(),
                })
            })
            .collect())
    }

    fn distill_l3(
        &self,
        _tenant_id: &str,
        scenarios: &[kura_memory::MemoryAsset],
    ) -> Result<Option<kura_memory::AtomDraft>, String> {
        if scenarios.is_empty() {
            return Ok(None);
        }
        let mut lines = String::new();
        for scenario in scenarios {
            lines.push_str(&format!("## {}\n{}\n\n", scenario.title, scenario.content));
        }
        let value = self.dispatch_json(PERSONA_SYSTEM_PROMPT, lines)?;
        let content = value["content"].as_str().unwrap_or("").trim().to_string();
        if content.is_empty() {
            return Ok(None);
        }
        Ok(Some(kura_memory::AtomDraft {
            atom_type: None,
            title: value["title"].as_str().unwrap_or("").trim().to_string(),
            content,
            source_links: Vec::new(),
        }))
    }
}

#[cfg(test)]
mod consolidator_tests {
    use super::*;
    use kura_memory::Consolidator as _;

    /// Provider stub replying with a fixed body regardless of input.
    struct StaticProvider {
        body: String,
    }

    impl kura_llm::Provider for StaticProvider {
        fn name(&self) -> &str {
            "static"
        }

        fn complete<'a>(
            &'a self,
            _request: kura_llm::ProviderRequest,
        ) -> futures::future::BoxFuture<'a, Result<kura_llm::ProviderResponse, kura_llm::ProviderError>>
        {
            let body = self.body.clone();
            Box::pin(async move {
                Ok(kura_llm::ProviderResponse {
            tool_calls: Vec::new(),
                    output: body,
                    finish_reason: "stop".to_string(),
                    ..kura_llm::ProviderResponse::default()
                })
            })
        }

        fn stream<'a>(
            &'a self,
            request: kura_llm::ProviderRequest,
            _emit: kura_llm::StreamEmitter<'a>,
        ) -> futures::future::BoxFuture<'a, Result<kura_llm::ProviderResponse, kura_llm::ProviderError>>
        {
            self.complete(request)
        }
    }

    fn consolidator_with(body: &str) -> LlmConsolidator {
        let dispatcher = kura_llm::Dispatcher::new();
        dispatcher.register_provider(Arc::new(StaticProvider { body: body.to_string() }));
        let _ = dispatcher.set_default_provider("static");
        dispatcher.set_default_model("static-1");
        LlmConsolidator::new(Arc::new(dispatcher))
    }

    fn window() -> Vec<kura_memory::L0Item> {
        vec![kura_memory::L0Item {
            source: kura_memory::SourceLink {
                kind: kura_memory::SourceKind::Message,
                id: "msg_1".to_string(),
                ..kura_memory::SourceLink::default()
            },
            role: "chat_turn".to_string(),
            text: "user: 用中文回复我\nassistant: 好的".to_string(),
            occurred_at: chrono::Utc::now(),
        }]
    }

    #[test]
    fn extracts_atoms_and_drops_invented_citations() {
        let consolidator = consolidator_with(
            r#"Here you go:
[
  {"atomType": "preference", "title": "语言偏好", "content": "The user prefers Chinese replies.", "sourceIds": ["msg_1"]},
  {"atomType": "fact", "title": "invented", "content": "Fabricated claim.", "sourceIds": ["msg_999"]}
]"#,
        );
        let drafts = consolidator.extract_l1("", &window()).expect("extract");
        // The atom with only an invented citation is dropped entirely.
        assert_eq!(drafts.len(), 1, "{drafts:?}");
        assert_eq!(drafts[0].source_links[0].id, "msg_1");
        assert_eq!(drafts[0].atom_type, Some(kura_memory::AtomType::Preference));
    }

    #[test]
    fn scenario_and_persona_replies_parse() {
        let consolidator =
            consolidator_with(r#"[{"title": "沟通偏好", "content": "- prefers Chinese"}]"#);
        let atoms = vec![kura_memory::MemoryAsset {
            asset_id: "mem_a".to_string(),
            content: "prefers Chinese".to_string(),
            ..kura_memory::MemoryAsset::default()
        }];
        let scenarios = consolidator.aggregate_l2("", &atoms).expect("aggregate");
        assert_eq!(scenarios.len(), 1);

        let consolidator =
            consolidator_with(r#"{"title": "画像", "content": "Chinese-speaking operator"}"#);
        let persona = consolidator
            .distill_l3("", &[kura_memory::MemoryAsset::default()])
            .expect("distill");
        assert!(persona.is_some());
    }

    #[test]
    fn non_json_reply_is_an_error_not_a_panic() {
        let consolidator = consolidator_with("I could not comply.");
        let err = consolidator.extract_l1("", &window()).expect_err("must error");
        assert!(err.contains("no JSON"), "{err}");
    }
}
