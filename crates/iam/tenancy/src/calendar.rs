//! Tenant-aware accessor for calendar_accounts, calendar_operations,
//! calendar_artifacts. Port of daemon/internal/store/tenancy/calendar.go.

use crate::{TenancyError, emit_denial, require};

/// Tenant-aware accessor for the calendar family.
pub struct Calendar {
    store: crate::SQLiteStore,
    emitter: Option<kura_audit::Emitter>,
}

impl Calendar {
    #[must_use]
    pub fn new(store: crate::SQLiteStore, emitter: Option<kura_audit::Emitter>) -> Self {
        Calendar { store, emitter }
    }

    fn emit(&self, surface: &str, resource_kind: &str) {
        emit_denial(&self.emitter, surface, resource_kind);
    }

    pub fn upsert_account_for_tenant(
        &self,
        item: &kura_calendar::AccountProjection,
    ) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        self.store
            .upsert_calendar_account(item)
            .map_err(TenancyError::from)?;
        match self.store.bind_row_tenant(
            "calendar_accounts",
            "calendar_account_id",
            &item.calendar_account_id,
            &tenant_id,
        ) {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit("store:UpsertCalendarAccountForTenant", "calendar_account");
                Err(TenancyError::CrossTenantWrite)
            }
            other => other.map_err(TenancyError::from),
        }
    }

    pub fn upsert_operation_for_tenant(
        &self,
        item: &kura_calendar::Operation,
    ) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        self.store
            .upsert_calendar_operation(item)
            .map_err(TenancyError::from)?;
        match self.store.bind_row_tenant(
            "calendar_operations",
            "operation_id",
            &item.operation_id,
            &tenant_id,
        ) {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit(
                    "store:UpsertCalendarOperationForTenant",
                    "calendar_operation",
                );
                Err(TenancyError::CrossTenantWrite)
            }
            other => other.map_err(TenancyError::from),
        }
    }

    pub fn upsert_artifact_for_tenant(
        &self,
        item: &kura_calendar::Artifact,
    ) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        self.store
            .upsert_calendar_artifact(item)
            .map_err(TenancyError::from)?;
        match self.store.bind_row_tenant(
            "calendar_artifacts",
            "artifact_id",
            &item.artifact_id,
            &tenant_id,
        ) {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit("store:UpsertCalendarArtifactForTenant", "calendar_artifact");
                Err(TenancyError::CrossTenantWrite)
            }
            other => other.map_err(TenancyError::from),
        }
    }
}
