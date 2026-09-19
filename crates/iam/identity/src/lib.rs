//! Port of `daemon/internal/identity`, with `daemon/internal/tenantctx` and
//! `daemon/internal/auth` folded in as modules. See rs/MIGRATION.md for
//! conventions.
//!
//! - Top level: the tenant/principal/membership/invitation domain model, the
//!   role→permission grant matrix, the fail-closed tenant context [`Resolver`],
//!   the [`Auditor`], and the [`Manager`] driving all tenant mutations.
//! - [`tenantctx`]: task-local carrier for the resolved [`TenantContext`].
//! - [`auth`]: pairing and access-token lifecycle (in-memory, restorable).

mod audit;
pub mod auth;
mod manager;
mod permissions;
mod resolver;
pub mod tenantctx;
mod types;

pub use audit::AUDIT_OUTCOME_DENIED;
pub use audit::AUDIT_OUTCOME_FAILED_CLOSED;
pub use audit::AUDIT_OUTCOME_SUCCEEDED;
pub use audit::AuditStore;
pub use audit::Auditor;
pub use manager::CreateInvitationInput;
pub use manager::Manager;
pub use manager::Store;
pub use permissions::can;
pub use permissions::can_inspect_credentials;
pub use permissions::can_resolve_live_validation_reconciliation;
pub use permissions::evaluate_permission;
pub use permissions::has_permission;
pub use permissions::permissions_for_role;
pub use permissions::require_permission;
pub use resolver::Resolver;
pub use resolver::ResolverStore;
pub use resolver::TENANT_SOURCE_DEFAULT;
pub use resolver::TENANT_SOURCE_EXPLICIT_HEADER;
pub use resolver::TokenAuthority;
pub use types::ALL_SENSITIVE_PERMISSIONS;
pub use types::AuditEventFilter;
pub use types::Denial;
pub use types::IdentityError;
pub use types::InvitationFilter;
pub use types::LifecycleStatus;
pub use types::Membership;
pub use types::MembershipFilter;
pub use types::Permission;
pub use types::PermissionEvaluation;
pub use types::Principal;
pub use types::PrincipalFilter;
pub use types::PrincipalKind;
pub use types::Role;
pub use types::Tenant;
pub use types::TenantAuditEvent;
pub use types::TenantContext;
pub use types::TenantFilter;
pub use types::TenantInvitation;
pub use types::TenantKind;
pub use types::TokenTenantGrant;
pub use types::stable_denial;

#[cfg(test)]
mod testutil;
