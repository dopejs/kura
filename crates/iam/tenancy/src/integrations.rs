//! Tenant-aware accessor for the integrations table.
//! Port of daemon/internal/store/tenancy/integrations.go.
//!
//! Roadmap 37 boundary (binding constraint, enforced by T089c): this helper exposes ONLY
//! ownership wiring and query isolation. It MUST NOT add, remove, or alter any function
//! that participates in credential storage, OAuth state, secret-reference resolution, or
//! readiness probing.

use crate::{TenancyError, emit_denial, require};

/// Tenant-aware accessor for the integrations table.
pub struct Integrations {
    store: crate::SQLiteStore,
    emitter: Option<kura_audit::Emitter>,
}

impl Integrations {
    #[must_use]
    pub fn new(store: crate::SQLiteStore, emitter: Option<kura_audit::Emitter>) -> Self {
        Integrations { store, emitter }
    }

    fn emit(&self, surface: &str, resource_kind: &str) {
        emit_denial(&self.emitter, surface, resource_kind);
    }

    /// Persists an integration row and binds its tenant_id. Pass A only wires ownership;
    /// readiness/credential probing remains owned by Roadmap 37 (untouched here).
    pub fn upsert_integration_for_tenant(
        &self,
        item: &kura_integrations::Resource,
    ) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        let owner = self
            .store
            .lookup_row_tenant("integrations", "integration_id", &item.integration_id)
            .map_err(TenancyError::from)?;
        if let Some(owner) = owner {
            if !owner.is_empty() && owner != tenant_id {
                self.emit("store:UpsertIntegrationForTenant", "integration");
                return Err(TenancyError::CrossTenantWrite);
            }
        }
        let mut bound = item.clone();
        bound.tenant_id = tenant_id.clone();
        self.store
            .upsert_integration(&bound)
            .map_err(TenancyError::from)?;
        match self.store.bind_row_tenant(
            "integrations",
            "integration_id",
            &bound.integration_id,
            &tenant_id,
        ) {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit("store:UpsertIntegrationForTenant", "integration");
                Err(TenancyError::CrossTenantWrite)
            }
            other => other.map_err(TenancyError::from),
        }
    }
}
