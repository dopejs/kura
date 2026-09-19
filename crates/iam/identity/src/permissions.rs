//! Role → permission grants and evaluation helpers.
//!
//! Port of `daemon/internal/identity/permissions.go`.

use crate::types::ALL_SENSITIVE_PERMISSIONS;
use crate::types::IdentityError;
use crate::types::LifecycleStatus;
use crate::types::Permission;
use crate::types::PermissionEvaluation;
use crate::types::Role;
use crate::types::TenantContext;

/// Tiered least-privilege grant set for a role. Any lifecycle other than
/// [`LifecycleStatus::Active`] yields no permissions at all (fail closed).
pub fn permissions_for_role(role: Role, lifecycle: LifecycleStatus) -> Vec<Permission> {
    if lifecycle != LifecycleStatus::Active {
        return Vec::new();
    }
    match role {
        Role::Owner => ALL_SENSITIVE_PERMISSIONS.to_vec(),
        Role::Admin => vec![
            Permission::TenantManage,
            Permission::SecretsManage,
            Permission::CredentialsInspect,
            Permission::IntegrationsManage,
            Permission::IntegrationDiagnosticsRead,
            Permission::IntegrationDiagnosticsRun,
            Permission::IntegrationDiagnosticsSmoke,
            Permission::IntegrationDiagnosticsSmokeRisky,
            Permission::ConnectorsManage,
            Permission::McpManage,
            Permission::LiveValidationReconcile,
            Permission::EvaluationManage,
            Permission::EvaluationDiscoveryRead,
            Permission::EvaluationDiscoveryRun,
            Permission::EvaluationDiscoverySuppress,
            Permission::EvaluationFixtureRead,
            Permission::EvaluationFixtureManage,
            Permission::EvaluationFixtureReview,
            Permission::EvaluationFixtureSuppress,
            Permission::EvaluationCampaignRead,
            Permission::EvaluationCampaignManage,
            Permission::EvaluationDashboardRead,
            Permission::EvaluationInspectionRead,
            Permission::EvaluationRetentionManage,
            Permission::BillingView,
            Permission::BillingManage,
            Permission::ProfilesInspect,
            Permission::ProfilesManage,
            Permission::BindingsInspect,
            Permission::BindingsManage,
        ],
        Role::Operator => vec![
            Permission::RunsExecute,
            Permission::ApprovalsResolve,
            Permission::LiveValidationExecute,
            Permission::IntegrationDiagnosticsRead,
            Permission::IntegrationDiagnosticsRun,
            Permission::IntegrationDiagnosticsSmoke,
            Permission::EvaluationDiscoveryRead,
            Permission::EvaluationDiscoveryRun,
            Permission::EvaluationDiscoverySuppress,
            Permission::EvaluationFixtureRead,
            Permission::EvaluationFixtureSuppress,
            Permission::EvaluationCampaignRead,
            Permission::EvaluationCampaignManage,
            Permission::EvaluationDashboardRead,
            Permission::EvaluationInspectionRead,
        ],
        Role::Viewer => vec![
            Permission::ReadOnlyInspect,
            Permission::EvaluationCampaignRead,
            Permission::EvaluationDashboardRead,
            Permission::EvaluationInspectionRead,
        ],
    }
}

pub fn has_permission(perms: &[Permission], permission: Permission) -> bool {
    perms.contains(&permission)
}

pub fn can(role: Role, lifecycle: LifecycleStatus, permission: Permission) -> bool {
    has_permission(&permissions_for_role(role, lifecycle), permission)
}

/// Credentials may be inspected either via the explicit inspect permission or
/// via any of the caller-supplied manage permissions.
pub fn can_inspect_credentials(
    tenant_context: &TenantContext,
    manage_permissions: &[Permission],
) -> bool {
    if tenant_context.principal_id.is_empty() || tenant_context.tenant_id.is_empty() {
        return false;
    }
    if has_permission(&tenant_context.permissions, Permission::CredentialsInspect) {
        return true;
    }
    manage_permissions
        .iter()
        .any(|permission| has_permission(&tenant_context.permissions, *permission))
}

