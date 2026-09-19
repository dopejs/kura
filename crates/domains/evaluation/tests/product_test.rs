//! Port of daemon/internal/evaluation/product_fixture_test.go,
//! product_fixture_revision_test.go, product_fixture_validation_test.go,
//! product_redaction_test.go, product_validation_test.go, and
//! fixtures_test.go (the product-side fixture immutability coverage).

mod common;

use chrono::DateTime;
use chrono::Utc;
use kura_evaluation::{
    CandidateEvidenceInput, DEFAULT_PRODUCT_PAGE_LIMIT, EvaluationError, FixtureDomainClass,
    FixtureReviewDecision, FixtureRevisionInput, MAX_PRODUCT_PAGE_LIMIT, ProductFixtureInput,
    ProductLifecycleStatus, RedactionPolicy, RedactionStatus, RetentionState, SourceKind,
    apply_product_fixture_retention, candidate_evidence_from_payload,
    create_product_fixture_from_candidate, create_product_fixture_revision,
    ensure_product_fixture_editable, failed_closed_redacted_evidence, normalize_product_limit,
    product_fixture_selectable, redact_evidence_payload, reject_repo_managed_fixture_edit,
    review_product_fixture, suppress_product_fixture, validate_discovery_policy,
    validate_product_fixture_payload, validate_tenant_scoped_product_request,
};

use common::{fixture_candidate, fixture_evidence};

fn ts(s: &str) -> DateTime<Utc> {
    s.parse::<DateTime<Utc>>().expect("ts")
}

fn obj(json: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    json.as_object().cloned().expect("object")
}

#[test]
fn product_fixture_lifecycle_create_review_suppress_retention() {
    let now = ts("2026-04-29T10:00:00Z");
    let (fixture, revision) = create_product_fixture_from_candidate(
        ProductFixtureInput {
            tenant_id: "ten_eval".to_string(),
            display_name: "Schedule Product Fixture".to_string(),
            domain_class: FixtureDomainClass::Schedule,
            source_candidate: fixture_candidate(now),
            source_evidence: fixture_evidence(now),
            fixture_payload: obj(serde_json::json!({ "goal": "schedule follow-up" })),
            change_summary: "initial product fixture".to_string(),
            created_by: "prn_eval".to_string(),
            ..Default::default()
        },
        now,
    )
    .expect("CreateProductFixtureFromCandidate");
    assert_eq!(fixture.review_state, ProductLifecycleStatus::Draft);
    assert_eq!(fixture.current_revision_id, revision.revision_id);
    assert_eq!(revision.revision_number, 1);
    assert_eq!(revision.source_evidence_refs[0], "evidence_1");

    let (revised, second_revision) = create_product_fixture_revision(
        fixture.clone(),
        FixtureRevisionInput {
            fixture_payload: obj(serde_json::json!({ "goal": "schedule revised follow-up" })),
            content_summary: "revised schedule fixture".to_string(),
            change_summary: "tighten expectation".to_string(),
            source_evidence_refs: vec!["evidence_1".to_string()],
            redaction_status: RedactionStatus::Redacted,
            created_by: "prn_eval".to_string(),
            ..Default::default()
        },
        2,
        now + chrono::Duration::minutes(1),
    )
    .expect("CreateProductFixtureRevision");
    assert_eq!(second_revision.revision_number, 2);
    assert_eq!(revised.current_revision_id, second_revision.revision_id);
    assert_eq!(revised.review_state, ProductLifecycleStatus::Draft);

    let approved = review_product_fixture(
        revised,
        &second_revision.revision_id,
        FixtureReviewDecision::Approved,
        now + chrono::Duration::minutes(2),
    )
    .expect("ReviewProductFixture");
    product_fixture_selectable(&approved).expect("approved fixture should be selectable");

    let suppressed = suppress_product_fixture(approved, now + chrono::Duration::minutes(3))
        .expect("SuppressProductFixture");
    let err =
        product_fixture_selectable(&suppressed).expect_err("suppressed must not be selectable");
    assert!(matches!(err, EvaluationError::ProductFixtureNotSelectable));

    let deleted = apply_product_fixture_retention(
        fixture,
        RetentionState::Deleted,
        now + chrono::Duration::minutes(4),
    )
    .expect("ApplyProductFixtureRetention");
    assert_eq!(deleted.review_state, ProductLifecycleStatus::Deleted);
    assert_eq!(deleted.retention_state, RetentionState::Deleted);
}

