//! Serde round-trip tests for the kura-evaluation wire types: camelCase field
//! names, the `expectedComparisonSummary` rename on ReplayCandidate, and
//! snake_case enum wire values, mirroring the Go JSON tags in
//! daemon/internal/evaluation/types.go.

use chrono::{DateTime, Utc};
use kura_evaluation::{
    ApprovalHandling, CandidateKind, ComparisonResult, ComparisonTerminalStatus, DriftFinding,
    DriftPlane, FixtureDomainClass, PlaneSummaries, ReadinessStatus, RegressionFixture,
    ReplayAttempt, ReplayAttemptStatus, ReplayCandidate, ReplayMode, SafetyScope,
    SideEffectHandling, SourceKind, SourceRef,
};

fn ts(s: &str) -> DateTime<Utc> {
    s.parse::<DateTime<Utc>>().unwrap()
}

#[test]
fn enum_wire_values_are_snake_case() {
    assert_eq!(CandidateKind::CuratedWork.as_str(), "curated_work");
    assert_eq!(CandidateKind::Fixture.as_str(), "fixture");
    assert_eq!(SourceKind::ComputerUse.as_str(), "computer_use");
    assert_eq!(
        ReadinessStatus::PartiallyReplayable.as_str(),
        "partially_replayable"
    );
    assert_eq!(ReplayMode::LiveValidation.as_str(), "live_validation");
    assert_eq!(ReplayAttemptStatus::Unreplayable.as_str(), "unreplayable");
    assert_eq!(
        ApprovalHandling::FreshApprovalRequired.as_str(),
        "fresh_approval_required"
    );
    assert_eq!(SideEffectHandling::EvidenceOnly.as_str(), "evidence_only");
    assert_eq!(ComparisonTerminalStatus::Drifted.as_str(), "drifted");
    assert_eq!(DriftPlane::Integration.as_str(), "integration");
    assert_eq!(FixtureDomainClass::ComputerUse.as_str(), "computer_use");
}

#[test]
fn replay_candidate_roundtrips_camel_case_and_rename() {
    let candidate = ReplayCandidate {
        candidate_id: "cand-1".into(),
        candidate_kind: CandidateKind::CuratedWork,
        display_name: "Example run".into(),
        description: "A curated run".into(),
        source_kind: SourceKind::Run,
        source_id: "run-42".into(),
        source_refs: vec![SourceRef {
            kind: SourceKind::Run,
            id: "run-42".into(),
            route: "traces/run-42.jsonl".into(),
        }],
        tool_classes: vec!["bash".into(), "read".into()],
        environment_scope: "test".into(),
        readiness_status: ReadinessStatus::FullyReplayable,
        readiness_reasons: vec!["all tool calls captured".into()],
        limitations: vec![],
        default_replay_mode: ReplayMode::NonLive,
        fixture_id: String::new(),
        latest_attempt_id: String::new(),
        latest_comparison_id: String::new(),
        expected_comparison: Some(PlaneSummaries {
            runtime: "identical".into(),
            ..Default::default()
        }),
        captured_evidence_refs: vec![],
        created_at: ts("2026-01-15T10:00:00Z"),
        updated_at: ts("2026-01-15T10:05:00Z"),
    };

    let json = serde_json::to_string(&candidate).unwrap();
    for key in [
        "candidateId",
        "candidateKind",
        "displayName",
        "sourceKind",
        "sourceId",
        "sourceRefs",
        "toolClasses",
        "environmentScope",
        "readinessStatus",
        "readinessReasons",
        "defaultReplayMode",
        "expectedComparisonSummary",
        "createdAt",
        "updatedAt",
    ] {
        assert!(json.contains(key), "missing {key} in {json}");
    }
    // The custom Go tag renames the field; neither snake nor plain camelCase leaks.
    assert!(
        !json.contains("expected_comparison"),
        "snake field leaked: {json}"
    );
    assert!(
        !json.contains("\"expectedComparison\""),
        "wrong rename: {json}"
    );
    // Empty optional fields (omitempty) are skipped.
    assert!(!json.contains("fixtureId"));
    assert!(!json.contains("latestAttemptId"));
    assert!(!json.contains("latestComparisonId"));
    assert!(!json.contains("capturedEvidenceRefs"));
    // Snake_case enum wire values.
    assert!(json.contains("\"curated_work\""));
    assert!(json.contains("\"run\""));
    assert!(json.contains("\"fully_replayable\""));
    assert!(json.contains("\"non_live\""));
    assert!(json.contains("\"identical\""));

    let back: ReplayCandidate = serde_json::from_str(&json).unwrap();
    assert_eq!(back, candidate);
}

