//! Tenant, membership, invitation, and token-grant management.
//!
//! Port of `daemon/internal/identity/manager.go`. Every mutation writes an
//! audit event first and fails closed when the audit write fails.

use std::collections::HashSet;
use std::sync::Arc;

use chrono::DateTime;
use chrono::Utc;

use crate::audit::AUDIT_OUTCOME_SUCCEEDED;
use crate::audit::AuditStore;
use crate::audit::Auditor;
use crate::permissions::require_permission;
use crate::resolver::Resolver;
use crate::resolver::ResolverStore;
use crate::resolver::TokenAuthority;
use crate::types::IdentityError;
use crate::types::InvitationFilter;
use crate::types::LifecycleStatus;
use crate::types::Membership;
use crate::types::MembershipFilter;
use crate::types::Permission;
use crate::types::Principal;
use crate::types::PrincipalFilter;
use crate::types::PrincipalKind;
use crate::types::Role;
use crate::types::Tenant;
use crate::types::TenantAuditEvent;
use crate::types::TenantContext;
use crate::types::TenantFilter;
use crate::types::TenantInvitation;
use crate::types::TenantKind;
use crate::types::TokenTenantGrant;

/// Full persistence surface for the manager: resolver reads, audit appends,
/// and the upsert/list operations the manager itself needs.
pub trait Store: ResolverStore + AuditStore {
    fn upsert_tenant(&self, tenant: &Tenant) -> Result<(), IdentityError>;
    fn upsert_principal(&self, principal: &Principal) -> Result<(), IdentityError>;
    fn upsert_membership(&self, membership: &Membership) -> Result<(), IdentityError>;
    fn upsert_tenant_invitation(&self, invitation: &TenantInvitation) -> Result<(), IdentityError>;
    fn upsert_token_tenant_grant(&self, grant: &TokenTenantGrant) -> Result<(), IdentityError>;
    fn list_tenants(&self, filter: &TenantFilter) -> Result<Vec<Tenant>, IdentityError>;
    fn list_principals(&self, filter: &PrincipalFilter) -> Result<Vec<Principal>, IdentityError>;
    fn list_tenant_invitations(
        &self,
        filter: &InvitationFilter,
    ) -> Result<Vec<TenantInvitation>, IdentityError>;
    fn list_token_authorities(&self) -> Result<Vec<TokenAuthority>, IdentityError>;
}

#[derive(Debug, Clone)]
pub struct CreateInvitationInput {
    pub tenant_id: String,
    pub invited_principal_id: String,
    pub role: Option<Role>,
    pub expires_at: Option<DateTime<Utc>>,
}

pub struct Manager<S: Store + ?Sized> {
    store: Arc<S>,
    auditor: Auditor<S>,
    resolver: Resolver<S>,
    now: Box<dyn Fn() -> DateTime<Utc> + Send + Sync>,
}

impl<S: Store + ?Sized> Manager<S> {
    pub fn new(store: Arc<S>) -> Self {
        Self {
            auditor: Auditor::new(store.clone()),
            resolver: Resolver::new(store.clone()),
            store,
            now: Box::new(Utc::now),
        }
    }

    pub fn with_now(mut self, now: impl Fn() -> DateTime<Utc> + Send + Sync + 'static) -> Self {
        self.now = Box::new(now);
        self
    }

    fn now(&self) -> DateTime<Utc> {
        (self.now)()
    }

    pub fn resolve(
        &self,
        token: &TokenAuthority,
        tenant_id: &str,
    ) -> Result<TenantContext, IdentityError> {
        self.resolver.resolve(token, tenant_id)
    }

