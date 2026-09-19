//! Port of daemon/internal/evaluation/dashboard_test.go and
//! dashboard_pagination_test.go.

mod common;

use chrono::DateTime;
use chrono::Utc;
use kura_evaluation::{
    CampaignAttemptGroup, DashboardProjectionInput, DiscoveredCandidate, ProductLifecycleStatus,
    ProductManagedFixture, ReplayCampaign, RetentionState, SuppressionState,
    build_dashboard_projection, page_dashboard_projections,
};

fn ts(s: &str) -> DateTime<Utc> {
    s.parse::<DateTime<Utc>>().expect("ts")
}

#[test]
fn build_dashboard_projection_aggregates_tenant_scoped_product_signals() {
    let now = ts("2026-04-29T10:00:00Z");
    let projection = build_dashboard_projection(DashboardProjectionInput {
        projection_id: "projection_eval".to_string(),
        tenant_id: "ten_eval".to_string(),
        window_start: now - chrono::Duration::hours(1),
        window_end: now,
        generated_at: now,
        campaigns: vec![
            ReplayCampaign {
                campaign_id: "campaign_complete".to_string(),
                tenant_id: "ten_eval".to_string(),
                status: ProductLifecycleStatus::Completed,
                ..Default::default()
            },
            ReplayCampaign {
                campaign_id: "campaign_failed".to_string(),
                tenant_id: "ten_eval".to_string(),
                status: ProductLifecycleStatus::Failed,
                ..Default::default()
            },
            ReplayCampaign {
                campaign_id: "campaign_other".to_string(),
                tenant_id: "ten_other".to_string(),
                status: ProductLifecycleStatus::Completed,
                ..Default::default()
            },
        ],
        candidates: vec![
            DiscoveredCandidate {
                tenant_id: "ten_eval".to_string(),
                retention_state: RetentionState::Active,
                suppression_state: SuppressionState::None,
                ..Default::default()
            },
            DiscoveredCandidate {
                tenant_id: "ten_eval".to_string(),
                retention_state: RetentionState::Expired,
                suppression_state: SuppressionState::Suppressed,
                ..Default::default()
            },
            DiscoveredCandidate {
                tenant_id: "ten_other".to_string(),
                retention_state: RetentionState::Active,
                suppression_state: SuppressionState::None,
                ..Default::default()
            },
        ],
        fixtures: vec![
            ProductManagedFixture {
                tenant_id: "ten_eval".to_string(),
                review_state: ProductLifecycleStatus::Approved,
                retention_state: RetentionState::Active,
                ..Default::default()
            },
            ProductManagedFixture {
                tenant_id: "ten_eval".to_string(),
                review_state: ProductLifecycleStatus::Draft,
                retention_state: RetentionState::Expired,
                ..Default::default()
            },
            ProductManagedFixture {
                tenant_id: "ten_other".to_string(),
                review_state: ProductLifecycleStatus::Approved,
                retention_state: RetentionState::Active,
                ..Default::default()
            },
        ],
        attempt_groups: vec![
            CampaignAttemptGroup {
                tenant_id: "ten_eval".to_string(),
                drift_count: 2,
                failure_count: 1,
                unsupported_count: 3,
                operator_action_needed_count: 4,
                live_validation_ids: vec!["ledger_1".to_string(), "ledger_2".to_string()],
                ..Default::default()
            },
            CampaignAttemptGroup {
                tenant_id: "ten_other".to_string(),
                drift_count: 100,
                failure_count: 100,
                unsupported_count: 100,
                live_validation_ids: vec!["ledger_other".to_string()],
                ..Default::default()
            },
        ],
        ..Default::default()
    })
    .expect("BuildDashboardProjection");

    assert_eq!(
        projection.campaign_status_counts[ProductLifecycleStatus::Completed.as_str()],
        1
    );
    assert_eq!(
        projection.campaign_status_counts[ProductLifecycleStatus::Failed.as_str()],
        1
    );
    assert_eq!(projection.drift_summary["total"], 2);
    assert_eq!(projection.failure_summary["total"], 1);
    assert_eq!(projection.unsupported_summary["total"], 3);
    assert_eq!(projection.operator_action_needed_summary["total"], 4);
    assert_eq!(projection.live_validation_summary["linked"], 2);
    assert_eq!(
        projection.candidate_summary[RetentionState::Active.as_str()],
        1
    );
    assert_eq!(
        projection.candidate_summary[SuppressionState::Suppressed.as_str()],
        1
    );
    assert_eq!(
        projection.fixture_summary[ProductLifecycleStatus::Approved.as_str()],
        1
    );
    assert_eq!(
        projection.fixture_summary[RetentionState::Expired.as_str()],
        1
    );
}

#[test]
fn page_dashboard_projections_is_deterministic() {
    let now = ts("2026-04-29T10:00:00Z");
    let items = vec![
        kura_evaluation::DashboardProjection {
            projection_id: "projection_a".to_string(),
            generated_at: now - chrono::Duration::minutes(2),
            ..Default::default()
        },
        kura_evaluation::DashboardProjection {
            projection_id: "projection_c".to_string(),
            generated_at: now,
            ..Default::default()
        },
        kura_evaluation::DashboardProjection {
            projection_id: "projection_b".to_string(),
            generated_at: now - chrono::Duration::minutes(1),
            ..Default::default()
        },
    ];
    let (first, cursor) = page_dashboard_projections(items.clone(), "", 2);
    assert_eq!(first.len(), 2);
    assert_eq!(first[0].projection_id, "projection_c");
    assert_eq!(first[1].projection_id, "projection_b");
    assert_eq!(cursor, "projection_b");
    let (second, next) = page_dashboard_projections(items, &cursor, 2);
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].projection_id, "projection_a");
    assert_eq!(next, "");
}
