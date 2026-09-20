//! Port of daemon/internal/evaluation/manager_test.go: manager CRUD, fixture
//! loading, normalization, readiness blocking, plane-level comparisons, quota
//! denial before attempt/runtime work, and live-validation blocking.

mod common;

use std::sync::Arc;

use kura_billing::Manager as BillingManager;
use kura_evaluation::{
    ApprovalHandling, AttemptFilter, CandidateFilter, CandidateKind, ComparisonFilter,
    ComparisonTerminalStatus, CreateComparisonInput, CreateReplayAttemptInput, Dependencies,
    DriftPlane, EvaluationError, FixtureDomainClass, FixtureFilter, Manager, ReadinessStatus,
    ReplayAttemptStatus, ReplayCandidate, ReplayMode, SideEffectHandling, SourceKind, SourceRef,
};
use kura_identity::tenantctx;

use common::{
    CountingRecorder, MemoryStore, QuotaDenyRepo, SqliteStoreAdapter, fixed_now, temp_dir,
    tenant_context, test_replay_candidate,
};

fn test_manager(store: MemoryStore, fixtures_dir: &str) -> Manager {
    Manager::new(Dependencies {
        environment_scope: "test".to_string(),
        store: Some(Arc::new(store)),
        fixtures_dir: fixtures_dir.to_string(),
        clock: Some(Arc::new(fixed_now)),
        ..Default::default()
    })
}

#[tokio::test]
async fn manager_loads_fixture_candidates_and_launches_non_live_replay() {
    let store = MemoryStore::new();
    let manager = test_manager(store, "testdata/fixtures");
    manager
        .load_fixtures()
        .expect("LoadFixtures returned error");

    let candidates = manager
        .list_replay_candidates(&CandidateFilter {
            environment_scope: "test".to_string(),
            ..Default::default()
        })
        .expect("ListReplayCandidates returned error");
    assert_eq!(candidates.len(), 3, "expected 3 fixture candidates");
    assert_eq!(
        candidates[0].default_replay_mode,
        ReplayMode::NonLive,
        "expected non-live default mode"
    );

    let attempt = manager
        .create_replay_attempt(
            &candidates[0].candidate_id,
            CreateReplayAttemptInput::default(),
        )
        .await
        .expect("CreateReplayAttempt returned error");
    assert_eq!(attempt.mode, ReplayMode::NonLive, "expected non-live mode");
    assert_eq!(
        attempt.status,
        ReplayAttemptStatus::Completed,
        "expected completed replay processing"
    );
    assert_eq!(
        attempt.side_effect_handling,
        SideEffectHandling::EvidenceOnly,
        "expected evidence-only side effect handling"
    );
    assert!(
        !attempt.evidence_refs.is_empty(),
        "expected replay attempt evidence refs"
    );
}

