//! Activation service and dependency traits (port of `service.go`).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use chrono::DateTime;
use chrono::Utc;
use kura_identity::AUDIT_OUTCOME_DENIED;
use kura_identity::AUDIT_OUTCOME_FAILED_CLOSED;
use kura_identity::AUDIT_OUTCOME_SUCCEEDED;
use kura_identity::LifecycleStatus;
use kura_identity::Membership;
use kura_identity::MembershipFilter;
use kura_identity::Principal;
use kura_identity::PrincipalFilter;
use kura_identity::PrincipalKind;
use kura_identity::Role;
use kura_identity::Tenant;
use kura_identity::TenantAuditEvent;
use kura_identity::TenantContext;
use kura_identity::TenantFilter;
use kura_identity::TenantKind;
use kura_identity::TokenAuthority;
use kura_identity::TokenTenantGrant;

use crate::audit::AuditRecord;
use crate::error::ActivationError;
use crate::error::StoreError;
use crate::error::activation_error;
use crate::readiness::active_state_for_personal_tenant;
use crate::types::FailureStage;
use crate::types::ReasonCode;
use crate::types::RemediationOwner;
use crate::types::STEP_RESOLVE_PERSONAL_TENANT;
use crate::types::State;
use crate::types::Status;
use crate::types::default_test_chat_first_action;

/// Object-safe boxed future used by the dependency traits.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Persistence abstraction for activation state (Go `StateStore`).
pub trait StateStore: Send + Sync {
    fn upsert_activation_state(&self, state: State) -> BoxFuture<'_, Result<(), StoreError>>;
    fn get_activation_state(
        &self,
        activation_id: &str,
    ) -> BoxFuture<'_, Result<Option<State>, StoreError>>;
    fn get_activation_state_for_principal_tenant(
        &self,
        principal_id: &str,
        tenant_id: &str,
    ) -> BoxFuture<'_, Result<Option<State>, StoreError>>;
}

/// Identity data access required by activation (Go `IdentityRepository`).
pub trait IdentityRepository: Send + Sync {
    fn get_principal(
        &self,
        principal_id: &str,
    ) -> BoxFuture<'_, Result<Option<Principal>, StoreError>>;
    fn list_principals(
        &self,
        filter: &PrincipalFilter,
    ) -> BoxFuture<'_, Result<Vec<Principal>, StoreError>>;
    fn upsert_principal(&self, principal: Principal) -> BoxFuture<'_, Result<(), StoreError>>;
    fn get_tenant(&self, tenant_id: &str) -> BoxFuture<'_, Result<Option<Tenant>, StoreError>>;
    fn list_tenants(&self, filter: &TenantFilter)
    -> BoxFuture<'_, Result<Vec<Tenant>, StoreError>>;
    fn upsert_tenant(&self, tenant: Tenant) -> BoxFuture<'_, Result<(), StoreError>>;
    fn list_memberships(
        &self,
        filter: &MembershipFilter,
    ) -> BoxFuture<'_, Result<Vec<Membership>, StoreError>>;
    fn upsert_membership(&self, membership: Membership) -> BoxFuture<'_, Result<(), StoreError>>;
    fn list_token_tenant_grants(
        &self,
        token_id: &str,
    ) -> BoxFuture<'_, Result<Vec<TokenTenantGrant>, StoreError>>;
    fn upsert_token_tenant_grant(
        &self,
        grant: TokenTenantGrant,
    ) -> BoxFuture<'_, Result<(), StoreError>>;
}

/// Quota baseline projection source (Go `BillingProjector`).
pub trait BillingProjector: Send + Sync {
    fn usage_summary(
        &self,
        tenant_id: &str,
        hosted: bool,
    ) -> BoxFuture<'_, Result<kura_billing::UsageSummary, kura_billing::BillingError>>;
}

