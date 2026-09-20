//! Behavioral tests for the evaluation-product DAOs
//! (rs/store/src/evaluation_product.rs), ported from
//! daemon/internal/store/evaluation_product_*_test.go: discovery policy/run
//! round-trips with tenant scoping, candidate + evidence persistence, product
//! fixtures, replay campaigns and items, dashboard projections, tool-call
//! inspections, suppression, and retention application.

use chrono::{Duration, Utc};
use kura_evaluation::{
    CampaignItem, DashboardProjection, DiscoveredCandidate, DiscoveryPolicy, DiscoveryRun,
    ProductLifecycleStatus, ProductListFilter, ProductManagedFixture, ProductResourceKind,
    ReadinessStatus, ReplayCampaign, RetentionState, ScoreBand, SourceKind, SuppressionRecord,
    SuppressionState, ToolCallInspection,
};
use kura_store::SQLiteStore;

fn temp_dir(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("kura_store_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().to_string()
}

fn policy_fixture(policy_id: &str, tenant_id: &str) -> DiscoveryPolicy {
    let now = Utc::now();
    DiscoveryPolicy {
        policy_id: policy_id.to_string(),
        tenant_id: tenant_id.to_string(),
        enabled: true,
        source_kinds: vec![SourceKind::Run],
        window_start: now,
        window_end: now + Duration::days(7),
        max_inspected_records: 100,
        max_emitted_candidates: 10,
        cost_budget: 500,
        created_by: "prn_1".to_string(),
        created_at: now,
        updated_at: now,
        ..DiscoveryPolicy::default()
    }
}

fn run_fixture(run_id: &str, tenant_id: &str) -> DiscoveryRun {
    let now = Utc::now();
    DiscoveryRun {
        discovery_run_id: run_id.to_string(),
        tenant_id: tenant_id.to_string(),
        policy_id: String::new(),
        status: ProductLifecycleStatus::Running,
        cursor: "cursor_1".to_string(),
        source_kinds: vec![SourceKind::Run, SourceKind::Workflow],
        window_start: now,
        window_end: now + Duration::days(7),
        max_inspected_records: 100,
        max_emitted_candidates: 10,
        cost_budget: 500,
        inspected_records: 42,
        emitted_candidates: 3,
        started_by: "prn_1".to_string(),
        started_at: now,
        completed_at: None,
        updated_at: now,
        idempotency_key: "idem_1".to_string(),
        ..DiscoveryRun::default()
    }
}

fn candidate_fixture(candidate_id: &str, tenant_id: &str, run_id: &str) -> DiscoveredCandidate {
    let now = Utc::now();
    DiscoveredCandidate {
        discovered_candidate_id: candidate_id.to_string(),
        tenant_id: tenant_id.to_string(),
        discovery_run_id: run_id.to_string(),
        source_kind: SourceKind::Run,
        source_id: "run_42".to_string(),
        score: 0.87,
        score_band: ScoreBand::High,
        redaction_status: kura_evaluation::RedactionStatus::Redacted,
        readiness_status: ReadinessStatus::FullyReplayable,
        suppression_state: SuppressionState::None,
        retention_state: RetentionState::Active,
        created_at: now,
        updated_at: now,
        expires_at: None,
        ..DiscoveredCandidate::default()
    }
}

fn campaign_fixture(campaign_id: &str, tenant_id: &str) -> ReplayCampaign {
    let now = Utc::now();
    ReplayCampaign {
        campaign_id: campaign_id.to_string(),
        tenant_id: tenant_id.to_string(),
        display_name: "June Replay".to_string(),
        status: ProductLifecycleStatus::Queued,
        created_at: now,
        started_at: None,
        completed_at: None,
        published_at: None,
        retention_state: RetentionState::Active,
        idempotency_key: "campaign_idem".to_string(),
        ..ReplayCampaign::default()
    }
}

#[test]
fn discovery_policy_upsert_list_and_tenant_scope() {
    let dir = temp_dir("eval_discovery_policy");
    let store = SQLiteStore::new(&dir).unwrap();

    let mut policy = policy_fixture("policy_1", "ten_1");
    store.upsert_discovery_policy(policy.clone()).unwrap();

    // Upsert flips enabled + limits; tenant preserved via COALESCE.
    policy.enabled = false;
    policy.max_inspected_records = 200;
    policy.updated_at = Utc::now();
    store.upsert_discovery_policy(policy.clone()).unwrap();

    let got = store
        .get_discovery_policy("ten_1", "policy_1")
        .unwrap()
        .expect("present");
    assert_eq!(got.policy_id, "policy_1");
    assert_eq!(got.tenant_id, "ten_1");
    assert!(!got.enabled);
    assert_eq!(got.max_inspected_records, 200);

    let filter = kura_evaluation::DiscoveryPolicyFilter {
        base: ProductListFilter {
            tenant_id: "ten_1".to_string(),
            cursor: String::new(),
            limit: 50,
        },
        enabled: None,
    };
    let listed = store.list_discovery_policies(&filter).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].policy_id, "policy_1");

    let disabled_only = kura_evaluation::DiscoveryPolicyFilter {
        enabled: Some(false),
        ..filter.clone()
    };
    assert_eq!(
        store.list_discovery_policies(&disabled_only).unwrap().len(),
        1
    );
    let enabled_only = kura_evaluation::DiscoveryPolicyFilter {
        enabled: Some(true),
        ..filter.clone()
    };
    assert!(
        store
            .list_discovery_policies(&enabled_only)
            .unwrap()
            .is_empty()
    );

    // Cross-tenant isolation.
    assert!(
        store
            .get_discovery_policy("ten_2", "policy_1")
            .unwrap()
            .is_none()
    );
    let other = kura_evaluation::DiscoveryPolicyFilter {
        base: ProductListFilter {
            tenant_id: "ten_2".to_string(),
            ..Default::default()
        },
        enabled: None,
    };
    assert!(store.list_discovery_policies(&other).unwrap().is_empty());
}