#[test]
fn manager_normalizes_nil_collections_for_api_responses() {
    let store = MemoryStore::new();
    let now = fixed_now();
    store.insert_candidate(ReplayCandidate {
        candidate_id: "candidate_legacy".to_string(),
        candidate_kind: CandidateKind::CuratedWork,
        display_name: "Legacy Candidate".to_string(),
        source_kind: SourceKind::Run,
        source_id: "run_1".to_string(),
        environment_scope: "test".to_string(),
        readiness_status: ReadinessStatus::FullyReplayable,
        default_replay_mode: ReplayMode::NonLive,
        created_at: now,
        updated_at: now,
        ..Default::default()
    });
    store.insert_attempt(kura_evaluation::ReplayAttempt {
        attempt_id: "attempt_legacy".to_string(),
        candidate_id: "candidate_legacy".to_string(),
        environment_scope: "test".to_string(),
        mode: ReplayMode::NonLive,
        status: ReplayAttemptStatus::Completed,
        created_at: now,
        updated_at: now,
        ..Default::default()
    });
    store.insert_comparison(kura_evaluation::ComparisonResult {
        comparison_id: "comparison_legacy".to_string(),
        candidate_id: "candidate_legacy".to_string(),
        attempt_id: "attempt_legacy".to_string(),
        environment_scope: "test".to_string(),
        terminal_status: ComparisonTerminalStatus::Matched,
        generated_at: now,
        ..Default::default()
    });
    store.insert_fixture(kura_evaluation::RegressionFixture {
        fixture_id: "fixture_legacy".to_string(),
        display_name: "Legacy Fixture".to_string(),
        domain_class: FixtureDomainClass::Schedule,
        manifest_path: "manifest.json".to_string(),
        environment_scope: "test".to_string(),
        created_at: now,
        updated_at: now,
        ..Default::default()
    });
    let manager = test_manager(store, "");

    let candidates = manager
        .list_replay_candidates(&CandidateFilter::default())
        .expect("ListReplayCandidates returned error");
    assert!(
        candidates[0].source_refs.is_empty()
            && candidates[0].readiness_reasons.is_empty()
            && candidates[0].limitations.is_empty()
            && candidates[0].captured_evidence_refs.is_empty(),
        "expected candidate collections to be normalized"
    );
    let candidate = manager
        .get_replay_candidate("candidate_legacy")
        .expect("GetReplayCandidate")
        .expect("candidate found");
    assert!(
        candidate.source_refs.is_empty()
            && candidate.readiness_reasons.is_empty()
            && candidate.limitations.is_empty()
            && candidate.captured_evidence_refs.is_empty(),
        "expected candidate detail collections to be normalized"
    );
    let attempts = manager
        .list_replay_attempts(&AttemptFilter::default())
        .expect("ListReplayAttempts returned error");
    assert!(
        attempts[0].source_refs.is_empty()
            && attempts[0].evidence_refs.is_empty()
            && attempts[0].blocked_reasons.is_empty(),
        "expected attempt collections to be normalized"
    );
    let comparisons = manager
        .list_comparisons(&ComparisonFilter::default())
        .expect("ListComparisons returned error");
    assert!(
        comparisons[0].limitations.is_empty() && comparisons[0].drift_findings.is_empty(),
        "expected comparison collections to be normalized"
    );
    let fixtures = manager
        .list_fixtures(&FixtureFilter::default())
        .expect("ListFixtures returned error");
    assert!(
        fixtures[0].source_refs.is_empty()
            && fixtures[0].captured_evidence_refs.is_empty()
            && fixtures[0].assumptions.is_empty()
            && fixtures[0].limitations.is_empty(),
        "expected fixture collections to be normalized"
    );
}

#[tokio::test]
async fn manager_blocks_unready_candidate_without_running_side_effects() {
    let store = MemoryStore::new();
    let manager = test_manager(store, "");
    let candidate = ReplayCandidate {
        candidate_id: "candidate_blocked".to_string(),
        candidate_kind: CandidateKind::CuratedWork,
        display_name: "Blocked approval".to_string(),
        source_kind: SourceKind::Run,
        source_id: "run_blocked".to_string(),
        environment_scope: "test".to_string(),
        readiness_status: ReadinessStatus::Blocked,
        readiness_reasons: vec!["approval-gated side effect requires live validation".to_string()],
        limitations: vec!["default replay remains non-live".to_string()],
        default_replay_mode: ReplayMode::NonLive,
        source_refs: vec![SourceRef {
            kind: SourceKind::Run,
            id: "run_blocked".to_string(),
            route: "/v1/runs/run_blocked".to_string(),
        }],
        ..Default::default()
    };
    manager
        .upsert_replay_candidate(candidate)
        .expect("UpsertReplayCandidate returned error");

    let attempt = manager
        .create_replay_attempt("candidate_blocked", CreateReplayAttemptInput::default())
        .await
        .expect("CreateReplayAttempt returned error");
    assert_eq!(
        attempt.status,
        ReplayAttemptStatus::Blocked,
        "expected blocked attempt"
    );
    assert_eq!(
        attempt.side_effect_handling,
        SideEffectHandling::Blocked,
        "expected blocked side effect handling"
    );
    assert!(
        !attempt.blocked_reasons.is_empty(),
        "expected blocked reasons"
    );
}

#[test]
fn manager_rejects_replay_candidate_without_source_provenance() {
    let store = Arc::new(MemoryStore::new());
    let manager = Manager::new(Dependencies {
        environment_scope: "test".to_string(),
        store: Some(store.clone()),
        clock: Some(Arc::new(fixed_now)),
        ..Default::default()
    });
    let candidate = ReplayCandidate {
        candidate_id: "candidate_missing_source".to_string(),
        candidate_kind: CandidateKind::CuratedWork,
        display_name: "Missing Source".to_string(),
        environment_scope: "test".to_string(),
        readiness_status: ReadinessStatus::FullyReplayable,
        default_replay_mode: ReplayMode::NonLive,
        ..Default::default()
    };
    let err = manager
        .upsert_replay_candidate(candidate)
        .expect_err("expected missing source provenance to be rejected");
    // The closed SourceKind enum defaults to Run, so the first failing check is
    // the empty sourceId (Go's zero SourceKind "" would fail on sourceKind).
    assert!(matches!(err, EvaluationError::SourceIdRequired));
    assert!(
        store.candidate("candidate_missing_source").is_none(),
        "expected rejected candidate not to be stored"
    );
}

