//! In-memory [`Store`] implementation shared by the crate's tests.

use std::collections::HashMap;

use parking_lot::Mutex;

use crate::audit::AuditStore;
use crate::manager::Store;
use crate::resolver::ResolverStore;
use crate::resolver::TokenAuthority;
use crate::types::IdentityError;
use crate::types::InvitationFilter;
use crate::types::Membership;
use crate::types::MembershipFilter;
use crate::types::Principal;
use crate::types::PrincipalFilter;
use crate::types::Tenant;
use crate::types::TenantAuditEvent;
use crate::types::TenantFilter;
use crate::types::TenantInvitation;
use crate::types::TokenTenantGrant;

#[derive(Debug, Default)]
pub(crate) struct MemoryStore {
    pub(crate) tenants: Mutex<HashMap<String, Tenant>>,
    pub(crate) principals: Mutex<HashMap<String, Principal>>,
    pub(crate) memberships: Mutex<HashMap<String, Membership>>,
    pub(crate) grants: Mutex<HashMap<String, TokenTenantGrant>>,
    pub(crate) invitations: Mutex<HashMap<String, TenantInvitation>>,
    pub(crate) audits: Mutex<Vec<TenantAuditEvent>>,
    pub(crate) audit_fail: Mutex<bool>,
}

impl MemoryStore {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn fail_audits(&self) {
        *self.audit_fail.lock() = true;
    }

    pub(crate) fn insert_tenant(
        &self,
        tenant_id: &str,
        status: crate::types::LifecycleStatus,
    ) -> Tenant {
        let tenant = Tenant {
            tenant_id: tenant_id.to_string(),
            tenant_kind: crate::types::TenantKind::Organization,
            display_name: tenant_id.to_string(),
            status,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            created_by_principal_id: String::new(),
            default_owner_principal_id: String::new(),
            caller_membership_role: None,
            caller_membership_status: None,
            caller_permissions: Vec::new(),
            default_for_current_token: false,
            default_for_current_principal: false,
        };
        self.tenants
            .lock()
            .insert(tenant_id.to_string(), tenant.clone());
        tenant
    }

    pub(crate) fn insert_principal(
        &self,
        principal_id: &str,
        status: crate::types::LifecycleStatus,
        default_tenant_id: &str,
    ) -> Principal {
        let principal = Principal {
            principal_id: principal_id.to_string(),
            principal_kind: crate::types::PrincipalKind::User,
            display_name: principal_id.to_string(),
            status,
            default_tenant_id: default_tenant_id.to_string(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            disabled_at: None,
            removed_at: None,
        };
        self.principals
            .lock()
            .insert(principal_id.to_string(), principal.clone());
        principal
    }

    pub(crate) fn insert_membership(
        &self,
        membership_id: &str,
        tenant_id: &str,
        principal_id: &str,
        role: crate::types::Role,
        status: crate::types::LifecycleStatus,
    ) -> Membership {
        let membership = Membership {
            membership_id: membership_id.to_string(),
            tenant_id: tenant_id.to_string(),
            principal_id: principal_id.to_string(),
            role,
            status,
            invitation_id: String::new(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            accepted_at: None,
            removed_at: None,
        };
        self.memberships
            .lock()
            .insert(membership_id.to_string(), membership.clone());
        membership
    }

    pub(crate) fn insert_grant(
        &self,
        grant_id: &str,
        token_id: &str,
        tenant_id: &str,
        status: crate::types::LifecycleStatus,
    ) -> TokenTenantGrant {
        let grant = TokenTenantGrant {
            grant_id: grant_id.to_string(),
            token_id: token_id.to_string(),
            tenant_id: tenant_id.to_string(),
            is_default: false,
            status,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            revoked_at: None,
            granted_by_principal_id: String::new(),
        };
        self.grants
            .lock()
            .insert(grant_id.to_string(), grant.clone());
        grant
    }
}

impl ResolverStore for MemoryStore {
    fn get_principal(&self, principal_id: &str) -> Result<Option<Principal>, IdentityError> {
        Ok(self.principals.lock().get(principal_id).cloned())
    }

    fn get_tenant(&self, tenant_id: &str) -> Result<Option<Tenant>, IdentityError> {
        Ok(self.tenants.lock().get(tenant_id).cloned())
    }

    fn list_memberships(
        &self,
        filter: &MembershipFilter,
    ) -> Result<Vec<Membership>, IdentityError> {
        Ok(self
            .memberships
            .lock()
            .values()
            .filter(|item| filter.tenant_id.is_empty() || item.tenant_id == filter.tenant_id)
            .filter(|item| filter.status.is_none_or(|status| item.status == status))
            .filter(|item| filter.role.is_none_or(|role| item.role == role))
            .cloned()
            .collect())
    }

    fn list_token_tenant_grants(
        &self,
        token_id: &str,
    ) -> Result<Vec<TokenTenantGrant>, IdentityError> {
        Ok(self
            .grants
            .lock()
            .values()
            .filter(|item| item.token_id == token_id)
            .cloned()
            .collect())
    }
}

impl AuditStore for MemoryStore {
    fn append_tenant_audit_event(
        &self,
        event: TenantAuditEvent,
    ) -> Result<TenantAuditEvent, IdentityError> {
        if *self.audit_fail.lock() {
            return Err(IdentityError::Store("audit store down".into()));
        }
        self.audits.lock().push(event.clone());
        Ok(event)
    }
}

impl Store for MemoryStore {
    fn upsert_tenant(&self, tenant: &Tenant) -> Result<(), IdentityError> {
        self.tenants
            .lock()
            .insert(tenant.tenant_id.clone(), tenant.clone());
        Ok(())
    }

    fn upsert_principal(&self, principal: &Principal) -> Result<(), IdentityError> {
        self.principals
            .lock()
            .insert(principal.principal_id.clone(), principal.clone());
        Ok(())
    }

    fn upsert_membership(&self, membership: &Membership) -> Result<(), IdentityError> {
        self.memberships
            .lock()
            .insert(membership.membership_id.clone(), membership.clone());
        Ok(())
    }

    fn upsert_tenant_invitation(&self, invitation: &TenantInvitation) -> Result<(), IdentityError> {
        self.invitations
            .lock()
            .insert(invitation.invitation_id.clone(), invitation.clone());
        Ok(())
    }

    fn upsert_token_tenant_grant(&self, grant: &TokenTenantGrant) -> Result<(), IdentityError> {
        self.grants
            .lock()
            .insert(grant.grant_id.clone(), grant.clone());
        Ok(())
    }

    fn list_tenants(&self, _filter: &TenantFilter) -> Result<Vec<Tenant>, IdentityError> {
        Ok(self.tenants.lock().values().cloned().collect())
    }

    fn list_principals(&self, filter: &PrincipalFilter) -> Result<Vec<Principal>, IdentityError> {
        let principals = self.principals.lock();
        let mut items: Vec<Principal> = principals.values().cloned().collect();
        if filter.limit > 0 {
            items.truncate(filter.limit);
        }
        Ok(items)
    }

    fn list_tenant_invitations(
        &self,
        _filter: &InvitationFilter,
    ) -> Result<Vec<TenantInvitation>, IdentityError> {
        Ok(self.invitations.lock().values().cloned().collect())
    }

    fn list_token_authorities(&self) -> Result<Vec<TokenAuthority>, IdentityError> {
        Ok(Vec::new())
    }
}
