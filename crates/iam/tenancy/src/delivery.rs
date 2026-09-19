//! Tenant-aware accessor for the delivery family — delivery_targets,
//! delivery_preferences, delivery_outcomes, delivery_attempts,
//! delivery_summary_windows. Port of daemon/internal/store/tenancy/delivery.go.

use crate::{TenancyError, emit_denial, require};
use kura_store::delivery::{
    DeliveryAttemptRecord, DeliveryOutcomeRecord, DeliveryPreferenceRecord,
    DeliverySummaryWindowRecord, DeliveryTargetRecord,
};

/// Tenant-aware accessor for the delivery family.
pub struct Delivery {
    store: crate::SQLiteStore,
    emitter: Option<kura_audit::Emitter>,
}

impl Delivery {
    #[must_use]
    pub fn new(store: crate::SQLiteStore, emitter: Option<kura_audit::Emitter>) -> Self {
        Delivery { store, emitter }
    }

    fn emit(&self, surface: &str, resource_kind: &str) {
        emit_denial(&self.emitter, surface, resource_kind);
    }

    pub fn upsert_target_for_tenant(
        &self,
        record: &DeliveryTargetRecord,
    ) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        self.store
            .upsert_delivery_target(record)
            .map_err(TenancyError::from)?;
        match self.store.bind_row_tenant(
            "delivery_targets",
            "target_id",
            &record.target_id,
            &tenant_id,
        ) {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit("store:UpsertDeliveryTargetForTenant", "delivery_target");
                Err(TenancyError::CrossTenantWrite)
            }
            other => other.map_err(TenancyError::from),
        }
    }

    pub fn upsert_preference_for_tenant(
        &self,
        record: &DeliveryPreferenceRecord,
    ) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        self.store
            .upsert_delivery_preference(record)
            .map_err(TenancyError::from)?;
        match self.store.bind_row_tenant(
            "delivery_preferences",
            "preference_id",
            &record.preference_id,
            &tenant_id,
        ) {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit(
                    "store:UpsertDeliveryPreferenceForTenant",
                    "delivery_preference",
                );
                Err(TenancyError::CrossTenantWrite)
            }
            other => other.map_err(TenancyError::from),
        }
    }

    pub fn upsert_outcome_for_tenant(
        &self,
        record: &DeliveryOutcomeRecord,
    ) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        self.store
            .upsert_delivery_outcome(record)
            .map_err(TenancyError::from)?;
        match self.store.bind_row_tenant(
            "delivery_outcomes",
            "delivery_id",
            &record.delivery_id,
            &tenant_id,
        ) {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit("store:UpsertDeliveryOutcomeForTenant", "delivery_outcome");
                Err(TenancyError::CrossTenantWrite)
            }
            other => other.map_err(TenancyError::from),
        }
    }

    pub fn upsert_attempt_for_tenant(
        &self,
        record: &DeliveryAttemptRecord,
    ) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        self.store
            .upsert_delivery_attempt(record)
            .map_err(TenancyError::from)?;
        match self.store.bind_row_tenant(
            "delivery_attempts",
            "attempt_id",
            &record.attempt_id,
            &tenant_id,
        ) {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit("store:UpsertDeliveryAttemptForTenant", "delivery_attempt");
                Err(TenancyError::CrossTenantWrite)
            }
            other => other.map_err(TenancyError::from),
        }
    }

    pub fn upsert_summary_window_for_tenant(
        &self,
        record: &DeliverySummaryWindowRecord,
    ) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        self.store
            .upsert_delivery_summary_window(record)
            .map_err(TenancyError::from)?;
        match self.store.bind_row_tenant(
            "delivery_summary_windows",
            "summary_window_id",
            &record.summary_window_id,
            &tenant_id,
        ) {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit(
                    "store:UpsertDeliverySummaryWindowForTenant",
                    "delivery_summary_window",
                );
                Err(TenancyError::CrossTenantWrite)
            }
            other => other.map_err(TenancyError::from),
        }
    }
}
