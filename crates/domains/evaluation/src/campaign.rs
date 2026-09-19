//! Port of `daemon/internal/evaluation/campaign.go`: replay campaign creation,
//! source selection, lifecycle transitions, and idempotency scoping.

use chrono::{DateTime, Utc};

use crate::error::EvaluationError;
use crate::product_validation::validate_tenant_scoped_product_request;
use crate::types::{
    CampaignItem, ProductLifecycleStatus, ProductResourceKind, ReplayCampaign, RetentionState,
    SuppressionState,
};

/// Go `CampaignTransition` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CampaignTransition {
    Start,
    Complete,
    Publish,
    Cancel,
    Fail,
}

/// Go `CampaignSourceSelection` (campaign.go).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CampaignSourceSelection {
    pub source_type: ProductResourceKind,
    pub source_id: String,
    pub tenant_id: String,
    pub source_snapshot: serde_json::Map<String, serde_json::Value>,
    pub selection_reason: String,
    pub suppression_state: SuppressionState,
    pub retention_state: RetentionState,
    pub review_state: ProductLifecycleStatus,
}

/// Go `CreateCampaignInput` (campaign.go).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CreateCampaignInput {
    pub campaign_id: String,
    pub tenant_id: String,
    pub display_name: String,
    pub scope_summary: String,
    pub started_by: String,
    pub idempotency_key: String,
    pub source_selections: Vec<CampaignSourceSelection>,
    pub start_immediately: bool,
}

/// Go `CreateReplayCampaign`.
pub fn create_replay_campaign(
    input: CreateCampaignInput,
    now: DateTime<Utc>,
) -> Result<(ReplayCampaign, Vec<CampaignItem>), EvaluationError> {
    validate_tenant_scoped_product_request(&input.tenant_id)?;
    if input.display_name.trim().is_empty() {
        return Err(EvaluationError::CampaignSelectionInvalid);
    }
    let now = if is_zero_time(now) { Utc::now() } else { now };
    let campaign_id = {
        let trimmed = input.campaign_id.trim().to_string();
        if trimmed.is_empty() {
            format!(
                "campaign_{}",
                input.display_name.trim().to_lowercase().replace(' ', "_")
            )
        } else {
            trimmed
        }
    };
    let status = if input.start_immediately {
        ProductLifecycleStatus::Queued
    } else {
        ProductLifecycleStatus::Draft
    };
    let started_at = if input.start_immediately {
        Some(now)
    } else {
        None
    };
    let campaign = ReplayCampaign {
        campaign_id,
        tenant_id: input.tenant_id.trim().to_string(),
        display_name: input.display_name.trim().to_string(),
        status,
        scope_summary: input.scope_summary.trim().to_string(),
        started_by: input.started_by.trim().to_string(),
        created_at: now,
        started_at,
        retention_state: RetentionState::Active,
        idempotency_key: input.idempotency_key.trim().to_string(),
        ..ReplayCampaign::default()
    };
    let mut items = Vec::with_capacity(input.source_selections.len());
    for (idx, selection) in input.source_selections.iter().enumerate() {
        items.push(campaign_item_from_selection(
            &campaign,
            selection,
            idx + 1,
            now,
        )?);
    }
    Ok((campaign, items))
}