#[tokio::test]
async fn manager_creates_plane_level_comparison() {
    let store = MemoryStore::new();
    let manager = test_manager(store, "testdata/fixtures");
    manager
        .load_fixtures()
        .expect("LoadFixtures returned error");
    let candidates = manager
        .list_replay_candidates(&CandidateFilter {
            environment_scope: "test".to_string(),
            source_kind: SourceKind::Fixture,
            ..Default::default()
        })
        .expect("ListReplayCandidates returned error");
    let attempt = manager
        .create_replay_attempt(
            &candidates[0].candidate_id,
            CreateReplayAttemptInput {
                change_window_label: "phase-33".to_string(),
                ..Default::default()
            },
        )
        .await
        .expect("CreateReplayAttempt returned error");

    let comparison = manager
        .create_comparison(
            &attempt.attempt_id,
            CreateComparisonInput {
                change_window_label: "phase-33".to_string(),
                ..Default::default()
            },
        )
        .expect("CreateComparison returned error");
    assert_eq!(
        comparison.terminal_status,
        ComparisonTerminalStatus::Matched,
        "expected matched comparison"
    );
    assert!(
        !comparison.runtime_summary.is_empty()
            && !comparison.policy_summary.is_empty()
            && !comparison.evidence_summary.is_empty(),
        "expected plane summaries"
    );
    assert_eq!(
        comparison.change_window_label, "phase-33",
        "expected change window label"
    );
}

#[tokio::test]
async fn fixture_replay_uses_captured_evidence_instead_of_expected_summary() {
    let root = temp_dir("fixture_drift");
    let fixture_dir = std::path::Path::new(&root).join("runtime-drift");
    std::fs::create_dir_all(&fixture_dir).expect("MkdirAll");
    std::fs::write(
        fixture_dir.join("manifest.json"),
        r#"{
            "fixtureId": "fixture_runtime_drift",
            "displayName": "Runtime Drift Fixture",
            "domainClass": "schedule",
            "sourceRefs": [{"kind":"schedule","id":"sched_drift"}],
            "capturedEvidenceRefs": [{"kind":"fixture_evidence","id":"evidence.json"}],
            "assumptions": ["test fixture"],
            "limitations": [],
            "expectedReplayMode": "non_live",
            "expectedComparisonSummary": {
                "runtime": "baseline runtime",
                "policy": "baseline policy",
                "integration": "baseline integration",
                "delivery": "baseline delivery",
                "evidence": "baseline evidence"
            }
        }"#,
    )
    .expect("WriteFile(manifest)");
    std::fs::write(
        fixture_dir.join("evidence.json"),
        r#"{
            "terminalStatus": "completed",
            "runtime": "replayed runtime from captured evidence",
            "policy": "baseline policy",
            "integration": "baseline integration",
            "delivery": "baseline delivery",
            "evidence": "baseline evidence"
        }"#,
    )
    .expect("WriteFile(evidence)");

    let store = MemoryStore::new();
    let manager = Manager::new(Dependencies {
        environment_scope: "test".to_string(),
        store: Some(Arc::new(store)),
        fixtures_dir: root.clone(),
        clock: Some(Arc::new(fixed_now)),
        ..Default::default()
    });
    manager
        .load_fixtures()
        .expect("LoadFixtures returned error");
    let attempt = manager
        .create_replay_attempt(
            "candidate_fixture_runtime_drift",
            CreateReplayAttemptInput::default(),
        )
        .await
        .expect("CreateReplayAttempt returned error");
    assert_eq!(
        attempt.runtime_summary, "replayed runtime from captured evidence",
        "expected runtime from evidence file"
    );
    let comparison = manager
        .create_comparison(&attempt.attempt_id, CreateComparisonInput::default())
        .expect("CreateComparison returned error");
    assert_eq!(
        comparison.terminal_status,
        ComparisonTerminalStatus::Drifted,
        "expected runtime drift from evidence replay"
    );
    assert_eq!(comparison.drift_findings.len(), 1);
    assert_eq!(
        comparison.drift_findings[0].plane,
        DriftPlane::Runtime,
        "expected runtime plane finding"
    );
}

