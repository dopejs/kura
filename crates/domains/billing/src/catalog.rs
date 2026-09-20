//! Quota catalog: the built-in category definitions and their accounting
//! contract metadata (port of `catalog.go`).

use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;

use crate::types::Category;
use crate::types::PeriodKind;
use crate::types::QuotaDefinition;
use crate::types::Unit;
use crate::types::go_zero_time;
use crate::types::is_false;

pub const PERIOD_ANCHOR_UTC: &str = "UTC";

pub const REASON_QUOTA_STATE_UNAVAILABLE: &str = "quota_denied:quota_state_unavailable";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEntry {
    pub definition: QuotaDefinition,
    pub operation_key_shape: String,
    pub concurrency_guard: String,
    pub required_tests: Vec<String>,
    pub reservation_point: String,
    pub commit_point: String,
    pub refund_point: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub over_limit_commit: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub future_denial_on_over: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogExport {
    pub categories: Vec<CatalogEntry>,
}

#[must_use]
pub fn export_catalog(now: DateTime<Utc>) -> CatalogExport {
    CatalogExport {
        categories: initial_catalog(now),
    }
}

#[must_use]
pub fn initial_catalog(now: DateTime<Utc>) -> Vec<CatalogEntry> {
    let now = if now == go_zero_time() {
        Utc::now()
    } else {
        now
    };
    let common_tests: &[&str] = &["allowed", "denied", "retry", "restart_pending"];
    let tests = |extra: &[&str]| -> Vec<String> {
        common_tests
            .iter()
            .chain(extra.iter())
            .map(|item| (*item).to_string())
            .collect()
    };
    let mut artifact = catalog_entry(
        Category::ARTIFACT_STORAGE_BYTES,
        Unit::BYTES,
        PeriodKind::MONTHLY,
        0,
        "artifact write service before writing bytes using estimate",
        "actual bytes known after write",
        "write failure before consumption or smaller actual refund",
        "tenant:{tenantId}:artifact:{artifactId|storageKey|clientKey}",
        "quota_denied:artifact_storage_bytes_exhausted",
        tests(&[
            "actual_smaller_refund",
            "actual_larger_over_limit_commit",
            "future_denial_after_over_limit_commit",
        ]),
        now,
    );
    artifact.over_limit_commit = true;
    artifact.future_denial_on_over = true;
    artifact.definition.document = Some(serde_json::Map::from_iter([(
        "artifactWriteReservationEstimateBytes".to_string(),
        serde_json::Value::from(4096_i64),
    )]));
    vec![
        catalog_entry(
            Category::RUN_LAUNCHES,
            Unit::COUNT,
            PeriodKind::MONTHLY,
            1,
            "POST /v1/runs before runtime.CreateRun",
            "run persisted",
            "route denial or failure before run persisted",
            "tenant:{tenantId}:run:{clientKey|runId}",
            "quota_denied:run_launches_exhausted",
            tests(&["concurrent_last_unit"]),
            now,
        ),
        catalog_entry(
            Category::WORKFLOW_LAUNCHES,
            Unit::COUNT,
            PeriodKind::MONTHLY,
            1,
            "workflow create/start before execution",
            "workflow planned/running",
            "planning/start denial or cancellation before execution",
            "tenant:{tenantId}:workflow:{runId}:{workflowId|clientKey}",
            "quota_denied:workflow_launches_exhausted",
            tests(&["concurrent_start"]),
            now,
        ),
        catalog_entry(
            Category::RUNTIME_TOOL_CALLS,
            Unit::COUNT,
            PeriodKind::DAILY,
            1,
            "tool call creation before invocation",
            "tool call accepted/running/completed",
            "denial, failed creation, cancellation before invocation",
            "tenant:{tenantId}:tool_call:{runId}:{stepId}:{toolCallId|clientKey}",
            "quota_denied:runtime_tool_calls_exhausted",
            tests(&["concurrent_tool_calls"]),
            now,
        ),
        catalog_entry(
            Category::LIVE_VALIDATION_ATTEMPTS,
            Unit::ATTEMPTS,
            PeriodKind::DAILY,
            1,
            "Roadmap 38 live-validation preflight gate",
            "validation starts or no executor is mounted",
            "denial or unsafe preflight failure before live action",
            "tenant:{tenantId}:live_validation:{validationId|clientKey}",
            "quota_denied:live_validation_attempts_exhausted",
            tests(&["fail_closed_unavailable", "no_roadmap_40_executor"]),
            now,
        ),
        catalog_entry(
            Category::INTEGRATION_OPERATIONS,
            Unit::COUNT,
            PeriodKind::MONTHLY,
            1,
            "integration operation handlers before backend operation",
            "operation record persisted after backend attempt",
            "denial or failed preflight before backend attempt",
            "tenant:{tenantId}:integration:{domain}:{operationId|clientKey}",
            "quota_denied:integration_operations_exhausted",
            tests(&["concurrent_operations"]),
            now,
        ),
        artifact,
        catalog_entry(
            Category::REPLAY_EVALUATION_ATTEMPTS,
            Unit::ATTEMPTS,
            PeriodKind::MONTHLY,
            1,
            "replay/evaluation attempt creation before work starts",
            "attempt persisted as accepted/started/completed",
            "denial or preflight unreplayable before attempt consumption",
            "tenant:{tenantId}:evaluation:{candidateId}:{attemptId|clientKey}",
            "quota_denied:replay_evaluation_attempts_exhausted",
            tests(&["concurrent_attempt"]),
            now,
        ),
    ]
}

#[must_use]
pub fn initial_definitions(now: DateTime<Utc>) -> Vec<QuotaDefinition> {
    initial_catalog(now)
        .into_iter()
        .map(|entry| entry.definition)
        .collect()
}

#[must_use]
pub fn definition_for(category: &Category) -> Option<QuotaDefinition> {
    initial_definitions(Utc::now())
        .into_iter()
        .find(|definition| definition.category == *category)
}

#[must_use]
pub fn required_categories() -> Vec<Category> {
    vec![
        Category::from(Category::RUN_LAUNCHES),
        Category::from(Category::WORKFLOW_LAUNCHES),
        Category::from(Category::RUNTIME_TOOL_CALLS),
        Category::from(Category::LIVE_VALIDATION_ATTEMPTS),
        Category::from(Category::INTEGRATION_OPERATIONS),
        Category::from(Category::ARTIFACT_STORAGE_BYTES),
        Category::from(Category::REPLAY_EVALUATION_ATTEMPTS),
    ]
}

#[allow(clippy::too_many_arguments)]
fn catalog_entry(
    category: &'static str,
    unit: &'static str,
    period: &'static str,
    default_limit: i64,
    reservation_point: &str,
    commit_point: &str,
    refund_point: &str,
    operation_key_shape: &str,
    denial_reason: &str,
    tests: Vec<String>,
    now: DateTime<Utc>,
) -> CatalogEntry {
    CatalogEntry {
        definition: QuotaDefinition {
            quota_definition_id: format!("quota_def_{category}"),
            category: Category::from(category),
            unit: Unit::from(unit),
            period_kind: PeriodKind::from(period),
            period_anchor: PERIOD_ANCHOR_UTC.to_string(),
            default_limit,
            reservation_rule: reservation_point.to_string(),
            commit_rule: commit_point.to_string(),
            refund_rule: refund_point.to_string(),
            denial_reason_code: denial_reason.to_string(),
            active: true,
            created_at: now,
            updated_at: now,
            ..Default::default()
        },
        operation_key_shape: operation_key_shape.to_string(),
        concurrency_guard:
            "single durable transaction over tenant/category/period counter and reservation row"
                .to_string(),
        required_tests: tests,
        reservation_point: reservation_point.to_string(),
        commit_point: commit_point.to_string(),
        refund_point: refund_point.to_string(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::fixed_now;

    #[test]
    fn initial_catalog_covers_required_categories() {
        let entries = initial_catalog(fixed_now());
        assert_eq!(entries.len(), required_categories().len());
        let mut seen = std::collections::HashMap::new();
        for entry in &entries {
            assert!(
                !entry.definition.category.is_empty(),
                "catalog entry missing category"
            );
            assert_eq!(entry.definition.period_anchor, PERIOD_ANCHOR_UTC);
            assert!(
                !entry.definition.denial_reason_code.is_empty(),
                "{} missing denial reason",
                entry.definition.category
            );
            assert!(
                !entry.operation_key_shape.is_empty()
                    && !entry.concurrency_guard.is_empty()
                    && !entry.required_tests.is_empty(),
                "{} missing contract metadata",
                entry.definition.category
            );
            seen.insert(entry.definition.category.clone(), entry);
        }
        for category in required_categories() {
            assert!(seen.contains_key(&category), "missing category {category}");
        }
        let artifact = seen[&Category::from(Category::ARTIFACT_STORAGE_BYTES)];
        assert!(
            artifact.over_limit_commit && artifact.future_denial_on_over,
            "artifact storage bytes must encode over-limit commit and future denial"
        );
    }
}
