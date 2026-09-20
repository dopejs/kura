//! Diagnostics runtime logic (port of diagnostics_domains.go, diagnostics_manager.go,
//! diagnostics_redaction.go, diagnostics_retention.go, and feishu_lark_diagnostics.go): the
//! diagnostic inspection/run manager, redaction, retention records, and the Feishu/Lark
//! diagnostic probe backend.

use std::collections::HashSet;
use std::sync::LazyLock;

use chrono::{DateTime, SecondsFormat, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    AuthState, BackendKind, DIAGNOSTIC_STALE_AFTER, DiagnosticReasonCode, DiagnosticResult,
    DiagnosticRun, DiagnosticRunStatus, FreshnessState, HealthState, IntegrationError, ProbeKind,
    ProbeResult, ProviderDiagnosticEvidence, ReadinessStatus, RedactionStatus, Resource,
    classify_provider_evidence, diagnostic_defaults, diagnostic_freshness, diagnostic_id,
    diagnostic_remediation_hint, diagnostic_retention_expiry,
};

pub const FEISHU_LARK_PROVIDER_KIND: &str = "feishu_lark";

/// Projects the diagnostic reason for a resource: Feishu/Lark uses the readiness vocabulary,
/// probe-capable domains get a limited diagnostic, and everything else is unsupported.
#[must_use]
pub fn diagnostic_projection_reason(resource: &Resource) -> DiagnosticReasonCode {
    let provider_kind = resource.backend_binding.backend_kind.as_str().trim();
    if provider_kind.eq_ignore_ascii_case(FEISHU_LARK_PROVIDER_KIND)
        || provider_kind.to_lowercase().contains("feishu")
        || provider_kind.to_lowercase().contains("lark")
    {
        return readiness_reason(resource);
    }
    if supports_limited_diagnostic(resource) {
        DiagnosticReasonCode::LimitedDiagnostic
    } else {
        DiagnosticReasonCode::UnsupportedDiagnostic
    }
}

#[must_use]
fn supports_limited_diagnostic(resource: &Resource) -> bool {
    if resource.backend_binding.supports_probe_read {
        return true;
    }
    matches!(
        resource.domain_kind.trim().to_lowercase().as_str(),
        "calendar" | "mail" | "reminders" | "delivery"
    )
}

#[must_use]
fn readiness_reason(resource: &Resource) -> DiagnosticReasonCode {
    match resource.readiness_status {
        ReadinessStatus::Healthy => DiagnosticReasonCode::Healthy,
        ReadinessStatus::AuthPending => {
            if resource.auth_state == AuthState::Pending.as_str()
                || resource.auth_state == AuthState::NotStarted.as_str()
            {
                DiagnosticReasonCode::UserAuthorizationMissing
            } else {
                DiagnosticReasonCode::AppAuthorizationMissing
            }
        }
        ReadinessStatus::NotConfigured => DiagnosticReasonCode::AppAuthorizationMissing,
        ReadinessStatus::Unavailable => {
            let auth_reason = match resource.auth_state.as_str() {
                s if s == AuthState::Expired.as_str() => Some(DiagnosticReasonCode::TokenExpired),
                s if s == AuthState::Revoked.as_str() => Some(DiagnosticReasonCode::TokenRevoked),
                s if s == AuthState::NotStarted.as_str() || s == AuthState::Pending.as_str() => {
                    Some(DiagnosticReasonCode::UserAuthorizationMissing)
                }
                _ => None,
            };
            if let Some(reason) = auth_reason {
                return reason;
            }
            if resource.health_state == HealthState::Unavailable.as_str() {
                DiagnosticReasonCode::ProviderUnavailable
            } else {
                DiagnosticReasonCode::UnknownProviderError
            }
        }
        ReadinessStatus::Degraded => {
            let reason = format!(
                "{} {} {}",
                resource.readiness_reason,
                resource.required_operator_action,
                resource.disabled_reason
            )
            .to_lowercase();
            if reason.contains("scope") {
                DiagnosticReasonCode::ScopeMissing
            } else if reason.contains("tenant") && reason.contains("approval") {
                DiagnosticReasonCode::TenantApprovalPending
            } else if reason.contains("rate") {
                DiagnosticReasonCode::RateLimited
            } else if reason.contains("network") {
                DiagnosticReasonCode::NetworkFailed
            } else if reason.contains("refresh") {
                DiagnosticReasonCode::TokenRefreshFailed
            } else if reason.contains("expired") {
                DiagnosticReasonCode::TokenExpired
            } else if reason.contains("revoked") {
                DiagnosticReasonCode::TokenRevoked
            } else if reason.contains("auth") {
                DiagnosticReasonCode::UserAuthorizationMissing
            } else {
                DiagnosticReasonCode::UnknownProviderError
            }
        }
    }
}

