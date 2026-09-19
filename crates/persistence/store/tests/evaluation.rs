//! Round-trip integration tests for the evaluation replay-ledger CRUD methods ported from
//! `daemon/internal/store/store.go` into `evaluation.rs`: replay candidates, replay
//! attempts, comparison results, and regression fixtures. Each test constructs a record,
//! upserts it, upserts again through the ON CONFLICT path with a changed field, then
//! lists/gets it back. Attempts and comparisons reference their parent rows via foreign
//! keys (enforced by `PRAGMA foreign_keys = ON`), so those tests seed the parent records
//! first. Wiring required before these compile: declare `pub mod evaluation;` in
//! `lib.rs` and add `kura-evaluation.workspace = true` to `Cargo.toml`.

use chrono::Utc;
use kura_evaluation::{
    AttemptFilter, CandidateFilter, CandidateKind, ComparisonFilter, ComparisonResult,
    ComparisonTerminalStatus, FixtureDomainClass, FixtureFilter, ReadinessStatus,
    RegressionFixture, ReplayAttempt, ReplayAttemptStatus, ReplayCandidate, ReplayMode, SourceKind,
};
use kura_store::SQLiteStore;

fn temp_dir(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("kura_store_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().to_string()
}

fn upsert_candidate(store: &SQLiteStore, candidate_id: &str, environment_scope: &str) {
    let now = Utc::now();
    let candidate = ReplayCandidate {
        candidate_id: candidate_id.to_string(),
        candidate_kind: CandidateKind::CuratedWork,
        display_name: "Test candidate".to_string(),
        description: String::new(),
        source_kind: SourceKind::Run,
        source_id: format!("run_for_{candidate_id}"),
        source_refs: Vec::new(),
        tool_classes: Vec::new(),
        environment_scope: environment_scope.to_string(),
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
    };
    store.upsert_replay_candidate(&candidate).unwrap();
}

fn upsert_attempt(
    store: &SQLiteStore,
    attempt_id: &str,
    candidate_id: &str,
    environment_scope: &str,
) {
    let now = Utc::now();
    let attempt = ReplayAttempt {
        attempt_id: attempt_id.to_string(),
        candidate_id: candidate_id.to_string(),
        source_refs: Vec::new(),
        environment_scope: environment_scope.to_string(),
        mode: ReplayMode::NonLive,
        status: ReplayAttemptStatus::Completed,
        safety_scope: kura_evaluation::SafetyScope::default(),
        approval_handling: kura_evaluation::ApprovalHandling::Blocked,
        side_effect_handling: kura_evaluation::SideEffectHandling::Blocked,
        launched_by: String::new(),
        change_window_label: String::new(),
        baseline_attempt_id: String::new(),
        result_run_id: String::new(),
        result_workflow_id: String::new(),
        evidence_refs: Vec::new(),
        blocked_reasons: Vec::new(),
        runtime_summary: String::new(),
        policy_summary: String::new(),
        integration_summary: String::new(),
        delivery_summary: String::new(),
        evidence_summary: String::new(),
        started_at: None,
        completed_at: None,
        created_at: now,
        updated_at: now,
    };
    store.upsert_replay_attempt(&attempt).unwrap();
}

#[test]
fn replay_candidate_round_trips_through_sqlite() {
    let dir = temp_dir("eval_candidate");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();

    let mut candidate = ReplayCandidate {
        candidate_id: "cand_1".to_string(),
        candidate_kind: CandidateKind::CuratedWork,
        display_name: "Draft release replay".to_string(),
        description: "Replay the draft-release run".to_string(),
        source_kind: SourceKind::Workflow,
        source_id: "wf_release".to_string(),
        source_refs: Vec::new(),
        tool_classes: vec!["shell".to_string(), "browser".to_string()],
        environment_scope: "test".to_string(),
        readiness_status: ReadinessStatus::PartiallyReplayable,
        readiness_reasons: vec!["missing secrets".to_string()],
        limitations: vec!["no live connectors".to_string()],
        default_replay_mode: ReplayMode::LiveValidation,
        fixture_id: String::new(),
        latest_attempt_id: String::new(),
        latest_comparison_id: String::new(),
        expected_comparison: None,
        captured_evidence_refs: Vec::new(),
        created_at: now,
        updated_at: now,
    };
    store.upsert_replay_candidate(&candidate).unwrap();
    // Upsert again through the ON CONFLICT path with a changed field.
    candidate.readiness_status = ReadinessStatus::FullyReplayable;
    candidate.latest_attempt_id = "att_1".to_string();
    store.upsert_replay_candidate(&candidate).unwrap();

    let listed = store
        .list_replay_candidates(&CandidateFilter::default())
        .unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.candidate_id, "cand_1");
    assert_eq!(got.candidate_kind, CandidateKind::CuratedWork);
    assert_eq!(got.display_name, "Draft release replay");
    assert_eq!(got.source_kind, SourceKind::Workflow);
    assert_eq!(got.source_id, "wf_release");
    assert_eq!(got.environment_scope, "test");
    assert_eq!(got.readiness_status, ReadinessStatus::FullyReplayable);
    assert_eq!(got.readiness_reasons, vec!["missing secrets".to_string()]);
    assert_eq!(got.default_replay_mode, ReplayMode::LiveValidation);
    assert_eq!(got.latest_attempt_id, "att_1");
    assert_eq!(
        got.tool_classes,
        vec!["shell".to_string(), "browser".to_string()]
    );

    // Dynamic filters match and miss.
    let kind_filter = CandidateFilter {
        candidate_kind: CandidateKind::Fixture,
        ..Default::default()
    };
    assert!(
        store
            .list_replay_candidates(&kind_filter)
            .unwrap()
            .is_empty()
    );
    // FullyReplayable is the enum default, so it maps to "unset" (Go's `!= ""` check);
    // only non-default values emit a readiness_status clause.
    let readiness_filter = CandidateFilter {
        readiness_status: ReadinessStatus::Blocked,
        ..Default::default()
    };
    assert!(
        store
            .list_replay_candidates(&readiness_filter)
            .unwrap()
            .is_empty()
    );
    let source_filter = CandidateFilter {
        source_kind: SourceKind::Workflow,
        ..Default::default()
    };
    assert_eq!(
        store.list_replay_candidates(&source_filter).unwrap().len(),
        1
    );
    let limit_filter = CandidateFilter {
        limit: 1,
        ..Default::default()
    };
    assert_eq!(
        store.list_replay_candidates(&limit_filter).unwrap().len(),
        1
    );
    // No rows in a different environment scope.
    let prod_filter = CandidateFilter {
        environment_scope: "prod".to_string(),
        ..Default::default()
    };
    assert!(
        store
            .list_replay_candidates(&prod_filter)
            .unwrap()
            .is_empty()
    );

    let fetched = store
        .get_replay_candidate("test", "cand_1")
        .unwrap()
        .expect("found");
    assert_eq!(fetched.candidate_id, "cand_1");
    assert_eq!(fetched.readiness_status, ReadinessStatus::FullyReplayable);
    // Environment scope narrows the lookup.
    assert_eq!(store.get_replay_candidate("prod", "cand_1").unwrap(), None);
    assert_eq!(store.get_replay_candidate("test", "missing").unwrap(), None);
    // Scope-less lookup still finds the row.
    assert_eq!(
        store
            .get_replay_candidate("", "cand_1")
            .unwrap()
            .map(|c| c.candidate_id),
        Some("cand_1".to_string())
    );
}

