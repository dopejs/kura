//! Tenant-aware accessor for credential-bearing resources whose ownership moved from
//! global/R35-boundary state into Roadmap 37: provider_auth_states, connectors,
//! mcp_servers, mcp_server_states, and mcp_tools.
//! Port of daemon/internal/store/tenancy/r37_resources.go.

use crate::{TenancyError, emit_denial, require};
use kura_store::mcp::{MCPServerRecord, MCPServerStateRecord, MCPToolRecord};

/// Storage key used to namespace provider auth states per tenant
/// (Go r37StorageKey: trimmed tenantID + "::" + trimmed id).
fn r37_storage_key(tenant_id: &str, id: &str) -> String {
    format!("{}::{}", tenant_id.trim(), id.trim())
}

/// Tenant-aware accessor for Roadmap 37 credential-bearing resources.
pub struct R37Resources {
    store: crate::SQLiteStore,
    emitter: Option<kura_audit::Emitter>,
}

impl R37Resources {
    #[must_use]
    pub fn new(store: crate::SQLiteStore, emitter: Option<kura_audit::Emitter>) -> Self {
        R37Resources { store, emitter }
    }

    fn emit(&self, surface: &str, resource_kind: &str) {
        emit_denial(&self.emitter, surface, resource_kind);
    }

    fn guard_scalar_tenant(
        &self,
        table: &str,
        pk_column: &str,
        pk: &str,
        tenant_id: &str,
        surface: &str,
        resource_kind: &str,
    ) -> Result<(), TenancyError> {
        let owner = self
            .store
            .lookup_row_tenant(table, pk_column, pk)
            .map_err(TenancyError::from)?;
        if let Some(owner) = owner {
            if !owner.is_empty() && owner != tenant_id {
                self.emit(surface, resource_kind);
                return Err(TenancyError::CrossTenantWrite);
            }
        }
        Ok(())
    }

    fn guard_mcp_tool_tenant(
        &self,
        server_id: &str,
        tool_name: &str,
        tenant_id: &str,
    ) -> Result<(), TenancyError> {
        let existing = self
            .store
            .mcp_tool_tenant_id(server_id, tool_name)
            .map_err(TenancyError::from)?;
        if let Some(existing) = existing {
            if !existing.is_empty() && existing != tenant_id {
                return Err(TenancyError::CrossTenantWrite);
            }
        }
        Ok(())
    }

    /// Upserts a provider auth state, namespacing the storage key with the tenant id and
    /// binding tenant_id on the row.
    pub fn upsert_provider_auth_state_for_tenant(
        &self,
        state: &mut kura_providers::AuthState,
    ) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        state.tenant_id = tenant_id.clone();
        state.provider_id = r37_storage_key(&tenant_id, &state.provider_id);
        self.store
            .upsert_provider_auth_state(state)
            .map_err(TenancyError::from)?;
        match self.store.bind_row_tenant(
            "provider_auth_states",
            "provider_id",
            &state.provider_id,
            &tenant_id,
        ) {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit(
                    "store:UpsertProviderAuthStateForTenant",
                    "provider_auth_state",
                );
                Err(TenancyError::CrossTenantWrite)
            }
            other => other.map_err(TenancyError::from),
        }
    }

    pub fn upsert_connector_for_tenant(
        &self,
        connector: &kura_connectors::Connector,
    ) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        self.guard_scalar_tenant(
            "connectors",
            "connector_id",
            &connector.connector_id,
            &tenant_id,
            "store:UpsertConnectorForTenant",
            "connector",
        )?;
        self.store
            .upsert_connector(connector)
            .map_err(TenancyError::from)?;
        match self.store.bind_row_tenant(
            "connectors",
            "connector_id",
            &connector.connector_id,
            &tenant_id,
        ) {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit("store:UpsertConnectorForTenant", "connector");
                Err(TenancyError::CrossTenantWrite)
            }
            other => other.map_err(TenancyError::from),
        }
    }

    pub fn upsert_mcp_server_for_tenant(
        &self,
        record: &MCPServerRecord,
    ) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        self.guard_scalar_tenant(
            "mcp_servers",
            "server_id",
            &record.server_id,
            &tenant_id,
            "store:UpsertMCPServerForTenant",
            "mcp_server",
        )?;
        self.store
            .upsert_mcp_server(record)
            .map_err(TenancyError::from)?;
        match self
            .store
            .bind_row_tenant("mcp_servers", "server_id", &record.server_id, &tenant_id)
        {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit("store:UpsertMCPServerForTenant", "mcp_server");
                Err(TenancyError::CrossTenantWrite)
            }
            other => other.map_err(TenancyError::from),
        }
    }

    pub fn upsert_mcp_server_state_for_tenant(
        &self,
        record: &MCPServerStateRecord,
    ) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        self.guard_scalar_tenant(
            "mcp_server_states",
            "server_id",
            &record.server_id,
            &tenant_id,
            "store:UpsertMCPServerStateForTenant",
            "mcp_server_state",
        )?;
        self.store
            .upsert_mcp_server_state(record)
            .map_err(TenancyError::from)?;
        match self.store.bind_row_tenant(
            "mcp_server_states",
            "server_id",
            &record.server_id,
            &tenant_id,
        ) {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit("store:UpsertMCPServerStateForTenant", "mcp_server_state");
                Err(TenancyError::CrossTenantWrite)
            }
            other => other.map_err(TenancyError::from),
        }
    }

    pub fn upsert_mcp_tool_for_tenant(&self, record: &MCPToolRecord) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        if let Err(e) = self.guard_mcp_tool_tenant(&record.server_id, &record.tool_name, &tenant_id)
        {
            if e == TenancyError::CrossTenantWrite {
                self.emit("store:UpsertMCPToolForTenant", "mcp_tool");
            }
            return Err(e);
        }
        self.store
            .upsert_mcp_tool(record)
            .map_err(TenancyError::from)?;
        match self
            .store
            .bind_mcp_tool_tenant(&record.server_id, &record.tool_name, &tenant_id)
        {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit("store:UpsertMCPToolForTenant", "mcp_tool");
                Err(TenancyError::CrossTenantWrite)
            }
            other => other.map_err(TenancyError::from),
        }
    }

    pub fn replace_mcp_tools_for_tenant(
        &self,
        server_id: &str,
        records: &[MCPToolRecord],
    ) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        self.guard_scalar_tenant(
            "mcp_servers",
            "server_id",
            server_id,
            &tenant_id,
            "store:ReplaceMCPToolsForTenant",
            "mcp_server",
        )?;
        for record in records {
            if let Err(e) =
                self.guard_mcp_tool_tenant(&record.server_id, &record.tool_name, &tenant_id)
            {
                if e == TenancyError::CrossTenantWrite {
                    self.emit("store:ReplaceMCPToolsForTenant", "mcp_tool");
                }
                return Err(e);
            }
        }
        self.store
            .replace_mcp_tools(server_id, records)
            .map_err(TenancyError::from)?;
        for record in records {
            match self
                .store
                .bind_mcp_tool_tenant(&record.server_id, &record.tool_name, &tenant_id)
            {
                Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                    self.emit("store:ReplaceMCPToolsForTenant", "mcp_tool");
                    return Err(TenancyError::CrossTenantWrite);
                }
                Err(e) => return Err(TenancyError::from(e)),
                Ok(()) => {}
            }
        }
        Ok(())
    }
}