/// Go `CampaignItemFromSelection`.
pub fn campaign_item_from_selection(
    campaign: &ReplayCampaign,
    selection: &CampaignSourceSelection,
    ordinal: usize,
    now: DateTime<Utc>,
) -> Result<CampaignItem, EvaluationError> {
    if !selection.tenant_id.trim().is_empty()
        && selection.tenant_id.trim() != campaign.tenant_id.trim()
    {
        return Err(EvaluationError::ProductCrossTenantSource);
    }
    if selection.source_type.as_str().is_empty() || selection.source_id.trim().is_empty() {
        return Err(EvaluationError::CampaignSelectionInvalid);
    }
    if selection.suppression_state == SuppressionState::Suppressed
        || selection.retention_state == RetentionState::Expired
        || selection.retention_state == RetentionState::Deleted
        || selection.retention_state == RetentionState::Tombstone
    {
        return Err(EvaluationError::CampaignSelectionInvalid);
    }
    if selection.source_type == ProductResourceKind::ProductFixture
        && selection.review_state != ProductLifecycleStatus::Approved
    {
        return Err(EvaluationError::CampaignSelectionInvalid);
    }
    let now = if is_zero_time(now) { Utc::now() } else { now };
    Ok(CampaignItem {
        campaign_item_id: format!("{}_item_{:03}", campaign.campaign_id, ordinal),
        campaign_id: campaign.campaign_id.clone(),
        tenant_id: campaign.tenant_id.clone(),
        source_type: selection.source_type,
        source_id: selection.source_id.trim().to_string(),
        source_snapshot: selection.source_snapshot.clone(),
        selection_reason: selection.selection_reason.trim().to_string(),
        suppression_checked_at: now,
        created_at: now,
        ..CampaignItem::default()
    })
}

/// Go `TransitionReplayCampaign`.
pub fn transition_replay_campaign(
    mut campaign: ReplayCampaign,
    transition: CampaignTransition,
    now: DateTime<Utc>,
) -> Result<ReplayCampaign, EvaluationError> {
    let now = if is_zero_time(now) { Utc::now() } else { now };
    match transition {
        CampaignTransition::Start => {
            if campaign.status != ProductLifecycleStatus::Draft
                && campaign.status != ProductLifecycleStatus::Queued
            {
                return Err(EvaluationError::CampaignTransitionInvalid);
            }
            campaign.status = ProductLifecycleStatus::Running;
            campaign.started_at = Some(now);
        }
        CampaignTransition::Complete => {
            if campaign.status != ProductLifecycleStatus::Running {
                return Err(EvaluationError::CampaignTransitionInvalid);
            }
            campaign.status = ProductLifecycleStatus::Completed;
            campaign.completed_at = Some(now);
        }
        CampaignTransition::Publish => {
            if campaign.status != ProductLifecycleStatus::Completed {
                return Err(EvaluationError::CampaignTransitionInvalid);
            }
            campaign.status = ProductLifecycleStatus::Published;
            campaign.published_at = Some(now);
        }
        CampaignTransition::Cancel => {
            if campaign.status != ProductLifecycleStatus::Draft
                && campaign.status != ProductLifecycleStatus::Queued
                && campaign.status != ProductLifecycleStatus::Running
            {
                return Err(EvaluationError::CampaignTransitionInvalid);
            }
            campaign.status = ProductLifecycleStatus::Cancelled;
        }
        CampaignTransition::Fail => {
            if campaign.status != ProductLifecycleStatus::Queued
                && campaign.status != ProductLifecycleStatus::Running
            {
                return Err(EvaluationError::CampaignTransitionInvalid);
            }
            campaign.status = ProductLifecycleStatus::Failed;
            campaign.completed_at = Some(now);
        }
    }
    Ok(campaign)
}

/// Go `CampaignIdempotencyScope`.
#[must_use]
pub fn campaign_idempotency_scope(campaign: &ReplayCampaign) -> String {
    if campaign.tenant_id.trim().is_empty() || campaign.idempotency_key.trim().is_empty() {
        return String::new();
    }
    format!(
        "{}:{}",
        campaign.tenant_id.trim(),
        campaign.idempotency_key.trim()
    )
}

/// Go's zero-value checks treat the zero `time.Time` (`0001-01-01`) as
/// unset; chrono's `DateTime::default()` is the UNIX epoch, so both count as
/// "not set" here.
#[must_use]
pub(crate) fn is_zero_time(value: DateTime<Utc>) -> bool {
    value == crate::util::go_zero_time() || value == DateTime::UNIX_EPOCH
}