/// Runs the hosted activation test chat (Go `ChatRunner`).
///
/// Failure returns a [`crate::ChatRunFailure`] carrying the partial result so
/// failure metadata (dispatch id, provider, model, usage) is preserved exactly
/// like Go's `(TestChatResult, error)` multi-return.
pub trait ChatRunner: Send + Sync {
    fn run_activation_test_chat(
        &self,
        input: crate::TestChatInput,
    ) -> BoxFuture<'_, Result<crate::TestChatResult, crate::ChatRunFailure>>;
}

/// Audit sink for activation transitions (Go `AuditSink`).
pub trait AuditSink: Send + Sync {
    fn append_tenant_audit_event(
        &self,
        event: TenantAuditEvent,
    ) -> BoxFuture<'_, Result<TenantAuditEvent, StoreError>>;
}

/// Service wiring. Missing stores/runners mirror Go's nil-dependency checks:
/// the affected method fails closed with a stable reason instead of panicking.
#[derive(Default)]
pub struct Dependencies {
    pub state_store: Option<Arc<dyn StateStore>>,
    pub identity: Option<Arc<dyn IdentityRepository>>,
    pub billing: Option<Arc<dyn BillingProjector>>,
    pub chat: Option<Arc<dyn ChatRunner>>,
    pub audit: Option<Arc<dyn AuditSink>>,
    pub now: Option<Box<dyn Fn() -> DateTime<Utc> + Send + Sync>>,
    pub environment_scope: String,
    pub hosted: bool,
}

/// Activation orchestration service.
pub struct Service {
    pub(crate) state_store: Option<Arc<dyn StateStore>>,
    pub(crate) identity: Option<Arc<dyn IdentityRepository>>,
    pub(crate) billing: Option<Arc<dyn BillingProjector>>,
    pub(crate) chat: Option<Arc<dyn ChatRunner>>,
    pub(crate) audit: Option<Arc<dyn AuditSink>>,
    pub(crate) now: Box<dyn Fn() -> DateTime<Utc> + Send + Sync>,
    pub(crate) environment_scope: String,
    pub(crate) hosted: bool,
}

impl Service {
    /// Builds the service over the SQLite-backed activation seams (wave 8
    /// parity): the store adapter supplies the [`StateStore`],
    /// [`IdentityRepository`], and [`AuditSink`] dependencies, mirroring Go's
    /// single `*SQLiteStore` satisfying all three interfaces. Billing and chat
    /// remain injectable (see [`BillingProjectorAdapter`] /
    /// [`ChatRunnerAdapter`]).
    #[must_use]
    pub fn with_sqlite(
        store: Arc<crate::sqlite::SqliteActivationStore>,
        billing: Option<Arc<dyn BillingProjector>>,
        chat: Option<Arc<dyn ChatRunner>>,
        environment_scope: &str,
        hosted: bool,
    ) -> Self {
        Service::new(Dependencies {
            state_store: Some(store.clone()),
            identity: Some(store.clone()),
            billing,
            chat,
            audit: Some(store),
            now: None,
            environment_scope: environment_scope.to_string(),
            hosted,
        })
    }

    #[must_use]
    pub fn new(deps: Dependencies) -> Self {
        let now = deps.now.unwrap_or_else(|| Box::new(Utc::now));
        let environment_scope = if deps.environment_scope.is_empty() {
            "test".to_string()
        } else {
            deps.environment_scope
        };
        Self {
            state_store: deps.state_store,
            identity: deps.identity,
            billing: deps.billing,
            chat: deps.chat,
            audit: deps.audit,
            now,
            environment_scope,
            hosted: deps.hosted,
        }
    }

    pub(crate) fn now(&self) -> DateTime<Utc> {
        (self.now)()
    }

    pub(crate) fn not_configured() -> ActivationError {
        activation_error(
            ReasonCode::UNEXPECTED_FAILED.into(),
            FailureStage::UNEXPECTED.into(),
            false,
            RemediationOwner::OPERATOR.into(),
            "activation service is not configured",
        )
    }

