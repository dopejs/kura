//! Port of `daemon/internal/evaluation/suppression.go`: tenant-scoped
//! suppression records, active lookups, revocation, and candidate filtering.

use chrono::{DateTime, Utc};

use crate::campaign::is_zero_time;
use crate::error::EvaluationError;
use crate::product_validation::validate_tenant_scoped_product_request;
use crate::types::{DiscoveredCandidate, ProductResourceKind, SuppressionRecord};

/// Go `CreateSuppressionInput`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CreateSuppressionInput {
    pub suppression_id: String,
    pub tenant_id: String,
    pub target_kind: ProductResourceKind,
    pub target_id: String,
    pub target_source_ref: String,
    pub reason_code: String,
    pub reason: String,
    pub created_by: String,
    pub expires_at: Option<DateTime<Utc>>,
}

/// Go `NewSuppressionRecord`.
pub fn new_suppression_record(
    input: CreateSuppressionInput,
    now: DateTime<Utc>,
) -> Result<SuppressionRecord, EvaluationError> {
    validate_tenant_scoped_product_request(&input.tenant_id)?;
    if input.target_kind.as_str().is_empty()
        || (input.target_id.trim().is_empty() && input.target_source_ref.trim().is_empty())
    {
        return Err(EvaluationError::ProductSuppressionTargetRequired);
    }
    let now = if is_zero_time(now) { Utc::now() } else { now };
    let suppression_id = {
        let trimmed = input.suppression_id.trim().to_string();
        if trimmed.is_empty() {
            let target = {
                let id = input.target_id.trim().to_string();
                if id.is_empty() {
                    input
                        .target_source_ref
                        .trim()
                        .replace(':', "_")
                        .replace('/', "_")
                } else {
                    id
                }
            };
            format!("suppression_{}_{}", input.target_kind.as_str(), target)
        } else {
            trimmed
        }
    };
    let reason_code = {
        let trimmed = input.reason_code.trim().to_string();
        if trimmed.is_empty() {
            "operator_hidden".to_string()
        } else {
            trimmed
        }
    };
    Ok(SuppressionRecord {
        suppression_id,
        tenant_id: input.tenant_id.trim().to_string(),
        target_kind: input.target_kind,
        target_id: input.target_id.trim().to_string(),
        target_source_ref: input.target_source_ref.trim().to_string(),
        reason_code,
        reason: input.reason.trim().to_string(),
        created_by: input.created_by.trim().to_string(),
        created_at: now,
        expires_at: input.expires_at,
        active: true,
        ..SuppressionRecord::default()
    })
}

/// Go `FindActiveSuppression`.
#[must_use]
pub fn find_active_suppression(
    records: &[SuppressionRecord],
    tenant_id: &str,
    suppression_id: &str,
    now: DateTime<Utc>,
) -> Option<SuppressionRecord> {
    records
        .iter()
        .find(|record| {
            record.suppression_id == suppression_id
                && active_suppression_for_tenant(record, tenant_id, now)
        })
        .cloned()
}

/// Go `RevokeSuppressionRecord`.
#[must_use]
pub fn revoke_suppression_record(
    mut record: SuppressionRecord,
    revoked_at: DateTime<Utc>,
) -> SuppressionRecord {
    let revoked_at = if is_zero_time(revoked_at) {
        Utc::now()
    } else {
        revoked_at
    };
    record.active = false;
    record.expires_at = Some(revoked_at);
    record
}

/// Go `FilterSuppressedCandidates`.
#[must_use]
pub fn filter_suppressed_candidates(
    candidates: Vec<DiscoveredCandidate>,
    records: &[SuppressionRecord],
    now: DateTime<Utc>,
) -> Vec<DiscoveredCandidate> {
    candidates
        .into_iter()
        .filter(|candidate| !suppression_applies(candidate, records, now))
        .collect()
}

/// Go `SuppressionApplies`.
#[must_use]
pub fn suppression_applies(
    candidate: &DiscoveredCandidate,
    records: &[SuppressionRecord],
    now: DateTime<Utc>,
) -> bool {
    for record in records {
        if !active_suppression_for_tenant(record, &candidate.tenant_id, now) {
            continue;
        }
        if record.target_kind == ProductResourceKind::DiscoveredCandidate
            && record.target_id == candidate.discovered_candidate_id
        {
            return true;
        }
        if !record.target_source_ref.is_empty()
            && record.target_source_ref == candidate_source_ref(candidate)
        {
            return true;
        }
    }
    false
}

fn active_suppression_for_tenant(
    record: &SuppressionRecord,
    tenant_id: &str,
    now: DateTime<Utc>,
) -> bool {
    if !record.active {
        return false;
    }
    if record.tenant_id.trim() != tenant_id.trim() {
        return false;
    }
    if let Some(expires_at) = record.expires_at {
        if expires_at <= now {
            return false;
        }
    }
    true
}

/// Go `candidateSourceRef`.
#[must_use]
pub fn candidate_source_ref(candidate: &DiscoveredCandidate) -> String {
    if candidate.source_kind.as_str().is_empty() || candidate.source_id.is_empty() {
        return String::new();
    }
    format!("{}:{}", candidate.source_kind.as_str(), candidate.source_id)
}
