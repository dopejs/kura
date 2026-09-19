//! Port of daemon/internal/evaluation/campaign_test.go,
//! campaign_aggregation_test.go, campaign_selection_test.go, and
//! campaign_snapshot_test.go.

mod common;

use std::collections::HashMap;

use chrono::DateTime;
use chrono::Utc;
use kura_evaluation::{CampaignAttemptAggregationInput, CampaignItem};
use kura_evaluation::{
    CampaignRunnerInput, CampaignSourceSelection, CampaignTransition, CreateCampaignInput,
    EvaluationError, ProductLifecycleStatus, ProductResourceKind, ReplayCampaign, ReplayMode,
    RetentionState, SuppressionState, build_campaign_attempt_group,
    build_campaign_replay_launch_plan, build_campaign_runner_plan, campaign_idempotency_scope,
    campaign_item_from_selection, create_replay_campaign, transition_replay_campaign,
};

fn ts(s: &str) -> DateTime<Utc> {
    s.parse::<DateTime<Utc>>().expect("ts")
}

#[test]
fn replay_campaign_lifecycle_transitions() {
    let now = ts("2026-04-29T10:00:00Z");
    let (campaign, items) = create_replay_campaign(
        CreateCampaignInput {
            campaign_id: "campaign_1".to_string(),
            tenant_id: "ten_eval".to_string(),
            display_name: "Release Campaign".to_string(),
            scope_summary: "release smoke".to_string(),
            idempotency_key: "idem_1".to_string(),
            source_selections: vec![CampaignSourceSelection {
                source_type: ProductResourceKind::ProductFixture,
                source_id: "fixture_1".to_string(),
                tenant_id: "ten_eval".to_string(),
                review_state: ProductLifecycleStatus::Approved,
                retention_state: RetentionState::Active,
                suppression_state: SuppressionState::None,
                ..Default::default()
            }],
            ..Default::default()
        },
        now,
    )
    .expect("CreateReplayCampaign");
    assert_eq!(campaign.status, ProductLifecycleStatus::Draft);
    assert_eq!(items.len(), 1);
    assert_eq!(campaign_idempotency_scope(&campaign), "ten_eval:idem_1");

    let campaign = transition_replay_campaign(
        campaign,
        CampaignTransition::Start,
        now + chrono::Duration::minutes(1),
    )
    .expect("start");
    assert_eq!(campaign.status, ProductLifecycleStatus::Running);
    assert!(campaign.started_at.is_some());
    let campaign = transition_replay_campaign(
        campaign,
        CampaignTransition::Complete,
        now + chrono::Duration::minutes(2),
    )
    .expect("complete");
    assert_eq!(campaign.status, ProductLifecycleStatus::Completed);
    assert!(campaign.completed_at.is_some());
    let campaign = transition_replay_campaign(
        campaign,
        CampaignTransition::Publish,
        now + chrono::Duration::minutes(3),
    )
    .expect("publish");
    assert_eq!(campaign.status, ProductLifecycleStatus::Published);
    assert!(campaign.published_at.is_some());
    let err = transition_replay_campaign(
        campaign,
        CampaignTransition::Cancel,
        now + chrono::Duration::minutes(4),
    )
    .expect_err("published cancel must be invalid");
    assert!(matches!(err, EvaluationError::CampaignTransitionInvalid));
}

#[test]
fn campaign_selection_rejects_suppressed_expired_draft_and_cross_tenant_sources() {
    let now = ts("2026-04-29T10:00:00Z");
    let campaign = ReplayCampaign {
        campaign_id: "campaign_selection".to_string(),
        tenant_id: "ten_eval".to_string(),
        ..Default::default()
    };
    let cases = [
        CampaignSourceSelection {
            source_type: ProductResourceKind::DiscoveredCandidate,
            source_id: "candidate_1".to_string(),
            tenant_id: "ten_other".to_string(),
            retention_state: RetentionState::Active,
            ..Default::default()
        },
        CampaignSourceSelection {
            source_type: ProductResourceKind::DiscoveredCandidate,
            source_id: "candidate_1".to_string(),
            tenant_id: "ten_eval".to_string(),
            retention_state: RetentionState::Active,
            suppression_state: SuppressionState::Suppressed,
            ..Default::default()
        },
        CampaignSourceSelection {
            source_type: ProductResourceKind::DiscoveredCandidate,
            source_id: "candidate_1".to_string(),
            tenant_id: "ten_eval".to_string(),
            retention_state: RetentionState::Expired,
            ..Default::default()
        },
        CampaignSourceSelection {
            source_type: ProductResourceKind::ProductFixture,
            source_id: "fixture_1".to_string(),
            tenant_id: "ten_eval".to_string(),
            retention_state: RetentionState::Active,
            review_state: ProductLifecycleStatus::Draft,
            ..Default::default()
        },
    ];
    for selection in cases {
        let err = campaign_item_from_selection(&campaign, &selection, 1, now)
            .expect_err("selection must be rejected");
        assert!(
            matches!(
                err,
                EvaluationError::CampaignSelectionInvalid
                    | EvaluationError::ProductCrossTenantSource
            ),
            "selection {selection:?} err={err:?}"
        );
    }
}