    /// Starts (or refreshes) activation for the token's principal, resolving
    /// the personal tenant, enforcing eligibility, and projecting the quota
    /// baseline. Every transition is audited fail-closed.
    pub async fn activate(&self, input: ActivateInput) -> Result<State, ActivationError> {
        let (Some(identity), Some(state_store)) = (&self.identity, &self.state_store) else {
            return Err(Self::not_configured());
        };
        if input.token.status != LifecycleStatus::Active {
            return Err(activation_error(
                ReasonCode::PRINCIPAL_DENIED.into(),
                FailureStage::ELIGIBILITY.into(),
                false,
                RemediationOwner::PRODUCT_USER.into(),
                "activation token is denied",
            ));
        }
        let mut principal_id = input.token.principal_id.trim().to_string();
        if principal_id.is_empty() {
            principal_id = input.tenant_context.principal_id.trim().to_string();
        }
        if principal_id.is_empty() {
            return Err(activation_error(
                ReasonCode::PRINCIPAL_DENIED.into(),
                FailureStage::ELIGIBILITY.into(),
                false,
                RemediationOwner::PRODUCT_USER.into(),
                "activation principal is required",
            ));
        }
        let mut principal = identity
            .get_principal(&principal_id)
            .await
            .map_err(ActivationError::dependency)?;
        let now = self.now();
        let mut principal = match principal.take() {
            Some(principal) => principal,
            None => Principal {
                principal_id: principal_id.clone(),
                principal_kind: PrincipalKind::User,
                display_name: "Hosted user".to_string(),
                status: LifecycleStatus::Active,
                default_tenant_id: String::new(),
                created_at: now,
                updated_at: now,
                disabled_at: None,
                removed_at: None,
            },
        };
        if principal.status == LifecycleStatus::Disabled {
            self.record_audit(AuditRecord {
                event_kind: "tenant.activation_denied".to_string(),
                principal_id: principal.principal_id.clone(),
                token_id: input.token.token_id.clone(),
                outcome: AUDIT_OUTCOME_DENIED.to_string(),
                reason_code: ReasonCode::PRINCIPAL_DISABLED.into(),
                stage: FailureStage::ELIGIBILITY.into(),
                to_status: Status::BLOCKED.into(),
                ..AuditRecord::default()
            })
            .await?;
            return Err(activation_error(
                ReasonCode::PRINCIPAL_DISABLED.into(),
                FailureStage::ELIGIBILITY.into(),
                false,
                RemediationOwner::PRODUCT_USER.into(),
                "activation principal is disabled",
            ));
        }
        if principal.status != LifecycleStatus::Active {
            return Err(activation_error(
                ReasonCode::PRINCIPAL_DENIED.into(),
                FailureStage::ELIGIBILITY.into(),
                false,
                RemediationOwner::PRODUCT_USER.into(),
                "activation principal is denied",
            ));
        }

        let tenant = self
            .resolve_personal_tenant(&principal, &input.token, now)
            .await?;
        if principal.default_tenant_id != tenant.tenant_id {
            principal.default_tenant_id = tenant.tenant_id.clone();
            principal.updated_at = now;
            identity
                .upsert_principal(principal.clone())
                .await
                .map_err(ActivationError::dependency)?;
        }

        let existing = state_store
            .get_activation_state_for_principal_tenant(&principal.principal_id, &tenant.tenant_id)
            .await
            .map_err(ActivationError::dependency)?;
        let mut state = active_state_for_personal_tenant(self, &principal, &tenant, now).await?;
        let from_status = existing
            .as_ref()
            .map(|state| state.status.clone())
            .unwrap_or_default();
        if let Some(existing) = existing {
            state.activation_id = existing.activation_id;
            state.created_at = existing.created_at;
            state.test_chat = existing.test_chat;
            state.first_action_completed_at = existing.first_action_completed_at;
            state.last_transition_audit_event = existing.last_transition_audit_event;
            if existing.status == Status::FIRST_ACTION_COMPLETED && state.status != Status::BLOCKED
            {
                state.status = existing.status;
                state.current_step_id = existing.current_step_id;
                state.completed_step_ids = existing.completed_step_ids;
            }
        }
        self.record_audit(AuditRecord {
            event_kind: "tenant.activation_started".to_string(),
            activation_id: state.activation_id.clone(),
            tenant_id: tenant.tenant_id.clone(),
            principal_id: principal.principal_id.clone(),
            token_id: input.token.token_id.clone(),
            outcome: AUDIT_OUTCOME_SUCCEEDED.to_string(),
            stage: FailureStage::TENANT_RESOLUTION.into(),
            from_status: from_status.clone(),
            to_status: state.status.clone(),
            ..AuditRecord::default()
        })
        .await?;
        state_store
            .upsert_activation_state(state.clone())
            .await
            .map_err(ActivationError::dependency)?;
        let mut completion_audit = AuditRecord {
            event_kind: "tenant.activation_completed".to_string(),
            activation_id: state.activation_id.clone(),
            tenant_id: tenant.tenant_id.clone(),
            principal_id: principal.principal_id.clone(),
            token_id: input.token.token_id.clone(),
            outcome: AUDIT_OUTCOME_SUCCEEDED.to_string(),
            stage: FailureStage::TENANT_RESOLUTION.into(),
            from_status,
            to_status: state.status.clone(),
            ..AuditRecord::default()
        };
        if state.status == Status::BLOCKED {
            if let Some(failure) = &state.failure_reason {
                completion_audit.event_kind = "tenant.activation_blocked".to_string();
                completion_audit.outcome = AUDIT_OUTCOME_FAILED_CLOSED.to_string();
                completion_audit.stage = failure.stage.clone();
                completion_audit.reason_code = failure.reason_code.clone();
                completion_audit.retryable = failure.retryable;
                completion_audit.remediation_owner = failure.remediation_owner.clone();
            }
        }
        self.record_audit(completion_audit).await?;
        Ok(state)
    }