#[test]
fn replay_attempt_round_trips_through_sqlite() {
    let dir = temp_dir("eval_attempt");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();

    upsert_candidate(&store, "cand_1", "test");

    let mut attempt = ReplayAttempt {
        attempt_id: "att_1".to_string(),
        candidate_id: "cand_1".to_string(),
        source_refs: Vec::new(),
        environment_scope: "test".to_string(),
        mode: ReplayMode::LiveValidation,
        status: ReplayAttemptStatus::Running,
        safety_scope: kura_evaluation::SafetyScope::default(),
        approval_handling: kura_evaluation::ApprovalHandling::EvidenceOnly,
        side_effect_handling: kura_evaluation::SideEffectHandling::EvidenceOnly,
        launched_by: "user_1".to_string(),
        change_window_label: "release-1".to_string(),
        baseline_attempt_id: String::new(),
        result_run_id: String::new(),
        result_workflow_id: String::new(),
        evidence_refs: Vec::new(),
        blocked_reasons: Vec::new(),
        runtime_summary: "completed cleanly".to_string(),
        policy_summary: String::new(),
        integration_summary: String::new(),
        delivery_summary: String::new(),
        evidence_summary: String::new(),
        started_at: Some(now),
        completed_at: None,
        created_at: now,
        updated_at: now,
    };
    store.upsert_replay_attempt(&attempt).unwrap();
    // Upsert again through the ON CONFLICT path with a changed field.
    attempt.status = ReplayAttemptStatus::Completed;
    attempt.completed_at = Some(now);
    store.upsert_replay_attempt(&attempt).unwrap();

    let listed = store
        .list_replay_attempts(&AttemptFilter::default())
        .unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.attempt_id, "att_1");
    assert_eq!(got.candidate_id, "cand_1");
    assert_eq!(got.environment_scope, "test");
    assert_eq!(got.mode, ReplayMode::LiveValidation);
    assert_eq!(got.status, ReplayAttemptStatus::Completed);
    assert_eq!(got.change_window_label, "release-1");
    assert_eq!(got.launched_by, "user_1");
    assert_eq!(got.runtime_summary, "completed cleanly");
    assert!(got.started_at.is_some());
    assert!(got.completed_at.is_some());

    // Dynamic filters match and miss.
    // Queued is the enum default ("unset"); use a non-default status for a real miss.
    let status_filter = AttemptFilter {
        status: ReplayAttemptStatus::Cancelled,
        ..Default::default()
    };
    assert!(
        store
            .list_replay_attempts(&status_filter)
            .unwrap()
            .is_empty()
    );
    let completed_filter = AttemptFilter {
        status: ReplayAttemptStatus::Completed,
        ..Default::default()
    };
    assert_eq!(
        store.list_replay_attempts(&completed_filter).unwrap().len(),
        1
    );
    let candidate_filter = AttemptFilter {
        candidate_id: "cand_1".to_string(),
        ..Default::default()
    };
    assert_eq!(
        store.list_replay_attempts(&candidate_filter).unwrap().len(),
        1
    );
    let limit_filter = AttemptFilter {
        limit: 1,
        ..Default::default()
    };
    assert_eq!(store.list_replay_attempts(&limit_filter).unwrap().len(), 1);
    // No rows in a different environment scope.
    let prod_filter = AttemptFilter {
        environment_scope: "prod".to_string(),
        ..Default::default()
    };
    assert!(store.list_replay_attempts(&prod_filter).unwrap().is_empty());

    let fetched = store
        .get_replay_attempt("test", "att_1")
        .unwrap()
        .expect("found");
    assert_eq!(fetched.attempt_id, "att_1");
    assert_eq!(fetched.status, ReplayAttemptStatus::Completed);
    assert_eq!(store.get_replay_attempt("prod", "att_1").unwrap(), None);
    assert_eq!(store.get_replay_attempt("test", "missing").unwrap(), None);
}

