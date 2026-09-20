//! Tenant-aware accessor for computer_use_sessions, computer_use_actions,
//! computer_use_artifacts. Port of daemon/internal/store/tenancy/computer_use.go.

use crate::{TenancyError, emit_denial, require};

/// Tenant-aware accessor for the computer-use family.
pub struct ComputerUse {
    store: crate::SQLiteStore,
    emitter: Option<kura_audit::Emitter>,
}

impl ComputerUse {
    #[must_use]
    pub fn new(store: crate::SQLiteStore, emitter: Option<kura_audit::Emitter>) -> Self {
        ComputerUse { store, emitter }
    }

    fn emit(&self, surface: &str, resource_kind: &str) {
        emit_denial(&self.emitter, surface, resource_kind);
    }

    pub fn upsert_session_for_tenant(
        &self,
        session: &kura_computeruse::Session,
    ) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        self.store
            .upsert_computer_use_session(session)
            .map_err(TenancyError::from)?;
        match self.store.bind_row_tenant(
            "computer_use_sessions",
            "computer_use_session_id",
            &session.computer_use_session_id,
            &tenant_id,
        ) {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit(
                    "store:UpsertComputerUseSessionForTenant",
                    "computer_use_session",
                );
                Err(TenancyError::CrossTenantWrite)
            }
            other => other.map_err(TenancyError::from),
        }
    }

    pub fn upsert_action_for_tenant(
        &self,
        action: &kura_computeruse::Action,
    ) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        self.store
            .upsert_computer_use_action(action)
            .map_err(TenancyError::from)?;
        match self.store.bind_row_tenant(
            "computer_use_actions",
            "computer_use_action_id",
            &action.computer_use_action_id,
            &tenant_id,
        ) {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit(
                    "store:UpsertComputerUseActionForTenant",
                    "computer_use_action",
                );
                Err(TenancyError::CrossTenantWrite)
            }
            other => other.map_err(TenancyError::from),
        }
    }

    pub fn upsert_artifact_for_tenant(
        &self,
        artifact: &kura_computeruse::Artifact,
    ) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        self.store
            .upsert_computer_use_artifact(artifact)
            .map_err(TenancyError::from)?;
        match self.store.bind_row_tenant(
            "computer_use_artifacts",
            "artifact_id",
            &artifact.artifact_id,
            &tenant_id,
        ) {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit(
                    "store:UpsertComputerUseArtifactForTenant",
                    "computer_use_artifact",
                );
                Err(TenancyError::CrossTenantWrite)
            }
            other => other.map_err(TenancyError::from),
        }
    }
}