    /// Returns the persisted activation state for the tenant context, or a
    /// fresh `not_started` projection when none exists yet.
    pub async fn get(&self, input: GetInput) -> Result<State, ActivationError> {
        let Some(state_store) = &self.state_store else {
            return Err(Self::not_configured());
        };
        let mut principal_id = input.tenant_context.principal_id.trim().to_string();
        if principal_id.is_empty() {
            principal_id = input.token.principal_id.trim().to_string();
        }
        let mut tenant_id = input.tenant_context.tenant_id.trim().to_string();
        if tenant_id.is_empty() {
            tenant_id = input.token.default_tenant_id.trim().to_string();
        }
        if principal_id.is_empty() || tenant_id.is_empty() {
            return Err(activation_error(
                ReasonCode::TENANT_ACCESS_REVOKED.into(),
                FailureStage::AUTHORIZATION.into(),
                false,
                RemediationOwner::PRODUCT_USER.into(),
                "activation tenant context is required",
            ));
        }
        let state = state_store
            .get_activation_state_for_principal_tenant(&principal_id, &tenant_id)
            .await
            .map_err(ActivationError::dependency)?;
        if let Some(state) = state {
            return Ok(state);
        }
        let now = self.now();
        Ok(State {
            activation_id: stable_activation_id("act", &[&principal_id, &tenant_id]),
            principal_id,
            tenant_id,
            environment_scope: self.environment_scope.clone(),
            status: Status::NOT_STARTED.into(),
            current_step_id: STEP_RESOLVE_PERSONAL_TENANT.to_string(),
            completed_step_ids: Vec::new(),
            blocking_reason_codes: Vec::new(),
            readiness_items: Vec::new(),
            quota_baseline: None,
            first_action: default_test_chat_first_action(false, vec!["tenant-access".to_string()]),
            test_chat: None,
            failure_reason: None,
            created_at: now,
            updated_at: now,
            first_action_completed_at: None,
            last_evaluated_at: now,
            last_transition_audit_event: String::new(),
            metadata: None,
        })
    }