#[test]
fn comparison_result_round_trips_through_sqlite() {
    let dir = temp_dir("eval_comparison");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();

    upsert_candidate(&store, "cand_1", "test");
    upsert_attempt(&store, "att_1", "cand_1", "test");

    let mut comparison = ComparisonResult {
        comparison_id: "cmp_1".to_string(),
        candidate_id: "cand_1".to_string(),
        baseline_ref: "att_baseline".to_string(),
        attempt_id: "att_1".to_string(),
        environment_scope: "test".to_string(),
        terminal_status: ComparisonTerminalStatus::Matched,
        runtime_summary: "runtime same".to_string(),
        policy_summary: String::new(),
        integration_summary: String::new(),
        delivery_summary: String::new(),
        evidence_summary: String::new(),
        confidence: "high".to_string(),
        limitations: Vec::new(),
        drift_findings: Vec::new(),
        change_window_label: "release-1".to_string(),
        generated_at: now,
    };
    store.upsert_comparison_result(&comparison).unwrap();
    // Upsert again through the ON CONFLICT path with a changed field.
    comparison.terminal_status = ComparisonTerminalStatus::Drifted;
    comparison.change_window_label = "release-2".to_string();
    store.upsert_comparison_result(&comparison).unwrap();

    let listed = store
        .list_comparison_results(&ComparisonFilter::default())
        .unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.comparison_id, "cmp_1");
    assert_eq!(got.candidate_id, "cand_1");
    assert_eq!(got.baseline_ref, "att_baseline");
    assert_eq!(got.attempt_id, "att_1");
    assert_eq!(got.environment_scope, "test");
    assert_eq!(got.terminal_status, ComparisonTerminalStatus::Drifted);
    assert_eq!(got.runtime_summary, "runtime same");
    assert_eq!(got.confidence, "high");
    assert_eq!(got.change_window_label, "release-2");

    // Dynamic filters match and miss.
    let status_filter = ComparisonFilter {
        terminal_status: ComparisonTerminalStatus::Blocked,
        ..Default::default()
    };
    assert!(
        store
            .list_comparison_results(&status_filter)
            .unwrap()
            .is_empty()
    );
    let drifted_filter = ComparisonFilter {
        terminal_status: ComparisonTerminalStatus::Drifted,
        ..Default::default()
    };
    assert_eq!(
        store
            .list_comparison_results(&drifted_filter)
            .unwrap()
            .len(),
        1
    );
    let candidate_filter = ComparisonFilter {
        candidate_id: "cand_1".to_string(),
        ..Default::default()
    };
    assert_eq!(
        store
            .list_comparison_results(&candidate_filter)
            .unwrap()
            .len(),
        1
    );
    let attempt_filter = ComparisonFilter {
        attempt_id: "att_1".to_string(),
        ..Default::default()
    };
    assert_eq!(
        store
            .list_comparison_results(&attempt_filter)
            .unwrap()
            .len(),
        1
    );
    let limit_filter = ComparisonFilter {
        limit: 1,
        ..Default::default()
    };
    assert_eq!(
        store.list_comparison_results(&limit_filter).unwrap().len(),
        1
    );
    // No rows in a different environment scope.
    let prod_filter = ComparisonFilter {
        environment_scope: "prod".to_string(),
        ..Default::default()
    };
    assert!(
        store
            .list_comparison_results(&prod_filter)
            .unwrap()
            .is_empty()
    );

    let fetched = store
        .get_comparison_result("test", "cmp_1")
        .unwrap()
        .expect("found");
    assert_eq!(fetched.comparison_id, "cmp_1");
    assert_eq!(fetched.terminal_status, ComparisonTerminalStatus::Drifted);
    assert_eq!(store.get_comparison_result("prod", "cmp_1").unwrap(), None);
    assert_eq!(
        store.get_comparison_result("test", "missing").unwrap(),
        None
    );
}