    pub fn create_organization_tenant(
        &self,
        actor: &TenantContext,
        display_name: &str,
    ) -> Result<(Tenant, Membership), IdentityError> {
        require_permission(actor, Permission::TenantManage)?;
        let now = self.now();
        let tenant = Tenant {
            tenant_id: format!("ten_{}", random_id(8)),
            tenant_kind: TenantKind::Organization,
            display_name: display_name.trim().to_string(),
            status: LifecycleStatus::Active,
            created_at: now,
            updated_at: now,
            created_by_principal_id: actor.principal_id.clone(),
            default_owner_principal_id: actor.principal_id.clone(),
            caller_membership_role: None,
            caller_membership_status: None,
            caller_permissions: Vec::new(),
            default_for_current_token: false,
            default_for_current_principal: false,
        };
        if tenant.display_name.is_empty() {
            return Err(IdentityError::TenantInvalid);
        }
        let membership = Membership {
            membership_id: format!("mem_{}", random_id(8)),
            tenant_id: tenant.tenant_id.clone(),
            principal_id: actor.principal_id.clone(),
            role: Role::Owner,
            status: LifecycleStatus::Active,
            invitation_id: String::new(),
            created_at: now,
            updated_at: now,
            accepted_at: Some(now),
            removed_at: None,
        };
        self.auditor.require(TenantAuditEvent {
            audit_event_id: format!("audit_{}", random_id(8)),
            event_kind: "tenant.organization_created".to_string(),
            tenant_id: tenant.tenant_id.clone(),
            principal_id: actor.principal_id.clone(),
            outcome: AUDIT_OUTCOME_SUCCEEDED.to_string(),
            reason_code: "organization_created".to_string(),
            created_at: now,
            ..TenantAuditEvent::default()
        })?;
        self.store.upsert_tenant(&tenant)?;
        self.store.upsert_membership(&membership)?;
        if !actor.token_id.is_empty() {
            self.store.upsert_token_tenant_grant(&TokenTenantGrant {
                grant_id: format!("grant_{}", random_id(8)),
                token_id: actor.token_id.clone(),
                tenant_id: tenant.tenant_id.clone(),
                is_default: false,
                status: LifecycleStatus::Active,
                created_at: now,
                updated_at: now,
                revoked_at: None,
                granted_by_principal_id: actor.principal_id.clone(),
            })?;
        }
        Ok((tenant, membership))
    }

    pub fn create_invitation(
        &self,
        actor: &TenantContext,
        input: CreateInvitationInput,
    ) -> Result<TenantInvitation, IdentityError> {
        require_permission(actor, Permission::TenantManage)?;
        let Some(role) = input.role else {
            return Err(IdentityError::InvitationInvalid);
        };
        if input.tenant_id.is_empty() || input.invited_principal_id.is_empty() {
            return Err(IdentityError::InvitationInvalid);
        }
        if input.tenant_id != actor.tenant_id {
            return Err(IdentityError::TenantAccessDenied);
        }
        if self
            .store
            .get_principal(&input.invited_principal_id)?
            .is_none()
        {
            return Err(IdentityError::PrincipalInvalid);
        }
        let now = self.now();
        let invitation = TenantInvitation {
            invitation_id: format!("inv_{}", random_id(8)),
            tenant_id: input.tenant_id,
            invited_principal_id: input.invited_principal_id,
            invited_by_principal_id: actor.principal_id.clone(),
            role,
            status: LifecycleStatus::Invited,
            created_at: now,
            updated_at: now,
            expires_at: input.expires_at,
            decided_at: None,
        };
        self.auditor.require(TenantAuditEvent {
            audit_event_id: format!("audit_{}", random_id(8)),
            event_kind: "tenant.invitation_created".to_string(),
            tenant_id: invitation.tenant_id.clone(),
            principal_id: actor.principal_id.clone(),
            target_principal_id: invitation.invited_principal_id.clone(),
            outcome: AUDIT_OUTCOME_SUCCEEDED.to_string(),
            reason_code: "invitation_created".to_string(),
            created_at: now,
            ..TenantAuditEvent::default()
        })?;
        self.store.upsert_tenant_invitation(&invitation)?;
        Ok(invitation)
    }