// ---- redaction ----

static SECRET_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)bearer\s+[a-z0-9._~+/=-]+",
        r#"(?i)(access|refresh|id)_token["'=:\s]+[a-z0-9._~+/=-]+"#,
        r#"(?i)(app_?secret|client_?secret)["'=:\s]+[a-z0-9._~+/=-]+"#,
        r#"(?i)authorization["'=:\s]+[a-z0-9._~+/=-]+"#,
    ]
    .iter()
    .map(|p| Regex::new(p).expect("valid diagnostic secret pattern"))
    .collect()
});

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticRedactionResult {
    pub status: RedactionStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
}

#[must_use]
pub fn redact_diagnostic_summary(input: &str) -> DiagnosticRedactionResult {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return DiagnosticRedactionResult {
            status: RedactionStatus::Redacted,
            summary: String::new(),
        };
    }
    let mut redacted = trimmed.to_string();
    for pattern in SECRET_PATTERNS.iter() {
        redacted = pattern.replace_all(&redacted, "[REDACTED]").to_string();
    }
    if redacted.contains("[REDACTED]") {
        DiagnosticRedactionResult {
            status: RedactionStatus::Suppressed,
            summary: "diagnostic detail suppressed".to_string(),
        }
    } else {
        DiagnosticRedactionResult {
            status: RedactionStatus::Redacted,
            summary: redacted,
        }
    }
}

#[must_use]
pub fn fail_closed_diagnostic_redaction() -> DiagnosticRedactionResult {
    DiagnosticRedactionResult {
        status: RedactionStatus::FailedClosed,
        summary: "diagnostic detail suppressed".to_string(),
    }
}

// ---- retention ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticRetentionState {
    #[default]
    Active,
    Expired,
    Purged,
}

impl DiagnosticRetentionState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            DiagnosticRetentionState::Active => "active",
            DiagnosticRetentionState::Expired => "expired",
            DiagnosticRetentionState::Purged => "purged",
        }
    }
}

impl std::fmt::Display for DiagnosticRetentionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticRetentionRecord {
    pub retention_record_id: String,
    pub tenant_id: String,
    pub target_kind: String,
    pub target_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub policy_ref: String,
    pub default_expires_at: DateTime<Utc>,
    pub effective_expires_at: DateTime<Utc>,
    pub retention_state: DiagnosticRetentionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[must_use]
pub fn new_diagnostic_retention_record(
    tenant_id: &str,
    target_kind: &str,
    target_id: &str,
    created_at: DateTime<Utc>,
) -> DiagnosticRetentionRecord {
    let created_at = if created_at == DateTime::<Utc>::default() {
        Utc::now()
    } else {
        created_at
    };
    let expires_at = diagnostic_retention_expiry(created_at);
    DiagnosticRetentionRecord {
        retention_record_id: diagnostic_id(
            "diag_retention",
            &[
                tenant_id,
                target_kind,
                target_id,
                &created_at.to_rfc3339_opts(SecondsFormat::Nanos, true),
            ],
        ),
        tenant_id: tenant_id.to_string(),
        target_kind: target_kind.to_string(),
        target_id: target_id.to_string(),
        default_expires_at: expires_at,
        effective_expires_at: expires_at,
        retention_state: DiagnosticRetentionState::Active,
        created_at,
        updated_at: created_at,
        ..DiagnosticRetentionRecord::default()
    }
}

// ---- diagnostic manager ----

pub struct DiagnosticManager;

impl Default for DiagnosticManager {
    fn default() -> Self {
        Self::new()
    }
}

impl DiagnosticManager {
    pub fn new() -> Self {
        DiagnosticManager
    }