#[test]
fn discovery_run_candidate_and_evidence_round_trip() {
    let dir = temp_dir("eval_discovery_run");
    let store = SQLiteStore::new(&dir).unwrap();

    store
        .save_discovery_run(run_fixture("run_1", "ten_1"))
        .unwrap();
    let mut run = run_fixture("run_1", "ten_1");
    run.status = ProductLifecycleStatus::Completed;
    run.inspected_records = 50;
    run.completed_at = Some(Utc::now());
    store.save_discovery_run(run.clone()).unwrap();

    let got_run = store
        .get_discovery_run("ten_1", "run_1")
        .unwrap()
        .expect("present");
    assert_eq!(got_run.status, ProductLifecycleStatus::Completed);
    assert_eq!(got_run.inspected_records, 50);
    assert!(got_run.completed_at.is_some());

    let run_filter = kura_evaluation::DiscoveryRunFilter {
        base: ProductListFilter {
            tenant_id: "ten_1".to_string(),
            cursor: String::new(),
            limit: 50,
        },
        status: ProductLifecycleStatus::Completed,
        source_kind: SourceKind::default(),
    };
    let runs = store.list_discovery_runs(&run_filter).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].discovery_run_id, "run_1");

    // Source-kind post filter narrows the list.
    let workflow_filter = kura_evaluation::DiscoveryRunFilter {
        source_kind: SourceKind::Schedule,
        ..run_filter.clone()
    };
    assert!(
        store
            .list_discovery_runs(&workflow_filter)
            .unwrap()
            .is_empty()
    );
    let run_only = kura_evaluation::DiscoveryRunFilter {
        source_kind: SourceKind::Run,
        ..run_filter.clone()
    };
    assert_eq!(store.list_discovery_runs(&run_only).unwrap().len(), 1);

    // Candidate + evidence save (transactional).
    let mut candidate = candidate_fixture("cand_1", "ten_1", "run_1");
    let evidence = kura_evaluation::CandidateEvidence {
        evidence_id: "ev_1".to_string(),
        tenant_id: "ten_1".to_string(),
        discovered_candidate_id: "cand_1".to_string(),
        redaction_rules_applied: vec!["mask:token".to_string()],
        materialization_allowed: true,
        retention_state: RetentionState::Active,
        created_at: Utc::now(),
        expires_at: None,
        ..kura_evaluation::CandidateEvidence::default()
    };
    store
        .save_discovered_candidate(candidate.clone(), evidence.clone())
        .unwrap();
    candidate.evidence_ref = "ev_1".to_string();
    candidate.score = 0.99;
    store
        .save_discovered_candidate(candidate.clone(), evidence.clone())
        .unwrap();

    let got_candidate = store
        .get_discovered_candidate("ten_1", "cand_1")
        .unwrap()
        .expect("present");
    assert_eq!(got_candidate.discovery_run_id, "run_1");
    assert_eq!(got_candidate.score, 0.99);
    assert_eq!(got_candidate.evidence_ref, "ev_1");

    let candidate_filter = kura_evaluation::DiscoveredCandidateFilter {
        base: ProductListFilter {
            tenant_id: "ten_1".to_string(),
            cursor: String::new(),
            limit: 50,
        },
        discovery_run_id: "run_1".to_string(),
        source_kind: SourceKind::default(),
        readiness_status: ReadinessStatus::default(),
        suppression_state: SuppressionState::default(),
        score_band: ScoreBand::default(),
    };
    let candidates = store.list_discovered_candidates(&candidate_filter).unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(
        candidates[0].readiness_status,
        ReadinessStatus::FullyReplayable
    );

    let latest_evidence = store
        .get_latest_candidate_evidence("ten_1", "cand_1")
        .unwrap()
        .expect("evidence present");
    assert_eq!(latest_evidence.evidence_id, "ev_1");
    assert_eq!(latest_evidence.retention_state, RetentionState::Active);

    // Cross-tenant isolation.
    assert!(
        store
            .get_discovered_candidate("ten_2", "cand_1")
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .get_latest_candidate_evidence("ten_2", "cand_1")
            .unwrap()
            .is_none()
    );
}

