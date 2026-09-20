//! Trait-surface tests for `kura_livevalidation::Store` implemented by
//! `LiveValidationStoreHandle` (the Send + Sync newtype over the SQLite
//! store). Exercises the exact async trait methods the live-validation
//! manager calls, including the ledger outcome update path and the dynamic
//! filters.

use std::sync::Arc;

use chrono::Utc;
use kura_livevalidation::{
    AmbiguousCommit, AmbiguousCommitCause, ApprovalStatus, ApprovalTarget, Attempt, AttemptFilter,
    AttemptStatus, Comparison, ComparisonFilter, ComparisonStatus, FreshApproval, KillSwitch,
    KillSwitchFilter, KillSwitchScope, LedgerFilter, LedgerOutcome, MatrixRow,
    ReconciliationResolution, ReconciliationResolutionValue, RetentionAppliesTo, RetentionMode,
    RetentionPolicy, SafetyClass, SideEffectLedgerEntry, SideEffectScope, Store, ToolClass,
};
use kura_store::{LiveValidationStoreHandle, SQLiteStore};

fn temp_dir(name: &str) -> String {
    let dir =
        std::env::temp_dir().join(format!("kura_store_lv_trait_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().to_string()
}

fn make_attempt(validation_id: &str) -> Attempt {
    let now = Utc::now();
    Attempt {
        validation_id: validation_id.to_string(),
        tenant_id: "ten_1".to_string(),
        candidate_id: "cand_1".to_string(),
        source_attempt_id: String::new(),
        requested_by: "trait_user".to_string(),
        environment_scope: "test".to_string(),
        requested_scope: SideEffectScope::default(),
        status: AttemptStatus::from("queued"),
        permission_decision: Default::default(),
        quota_decision: Default::default(),
        kill_switch_decision: Default::default(),
        approval_summary: Default::default(),
        ledger_summary: Default::default(),
        comparison_id: String::new(),
        created_at: now,
        started_at: None,
        completed_at: None,
        updated_at: now,
    }
}

fn make_scope() -> SideEffectScope {
    SideEffectScope {
        scope_id: "scope_1".to_string(),
        validation_id: "lv_1".to_string(),
        included_tool_classes: vec![ToolClass::from("mcp.tool_call")],
        excluded_tool_classes: Vec::new(),
        included_actions: vec!["read".to_string()],
        excluded_actions: Vec::new(),
        approval_mode: kura_livevalidation::ApprovalMode::from("scope_level"),
        declared_by: "trait_user".to_string(),
        declared_at: Utc::now(),
    }
}

fn make_approval() -> FreshApproval {
    let now = Utc::now();
    FreshApproval {
        approval_id: "apr_1".to_string(),
        validation_id: "lv_1".to_string(),
        tenant_id: "ten_1".to_string(),
        approval_target: ApprovalTarget::from("scope"),
        tool_class: ToolClass::from("mcp.tool_call"),
        safety_class: SafetyClass::from("idempotent_mutation"),
        action_ref: String::new(),
        approved_scope: String::new(),
        status: ApprovalStatus::from("approved"),
        requested_by: "trait_user".to_string(),
        resolved_by: "trait_admin".to_string(),
        requested_at: now,
        resolved_at: Some(now),
    }
}

fn make_ledger_entry(entry_id: &str, outcome: &str) -> SideEffectLedgerEntry {
    let now = Utc::now();
    SideEffectLedgerEntry {
        ledger_entry_id: entry_id.to_string(),
        validation_id: "lv_1".to_string(),
        tenant_id: "ten_1".to_string(),
        candidate_id: "cand_1".to_string(),
        source_ref: "source".to_string(),
        tool_class: ToolClass::from("mcp.tool_call"),
        safety_class: SafetyClass::from("idempotent_mutation"),
        action_ref: "action_1".to_string(),
        approval_id: String::new(),
        correlation_key: String::new(),
        downstream_ref: String::new(),
        outcome: LedgerOutcome::from(outcome),
        reason_code: String::new(),
        attempted_at: Some(now),
        completed_at: None,
        updated_at: now,
        evidence_refs: Vec::new(),
        retry_count: 0,
        ambiguous_commit: false,
        reconciliation_id: String::new(),
    }
}

fn make_kill_switch(id: &str) -> KillSwitch {
    KillSwitch {
        kill_switch_id: id.to_string(),
        scope: KillSwitchScope::from("global"),
        tenant_id: "ten_1".to_string(),
        enabled: true,
        reason: "trait test".to_string(),
        changed_by: "trait_user".to_string(),
        changed_at: Utc::now(),
        expires_at: None,
    }
}

fn make_matrix_row() -> MatrixRow {
    MatrixRow {
        tool_class: ToolClass::from("mcp.tool_call"),
        safety_class: SafetyClass::from("idempotent_mutation"),
        permission: "live_validation.mcp.tool_call".to_string(),
        approval: kura_livevalidation::MatrixApproval::from("scope_level"),
        approval_action: String::new(),
        idempotency: "idempotent".to_string(),
        retry_policy: kura_livevalidation::RetryPolicy::from("manual_retry"),
        ambiguous_commit_behavior: "reconcile".to_string(),
        compensation: kura_livevalidation::CompensationKind::from("not_applicable"),
        ledger_events: vec![LedgerOutcome::from("completed")],
        test_case: "test_case_1".to_string(),
        version: "v1".to_string(),
    }
}

fn make_ambiguous_commit() -> AmbiguousCommit {
    let now = Utc::now();
    AmbiguousCommit {
        ambiguous_commit_id: "amb_1".to_string(),
        ledger_entry_id: "ledger_1".to_string(),
        validation_id: "lv_1".to_string(),
        tenant_id: "ten_1".to_string(),
        cause: AmbiguousCommitCause::from("timeout"),
        last_known_request_ref: String::new(),
        automatic_retry_stopped: true,
        created_at: now,
        updated_at: now,
    }
}

fn make_reconciliation() -> ReconciliationResolution {
    ReconciliationResolution {
        reconciliation_id: "rec_1".to_string(),
        ambiguous_commit_id: "amb_1".to_string(),
        tenant_id: "ten_1".to_string(),
        resolved_by: "trait_admin".to_string(),
        resolution: ReconciliationResolutionValue::from("confirmed_committed"),
        reason: "evidence".to_string(),
        evidence_refs: Vec::new(),
        resolved_at: Utc::now(),
    }
}

fn make_comparison() -> Comparison {
    Comparison {
        comparison_id: "cmp_1".to_string(),
        validation_id: "lv_1".to_string(),
        candidate_id: "cand_1".to_string(),
        baseline_ref: "baseline".to_string(),
        terminal_status: ComparisonStatus::from("matched"),
        ledger_summary: Default::default(),
        unsupported_classes: Vec::new(),
        denials: Vec::new(),
        ambiguous_commits: Vec::new(),
        drift_findings: Vec::new(),
        generated_at: Utc::now(),
    }
}

fn make_retention_policy() -> RetentionPolicy {
    RetentionPolicy {
        policy_id: "pol_1".to_string(),
        tenant_id: "ten_1".to_string(),
        applies_to: RetentionAppliesTo::from("all"),
        mode: RetentionMode::from("indefinite"),
        retention_period: String::new(),
        created_by_principal_id: "trait_admin".to_string(),
        reason: String::new(),
        created_at: Utc::now(),
        expires_at: None,
    }
}

#[tokio::test]
async fn live_validation_store_trait_attempt_and_scope_round_trip() {
    let dir = temp_dir("attempt");
    let store = SQLiteStore::new(&dir).unwrap();
    let handle = Arc::new(LiveValidationStoreHandle::new(store));

    let mut attempt = make_attempt("lv_1");
    handle.upsert_attempt(attempt.clone()).await.unwrap();
    attempt.status = AttemptStatus::from("running");
    handle.upsert_attempt(attempt).await.unwrap();

    let got = handle
        .get_attempt("ten_1", "lv_1")
        .await
        .unwrap()
        .expect("attempt");
    assert_eq!(got.validation_id, "lv_1");
    assert_eq!(got.status.as_str(), "running");
    assert_eq!(handle.get_attempt("ten_other", "lv_1").await.unwrap(), None);

    let by_candidate = AttemptFilter {
        candidate_id: "cand_1".to_string(),
        ..Default::default()
    };
    assert_eq!(
        handle
            .list_attempts(by_candidate.clone())
            .await
            .unwrap()
            .len(),
        1
    );
    let by_status = AttemptFilter {
        status: AttemptStatus::from("blocked"),
        ..Default::default()
    };
    assert!(handle.list_attempts(by_status).await.unwrap().is_empty());

    handle.upsert_scope(make_scope(), "ten_1").await.unwrap();
}

#[tokio::test]
async fn live_validation_store_trait_ledger_and_kill_switch() {
    let dir = temp_dir("ledger");
    let store = SQLiteStore::new(&dir).unwrap();
    let handle = Arc::new(LiveValidationStoreHandle::new(store));

    handle.upsert_attempt(make_attempt("lv_1")).await.unwrap();
    handle.upsert_approval(make_approval()).await.unwrap();

    let mut entry = make_ledger_entry("ledger_1", "attempted");
    handle.append_ledger_entry(entry.clone()).await.unwrap();
    // Idempotent re-append (same id) must not duplicate rows.
    handle.append_ledger_entry(entry.clone()).await.unwrap();
    let all = handle
        .list_ledger_entries(LedgerFilter::default())
        .await
        .unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].outcome.as_str(), "attempted");

    // Update the outcome through the trait; the persisted document follows.
    handle
        .update_ledger_entry_outcome("ledger_1", &LedgerOutcome::from("completed"), "done")
        .await
        .unwrap();
    let updated = handle
        .list_ledger_entries(LedgerFilter {
            outcome: LedgerOutcome::from("completed"),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0].reason_code, "done");
    assert!(updated[0].completed_at.is_some());
    // The old-outcome filter misses after the transition.
    let old = handle
        .list_ledger_entries(LedgerFilter {
            outcome: LedgerOutcome::from("attempted"),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(old.is_empty());

    handle
        .upsert_kill_switch(make_kill_switch("ks_1"))
        .await
        .unwrap();
    let mut disabled = make_kill_switch("ks_1");
    disabled.enabled = false;
    handle.upsert_kill_switch(disabled).await.unwrap();
    let enabled = handle
        .list_kill_switches(KillSwitchFilter {
            enabled: Some(true),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(enabled.is_empty());
    let all_switches = handle
        .list_kill_switches(KillSwitchFilter::default())
        .await
        .unwrap();
    assert_eq!(all_switches.len(), 1);
    assert!(!all_switches[0].enabled);
}

#[tokio::test]
async fn live_validation_store_trait_matrix_ambiguous_comparison_retention() {
    let dir = temp_dir("matrix");
    let store = SQLiteStore::new(&dir).unwrap();
    let handle = Arc::new(LiveValidationStoreHandle::new(store));

    handle
        .upsert_support_matrix_snapshot("ten_1", "snap_1", vec![make_matrix_row()])
        .await
        .unwrap();
    // The ambiguous commit and reconciliation rows reference the ledger entry
    // via foreign keys, so seed the attempt + ledger entry first.
    handle.upsert_attempt(make_attempt("lv_1")).await.unwrap();
    handle
        .append_ledger_entry(make_ledger_entry("ledger_1", "attempted"))
        .await
        .unwrap();
    handle
        .save_ambiguous_commit(make_ambiguous_commit())
        .await
        .unwrap();
    handle
        .save_reconciliation_resolution(make_reconciliation())
        .await
        .unwrap();

    let mut comparison = make_comparison();
    handle.save_comparison(comparison.clone()).await.unwrap();
    comparison.terminal_status = ComparisonStatus::from("drifted");
    handle.save_comparison(comparison).await.unwrap();
    let by_status = ComparisonFilter {
        terminal_status: ComparisonStatus::from("drifted"),
        ..Default::default()
    };
    assert_eq!(handle.list_comparisons(by_status).await.unwrap().len(), 1);

    handle
        .save_retention_policy(make_retention_policy())
        .await
        .unwrap();
}
