use std::sync::Arc;

use kura_audit::{AuditError, EVENT_CATEGORY, EVENT_NAME, Emitter};
use kura_events::{Bus, Filter};
use kura_identity::TenantContext;
use kura_identity::tenantctx;

#[test]
fn emit_requires_tenant_context() {
    let emitter = Emitter::new(Arc::new(Bus::new()));
    assert_eq!(emitter.emit("api", "run"), Err(AuditError::NoActingTenant));
}

#[test]
fn emit_publishes_denial_event() {
    let bus = Arc::new(Bus::new());
    let emitter = Emitter::new(Arc::clone(&bus));
    let tc = TenantContext {
        principal_id: "principal_1".to_string(),
        tenant_id: "tenant_1".to_string(),
        ..TenantContext::default()
    };
    tenantctx::with_context(tc, || {
        emitter.emit("api:GET /v1/runs/{id}", "run").unwrap();
    });

    let events = bus.list(&Filter {
        category: EVENT_CATEGORY.to_string(),
        ..Filter::default()
    });
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].name, EVENT_NAME);
    assert_eq!(events[0].tenant_id, "tenant_1");
    assert_eq!(events[0].resource.kind, "tenant");
    assert_eq!(events[0].payload["actingTenantId"], "tenant_1");
    assert_eq!(events[0].payload["surface"], "api:GET /v1/runs/{id}");
    assert_eq!(events[0].payload["resourceKind"], "run");
}
use kura_audit::{
    BILLING_AUDIT_EVENT_KIND, BillingAuditInput, CREDENTIAL_EVENT_KIND, CredentialAuditInput,
    INTEGRATION_DIAGNOSTIC_AUDIT_EVENT_KIND, IntegrationDiagnosticAuditInput,
    build_billing_audit_event, build_credential_audit_event,
    build_integration_diagnostic_audit_event,
};
use kura_identity::AUDIT_OUTCOME_SUCCEEDED;

#[test]
fn builders_produce_tenant_audit_events() {
    let billing = build_billing_audit_event(&BillingAuditInput {
        tenant_id: "t1".to_string(),
        principal_id: "p1".to_string(),
        category: kura_billing::Category::new("run_launches"),
        operation_key: "op".to_string(),
        amount: 5,
        ..BillingAuditInput::default()
    });
    assert_eq!(billing.event_kind, BILLING_AUDIT_EVENT_KIND);
    assert_eq!(billing.outcome, AUDIT_OUTCOME_SUCCEEDED);
    assert_eq!(
        billing.document.as_ref().unwrap()["category"],
        "run_launches"
    );
    assert_eq!(billing.document.as_ref().unwrap()["amount"], 5);

    let credential = build_credential_audit_event(&CredentialAuditInput {
        tenant_id: "t1".to_string(),
        principal_id: "p1".to_string(),
        resource_kind: kura_secrets::ResourceKind::TenantSecret,
        action: kura_secrets::AuditAction::SecretUse,
        secret_refs: vec!["secret/abc".to_string()],
        ..CredentialAuditInput {
            outcome: String::new(),
            reason_code: String::new(),
            secret_ref: String::new(),
            secret_version_id: String::new(),
            created_at: chrono::Utc::now(),
            tenant_id: "t1".to_string(),
            principal_id: "p1".to_string(),
            resource_kind: kura_secrets::ResourceKind::TenantSecret,
            action: kura_secrets::AuditAction::SecretUse,
            resource_id: String::new(),
            secret_refs: vec!["secret/abc".to_string()],
        }
    });
    assert_eq!(credential.event_kind, CREDENTIAL_EVENT_KIND);
    assert_eq!(
        credential.document.as_ref().unwrap()["resourceKind"],
        "tenant_secret"
    );
    assert_eq!(credential.document.as_ref().unwrap()["secretRefCount"], 1);

    let diag = build_integration_diagnostic_audit_event(&IntegrationDiagnosticAuditInput {
        tenant_id: "t1".to_string(),
        principal_id: "p1".to_string(),
        action: "diagnose".to_string(),
        reason_code: kura_integrations::DiagnosticReasonCode::Healthy,
        redaction_status: kura_integrations::RedactionStatus::Redacted,
        ..IntegrationDiagnosticAuditInput::default()
    });
    assert_eq!(diag.event_kind, INTEGRATION_DIAGNOSTIC_AUDIT_EVENT_KIND);
    assert_eq!(
        diag.document.as_ref().unwrap()["redactionStatus"],
        "redacted"
    );
}
