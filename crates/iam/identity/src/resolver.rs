//! Tenant context resolution from a token authority.
//!
//! Port of `daemon/internal/identity/resolver.go`. Fails closed: any lifecycle,
//! expiry, grant, or membership mismatch yields [`IdentityError::TenantAccessDenied`].

use std::sync::Arc;

use chrono::DateTime;
use chrono::Utc;

use crate::permissions::permissions_for_role;
use crate::types::IdentityError;
use crate::types::LifecycleStatus;
use crate::types::Membership;
use crate::types::MembershipFilter;
use crate::types::Principal;
use crate::types::Tenant;
use crate::types::TenantContext;
use crate::types::TokenTenantGrant;

pub const TENANT_SOURCE_DEFAULT: &str = "default";
pub const TENANT_SOURCE_EXPLICIT_HEADER: &str = "explicit_header";

/// Persistence surface the resolver needs. Implemented by the store layer.
pub trait ResolverStore {
    fn get_principal(&self, principal_id: &str) -> Result<Option<Principal>, IdentityError>;
    fn get_tenant(&self, tenant_id: &str) -> Result<Option<Tenant>, IdentityError>;
    fn list_memberships(&self, filter: &MembershipFilter)
    -> Result<Vec<Membership>, IdentityError>;
    fn list_token_tenant_grants(
        &self,
        token_id: &str,
    ) -> Result<Vec<TokenTenantGrant>, IdentityError>;
}

#[derive(Debug, Clone)]
pub struct TokenAuthority {
    pub token_id: String,
    pub principal_id: String,
    pub default_tenant_id: String,
    pub status: LifecycleStatus,
    pub expires_at: Option<DateTime<Utc>>,
}

pub struct Resolver<S: ResolverStore + ?Sized> {
    store: Arc<S>,
    now: Box<dyn Fn() -> DateTime<Utc> + Send + Sync>,
}

impl<S: ResolverStore + ?Sized> Resolver<S> {
    pub fn new(store: Arc<S>) -> Self {
        Self {
            store,
            now: Box::new(Utc::now),
        }
    }

    /// Overrides the clock; used by tests and deterministic harnesses.
    pub fn with_now(mut self, now: impl Fn() -> DateTime<Utc> + Send + Sync + 'static) -> Self {
        self.now = Box::new(now);
        self
    }

    pub fn resolve(
        &self,
        token: &TokenAuthority,
        selected_tenant_id: &str,
    ) -> Result<TenantContext, IdentityError> {
        let now = (self.now)();
        if token.token_id.is_empty()
            || token.principal_id.is_empty()
            || token.status != LifecycleStatus::Active
        {
            return Err(IdentityError::TenantAccessDenied);
        }
        if token.expires_at.is_some_and(|expires_at| expires_at <= now) {
            return Err(IdentityError::TenantAccessDenied);
        }
        let Some(principal) = self.store.get_principal(&token.principal_id)? else {
            return Err(IdentityError::TenantAccessDenied);
        };
        if principal.status != LifecycleStatus::Active {
            return Err(IdentityError::TenantAccessDenied);
        }

        let mut tenant_id = selected_tenant_id.to_string();
        let mut source = TENANT_SOURCE_EXPLICIT_HEADER;
        if tenant_id.is_empty() {
            tenant_id = token.default_tenant_id.clone();
            if tenant_id.is_empty() {
                tenant_id = principal.default_tenant_id.clone();
            }
            source = TENANT_SOURCE_DEFAULT;
        }
        if tenant_id.is_empty() {
            return Err(IdentityError::TenantAccessDenied);
        }

        let Some(tenant) = self.store.get_tenant(&tenant_id)? else {
            return Err(IdentityError::TenantAccessDenied);
        };
        if tenant.status != LifecycleStatus::Active {
            return Err(IdentityError::TenantAccessDenied);
        }
        if !self.token_has_tenant_grant(&token.token_id, &tenant_id) {
            return Err(IdentityError::TenantAccessDenied);
        }

        let memberships = self.store.list_memberships(&MembershipFilter {
            tenant_id: tenant_id.clone(),
            status: Some(LifecycleStatus::Active),
            role: None,
            limit: 500,
        })?;
        for membership in memberships {
            if membership.principal_id != principal.principal_id
                || membership.status != LifecycleStatus::Active
            {
                continue;
            }
            let perms = permissions_for_role(membership.role, membership.status);
            return Ok(TenantContext {
                principal_id: principal.principal_id.clone(),
                token_id: token.token_id.clone(),
                tenant_id,
                tenant_source: source.to_string(),
                membership_id: membership.membership_id.clone(),
                role: Some(membership.role),
                permissions: perms,
                resolved_at: now,
            });
        }
        Err(IdentityError::TenantAccessDenied)
    }