#[test]
fn replay_candidate_accepts_missing_optionals() {
    let json = r#"{
        "candidateId": "cand-2",
        "candidateKind": "fixture",
        "displayName": "F",
        "sourceKind": "fixture",
        "sourceId": "fx-1",
        "sourceRefs": [],
        "environmentScope": "test",
        "readinessStatus": "blocked",
        "readinessReasons": [],
        "limitations": [],
        "defaultReplayMode": "live_validation",
        "createdAt": "2026-01-15T10:00:00Z",
        "updatedAt": "2026-01-15T10:05:00Z"
    }"#;
    let c: ReplayCandidate = serde_json::from_str(json).unwrap();
    assert_eq!(c.candidate_kind, CandidateKind::Fixture);
    assert_eq!(c.source_kind, SourceKind::Fixture);
    assert_eq!(c.readiness_status, ReadinessStatus::Blocked);
    assert_eq!(c.default_replay_mode, ReplayMode::LiveValidation);
    assert!(c.expected_comparison.is_none());
    assert!(c.tool_classes.is_empty());
    assert!(c.description.is_empty());
}

#[test]
fn replay_attempt_roundtrips_camel_case() {
    let attempt = ReplayAttempt {
        attempt_id: "att-1".into(),
        candidate_id: "cand-1".into(),
        source_refs: vec![],
        environment_scope: "test".into(),
        mode: ReplayMode::LiveValidation,
        status: ReplayAttemptStatus::Completed,
        safety_scope: SafetyScope {
            mode: ReplayMode::NonLive,
            description: "dry run".into(),
        },
        approval_handling: ApprovalHandling::EvidenceOnly,
        side_effect_handling: SideEffectHandling::Blocked,
        launched_by: "ci".into(),
        change_window_label: String::new(),
        baseline_attempt_id: String::new(),
        result_run_id: "run-7".into(),
        result_workflow_id: String::new(),
        evidence_refs: vec![],
        blocked_reasons: vec![],
        runtime_summary: String::new(),
        policy_summary: "no policy drift".into(),
        integration_summary: String::new(),
        delivery_summary: String::new(),
        evidence_summary: String::new(),
        started_at: Some(ts("2026-01-15T11:00:00Z")),
        completed_at: Some(ts("2026-01-15T11:04:00Z")),
        created_at: ts("2026-01-15T10:59:00Z"),
        updated_at: ts("2026-01-15T11:05:00Z"),
    };

    let json = serde_json::to_string(&attempt).unwrap();
    for key in [
        "attemptId",
        "candidateId",
        "environmentScope",
        "safetyScope",
        "approvalHandling",
        "sideEffectHandling",
        "resultRunId",
        "policySummary",
        "blockedReasons",
        "startedAt",
        "completedAt",
        "createdAt",
        "updatedAt",
    ] {
        assert!(json.contains(key), "missing {key} in {json}");
    }
    assert!(json.contains("\"live_validation\""));
    assert!(json.contains("\"completed\""));
    assert!(json.contains("\"evidence_only\""));
    assert!(json.contains("\"blocked\""));

    let back: ReplayAttempt = serde_json::from_str(&json).unwrap();
    assert_eq!(back, attempt);
}

