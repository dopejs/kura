//! Audit event builders for domain-specific audit trails (billing, credential, integration
//! diagnostics). These build a `TenantAuditEvent` that the store's `append_tenant_audit_event`
//! persists. Ported from `daemon/internal/audit/{billing,credential,integration_diagnostics}.go`.
//! The evaluation-product and live-validation builders follow once their domain types land.

use chrono::{DateTime, Utc};
use kura_identity::AUDIT_OUTCOME_SUCCEEDED;
use kura_identity::TenantAuditEvent;

pub const BILLING_AUDIT_EVENT_KIND: &str = "billing.audit_recorded";
pub const CREDENTIAL_EVENT_KIND: &str = "credential.audit_recorded";
pub const INTEGRATION_DIAGNOSTIC_AUDIT_EVENT_KIND: &str = "integration_diagnostic.audit_recorded";

fn string_or_default(value: &str, fallback: &str) -> String {
    if value.is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    }
}

fn enum_str<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value)
        .map(|s| s.trim_matches('"').to_string())
        .unwrap_or_default()
}

fn doc() -> serde_json::Map<String, serde_json::Value> {
    serde_json::Map::new()
}

fn now_if_zero(dt: DateTime<Utc>) -> DateTime<Utc> {
    if dt.timestamp() == 0 && dt.timestamp_subsec_nanos() == 0 {
        Utc::now()
    } else {
        dt
    }
}

#[derive(Debug, Clone, Default)]
pub struct BillingAuditInput {
    pub tenant_id: String,
    pub principal_id: String,
    pub category: kura_billing::Category,
    pub operation_key: String,
    pub reservation_id: String,
    pub adjustment_id: String,
    pub action: String,
    pub outcome: String,
    pub reason_code: String,
    pub reason: String,
    pub amount: i64,
    pub remaining_amount: i64,
    pub created_at: DateTime<Utc>,
}

pub fn build_billing_audit_event(input: &BillingAuditInput) -> TenantAuditEvent {
    let created_at = now_if_zero(input.created_at);
    let outcome = string_or_default(&input.outcome, AUDIT_OUTCOME_SUCCEEDED);
    let mut document = doc();
    document.insert(
        "action".to_string(),
        serde_json::json!(string_or_default(&input.action, "billing.usage_event")),
    );
    if !input.category.is_empty() {
        document.insert(
            "category".to_string(),
            serde_json::json!(input.category.as_str()),
        );
    }
    if !input.operation_key.is_empty() {
        document.insert(
            "operationKey".to_string(),
            serde_json::json!(input.operation_key),
        );
    }
    if !input.reservation_id.is_empty() {
        document.insert(
            "reservationId".to_string(),
            serde_json::json!(input.reservation_id),
        );
    }
    if !input.adjustment_id.is_empty() {
        document.insert(
            "adjustmentId".to_string(),
            serde_json::json!(input.adjustment_id),
        );
    }
    if !input.reason.is_empty() {
        document.insert("reason".to_string(), serde_json::json!(input.reason));
    }
    if input.amount != 0 {
        document.insert("amount".to_string(), serde_json::json!(input.amount));
    }
    if input.remaining_amount != 0 {
        document.insert(
            "remainingAmount".to_string(),
            serde_json::json!(input.remaining_amount),
        );
    }
    TenantAuditEvent {
        event_kind: BILLING_AUDIT_EVENT_KIND.to_string(),
        tenant_id: input.tenant_id.clone(),
        principal_id: input.principal_id.clone(),
        outcome,
        reason_code: input.reason_code.clone(),
        created_at,
        document: Some(document),
        ..TenantAuditEvent::default()
    }
}

pub fn default_billing_audit_retention_policy(
    tenant_id: &str,
) -> kura_billing::AuditRetentionPolicy {
    kura_billing::AuditRetentionPolicy {
        tenant_id: tenant_id.to_string(),
        retention_mode: "indefinite".to_string(),
        ..kura_billing::AuditRetentionPolicy::default()
    }
}