    pub fn accept_invitation(
        &self,
        principal_id: &str,
        invitation_id: &str,
    ) -> Result<Membership, IdentityError> {
        let mut invitation = self.find_invitation(invitation_id)?;
        let now = self.now();
        if invitation.invited_principal_id != principal_id
            || invitation.status != LifecycleStatus::Invited
            || invitation
                .expires_at
                .is_some_and(|expires_at| expires_at <= now)
        {
            return Err(IdentityError::InvitationInvalid);
        }
        let Some(principal) = self.store.get_principal(principal_id)? else {
            return Err(IdentityError::PrincipalInvalid);
        };
        if principal.status != LifecycleStatus::Active {
            return Err(IdentityError::PrincipalInvalid);
        }
        invitation.status = LifecycleStatus::Accepted;
        invitation.updated_at = now;
        invitation.decided_at = Some(now);
        let membership = Membership {
            membership_id: format!("mem_{}", random_id(8)),
            tenant_id: invitation.tenant_id.clone(),
            principal_id: principal_id.to_string(),
            role: invitation.role,
            status: LifecycleStatus::Active,
            invitation_id: invitation.invitation_id.clone(),
            created_at: now,
            updated_at: now,
            accepted_at: Some(now),
            removed_at: None,
        };
        self.auditor.require(TenantAuditEvent {
            audit_event_id: format!("audit_{}", random_id(8)),
            event_kind: "tenant.invitation_accepted".to_string(),
            tenant_id: invitation.tenant_id.clone(),
            principal_id: principal_id.to_string(),
            outcome: AUDIT_OUTCOME_SUCCEEDED.to_string(),
            reason_code: "invitation_accepted".to_string(),
            created_at: now,
            ..TenantAuditEvent::default()
        })?;
        self.store.upsert_tenant_invitation(&invitation)?;
        self.store.upsert_membership(&membership)?;
        Ok(membership)
    }

    pub fn decide_invitation(
        &self,
        principal_id: &str,
        invitation_id: &str,
        status: LifecycleStatus,
    ) -> Result<TenantInvitation, IdentityError> {
        if !matches!(
            status,
            LifecycleStatus::Rejected | LifecycleStatus::Revoked | LifecycleStatus::Expired
        ) {
            return Err(IdentityError::InvitationInvalid);
        }
        let mut invitation = self.find_invitation(invitation_id)?;
        if status == LifecycleStatus::Rejected && invitation.invited_principal_id != principal_id {
            return Err(IdentityError::InvitationInvalid);
        }
        let now = self.now();
        invitation.status = status;
        invitation.updated_at = now;
        invitation.decided_at = Some(now);
        let status_label = lifecycle_label(status);
        self.auditor.require(TenantAuditEvent {
            audit_event_id: format!("audit_{}", random_id(8)),
            event_kind: format!("tenant.invitation_{status_label}"),
            tenant_id: invitation.tenant_id.clone(),
            principal_id: principal_id.to_string(),
            target_principal_id: invitation.invited_principal_id.clone(),
            outcome: AUDIT_OUTCOME_SUCCEEDED.to_string(),
            reason_code: format!("invitation_{status_label}"),
            created_at: now,
            ..TenantAuditEvent::default()
        })?;
        self.store.upsert_tenant_invitation(&invitation)?;
        Ok(invitation)
    }

    pub fn update_membership_role(
        &self,
        actor: &TenantContext,
        tenant_id: &str,
        membership_id: &str,
        role: Role,
    ) -> Result<Membership, IdentityError> {
        require_permission(actor, Permission::TenantManage)?;
        let mut membership = self.find_membership(tenant_id, membership_id)?;
        if membership.role == Role::Owner && role != Role::Owner {
            self.ensure_another_active_owner(tenant_id, membership_id)?;
        }
        let now = self.now();
        let old_role = membership.role;
        membership.role = role;
        membership.updated_at = now;
        self.auditor.require(TenantAuditEvent {
            audit_event_id: format!("audit_{}", random_id(8)),
            event_kind: "tenant.membership_role_updated".to_string(),
            tenant_id: tenant_id.to_string(),
            principal_id: actor.principal_id.clone(),
            target_principal_id: membership.principal_id.clone(),
            outcome: AUDIT_OUTCOME_SUCCEEDED.to_string(),
            reason_code: "membership_role_updated".to_string(),
            created_at: now,
            document: Some(
                serde_json::json!({
                    "membershipId": membership.membership_id,
                    "oldRole": old_role,
                    "newRole": role,
                })
                .as_object()
                .cloned()
                .unwrap_or_default(),
            ),
            ..TenantAuditEvent::default()
        })?;
        self.store.upsert_membership(&membership)?;
        Ok(membership)
    }

