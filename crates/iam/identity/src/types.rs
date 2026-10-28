//! Core domain types for the tenant/permission model.
//!
//! Port of `daemon/internal/identity/types.go`. JSON field names match the Go
//! tags (camelCase) so persisted and wire representations stay compatible.

use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;

pub(crate) fn epoch() -> DateTime<Utc> {
    DateTime::<Utc>::UNIX_EPOCH
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TenantKind {
    Personal,
    Organization,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleStatus {
    Invited,
    Pending,
    Active,
    Disabled,
    Removed,
    Rejected,
    Revoked,
    Expired,
    Accepted,
    Rotated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalKind {
    LocalOperator,
    User,
    ServiceClient,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Owner,
    Admin,
    Operator,
    Viewer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Permission {
    #[serde(rename = "tenant.manage")]
    TenantManage,
    #[serde(rename = "secrets.manage")]
    SecretsManage,
    #[serde(rename = "credentials.inspect")]
    CredentialsInspect,
    #[serde(rename = "integrations.manage")]
    IntegrationsManage,
    #[serde(rename = "integrations.diagnostics.read")]
    IntegrationDiagnosticsRead,
    #[serde(rename = "integrations.diagnostics.run")]
    IntegrationDiagnosticsRun,
    #[serde(rename = "integrations.diagnostics.smoke")]
    IntegrationDiagnosticsSmoke,
    #[serde(rename = "integrations.diagnostics.smoke_risky")]
    IntegrationDiagnosticsSmokeRisky,
    #[serde(rename = "connectors.manage")]
    ConnectorsManage,
    #[serde(rename = "mcp.manage")]
    McpManage,
    #[serde(rename = "runs.execute")]
    RunsExecute,
    #[serde(rename = "approvals.resolve")]
    ApprovalsResolve,
    #[serde(rename = "live_validation.execute")]
    LiveValidationExecute,
    #[serde(rename = "live_validation.reconcile")]
    LiveValidationReconcile,
    #[serde(rename = "evaluation.manage")]
    EvaluationManage,
    #[serde(rename = "evaluation.discovery.read")]
    EvaluationDiscoveryRead,
    #[serde(rename = "evaluation.discovery.run")]
    EvaluationDiscoveryRun,
    #[serde(rename = "evaluation.discovery.suppress")]
    EvaluationDiscoverySuppress,
    #[serde(rename = "evaluation.fixture.read")]
    EvaluationFixtureRead,
    #[serde(rename = "evaluation.fixture.manage")]
    EvaluationFixtureManage,
    #[serde(rename = "evaluation.fixture.review")]
    EvaluationFixtureReview,
    #[serde(rename = "evaluation.fixture.suppress")]
    EvaluationFixtureSuppress,
    #[serde(rename = "evaluation.campaign.read")]
    EvaluationCampaignRead,
    #[serde(rename = "evaluation.campaign.manage")]
    EvaluationCampaignManage,
    #[serde(rename = "evaluation.dashboard.read")]
    EvaluationDashboardRead,
    #[serde(rename = "evaluation.inspection.read")]
    EvaluationInspectionRead,
    #[serde(rename = "evaluation.retention.manage")]
    EvaluationRetentionManage,
    #[serde(rename = "billing.view")]
    BillingView,
    #[serde(rename = "billing.manage")]
    BillingManage,
    #[serde(rename = "billing.evidence_export")]
    BillingEvidenceExport,
    #[serde(rename = "profiles.inspect")]
    ProfilesInspect,
    #[serde(rename = "profiles.manage")]
    ProfilesManage,
    #[serde(rename = "bindings.inspect")]
    BindingsInspect,
    #[serde(rename = "bindings.manage")]
    BindingsManage,
    #[serde(rename = "read_only.inspect")]
    ReadOnlyInspect,
}

/// Every permission that requires step-up / sensitive treatment. `Role::Owner`
/// is granted exactly this set.
pub const ALL_SENSITIVE_PERMISSIONS: &[Permission] = &[
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
    Permission::RunsExecute,
    Permission::ApprovalsResolve,
    Permission::LiveValidationExecute,
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
    Permission::BillingEvidenceExport,
    Permission::ProfilesInspect,
    Permission::ProfilesManage,
    Permission::BindingsInspect,
    Permission::BindingsManage,
];

#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("tenant access denied")]
    TenantAccessDenied,
    #[error("tenant permission denied")]
    PermissionDenied,
    #[error("tenant audit write failed")]
    AuditWriteFailed,
    #[error("organization tenant requires at least one active owner")]
    OwnerInvariant,
    #[error("tenant invitation is invalid")]
    InvitationInvalid,
    #[error("principal is invalid")]
    PrincipalInvalid,
    #[error("token tenant grant is invalid")]
    TokenGrantInvalid,
    #[error("tenant is invalid")]
    TenantInvalid,
    #[error("membership is invalid")]
    MembershipInvalid,
    #[error("tenant role is invalid")]
    UnsupportedRole,
    #[error("tenant lifecycle status is invalid")]
    UnsupportedStatus,
    #[error("tenant kind is invalid")]
    UnsupportedTenant,
    #[error("tenant context required")]
    TenantContextRequired,
    #[error("external subject must be 1-256 characters")]
    ProvisioningSubjectInvalid,
    #[error("the provisioned identity is not active")]
    ProvisioningIdentityInactive,
    #[error("identity store error: {0}")]
    Store(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// Stable denial payload surfaced to clients; deliberately free of internals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Denial {
    pub error: String,
    pub error_code: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub request_id: String,
}

pub fn stable_denial() -> Denial {
    Denial {
        error: "tenant access denied".to_string(),
        error_code: "tenant_access_denied".to_string(),
        request_id: String::new(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tenant {
    pub tenant_id: String,
    pub tenant_kind: TenantKind,
    pub display_name: String,
    pub status: LifecycleStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub created_by_principal_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub default_owner_principal_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_membership_role: Option<Role>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_membership_status: Option<LifecycleStatus>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub caller_permissions: Vec<Permission>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub default_for_current_token: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub default_for_current_principal: bool,
}

fn is_false(v: &bool) -> bool {
    !*v
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Principal {
    pub principal_id: String,
    pub principal_kind: PrincipalKind,
    pub display_name: String,
    pub status: LifecycleStatus,
    pub default_tenant_id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Membership {
    pub membership_id: String,
    pub tenant_id: String,
    pub principal_id: String,
    pub role: Role,
    pub status: LifecycleStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub invitation_id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TenantInvitation {
    pub invitation_id: String,
    pub tenant_id: String,
    pub invited_principal_id: String,
    pub invited_by_principal_id: String,
    pub role: Role,
    pub status: LifecycleStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenTenantGrant {
    pub grant_id: String,
    pub token_id: String,
    pub tenant_id: String,
    pub is_default: bool,
    pub status: LifecycleStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub granted_by_principal_id: String,
}

/// Resolved tenant scope for one authenticated call.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TenantContext {
    pub principal_id: String,
    pub token_id: String,
    pub tenant_id: String,
    pub tenant_source: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub membership_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    pub permissions: Vec<Permission>,
    #[serde(default = "epoch")]
    pub resolved_at: DateTime<Utc>,
}

impl Default for TenantContext {
    fn default() -> Self {
        Self {
            principal_id: String::new(),
            token_id: String::new(),
            tenant_id: String::new(),
            tenant_source: String::new(),
            membership_id: String::new(),
            role: None,
            permissions: Vec::new(),
            resolved_at: epoch(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionEvaluation {
    pub permission: Permission,
    pub allowed: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TenantAuditEvent {
    pub audit_event_id: String,
    pub event_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub principal_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub target_principal_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub token_id: String,
    pub outcome: String,
    pub reason_code: String,
    #[serde(default = "epoch")]
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document: Option<serde_json::Map<String, serde_json::Value>>,
}

impl Default for TenantAuditEvent {
    fn default() -> Self {
        Self {
            audit_event_id: String::new(),
            event_kind: String::new(),
            tenant_id: String::new(),
            principal_id: String::new(),
            target_principal_id: String::new(),
            token_id: String::new(),
            outcome: String::new(),
            reason_code: String::new(),
            created_at: epoch(),
            document: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct TenantFilter {
    pub tenant_kind: Option<TenantKind>,
    pub status: Option<LifecycleStatus>,
    pub limit: usize,
}

#[derive(Debug, Clone, Default)]
pub struct PrincipalFilter {
    pub tenant_id: String,
    pub status: Option<LifecycleStatus>,
    pub limit: usize,
}

#[derive(Debug, Clone, Default)]
pub struct MembershipFilter {
    pub tenant_id: String,
    pub status: Option<LifecycleStatus>,
    pub role: Option<Role>,
    pub limit: usize,
}

#[derive(Debug, Clone, Default)]
pub struct InvitationFilter {
    pub tenant_id: String,
    pub principal_id: String,
    pub status: Option<LifecycleStatus>,
    pub limit: usize,
}

#[derive(Debug, Clone, Default)]
pub struct AuditEventFilter {
    pub tenant_id: String,
    pub principal_id: String,
    pub token_id: String,
    pub event_kind: String,
    pub outcome: String,
    pub limit: usize,
}
