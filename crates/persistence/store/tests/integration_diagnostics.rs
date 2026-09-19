//! Round-trip integration tests for the integration diagnostics DAOs ported from
//! `daemon/internal/store/integration_diagnostics.go` into
//! `integration_diagnostics.rs`. Ports of
//! TestIntegrationDiagnosticStorePersistsLatestStateAndRetention,
//! TestIntegrationDiagnosticStoreMarksStaleAndHidesExpiredResults,
//! TestIntegrationDiagnosticStoreListsAndGetsRunsByTenant, and
//! TestDiagnosticRetentionRecordsTrackExpiredEvidence.

use chrono::{Duration, TimeZone, Utc};
use kura_integrations::{
    DiagnosticReasonCode, DiagnosticResult, DiagnosticResultFilter, DiagnosticRetentionState,
    DiagnosticRun, DiagnosticRunFilter, DiagnosticRunStatus, DiagnosticStatus, FreshnessState,
    RedactionStatus, RemediationOwner, RetrySafety, new_diagnostic_retention_record,
};
use kura_store::SQLiteStore;

fn temp_dir(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("kura_store_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().to_string()
}

#[test]
fn diagnostic_run_and_result_persist_with_latest_state_and_retention() {
    let dir = temp_dir("integration_diag_latest");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 5, 10, 10, 0, 0).unwrap();
    let stale_after = now + Duration::minutes(15);

    let run = DiagnosticRun {
        diagnostic_run_id: "diag_run_store_1".to_string(),
        tenant_id: "ten_diag".to_string(),
        integration_id: "integration_diag".to_string(),
        requested_by: "operator".to_string(),
        trigger: "operator_inspection".to_string(),
        status: DiagnosticRunStatus::Completed,
        started_at: now,
        checked_capabilities: vec!["calendar.read".to_string()],
        result_ids: vec!["diag_result_store_1".to_string()],
        redaction_status: RedactionStatus::Redacted,
        retention_expires_at: now + Duration::days(90),
        idempotency_key: "client_key_1".to_string(),
        ..DiagnosticRun::default()
    };
    store.save_integration_diagnostic_run(&run).unwrap();

    let result = DiagnosticResult {
        diagnostic_result_id: "diag_result_store_1".to_string(),
        tenant_id: "ten_diag".to_string(),
        integration_id: "integration_diag".to_string(),
        domain_kind: "calendar".to_string(),
        provider_kind: "feishu_lark".to_string(),
        capability: "calendar.read".to_string(),
        status: DiagnosticStatus::Healthy,
        reason_code: DiagnosticReasonCode::Healthy,
        remediation_owner: RemediationOwner::NoneRequired,
        retry_safety: RetrySafety::NoActionNeeded,
        checked_at: now,
        stale_after,
        freshness_state: FreshnessState::Fresh,
        run_id: run.diagnostic_run_id.clone(),
        redaction_status: RedactionStatus::Redacted,
        retention_expires_at: now + Duration::days(90),
        ..DiagnosticResult::default()
    };
    store.save_integration_diagnostic_result(&result).unwrap();

    let got = store
        .latest_integration_diagnostic_results(
            &DiagnosticResultFilter {
                tenant_id: "ten_diag".to_string(),
                integration_id: "integration_diag".to_string(),
                limit: 10,
                ..DiagnosticResultFilter::default()
            },
            now,
        )
        .unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].diagnostic_result_id, "diag_result_store_1");
    assert_eq!(got[0].status, DiagnosticStatus::Healthy);
    assert_eq!(got[0].freshness_state, FreshnessState::Fresh);
    assert_eq!(got[0].run_id, "diag_run_store_1");
}