    /// Resolves (or creates) the principal's personal tenant.
    pub(crate) async fn resolve_personal_tenant(
        &self,
        principal: &Principal,
        token: &TokenAuthority,
        now: DateTime<Utc>,
    ) -> Result<Tenant, ActivationError> {
        let identity = self.identity.as_ref().ok_or_else(Self::not_configured)?;
        if !principal.default_tenant_id.is_empty() {
            let tenant = identity
                .get_tenant(&principal.default_tenant_id)
                .await
                .map_err(ActivationError::dependency)?;
            if let Some(tenant) = tenant {
                if tenant.tenant_kind == TenantKind::Personal
                    && tenant.status == LifecycleStatus::Active
                {
                    self.ensure_personal_tenant_access(principal, &tenant, token, now)
                        .await?;
                    return Ok(tenant);
                }
            }
        }
        let tenants = identity
            .list_tenants(&TenantFilter {
                tenant_kind: Some(TenantKind::Personal),
                status: Some(LifecycleStatus::Active),
                limit: 1000,
            })
            .await
            .map_err(ActivationError::dependency)?;
        for tenant in tenants {
            if tenant.default_owner_principal_id == principal.principal_id {
                self.ensure_personal_tenant_access(principal, &tenant, token, now)
                    .await?;
                return Ok(tenant);
            }
        }
        let tenant = Tenant {
            tenant_id: stable_activation_id("ten_personal", &[&principal.principal_id]),
            tenant_kind: TenantKind::Personal,
            display_name: "Personal tenant".to_string(),
            status: LifecycleStatus::Active,
            created_at: now,
            updated_at: now,
            created_by_principal_id: principal.principal_id.clone(),
            default_owner_principal_id: principal.principal_id.clone(),
            caller_membership_role: None,
            caller_membership_status: None,
            caller_permissions: Vec::new(),
            default_for_current_token: false,
            default_for_current_principal: false,
        };
        identity
            .upsert_tenant(tenant.clone())
            .await
            .map_err(ActivationError::dependency)?;
        self.ensure_personal_tenant_access(principal, &tenant, token, now)
            .await?;
        Ok(tenant)
    }