pub fn can_resolve_live_validation_reconciliation(tenant_context: &TenantContext) -> bool {
    if tenant_context.principal_id.is_empty() || tenant_context.tenant_id.is_empty() {
        return false;
    }
    if matches!(tenant_context.role, Some(Role::Owner) | Some(Role::Admin)) {
        return true;
    }
    has_permission(
        &tenant_context.permissions,
        Permission::LiveValidationReconcile,
    )
}

pub fn evaluate_permission(
    tenant_context: &TenantContext,
    permission: Permission,
) -> PermissionEvaluation {
    let mut evaluation = PermissionEvaluation {
        permission,
        allowed: false,
        reason_code: "permission_missing".to_string(),
    };
    if tenant_context.principal_id.is_empty() || tenant_context.tenant_id.is_empty() {
        evaluation.reason_code = "tenant_context_missing".to_string();
        return evaluation;
    }
    if !has_permission(&tenant_context.permissions, permission) {
        return evaluation;
    }
    evaluation.allowed = true;
    evaluation.reason_code = String::new();
    evaluation
}

pub fn require_permission(
    tenant_context: &TenantContext,
    permission: Permission,
) -> Result<(), IdentityError> {
    if !evaluate_permission(tenant_context, permission).allowed {
        return Err(IdentityError::PermissionDenied);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(
        principal: &str,
        tenant: &str,
        role: Option<Role>,
        permissions: Vec<Permission>,
    ) -> TenantContext {
        TenantContext {
            principal_id: principal.to_string(),
            tenant_id: tenant.to_string(),
            role,
            permissions,
            ..TenantContext::default()
        }
    }

    #[test]
    fn permissions_for_role_uses_tiered_least_privilege() {
        let cases: [(Role, Vec<Permission>); 4] = [
            (Role::Owner, ALL_SENSITIVE_PERMISSIONS.to_vec()),
            (
                Role::Admin,
                vec![
                    Permission::TenantManage,
                    Permission::SecretsManage,
                    Permission::CredentialsInspect,
                    Permission::IntegrationsManage,
                    Permission::IntegrationDiagnosticsRead,
                    Permission::IntegrationDiagnosticsRun,
                    Permission::IntegrationDiagnosticsSmoke,
                    Permission::IntegrationDiagnosticsSmokeRisky,
                    Permission::ConnectorsManage,
                    Permission::McpManage,
                    Permission::LiveValidationReconcile,
                    Permission::EvaluationManage,
                    Permission::EvaluationDiscoveryRead,
                    Permission::EvaluationDiscoveryRun,
                    Permission::EvaluationDiscoverySuppress,
                    Permission::EvaluationFixtureRead,
                    Permission::EvaluationFixtureManage,
                    Permission::EvaluationFixtureReview,
                    Permission::EvaluationFixtureSuppress,
                    Permission::EvaluationCampaignRead,
                    Permission::EvaluationCampaignManage,
                    Permission::EvaluationDashboardRead,
                    Permission::EvaluationInspectionRead,
                    Permission::EvaluationRetentionManage,
                    Permission::BillingView,
                    Permission::BillingManage,
                    Permission::ProfilesInspect,
                    Permission::ProfilesManage,
                    Permission::BindingsInspect,
                    Permission::BindingsManage,
                ],
            ),
            (
                Role::Operator,
                vec![
                    Permission::RunsExecute,
                    Permission::ApprovalsResolve,
                    Permission::LiveValidationExecute,
                    Permission::IntegrationDiagnosticsRead,
                    Permission::IntegrationDiagnosticsRun,
                    Permission::IntegrationDiagnosticsSmoke,
                    Permission::EvaluationDiscoveryRead,
                    Permission::EvaluationDiscoveryRun,
                    Permission::EvaluationDiscoverySuppress,
                    Permission::EvaluationFixtureRead,
                    Permission::EvaluationFixtureSuppress,
                    Permission::EvaluationCampaignRead,
                    Permission::EvaluationCampaignManage,
                    Permission::EvaluationDashboardRead,
                    Permission::EvaluationInspectionRead,
                ],
            ),
            (
                Role::Viewer,
                vec![
                    Permission::ReadOnlyInspect,
                    Permission::EvaluationCampaignRead,
                    Permission::EvaluationDashboardRead,
                    Permission::EvaluationInspectionRead,
                ],
            ),
        ];
        for (role, want) in cases {
            let got = permissions_for_role(role, LifecycleStatus::Active);
            assert_eq!(got.len(), want.len(), "role {role:?}: {got:?}");
            for permission in &want {
                assert!(
                    has_permission(&got, *permission),
                    "role {role:?} missing {permission:?}"
                );
            }
        }
    }

    #[test]
    fn permissions_denied_for_inactive_lifecycle() {
        for status in [
            LifecycleStatus::Invited,
            LifecycleStatus::Disabled,
            LifecycleStatus::Removed,
            LifecycleStatus::Revoked,
            LifecycleStatus::Expired,
            LifecycleStatus::Rotated,
        ] {
            assert!(
                permissions_for_role(Role::Owner, status).is_empty(),
                "expected no permissions for status {status:?}"
            );
        }
    }

    #[test]
    fn permission_evaluator_covers_sensitive_capabilities() {
        let role_permissions: [(Role, Vec<Permission>); 4] = [
            (Role::Owner, ALL_SENSITIVE_PERMISSIONS.to_vec()),
            (
                Role::Admin,
                permissions_for_role(Role::Admin, LifecycleStatus::Active),
            ),
            (
                Role::Operator,
                permissions_for_role(Role::Operator, LifecycleStatus::Active),
            ),
            (
                Role::Viewer,
                permissions_for_role(Role::Viewer, LifecycleStatus::Active),
            ),
        ];
        let all: Vec<Permission> = ALL_SENSITIVE_PERMISSIONS
            .iter()
            .copied()
            .chain(std::iter::once(Permission::ReadOnlyInspect))
            .collect();
        for (role, allowed) in role_permissions {
            for permission in &all {
                let context = TenantContext {
                    principal_id: "prn_1".to_string(),
                    tenant_id: "ten_1".to_string(),
                    role: Some(role),
                    permissions: permissions_for_role(role, LifecycleStatus::Active),
                    ..TenantContext::default()
                };
                let evaluation = evaluate_permission(&context, *permission);
                assert_eq!(
                    evaluation.allowed,
                    has_permission(&allowed, *permission),
                    "role {role:?} permission {permission:?}"
                );
            }
        }
    }

    #[test]
    fn live_validation_reconciliation_requires_owner_admin_or_permission() {
        let cases = [
            (
                "owner role",
                ctx(
                    "prn_owner",
                    "ten_1",
                    Some(Role::Owner),
                    permissions_for_role(Role::Owner, LifecycleStatus::Active),
                ),
                true,
            ),
            (
                "admin role",
                ctx(
                    "prn_admin",
                    "ten_1",
                    Some(Role::Admin),
                    permissions_for_role(Role::Admin, LifecycleStatus::Active),
                ),
                true,
            ),
            (
                "explicit permission",
                ctx(
                    "prn_reconciler",
                    "ten_1",
                    Some(Role::Operator),
                    vec![Permission::LiveValidationReconcile],
                ),
                true,
            ),
            (
                "operator execute only",
                ctx(
                    "prn_operator",
                    "ten_1",
                    Some(Role::Operator),
                    permissions_for_role(Role::Operator, LifecycleStatus::Active),
                ),
                false,
            ),
            (
                "missing tenant context",
                ctx(
                    "prn_owner",
                    "",
                    Some(Role::Owner),
                    permissions_for_role(Role::Owner, LifecycleStatus::Active),
                ),
                false,
            ),
        ];
        for (name, context, want) in cases {
            assert_eq!(
                can_resolve_live_validation_reconciliation(&context),
                want,
                "{name}"
            );
        }
    }

    #[test]
    fn evaluation_product_permissions_for_roles() {
        let read_only_product = [
            Permission::EvaluationCampaignRead,
            Permission::EvaluationDashboardRead,
            Permission::EvaluationInspectionRead,
        ];
        let admin_product = [
            Permission::EvaluationDiscoveryRead,
            Permission::EvaluationDiscoveryRun,
            Permission::EvaluationDiscoverySuppress,
            Permission::EvaluationFixtureRead,
            Permission::EvaluationFixtureManage,
            Permission::EvaluationFixtureReview,
            Permission::EvaluationFixtureSuppress,
            Permission::EvaluationCampaignRead,
            Permission::EvaluationCampaignManage,
            Permission::EvaluationDashboardRead,
            Permission::EvaluationInspectionRead,
            Permission::EvaluationRetentionManage,
        ];
        let operator_product = [
            Permission::EvaluationDiscoveryRead,
            Permission::EvaluationDiscoveryRun,
            Permission::EvaluationDiscoverySuppress,
            Permission::EvaluationFixtureRead,
            Permission::EvaluationFixtureSuppress,
            Permission::EvaluationCampaignRead,
            Permission::EvaluationCampaignManage,
            Permission::EvaluationDashboardRead,
            Permission::EvaluationInspectionRead,
        ];

        for permission in admin_product {
            assert!(can(Role::Owner, LifecycleStatus::Active, permission));
            assert!(can(Role::Admin, LifecycleStatus::Active, permission));
        }
        for permission in operator_product {
            assert!(can(Role::Operator, LifecycleStatus::Active, permission));
        }
        for permission in read_only_product {
            assert!(can(Role::Viewer, LifecycleStatus::Active, permission));
        }
        for permission in [
            Permission::EvaluationFixtureManage,
            Permission::EvaluationFixtureReview,
            Permission::EvaluationRetentionManage,
        ] {
            assert!(!can(Role::Operator, LifecycleStatus::Active, permission));
        }
        assert!(!can(
            Role::Viewer,
            LifecycleStatus::Active,
            Permission::EvaluationCampaignManage
        ));
        assert!(!can(
            Role::Viewer,
            LifecycleStatus::Active,
            Permission::EvaluationDiscoveryRun
        ));
    }

    #[test]
    fn billing_evidence_export_is_canonical_and_separate_from_billing_view() {
        assert!(can(
            Role::Owner,
            LifecycleStatus::Active,
            Permission::BillingEvidenceExport
        ));
        assert!(!can(
            Role::Admin,
            LifecycleStatus::Active,
            Permission::BillingEvidenceExport
        ));

        let view_only = TenantContext {
            principal_id: "prn_view".to_string(),
            tenant_id: "ten_1".to_string(),
            role: Some(Role::Admin),
            permissions: vec![Permission::BillingView],
            ..TenantContext::default()
        };
        assert!(!evaluate_permission(&view_only, Permission::BillingEvidenceExport).allowed);

        let mut explicit = view_only.clone();
        explicit.permissions.push(Permission::BillingEvidenceExport);
        assert!(evaluate_permission(&explicit, Permission::BillingEvidenceExport).allowed);
    }

    #[test]
    fn binding_permissions_granted_and_isolated() {
        for permission in [Permission::BindingsInspect, Permission::BindingsManage] {
            assert!(ALL_SENSITIVE_PERMISSIONS.contains(&permission));
            assert!(can(Role::Owner, LifecycleStatus::Active, permission));
            assert!(can(Role::Admin, LifecycleStatus::Active, permission));
            assert!(!can(Role::Operator, LifecycleStatus::Active, permission));
            assert!(!can(Role::Viewer, LifecycleStatus::Active, permission));
            assert!(!can(Role::Owner, LifecycleStatus::Disabled, permission));
        }
    }

    #[test]
    fn evaluate_permission_reason_codes() {
        let missing_context = TenantContext {
            principal_id: "prn_1".to_string(),
            ..TenantContext::default()
        };
        let evaluation = evaluate_permission(&missing_context, Permission::TenantManage);
        assert!(!evaluation.allowed);
        assert_eq!(evaluation.reason_code, "tenant_context_missing");

        let context = ctx("prn_1", "ten_1", Some(Role::Viewer), vec![]);
        let evaluation = evaluate_permission(&context, Permission::TenantManage);
        assert!(!evaluation.allowed);
        assert_eq!(evaluation.reason_code, "permission_missing");
        assert!(matches!(
            require_permission(&context, Permission::TenantManage),
            Err(IdentityError::PermissionDenied)
        ));
    }
}
