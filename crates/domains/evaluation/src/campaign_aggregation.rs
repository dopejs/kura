//! Port of `daemon/internal/evaluation/campaign_aggregation.go`: per-item
//! attempt-group aggregation and the non-live replay launch plan.

use chrono::{DateTime, Utc};

use crate::campaign::is_zero_time;
use crate::error::EvaluationError;
use crate::product_validation::validate_tenant_scoped_product_request;
use crate::types::{
    CampaignAttemptGroup, CampaignItem, ProductLifecycleStatus, RedactionStatus, ReplayCampaign,
    ReplayMode,
};

/// Go `CampaignAttemptAggregationInput`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CampaignAttemptAggregationInput {
    pub campaign_id: String,
    pub campaign_item_id: String,
    pub tenant_id: String,
    pub replay_attempt_ids: Vec<String>,
    pub comparison_ids: Vec<String>,
    pub live_validation_ids: Vec<String>,
    pub drift_count: i64,
    pub failure_count: i64,
    pub unsupported_count: i64,
    pub operator_action_needed_count: i64,
    pub redaction_status: RedactionStatus,
}

/// Go `CampaignReplayLaunchPlan`.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CampaignReplayLaunchPlan {
    pub campaign_id: String,
    pub campaign_item_id: String,
    pub replay_attempt_ids: Vec<String>,
    pub mode: String,
}

/// Go `BuildCampaignAttemptGroup`.
pub fn build_campaign_attempt_group(
    input: CampaignAttemptAggregationInput,
    now: DateTime<Utc>,
) -> Result<CampaignAttemptGroup, EvaluationError> {
    validate_tenant_scoped_product_request(&input.tenant_id)?;
    if input.campaign_id.is_empty() || input.campaign_item_id.is_empty() {
        return Err(EvaluationError::CampaignSelectionInvalid);
    }
    let now = if is_zero_time(now) { Utc::now() } else { now };
    let status = if input.redaction_status == RedactionStatus::Failed || input.failure_count > 0 {
        ProductLifecycleStatus::Failed
    } else {
        ProductLifecycleStatus::Completed
    };
    let summary = campaign_attempt_summary(&input);
    Ok(CampaignAttemptGroup {
        attempt_group_id: format!(
            "attempt_group_{}_{}",
            input.campaign_id, input.campaign_item_id
        ),
        campaign_id: input.campaign_id,
        campaign_item_id: input.campaign_item_id,
        tenant_id: input.tenant_id,
        replay_attempt_ids: input.replay_attempt_ids,
        comparison_ids: input.comparison_ids,
        live_validation_ids: input.live_validation_ids,
        status,
        drift_count: input.drift_count,
        failure_count: input.failure_count,
        unsupported_count: input.unsupported_count,
        operator_action_needed_count: input.operator_action_needed_count,
        summary,
        created_at: now,
        updated_at: now,
        ..CampaignAttemptGroup::default()
    })
}

/// Go `BuildCampaignReplayLaunchPlan`.
#[must_use]
pub fn build_campaign_replay_launch_plan(
    campaign: &ReplayCampaign,
    items: &[CampaignItem],
) -> Vec<CampaignReplayLaunchPlan> {
    items
        .iter()
        .map(|item| CampaignReplayLaunchPlan {
            campaign_id: campaign.campaign_id.clone(),
            campaign_item_id: item.campaign_item_id.clone(),
            replay_attempt_ids: vec![format!("attempt_{}", item.campaign_item_id)],
            mode: ReplayMode::NonLive.as_str().to_string(),
        })
        .collect()
}

fn campaign_attempt_summary(input: &CampaignAttemptAggregationInput) -> String {
    format!(
        "{} drift, {} failure, {} unsupported, {} operator-action-needed",
        input.drift_count,
        input.failure_count,
        input.unsupported_count,
        input.operator_action_needed_count
    )
}