#[test]
fn product_fixture_revision_and_campaign_round_trip() {
    let dir = temp_dir("eval_fixture_campaign");
    let store = SQLiteStore::new(&dir).unwrap();

    // Product-managed fixture upsert.
    let mut fixture = ProductManagedFixture {
        fixture_id: "fx_1".to_string(),
        tenant_id: "ten_1".to_string(),
        display_name: "Terminal fixture".to_string(),
        domain_class: kura_evaluation::FixtureDomainClass::Integration,
        source_kind: "run".to_string(),
        source_candidate_id: "cand_1".to_string(),
        review_state: ProductLifecycleStatus::InReview,
        suppression_state: SuppressionState::None,
        retention_state: RetentionState::Active,
        created_by: "prn_1".to_string(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        ..ProductManagedFixture::default()
    };
    store.upsert_product_fixture(fixture.clone()).unwrap();
    fixture.display_name = "Terminal fixture v2".to_string();
    fixture.review_state = ProductLifecycleStatus::Approved;
    store.upsert_product_fixture(fixture.clone()).unwrap();

    let got = store
        .get_product_fixture("ten_1", "fx_1")
        .unwrap()
        .expect("present");
    assert_eq!(got.display_name, "Terminal fixture v2");
    assert_eq!(got.review_state, ProductLifecycleStatus::Approved);
    assert_eq!(got.source_candidate_id, "cand_1");

    let filter = ProductListFilter {
        tenant_id: "ten_1".to_string(),
        cursor: String::new(),
        limit: 50,
    };
    let fixtures = store.list_product_fixtures(&filter).unwrap();
    assert_eq!(fixtures.len(), 1);
    assert_eq!(fixtures[0].fixture_id, "fx_1");
    assert!(
        store
            .get_product_fixture("ten_2", "fx_1")
            .unwrap()
            .is_none()
    );

    // Fixture revisions.
    store
        .save_fixture_revision(kura_evaluation::FixtureRevision {
            revision_id: "rev_1".to_string(),
            fixture_id: "fx_1".to_string(),
            tenant_id: "ten_1".to_string(),
            revision_number: 1,
            redaction_status: kura_evaluation::RedactionStatus::Redacted,
            created_by: "prn_1".to_string(),
            created_at: Utc::now(),
            ..kura_evaluation::FixtureRevision::default()
        })
        .unwrap();
    let revisions = store.list_fixture_revisions("ten_1", "fx_1", 10).unwrap();
    assert_eq!(revisions.len(), 1);
    assert_eq!(revisions[0].revision_number, 1);

    // Replay campaign + items.
    store
        .save_replay_campaign(campaign_fixture("camp_1", "ten_1"))
        .unwrap();
    let campaigns = store.list_replay_campaigns(&filter).unwrap();
    assert_eq!(campaigns.len(), 1);
    assert_eq!(campaigns[0].display_name, "June Replay");

    store
        .save_campaign_item(CampaignItem {
            campaign_item_id: "ci_1".to_string(),
            campaign_id: "camp_1".to_string(),
            tenant_id: "ten_1".to_string(),
            source_type: ProductResourceKind::DiscoveredCandidate,
            source_id: "cand_1".to_string(),
            suppression_checked_at: Utc::now(),
            created_at: Utc::now(),
            ..CampaignItem::default()
        })
        .unwrap();
    let items = store.list_campaign_items(&filter, "camp_1").unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].campaign_item_id, "ci_1");
    let other_campaign = store.list_campaign_items(&filter, "camp_other").unwrap();
    assert!(other_campaign.is_empty());
    assert!(
        store
            .get_replay_campaign("ten_2", "camp_1")
            .unwrap()
            .is_none()
    );
}

