//! Port of `daemon/internal/evaluation/comparison.go`: plane-level comparison
//! between a replay attempt and the expected/baseline summaries.

use chrono::{DateTime, Utc};

use crate::types::{
    ComparisonResult, ComparisonTerminalStatus, CreateComparisonInput, DriftFinding, DriftPlane,
    PlaneSummaries, ReplayAttempt, ReplayAttemptStatus, ReplayCandidate, SourceRef,
};
use crate::util::{first_non_empty, new_id};

/// Go `CompareAttempt`.
#[must_use]
pub fn compare_attempt(
    candidate: &ReplayCandidate,
    baseline: Option<&ReplayAttempt>,
    attempt: &ReplayAttempt,
    input: &CreateComparisonInput,
    now: DateTime<Utc>,
) -> ComparisonResult {
    let mut expected = candidate.expected_comparison.clone().unwrap_or_default();
    let mut baseline_ref = first_non_empty(&[
        &input.baseline_ref,
        &input.baseline_attempt_id,
        &attempt.baseline_attempt_id,
        &candidate.source_id,
    ]);
    if let Some(baseline) = baseline {
        baseline_ref = baseline.attempt_id.clone();
        expected = PlaneSummaries {
            runtime: baseline.runtime_summary.clone(),
            policy: baseline.policy_summary.clone(),
            integration: baseline.integration_summary.clone(),
            delivery: baseline.delivery_summary.clone(),
            evidence: baseline.evidence_summary.clone(),
        };
    }
    let mut result = ComparisonResult {
        comparison_id: new_id("replay_comparison"),
        candidate_id: candidate.candidate_id.clone(),
        baseline_ref,
        attempt_id: attempt.attempt_id.clone(),
        environment_scope: attempt.environment_scope.clone(),
        terminal_status: ComparisonTerminalStatus::Matched,
        runtime_summary: attempt.runtime_summary.clone(),
        policy_summary: attempt.policy_summary.clone(),
        integration_summary: attempt.integration_summary.clone(),
        delivery_summary: attempt.delivery_summary.clone(),
        evidence_summary: attempt.evidence_summary.clone(),
        confidence: "high".to_string(),
        limitations: candidate.limitations.clone(),
        change_window_label: input.change_window_label.clone(),
        generated_at: now,
        ..ComparisonResult::default()
    };
    if attempt.status == ReplayAttemptStatus::Blocked {
        result.terminal_status = ComparisonTerminalStatus::Blocked;
        result.confidence = "medium".to_string();
        result.limitations.extend(attempt.blocked_reasons.clone());
        return result;
    }
    if attempt.status == ReplayAttemptStatus::Unreplayable {
        result.terminal_status = ComparisonTerminalStatus::Unreplayable;
        result.confidence = "low".to_string();
        result.limitations.extend(attempt.blocked_reasons.clone());
        return result;
    }

    result.drift_findings.extend(compare_plane(
        &result.comparison_id,
        DriftPlane::Runtime,
        &expected.runtime,
        &attempt.runtime_summary,
        &attempt.evidence_refs,
        now,
    ));
    result.drift_findings.extend(compare_plane(
        &result.comparison_id,
        DriftPlane::Policy,
        &expected.policy,
        &attempt.policy_summary,
        &attempt.evidence_refs,
        now,
    ));
    result.drift_findings.extend(compare_plane(
        &result.comparison_id,
        DriftPlane::Integration,
        &expected.integration,
        &attempt.integration_summary,
        &attempt.evidence_refs,
        now,
    ));
    result.drift_findings.extend(compare_plane(
        &result.comparison_id,
        DriftPlane::Delivery,
        &expected.delivery,
        &attempt.delivery_summary,
        &attempt.evidence_refs,
        now,
    ));
    result.drift_findings.extend(compare_plane(
        &result.comparison_id,
        DriftPlane::Evidence,
        &expected.evidence,
        &attempt.evidence_summary,
        &attempt.evidence_refs,
        now,
    ));
    if !result.drift_findings.is_empty() {
        result.terminal_status = ComparisonTerminalStatus::Drifted;
        result.confidence = "medium".to_string();
    }
    result
}

/// Go `comparePlane`.
fn compare_plane(
    comparison_id: &str,
    plane: DriftPlane,
    baseline: &str,
    replay: &str,
    refs: &[SourceRef],
    now: DateTime<Utc>,
) -> Vec<DriftFinding> {
    if baseline.is_empty() || replay.is_empty() || baseline == replay {
        return Vec::new();
    }
    vec![DriftFinding {
        finding_id: new_id("drift_finding"),
        comparison_id: comparison_id.to_string(),
        plane,
        severity: "warning".to_string(),
        summary: format!("{} summary changed", plane.as_str()),
        baseline_value: baseline.to_string(),
        replay_value: replay.to_string(),
        evidence_refs: refs.to_vec(),
        recommended_action:
            "Inspect authoritative replay evidence before treating this drift as expected."
                .to_string(),
        created_at: now,
        ..DriftFinding::default()
    }]
}