#[test]
fn replay_attempt_omits_empty_optional_fields() {
    let attempt = ReplayAttempt {
        attempt_id: "att-2".into(),
        candidate_id: "cand-1".into(),
        created_at: ts("2026-01-15T10:59:00Z"),
        updated_at: ts("2026-01-15T11:05:00Z"),
        ..Default::default()
    };
    let json = serde_json::to_string(&attempt).unwrap();
    assert!(!json.contains("startedAt"));
    assert!(!json.contains("completedAt"));
    assert!(!json.contains("launchedBy"));
    assert!(!json.contains("resultRunId"));
    assert!(json.contains("\"queued\""));
}

#[test]
fn comparison_result_roundtrips_camel_case() {
    let result = ComparisonResult {
        comparison_id: "cmp-1".into(),
        candidate_id: "cand-1".into(),
        baseline_ref: "baseline-2026-01-01".into(),
        attempt_id: "att-1".into(),
        environment_scope: "test".into(),
        terminal_status: ComparisonTerminalStatus::Drifted,
        runtime_summary: "replay diverged at step 3".into(),
        policy_summary: String::new(),
        integration_summary: String::new(),
        delivery_summary: String::new(),
        evidence_summary: String::new(),
        confidence: "medium".into(),
        limitations: vec!["network variance".into()],
        drift_findings: vec![DriftFinding {
            finding_id: "f-1".into(),
            comparison_id: "cmp-1".into(),
            plane: DriftPlane::Runtime,
            severity: "high".into(),
            summary: "exit code differed".into(),
            baseline_value: "0".into(),
            replay_value: "1".into(),
            evidence_refs: vec![],
            recommended_action: String::new(),
            created_at: ts("2026-01-15T12:00:00Z"),
        }],
        change_window_label: String::new(),
        generated_at: ts("2026-01-15T12:30:00Z"),
    };

    let json = serde_json::to_string(&result).unwrap();
    for key in [
        "comparisonId",
        "candidateId",
        "baselineRef",
        "attemptId",
        "environmentScope",
        "terminalStatus",
        "runtimeSummary",
        "confidence",
        "limitations",
        "driftFindings",
        "generatedAt",
    ] {
        assert!(json.contains(key), "missing {key} in {json}");
    }
    assert!(json.contains("\"drifted\""));
    assert!(json.contains("\"runtime\""));
    assert!(json.contains("\"high\""));

    let back: ComparisonResult = serde_json::from_str(&json).unwrap();
    assert_eq!(back, result);
    assert_eq!(back.drift_findings[0].plane, DriftPlane::Runtime);
}

#[test]
fn regression_fixture_roundtrips_camel_case() {
    let fixture = RegressionFixture {
        fixture_id: "fx-1".into(),
        display_name: "Schedule regression".into(),
        domain_class: FixtureDomainClass::Schedule,
        manifest_path: "fixtures/schedule-1/manifest.json".into(),
        source_refs: vec![],
        captured_evidence_refs: vec![],
        assumptions: vec!["clock is faked".into()],
        limitations: vec![],
        expected_replay_mode: ReplayMode::LiveValidation,
        expected_comparison_summary: PlaneSummaries {
            policy: "identical".into(),
            ..Default::default()
        },
        candidate_id: "cand-1".into(),
        environment_scope: "test".into(),
        created_at: ts("2026-01-15T10:00:00Z"),
        updated_at: ts("2026-01-15T10:05:00Z"),
    };

    let json = serde_json::to_string(&fixture).unwrap();
    for key in [
        "fixtureId",
        "displayName",
        "domainClass",
        "manifestPath",
        "capturedEvidenceRefs",
        "assumptions",
        "expectedReplayMode",
        "expectedComparisonSummary",
        "candidateId",
        "environmentScope",
        "createdAt",
        "updatedAt",
    ] {
        assert!(json.contains(key), "missing {key} in {json}");
    }
    assert!(json.contains("\"schedule\""));
    assert!(json.contains("\"live_validation\""));
    assert!(json.contains("\"identical\""));

    let back: RegressionFixture = serde_json::from_str(&json).unwrap();
    assert_eq!(back, fixture);
    assert_eq!(back.expected_comparison_summary.policy, "identical");
}
