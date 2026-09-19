//! Port of daemon/internal/evaluation/discovery_test.go,
//! discovery_scoring_test.go, and discovery_sources_test.go.

mod common;

use chrono::DateTime;
use chrono::Utc;
use kura_evaluation::{
    CandidateScoringInput, DISCOVERY_PARTIAL_REASON_MAX_INSPECTED_RECORDS, DiscoveryPolicy,
    DiscoveryProgress, DiscoverySourceFilter, DiscoverySourceRecord, EvaluationError,
    ProductLifecycleStatus, ReadinessStatus, RedactionStatus, ScoreBand, SourceKind, SourceRef,
    StartDiscoveryRunInput, apply_discovery_run_progress, build_discovered_candidate_from_signals,
    build_discovery_run_from_policy, collect_discovery_source_refs, discovery_idempotency_scope,
    discovery_source_route, read_discovery_source_refs,
};

fn ts(s: &str) -> DateTime<Utc> {
    s.parse::<DateTime<Utc>>().expect("ts")
}

#[test]
fn build_discovery_run_from_policy_validates_bounds_and_idempotency() {
    let now = ts("2026-04-29T10:00:00Z");
    let policy = DiscoveryPolicy {
        policy_id: "policy_1".to_string(),
        tenant_id: "ten_eval".to_string(),
        enabled: true,
        source_kinds: vec![SourceKind::Run],
        window_start: now - chrono::Duration::hours(1),
        window_end: now,
        max_inspected_records: 10,
        max_emitted_candidates: 2,
        cost_budget: 5,
        ..Default::default()
    };
    let run = build_discovery_run_from_policy(
        policy.clone(),
        StartDiscoveryRunInput {
            started_by: "prn_eval".to_string(),
            cursor: "cursor_1".to_string(),
            idempotency_key: "idem_1".to_string(),
            ..Default::default()
        },
        now,
    )
    .expect("BuildDiscoveryRunFromPolicy");
    assert_eq!(run.tenant_id, policy.tenant_id);
    assert_eq!(run.policy_id, policy.policy_id);
    assert_eq!(run.status, ProductLifecycleStatus::Queued);
    assert_eq!(run.cursor, "cursor_1");
    assert_eq!(run.idempotency_key, "idem_1");
    assert_eq!(discovery_idempotency_scope(&run), "ten_eval:idem_1");
}

#[test]
fn build_discovery_run_from_policy_rejects_invalid_bounds() {
    let now = ts("2026-04-29T10:00:00Z");
    let err = build_discovery_run_from_policy(
        DiscoveryPolicy {
            policy_id: "policy_bad".to_string(),
            tenant_id: "ten_eval".to_string(),
            enabled: true,
            window_start: now,
            window_end: now - chrono::Duration::hours(1),
            max_inspected_records: 10,
            max_emitted_candidates: 2,
            cost_budget: 5,
            ..Default::default()
        },
        StartDiscoveryRunInput::default(),
        now,
    )
    .expect_err("invalid bounds must be rejected");
    assert!(matches!(err, EvaluationError::ProductBoundsInvalid));
}

#[test]
fn apply_discovery_run_progress_marks_partial_at_bounds() {
    let now = ts("2026-04-29T10:00:00Z");
    let run = kura_evaluation::DiscoveryRun {
        discovery_run_id: "run_1".to_string(),
        tenant_id: "ten_eval".to_string(),
        status: ProductLifecycleStatus::Running,
        max_inspected_records: 10,
        max_emitted_candidates: 2,
        updated_at: now,
        ..Default::default()
    };
    let updated = apply_discovery_run_progress(
        run,
        DiscoveryProgress {
            inspected_records: 10,
            emitted_candidates: 1,
            cursor: "next".to_string(),
            ..Default::default()
        },
        now + chrono::Duration::minutes(1),
    );
    assert_eq!(updated.status, ProductLifecycleStatus::Partial);
    assert_eq!(
        updated.partial_reason,
        DISCOVERY_PARTIAL_REASON_MAX_INSPECTED_RECORDS
    );
    assert_eq!(updated.cursor, "next");
    assert_eq!(updated.updated_at, now + chrono::Duration::minutes(1));
}

#[test]
fn build_discovered_candidate_from_signals_scores_and_explains_candidate() {
    let now = ts("2026-04-29T10:00:00Z");
    let candidate = build_discovered_candidate_from_signals(
        CandidateScoringInput {
            tenant_id: "ten_eval".to_string(),
            discovery_run_id: "discovery_run_1".to_string(),
            source_kind: SourceKind::Run,
            source_id: "run_1".to_string(),
            source_refs: vec![SourceRef {
                kind: SourceKind::Run,
                id: "run_1".to_string(),
                route: "/v1/runs/run_1".to_string(),
            }],
            failure_recurrence: 3,
            drift_signal: true,
            tool_call_class: "mail.send".to_string(),
            live_validation_outcome: "operator_action_needed".to_string(),
            workflow_coverage: 2,
            operator_relevance: 2,
            observed_at: now - chrono::Duration::hours(2),
            redaction_status: RedactionStatus::Redacted,
            ..Default::default()
        },
        now,
    )
    .expect("BuildDiscoveredCandidateFromSignals");
    assert_eq!(candidate.score_band, ScoreBand::High);
    assert!(candidate.score >= 0.75, "score={}", candidate.score);
    assert_eq!(candidate.readiness_status, ReadinessStatus::FullyReplayable);
    assert_eq!(candidate.redaction_status, RedactionStatus::Redacted);
    assert_eq!(
        candidate
            .explanation_fields
            .get("toolCallClass")
            .and_then(|v| v.as_str()),
        Some("mail.send")
    );
    assert_eq!(
        candidate
            .explanation_fields
            .get("liveValidationOutcome")
            .and_then(|v| v.as_str()),
        Some("operator_action_needed")
    );
}