    pub fn remove_membership(
        &self,
        actor: &TenantContext,
        tenant_id: &str,
        membership_id: &str,
    ) -> Result<Membership, IdentityError> {
        require_permission(actor, Permission::TenantManage)?;
        let mut membership = self.find_membership(tenant_id, membership_id)?;
        if membership.role == Role::Owner {
            self.ensure_another_active_owner(tenant_id, membership_id)?;
        }
        let now = self.now();
        membership.status = LifecycleStatus::Removed;
        membership.updated_at = now;
        membership.removed_at = Some(now);
        self.auditor.require(TenantAuditEvent {
            audit_event_id: format!("audit_{}", random_id(8)),
            event_kind: "tenant.membership_removed".to_string(),
            tenant_id: tenant_id.to_string(),
            principal_id: actor.principal_id.clone(),
            target_principal_id: membership.principal_id.clone(),
            outcome: AUDIT_OUTCOME_SUCCEEDED.to_string(),
            reason_code: "membership_removed".to_string(),
            created_at: now,
            document: Some(
                serde_json::json!({
                    "membershipId": membership.membership_id,
                    "oldRole": membership.role,
                    "newStatus": LifecycleStatus::Removed,
                })
                .as_object()
                .cloned()
                .unwrap_or_default(),
            ),
            ..TenantAuditEvent::default()
        })?;
        self.store.upsert_membership(&membership)?;
        Ok(membership)
    }

    pub fn replace_token_tenant_grants(
        &self,
        actor: &TenantContext,
        token_id: &str,
        tenant_ids: &[String],
        default_tenant_id: &str,
    ) -> Result<Vec<TokenTenantGrant>, IdentityError> {
        require_permission(actor, Permission::TenantManage)?;
        let target_principal_id = self.token_principal_id(actor, token_id)?;
        let seen = self.validate_token_tenant_grant_set(
            &target_principal_id,
            tenant_ids,
            default_tenant_id,
        )?;
        if default_tenant_id.is_empty() {
            return Err(IdentityError::TokenGrantInvalid);
        }
        let now = self.now();
        self.auditor.require(TenantAuditEvent {
            audit_event_id: format!("audit_{}", random_id(8)),
            event_kind: "tenant.token_grants_changed".to_string(),
            tenant_id: default_tenant_id.to_string(),
            principal_id: actor.principal_id.clone(),
            target_principal_id: target_principal_id,
            token_id: token_id.to_string(),
            outcome: AUDIT_OUTCOME_SUCCEEDED.to_string(),
            reason_code: "token_grants_changed".to_string(),
            created_at: now,
            ..TenantAuditEvent::default()
        })?;
        let existing = self.store.list_token_tenant_grants(token_id)?;
        for grant in &existing {
            if seen.contains(&grant.tenant_id) && grant.status == LifecycleStatus::Active {
                continue;
            }
            if grant.status == LifecycleStatus::Active {
                let mut revoked = grant.clone();
                revoked.status = LifecycleStatus::Revoked;
                revoked.updated_at = now;
                revoked.revoked_at = Some(now);
                self.store.upsert_token_tenant_grant(&revoked)?;
            }
        }
        let mut result = Vec::with_capacity(seen.len());
        for tenant_id in &seen {
            let existing_grant = existing.iter().find(|g| g.tenant_id == *tenant_id).cloned();
            let mut grant = match existing_grant {
                Some(g) if !g.grant_id.is_empty() && g.status == LifecycleStatus::Active => g,
                _ => TokenTenantGrant {
                    grant_id: format!("grant_{}", random_id(8)),
                    token_id: token_id.to_string(),
                    tenant_id: tenant_id.clone(),
                    is_default: false,
                    status: LifecycleStatus::Active,
                    created_at: now,
                    updated_at: now,
                    revoked_at: None,
                    granted_by_principal_id: String::new(),
                },
            };
            grant.is_default = *tenant_id == default_tenant_id;
            grant.updated_at = now;
            grant.revoked_at = None;
            grant.granted_by_principal_id = actor.principal_id.clone();
            self.store.upsert_token_tenant_grant(&grant)?;
            result.push(grant);
        }
        Ok(result)
    }

    pub fn validate_token_tenant_grants(
        &self,
        actor: &TenantContext,
        token_id: &str,
        tenant_ids: &[String],
        default_tenant_id: &str,
    ) -> Result<(), IdentityError> {
        require_permission(actor, Permission::TenantManage)?;
        let target_principal_id = self.token_principal_id(actor, token_id)?;
        self.validate_token_tenant_grant_set(&target_principal_id, tenant_ids, default_tenant_id)?;
        Ok(())
    }