#[test]
fn product_fixture_creation_rejects_suppressed_expired_or_failed_evidence() {
    let now = ts("2026-04-29T10:00:00Z");
    let mut candidate = fixture_candidate(now);
    candidate.suppression_state = kura_evaluation::SuppressionState::Suppressed;
    let err = create_product_fixture_from_candidate(
        ProductFixtureInput {
            tenant_id: "ten_eval".to_string(),
            display_name: "Suppressed".to_string(),
            domain_class: FixtureDomainClass::Schedule,
            source_candidate: candidate,
            source_evidence: fixture_evidence(now),
            fixture_payload: serde_json::Map::new(),
            ..Default::default()
        },
        now,
    )
    .expect_err("suppressed candidate must not be selectable");
    assert!(matches!(err, EvaluationError::ProductFixtureNotSelectable));

    let candidate = fixture_candidate(now);
    let mut evidence = fixture_evidence(now);
    evidence.materialization_allowed = false;
    let err = create_product_fixture_from_candidate(
        ProductFixtureInput {
            tenant_id: "ten_eval".to_string(),
            display_name: "Failed Redaction".to_string(),
            domain_class: FixtureDomainClass::Schedule,
            source_candidate: candidate,
            source_evidence: evidence,
            fixture_payload: serde_json::Map::new(),
            ..Default::default()
        },
        now,
    )
    .expect_err("failed redaction must not be selectable");
    assert!(matches!(err, EvaluationError::ProductFixtureNotSelectable));
}

#[test]
fn product_fixture_revision_is_immutable_and_payload_is_copied() {
    let now = ts("2026-04-29T10:00:00Z");
    let (fixture, mut revision) = create_product_fixture_from_candidate(
        ProductFixtureInput {
            tenant_id: "ten_eval".to_string(),
            display_name: "Schedule Product Fixture".to_string(),
            domain_class: FixtureDomainClass::Schedule,
            source_candidate: fixture_candidate(now),
            source_evidence: fixture_evidence(now),
            fixture_payload: obj(serde_json::json!({ "goal": "initial" })),
            created_by: "prn_eval".to_string(),
            ..Default::default()
        },
        now,
    )
    .expect("CreateProductFixtureFromCandidate");
    revision.fixture_payload.insert(
        "goal".to_string(),
        serde_json::json!("mutated after return"),
    );

    let (_revised, second_revision) = create_product_fixture_revision(
        fixture,
        FixtureRevisionInput {
            fixture_payload: obj(serde_json::json!({ "goal": "second" })),
            content_summary: "second revision".to_string(),
            source_evidence_refs: vec!["evidence_1".to_string()],
            created_by: "prn_eval".to_string(),
            ..Default::default()
        },
        2,
        now + chrono::Duration::minutes(1),
    )
    .expect("CreateProductFixtureRevision");
    assert_eq!(second_revision.revision_number, 2);
    assert_ne!(second_revision.revision_id, revision.revision_id);
    assert_eq!(second_revision.source_evidence_refs[0], "evidence_1");
    assert_eq!(second_revision.tenant_id, "ten_eval");
}

#[test]
fn validate_product_fixture_payload_materializes_only_redacted_payload() {
    let now = ts("2026-04-29T10:00:00Z");
    let result = validate_product_fixture_payload(CandidateEvidenceInput {
        tenant_id: "ten_eval".to_string(),
        discovered_candidate_id: "candidate_1".to_string(),
        payload: obj(serde_json::json!({
            "goal": "safe",
            "access_token": "secret",
            "custom_secret": "tenant secret"
        })),
        redaction_policy: RedactionPolicy {
            sensitive_field_rules: vec!["custom_secret".to_string()],
        },
        now,
        ..Default::default()
    })
    .expect("ValidateProductFixturePayload");
    assert_eq!(result.status, RedactionStatus::Redacted);
    assert!(!result.payload.contains_key("access_token"));
    assert!(!result.payload.contains_key("custom_secret"));
    assert!(result.evidence.materialization_allowed);
}