#[tokio::test]
async fn comparison_can_use_baseline_attempt_evidence() {
    let store = Arc::new(MemoryStore::new());
    let manager = Manager::new(Dependencies {
        environment_scope: "test".to_string(),
        store: Some(store.clone()),
        clock: Some(Arc::new(fixed_now)),
        ..Default::default()
    });
    let candidate = ReplayCandidate {
        candidate_id: "candidate_baseline".to_string(),
        candidate_kind: CandidateKind::CuratedWork,
        display_name: "Baseline candidate".to_string(),
        source_kind: SourceKind::Run,
        source_id: "run_source".to_string(),
        environment_scope: "test".to_string(),
        readiness_status: ReadinessStatus::FullyReplayable,
        default_replay_mode: ReplayMode::NonLive,
        source_refs: vec![SourceRef {
            kind: SourceKind::Run,
            id: "run_source".to_string(),
            ..Default::default()
        }],
        ..Default::default()
    };
    manager
        .upsert_replay_candidate(candidate)
        .expect("UpsertReplayCandidate returned error");
    let now = fixed_now();
    let baseline = kura_evaluation::ReplayAttempt {
        attempt_id: "attempt_baseline".to_string(),
        candidate_id: "candidate_baseline".to_string(),
        environment_scope: "test".to_string(),
        mode: ReplayMode::NonLive,
        status: ReplayAttemptStatus::Completed,
        approval_handling: ApprovalHandling::EvidenceOnly,
        side_effect_handling: SideEffectHandling::EvidenceOnly,
        runtime_summary: "baseline runtime".to_string(),
        policy_summary: "same policy".to_string(),
        integration_summary: "same integration".to_string(),
        delivery_summary: "same delivery".to_string(),
        evidence_summary: "same evidence".to_string(),
        created_at: now,
        updated_at: now,
        ..Default::default()
    };
    let mut replay = baseline.clone();
    replay.attempt_id = "attempt_replay".to_string();
    replay.runtime_summary = "changed runtime".to_string();
    store.insert_attempt(baseline.clone());
    store.insert_attempt(replay.clone());

    let comparison = manager
        .create_comparison(
            &replay.attempt_id,
            CreateComparisonInput {
                baseline_attempt_id: baseline.attempt_id.clone(),
                ..Default::default()
            },
        )
        .expect("CreateComparison returned error");
    assert_eq!(comparison.baseline_ref, baseline.attempt_id);
    assert_eq!(
        comparison.terminal_status,
        ComparisonTerminalStatus::Drifted,
        "expected baseline-attempt drift comparison"
    );
    assert_eq!(comparison.drift_findings.len(), 1);
    assert_eq!(
        comparison.drift_findings[0].baseline_value, "baseline runtime",
        "expected runtime finding against baseline attempt"
    );
}

#[tokio::test]
async fn live_validation_replay_is_explicitly_blocked_until_executor_exists() {
    let store = MemoryStore::new();
    let manager = test_manager(store, "testdata/fixtures");
    manager
        .load_fixtures()
        .expect("LoadFixtures returned error");
    let candidates = manager
        .list_replay_candidates(&CandidateFilter {
            environment_scope: "test".to_string(),
            ..Default::default()
        })
        .expect("ListReplayCandidates returned error");
    let attempt = manager
        .create_replay_attempt(
            &candidates[0].candidate_id,
            CreateReplayAttemptInput {
                mode: Some(ReplayMode::LiveValidation),
                ..Default::default()
            },
        )
        .await
        .expect("CreateReplayAttempt returned error");
    assert_eq!(
        attempt.status,
        ReplayAttemptStatus::Blocked,
        "expected live validation to block before an executor exists"
    );
    assert!(
        attempt.side_effect_handling == SideEffectHandling::Blocked
            && !attempt.blocked_reasons.is_empty(),
        "expected blocked side-effect handling and reason"
    );
}

#[tokio::test]
async fn non_live_replay_is_unaffected_by_live_validation_kill_switch_concept() {
    let store = MemoryStore::new();
    let manager = test_manager(store, "testdata/fixtures");
    manager
        .load_fixtures()
        .expect("LoadFixtures returned error");
    let candidates = manager
        .list_replay_candidates(&CandidateFilter {
            environment_scope: "test".to_string(),
            ..Default::default()
        })
        .expect("ListReplayCandidates returned error");
    let attempt = manager
        .create_replay_attempt(
            &candidates[0].candidate_id,
            CreateReplayAttemptInput {
                mode: Some(ReplayMode::NonLive),
                ..Default::default()
            },
        )
        .await
        .expect("CreateReplayAttempt returned error");
    assert_eq!(attempt.status, ReplayAttemptStatus::Completed);
    assert_eq!(
        attempt.side_effect_handling,
        SideEffectHandling::EvidenceOnly,
        "non-live replay should remain evidence-only and completed"
    );
}

