//! Shared in-memory fakes for the ported Go behavioral tests.

use std::collections::HashMap;

use chrono::DateTime;
use chrono::TimeZone;
use chrono::Utc;
use kura_billing::UsageSummary;
use kura_identity::LifecycleStatus;
use kura_identity::Membership;
use kura_identity::MembershipFilter;
use kura_identity::Principal;
use kura_identity::PrincipalFilter;
use kura_identity::PrincipalKind;
use kura_identity::Tenant;
use kura_identity::TenantAuditEvent;
use kura_identity::TenantContext;
use kura_identity::TenantFilter;
use kura_identity::TokenAuthority;
use kura_identity::TokenTenantGrant;
use parking_lot::Mutex;

use crate::AuditSink;
use crate::BillingProjector;
use crate::ChatRunFailure;
use crate::ChatRunner;
use crate::IdentityRepository;
use crate::StateStore;
use crate::StoreError;
use crate::TestChatInput;
use crate::TestChatResult;
use crate::service::BoxFuture;

/// Fixed timestamp used by the Go tests: 2026-05-06T10:00:00Z.
pub(crate) fn test_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 5, 6, 10, 0, 0)
        .single()
        .unwrap_or_else(Utc::now)
}

pub(crate) fn active_token(token_id: &str, principal_id: &str) -> TokenAuthority {
    TokenAuthority {
        token_id: token_id.to_string(),
        principal_id: principal_id.to_string(),
        default_tenant_id: String::new(),
        status: LifecycleStatus::Active,
        expires_at: None,
    }
}

pub(crate) fn tenant_context(principal_id: &str, tenant_id: &str, token_id: &str) -> TenantContext {
    TenantContext {
        principal_id: principal_id.to_string(),
        tenant_id: tenant_id.to_string(),
        token_id: token_id.to_string(),
        ..TenantContext::default()
    }
}

pub(crate) fn active_principal(principal_id: &str, now: DateTime<Utc>) -> Principal {
    Principal {
        principal_id: principal_id.to_string(),
        principal_kind: PrincipalKind::User,
        display_name: "Hosted User".to_string(),
        status: LifecycleStatus::Active,
        default_tenant_id: String::new(),
        created_at: now,
        updated_at: now,
        disabled_at: None,
        removed_at: None,
    }
}

pub(crate) fn boxed_err(message: &str) -> StoreError {
    message.to_string().into()
}

#[derive(Default)]
pub(crate) struct MemoryStateStore {
    pub states_by_id: Mutex<HashMap<String, crate::State>>,
    pub states_by_key: Mutex<HashMap<String, crate::State>>,
}

impl StateStore for MemoryStateStore {
    fn upsert_activation_state(
        &self,
        state: crate::State,
    ) -> BoxFuture<'_, Result<(), StoreError>> {
        Box::pin(async move {
            self.states_by_id
                .lock()
                .insert(state.activation_id.clone(), state.clone());
            self.states_by_key
                .lock()
                .insert(format!("{}|{}", state.principal_id, state.tenant_id), state);
            Ok(())
        })
    }

    fn get_activation_state(
        &self,
        activation_id: &str,
    ) -> BoxFuture<'_, Result<Option<crate::State>, StoreError>> {
        let activation_id = activation_id.to_string();
        Box::pin(async move { Ok(self.states_by_id.lock().get(&activation_id).cloned()) })
    }

    fn get_activation_state_for_principal_tenant(
        &self,
        principal_id: &str,
        tenant_id: &str,
    ) -> BoxFuture<'_, Result<Option<crate::State>, StoreError>> {
        let key = format!("{principal_id}|{tenant_id}");
        Box::pin(async move { Ok(self.states_by_key.lock().get(&key).cloned()) })
    }
}

#[derive(Default)]
pub(crate) struct MemoryIdentityRepository {
    pub principals: Mutex<HashMap<String, Principal>>,
    pub tenants: Mutex<HashMap<String, Tenant>>,
    pub memberships: Mutex<HashMap<String, Membership>>,
    pub grants: Mutex<HashMap<String, TokenTenantGrant>>,
}

