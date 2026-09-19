//! Port of daemon/internal/evaluation/suppression_test.go.

mod common;

use chrono::DateTime;
use chrono::Utc;
use kura_evaluation::SuppressionRecord;
use kura_evaluation::{
    CreateSuppressionInput, DiscoveredCandidate, ProductResourceKind, SourceKind,
    filter_suppressed_candidates, find_active_suppression, new_suppression_record,
    revoke_suppression_record, suppression_applies,
};

fn ts(s: &str) -> DateTime<Utc> {
    s.parse::<DateTime<Utc>>().expect("ts")
}

#[test]
fn new_suppression_record_defaults_and_requires_target() {
    let now = ts("2026-04-29T10:00:00Z");
    let record = new_suppression_record(
        CreateSuppressionInput {
            tenant_id: "ten_eval".to_string(),
            target_kind: ProductResourceKind::DiscoveredCandidate,
            target_id: "candidate_1".to_string(),
            created_by: "prn_eval".to_string(),
            ..Default::default()
        },
        now,
    )
    .expect("NewSuppressionRecord");
    assert_eq!(
        record.suppression_id,
        "suppression_discovered_candidate_candidate_1"
    );
    assert_eq!(record.reason_code, "operator_hidden");
    assert!(record.active);

    let err = new_suppression_record(
        CreateSuppressionInput {
            tenant_id: "ten_eval".to_string(),
            target_kind: ProductResourceKind::DiscoveredCandidate,
            ..Default::default()
        },
        now,
    )
    .expect_err("missing target must fail");
    assert!(matches!(
        err,
        kura_evaluation::EvaluationError::ProductSuppressionTargetRequired
    ));
}

#[test]
fn suppression_matches_target_and_source_families() {
    let now = ts("2026-04-29T10:00:00Z");
    let candidate = DiscoveredCandidate {
        discovered_candidate_id: "candidate_1".to_string(),
        tenant_id: "ten_eval".to_string(),
        source_kind: SourceKind::Run,
        source_id: "run_1".to_string(),
        ..Default::default()
    };
    let records = vec![
        SuppressionRecord {
            tenant_id: "ten_eval".to_string(),
            target_kind: ProductResourceKind::DiscoveredCandidate,
            target_id: "candidate_2".to_string(),
            active: true,
            created_at: now,
            ..Default::default()
        },
        SuppressionRecord {
            tenant_id: "ten_eval".to_string(),
            target_kind: ProductResourceKind::DiscoveredCandidate,
            target_id: "candidate_1".to_string(),
            active: true,
            created_at: now,
            ..Default::default()
        },
    ];
    assert!(
        suppression_applies(&candidate, &records, now),
        "target suppression must apply"
    );

    let source_family = vec![SuppressionRecord {
        tenant_id: "ten_eval".to_string(),
        target_kind: ProductResourceKind::DiscoveryRun,
        target_source_ref: "run:run_1".to_string(),
        active: true,
        created_at: now,
        ..Default::default()
    }];
    assert!(
        suppression_applies(&candidate, &source_family, now),
        "source family suppression must apply"
    );
}

#[test]
fn suppression_ignores_inactive_expired_and_cross_tenant_records() {
    let now = ts("2026-04-29T10:00:00Z");
    let expired_at = now - chrono::Duration::minutes(1);
    let candidate = DiscoveredCandidate {
        discovered_candidate_id: "candidate_1".to_string(),
        tenant_id: "ten_eval".to_string(),
        source_kind: SourceKind::Run,
        source_id: "run_1".to_string(),
        ..Default::default()
    };
    let records = vec![
        SuppressionRecord {
            tenant_id: "ten_eval".to_string(),
            target_kind: ProductResourceKind::DiscoveredCandidate,
            target_id: "candidate_1".to_string(),
            active: false,
            created_at: now,
            ..Default::default()
        },
        SuppressionRecord {
            tenant_id: "ten_eval".to_string(),
            target_kind: ProductResourceKind::DiscoveredCandidate,
            target_id: "candidate_1".to_string(),
            active: true,
            expires_at: Some(expired_at),
            created_at: now,
            ..Default::default()
        },
        SuppressionRecord {
            tenant_id: "ten_other".to_string(),
            target_kind: ProductResourceKind::DiscoveredCandidate,
            target_id: "candidate_1".to_string(),
            active: true,
            created_at: now,
            ..Default::default()
        },
    ];
    assert!(
        !suppression_applies(&candidate, &records, now),
        "inactive, expired, or cross-tenant suppression applied"
    );
}

#[test]
fn suppression_lookup_revocation_and_candidate_filtering() {
    let now = ts("2026-04-29T10:00:00Z");
    let candidates = vec![
        DiscoveredCandidate {
            discovered_candidate_id: "candidate_1".to_string(),
            tenant_id: "ten_eval".to_string(),
            source_kind: SourceKind::Run,
            source_id: "run_1".to_string(),
            ..Default::default()
        },
        DiscoveredCandidate {
            discovered_candidate_id: "candidate_2".to_string(),
            tenant_id: "ten_eval".to_string(),
            source_kind: SourceKind::Workflow,
            source_id: "workflow_1".to_string(),
            ..Default::default()
        },
    ];
    let record = new_suppression_record(
        CreateSuppressionInput {
            suppression_id: "suppression_1".to_string(),
            tenant_id: "ten_eval".to_string(),
            target_kind: ProductResourceKind::DiscoveredCandidate,
            target_source_ref: "run:run_1".to_string(),
            reason_code: "operator_hidden".to_string(),
            ..Default::default()
        },
        now,
    )
    .expect("NewSuppressionRecord");

    assert!(
        find_active_suppression(&[record.clone()], "ten_eval", "suppression_1", now).is_some(),
        "active suppression was not found"
    );
    let filtered = filter_suppressed_candidates(candidates.clone(), &[record.clone()], now);
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].discovered_candidate_id, "candidate_2");

    let revoked = revoke_suppression_record(record, now + chrono::Duration::minutes(1));
    assert!(
        find_active_suppression(
            &[revoked.clone()],
            "ten_eval",
            "suppression_1",
            now + chrono::Duration::minutes(2)
        )
        .is_none(),
        "revoked suppression should not be active"
    );
    let filtered =
        filter_suppressed_candidates(candidates, &[revoked], now + chrono::Duration::minutes(2));
    assert_eq!(filtered.len(), 2, "revoked suppression filtered candidates");
}
