//! Tenant-aware accessor for mail_accounts, mail_operations, mail_artifacts.
//! Port of daemon/internal/store/tenancy/mail.go.

use crate::{TenancyError, emit_denial, require};

/// Tenant-aware accessor for the mail family.
pub struct Mail {
    store: crate::SQLiteStore,
    emitter: Option<kura_audit::Emitter>,
}

impl Mail {
    #[must_use]
    pub fn new(store: crate::SQLiteStore, emitter: Option<kura_audit::Emitter>) -> Self {
        Mail { store, emitter }
    }

    fn emit(&self, surface: &str, resource_kind: &str) {
        emit_denial(&self.emitter, surface, resource_kind);
    }

    pub fn upsert_account_for_tenant(
        &self,
        item: &kura_mail::AccountProjection,
    ) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        self.store
            .upsert_mail_account(item)
            .map_err(TenancyError::from)?;
        match self.store.bind_row_tenant(
            "mail_accounts",
            "mail_account_id",
            &item.mail_account_id,
            &tenant_id,
        ) {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit("store:UpsertMailAccountForTenant", "mail_account");
                Err(TenancyError::CrossTenantWrite)
            }
            other => other.map_err(TenancyError::from),
        }
    }

    pub fn upsert_operation_for_tenant(
        &self,
        item: &kura_mail::Operation,
    ) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        self.store
            .upsert_mail_operation(item)
            .map_err(TenancyError::from)?;
        match self.store.bind_row_tenant(
            "mail_operations",
            "operation_id",
            &item.operation_id,
            &tenant_id,
        ) {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit("store:UpsertMailOperationForTenant", "mail_operation");
                Err(TenancyError::CrossTenantWrite)
            }
            other => other.map_err(TenancyError::from),
        }
    }

    pub fn upsert_artifact_for_tenant(
        &self,
        item: &kura_mail::Artifact,
    ) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        self.store
            .upsert_mail_artifact(item)
            .map_err(TenancyError::from)?;
        match self.store.bind_row_tenant(
            "mail_artifacts",
            "artifact_id",
            &item.artifact_id,
            &tenant_id,
        ) {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit("store:UpsertMailArtifactForTenant", "mail_artifact");
                Err(TenancyError::CrossTenantWrite)
            }
            other => other.map_err(TenancyError::from),
        }
    }
}
