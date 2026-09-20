//! Tenant-aware accessor for the workflows family (workflows, workflow_steps,
//! workflow_dependencies, workflow_handoffs). Port of
//! daemon/internal/store/tenancy/workflows.go.
//!
//! Pass A: workflow_steps / workflow_dependencies / workflow_handoffs are CRUD'd via the
//! orchestration crate against the parent workflow row; the binding here covers the
//! parent workflow and inherits to children at backfill time per the parent-child orphan
//! rule.

use crate::{TenancyError, emit_denial, require};

/// Tenant-aware accessor for the workflows family.
pub struct Workflows {
    store: crate::SQLiteStore,
    emitter: Option<kura_audit::Emitter>,
}

impl Workflows {
    #[must_use]
    pub fn new(store: crate::SQLiteStore, emitter: Option<kura_audit::Emitter>) -> Self {
        Workflows { store, emitter }
    }

    fn emit(&self, surface: &str, resource_kind: &str) {
        emit_denial(&self.emitter, surface, resource_kind);
    }

    pub fn list_workflows_for_tenant(
        &self,
        environment_scope: &str,
        run_id: &str,
    ) -> Result<Vec<kura_orchestration::Workflow>, TenancyError> {
        let tenant_id = require()?;
        self.store
            .list_workflows_for_tenant_raw(&tenant_id, environment_scope, run_id)
            .map_err(TenancyError::from)
    }

    pub fn get_workflow_for_tenant(
        &self,
        environment_scope: &str,
        run_id: &str,
        workflow_id: &str,
    ) -> Result<Option<kura_orchestration::Workflow>, TenancyError> {
        let tenant_id = require()?;
        let owner = self
            .store
            .lookup_row_tenant("workflows", "workflow_id", workflow_id)
            .map_err(TenancyError::from)?;
        match owner {
            None => Ok(None),
            Some(owner) if !owner.is_empty() && owner != tenant_id => {
                self.emit("store:GetWorkflowForTenant", "workflow");
                Ok(None)
            }
            _ => self
                .store
                .get_workflow(environment_scope, run_id, workflow_id)
                .map_err(TenancyError::from),
        }
    }

    pub fn upsert_workflow_for_tenant(
        &self,
        workflow: &kura_orchestration::Workflow,
    ) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        self.store
            .upsert_workflow(workflow)
            .map_err(TenancyError::from)?;
        match self.store.bind_row_tenant(
            "workflows",
            "workflow_id",
            &workflow.workflow_id,
            &tenant_id,
        ) {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit("store:UpsertWorkflowForTenant", "workflow");
                Err(TenancyError::CrossTenantWrite)
            }
            other => other.map_err(TenancyError::from),
        }
    }
}