#[test]
fn regression_fixture_round_trips_through_sqlite() {
    let dir = temp_dir("eval_fixture");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();

    let mut fixture = RegressionFixture {
        fixture_id: "fix_1".to_string(),
        display_name: "Calendar scheduling regression".to_string(),
        domain_class: FixtureDomainClass::Schedule,
        manifest_path: "fixtures/calendar-schedule.json".to_string(),
        source_refs: Vec::new(),
        captured_evidence_refs: Vec::new(),
        assumptions: vec!["calendar is empty".to_string()],
        limitations: Vec::new(),
        expected_replay_mode: ReplayMode::NonLive,
        expected_comparison_summary: kura_evaluation::PlaneSummaries::default(),
        candidate_id: String::new(),
        environment_scope: "test".to_string(),
        created_at: now,
        updated_at: now,
    };
    store.upsert_regression_fixture(&fixture).unwrap();
    // Upsert again through the ON CONFLICT path with a changed field.
    fixture.manifest_path = "fixtures/calendar-schedule-v2.json".to_string();
    fixture.candidate_id = "cand_1".to_string();
    store.upsert_regression_fixture(&fixture).unwrap();

    let listed = store
        .list_regression_fixtures(&FixtureFilter::default())
        .unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.fixture_id, "fix_1");
    assert_eq!(got.display_name, "Calendar scheduling regression");
    assert_eq!(got.domain_class, FixtureDomainClass::Schedule);
    assert_eq!(got.manifest_path, "fixtures/calendar-schedule-v2.json");
    assert_eq!(got.candidate_id, "cand_1");
    assert_eq!(got.environment_scope, "test");
    assert_eq!(got.assumptions, vec!["calendar is empty".to_string()]);
    assert_eq!(got.expected_replay_mode, ReplayMode::NonLive);

    // Dynamic filters match and miss.
    let domain_filter = FixtureFilter {
        domain_class: FixtureDomainClass::Integration,
        ..Default::default()
    };
    assert!(
        store
            .list_regression_fixtures(&domain_filter)
            .unwrap()
            .is_empty()
    );
    // Schedule is the enum default ("unset"); use non-default classes for real misses.
    let computer_use_filter = FixtureFilter {
        domain_class: FixtureDomainClass::ComputerUse,
        ..Default::default()
    };
    assert!(
        store
            .list_regression_fixtures(&computer_use_filter)
            .unwrap()
            .is_empty()
    );
    let limit_filter = FixtureFilter {
        limit: 1,
        ..Default::default()
    };
    assert_eq!(
        store.list_regression_fixtures(&limit_filter).unwrap().len(),
        1
    );
    // No rows in a different environment scope.
    let prod_filter = FixtureFilter {
        environment_scope: "prod".to_string(),
        ..Default::default()
    };
    assert!(
        store
            .list_regression_fixtures(&prod_filter)
            .unwrap()
            .is_empty()
    );

    let fetched = store
        .get_regression_fixture("test", "fix_1")
        .unwrap()
        .expect("found");
    assert_eq!(fetched.fixture_id, "fix_1");
    assert_eq!(fetched.manifest_path, "fixtures/calendar-schedule-v2.json");
    assert_eq!(store.get_regression_fixture("prod", "fix_1").unwrap(), None);
    assert_eq!(
        store.get_regression_fixture("test", "missing").unwrap(),
        None
    );
}