impl IdentityRepository for MemoryIdentityRepository {
    fn get_principal(
        &self,
        principal_id: &str,
    ) -> BoxFuture<'_, Result<Option<Principal>, StoreError>> {
        let principal_id = principal_id.to_string();
        Box::pin(async move { Ok(self.principals.lock().get(&principal_id).cloned()) })
    }

    fn list_principals(
        &self,
        filter: &PrincipalFilter,
    ) -> BoxFuture<'_, Result<Vec<Principal>, StoreError>> {
        let filter = filter.clone();
        Box::pin(async move {
            Ok(self
                .principals
                .lock()
                .values()
                .filter(|principal| {
                    filter
                        .status
                        .is_none_or(|status| principal.status == status)
                })
                .cloned()
                .collect())
        })
    }

    fn upsert_principal(&self, principal: Principal) -> BoxFuture<'_, Result<(), StoreError>> {
        Box::pin(async move {
            self.principals
                .lock()
                .insert(principal.principal_id.clone(), principal);
            Ok(())
        })
    }

    fn get_tenant(&self, tenant_id: &str) -> BoxFuture<'_, Result<Option<Tenant>, StoreError>> {
        let tenant_id = tenant_id.to_string();
        Box::pin(async move { Ok(self.tenants.lock().get(&tenant_id).cloned()) })
    }

    fn list_tenants(
        &self,
        filter: &TenantFilter,
    ) -> BoxFuture<'_, Result<Vec<Tenant>, StoreError>> {
        let filter = filter.clone();
        Box::pin(async move {
            Ok(self
                .tenants
                .lock()
                .values()
                .filter(|tenant| {
                    filter
                        .tenant_kind
                        .is_none_or(|kind| tenant.tenant_kind == kind)
                        && filter.status.is_none_or(|status| tenant.status == status)
                })
                .cloned()
                .collect())
        })
    }

    fn upsert_tenant(&self, tenant: Tenant) -> BoxFuture<'_, Result<(), StoreError>> {
        Box::pin(async move {
            self.tenants.lock().insert(tenant.tenant_id.clone(), tenant);
            Ok(())
        })
    }

    fn list_memberships(
        &self,
        filter: &MembershipFilter,
    ) -> BoxFuture<'_, Result<Vec<Membership>, StoreError>> {
        let filter = filter.clone();
        Box::pin(async move {
            Ok(self
                .memberships
                .lock()
                .values()
                .filter(|membership| {
                    (filter.tenant_id.is_empty() || membership.tenant_id == filter.tenant_id)
                        && filter
                            .status
                            .is_none_or(|status| membership.status == status)
                })
                .cloned()
                .collect())
        })
    }

    fn upsert_membership(&self, membership: Membership) -> BoxFuture<'_, Result<(), StoreError>> {
        Box::pin(async move {
            self.memberships
                .lock()
                .insert(membership.membership_id.clone(), membership);
            Ok(())
        })
    }

    fn list_token_tenant_grants(
        &self,
        token_id: &str,
    ) -> BoxFuture<'_, Result<Vec<TokenTenantGrant>, StoreError>> {
        let token_id = token_id.to_string();
        Box::pin(async move {
            Ok(self
                .grants
                .lock()
                .values()
                .filter(|grant| grant.token_id == token_id)
                .cloned()
                .collect())
        })
    }

    fn upsert_token_tenant_grant(
        &self,
        grant: TokenTenantGrant,
    ) -> BoxFuture<'_, Result<(), StoreError>> {
        Box::pin(async move {
            self.grants.lock().insert(grant.grant_id.clone(), grant);
            Ok(())
        })
    }
}

#[derive(Default)]
pub(crate) struct RecordingAuditSink {
    pub events: Mutex<Vec<TenantAuditEvent>>,
}

impl RecordingAuditSink {
    pub(crate) fn has_event(&self, kind: &str, reason: &str) -> bool {
        self.events.lock().iter().any(|event| {
            event.event_kind == kind && (reason.is_empty() || event.reason_code == reason)
        })
    }

    pub(crate) fn payload(&self) -> String {
        serde_json::to_string(&*self.events.lock()).unwrap_or_default()
    }
}

impl AuditSink for RecordingAuditSink {
    fn append_tenant_audit_event(
        &self,
        event: TenantAuditEvent,
    ) -> BoxFuture<'_, Result<TenantAuditEvent, StoreError>> {
        Box::pin(async move {
            self.events.lock().push(event.clone());
            Ok(event)
        })
    }
}

/// Audit sink that always fails (Go `failingActivationAuditSink`).
pub(crate) struct FailingAuditSink;

impl AuditSink for FailingAuditSink {
    fn append_tenant_audit_event(
        &self,
        _event: TenantAuditEvent,
    ) -> BoxFuture<'_, Result<TenantAuditEvent, StoreError>> {
        Box::pin(async { Err(boxed_err("audit unavailable")) })
    }
}

#[derive(Default)]
pub(crate) struct StaticBillingProjector {
    pub summary: Option<UsageSummary>,
    pub err: Option<kura_billing::BillingError>,
}

impl BillingProjector for StaticBillingProjector {
    fn usage_summary(
        &self,
        _tenant_id: &str,
        _hosted: bool,
    ) -> BoxFuture<'_, Result<UsageSummary, kura_billing::BillingError>> {
        let summary = self.summary.clone();
        let err = self.err.clone();
        Box::pin(async move {
            match err {
                Some(err) => Err(err),
                None => Ok(summary.unwrap_or_default()),
            }
        })
    }
}

pub(crate) struct RecordingChatRunner {
    pub result: TestChatResult,
    pub last: Mutex<Option<TestChatInput>>,
}

impl ChatRunner for RecordingChatRunner {
    fn run_activation_test_chat(
        &self,
        input: TestChatInput,
    ) -> BoxFuture<'_, Result<TestChatResult, ChatRunFailure>> {
        let result = self.result.clone();
        *self.last.lock() = Some(input);
        Box::pin(async move { Ok(result) })
    }
}

/// Chat runner that fails with a populated partial result (Go
/// `failingActivationChatRunner`).
pub(crate) struct FailingChatRunner;

impl ChatRunner for FailingChatRunner {
    fn run_activation_test_chat(
        &self,
        _input: TestChatInput,
    ) -> BoxFuture<'_, Result<TestChatResult, ChatRunFailure>> {
        Box::pin(async {
            Err(ChatRunFailure {
                result: TestChatResult {
                    dispatch_id: "dispatch_failed".to_string(),
                    status: crate::TestChatStatus::FAILED.into(),
                    provider: "test".to_string(),
                    model: "test-chat".to_string(),
                    usage: serde_json::Map::from_iter([
                        ("totalTokens".to_string(), serde_json::json!(1)),
                        ("prompt".to_string(), serde_json::json!("forbidden")),
                    ]),
                    finish_reason: "error".to_string(),
                    completed_at: None,
                },
                message: "upstream test chat failed".to_string(),
            })
        })
    }
}