#[test]
fn build_discovered_candidate_from_signals_fails_closed_on_redaction_failure() {
    let now = ts("2026-04-29T10:00:00Z");
    let candidate = build_discovered_candidate_from_signals(
        CandidateScoringInput {
            tenant_id: "ten_eval".to_string(),
            discovery_run_id: "discovery_run_1".to_string(),
            source_kind: SourceKind::Workflow,
            source_id: "workflow_1".to_string(),
            redaction_status: RedactionStatus::Failed,
            ..Default::default()
        },
        now,
    )
    .expect("BuildDiscoveredCandidateFromSignals");
    assert_eq!(
        candidate.readiness_status,
        ReadinessStatus::Blocked,
        "readiness must be blocked after redaction failure"
    );
}

#[test]
fn build_discovered_candidate_from_signals_requires_source() {
    let err = build_discovered_candidate_from_signals(
        CandidateScoringInput {
            tenant_id: "ten_eval".to_string(),
            discovery_run_id: "discovery_run_1".to_string(),
            source_kind: SourceKind::Run,
            ..Default::default()
        },
        Utc::now(),
    )
    .expect_err("source required");
    assert!(matches!(err, EvaluationError::ProductSourceRequired));
}

#[test]
fn discovery_source_refs_require_single_tenant() {
    let refs = collect_discovery_source_refs(
        "ten_eval",
        &[
            DiscoverySourceRecord {
                tenant_id: "ten_eval".to_string(),
                kind: SourceKind::Run,
                id: "run_1".to_string(),
                ..Default::default()
            },
            DiscoverySourceRecord {
                tenant_id: "ten_eval".to_string(),
                kind: SourceKind::Workflow,
                id: "workflow_1".to_string(),
                ..Default::default()
            },
            DiscoverySourceRecord {
                tenant_id: "ten_eval".to_string(),
                kind: SourceKind::ToolCall,
                id: "tool_call_1".to_string(),
                ..Default::default()
            },
            DiscoverySourceRecord {
                tenant_id: "ten_eval".to_string(),
                kind: SourceKind::Fixture,
                id: "fixture_1".to_string(),
                ..Default::default()
            },
            DiscoverySourceRecord {
                tenant_id: "ten_eval".to_string(),
                kind: SourceKind::LiveValidationLedger,
                id: "ledger_1".to_string(),
                ..Default::default()
            },
        ],
    )
    .expect("CollectDiscoverySourceRefs");
    assert_eq!(refs.len(), 5);
    assert_eq!(refs[2].route, "/v1/tool-calls/tool_call_1");
    assert_eq!(refs[4].route, "/v1/live-validations/ledger/ledger_1");
    // Route coverage for the raw string kinds:
    assert_eq!(
        discovery_source_route(SourceKind::ToolCall, "tc"),
        "/v1/tool-calls/tc"
    );
    assert_eq!(
        discovery_source_route(SourceKind::LiveValidationLedger, "lv"),
        "/v1/live-validations/ledger/lv"
    );
}

#[test]
fn discovery_source_refs_reject_cross_tenant_records() {
    let err = collect_discovery_source_refs(
        "ten_eval",
        &[
            DiscoverySourceRecord {
                tenant_id: "ten_eval".to_string(),
                kind: SourceKind::Run,
                id: "run_1".to_string(),
                ..Default::default()
            },
            DiscoverySourceRecord {
                tenant_id: "ten_other".to_string(),
                kind: SourceKind::Workflow,
                id: "workflow_1".to_string(),
                ..Default::default()
            },
        ],
    )
    .expect_err("cross-tenant rows must be rejected");
    assert!(matches!(err, EvaluationError::ProductCrossTenantSource));
}

struct FakeReader {
    records: Vec<DiscoverySourceRecord>,
    cursor: String,
    last_limit: std::sync::atomic::AtomicI64,
}

impl kura_evaluation::DiscoverySourceReader for FakeReader {
    fn list_discovery_sources(
        &self,
        filter: &DiscoverySourceFilter,
    ) -> Result<(Vec<DiscoverySourceRecord>, String), EvaluationError> {
        self.last_limit
            .store(filter.limit, std::sync::atomic::Ordering::Relaxed);
        Ok((self.records.clone(), self.cursor.clone()))
    }
}

#[test]
fn read_discovery_source_refs_normalizes_bounds_and_rejects_cross_tenant_rows() {
    let reader = FakeReader {
        records: vec![DiscoverySourceRecord {
            tenant_id: "ten_eval".to_string(),
            kind: SourceKind::Run,
            id: "run_1".to_string(),
            ..Default::default()
        }],
        cursor: "cursor_next".to_string(),
        last_limit: std::sync::atomic::AtomicI64::new(0),
    };
    assert_eq!(
        reader.last_limit.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "reader not called yet"
    );
    let (refs, cursor) = read_discovery_source_refs(
        &reader,
        &DiscoverySourceFilter {
            tenant_id: "ten_eval".to_string(),
            ..Default::default()
        },
    )
    .expect("ReadDiscoverySourceRefs");
    assert_eq!(
        reader.last_limit.load(std::sync::atomic::Ordering::Relaxed),
        kura_evaluation::DEFAULT_PRODUCT_PAGE_LIMIT,
        "expected default product page limit to be normalized into the reader filter"
    );
    assert_eq!(cursor, "cursor_next");
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].route, "/v1/runs/run_1");
}
