//! Port of `daemon/internal/evaluation/discovery_scoring.go`: signal-based
//! scoring of discovered candidates, score bands, and fail-closed redaction
//! defaults.

use chrono::{DateTime, SecondsFormat, Utc};

use crate::campaign::is_zero_time;
use crate::error::EvaluationError;
use crate::product_validation::validate_tenant_scoped_product_request;
use crate::types::{
    DiscoveredCandidate, ReadinessStatus, RedactionStatus, RetentionState, ScoreBand, SourceKind,
    SourceRef, SuppressionState,
};

/// Go `CandidateScoringInput`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CandidateScoringInput {
    pub tenant_id: String,
    pub discovery_run_id: String,
    pub source_kind: SourceKind,
    pub source_id: String,
    pub source_refs: Vec<SourceRef>,
    pub failure_recurrence: i64,
    pub drift_signal: bool,
    pub tool_call_class: String,
    pub live_validation_outcome: String,
    pub workflow_coverage: i64,
    pub operator_relevance: i64,
    pub observed_at: DateTime<Utc>,
    pub redaction_status: RedactionStatus,
    pub readiness_status: ReadinessStatus,
}

/// Go `BuildDiscoveredCandidateFromSignals`.
pub fn build_discovered_candidate_from_signals(
    input: CandidateScoringInput,
    now: DateTime<Utc>,
) -> Result<DiscoveredCandidate, EvaluationError> {
    validate_tenant_scoped_product_request(&input.tenant_id)?;
    if input.discovery_run_id.trim().is_empty()
        || input.source_kind.as_str().is_empty()
        || input.source_id.trim().is_empty()
    {
        return Err(EvaluationError::ProductSourceRequired);
    }
    let now = if is_zero_time(now) { Utc::now() } else { now };
    let score = candidate_discovery_score(&input, now);
    Ok(DiscoveredCandidate {
        discovered_candidate_id: format!(
            "candidate_{}",
            format!("{}_{}", input.source_kind.as_str(), input.source_id.trim()).replace(':', "_")
        ),
        tenant_id: input.tenant_id.trim().to_string(),
        discovery_run_id: input.discovery_run_id.trim().to_string(),
        source_kind: input.source_kind,
        source_id: input.source_id.trim().to_string(),
        source_refs: input.source_refs.clone(),
        score,
        score_band: score_band_for(score),
        explanation_fields: candidate_explanation_fields(&input, now),
        redaction_status: redaction_status_default(input.redaction_status),
        readiness_status: readiness_status_default(input.readiness_status, input.redaction_status),
        suppression_state: SuppressionState::None,
        retention_state: RetentionState::Active,
        created_at: now,
        updated_at: now,
        ..DiscoveredCandidate::default()
    })
}

/// Go `candidateDiscoveryScore`.
#[must_use]
pub fn candidate_discovery_score(input: &CandidateScoringInput, now: DateTime<Utc>) -> f64 {
    let mut score = 0.20;
    score += max_int(input.failure_recurrence, 0).min(5) as f64 * 0.08;
    if input.drift_signal {
        score += 0.20;
    }
    if !input.tool_call_class.trim().is_empty() {
        score += 0.10;
    }
    match input.live_validation_outcome.trim() {
        "operator_action_needed" | "failed" | "denied" | "aborted" => score += 0.15,
        "completed" => score += 0.05,
        _ => {}
    }
    if input.workflow_coverage >= 2 {
        score += 0.10;
    }
    score += max_int(input.operator_relevance, 0).min(3) as f64 * 0.05;
    if !is_zero_time(input.observed_at) {
        let age = now - input.observed_at;
        if age <= chrono::Duration::hours(24) {
            score += 0.10;
        } else if age <= chrono::Duration::hours(7 * 24) {
            score += 0.05;
        }
    }
    if score > 1.0 {
        score = 1.0;
    }
    (score * 100.0).round() / 100.0
}

/// Go `candidateExplanationFields`.
#[must_use]
pub fn candidate_explanation_fields(
    input: &CandidateScoringInput,
    now: DateTime<Utc>,
) -> serde_json::Map<String, serde_json::Value> {
    let mut fields = serde_json::Map::new();
    fields.insert(
        "failureRecurrence".to_string(),
        serde_json::json!(input.failure_recurrence),
    );
    fields.insert(
        "driftSignal".to_string(),
        serde_json::json!(input.drift_signal),
    );
    fields.insert(
        "workflowCoverage".to_string(),
        serde_json::json!(input.workflow_coverage),
    );
    fields.insert(
        "operatorRelevance".to_string(),
        serde_json::json!(input.operator_relevance),
    );
    if !input.tool_call_class.trim().is_empty() {
        fields.insert(
            "toolCallClass".to_string(),
            serde_json::json!(input.tool_call_class.trim()),
        );
    }
    if !input.live_validation_outcome.trim().is_empty() {
        fields.insert(
            "liveValidationOutcome".to_string(),
            serde_json::json!(input.live_validation_outcome.trim()),
        );
    }
    if !is_zero_time(input.observed_at) {
        fields.insert(
            "observedAt".to_string(),
            serde_json::json!(
                input
                    .observed_at
                    .to_rfc3339_opts(SecondsFormat::Nanos, true)
            ),
        );
        fields.insert(
            "ageHours".to_string(),
            serde_json::json!((now - input.observed_at).num_hours()),
        );
    }
    fields
}

/// Go `scoreBandFor`.
#[must_use]
pub fn score_band_for(score: f64) -> ScoreBand {
    if score >= 0.75 {
        ScoreBand::High
    } else if score >= 0.40 {
        ScoreBand::Medium
    } else {
        ScoreBand::Low
    }
}

/// Go `redactionStatusDefault` (shared by fixture/inspection builders).
#[must_use]
pub fn redaction_status_default(status: RedactionStatus) -> RedactionStatus {
    if status.as_str().is_empty() {
        RedactionStatus::Clean
    } else {
        status
    }
}

/// Go `readinessStatusDefault`.
#[must_use]
pub fn readiness_status_default(
    status: ReadinessStatus,
    redaction_status: RedactionStatus,
) -> ReadinessStatus {
    if redaction_status == RedactionStatus::Failed {
        return ReadinessStatus::Blocked;
    }
    if status.as_str().is_empty() {
        ReadinessStatus::FullyReplayable
    } else {
        status
    }
}

#[must_use]
pub(crate) fn max_int(value: i64, floor: i64) -> i64 {
    if value < floor { floor } else { value }
}