#[test]
fn redact_evidence_payload_removes_sensitive_fields_before_persist() {
    let payload = obj(serde_json::json!({
        "safe": "value",
        "access_token": "secret-token",
        "Authorization": "Bearer secret",
        "nested": { "refresh_token": "secret-refresh", "count": 1 }
    }));
    let redacted = redact_evidence_payload(
        &payload,
        &RedactionPolicy {
            sensitive_field_rules: vec!["custom_secret".to_string()],
        },
    );
    assert_eq!(redacted.status, RedactionStatus::Redacted);
    assert!(!redacted.payload.contains_key("access_token"));
    assert!(!redacted.payload.contains_key("Authorization"));
    let nested = redacted
        .payload
        .get("nested")
        .and_then(|v| v.as_object())
        .expect("nested object");
    assert!(!nested.contains_key("refresh_token"));
    assert_eq!(nested.get("count").and_then(|v| v.as_i64()), Some(1));
    assert_eq!(redacted.sensitive_fields_excluded.len(), 3);
}

#[test]
fn redact_evidence_payload_handles_nested_arrays_and_configured_fields() {
    let payload = obj(serde_json::json!({
        "events": [
            { "credential": "secret", "message": "ok" },
            { "custom_secret": "tenant-secret", "count": 2 }
        ],
        "profile": { "api-key": "secret", "name": "safe" }
    }));
    let redacted = redact_evidence_payload(
        &payload,
        &RedactionPolicy {
            sensitive_field_rules: vec!["custom_secret".to_string(), "api-key".to_string()],
        },
    );
    assert_eq!(redacted.status, RedactionStatus::Redacted);
    let events = redacted
        .payload
        .get("events")
        .and_then(|v| v.as_array())
        .expect("events");
    assert!(
        !events[0]
            .as_object()
            .expect("obj")
            .contains_key("credential")
    );
    assert!(
        !events[1]
            .as_object()
            .expect("obj")
            .contains_key("custom_secret")
    );
    let profile = redacted
        .payload
        .get("profile")
        .and_then(|v| v.as_object())
        .expect("profile");
    assert!(!profile.contains_key("api-key"));
    assert_eq!(redacted.sensitive_fields_excluded.len(), 3);
}

#[test]
fn failed_closed_redacted_evidence_carries_reason_without_payload() {
    let redacted = failed_closed_redacted_evidence("evaluation.redaction_failed");
    assert_eq!(redacted.status, RedactionStatus::Failed);
    assert!(redacted.payload.is_empty());
    assert_eq!(
        redacted.redaction_rules_applied,
        vec!["failed_closed".to_string()]
    );
    assert_eq!(
        redacted.sensitive_fields_excluded,
        vec!["evaluation.redaction_failed".to_string()]
    );
}

#[test]
fn candidate_evidence_from_payload_redacts_before_persist() {
    let now = ts("2026-04-29T10:00:00Z");
    let evidence = candidate_evidence_from_payload(CandidateEvidenceInput {
        tenant_id: "ten_eval".to_string(),
        discovered_candidate_id: "candidate_1".to_string(),
        source_refs: vec![kura_evaluation::SourceRef {
            kind: SourceKind::Run,
            id: "run_1".to_string(),
            ..Default::default()
        }],
        summary: " candidate evidence ".to_string(),
        payload: obj(serde_json::json!({
            "safe": "value",
            "sessionToken": "secret",
            "nested": { "custom_sensitive": "tenant secret", "status": "failed" }
        })),
        redaction_policy: RedactionPolicy {
            sensitive_field_rules: vec!["custom_sensitive".to_string()],
        },
        now,
        ..Default::default()
    })
    .expect("CandidateEvidenceFromPayload");
    assert_eq!(evidence.evidence_id, "evidence_candidate_1");
    assert_eq!(evidence.created_at, now);
    assert!(evidence.materialization_allowed);
    assert_eq!(evidence.retention_state, RetentionState::Active);
    assert!(!evidence.redacted_payload.contains_key("sessionToken"));
    let nested = evidence
        .redacted_payload
        .get("nested")
        .and_then(|v| v.as_object())
        .expect("nested");
    assert!(!nested.contains_key("custom_sensitive"));
    assert_eq!(evidence.sensitive_fields_excluded.len(), 2);
}

