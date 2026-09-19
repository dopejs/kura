//! Port of `daemon/internal/evaluation/product_redaction.go`: fail-closed
//! redaction of candidate evidence payloads before persistence.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};

use crate::campaign::is_zero_time;
use crate::error::EvaluationError;
use crate::product_validation::validate_tenant_scoped_product_request;
use crate::types::{CandidateEvidence, RedactionStatus, RetentionState, SourceRef};

/// Go `RedactionPolicy`.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactionPolicy {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sensitive_field_rules: Vec<String>,
}

/// Go `RedactedEvidence`.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactedEvidence {
    pub status: RedactionStatus,
    pub payload: serde_json::Map<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redaction_rules_applied: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sensitive_fields_excluded: Vec<String>,
}

/// Go `CandidateEvidenceInput`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CandidateEvidenceInput {
    pub evidence_id: String,
    pub tenant_id: String,
    pub discovered_candidate_id: String,
    pub source_refs: Vec<SourceRef>,
    pub summary: String,
    pub payload: serde_json::Map<String, serde_json::Value>,
    pub redaction_policy: RedactionPolicy,
    pub now: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

/// Go `FailedClosedRedactedEvidence`.
#[must_use]
pub fn failed_closed_redacted_evidence(reason_code: &str) -> RedactedEvidence {
    let reason_code = if reason_code.trim().is_empty() {
        "evaluation.redaction_failed"
    } else {
        reason_code.trim()
    };
    RedactedEvidence {
        status: RedactionStatus::Failed,
        payload: serde_json::Map::new(),
        redaction_rules_applied: vec!["failed_closed".to_string()],
        sensitive_fields_excluded: vec![reason_code.to_string()],
    }
}

/// Go `RedactEvidencePayload`.
#[must_use]
pub fn redact_evidence_payload(
    payload: &serde_json::Map<String, serde_json::Value>,
    policy: &RedactionPolicy,
) -> RedactedEvidence {
    let mut sensitive = default_sensitive_field_set();
    for field in &policy.sensitive_field_rules {
        if !field.trim().is_empty() {
            sensitive.insert(normalize_sensitive_field(field));
        }
    }
    let (redacted, excluded) = redact_map(payload, &sensitive);
    let status = if excluded.is_empty() {
        RedactionStatus::Clean
    } else {
        RedactionStatus::Redacted
    };
    let mut rules = vec!["default_sensitive_fields".to_string()];
    if !policy.sensitive_field_rules.is_empty() {
        rules.push("tenant_sensitive_fields".to_string());
    }
    RedactedEvidence {
        status,
        payload: redacted,
        redaction_rules_applied: rules,
        sensitive_fields_excluded: excluded,
    }
}

/// Go `CandidateEvidenceFromPayload`.
pub fn candidate_evidence_from_payload(
    input: CandidateEvidenceInput,
) -> Result<CandidateEvidence, EvaluationError> {
    validate_tenant_scoped_product_request(&input.tenant_id)?;
    if input.discovered_candidate_id.trim().is_empty() {
        return Err(EvaluationError::ProductSourceRequired);
    }
    let now = if is_zero_time(input.now) {
        Utc::now()
    } else {
        input.now
    };
    let evidence_id = {
        let trimmed = input.evidence_id.trim().to_string();
        if trimmed.is_empty() {
            format!("evidence_{}", input.discovered_candidate_id.trim())
        } else {
            trimmed
        }
    };
    let redacted = redact_evidence_payload(&input.payload, &input.redaction_policy);
    Ok(CandidateEvidence {
        evidence_id,
        tenant_id: input.tenant_id.trim().to_string(),
        discovered_candidate_id: input.discovered_candidate_id.trim().to_string(),
        source_refs: input.source_refs.clone(),
        summary: input.summary.trim().to_string(),
        redacted_payload: redacted.payload,
        redaction_rules_applied: redacted.redaction_rules_applied.clone(),
        sensitive_fields_excluded: redacted.sensitive_fields_excluded.clone(),
        materialization_allowed: redacted.status != RedactionStatus::Failed,
        retention_state: RetentionState::Active,
        created_at: now,
        expires_at: input.expires_at,
        ..CandidateEvidence::default()
    })
}

fn default_sensitive_field_set() -> BTreeSet<String> {
    let fields = [
        "authorization",
        "access_token",
        "refresh_token",
        "session_token",
        "bearer_token",
        "credential",
        "credentials",
        "secret",
        "secrets",
        "token",
    ];
    fields
        .iter()
        .map(|field| normalize_sensitive_field(field))
        .collect()
}

fn redact_map(
    payload: &serde_json::Map<String, serde_json::Value>,
    sensitive: &BTreeSet<String>,
) -> (serde_json::Map<String, serde_json::Value>, Vec<String>) {
    let mut out = serde_json::Map::new();
    let mut excluded = Vec::new();
    for (key, value) in payload {
        if sensitive.contains(&normalize_sensitive_field(key)) {
            excluded.push(key.clone());
            continue;
        }
        match value {
            serde_json::Value::Object(nested) => {
                let (nested_map, nested_excluded) = redact_map(nested, sensitive);
                out.insert(key.clone(), serde_json::Value::Object(nested_map));
                excluded.extend(nested_excluded);
            }
            serde_json::Value::Array(items) => {
                let (nested, nested_excluded) = redact_slice(items, sensitive);
                out.insert(key.clone(), serde_json::Value::Array(nested));
                excluded.extend(nested_excluded);
            }
            other => {
                out.insert(key.clone(), other.clone());
            }
        }
    }
    (out, excluded)
}

fn redact_slice(
    payload: &[serde_json::Value],
    sensitive: &BTreeSet<String>,
) -> (Vec<serde_json::Value>, Vec<String>) {
    let mut out = Vec::with_capacity(payload.len());
    let mut excluded = Vec::new();
    for value in payload {
        match value {
            serde_json::Value::Object(nested) => {
                let (nested_map, nested_excluded) = redact_map(nested, sensitive);
                out.push(serde_json::Value::Object(nested_map));
                excluded.extend(nested_excluded);
            }
            serde_json::Value::Array(items) => {
                let (nested, nested_excluded) = redact_slice(items, sensitive);
                out.push(serde_json::Value::Array(nested));
                excluded.extend(nested_excluded);
            }
            other => out.push(other.clone()),
        }
    }
    (out, excluded)
}

/// Go `normalizeSensitiveField`: lowercase, strip _ - . and spaces.
#[must_use]
pub fn normalize_sensitive_field(field: &str) -> String {
    field
        .trim()
        .to_lowercase()
        .chars()
        .filter(|ch| *ch != '_' && *ch != '-' && *ch != '.' && *ch != ' ')
        .collect()
}
