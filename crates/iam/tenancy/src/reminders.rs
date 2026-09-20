//! Tenant-aware accessor for reminders, reminder_occurrences, reminder_actions.
//! Port of daemon/internal/store/tenancy/reminders.go.

use crate::{TenancyError, emit_denial, require};
use kura_store::reminders::{ReminderOccurrenceRecord, ReminderRecord};

/// Tenant-aware accessor for the reminders family.
pub struct Reminders {
    store: crate::SQLiteStore,
    emitter: Option<kura_audit::Emitter>,
}

impl Reminders {
    #[must_use]
    pub fn new(store: crate::SQLiteStore, emitter: Option<kura_audit::Emitter>) -> Self {
        Reminders { store, emitter }
    }

    fn emit(&self, surface: &str, resource_kind: &str) {
        emit_denial(&self.emitter, surface, resource_kind);
    }

    pub fn upsert_reminder_for_tenant(&self, record: &ReminderRecord) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        self.store
            .upsert_reminder(record)
            .map_err(TenancyError::from)?;
        match self.store.bind_row_tenant(
            "reminders",
            "reminder_id",
            &record.reminder_id,
            &tenant_id,
        ) {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit("store:UpsertReminderForTenant", "reminder");
                Err(TenancyError::CrossTenantWrite)
            }
            other => other.map_err(TenancyError::from),
        }
    }

    pub fn upsert_occurrence_for_tenant(
        &self,
        record: &ReminderOccurrenceRecord,
    ) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        self.store
            .upsert_reminder_occurrence(record)
            .map_err(TenancyError::from)?;
        match self.store.bind_row_tenant(
            "reminder_occurrences",
            "occurrence_id",
            &record.occurrence_id,
            &tenant_id,
        ) {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit(
                    "store:UpsertReminderOccurrenceForTenant",
                    "reminder_occurrence",
                );
                Err(TenancyError::CrossTenantWrite)
            }
            other => other.map_err(TenancyError::from),
        }
    }
}