#[test]
fn diagnostic_results_mark_stale_and_hide_expired() {
    let dir = temp_dir("integration_diag_stale");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 5, 10, 10, 0, 0).unwrap();
    let old_checked_at = now - Duration::minutes(20);
    let expired_checked_at = now - Duration::hours(2);

    let stale_result = DiagnosticResult {
        diagnostic_result_id: "diag_result_stale".to_string(),
        tenant_id: "ten_diag".to_string(),
        integration_id: "integration_diag".to_string(),
        domain_kind: "calendar".to_string(),
        provider_kind: "feishu_lark".to_string(),
        capability: "calendar.read".to_string(),
        status: DiagnosticStatus::Blocked,
        reason_code: DiagnosticReasonCode::ScopeMissing,
        remediation_owner: RemediationOwner::TenantAdmin,
        retry_safety: RetrySafety::Blocked,
        checked_at: old_checked_at,
        stale_after: old_checked_at + Duration::minutes(15),
        freshness_state: FreshnessState::Fresh,
        redaction_status: RedactionStatus::Redacted,
        retention_expires_at: now + Duration::hours(1),
        ..DiagnosticResult::default()
    };
    store
        .save_integration_diagnostic_result(&stale_result)
        .unwrap();

    let expired_result = DiagnosticResult {
        diagnostic_result_id: "diag_result_expired".to_string(),
        checked_at: expired_checked_at,
        stale_after: expired_checked_at + Duration::minutes(15),
        retention_expires_at: now - Duration::minutes(1),
        ..stale_result.clone()
    };
    store
        .save_integration_diagnostic_result(&expired_result)
        .unwrap();

    // Expired rows are hidden; the retained stale result is refreshed to Stale.
    let visible = store
        .latest_integration_diagnostic_results(
            &DiagnosticResultFilter {
                tenant_id: "ten_diag".to_string(),
                integration_id: "integration_diag".to_string(),
                include_expired: false,
                ..DiagnosticResultFilter::default()
            },
            now,
        )
        .unwrap();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].diagnostic_result_id, "diag_result_stale");
    assert_eq!(visible[0].freshness_state, FreshnessState::Stale);

    let all = store
        .latest_integration_diagnostic_results(
            &DiagnosticResultFilter {
                tenant_id: "ten_diag".to_string(),
                integration_id: "integration_diag".to_string(),
                include_expired: true,
                limit: 10,
                ..DiagnosticResultFilter::default()
            },
            now,
        )
        .unwrap();
    assert_eq!(all.len(), 2);
}

#[test]
fn diagnostic_runs_list_and_get_are_tenant_scoped() {
    let dir = temp_dir("integration_diag_tenant");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 5, 10, 10, 0, 0).unwrap();

    for run in [
        DiagnosticRun {
            diagnostic_run_id: "diag_run_a".to_string(),
            tenant_id: "ten_a".to_string(),
            integration_id: "integration_a".to_string(),
            requested_by: "operator".to_string(),
            trigger: "operator_inspection".to_string(),
            status: DiagnosticRunStatus::Completed,
            started_at: now,
            redaction_status: RedactionStatus::Redacted,
            retention_expires_at: now + Duration::hours(1),
            ..DiagnosticRun::default()
        },
        DiagnosticRun {
            diagnostic_run_id: "diag_run_b".to_string(),
            tenant_id: "ten_b".to_string(),
            integration_id: "integration_b".to_string(),
            requested_by: "operator".to_string(),
            trigger: "operator_inspection".to_string(),
            status: DiagnosticRunStatus::Completed,
            started_at: now,
            redaction_status: RedactionStatus::Redacted,
            retention_expires_at: now + Duration::hours(1),
            ..DiagnosticRun::default()
        },
    ] {
        store.save_integration_diagnostic_run(&run).unwrap();
    }

    let items = store
        .list_integration_diagnostic_runs(
            &DiagnosticRunFilter {
                tenant_id: "ten_a".to_string(),
                ..DiagnosticRunFilter::default()
            },
            now,
        )
        .unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].diagnostic_run_id, "diag_run_a");
    assert_eq!(items[0].tenant_id, "ten_a");

    // Tenant A must not read tenant B's run.
    let cross = store
        .get_integration_diagnostic_run("ten_a", "diag_run_b", false, now)
        .unwrap();
    assert!(cross.is_none());

    let own = store
        .get_integration_diagnostic_run("ten_a", "diag_run_a", false, now)
        .unwrap();
    assert_eq!(own.expect("run found").diagnostic_run_id, "diag_run_a");
}

#[test]
fn retention_records_track_and_apply_expired_evidence() {
    let dir = temp_dir("integration_diag_retention");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 5, 10, 10, 0, 0).unwrap();
    let created_at = now - Duration::days(91);

    let record = new_diagnostic_retention_record(
        "ten_diag",
        "diagnostic_run",
        "diag_run_expired",
        created_at,
    );
    store.save_diagnostic_retention_record(&record).unwrap();

    let expired = store
        .expired_diagnostic_retention_records("ten_diag", now, 10)
        .unwrap();
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].target_id, "diag_run_expired");
    assert_eq!(expired[0].retention_state, DiagnosticRetentionState::Active);

    let applied = store
        .apply_expired_diagnostic_retention_records("ten_diag", now, 10)
        .unwrap();
    assert_eq!(applied.len(), 1);
    assert_eq!(
        applied[0].retention_state,
        DiagnosticRetentionState::Expired
    );
    assert!(applied[0].applied_at.is_some());

    let after = store
        .expired_diagnostic_retention_records("ten_diag", now, 10)
        .unwrap();
    assert!(
        after.is_empty(),
        "applied retention records hidden from expiry query"
    );
}