#[test]
fn candidate_evidence_from_payload_requires_candidate_source() {
    let err = candidate_evidence_from_payload(CandidateEvidenceInput {
        tenant_id: "ten_eval".to_string(),
        ..Default::default()
    })
    .expect_err("candidate source required");
    assert!(matches!(err, EvaluationError::ProductSourceRequired));
}

#[test]
fn validate_tenant_scoped_product_request_requires_tenant() {
    assert!(matches!(
        validate_tenant_scoped_product_request(""),
        Err(EvaluationError::ProductTenantRequired)
    ));
    validate_tenant_scoped_product_request("ten_eval").expect("tenant must pass");
}

#[test]
fn normalize_product_limit_bounds_lists() {
    assert_eq!(normalize_product_limit(0), DEFAULT_PRODUCT_PAGE_LIMIT);
    assert_eq!(normalize_product_limit(-1), DEFAULT_PRODUCT_PAGE_LIMIT);
    assert_eq!(normalize_product_limit(25), 25);
    assert_eq!(
        normalize_product_limit(MAX_PRODUCT_PAGE_LIMIT + 1),
        MAX_PRODUCT_PAGE_LIMIT
    );
}

#[test]
fn validate_discovery_policy_rejects_bounds_and_window_ordering() {
    let now = ts("2026-04-29T10:00:00Z");
    let policy = kura_evaluation::DiscoveryPolicy {
        policy_id: "policy_1".to_string(),
        tenant_id: "ten_eval".to_string(),
        enabled: true,
        source_kinds: vec![SourceKind::Run],
        window_start: now,
        window_end: now - chrono::Duration::hours(1),
        max_inspected_records: 10,
        max_emitted_candidates: 2,
        cost_budget: 5,
        ..Default::default()
    };
    let err = validate_discovery_policy(&policy).expect_err("invalid window must fail");
    assert!(matches!(err, EvaluationError::ProductBoundsInvalid));
}

#[test]
fn repo_managed_fixture_cannot_be_edited_through_product_path() {
    let now = ts("2026-04-29T10:00:00Z");
    let repo_fixture = kura_evaluation::ProductManagedFixture {
        fixture_id: "fixture_repo_schedule".to_string(),
        tenant_id: "ten_eval".to_string(),
        display_name: "Repo Fixture".to_string(),
        domain_class: FixtureDomainClass::Schedule,
        source_kind: SourceKind::Fixture.as_str().to_string(),
        review_state: ProductLifecycleStatus::Approved,
        suppression_state: kura_evaluation::SuppressionState::None,
        retention_state: RetentionState::Active,
        created_at: now,
        updated_at: now,
        ..Default::default()
    };
    let err = ensure_product_fixture_editable(&repo_fixture).expect_err("repo fixture immutable");
    assert!(matches!(err, EvaluationError::RepoFixtureImmutable));
    let err = reject_repo_managed_fixture_edit("repo_fixture").expect_err("repo fixture immutable");
    assert!(matches!(err, EvaluationError::RepoFixtureImmutable));
    let err = create_product_fixture_revision(
        repo_fixture,
        FixtureRevisionInput {
            fixture_payload: serde_json::Map::new(),
            ..Default::default()
        },
        2,
        now,
    )
    .expect_err("repo fixture immutable");
    assert!(matches!(err, EvaluationError::RepoFixtureImmutable));
}
