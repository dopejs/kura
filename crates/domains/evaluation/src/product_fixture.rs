//! Port of `daemon/internal/evaluation/product_fixture.go`: product-managed
//! fixture lifecycle — creation from a discovered candidate, revisions, review,
//! suppression, and retention.

use chrono::{DateTime, Utc};

use crate::campaign::is_zero_time;
use crate::discovery_scoring::redaction_status_default;
use crate::error::EvaluationError;
use crate::product_validation::validate_tenant_scoped_product_request;
use crate::types::{
    CandidateEvidence, DiscoveredCandidate, FixtureDomainClass, FixtureRevision,
    ProductLifecycleStatus, ProductManagedFixture, ProductResourceKind, RedactionStatus,
    RetentionState, SuppressionState,
};

/// Go `ProductFixtureInput`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProductFixtureInput {
    pub fixture_id: String,
    pub tenant_id: String,
    pub display_name: String,
    pub domain_class: FixtureDomainClass,
    pub source_candidate: DiscoveredCandidate,
    pub source_evidence: CandidateEvidence,
    pub fixture_payload: serde_json::Map<String, serde_json::Value>,
    pub change_summary: String,
    pub created_by: String,
    pub idempotency_key: String,
}

/// Go `FixtureRevisionInput`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FixtureRevisionInput {
    pub revision_id: String,
    pub fixture_payload: serde_json::Map<String, serde_json::Value>,
    pub content_summary: String,
    pub change_summary: String,
    pub source_evidence_refs: Vec<String>,
    pub redaction_status: RedactionStatus,
    pub created_by: String,
}

/// Go `FixtureReviewDecision`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixtureReviewDecision {
    Approved,
    Rejected,
    NeedsChanges,
}

/// Go `CreateProductFixtureFromCandidate`.
pub fn create_product_fixture_from_candidate(
    input: ProductFixtureInput,
    now: DateTime<Utc>,
) -> Result<(ProductManagedFixture, FixtureRevision), EvaluationError> {
    validate_product_fixture_input(&input)?;
    let now = if is_zero_time(now) { Utc::now() } else { now };
    let fixture_id = {
        let trimmed = input.fixture_id.trim().to_string();
        if trimmed.is_empty() {
            format!(
                "product_fixture_{}",
                input
                    .source_candidate
                    .discovered_candidate_id
                    .trim()
                    .strip_prefix("candidate_")
                    .unwrap_or(&input.source_candidate.discovered_candidate_id)
            )
        } else {
            trimmed
        }
    };
    let revision_id = format!("revision_{fixture_id}_1");
    let revision = FixtureRevision {
        revision_id: revision_id.clone(),
        fixture_id: fixture_id.clone(),
        tenant_id: input.tenant_id.trim().to_string(),
        revision_number: 1,
        content_summary: input.display_name.trim().to_string(),
        fixture_payload: input.fixture_payload.clone(),
        change_summary: input.change_summary.trim().to_string(),
        source_evidence_refs: vec![input.source_evidence.evidence_id.clone()],
        redaction_status: redaction_status_default(input.source_candidate.redaction_status),
        created_by: input.created_by.trim().to_string(),
        created_at: now,
        ..FixtureRevision::default()
    };
    let fixture = ProductManagedFixture {
        fixture_id: fixture_id.clone(),
        tenant_id: input.tenant_id.trim().to_string(),
        display_name: input.display_name.trim().to_string(),
        domain_class: input.domain_class,
        source_kind: ProductResourceKind::DiscoveredCandidate
            .as_str()
            .to_string(),
        source_refs: input.source_candidate.source_refs.clone(),
        source_candidate_id: input.source_candidate.discovered_candidate_id.clone(),
        current_revision_id: revision_id,
        review_state: ProductLifecycleStatus::Draft,
        suppression_state: SuppressionState::None,
        retention_state: RetentionState::Active,
        created_by: input.created_by.trim().to_string(),
        created_at: now,
        updated_at: now,
        ..ProductManagedFixture::default()
    };
    Ok((fixture, revision))
}

/// Go `CreateProductFixtureRevision`.
pub fn create_product_fixture_revision(
    fixture: ProductManagedFixture,
    input: FixtureRevisionInput,
    next_revision_number: i64,
    now: DateTime<Utc>,
) -> Result<(ProductManagedFixture, FixtureRevision), EvaluationError> {
    ensure_product_fixture_editable(&fixture)?;
    if next_revision_number <= 0 {
        return Err(EvaluationError::ProductBoundsInvalid);
    }
    let now = if is_zero_time(now) { Utc::now() } else { now };
    let revision_id = {
        let trimmed = input.revision_id.trim().to_string();
        if trimmed.is_empty() {
            format!("revision_{}_{}", fixture.fixture_id, next_revision_number)
        } else {
            trimmed
        }
    };
    let revision = FixtureRevision {
        revision_id: revision_id.clone(),
        fixture_id: fixture.fixture_id.clone(),
        tenant_id: fixture.tenant_id.clone(),
        revision_number: next_revision_number,
        content_summary: input.content_summary.trim().to_string(),
        fixture_payload: input.fixture_payload.clone(),
        change_summary: input.change_summary.trim().to_string(),
        source_evidence_refs: input.source_evidence_refs.clone(),
        redaction_status: redaction_status_default(input.redaction_status),
        created_by: input.created_by.trim().to_string(),
        created_at: now,
        ..FixtureRevision::default()
    };
    let mut updated = fixture;
    updated.current_revision_id = revision.revision_id.clone();
    updated.review_state = ProductLifecycleStatus::Draft;
    updated.updated_at = now;
    Ok((updated, revision))
}