    #[must_use]
    pub fn inspect(&self, input: crate::DiagnosticInspectionInput) -> DiagnosticResult {
        let mut now = input.checked_at;
        if now == DateTime::<Utc>::default() {
            now = Utc::now();
        }
        let capability = if input.capability.trim().is_empty() {
            "integration.readiness".to_string()
        } else {
            input.capability.trim().to_string()
        };
        let mut reason = diagnostic_projection_reason(&input.resource);
        if input.force_generic {
            reason = DiagnosticReasonCode::RedactionFailedClosed;
        }
        let (mut status, mut owner, mut retry_safety) = diagnostic_defaults(reason);
        let mut redaction = redact_diagnostic_summary(&input.evidence_text);
        if input.force_generic || redaction.status == RedactionStatus::FailedClosed {
            redaction = fail_closed_diagnostic_redaction();
            reason = DiagnosticReasonCode::RedactionFailedClosed;
            (status, owner, retry_safety) = diagnostic_defaults(reason);
        }
        let ts = now.to_rfc3339_opts(SecondsFormat::Nanos, true);
        let result_id = diagnostic_id(
            "diag_result",
            &[
                &input.resource.tenant_id,
                &input.resource.integration_id,
                &capability,
                &input.run_id,
                &ts,
            ],
        );
        DiagnosticResult {
            diagnostic_result_id: result_id,
            tenant_id: input.resource.tenant_id.trim().to_string(),
            integration_id: input.resource.integration_id.trim().to_string(),
            integration_account_id: input
                .resource
                .account_binding
                .as_ref()
                .map(|b| b.account_key.trim().to_string())
                .unwrap_or_default(),
            domain_kind: input.resource.domain_kind.trim().to_string(),
            provider_kind: provider_kind(&input.resource),
            capability,
            status,
            reason_code: reason,
            remediation_owner: owner,
            remediation_hint: diagnostic_remediation_hint(reason),
            retry_safety,
            checked_at: now,
            stale_after: now + DIAGNOSTIC_STALE_AFTER,
            freshness_state: FreshnessState::Fresh,
            run_id: input.run_id.trim().to_string(),
            redaction_status: redaction.status,
            evidence_summary: redaction.summary,
            retention_expires_at: diagnostic_retention_expiry(now),
            created_at: Some(now),
            updated_at: Some(now),
            ..DiagnosticResult::default()
        }
    }