#[tokio::test]
async fn manager_replays_and_compares_required_fixture_classes() {
    let store = MemoryStore::new();
    let manager = test_manager(store, "testdata/fixtures");
    manager
        .load_fixtures()
        .expect("LoadFixtures returned error");
    let fixtures = manager
        .list_fixtures(&FixtureFilter {
            environment_scope: "test".to_string(),
            ..Default::default()
        })
        .expect("ListFixtures returned error");
    let mut seen = std::collections::HashSet::new();
    for fixture in &fixtures {
        seen.insert(fixture.domain_class);
        let attempt = manager
            .create_replay_attempt(&fixture.candidate_id, CreateReplayAttemptInput::default())
            .await
            .expect("CreateReplayAttempt returned error");
        assert_eq!(attempt.status, ReplayAttemptStatus::Completed);
        let comparison = manager
            .create_comparison(&attempt.attempt_id, CreateComparisonInput::default())
            .expect("CreateComparison returned error");
        assert_eq!(
            comparison.terminal_status,
            ComparisonTerminalStatus::Matched,
            "expected matched comparison for {}",
            fixture.fixture_id
        );
    }
    for domain in [
        FixtureDomainClass::Schedule,
        FixtureDomainClass::Integration,
        FixtureDomainClass::ComputerUse,
    ] {
        assert!(seen.contains(&domain), "expected fixture class {domain:?}");
    }
}

#[tokio::test]
async fn replay_evaluation_attempt_quota_denies_before_attempt_and_runtime_work() {
    let store = SqliteStoreAdapter::new(
        kura_store::SQLiteStore::new(&temp_dir("quota_deny")).expect("store"),
    );
    let billing = BillingManager::with_clock(Arc::new(QuotaDenyRepo), fixed_now);
    let recorder = Arc::new(CountingRecorder::default());
    let manager = Manager::new(Dependencies {
        environment_scope: "test".to_string(),
        store: Some(Arc::new(store)),
        runtime_recorder: Some(recorder.clone()),
        billing: Some(Arc::new(billing)),
        hosted_billing: true,
        clock: Some(Arc::new(fixed_now)),
        ..Default::default()
    });
    manager
        .upsert_replay_candidate(test_replay_candidate("candidate_attempt_denied"))
        .expect("UpsertReplayCandidate returned error");

    let tenant = tenant_context("ten_eval_quota_deny");
    let result = tenantctx::scope(tenant, async {
        manager
            .create_replay_attempt(
                "candidate_attempt_denied",
                CreateReplayAttemptInput::default(),
            )
            .await
    })
    .await;
    let err = result.expect_err("expected quota denial");
    assert!(
        matches!(
            err,
            EvaluationError::BillingReservation(ref bre)
                if matches!(bre.error, kura_billing::BillingError::QuotaDenied)
        ),
        "expected ErrQuotaDenied, got {err:?}"
    );
    assert_eq!(
        recorder.calls.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "expected runtime recorder not to be called"
    );
    let attempts = manager
        .list_replay_attempts(&AttemptFilter::default())
        .expect("ListReplayAttempts returned error");
    assert!(
        attempts.is_empty(),
        "expected no replay attempt persisted before quota denial"
    );
}

#[tokio::test]
async fn manager_allows_attempt_without_billing_manager() {
    let store = MemoryStore::new();
    let manager = test_manager(store, "");
    manager
        .upsert_replay_candidate(test_replay_candidate("candidate_unmetered"))
        .expect("UpsertReplayCandidate returned error");
    let attempt = manager
        .create_replay_attempt("candidate_unmetered", CreateReplayAttemptInput::default())
        .await
        .expect("attempt without billing manager should be allowed");
    assert_eq!(attempt.status, ReplayAttemptStatus::Completed);
    assert!(
        attempt.completed_at.is_some() && attempt.started_at.is_some(),
        "expected started/completed timestamps"
    );
}

#[test]
fn manager_prepare_live_validation_handoff_requires_candidate() {
    let store = MemoryStore::new();
    let manager = test_manager(store, "");
    let err = manager
        .prepare_live_validation_handoff("candidate_missing")
        .expect_err("expected missing candidate error");
    assert!(
        matches!(err, EvaluationError::CandidateNotFound(_)),
        "got {err:?}"
    );
}