    fn token_has_tenant_grant(&self, token_id: &str, tenant_id: &str) -> bool {
        let Ok(grants) = self.store.list_token_tenant_grants(token_id) else {
            return false;
        };
        grants
            .iter()
            .any(|grant| grant.tenant_id == tenant_id && grant.status == LifecycleStatus::Active)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::TimeZone;

    use super::*;
    use crate::audit::Auditor;
    use crate::testutil::MemoryStore;
    use crate::types::Role;
    use crate::types::TenantAuditEvent;

    fn active_token(principal_id: &str, default_tenant_id: &str) -> TokenAuthority {
        TokenAuthority {
            token_id: "tok_1".to_string(),
            principal_id: principal_id.to_string(),
            default_tenant_id: default_tenant_id.to_string(),
            status: LifecycleStatus::Active,
            expires_at: None,
        }
    }

    #[test]
    fn resolve_default_explicit_and_denied_tenants() {
        let store = Arc::new(MemoryStore::new());
        let now = Utc.with_ymd_and_hms(2026, 4, 24, 0, 0, 0).unwrap();
        store.insert_principal("prn_1", LifecycleStatus::Active, "ten_default");
        for tenant_id in ["ten_default", "ten_other"] {
            store.insert_tenant(tenant_id, LifecycleStatus::Active);
            store.insert_membership(
                &format!("mem_{tenant_id}"),
                tenant_id,
                "prn_1",
                Role::Owner,
                LifecycleStatus::Active,
            );
            store.insert_grant(
                &format!("grant_{tenant_id}"),
                "tok_1",
                tenant_id,
                LifecycleStatus::Active,
            );
        }
        let resolver = Resolver::new(store).with_now(move || now);
        let token = active_token("prn_1", "ten_default");

        let default_ctx = resolver.resolve(&token, "").expect("default resolve");
        assert_eq!(default_ctx.tenant_id, "ten_default");
        assert_eq!(default_ctx.tenant_source, TENANT_SOURCE_DEFAULT);

        let explicit_ctx = resolver
            .resolve(&token, "ten_other")
            .expect("explicit resolve");
        assert_eq!(explicit_ctx.tenant_id, "ten_other");
        assert_eq!(explicit_ctx.tenant_source, TENANT_SOURCE_EXPLICIT_HEADER);

        assert!(matches!(
            resolver.resolve(&token, "ten_missing"),
            Err(IdentityError::TenantAccessDenied)
        ));
    }

    #[test]
    fn resolve_denies_lifecycle_and_grant_failures() {
        let store = Arc::new(MemoryStore::new());
        store.insert_principal("prn_1", LifecycleStatus::Disabled, "ten_1");
        store.insert_tenant("ten_1", LifecycleStatus::Active);
        store.insert_membership(
            "mem_1",
            "ten_1",
            "prn_1",
            Role::Owner,
            LifecycleStatus::Active,
        );
        store.insert_grant("grant_1", "tok_1", "ten_1", LifecycleStatus::Active);
        let resolver = Resolver::new(store.clone());

        assert!(matches!(
            resolver.resolve(&active_token("prn_1", ""), "ten_1"),
            Err(IdentityError::TenantAccessDenied)
        ));

        store.insert_principal("prn_1", LifecycleStatus::Active, "ten_1");
        store.insert_membership(
            "mem_1",
            "ten_1",
            "prn_1",
            Role::Owner,
            LifecycleStatus::Removed,
        );
        assert!(matches!(
            resolver.resolve(&active_token("prn_1", ""), "ten_1"),
            Err(IdentityError::TenantAccessDenied)
        ));
    }

    #[test]
    fn resolve_denies_expired_and_inactive_tokens() {
        let store = Arc::new(MemoryStore::new());
        store.insert_principal("prn_1", LifecycleStatus::Active, "ten_1");
        store.insert_tenant("ten_1", LifecycleStatus::Active);
        store.insert_membership(
            "mem_1",
            "ten_1",
            "prn_1",
            Role::Owner,
            LifecycleStatus::Active,
        );
        store.insert_grant("grant_1", "tok_1", "ten_1", LifecycleStatus::Active);
        let resolver = Resolver::new(store);

        let mut token = active_token("prn_1", "ten_1");
        token.status = LifecycleStatus::Revoked;
        assert!(matches!(
            resolver.resolve(&token, ""),
            Err(IdentityError::TenantAccessDenied)
        ));

        let mut token = active_token("prn_1", "ten_1");
        token.expires_at = Some(Utc::now() - chrono::Duration::minutes(1));
        assert!(matches!(
            resolver.resolve(&token, ""),
            Err(IdentityError::TenantAccessDenied)
        ));
    }

    #[test]
    fn resolve_uses_bounded_store_lookups_for_large_membership_set() {
        #[derive(Default)]
        struct CountingStore {
            inner: MemoryStore,
            get_principal_calls: parking_lot::Mutex<usize>,
            get_tenant_calls: parking_lot::Mutex<usize>,
            list_membership_calls: parking_lot::Mutex<usize>,
            list_grant_calls: parking_lot::Mutex<usize>,
        }

        impl ResolverStore for CountingStore {
            fn get_principal(
                &self,
                principal_id: &str,
            ) -> Result<Option<Principal>, IdentityError> {
                *self.get_principal_calls.lock() += 1;
                self.inner.get_principal(principal_id)
            }
            fn get_tenant(&self, tenant_id: &str) -> Result<Option<Tenant>, IdentityError> {
                *self.get_tenant_calls.lock() += 1;
                self.inner.get_tenant(tenant_id)
            }
            fn list_memberships(
                &self,
                filter: &MembershipFilter,
            ) -> Result<Vec<Membership>, IdentityError> {
                *self.list_membership_calls.lock() += 1;
                self.inner.list_memberships(filter)
            }
            fn list_token_tenant_grants(
                &self,
                token_id: &str,
            ) -> Result<Vec<TokenTenantGrant>, IdentityError> {
                *self.list_grant_calls.lock() += 1;
                self.inner.list_token_tenant_grants(token_id)
            }
        }

        let counting = Arc::new(CountingStore::default());
        counting
            .inner
            .insert_principal("prn_1", LifecycleStatus::Active, "ten_199");
        for idx in 0..250 {
            let tenant_id = format!("ten_{idx}");
            counting
                .inner
                .insert_tenant(&tenant_id, LifecycleStatus::Active);
            counting.inner.insert_membership(
                &format!("mem_{tenant_id}"),
                &tenant_id,
                "prn_1",
                Role::Viewer,
                LifecycleStatus::Active,
            );
            counting.inner.insert_grant(
                &format!("grant_{tenant_id}"),
                "tok_1",
                &tenant_id,
                LifecycleStatus::Active,
            );
        }
        let resolver = Resolver::new(counting.clone());

        let tenant_context = resolver
            .resolve(&active_token("prn_1", "ten_199"), "")
            .expect("resolve");
        assert_eq!(tenant_context.tenant_id, "ten_199");
        assert_eq!(*counting.get_principal_calls.lock(), 1);
        assert_eq!(*counting.get_tenant_calls.lock(), 1);
        assert_eq!(*counting.list_membership_calls.lock(), 1);
        assert_eq!(*counting.list_grant_calls.lock(), 1);
    }

    #[test]
    fn auditor_require_fails_closed() {
        let store = Arc::new(MemoryStore::new());
        store.fail_audits();
        let auditor = Auditor::new(store);
        assert!(matches!(
            auditor.require(TenantAuditEvent {
                event_kind: "tenant.access_denied".to_string(),
                ..TenantAuditEvent::default()
            }),
            Err(IdentityError::AuditWriteFailed)
        ));
    }
}