    /// Creates the local operator principal and personal tenant on first run,
    /// or returns the existing pair, and ensures every given token holds a
    /// default grant on the tenant.
    pub fn bootstrap_local(
        &self,
        token_ids: &[String],
    ) -> Result<(Principal, Tenant), IdentityError> {
        let now = self.now();
        let principals = self.store.list_principals(&PrincipalFilter {
            tenant_id: String::new(),
            status: None,
            limit: 1,
        })?;
        if let Some(principal) = principals.into_iter().next() {
            if let Some(tenant) = self.store.get_tenant(&principal.default_tenant_id)? {
                self.ensure_token_grants(&principal, &tenant, token_ids)?;
                return Ok((principal, tenant));
            }
        }
        let principal = Principal {
            principal_id: format!("prn_{}", random_id(8)),
            principal_kind: PrincipalKind::LocalOperator,
            display_name: "Local operator".to_string(),
            status: LifecycleStatus::Active,
            default_tenant_id: format!("ten_{}", random_id(8)),
            created_at: now,
            updated_at: now,
            disabled_at: None,
            removed_at: None,
        };
        let tenant = Tenant {
            tenant_id: principal.default_tenant_id.clone(),
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
        let membership = Membership {
            membership_id: format!("mem_{}", random_id(8)),
            tenant_id: tenant.tenant_id.clone(),
            principal_id: principal.principal_id.clone(),
            role: Role::Owner,
            status: LifecycleStatus::Active,
            invitation_id: String::new(),
            created_at: now,
            updated_at: now,
            accepted_at: Some(now),
            removed_at: None,
        };
        self.store.upsert_principal(&principal)?;
        self.store.upsert_tenant(&tenant)?;
        self.store.upsert_membership(&membership)?;
        self.auditor.require(TenantAuditEvent {
            audit_event_id: format!("audit_{}", random_id(8)),
            event_kind: "tenant.bootstrap_completed".to_string(),
            tenant_id: tenant.tenant_id.clone(),
            principal_id: principal.principal_id.clone(),
            outcome: AUDIT_OUTCOME_SUCCEEDED.to_string(),
            reason_code: "local_bootstrap".to_string(),
            created_at: now,
            ..TenantAuditEvent::default()
        })?;
        self.ensure_token_grants(&principal, &tenant, token_ids)?;
        Ok((principal, tenant))
    }

    fn ensure_token_grants(
        &self,
        principal: &Principal,
        tenant: &Tenant,
        token_ids: &[String],
    ) -> Result<(), IdentityError> {
        let now = self.now();
        for token_id in token_ids {
            let token_id = token_id.trim();
            if token_id.is_empty() {
                continue;
            }
            let grants = self.store.list_token_tenant_grants(token_id)?;
            let has_grant = grants.iter().any(|grant| {
                grant.tenant_id == tenant.tenant_id && grant.status == LifecycleStatus::Active
            });
            if has_grant {
                continue;
            }
            self.store.upsert_token_tenant_grant(&TokenTenantGrant {
                grant_id: format!("grant_{}", random_id(8)),
                token_id: token_id.to_string(),
                tenant_id: tenant.tenant_id.clone(),
                is_default: true,
                status: LifecycleStatus::Active,
                created_at: now,
                updated_at: now,
                revoked_at: None,
                granted_by_principal_id: principal.principal_id.clone(),
            })?;
        }
        Ok(())
    }

    fn token_principal_id(
        &self,
        actor: &TenantContext,
        token_id: &str,
    ) -> Result<String, IdentityError> {
        if token_id.is_empty() {
            return Err(IdentityError::TokenGrantInvalid);
        }
        if token_id == actor.token_id {
            return Ok(actor.principal_id.clone());
        }
        let tokens = self.store.list_token_authorities()?;
        for token in tokens {
            if token.token_id == token_id && !token.principal_id.is_empty() {
                return Ok(token.principal_id);
            }
        }
        Err(IdentityError::TokenGrantInvalid)
    }

    fn validate_token_tenant_grant_set(
        &self,
        principal_id: &str,
        tenant_ids: &[String],
        default_tenant_id: &str,
    ) -> Result<HashSet<String>, IdentityError> {
        if principal_id.is_empty() || tenant_ids.is_empty() || default_tenant_id.is_empty() {
            return Err(IdentityError::TokenGrantInvalid);
        }
        let mut allowed = HashSet::new();
        let memberships = self.store.list_memberships(&MembershipFilter {
            tenant_id: String::new(),
            status: Some(LifecycleStatus::Active),
            role: None,
            limit: 1000,
        })?;
        for membership in memberships {
            if membership.principal_id == principal_id
                && membership.status == LifecycleStatus::Active
            {
                allowed.insert(membership.tenant_id);
            }
        }
        let mut seen = HashSet::new();
        for tenant_id in tenant_ids {
            let tenant_id = tenant_id.trim();
            if tenant_id.is_empty() {
                return Err(IdentityError::TokenGrantInvalid);
            }
            if !allowed.contains(tenant_id) {
                return Err(IdentityError::TokenGrantInvalid);
            }
            seen.insert(tenant_id.to_string());
        }
        if !seen.contains(default_tenant_id) {
            return Err(IdentityError::TokenGrantInvalid);
        }
        Ok(seen)
    }

    fn find_invitation(&self, invitation_id: &str) -> Result<TenantInvitation, IdentityError> {
        let invitations = self.store.list_tenant_invitations(&InvitationFilter {
            tenant_id: String::new(),
            principal_id: String::new(),
            status: None,
            limit: 1000,
        })?;
        invitations
            .into_iter()
            .find(|invitation| invitation.invitation_id == invitation_id)
            .ok_or(IdentityError::InvitationInvalid)
    }

    fn find_membership(
        &self,
        tenant_id: &str,
        membership_id: &str,
    ) -> Result<Membership, IdentityError> {
        let memberships = self.store.list_memberships(&MembershipFilter {
            tenant_id: tenant_id.to_string(),
            status: None,
            role: None,
            limit: 1000,
        })?;
        memberships
            .into_iter()
            .find(|membership| membership.membership_id == membership_id)
            .ok_or(IdentityError::MembershipInvalid)
    }

    fn ensure_another_active_owner(
        &self,
        tenant_id: &str,
        membership_id: &str,
    ) -> Result<(), IdentityError> {
        let memberships = self.store.list_memberships(&MembershipFilter {
            tenant_id: tenant_id.to_string(),
            status: Some(LifecycleStatus::Active),
            role: Some(Role::Owner),
            limit: 1000,
        })?;
        for membership in memberships {
            if membership.membership_id != membership_id
                && membership.status == LifecycleStatus::Active
                && membership.role == Role::Owner
            {
                return Ok(());
            }
        }
        Err(IdentityError::OwnerInvariant)
    }
}

pub(crate) fn random_id(size: usize) -> String {
    let mut buf = vec![0u8; size];
    rand::fill(&mut buf[..]);
    hex::encode(buf)
}

fn lifecycle_label(status: LifecycleStatus) -> &'static str {
    match status {
        LifecycleStatus::Invited => "invited",
        LifecycleStatus::Pending => "pending",
        LifecycleStatus::Active => "active",
        LifecycleStatus::Disabled => "disabled",
        LifecycleStatus::Removed => "removed",
        LifecycleStatus::Rejected => "rejected",
        LifecycleStatus::Revoked => "revoked",
        LifecycleStatus::Expired => "expired",
        LifecycleStatus::Accepted => "accepted",
        LifecycleStatus::Rotated => "rotated",
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::permissions::permissions_for_role;
    use crate::testutil::MemoryStore;

    fn owner_actor(principal_id: &str, token_id: &str, tenant_id: &str) -> TenantContext {
        TenantContext {
            principal_id: principal_id.to_string(),
            token_id: token_id.to_string(),
            tenant_id: tenant_id.to_string(),
            role: Some(Role::Owner),
            permissions: permissions_for_role(Role::Owner, LifecycleStatus::Active),
            ..TenantContext::default()
        }
    }

    #[test]
    fn bootstrap_local_creates_personal_tenant_and_token_grant() {
        let store = Arc::new(MemoryStore::new());
        let manager = Manager::new(store.clone());

        let (principal, tenant) = manager
            .bootstrap_local(&["tok_1".to_string()])
            .expect("bootstrap");
        assert_eq!(principal.status, LifecycleStatus::Active);
        assert_eq!(tenant.tenant_kind, TenantKind::Personal);

        let grants = store
            .list_token_tenant_grants("tok_1")
            .expect("list grants");
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].tenant_id, tenant.tenant_id);
        assert!(grants[0].is_default);

