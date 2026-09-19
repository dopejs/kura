//! Trait-surface tests for `kura_evaluation::Store` implemented by
//! `EvaluationStoreHandle` (the Send + Sync newtype over the SQLite store).
//! The underlying DAOs are covered by tests/evaluation.rs; here we exercise
//! the exact trait methods an evaluation manager would call through the
//! handle, including the dynamic filters.

use std::sync::Arc;

use chrono::Utc;
use kura_evaluation::{
    AttemptFilter, CandidateFilter, CandidateKind, ComparisonFilter, ComparisonResult,
    ComparisonTerminalStatus, FixtureDomainClass, FixtureFilter, ReadinessStatus,
    RegressionFixture, ReplayAttempt, ReplayAttemptStatus, ReplayCandidate, ReplayMode, SourceKind,
    Store,
};
use kura_store::{EvaluationStoreHandle, SQLiteStore};

fn temp_dir(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!(
        "kura_store_eval_trait_{name}_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().to_string()
}

fn make_candidate(id: &str, scope: &str) -> ReplayCandidate {
    let now = Utc::now();
    ReplayCandidate {
        candidate_id: id.to_string(),
        candidate_kind: CandidateKind::CuratedWork,
        display_name: "Trait candidate".to_string(),
        description: String::new(),
        source_kind: SourceKind::Run,
        source_id: format!("run_{id}"),
        source_refs: Vec::new(),
        tool_classes: vec!["shell".to_string()],
        environment_scope: scope.to_string(),
        readiness_status: ReadinessStatus::FullyReplayable,
        readiness_reasons: Vec::new(),
        limitations: Vec::new(),
        default_replay_mode: ReplayMode::NonLive,
        fixture_id: String::new(),
        latest_attempt_id: String::new(),
        latest_comparison_id: String::new(),
        expected_comparison: None,
        captured_evidence_refs: Vec::new(),
        created_at: now,
        updated_at: now,
    }
}

fn make_attempt(id: &str, candidate_id: &str, scope: &str) -> ReplayAttempt {
    let now = Utc::now();
    ReplayAttempt {
        attempt_id: id.to_string(),
        candidate_id: candidate_id.to_string(),
        source_refs: Vec::new(),
        environment_scope: scope.to_string(),
        mode: ReplayMode::NonLive,
        status: ReplayAttemptStatus::Completed,
        safety_scope: kura_evaluation::SafetyScope::default(),
        approval_handling: kura_evaluation::ApprovalHandling::EvidenceOnly,
        side_effect_handling: kura_evaluation::SideEffectHandling::EvidenceOnly,
        launched_by: "trait_user".to_string(),
        change_window_label: "trait-release".to_string(),
        baseline_attempt_id: String::new(),
        result_run_id: String::new(),
        result_workflow_id: String::new(),
        evidence_refs: Vec::new(),
        blocked_reasons: Vec::new(),
        runtime_summary: "ran cleanly".to_string(),
        policy_summary: String::new(),
        integration_summary: String::new(),
        delivery_summary: String::new(),
        evidence_summary: String::new(),
        started_at: Some(now),
        completed_at: Some(now),
        created_at: now,
        updated_at: now,
    }
}

fn make_comparison(id: &str, attempt_id: &str, scope: &str) -> ComparisonResult {
    ComparisonResult {
        comparison_id: id.to_string(),
        candidate_id: "cand_1".to_string(),
        baseline_ref: "baseline".to_string(),
        attempt_id: attempt_id.to_string(),
        environment_scope: scope.to_string(),
        terminal_status: ComparisonTerminalStatus::Matched,
        runtime_summary: "same".to_string(),
        policy_summary: String::new(),
        integration_summary: String::new(),
        delivery_summary: String::new(),
        evidence_summary: String::new(),
        confidence: "high".to_string(),
        limitations: Vec::new(),
        drift_findings: Vec::new(),
        change_window_label: String::new(),
        generated_at: Utc::now(),
    }
}

fn make_fixture(id: &str, scope: &str) -> RegressionFixture {
    let now = Utc::now();
    RegressionFixture {
        fixture_id: id.to_string(),
        display_name: "Trait fixture".to_string(),
        domain_class: FixtureDomainClass::Integration,
        manifest_path: format!("fixtures/{id}.json"),
        source_refs: Vec::new(),
        captured_evidence_refs: Vec::new(),
        assumptions: vec!["assumption".to_string()],
        limitations: Vec::new(),
        expected_replay_mode: ReplayMode::NonLive,
        expected_comparison_summary: kura_evaluation::PlaneSummaries::default(),
        candidate_id: String::new(),
        environment_scope: scope.to_string(),
        created_at: now,
        updated_at: now,
    }
}

#[test]
fn evaluation_store_trait_replay_candidate_round_trip() {
    let dir = temp_dir("candidate");
    let store = SQLiteStore::new(&dir).unwrap();
    let handle = Arc::new(EvaluationStoreHandle::new(store));

    let mut candidate = make_candidate("cand_1", "test");
    handle.upsert_replay_candidate(candidate.clone()).unwrap();
    candidate.latest_attempt_id = "att_1".to_string();
    handle.upsert_replay_candidate(candidate).unwrap();

    let got = handle
        .get_replay_candidate("test", "cand_1")
        .unwrap()
        .expect("candidate");
    assert_eq!(got.candidate_id, "cand_1");
    assert_eq!(got.readiness_status, ReadinessStatus::FullyReplayable);
    assert_eq!(got.latest_attempt_id, "att_1");
    assert_eq!(handle.get_replay_candidate("prod", "cand_1").unwrap(), None);

    let all = handle
        .list_replay_candidates(&CandidateFilter::default())
        .unwrap();
    assert_eq!(all.len(), 1);
    let source_filter = CandidateFilter {
        source_kind: SourceKind::Run,
        ..Default::default()
    };
    assert_eq!(
        handle.list_replay_candidates(&source_filter).unwrap().len(),
        1
    );
    let miss = CandidateFilter {
        candidate_kind: CandidateKind::Fixture,
        ..Default::default()
    };
    assert!(handle.list_replay_candidates(&miss).unwrap().is_empty());
}

#[test]
fn evaluation_store_trait_replay_attempt_round_trip() {
    let dir = temp_dir("attempt");
    let store = SQLiteStore::new(&dir).unwrap();
    let handle = Arc::new(EvaluationStoreHandle::new(store));

    handle
        .upsert_replay_candidate(make_candidate("cand_1", "test"))
        .unwrap();
    let mut attempt = make_attempt("att_1", "cand_1", "test");
    handle.upsert_replay_attempt(attempt.clone()).unwrap();
    attempt.status = ReplayAttemptStatus::Running;
    handle.upsert_replay_attempt(attempt).unwrap();

    let got = handle
        .get_replay_attempt("test", "att_1")
        .unwrap()
        .expect("attempt");
    assert_eq!(got.attempt_id, "att_1");
    assert_eq!(got.candidate_id, "cand_1");
    assert_eq!(got.status, ReplayAttemptStatus::Running);
    assert_eq!(got.runtime_summary, "ran cleanly");
    assert_eq!(handle.get_replay_attempt("prod", "att_1").unwrap(), None);

    let by_candidate = AttemptFilter {
        candidate_id: "cand_1".to_string(),
        ..Default::default()
    };
    assert_eq!(handle.list_replay_attempts(&by_candidate).unwrap().len(), 1);
    let by_status = AttemptFilter {
        status: ReplayAttemptStatus::Cancelled,
        ..Default::default()
    };
    assert!(handle.list_replay_attempts(&by_status).unwrap().is_empty());
}

#[test]
fn evaluation_store_trait_comparison_and_fixture_round_trip() {
    let dir = temp_dir("comparison");
    let store = SQLiteStore::new(&dir).unwrap();
    let handle = Arc::new(EvaluationStoreHandle::new(store));

    handle
        .upsert_replay_candidate(make_candidate("cand_1", "test"))
        .unwrap();
    handle
        .upsert_replay_attempt(make_attempt("att_1", "cand_1", "test"))
        .unwrap();
    let mut comparison = make_comparison("cmp_1", "att_1", "test");
    handle.upsert_comparison_result(comparison.clone()).unwrap();
    comparison.terminal_status = ComparisonTerminalStatus::Drifted;
    handle.upsert_comparison_result(comparison).unwrap();

    let got = handle
        .get_comparison_result("test", "cmp_1")
        .unwrap()
        .expect("comparison");
    assert_eq!(got.terminal_status, ComparisonTerminalStatus::Drifted);
    let by_attempt = ComparisonFilter {
        attempt_id: "att_1".to_string(),
        ..Default::default()
    };
    assert_eq!(
        handle.list_comparison_results(&by_attempt).unwrap().len(),
        1
    );

    let mut fixture = make_fixture("fix_1", "test");
    handle.upsert_regression_fixture(fixture.clone()).unwrap();
    fixture.manifest_path = "fixtures/fix_1_v2.json".to_string();
    handle.upsert_regression_fixture(fixture).unwrap();
    let by_domain = FixtureFilter {
        domain_class: FixtureDomainClass::Integration,
        ..Default::default()
    };
    let fixtures = handle.list_regression_fixtures(&by_domain).unwrap();
    assert_eq!(fixtures.len(), 1);
    assert_eq!(fixtures[0].manifest_path, "fixtures/fix_1_v2.json");
}