    /// Guarantees owner membership and (when a token is present) an active
    /// token tenant grant; fails closed when either was revoked.
    pub(crate) async fn ensure_personal_tenant_access(
        &self,
        principal: &Principal,
        tenant: &Tenant,
        token: &TokenAuthority,
        now: DateTime<Utc>,
    ) -> Result<(), ActivationError> {
        let identity = self.identity.as_ref().ok_or_else(Self::not_configured)?;
        let memberships = identity
            .list_memberships(&MembershipFilter {
                tenant_id: tenant.tenant_id.clone(),
                limit: 1000,
                ..MembershipFilter::default()
            })
            .await
            .map_err(ActivationError::dependency)?;
        let mut has_membership = false;
        for membership in memberships {
            if membership.principal_id != principal.principal_id {
                continue;
            }
            if membership.status != LifecycleStatus::Active {
                return Err(activation_error(
                    ReasonCode::TENANT_ACCESS_REVOKED.into(),
                    FailureStage::AUTHORIZATION.into(),
                    false,
                    RemediationOwner::PRODUCT_USER.into(),
                    "activation tenant access is revoked",
                ));
            }
            has_membership = true;
            break;
        }
        if !has_membership {
            identity
                .upsert_membership(Membership {
                    membership_id: stable_activation_id(
                        "mem",
                        &[&principal.principal_id, &tenant.tenant_id],
                    ),
                    tenant_id: tenant.tenant_id.clone(),
                    principal_id: principal.principal_id.clone(),
                    role: Role::Owner,
                    status: LifecycleStatus::Active,
                    invitation_id: String::new(),
                    created_at: now,
                    updated_at: now,
                    accepted_at: Some(now),
                    removed_at: None,
                })
                .await
                .map_err(ActivationError::dependency)?;
        }
        if token.token_id.trim().is_empty() {
            return Ok(());
        }
        let grants = identity
            .list_token_tenant_grants(&token.token_id)
            .await
            .map_err(ActivationError::dependency)?;
        for grant in grants {
            if grant.tenant_id != tenant.tenant_id {
                continue;
            }
            if grant.status != LifecycleStatus::Active {
                return Err(activation_error(
                    ReasonCode::TENANT_ACCESS_REVOKED.into(),
                    FailureStage::AUTHORIZATION.into(),
                    false,
                    RemediationOwner::PRODUCT_USER.into(),
                    "activation token tenant grant is revoked",
                ));
            }
            return Ok(());
        }
        identity
            .upsert_token_tenant_grant(TokenTenantGrant {
                grant_id: stable_activation_id("grant", &[&token.token_id, &tenant.tenant_id]),
                token_id: token.token_id.clone(),
                tenant_id: tenant.tenant_id.clone(),
                is_default: true,
                status: LifecycleStatus::Active,
                created_at: now,
                updated_at: now,
                revoked_at: None,
                granted_by_principal_id: principal.principal_id.clone(),
            })
            .await
            .map_err(ActivationError::dependency)?;
        Ok(())
    }
}

/// Builds a deterministic id from a prefix and caller-supplied parts: each
/// part is trimmed, non-alphanumeric runes become `_`, surrounding `_` are
/// stripped, and the survivors are lowercased and joined with `_`.
#[must_use]
pub fn stable_activation_id(prefix: &str, parts: &[&str]) -> String {
    let mut cleaned = vec![prefix.to_string()];
    for part in parts {
        let mapped: String = part
            .trim()
            .chars()
            .map(|r| if r.is_ascii_alphanumeric() { r } else { '_' })
            .collect();
        let trimmed = mapped.trim_matches('_');
        if !trimmed.is_empty() {
            cleaned.push(trimmed.to_lowercase());
        }
    }
    cleaned.join("_")
}

/// Input for [`Service::activate`].
#[derive(Debug, Clone)]
pub struct ActivateInput {
    pub token: TokenAuthority,
    pub tenant_context: TenantContext,
    pub source: String,
}

/// Input for [`Service::get`] and [`Service::diagnostics`].
#[derive(Debug, Clone)]
pub struct GetInput {
    pub token: TokenAuthority,
    pub tenant_context: TenantContext,
}

/// Input for [`Service::run_test_chat`].
#[derive(Debug, Clone)]
pub struct RunTestChatInput {
    pub token: TokenAuthority,
    pub tenant_context: TenantContext,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::reason_code_from_error;
    use crate::testutil::*;

    #[test]
    fn new_service_defaults_boundary_configuration() {
        let now = test_now();
        let svc = Service::new(Dependencies {
            state_store: Some(Arc::new(MemoryStateStore::default())),
            identity: Some(Arc::new(MemoryIdentityRepository::default())),
            billing: Some(Arc::new(StaticBillingProjector::default())),
            chat: None,
            audit: None,
            now: Some(Box::new(move || now)),
            environment_scope: "prod".to_string(),
            hosted: true,
        });
        assert_eq!(svc.now(), now, "expected injected now");
        assert_eq!(svc.environment_scope, "prod");
        assert!(svc.hosted);

        let defaulted = Service::new(Dependencies::default());
        assert_eq!(
            defaulted.environment_scope, "test",
            "empty scope defaults to test"
        );
    }