#[test]
fn dashboard_projection_and_tool_call_inspection() {
    let dir = temp_dir("eval_dashboard_inspection");
    let store = SQLiteStore::new(&dir).unwrap();
    store
        .save_replay_campaign(campaign_fixture("camp_1", "ten_1"))
        .unwrap();
    store
        .save_campaign_item(CampaignItem {
            campaign_item_id: "ci_1".to_string(),
            campaign_id: "camp_1".to_string(),
            tenant_id: "ten_1".to_string(),
            source_type: ProductResourceKind::DiscoveredCandidate,
            source_id: "cand_1".to_string(),
            suppression_checked_at: Utc::now(),
            created_at: Utc::now(),
            ..CampaignItem::default()
        })
        .unwrap();

    let now = Utc::now();
    let projection = DashboardProjection {
        projection_id: "proj_1".to_string(),
        tenant_id: "ten_1".to_string(),
        window_start: now,
        window_end: now + Duration::days(7),
        generated_at: now,
        retention_state: RetentionState::Active,
        ..DashboardProjection::default()
    };
    store.save_dashboard_projection(projection).unwrap();
    let got_proj = store
        .get_dashboard_projection("ten_1", "proj_1")
        .unwrap()
        .expect("present");
    assert_eq!(got_proj.projection_id, "proj_1");
    let filter = ProductListFilter {
        tenant_id: "ten_1".to_string(),
        cursor: String::new(),
        limit: 50,
    };
    let projections = store.list_dashboard_projections(&filter).unwrap();
    assert_eq!(projections.len(), 1);

    // Tool-call inspection for a campaign item.
    let inspection = ToolCallInspection {
        inspection_id: "ins_1".to_string(),
        tenant_id: "ten_1".to_string(),
        campaign_id: "camp_1".to_string(),
        campaign_item_id: "ci_1".to_string(),
        tool_call_ref: "tool_call_7".to_string(),
        classification: kura_evaluation::INSPECTION_MATCHED.to_string(),
        redaction_status: kura_evaluation::RedactionStatus::Redacted,
        retention_state: RetentionState::Active,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        ..ToolCallInspection::default()
    };
    store.save_tool_call_inspection(inspection.clone()).unwrap();
    let inspections = store.list_tool_call_inspections(&filter, "camp_1").unwrap();
    assert_eq!(inspections.len(), 1);
    assert_eq!(
        inspections[0].classification,
        kura_evaluation::INSPECTION_MATCHED
    );
    let got_ins = store
        .get_tool_call_inspection("ten_1", "ins_1")
        .unwrap()
        .expect("present");
    assert_eq!(got_ins.campaign_item_id, "ci_1");
    // Retention filter: listing only returns active rows.
    assert!(
        store
            .list_tool_call_inspections(&filter, "camp_other")
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .get_tool_call_inspection("ten_2", "ins_1")
            .unwrap()
            .is_none()
    );
}

