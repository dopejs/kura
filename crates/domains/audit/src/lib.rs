//! Port of daemon/internal/audit: operator-visible audit event emitters for tenant-related
//! security failures and domain-specific audit trails. The cross-tenant access denial emitter is
//! ported first; billing/credential/evaluation-product/integration-diagnostics/live-validation
//! emitters follow incrementally as their domain payload types land.

use std::sync::Arc;

use kura_events::{Bus, Event, Resource};
mod builders;

pub use builders::{
    BILLING_AUDIT_EVENT_KIND, BillingAuditInput, CREDENTIAL_EVENT_KIND, CredentialAuditInput,
    INTEGRATION_DIAGNOSTIC_AUDIT_EVENT_KIND, IntegrationDiagnosticAuditInput,
    build_billing_audit_event, build_credential_audit_event,
    build_integration_diagnostic_audit_event, default_billing_audit_retention_policy,
};

use kura_identity::tenantctx;
use thiserror::Error;

/// Stable structured log code emitted alongside the cross-tenant denial event.
pub const LOG_CODE: &str = "audit.cross_tenant_access_denied";
/// Runtime-event envelope category for the cross-tenant denial event.
pub const EVENT_CATEGORY: &str = "audit";
/// Runtime-event envelope name for the cross-tenant denial event.
pub const EVENT_NAME: &str = "audit.cross_tenant_access_denied";

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AuditError {
    #[error("audit: cannot emit tenant breach without acting tenant context")]
    NoActingTenant,
}

/// Publishes cross-tenant access denial audit events on the bus. Instances are stateless and
/// cheap to allocate.
pub struct Emitter {
    bus: Arc<Bus>,
}

impl Emitter {
    #[must_use]
    pub fn new(bus: Arc<Bus>) -> Self {
        Emitter { bus }
    }

    /// Publishes the audit event for a denied cross-tenant access. The acting tenant context is
    /// read from the task-local carrier; the payload intentionally contains only the acting
    /// tenant id, principal id, surface, and resource kind (never the target tenant/resource).
    pub fn emit(&self, surface: &str, resource_kind: &str) -> Result<(), AuditError> {
        let tc = tenantctx::from_context().ok_or(AuditError::NoActingTenant)?;
        if tc.tenant_id.is_empty() {
            return Err(AuditError::NoActingTenant);
        }

        let mut payload = serde_json::Map::new();
        payload.insert(
            "actingTenantId".to_string(),
            serde_json::json!(tc.tenant_id),
        );
        payload.insert(
            "principalId".to_string(),
            serde_json::json!(tc.principal_id),
        );
        payload.insert("surface".to_string(), serde_json::json!(surface));
        payload.insert("resourceKind".to_string(), serde_json::json!(resource_kind));

        self.bus.publish(Event {
            tenant_id: tc.tenant_id.clone(),
            category: EVENT_CATEGORY.to_string(),
            name: EVENT_NAME.to_string(),
            resource: Resource {
                kind: "tenant".to_string(),
                id: tc.tenant_id,
            },
            payload,
            ..Event::default()
        });

        Ok(())
    }
}