#[derive(Debug, Clone)]
pub struct CredentialAuditInput {
    pub tenant_id: String,
    pub principal_id: String,
    pub resource_kind: kura_secrets::ResourceKind,
    pub resource_id: String,
    pub action: kura_secrets::AuditAction,
    pub outcome: String,
    pub reason_code: String,
    pub secret_ref: String,
    pub secret_version_id: String,
    pub secret_refs: Vec<String>,
    pub created_at: DateTime<Utc>,
}

pub fn build_credential_audit_event(input: &CredentialAuditInput) -> TenantAuditEvent {
    let created_at = now_if_zero(input.created_at);
    let outcome = string_or_default(&input.outcome, AUDIT_OUTCOME_SUCCEEDED);
    let mut document = doc();
    document.insert(
        "resourceKind".to_string(),
        serde_json::json!(enum_str(&input.resource_kind)),
    );
    document.insert(
        "action".to_string(),
        serde_json::json!(enum_str(&input.action)),
    );
    if !input.resource_id.is_empty() {
        document.insert(
            "resourceId".to_string(),
            serde_json::json!(input.resource_id),
        );
    }
    if !input.secret_ref.is_empty() {
        document.insert("secretRef".to_string(), serde_json::json!(input.secret_ref));
    }
    if !input.secret_version_id.is_empty() {
        document.insert(
            "secretVersionId".to_string(),
            serde_json::json!(input.secret_version_id),
        );
    }
    if !input.secret_refs.is_empty() {
        document.insert(
            "secretRefs".to_string(),
            serde_json::json!(kura_secrets::redact_secret_refs(&input.secret_refs)),
        );
        document.insert(
            "secretRefCount".to_string(),
            serde_json::json!(input.secret_refs.len()),
        );
    }
    TenantAuditEvent {
        event_kind: CREDENTIAL_EVENT_KIND.to_string(),
        tenant_id: input.tenant_id.clone(),
        principal_id: input.principal_id.clone(),
        outcome,
        reason_code: input.reason_code.clone(),
        created_at,
        document: Some(document),
        ..TenantAuditEvent::default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct IntegrationDiagnosticAuditInput {
    pub tenant_id: String,
    pub principal_id: String,
    pub action: String,
    pub target_kind: String,
    pub target_id: String,
    pub outcome: String,
    pub reason_code: kura_integrations::DiagnosticReasonCode,
    pub diagnostic_run_id: String,
    pub smoke_report_id: String,
    pub redaction_status: kura_integrations::RedactionStatus,
    pub created_at: DateTime<Utc>,
}

pub fn build_integration_diagnostic_audit_event(
    input: &IntegrationDiagnosticAuditInput,
) -> TenantAuditEvent {
    let created_at = now_if_zero(input.created_at);
    let outcome = string_or_default(&input.outcome, AUDIT_OUTCOME_SUCCEEDED);
    let mut document = doc();
    document.insert("action".to_string(), serde_json::json!(input.action));
    if !input.target_kind.is_empty() {
        document.insert(
            "targetKind".to_string(),
            serde_json::json!(input.target_kind),
        );
    }
    if !input.target_id.is_empty() {
        document.insert("targetId".to_string(), serde_json::json!(input.target_id));
    }
    if !input.diagnostic_run_id.is_empty() {
        document.insert(
            "diagnosticRunId".to_string(),
            serde_json::json!(input.diagnostic_run_id),
        );
    }
    if !input.smoke_report_id.is_empty() {
        document.insert(
            "smokeReportId".to_string(),
            serde_json::json!(input.smoke_report_id),
        );
    }
    if !input.redaction_status.as_str().is_empty() {
        document.insert(
            "redactionStatus".to_string(),
            serde_json::json!(input.redaction_status.as_str()),
        );
    }
    TenantAuditEvent {
        event_kind: INTEGRATION_DIAGNOSTIC_AUDIT_EVENT_KIND.to_string(),
        tenant_id: input.tenant_id.clone(),
        principal_id: input.principal_id.clone(),
        outcome,
        reason_code: input.reason_code.as_str().to_string(),
        created_at,
        document: Some(document),
        ..TenantAuditEvent::default()
    }
}