    #[must_use]
    pub fn create_run(&self, input: crate::DiagnosticRunInput) -> DiagnosticRun {
        let mut now = input.started_at;
        if now == DateTime::<Utc>::default() {
            now = Utc::now();
        }
        let client_key = input.client_key.trim().to_string();
        let run_id = if client_key.is_empty() {
            let ts = now.to_rfc3339_opts(SecondsFormat::Nanos, true);
            diagnostic_id(
                "diag_run",
                &[
                    &input.resource.tenant_id,
                    &input.resource.integration_id,
                    &input.requested_by,
                    &ts,
                ],
            )
        } else {
            format!("diag_run_{client_key}")
        };
        let capabilities = normalize_diagnostic_capabilities(&input.capabilities);
        let trigger = if input.trigger.trim().is_empty() {
            "operator_inspection".to_string()
        } else {
            input.trigger.trim().to_string()
        };
        DiagnosticRun {
            diagnostic_run_id: run_id,
            tenant_id: input.resource.tenant_id.trim().to_string(),
            integration_id: input.resource.integration_id.trim().to_string(),
            integration_account_id: input
                .resource
                .account_binding
                .as_ref()
                .map(|b| b.account_key.trim().to_string())
                .unwrap_or_default(),
            domain_kind: input.resource.domain_kind.trim().to_string(),
            provider_kind: provider_kind(&input.resource),
            requested_by: input.requested_by.trim().to_string(),
            trigger,
            status: DiagnosticRunStatus::Running,
            started_at: now,
            checked_capabilities: capabilities,
            result_ids: Vec::new(),
            redaction_status: RedactionStatus::Redacted,
            retention_expires_at: diagnostic_retention_expiry(now),
            idempotency_key: client_key,
            ..DiagnosticRun::default()
        }
    }
}

#[must_use]
pub fn complete_diagnostic_run(
    mut run: DiagnosticRun,
    results: &[DiagnosticResult],
    completed_at: DateTime<Utc>,
) -> DiagnosticRun {
    let completed_at = if completed_at == DateTime::<Utc>::default() {
        Utc::now()
    } else {
        completed_at
    };
    run.status = DiagnosticRunStatus::Completed;
    run.completed_at = Some(completed_at);
    run.result_ids = Vec::with_capacity(results.len());
    for result in results {
        run.result_ids.push(result.diagnostic_result_id.clone());
        if result.redaction_status == RedactionStatus::FailedClosed {
            run.redaction_status = RedactionStatus::FailedClosed;
            run.failure_reason_code = DiagnosticReasonCode::RedactionFailedClosed
                .as_str()
                .to_string();
        }
    }
    run
}

#[must_use]
pub fn refresh_diagnostic_result_freshness(
    mut result: DiagnosticResult,
    now: DateTime<Utc>,
) -> DiagnosticResult {
    let now = if now == DateTime::<Utc>::default() {
        Utc::now()
    } else {
        now
    };
    result.freshness_state = diagnostic_freshness(now, result.stale_after);
    result
}

#[must_use]
fn normalize_diagnostic_capabilities(capabilities: &[String]) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut normalized = Vec::new();
    for capability in capabilities {
        let c = capability.trim();
        if c.is_empty() || !seen.insert(c.to_string()) {
            continue;
        }
        normalized.push(c.to_string());
    }
    if normalized.is_empty() {
        vec!["integration.readiness".to_string()]
    } else {
        normalized
    }
}

#[must_use]
fn provider_kind(resource: &Resource) -> String {
    let value = resource.backend_binding.backend_kind.as_str().trim();
    if value.is_empty() {
        "unknown".to_string()
    } else {
        value.to_string()
    }
}

// ---- Feishu/Lark diagnostic backend ----

pub struct FeishuLarkDiagnosticBackend;

impl Default for FeishuLarkDiagnosticBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl FeishuLarkDiagnosticBackend {
    pub fn new() -> Self {
        FeishuLarkDiagnosticBackend
    }

    #[must_use]
    pub fn supported_domain_kinds(&self) -> Vec<String> {
        vec![
            "calendar".to_string(),
            "mail".to_string(),
            "reminders".to_string(),
            "delivery".to_string(),
        ]
    }