#[test]
fn suppression_and_retention_application() {
    let dir = temp_dir("eval_suppression_retention");
    let store = SQLiteStore::new(&dir).unwrap();

    store
        .create_suppression(SuppressionRecord {
            suppression_id: "sup_1".to_string(),
            tenant_id: "ten_1".to_string(),
            target_kind: ProductResourceKind::DiscoveredCandidate,
            target_id: "cand_1".to_string(),
            reason_code: "operator_suppressed".to_string(),
            created_by: "prn_1".to_string(),
            created_at: Utc::now(),
            expires_at: None,
            active: true,
            ..SuppressionRecord::default()
        })
        .unwrap();
    // Upsert path flips active.
    store
        .create_suppression(SuppressionRecord {
            suppression_id: "sup_1".to_string(),
            tenant_id: "ten_1".to_string(),
            target_kind: ProductResourceKind::DiscoveredCandidate,
            target_id: "cand_1".to_string(),
            reason_code: "operator_suppressed".to_string(),
            created_by: "prn_1".to_string(),
            created_at: Utc::now(),
            expires_at: None,
            active: false,
            ..SuppressionRecord::default()
        })
        .unwrap();

    // Seed a candidate so retention has a row to expire.
    store
        .save_discovery_run(run_fixture("run_1", "ten_1"))
        .unwrap();
    store
        .save_discovered_candidate(
            candidate_fixture("cand_1", "ten_1", "run_1"),
            kura_evaluation::CandidateEvidence::default(),
        )
        .unwrap();

    // Dry run records applications without expiring.
    let dry_ids = store
        .apply_retention(&kura_evaluation::RetentionApplicationFilter {
            base: ProductListFilter {
                tenant_id: "ten_1".to_string(),
                cursor: String::new(),
                limit: 50,
            },
            resource_kinds: vec![ProductResourceKind::DiscoveredCandidate],
            dry_run: true,
        })
        .unwrap();
    assert_eq!(dry_ids.len(), 1);
    let before = store
        .get_discovered_candidate("ten_1", "cand_1")
        .unwrap()
        .unwrap();
    assert_eq!(before.retention_state, RetentionState::Active);

    // Real retention expires the candidate and records the application.
    let ids = store
        .apply_retention(&kura_evaluation::RetentionApplicationFilter {
            base: ProductListFilter {
                tenant_id: "ten_1".to_string(),
                cursor: String::new(),
                limit: 50,
            },
            resource_kinds: vec![ProductResourceKind::DiscoveredCandidate],
            dry_run: false,
        })
        .unwrap();
    assert_eq!(ids.len(), 1);
    let after = store
        .get_discovered_candidate("ten_1", "cand_1")
        .unwrap()
        .unwrap();
    assert_eq!(after.retention_state, RetentionState::Expired);
    assert!(after.expires_at.is_some());

    // Default kind set expires all product families.
    let all_ids = store
        .apply_retention(&kura_evaluation::RetentionApplicationFilter {
            base: ProductListFilter {
                tenant_id: "ten_1".to_string(),
                cursor: String::new(),
                limit: 50,
            },
            resource_kinds: Vec::new(),
            dry_run: true,
        })
        .unwrap();
    assert_eq!(all_ids.len(), 6, "six product resource kinds by default");
}
