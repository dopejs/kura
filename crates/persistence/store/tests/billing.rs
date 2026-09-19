//! Behavioral tests for the billing DAOs (rs/store/src/billing.rs) and the
//! `kura_billing::Repository` trait surface through `BillingRepositoryHandle`.
//! The reserve/commit/denial flows run through `kura_billing::Manager` so the
//! route-facing accounting path is exercised against a real SQLite store.

use std::sync::Arc;

use chrono::{Duration, Utc};
use kura_billing::{
    AbuseRestrictionRecord, AbuseRestrictionStatus, BillingError, Category, EnforcementMode,
    Manager, ManualAdjustment, PlanStatus, QuotaOverride, RecoveryAction, Repository, ReserveInput,
    ResolveInput, TenantPlan, definition_for, run_operation_key,
};
use kura_store::{BillingRepositoryHandle, SQLiteStore};

fn temp_dir(name: &str) -> String {
    let dir =
        std::env::temp_dir().join(format!("kura_store_billing_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().to_string()
}

fn plan_fixture(tenant_id: &str, plan_id: &str) -> TenantPlan {
    TenantPlan {
        plan_id: plan_id.to_string(),
        tenant_id: tenant_id.to_string(),
        plan_key: "test".to_string(),
        status: PlanStatus::from(PlanStatus::ACTIVE),
        enforcement_mode: EnforcementMode::from(EnforcementMode::ENFORCED),
        effective_at: Utc::now() - Duration::minutes(5),
        ..Default::default()
    }
}

fn reserve_input(tenant_id: &str, operation_key: &str, amount: i64) -> ReserveInput {
    ReserveInput {
        tenant_id: tenant_id.to_string(),
        category: Category::from(Category::RUN_LAUNCHES),
        amount,
        operation_key: operation_key.to_string(),
        reservation_point: "before".to_string(),
        guarded_entry_point: "/v1/runs".to_string(),
        actor_principal_id: "principal_1".to_string(),
        hosted: true,
    }
}

#[tokio::test]
async fn repository_reserve_commit_flow() {
    let dir = temp_dir("reserve_commit");
    let store = SQLiteStore::new(&dir).unwrap();
    let repo = Arc::new(BillingRepositoryHandle::new(store));
    repo.save_plan(plan_fixture("ten_1", "plan_1"))
        .await
        .expect("save plan");
    let manager = Manager::new(repo.clone());

    let operation_key = run_operation_key("ten_1", "", "run_1");
    let result = manager
        .reserve(reserve_input("ten_1", &operation_key, 1))
        .await
        .expect("reserve succeeds");
    assert!(result.allowed, "first launch within limit is allowed");
    let reservation = result.reservation.expect("reservation present");
    assert_eq!(reservation.amount_reserved, 1);
    assert_eq!(reservation.status.as_str(), "reserved");

    // Reservation is durable and findable by operation key.
    let stored = repo
        .reservation_by_operation(
            "ten_1",
            &Category::from(Category::RUN_LAUNCHES),
            &operation_key,
        )
        .await
        .expect("reservation lookup")
        .expect("reservation present");
    assert_eq!(stored.reservation_id, reservation.reservation_id);

    // The counter for the open period tracks the reservation.
    let definition = definition_for(&Category::from(Category::RUN_LAUNCHES)).unwrap();
    let period = repo
        .open_period("ten_1", &definition, Utc::now())
        .await
        .expect("open period");
    let counter = repo
        .usage_counter(
            "ten_1",
            &Category::from(Category::RUN_LAUNCHES),
            &period.quota_period_id,
        )
        .await
        .expect("counter lookup")
        .expect("counter present");
    assert_eq!(counter.reserved_amount, 1);

    // Pending reservations surface in the recovery sweep.
    let pending = repo
        .list_pending_reservations()
        .await
        .expect("pending reservations");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].operation_key, operation_key);

    // Commit the reservation through the manager.
    let committed = manager
        .commit(ResolveInput {
            tenant_id: "ten_1".to_string(),
            category: Category::from(Category::RUN_LAUNCHES),
            operation_key: operation_key.clone(),
            amount: 0,
            reason_code: String::new(),
            reason: "run finished".to_string(),
            actor_principal_id: "principal_1".to_string(),
        })
        .await
        .expect("commit succeeds");
    assert_eq!(committed.status.as_str(), "committed");

    let counter_after = repo
        .usage_counter(
            "ten_1",
            &Category::from(Category::RUN_LAUNCHES),
            &period.quota_period_id,
        )
        .await
        .expect("counter lookup")
        .expect("counter present");
    assert_eq!(counter_after.committed_amount, 1);
    assert_eq!(counter_after.reserved_amount, 0);

    // Evidence log carries reservation + commit events with a stable ref prefix.
    let refs = repo
        .list_usage_evidence_refs("ten_1", &operation_key, 10)
        .await
        .expect("evidence refs");
    assert_eq!(refs.len(), 2);
    assert!(refs.iter().all(|r| r.starts_with("billing_usage_event:")));
}

