//! Tenant-aware accessor for the schedules family (schedules, schedule_targets,
//! schedule_dispatch_attempts). Port of daemon/internal/store/tenancy/schedules.go.
//! Pass A semantics mirror tenancy::Runtime: fail-closed reads, write-then-bind
//! upserts, audit emission on cross-tenant lookup.

use crate::{TenancyError, emit_denial, require};
use kura_store::schedule::{ScheduleDispatchAttemptRecord, ScheduleRecord, ScheduleTargetRecord};

/// Tenant-aware accessor for the schedules family.
pub struct Schedules {
    store: crate::SQLiteStore,
    emitter: Option<kura_audit::Emitter>,
}

impl Schedules {
    #[must_use]
    pub fn new(store: crate::SQLiteStore, emitter: Option<kura_audit::Emitter>) -> Self {
        Schedules { store, emitter }
    }

    fn emit(&self, surface: &str, resource_kind: &str) {
        emit_denial(&self.emitter, surface, resource_kind);
    }

    // ----- schedules -----

    pub fn list_schedules_for_tenant(
        &self,
        environment_scope: &str,
    ) -> Result<Vec<ScheduleRecord>, TenancyError> {
        let tenant_id = require()?;
        self.store
            .list_schedules_for_tenant_raw(&tenant_id, environment_scope)
            .map_err(TenancyError::from)
    }

    pub fn get_schedule_for_tenant(
        &self,
        environment_scope: &str,
        schedule_id: &str,
    ) -> Result<Option<ScheduleRecord>, TenancyError> {
        let tenant_id = require()?;
        let owner = self
            .store
            .lookup_row_tenant("schedules", "schedule_id", schedule_id)
            .map_err(TenancyError::from)?;
        match owner {
            None => Ok(None),
            Some(owner) if !owner.is_empty() && owner != tenant_id => {
                self.emit("store:GetScheduleForTenant", "schedule");
                Ok(None)
            }
            _ => self
                .store
                .get_schedule(environment_scope, schedule_id)
                .map_err(TenancyError::from),
        }
    }

    pub fn upsert_schedule_for_tenant(&self, record: &ScheduleRecord) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        self.store
            .upsert_schedule(record)
            .map_err(TenancyError::from)?;
        match self.store.bind_row_tenant(
            "schedules",
            "schedule_id",
            &record.schedule_id,
            &tenant_id,
        ) {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit("store:UpsertScheduleForTenant", "schedule");
                Err(TenancyError::CrossTenantWrite)
            }
            other => other.map_err(TenancyError::from),
        }
    }

    // ----- schedule_targets -----

    pub fn upsert_schedule_target_for_tenant(
        &self,
        record: &ScheduleTargetRecord,
    ) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        self.store
            .upsert_schedule_target(record)
            .map_err(TenancyError::from)?;
        match self.store.bind_row_tenant(
            "schedule_targets",
            "target_ref_id",
            &record.target_ref_id,
            &tenant_id,
        ) {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit("store:UpsertScheduleTargetForTenant", "schedule_target");
                Err(TenancyError::CrossTenantWrite)
            }
            other => other.map_err(TenancyError::from),
        }
    }

    pub fn get_schedule_target_for_tenant(
        &self,
        schedule_id: &str,
        target_ref_id: &str,
    ) -> Result<Option<ScheduleTargetRecord>, TenancyError> {
        let tenant_id = require()?;
        let owner = self
            .store
            .lookup_row_tenant("schedule_targets", "target_ref_id", target_ref_id)
            .map_err(TenancyError::from)?;
        match owner {
            None => Ok(None),
            Some(owner) if !owner.is_empty() && owner != tenant_id => {
                self.emit("store:GetScheduleTargetForTenant", "schedule_target");
                Ok(None)
            }
            _ => self
                .store
                .get_schedule_target(schedule_id, target_ref_id)
                .map_err(TenancyError::from),
        }
    }

    // ----- schedule_dispatch_attempts -----

    pub fn upsert_schedule_dispatch_attempt_for_tenant(
        &self,
        record: &ScheduleDispatchAttemptRecord,
    ) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        self.store
            .upsert_schedule_dispatch_attempt(record)
            .map_err(TenancyError::from)?;
        match self.store.bind_row_tenant(
            "schedule_dispatch_attempts",
            "attempt_id",
            &record.attempt_id,
            &tenant_id,
        ) {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit(
                    "store:UpsertScheduleDispatchAttemptForTenant",
                    "schedule_dispatch_attempt",
                );
                Err(TenancyError::CrossTenantWrite)
            }
            other => other.map_err(TenancyError::from),
        }
    }

    pub fn list_schedule_dispatch_attempts_for_tenant(
        &self,
        schedule_id: &str,
    ) -> Result<Vec<ScheduleDispatchAttemptRecord>, TenancyError> {
        let tenant_id = require()?;
        let owner = self
            .store
            .lookup_row_tenant("schedules", "schedule_id", schedule_id)
            .map_err(TenancyError::from)?;
        match owner {
            None => Ok(Vec::new()),
            Some(owner) if !owner.is_empty() && owner != tenant_id => {
                self.emit("store:ListScheduleDispatchAttemptsForTenant", "schedule");
                Ok(Vec::new())
            }
            _ => self
                .store
                .list_schedule_dispatch_attempts(schedule_id)
                .map_err(TenancyError::from),
        }
    }
}
