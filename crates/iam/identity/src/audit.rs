//! Tenant audit recording.
//!
//! Port of `daemon/internal/identity/audit.go`. [`Auditor::require`] fails
//! closed: a store error is collapsed to [`IdentityError::AuditWriteFailed`]
//! so callers cannot proceed unaudited.

use std::sync::Arc;

use chrono::DateTime;
use chrono::Utc;

use crate::types::IdentityError;
use crate::types::TenantAuditEvent;
use crate::types::epoch;

pub const AUDIT_OUTCOME_SUCCEEDED: &str = "succeeded";
pub const AUDIT_OUTCOME_DENIED: &str = "denied";
pub const AUDIT_OUTCOME_FAILED_CLOSED: &str = "failed_closed";

pub trait AuditStore {
    fn append_tenant_audit_event(
        &self,
        event: TenantAuditEvent,
    ) -> Result<TenantAuditEvent, IdentityError>;
}

pub struct Auditor<S: AuditStore + ?Sized> {
    store: Arc<S>,
    now: Box<dyn Fn() -> DateTime<Utc> + Send + Sync>,
}

impl<S: AuditStore + ?Sized> Auditor<S> {
    pub fn new(store: Arc<S>) -> Self {
        Self {
            store,
            now: Box::new(Utc::now),
        }
    }

    pub fn with_now(mut self, now: impl Fn() -> DateTime<Utc> + Send + Sync + 'static) -> Self {
        self.now = Box::new(now);
        self
    }

    /// Stamps `created_at` when unset, then appends. Store errors propagate.
    pub fn record(&self, mut event: TenantAuditEvent) -> Result<TenantAuditEvent, IdentityError> {
        if event.created_at == epoch() {
            event.created_at = (self.now)();
        }
        self.store.append_tenant_audit_event(event)
    }

    /// Like [`Auditor::record`] but any write failure is reported as
    /// [`IdentityError::AuditWriteFailed`] so the guarded action must abort.
    pub fn require(&self, event: TenantAuditEvent) -> Result<TenantAuditEvent, IdentityError> {
        self.record(event)
            .map_err(|_| IdentityError::AuditWriteFailed)
    }
}