#[tokio::test]
async fn repository_denies_when_over_limit_and_records_denial() {
    let dir = temp_dir("deny");
    let store = SQLiteStore::new(&dir).unwrap();
    let repo = Arc::new(BillingRepositoryHandle::new(store));
    repo.save_plan(plan_fixture("ten_1", "plan_1"))
        .await
        .expect("save plan");
    let manager = Manager::new(repo.clone());

    // RUN_LAUNCHES catalog default limit is 1: the second launch must be denied.
    let operation_key = run_operation_key("ten_1", "", "run_1");
    let first = manager
        .reserve(reserve_input("ten_1", &operation_key, 1))
        .await
        .expect("reserve");
    assert!(first.allowed);

    let second_key = run_operation_key("ten_1", "", "run_2");
    let denied = manager
        .reserve(reserve_input("ten_1", &second_key, 1))
        .await
        .expect("reserve");
    assert!(!denied.allowed, "second launch over limit is denied");
    assert!(matches!(denied.failure, Some(BillingError::QuotaDenied)));
    assert!(denied.denial.is_some());

    // Denial is durable and lookup-able.
    let denials = repo
        .list_quota_denials("ten_1", 10)
        .await
        .expect("list denials");
    assert_eq!(denials.len(), 1);
    assert_eq!(denials[0].operation_key, second_key);
    assert_eq!(denials[0].requested_amount, 1);
    assert_eq!(denials[0].remaining_amount, 0);
    let by_id = repo
        .quota_denial_by_id("ten_1", &denials[0].denial_id)
        .await
        .expect("denial by id")
        .expect("denial present");
    assert_eq!(by_id.guarded_entry_point, "/v1/runs");
}

#[tokio::test]
async fn repository_quota_override_round_trip() {
    let dir = temp_dir("override");
    let store = SQLiteStore::new(&dir).unwrap();
    let repo = Arc::new(BillingRepositoryHandle::new(store));

    let override_ = QuotaOverride {
        quota_override_id: "qo_1".to_string(),
        tenant_id: "ten_1".to_string(),
        category: Category::from(Category::RUN_LAUNCHES),
        limit: Some(5),
        effective_at: Utc::now() - Duration::minutes(1),
        reason: "test grant".to_string(),
        ..Default::default()
    };
    repo.save_quota_override(override_)
        .await
        .expect("save override");

    let got = repo
        .quota_override("ten_1", &Category::from(Category::RUN_LAUNCHES), Utc::now())
        .await
        .expect("override lookup")
        .expect("override present");
    assert_eq!(got.limit, Some(5));
    assert_eq!(got.reason, "test grant");

    // An expired override is not effective.
    repo.save_quota_override(QuotaOverride {
        quota_override_id: "qo_expired".to_string(),
        tenant_id: "ten_1".to_string(),
        category: Category::from(Category::RUN_LAUNCHES),
        limit: Some(99),
        effective_at: Utc::now() - Duration::hours(2),
        expires_at: Some(Utc::now() - Duration::hours(1)),
        reason: "expired".to_string(),
        ..Default::default()
    })
    .await
    .expect("save expired override");
    let still = repo
        .quota_override("ten_1", &Category::from(Category::RUN_LAUNCHES), Utc::now())
        .await
        .expect("override lookup");
    assert_eq!(
        still.unwrap().limit,
        Some(5),
        "expired override ignored, newest effective wins"
    );
}

