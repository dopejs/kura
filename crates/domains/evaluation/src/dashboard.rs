//! Port of `daemon/internal/evaluation/dashboard.go`: tenant-scoped dashboard
//! projection aggregation and deterministic cursor pagination.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::campaign::is_zero_time;
use crate::error::EvaluationError;
use crate::product_validation::{normalize_product_limit, validate_tenant_scoped_product_request};
use crate::types::{
    CampaignAttemptGroup, DashboardProjection, DiscoveredCandidate, ProductManagedFixture,
    ReplayCampaign, RetentionState,
};

/// Go `DashboardProjectionInput`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DashboardProjectionInput {
    pub projection_id: String,
    pub tenant_id: String,
    pub window_start: DateTime<Utc>,
    pub window_end: DateTime<Utc>,
    pub campaigns: Vec<ReplayCampaign>,
    pub candidates: Vec<DiscoveredCandidate>,
    pub fixtures: Vec<ProductManagedFixture>,
    pub attempt_groups: Vec<CampaignAttemptGroup>,
    pub generated_at: DateTime<Utc>,
}

/// Go `BuildDashboardProjection`.
pub fn build_dashboard_projection(
    input: DashboardProjectionInput,
) -> Result<DashboardProjection, EvaluationError> {
    validate_tenant_scoped_product_request(&input.tenant_id)?;
    let generated_at = if is_zero_time(input.generated_at) {
        Utc::now()
    } else {
        input.generated_at
    };
    let projection_id = {
        let trimmed = input.projection_id.trim().to_string();
        if trimmed.is_empty() {
            format!(
                "dashboard_{}",
                generated_at.timestamp_nanos_opt().unwrap_or_default()
            )
        } else {
            trimmed
        }
    };
    let mut projection = DashboardProjection {
        projection_id,
        tenant_id: input.tenant_id.clone(),
        window_start: input.window_start,
        window_end: input.window_end,
        campaign_status_counts: HashMap::new(),
        drift_summary: HashMap::from([("total".to_string(), 0)]),
        failure_summary: HashMap::from([("total".to_string(), 0)]),
        unsupported_summary: HashMap::from([("total".to_string(), 0)]),
        operator_action_needed_summary: HashMap::from([("total".to_string(), 0)]),
        live_validation_summary: HashMap::from([("linked".to_string(), 0)]),
        candidate_summary: HashMap::new(),
        fixture_summary: HashMap::new(),
        generated_at,
        retention_state: RetentionState::Active,
        ..DashboardProjection::default()
    };
    for campaign in &input.campaigns {
        if campaign.tenant_id == input.tenant_id {
            *projection
                .campaign_status_counts
                .entry(campaign.status.as_str().to_string())
                .or_insert(0) += 1;
        }
    }
    for candidate in &input.candidates {
        if candidate.tenant_id == input.tenant_id {
            *projection
                .candidate_summary
                .entry(candidate.retention_state.as_str().to_string())
                .or_insert(0) += 1;
            *projection
                .candidate_summary
                .entry(candidate.suppression_state.as_str().to_string())
                .or_insert(0) += 1;
        }
    }
    for fixture in &input.fixtures {
        if fixture.tenant_id == input.tenant_id {
            *projection
                .fixture_summary
                .entry(fixture.review_state.as_str().to_string())
                .or_insert(0) += 1;
            *projection
                .fixture_summary
                .entry(fixture.retention_state.as_str().to_string())
                .or_insert(0) += 1;
        }
    }
    for group in &input.attempt_groups {
        if group.tenant_id == input.tenant_id {
            *projection
                .drift_summary
                .entry("total".to_string())
                .or_insert(0) += group.drift_count;
            *projection
                .failure_summary
                .entry("total".to_string())
                .or_insert(0) += group.failure_count;
            *projection
                .unsupported_summary
                .entry("total".to_string())
                .or_insert(0) += group.unsupported_count;
            *projection
                .operator_action_needed_summary
                .entry("total".to_string())
                .or_insert(0) += group.operator_action_needed_count;
            if !group.live_validation_ids.is_empty() {
                *projection
                    .live_validation_summary
                    .entry("linked".to_string())
                    .or_insert(0) += group.live_validation_ids.len() as i64;
            }
        }
    }
    Ok(projection)
}

/// Go `PageDashboardProjections`: newest-first deterministic pagination.
#[must_use]
pub fn page_dashboard_projections(
    mut items: Vec<DashboardProjection>,
    cursor: &str,
    limit: i64,
) -> (Vec<DashboardProjection>, String) {
    items.sort_by(|a, b| {
        b.generated_at
            .cmp(&a.generated_at)
            .then_with(|| b.projection_id.cmp(&a.projection_id))
    });
    if !cursor.is_empty() {
        items.retain(|item| item.projection_id.as_str() < cursor);
    }
    let limit = normalize_product_limit(limit) as usize;
    if items.len() <= limit {
        return (items, String::new());
    }
    let next = items[limit - 1].projection_id.clone();
    (items[..limit].to_vec(), next)
}