/// Go `ReviewProductFixture`.
pub fn review_product_fixture(
    mut fixture: ProductManagedFixture,
    revision_id: &str,
    decision: FixtureReviewDecision,
    now: DateTime<Utc>,
) -> Result<ProductManagedFixture, EvaluationError> {
    ensure_product_fixture_editable(&fixture)?;
    if revision_id.trim().is_empty() || revision_id.trim() != fixture.current_revision_id {
        return Err(EvaluationError::ProductFixtureSourceRequired);
    }
    let now = if is_zero_time(now) { Utc::now() } else { now };
    match decision {
        FixtureReviewDecision::Approved => fixture.review_state = ProductLifecycleStatus::Approved,
        FixtureReviewDecision::Rejected => fixture.review_state = ProductLifecycleStatus::Rejected,
        FixtureReviewDecision::NeedsChanges => fixture.review_state = ProductLifecycleStatus::Draft,
    }
    fixture.updated_at = now;
    Ok(fixture)
}

/// Go `SuppressProductFixture`.
pub fn suppress_product_fixture(
    mut fixture: ProductManagedFixture,
    now: DateTime<Utc>,
) -> Result<ProductManagedFixture, EvaluationError> {
    ensure_product_fixture_editable(&fixture)?;
    let now = if is_zero_time(now) { Utc::now() } else { now };
    fixture.suppression_state = SuppressionState::Suppressed;
    fixture.updated_at = now;
    Ok(fixture)
}

/// Go `ApplyProductFixtureRetention`.
pub fn apply_product_fixture_retention(
    mut fixture: ProductManagedFixture,
    state: RetentionState,
    now: DateTime<Utc>,
) -> Result<ProductManagedFixture, EvaluationError> {
    ensure_product_fixture_editable(&fixture)?;
    let now = if is_zero_time(now) { Utc::now() } else { now };
    if !matches!(
        state,
        RetentionState::Active
            | RetentionState::Expired
            | RetentionState::Deleted
            | RetentionState::Tombstone
    ) {
        return Err(EvaluationError::ProductBoundsInvalid);
    }
    fixture.retention_state = state;
    if fixture.retention_state == RetentionState::Deleted
        || fixture.retention_state == RetentionState::Tombstone
    {
        fixture.review_state = ProductLifecycleStatus::Deleted;
    }
    fixture.updated_at = now;
    Ok(fixture)
}

/// Go `EnsureProductFixtureEditable`.
pub fn ensure_product_fixture_editable(
    fixture: &ProductManagedFixture,
) -> Result<(), EvaluationError> {
    if fixture.fixture_id.trim().is_empty() || fixture.tenant_id.trim().is_empty() {
        return Err(EvaluationError::ProductFixtureSourceRequired);
    }
    if fixture.source_kind == "repo_fixture"
        || fixture.source_kind == crate::types::SourceKind::Fixture.as_str()
    {
        return Err(EvaluationError::RepoFixtureImmutable);
    }
    if fixture.retention_state == RetentionState::Deleted
        || fixture.retention_state == RetentionState::Tombstone
        || fixture.review_state == ProductLifecycleStatus::Deleted
    {
        return Err(EvaluationError::ProductFixtureNotEditable);
    }
    Ok(())
}

/// Go `ProductFixtureSelectable`.
pub fn product_fixture_selectable(fixture: &ProductManagedFixture) -> Result<(), EvaluationError> {
    if fixture.review_state != ProductLifecycleStatus::Approved
        || fixture.suppression_state == SuppressionState::Suppressed
        || fixture.retention_state != RetentionState::Active
    {
        return Err(EvaluationError::ProductFixtureNotSelectable);
    }
    Ok(())
}

fn validate_product_fixture_input(input: &ProductFixtureInput) -> Result<(), EvaluationError> {
    validate_tenant_scoped_product_request(&input.tenant_id)?;
    if input.display_name.trim().is_empty() || input.domain_class.as_str().is_empty() {
        return Err(EvaluationError::ProductFixtureSourceRequired);
    }
    if input
        .source_candidate
        .discovered_candidate_id
        .trim()
        .is_empty()
        || input.source_evidence.evidence_id.trim().is_empty()
    {
        return Err(EvaluationError::ProductFixtureSourceRequired);
    }
    if input.source_candidate.tenant_id.trim() != input.tenant_id.trim()
        || input.source_evidence.tenant_id.trim() != input.tenant_id.trim()
    {
        return Err(EvaluationError::ProductCrossTenantSource);
    }
    if input.source_candidate.suppression_state == SuppressionState::Suppressed
        || input.source_candidate.retention_state != RetentionState::Active
        || input.source_candidate.redaction_status == RedactionStatus::Failed
        || !input.source_evidence.materialization_allowed
    {
        return Err(EvaluationError::ProductFixtureNotSelectable);
    }
    Ok(())
}