        // Second bootstrap reuses the existing principal/tenant.
        let (again_principal, again_tenant) = manager
            .bootstrap_local(&["tok_1".to_string()])
            .expect("re-bootstrap");
        assert_eq!(again_principal.principal_id, principal.principal_id);
        assert_eq!(again_tenant.tenant_id, tenant.tenant_id);
        assert_eq!(
            store
                .list_token_tenant_grants("tok_1")
                .expect("grants")
                .len(),
            1
        );
    }

    #[test]
    fn organization_membership_lifecycle() {
        let store = Arc::new(MemoryStore::new());
        let manager = Manager::new(store.clone());
        let (principal, tenant) = manager
            .bootstrap_local(&["tok_owner".to_string()])
            .expect("bootstrap");
        let actor = owner_actor(&principal.principal_id, "tok_owner", &tenant.tenant_id);
        store.insert_principal("prn_invited", LifecycleStatus::Active, &tenant.tenant_id);

        let (org, owner_membership) = manager
            .create_organization_tenant(&actor, "Acme")
            .expect("create org");
        assert_eq!(org.tenant_kind, TenantKind::Organization);
        assert_eq!(owner_membership.role, Role::Owner);
        assert_eq!(owner_membership.status, LifecycleStatus::Active);

        let org_actor = owner_actor(&principal.principal_id, "tok_owner", &org.tenant_id);
        let invitation = manager
            .create_invitation(
                &org_actor,
                CreateInvitationInput {
                    tenant_id: org.tenant_id.clone(),
                    invited_principal_id: "prn_invited".to_string(),
                    role: Some(Role::Admin),
                    expires_at: None,
                },
            )
            .expect("create invitation");
        let membership = manager
            .accept_invitation("prn_invited", &invitation.invitation_id)
            .expect("accept invitation");
        assert_eq!(membership.role, Role::Admin);
        assert_eq!(membership.status, LifecycleStatus::Active);

        let updated = manager
            .update_membership_role(
                &org_actor,
                &org.tenant_id,
                &membership.membership_id,
                Role::Viewer,
            )
            .expect("update role");
        assert_eq!(updated.role, Role::Viewer);

        let removed = manager
            .remove_membership(&org_actor, &org.tenant_id, &membership.membership_id)
            .expect("remove membership");
        assert_eq!(removed.status, LifecycleStatus::Removed);

        assert!(matches!(
            manager.remove_membership(&org_actor, &org.tenant_id, &owner_membership.membership_id),
            Err(IdentityError::OwnerInvariant)
        ));
    }

    #[test]
    fn fails_closed_when_membership_audit_fails() {
        let store = Arc::new(MemoryStore::new());
        let manager = Manager::new(store.clone());
        let (principal, tenant) = manager
            .bootstrap_local(&["tok_owner".to_string()])
            .expect("bootstrap");
        store.fail_audits();
        let actor = owner_actor(&principal.principal_id, "tok_owner", &tenant.tenant_id);
        assert!(matches!(
            manager.create_organization_tenant(&actor, "Denied Org"),
            Err(IdentityError::AuditWriteFailed)
        ));
        // Fail-closed: no tenant persisted when the audit write fails.
        assert!(store.tenants.lock().len() == 1);
    }

    #[test]
    fn permission_denied_actor_cannot_manage_tenants() {
        let store = Arc::new(MemoryStore::new());
        let manager = Manager::new(store.clone());
        let (principal, tenant) = manager
            .bootstrap_local(&["tok_1".to_string()])
            .expect("bootstrap");
        let viewer_actor = TenantContext {
            principal_id: principal.principal_id.clone(),
            token_id: "tok_1".to_string(),
            tenant_id: tenant.tenant_id.clone(),
            role: Some(Role::Viewer),
            permissions: permissions_for_role(Role::Viewer, LifecycleStatus::Active),
            ..TenantContext::default()
        };
        assert!(matches!(
            manager.create_organization_tenant(&viewer_actor, "Acme"),
            Err(IdentityError::PermissionDenied)
        ));
    }

    #[test]
    fn replace_token_grants_without_widening() {
        let store = Arc::new(MemoryStore::new());
        let manager = Manager::new(store.clone());
        let (principal, personal) = manager
            .bootstrap_local(&["tok_1".to_string()])
            .expect("bootstrap");
        let actor = owner_actor(&principal.principal_id, "tok_1", &personal.tenant_id);
        let (org, _) = manager
            .create_organization_tenant(&actor, "Acme")
            .expect("create org");

        let grants = manager
            .replace_token_tenant_grants(&actor, "tok_1", &[org.tenant_id.clone()], &org.tenant_id)
            .expect("replace grants");
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].tenant_id, org.tenant_id);
        assert!(grants[0].is_default);

        let old_grants = store
            .list_token_tenant_grants("tok_1")
            .expect("list grants");
        for grant in &old_grants {
            assert!(
                !(grant.tenant_id == personal.tenant_id && grant.status == LifecycleStatus::Active),
                "expected old personal grant to be revoked: {old_grants:?}"
            );
        }

        assert!(matches!(
            manager.replace_token_tenant_grants(
                &actor,
                "tok_1",
                &["ten_missing".to_string()],
                "ten_missing",
            ),
            Err(IdentityError::TokenGrantInvalid)
        ));
    }

    #[test]
    fn invitation_decisions_and_expiry() {
        let store = Arc::new(MemoryStore::new());
        let manager = Manager::new(store.clone());
        let (principal, tenant) = manager
            .bootstrap_local(&["tok_1".to_string()])
            .expect("bootstrap");
        let actor = owner_actor(&principal.principal_id, "tok_1", &tenant.tenant_id);
        store.insert_principal("prn_invited", LifecycleStatus::Active, &tenant.tenant_id);

        let invitation = manager
            .create_invitation(
                &actor,
                CreateInvitationInput {
                    tenant_id: tenant.tenant_id.clone(),
                    invited_principal_id: "prn_invited".to_string(),
                    role: Some(Role::Viewer),
                    expires_at: None,
                },
            )
            .expect("create invitation");

        // Only the invited principal may reject.
        assert!(matches!(
            manager.decide_invitation(
                "prn_other",
                &invitation.invitation_id,
                LifecycleStatus::Rejected
            ),
            Err(IdentityError::InvitationInvalid)
        ));
        // Accepted is not a valid decision here.
        assert!(matches!(
            manager.decide_invitation(
                "prn_invited",
                &invitation.invitation_id,
                LifecycleStatus::Accepted
            ),
            Err(IdentityError::InvitationInvalid)
        ));
        let rejected = manager
            .decide_invitation(
                "prn_invited",
                &invitation.invitation_id,
                LifecycleStatus::Rejected,
            )
            .expect("reject invitation");
        assert_eq!(rejected.status, LifecycleStatus::Rejected);
        assert!(rejected.decided_at.is_some());
        // A decided invitation can no longer be accepted.
        assert!(matches!(
            manager.accept_invitation("prn_invited", &invitation.invitation_id),
            Err(IdentityError::InvitationInvalid)
        ));

        // Expired invitations cannot be accepted.
        let expired = manager
            .create_invitation(
                &actor,
                CreateInvitationInput {
                    tenant_id: tenant.tenant_id.clone(),
                    invited_principal_id: "prn_invited".to_string(),
                    role: Some(Role::Viewer),
                    expires_at: Some(Utc::now() - chrono::Duration::hours(1)),
                },
            )
            .expect("create expired invitation");
        assert!(matches!(
            manager.accept_invitation("prn_invited", &expired.invitation_id),
            Err(IdentityError::InvitationInvalid)
        ));
    }

    #[test]
    fn cross_tenant_invitation_is_denied() {
        let store = Arc::new(MemoryStore::new());
        let manager = Manager::new(store.clone());
        let (principal, tenant) = manager
            .bootstrap_local(&["tok_1".to_string()])
            .expect("bootstrap");
        let actor = owner_actor(&principal.principal_id, "tok_1", &tenant.tenant_id);
        store.insert_principal("prn_invited", LifecycleStatus::Active, &tenant.tenant_id);
        assert!(matches!(
            manager.create_invitation(
                &actor,
                CreateInvitationInput {
                    tenant_id: "ten_other".to_string(),
                    invited_principal_id: "prn_invited".to_string(),
                    role: Some(Role::Viewer),
                    expires_at: None,
                },
            ),
            Err(IdentityError::TenantAccessDenied)
        ));
    }
}
