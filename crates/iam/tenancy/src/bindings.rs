//! BindingAccessScope guards tenant-scoped binding and workspace access by permission.
//! Port of daemon/internal/store/tenancy/bindings.go.

use kura_identity::{Permission, has_permission};

/// Tenant-scoped permission scope for binding rules and workspaces.
#[derive(Debug, Clone, Default)]
pub struct BindingAccessScope {
    pub tenant_id: String,
    pub permissions: Vec<Permission>,
}

impl BindingAccessScope {
    fn allows(&self, tenant_id: &str, permission: Permission) -> bool {
        !self.tenant_id.is_empty()
            && self.tenant_id == tenant_id
            && has_permission(&self.permissions, permission)
    }

    /// Whether the scope may read the workspace.
    #[must_use]
    pub fn can_inspect_workspace(&self, ws: &kura_bindings::Workspace) -> bool {
        self.allows(&ws.tenant_id, Permission::BindingsInspect)
    }

    /// Whether the scope may mutate the workspace.
    #[must_use]
    pub fn can_manage_workspace(&self, ws: &kura_bindings::Workspace) -> bool {
        self.allows(&ws.tenant_id, Permission::BindingsManage)
    }

    /// Whether the scope may read the binding rule.
    #[must_use]
    pub fn can_inspect_binding(&self, rule: &kura_bindings::BindingRule) -> bool {
        self.allows(&rule.tenant_id, Permission::BindingsInspect)
    }

    /// Whether the scope may mutate the binding rule.
    #[must_use]
    pub fn can_manage_binding(&self, rule: &kura_bindings::BindingRule) -> bool {
        self.allows(&rule.tenant_id, Permission::BindingsManage)
    }

    /// Whether the scope may inspect binding state for a tenant id (used before any
    /// resource is loaded, so denial never reveals existence — FR-007).
    #[must_use]
    pub fn can_inspect_tenant(&self, tenant_id: &str) -> bool {
        self.allows(tenant_id, Permission::BindingsInspect)
    }

    /// Whether the scope may manage binding state for a tenant id.
    #[must_use]
    pub fn can_manage_tenant(&self, tenant_id: &str) -> bool {
        self.allows(tenant_id, Permission::BindingsManage)
    }
}