    #[tokio::test]
    async fn activate_creates_and_reuses_one_personal_tenant() {
        let now = test_now();
        let repo = Arc::new(MemoryIdentityRepository::default());
        repo.principals.lock().insert(
            "prn_hosted".to_string(),
            active_principal("prn_hosted", now),
        );
        let state_store = Arc::new(MemoryStateStore::default());
        let audit_sink = Arc::new(RecordingAuditSink::default());
        let svc = Service::new(Dependencies {
            state_store: Some(state_store.clone()),
            identity: Some(repo.clone()),
            billing: None,
            chat: None,
            audit: Some(audit_sink.clone()),
            now: Some(Box::new(move || now)),
            environment_scope: "test".to_string(),
            hosted: true,
        });
        let input = ActivateInput {
            token: active_token("tok_hosted", "prn_hosted"),
            tenant_context: TenantContext::default(),
            source: "signup".to_string(),
        };

        let first = svc.activate(input.clone()).await.expect("first activate");
        let second = svc.activate(input).await.expect("second activate");

        assert!(!first.tenant_id.is_empty());
        assert_eq!(
            first.tenant_id, second.tenant_id,
            "expected stable personal tenant"
        );
        assert_eq!(first.activation_id, second.activation_id);
        assert_eq!(second.status, Status::ACTIVE);
        let personal_tenants = repo
            .tenants
            .lock()
            .values()
            .filter(|tenant| {
                tenant.tenant_kind == TenantKind::Personal
                    && tenant.default_owner_principal_id == "prn_hosted"
            })
            .count();
        assert_eq!(personal_tenants, 1, "expected one personal tenant");
        assert_eq!(state_store.states_by_key.lock().len(), 1);
        assert!(audit_sink.has_event("tenant.activation_started", ""));
        assert!(audit_sink.has_event("tenant.activation_completed", ""));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn activate_concurrent_attempts_converge() {
        let now = test_now();
        let repo = Arc::new(MemoryIdentityRepository::default());
        repo.principals.lock().insert(
            "prn_concurrent".to_string(),
            active_principal("prn_concurrent", now),
        );
        let state_store = Arc::new(MemoryStateStore::default());
        let svc = Arc::new(Service::new(Dependencies {
            state_store: Some(state_store.clone()),
            identity: Some(repo),
            billing: None,
            chat: None,
            audit: Some(Arc::new(RecordingAuditSink::default())),
            now: Some(Box::new(move || now)),
            environment_scope: "test".to_string(),
            hosted: true,
        }));

        const ATTEMPTS: usize = 12;
        let mut handles = Vec::with_capacity(ATTEMPTS);
        for _ in 0..ATTEMPTS {
            let svc = svc.clone();
            handles.push(tokio::spawn(async move {
                svc.activate(ActivateInput {
                    token: active_token("tok_concurrent", "prn_concurrent"),
                    tenant_context: TenantContext::default(),
                    source: String::new(),
                })
                .await
            }));
        }
        let mut tenant_id = String::new();
        for handle in handles {
            let state = handle.await.expect("join").expect("activate");
            if tenant_id.is_empty() {
                tenant_id = state.tenant_id.clone();
            }
            assert_eq!(state.tenant_id, tenant_id, "concurrent activation diverged");
            assert_eq!(state.status, Status::ACTIVE);
        }
        assert_eq!(
            state_store.states_by_key.lock().len(),
            1,
            "expected one activation state after concurrent attempts"
        );
    }

    #[tokio::test]
    async fn activate_denies_disabled_principal_with_stable_reason() {
        let now = test_now();
        let repo = Arc::new(MemoryIdentityRepository::default());
        repo.principals.lock().insert(
            "prn_disabled".to_string(),
            Principal {
                status: LifecycleStatus::Disabled,
                ..active_principal("prn_disabled", now)
            },
        );
        let state_store = Arc::new(MemoryStateStore::default());
        let audit_sink = Arc::new(RecordingAuditSink::default());
        let svc = Service::new(Dependencies {
            state_store: Some(state_store.clone()),
            identity: Some(repo),
            billing: None,
            chat: None,
            audit: Some(audit_sink.clone()),
            now: Some(Box::new(move || now)),
            environment_scope: "test".to_string(),
            hosted: true,
        });

        let err = svc
            .activate(ActivateInput {
                token: active_token("tok_disabled", "prn_disabled"),
                tenant_context: TenantContext::default(),
                source: String::new(),
            })
            .await
            .expect_err("disabled principal must be denied");
        assert_eq!(reason_code_from_error(&err), ReasonCode::PRINCIPAL_DISABLED);
        assert!(
            state_store.states_by_key.lock().is_empty(),
            "disabled activation should not persist completion state"
        );
        assert!(audit_sink.has_event("tenant.activation_denied", ReasonCode::PRINCIPAL_DISABLED));
    }

    #[tokio::test]
    async fn activate_denies_revoked_token_and_tenant_access_with_stable_reasons() {
        let now = test_now();
        let repo = Arc::new(MemoryIdentityRepository::default());
        repo.principals.lock().insert(
            "prn_revoked".to_string(),
            Principal {
                default_tenant_id: "ten_personal".to_string(),
                ..active_principal("prn_revoked", now)
            },
        );
        repo.tenants.lock().insert(
            "ten_personal".to_string(),
            Tenant {
                tenant_id: "ten_personal".to_string(),
                tenant_kind: TenantKind::Personal,
                display_name: "Personal tenant".to_string(),
                status: LifecycleStatus::Active,
                created_at: now,
                updated_at: now,
                created_by_principal_id: String::new(),
                default_owner_principal_id: "prn_revoked".to_string(),
                caller_membership_role: None,
                caller_membership_status: None,
                caller_permissions: Vec::new(),
                default_for_current_token: false,
                default_for_current_principal: false,
            },
        );
        repo.memberships.lock().insert(
            "mem_revoked".to_string(),
            Membership {
                membership_id: "mem_revoked".to_string(),
                tenant_id: "ten_personal".to_string(),
                principal_id: "prn_revoked".to_string(),
                role: Role::Owner,
                status: LifecycleStatus::Removed,
                invitation_id: String::new(),
                created_at: now,
                updated_at: now,
                accepted_at: None,
                removed_at: None,
            },
        );
        let state_store = Arc::new(MemoryStateStore::default());
        let svc = Service::new(Dependencies {
            state_store: Some(state_store.clone()),
            identity: Some(repo),
            billing: None,
            chat: None,
            audit: Some(Arc::new(RecordingAuditSink::default())),
            now: Some(Box::new(move || now)),
            environment_scope: "test".to_string(),
            hosted: true,
        });

        let mut revoked_token = active_token("tok_revoked", "prn_revoked");
        revoked_token.status = LifecycleStatus::Revoked;
        let err = svc
            .activate(ActivateInput {
                token: revoked_token,
                tenant_context: TenantContext::default(),
                source: String::new(),
            })
            .await
            .expect_err("revoked token must be denied");
        assert_eq!(reason_code_from_error(&err), ReasonCode::PRINCIPAL_DENIED);

        let err = svc
            .activate(ActivateInput {
                token: active_token("tok_active", "prn_revoked"),
                tenant_context: TenantContext::default(),
                source: String::new(),
            })
            .await
            .expect_err("revoked membership must be denied");
        assert_eq!(
            reason_code_from_error(&err),
            ReasonCode::TENANT_ACCESS_REVOKED
        );
        assert!(
            state_store.states_by_key.lock().is_empty(),
            "revoked activation should not persist completion state"
        );
    }

    #[test]
    fn stable_activation_id_normalizes_parts() {
        assert_eq!(
            stable_activation_id("act", &[" prn.User-1 ", "ten/2"]),
            "act_prn_user_1_ten_2"
        );
        assert_eq!(stable_activation_id("act", &["", "___"]), "act");
    }
}
