//! Tenant-aware accessor for the mixed events table.
//! Port of daemon/internal/store/tenancy/events.go.
//!
//! Usage rules:
//! - For tenant-owned categories (every category not global), call append_event_for_tenant
//!   — it requires a resolved tenant context and binds tenant_id on the persisted row.
//! - For global categories (mcp, provider, system, daemon.migration, connector_global,
//!   capability_global), the store's append_event path is preserved (tenant_id NULL).
//!
//! append_event_for_tenant returns TenancyError::TenantContextRequired when the context
//! lacks a tenant; this is fail-closed by design.

use crate::{TenancyError, require};

/// Tenant-aware accessor for the events table.
pub struct Events {
    store: crate::SQLiteStore,
    #[allow(dead_code)]
    emitter: Option<kura_audit::Emitter>,
}

impl Events {
    #[must_use]
    pub fn new(store: crate::SQLiteStore, emitter: Option<kura_audit::Emitter>) -> Self {
        Events { store, emitter }
    }

    /// Reports whether the given event category is global (NULL tenant_id allowed).
    /// Delegates to kura_events::is_global_category so the global set has a single
    /// source of truth in the events crate.
    #[must_use]
    pub fn is_global_category(category: &str) -> bool {
        kura_events::is_global_category(category)
    }

    /// Persists a tenant-owned event with tenant_id pre-bound. Returns the persisted
    /// event including the assigned sequence and the bound tenant id.
    pub fn append_event_for_tenant(
        &self,
        event: &kura_events::Event,
    ) -> Result<kura_events::Event, TenancyError> {
        let tenant_id = require()?;
        if Self::is_global_category(&event.category) {
            return Err(TenancyError::Store(
                "tenancy: refused to bind tenant on a global event category — use store.append_event".to_string(),
            ));
        }
        let mut prepared = event.clone();
        prepared.tenant_id = tenant_id.clone();
        self.store
            .append_event_for_tenant_raw(&prepared, &tenant_id)
            .map_err(TenancyError::from)
    }

    /// Returns persisted events whose tenant_id matches the caller. Global rows
    /// (tenant_id NULL) are NOT returned.
    pub fn list_events_for_tenant(
        &self,
        filter: &kura_events::Filter,
    ) -> Result<Vec<kura_events::Event>, TenancyError> {
        let tenant_id = require()?;
        self.store
            .list_events_for_tenant_raw(&tenant_id, filter)
            .map_err(TenancyError::from)
    }
}