    pub fn run_probe(
        &self,
        resource: &Resource,
        probe_kind: ProbeKind,
        input: &Map<String, Value>,
    ) -> Result<ProbeResult, IntegrationError> {
        if !self.supports_domain_kind(&resource.domain_kind) {
            return Err(IntegrationError::ProbeUnsupported);
        }
        if probe_kind != ProbeKind::Inspect && probe_kind != ProbeKind::Mutate {
            return Err(IntegrationError::ProbeUnsupported);
        }
        let evidence = feishu_lark_evidence_from_probe(resource, input);
        let classification = classify_provider_evidence(&evidence);
        let status = if classification.reason_code == DiagnosticReasonCode::Healthy {
            "completed"
        } else {
            "failed"
        };
        let mut summary = Map::new();
        summary.insert(
            "integrationId".to_string(),
            Value::String(resource.integration_id.clone()),
        );
        summary.insert(
            "domainKind".to_string(),
            Value::String(resource.domain_kind.clone()),
        );
        summary.insert(
            "backendKind".to_string(),
            Value::String(resource.backend_binding.backend_kind.as_str().to_string()),
        );
        summary.insert(
            "probeKind".to_string(),
            Value::String(probe_kind.as_str().to_string()),
        );
        summary.insert(
            "operationClass".to_string(),
            Value::String(evidence.operation_class.clone()),
        );
        summary.insert(
            "reasonCode".to_string(),
            Value::String(classification.reason_code.as_str().to_string()),
        );
        summary.insert(
            "retrySafety".to_string(),
            Value::String(classification.retry_safety.as_str().to_string()),
        );
        summary.insert(
            "remediationOwner".to_string(),
            Value::String(classification.remediation_owner.as_str().to_string()),
        );
        summary.insert(
            "redactionStatus".to_string(),
            Value::String(classification.redaction_status.as_str().to_string()),
        );
        summary.insert(
            "evidenceConfidence".to_string(),
            Value::String(classification.evidence_confidence.clone()),
        );
        Ok(ProbeResult {
            probe_kind,
            status: status.to_string(),
            failure_class: crate::first_non_empty(&[
                &classification.redacted_provider_code,
                classification.reason_code.as_str(),
            ]),
            result_summary: summary,
            ..ProbeResult::default()
        })
    }

    #[must_use]
    fn supports_domain_kind(&self, domain_kind: &str) -> bool {
        matches!(
            domain_kind.trim().to_lowercase().as_str(),
            "calendar" | "mail" | "reminders" | "delivery"
        )
    }
}

#[must_use]
fn feishu_lark_evidence_from_probe(
    resource: &Resource,
    input: &Map<String, Value>,
) -> ProviderDiagnosticEvidence {
    let mut raw_evidence: Map<String, Value> = Map::new();
    if let Some(Value::Object(nested)) = input.get("providerEvidence") {
        for (key, value) in nested {
            raw_evidence.insert(key.clone(), value.clone());
        }
    }
    for key in ["code", "status", "message"] {
        if let Some(value) = input.get(key) {
            let text = stringify_value(value).trim().to_string();
            if !text.is_empty() && text != "<nil>" {
                raw_evidence.insert(key.to_string(), Value::String(text));
            }
        }
    }
    if raw_evidence.is_empty() && !resource.readiness_reason.trim().is_empty() {
        raw_evidence.insert(
            "message".to_string(),
            Value::String(resource.readiness_reason.clone()),
        );
    }
    let mut evidence = provider_evidence_from_map(
        BackendKind::FeishuLark.as_str(),
        &resource.domain_kind,
        &raw_evidence,
    );
    evidence.integration_id = resource.integration_id.clone();
    evidence.operation_class = input
        .get("operationClass")
        .map(|v| stringify_value(v).trim().to_string())
        .unwrap_or_default();
    if evidence.operation_class.is_empty() || evidence.operation_class == "<nil>" {
        evidence.operation_class = "integration.readiness".to_string();
    }
    evidence.created_at = Utc::now();
    evidence
}

#[must_use]
pub fn provider_evidence_from_map(
    provider_kind: &str,
    domain_kind: &str,
    evidence: &Map<String, Value>,
) -> ProviderDiagnosticEvidence {
    let mut item = ProviderDiagnosticEvidence {
        provider_kind: provider_kind.to_string(),
        domain_kind: domain_kind.to_string(),
        ..ProviderDiagnosticEvidence::default()
    };
    for (key, value) in evidence {
        let text = stringify_value(value).trim().to_string();
        match key.as_str() {
            "code" => item.redacted_provider_code = text,
            "status" | "statusClass" => item.provider_status_code = text,
            "errorClass" => item.provider_error_class = text,
            "message" => {
                item.message.push(' ');
                item.message.push_str(&text);
            }
            "approval" => {
                item.message.push_str(" approval ");
                item.message.push_str(&text);
            }
            "redactionConfidence" => item.redaction_confidence = text,
            "commitAmbiguous" => item.commit_ambiguous = text == "true",
            _ => {}
        }
    }
    item
}

#[must_use]
fn stringify_value(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "<nil>".to_string(),
        other => other.to_string(),
    }
}