#[tokio::test]
async fn repository_abuse_restriction_and_manual_adjustment_round_trip() {
    let dir = temp_dir("abuse_adjust");
    let store = SQLiteStore::new(&dir).unwrap();
    let repo = Arc::new(BillingRepositoryHandle::new(store));

    let restriction = AbuseRestrictionRecord {
        restriction_id: "ab_1".to_string(),
        tenant_id: "ten_1".to_string(),
        status: AbuseRestrictionStatus::from(AbuseRestrictionStatus::ACTIVE),
        affected_category: Category::from(Category::RUN_LAUNCHES),
        recovery_action: RecoveryAction::from(RecoveryAction::WAIT),
        visible_reason_code: "manual_review".to_string(),
        started_at: Utc::now() - Duration::minutes(5),
        document: Some(serde_json::Map::from_iter([(
            "note".to_string(),
            serde_json::json!("flagged by test"),
        )])),
        ..Default::default()
    };
    // The Repository trait exposes list-only for abuse restrictions; the
    // save path is exercised through the DAO (Go SaveAbuseRestriction) via the
    // handle's public inner store.
    repo.0
        .lock()
        .billing_save_abuse_restriction(restriction)
        .expect("save restriction");
    let restrictions = repo
        .list_abuse_restrictions("ten_1", Utc::now())
        .await
        .expect("list restrictions");
    assert_eq!(restrictions.len(), 1);
    assert_eq!(restrictions[0].visible_reason_code, "manual_review");
    assert_eq!(
        restrictions[0].document.as_ref().unwrap()["note"],
        "flagged by test"
    );

    let adjustment = ManualAdjustment {
        adjustment_id: "adj_1".to_string(),
        tenant_id: "ten_1".to_string(),
        category: Category::from(Category::RUN_LAUNCHES),
        quota_period_id: "period_1".to_string(),
        amount_delta: 3,
        reason: "goodwill".to_string(),
        created_by_principal_id: "principal_1".to_string(),
        created_at: Utc::now(),
    };
    repo.save_manual_adjustment(adjustment)
        .await
        .expect("save adjustment");
    let adjustments = repo
        .list_manual_adjustments("ten_1", 10)
        .await
        .expect("list adjustments");
    assert_eq!(adjustments.len(), 1);
    assert_eq!(adjustments[0].amount_delta, 3);
}

#[tokio::test]
async fn repository_plan_supersede_on_active_save() {
    let dir = temp_dir("plan_supersede");
    let store = SQLiteStore::new(&dir).unwrap();
    let repo = Arc::new(BillingRepositoryHandle::new(store));

    repo.save_plan(plan_fixture("ten_1", "plan_v1"))
        .await
        .expect("save plan v1");
    // Saving a second active plan supersedes the first.
    repo.save_plan(plan_fixture("ten_1", "plan_v2"))
        .await
        .expect("save plan v2");

    let active = repo
        .active_plan("ten_1")
        .await
        .expect("active plan")
        .expect("plan present");
    assert_eq!(active.plan_id, "plan_v2");
    let v1 = repo
        .active_plan("other_tenant")
        .await
        .expect("no plan for other tenant");
    assert!(v1.is_none());
}