#[test]
fn campaign_source_snapshot_remains_stable_after_source_edit() {
    let now = ts("2026-04-29T10:00:00Z");
    let (campaign, items) = create_replay_campaign(
        CreateCampaignInput {
            campaign_id: "campaign_snapshot".to_string(),
            tenant_id: "ten_eval".to_string(),
            display_name: "Snapshot Campaign".to_string(),
            scope_summary: "snapshot".to_string(),
            source_selections: vec![CampaignSourceSelection {
                source_type: ProductResourceKind::ProductFixture,
                source_id: "fixture_1".to_string(),
                tenant_id: "ten_eval".to_string(),
                source_snapshot: serde_json::json!({
                    "revisionId": "revision_1",
                    "goal": "original"
                })
                .as_object()
                .cloned()
                .expect("snapshot object"),
                review_state: ProductLifecycleStatus::Approved,
                retention_state: RetentionState::Active,
                suppression_state: SuppressionState::None,
                ..Default::default()
            }],
            ..Default::default()
        },
        now,
    )
    .expect("CreateReplayCampaign");
    assert_eq!(
        items[0]
            .source_snapshot
            .get("revisionId")
            .and_then(|v| v.as_str()),
        Some("revision_1"),
        "campaign item snapshot changed"
    );
    assert_eq!(campaign.campaign_id, "campaign_snapshot");
}

#[test]
fn campaign_attempt_group_aggregates_replay_comparison_and_live_validation_signals() {
    let now = ts("2026-04-29T10:00:00Z");
    let group = build_campaign_attempt_group(
        CampaignAttemptAggregationInput {
            tenant_id: "ten_eval".to_string(),
            campaign_id: "campaign_1".to_string(),
            campaign_item_id: "campaign_1_item_001".to_string(),
            replay_attempt_ids: vec!["attempt_1".to_string()],
            comparison_ids: vec!["comparison_1".to_string()],
            live_validation_ids: vec!["lv_1".to_string()],
            drift_count: 2,
            failure_count: 0,
            unsupported_count: 1,
            operator_action_needed_count: 1,
            ..Default::default()
        },
        now,
    )
    .expect("BuildCampaignAttemptGroup");
    assert_eq!(group.status, ProductLifecycleStatus::Completed);
    assert_eq!(group.drift_count, 2);
    assert_eq!(group.unsupported_count, 1);
    assert_eq!(group.operator_action_needed_count, 1);
    assert_eq!(group.replay_attempt_ids.len(), 1);
    assert_eq!(group.comparison_ids.len(), 1);
    assert_eq!(group.live_validation_ids.len(), 1);
}

#[test]
fn campaign_replay_launch_plan_uses_non_live_attempts() {
    let plans = build_campaign_replay_launch_plan(
        &ReplayCampaign {
            campaign_id: "campaign_1".to_string(),
            ..Default::default()
        },
        &[CampaignItem {
            campaign_item_id: "item_1".to_string(),
            ..Default::default()
        }],
    );
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].mode, ReplayMode::NonLive.as_str());
    assert_eq!(plans[0].replay_attempt_ids.len(), 1);
}

#[test]
fn build_campaign_runner_plan_launches_non_live_attempts_and_carries_live_validation_links() {
    let now = ts("2026-04-29T10:00:00Z");
    let campaign = ReplayCampaign {
        campaign_id: "campaign_runner".to_string(),
        tenant_id: "ten_runner".to_string(),
        ..Default::default()
    };
    let items = vec![CampaignItem {
        campaign_item_id: "campaign_runner_item_001".to_string(),
        campaign_id: campaign.campaign_id.clone(),
        tenant_id: campaign.tenant_id.clone(),
        ..Default::default()
    }];
    let plan = build_campaign_runner_plan(CampaignRunnerInput {
        campaign,
        items,
        live_validation_ids: HashMap::from([(
            "campaign_runner_item_001".to_string(),
            vec!["ledger_runner".to_string()],
        )]),
        now,
    })
    .expect("BuildCampaignRunnerPlan");
    assert_eq!(plan.launches.len(), 1);
    assert_eq!(plan.launches[0].mode, ReplayMode::NonLive.as_str());
    assert_eq!(plan.groups.len(), 1);
    assert_eq!(
        plan.groups[0].live_validation_ids,
        vec!["ledger_runner".to_string()]
    );
}
