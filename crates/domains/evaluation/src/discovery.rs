//! Port of `daemon/internal/evaluation/discovery.go`: building bounded
//! discovery runs from a policy, applying run progress, and idempotency
//! scoping.

use chrono::{DateTime, Utc};

use crate::campaign::is_zero_time;
use crate::error::EvaluationError;
use crate::product_validation::validate_discovery_policy;
use crate::types::{DiscoveryPolicy, DiscoveryRun, ProductLifecycleStatus, SourceKind};

/// Go `DiscoveryPartialReasonMaxInspectedRecords` / `...MaxEmittedCandidates`.
pub const DISCOVERY_PARTIAL_REASON_MAX_INSPECTED_RECORDS: &str = "max_inspected_records";
pub const DISCOVERY_PARTIAL_REASON_MAX_EMITTED_CANDIDATES: &str = "max_emitted_candidates";

/// Go `StartDiscoveryRunInput`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StartDiscoveryRunInput {
    pub window_start: DateTime<Utc>,
    pub window_end: DateTime<Utc>,
    pub source_kinds: Vec<SourceKind>,
    pub max_inspected_records: i64,
    pub max_emitted_candidates: i64,
    pub cost_budget: i64,
    pub cursor: String,
    pub started_by: String,
    pub idempotency_key: String,
}

/// Go `DiscoveryProgress`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiscoveryProgress {
    pub inspected_records: i64,
    pub emitted_candidates: i64,
    pub cursor: String,
    pub completed: bool,
    pub failed_reason: String,
}

/// Go `BuildDiscoveryRunFromPolicy`.
pub fn build_discovery_run_from_policy(
    policy: DiscoveryPolicy,
    input: StartDiscoveryRunInput,
    now: DateTime<Utc>,
) -> Result<DiscoveryRun, EvaluationError> {
    let policy = merge_discovery_policy_input(policy, &input);
    validate_discovery_policy(&policy)?;
    let now = if is_zero_time(now) { Utc::now() } else { now };
    let run_id = {
        let scoped = discovery_idempotency_scope(&DiscoveryRun {
            tenant_id: policy.tenant_id.clone(),
            idempotency_key: input.idempotency_key.clone(),
            ..DiscoveryRun::default()
        });
        if input.idempotency_key.trim().is_empty() {
            format!(
                "discovery_run_{}",
                now.timestamp_nanos_opt().unwrap_or_default()
            )
        } else {
            format!("discovery_run_{}", scoped.replace(':', "_"))
        }
    };
    Ok(DiscoveryRun {
        discovery_run_id: run_id,
        tenant_id: policy.tenant_id,
        policy_id: policy.policy_id,
        status: ProductLifecycleStatus::Queued,
        cursor: input.cursor,
        source_kinds: policy.source_kinds,
        window_start: policy.window_start,
        window_end: policy.window_end,
        max_inspected_records: policy.max_inspected_records,
        max_emitted_candidates: policy.max_emitted_candidates,
        cost_budget: policy.cost_budget,
        started_by: input.started_by,
        started_at: now,
        updated_at: now,
        idempotency_key: input.idempotency_key,
        ..DiscoveryRun::default()
    })
}

/// Go `ApplyDiscoveryRunProgress`.
#[must_use]
pub fn apply_discovery_run_progress(
    mut run: DiscoveryRun,
    progress: DiscoveryProgress,
    now: DateTime<Utc>,
) -> DiscoveryRun {
    let now = if is_zero_time(now) { Utc::now() } else { now };
    run.inspected_records += progress.inspected_records;
    run.emitted_candidates += progress.emitted_candidates;
    if !progress.cursor.trim().is_empty() {
        run.cursor = progress.cursor;
    }
    run.updated_at = now;
    if !progress.failed_reason.trim().is_empty() {
        run.status = ProductLifecycleStatus::Failed;
        run.partial_reason = progress.failed_reason;
        run.completed_at = Some(now);
        return run;
    }
    if run.max_inspected_records > 0 && run.inspected_records >= run.max_inspected_records {
        run.status = ProductLifecycleStatus::Partial;
        run.partial_reason = DISCOVERY_PARTIAL_REASON_MAX_INSPECTED_RECORDS.to_string();
        run.completed_at = Some(now);
        return run;
    }
    if run.max_emitted_candidates > 0 && run.emitted_candidates >= run.max_emitted_candidates {
        run.status = ProductLifecycleStatus::Partial;
        run.partial_reason = DISCOVERY_PARTIAL_REASON_MAX_EMITTED_CANDIDATES.to_string();
        run.completed_at = Some(now);
        return run;
    }
    if progress.completed {
        run.status = ProductLifecycleStatus::Completed;
        run.completed_at = Some(now);
    }
    run
}

/// Go `DiscoveryIdempotencyScope`.
#[must_use]
pub fn discovery_idempotency_scope(run: &DiscoveryRun) -> String {
    if run.tenant_id.trim().is_empty() || run.idempotency_key.trim().is_empty() {
        return String::new();
    }
    format!("{}:{}", run.tenant_id.trim(), run.idempotency_key.trim())
}

/// Go `mergeDiscoveryPolicyInput`: explicit input overrides the policy.
fn merge_discovery_policy_input(
    mut policy: DiscoveryPolicy,
    input: &StartDiscoveryRunInput,
) -> DiscoveryPolicy {
    if !is_zero_time(input.window_start) {
        policy.window_start = input.window_start;
    }
    if !is_zero_time(input.window_end) {
        policy.window_end = input.window_end;
    }
    if !input.source_kinds.is_empty() {
        policy.source_kinds = input.source_kinds.clone();
    }
    if input.max_inspected_records > 0 {
        policy.max_inspected_records = input.max_inspected_records;
    }
    if input.max_emitted_candidates > 0 {
        policy.max_emitted_candidates = input.max_emitted_candidates;
    }
    if input.cost_budget > 0 {
        policy.cost_budget = input.cost_budget;
    }
    policy
}
