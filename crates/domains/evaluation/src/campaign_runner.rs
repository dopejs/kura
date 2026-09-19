//! Port of `daemon/internal/evaluation/campaign_runner.go`: the runner plan
//! that couples the per-item launch plans with their attempt groups.

use chrono::{DateTime, Utc};

use crate::campaign::is_zero_time;
use crate::campaign_aggregation::{
    CampaignAttemptAggregationInput, CampaignReplayLaunchPlan, build_campaign_attempt_group,
    build_campaign_replay_launch_plan,
};
use crate::error::EvaluationError;
use crate::types::{CampaignAttemptGroup, CampaignItem, ReplayCampaign};

/// Go `CampaignRunnerInput`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CampaignRunnerInput {
    pub campaign: ReplayCampaign,
    pub items: Vec<CampaignItem>,
    pub live_validation_ids: std::collections::HashMap<String, Vec<String>>,
    pub now: DateTime<Utc>,
}

/// Go `CampaignRunnerPlan`.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CampaignRunnerPlan {
    pub campaign_id: String,
    pub launches: Vec<CampaignReplayLaunchPlan>,
    pub groups: Vec<CampaignAttemptGroup>,
}

/// Go `BuildCampaignRunnerPlan`.
pub fn build_campaign_runner_plan(
    input: CampaignRunnerInput,
) -> Result<CampaignRunnerPlan, EvaluationError> {
    let now = if is_zero_time(input.now) {
        Utc::now()
    } else {
        input.now
    };
    let launches = build_campaign_replay_launch_plan(&input.campaign, &input.items);
    let mut groups = Vec::with_capacity(input.items.len());
    for item in &input.items {
        let group = build_campaign_attempt_group(
            CampaignAttemptAggregationInput {
                campaign_id: input.campaign.campaign_id.clone(),
                campaign_item_id: item.campaign_item_id.clone(),
                tenant_id: input.campaign.tenant_id.clone(),
                replay_attempt_ids: vec![format!("attempt_{}", item.campaign_item_id)],
                live_validation_ids: input
                    .live_validation_ids
                    .get(&item.campaign_item_id)
                    .cloned()
                    .unwrap_or_default(),
                ..CampaignAttemptAggregationInput::default()
            },
            now,
        )?;
        groups.push(group);
    }
    Ok(CampaignRunnerPlan {
        campaign_id: input.campaign.campaign_id.clone(),
        launches,
        groups,
    })
}
