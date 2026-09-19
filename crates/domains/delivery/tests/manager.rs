//! Manager/dispatcher integration tests (ports of dispatcher_test.go): retry without
//! failover, restore-resume, terminal failure diagnostics, suppression, and CRUD.

mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use kura_delivery::{
    DeliveryAdapter, DeliveryPreference, DeliveryTarget, OutcomeInput, OutcomeStatus,
    PreferenceScopeKind, ResultClass, SuppressionPolicy, TargetKind, TargetStatus,
};
use kura_events::Bus;
use kura_integrations::DiagnosticReasonCode;
use kura_store::delivery::DeliveryAttemptRecord;

use common::{
    ScriptedAdapter, manager_with, seed_delivery_preference_state, store, wait_for_outcome_status,
};

#[test]
fn delivery_retries_without_failover_and_retains_attempt_history() {
    let store = store("retry");
    let adapter = ScriptedAdapter::new(
        TargetKind::TestSink,
        vec![Err("transient send failure".to_string())],
    );
    let manager = kura_delivery::Manager::new(
        "test",
        Bus::new(),
        Arc::clone(&store),
        vec![Arc::clone(&adapter) as Arc<dyn DeliveryAdapter>],
    );
    manager.configure_for_testing(3, Duration::from_millis(10), Duration::from_millis(20));

    let (primary, mut pref) = seed_delivery_preference_state(&manager, "primary-target");
    let secondary = manager
        .create_target(DeliveryTarget {
            target_id: "secondary-target".to_string(),
            display_name: "Secondary".to_string(),
            target_kind: TargetKind::TestSink,
            environment_scope: "test".to_string(),
            ..DeliveryTarget::default()
        })
        .unwrap();
    pref.preferred_targets_by_class
        .insert(ResultClass::Failure, primary.target_id.clone());
    manager.upsert_preference(pref).unwrap();

    let outcome = manager
        .emit_outcome(OutcomeInput {
            source_kind: "run".to_string(),
            source_id: "run_retry".to_string(),
            run_id: "run_retry".to_string(),
            result_class: ResultClass::Failure,
            payload_preview: "retry me".to_string(),
            ..OutcomeInput::default()
        })
        .unwrap();
    assert_eq!(
        outcome.status,
        OutcomeStatus::Queued,
        "expected queued after first retryable failure: {outcome:?}"
    );

    let final_outcome =
        wait_for_outcome_status(&manager, &outcome.delivery_id, OutcomeStatus::Delivered);
    assert_eq!(
        final_outcome.attempts.len(),
        2,
        "expected two attempts: {final_outcome:?}"
    );
    assert_eq!(
        final_outcome.attempts[0].status,
        kura_delivery::AttemptStatus::RetryableFailure
    );
    assert_eq!(
        final_outcome.attempts[1].status,
        kura_delivery::AttemptStatus::Delivered
    );
    assert_eq!(final_outcome.attempts[0].target_id, primary.target_id);
    assert_eq!(final_outcome.attempts[1].target_id, primary.target_id);
    assert_ne!(final_outcome.attempts[0].target_id, secondary.target_id);
    assert_ne!(final_outcome.attempts[1].target_id, secondary.target_id);
}

#[test]
fn delivery_restore_resumes_queued_attempt() {
    let store = store("restore");

    // First manager fails once and schedules a far-future retry; the test rewrites the
    // attempt next_retry_at through the store, exactly like the Go test.
    let first_adapter = ScriptedAdapter::new(
        TargetKind::TestSink,
        vec![Err("transient send failure".to_string())],
    );
    let first = kura_delivery::Manager::new(
        "test",
        Bus::new(),
        Arc::clone(&store),
        vec![Arc::clone(&first_adapter) as Arc<dyn DeliveryAdapter>],
    );
    first.configure_for_testing(3, Duration::from_secs(3600), Duration::from_secs(7200));
    let (target, _) = seed_delivery_preference_state(&first, "restore-target");

    let outcome = first
        .emit_outcome(OutcomeInput {
            source_kind: "run".to_string(),
            source_id: "run_restore".to_string(),
            run_id: "run_restore".to_string(),
            result_class: ResultClass::Failure,
            payload_preview: "restore me".to_string(),
            ..OutcomeInput::default()
        })
        .unwrap();
    assert_eq!(outcome.status, OutcomeStatus::Queued);
    let (outcome, ok) = first.get_outcome(&outcome.delivery_id).unwrap();
    assert!(ok);
    let mut attempt = outcome.attempts[0].clone();
    attempt.next_retry_at = Some(Utc::now() + chrono::Duration::milliseconds(20));
    store
        .lock()
        .upsert_delivery_attempt(&DeliveryAttemptRecord {
            attempt_id: attempt.attempt_id.clone(),
            delivery_id: attempt.delivery_id.clone(),
            attempt_number: attempt.attempt_number,
            target_id: attempt.target_id.clone(),
            status: attempt.status.as_str().to_string(),
            next_retry_at: attempt.next_retry_at,
            document: serde_json::to_string(&attempt).unwrap(),
        })
        .unwrap();

    let second_adapter = ScriptedAdapter::new(TargetKind::TestSink, Vec::new());
    let second = kura_delivery::Manager::new(
        "test",
        Bus::new(),
        Arc::clone(&store),
        vec![Arc::clone(&second_adapter) as Arc<dyn DeliveryAdapter>],
    );
    second.configure_for_testing(3, Duration::from_millis(10), Duration::from_millis(20));
    second.restore().unwrap();

    let final_outcome =
        wait_for_outcome_status(&second, &outcome.delivery_id, OutcomeStatus::Delivered);
    assert_eq!(
        second_adapter.sends(),
        1,
        "expected one resumed send after restore"
    );
    assert_eq!(
        final_outcome.attempts.len(),
        2,
        "expected retained attempt history across restore"
    );
    assert_eq!(final_outcome.attempts[1].target_id, target.target_id);
}

#[test]
fn delivery_terminal_failure_projects_diagnostic_failure() {
    let (manager, _store) = manager_with(vec![ScriptedAdapter::new(
        TargetKind::TestSink,
        vec![Err("network timeout".to_string())],
    )]);
    manager.configure_for_testing(1, Duration::from_secs(5), Duration::from_secs(60));
    seed_delivery_preference_state(&manager, "terminal-target");

    let outcome = manager
        .emit_outcome(OutcomeInput {
            source_kind: "run".to_string(),
            source_id: "run_terminal".to_string(),
            run_id: "run_terminal".to_string(),
            result_class: ResultClass::Failure,
            integration_id: "delivery-integration".to_string(),
            payload_preview: "terminal failure".to_string(),
            ..OutcomeInput::default()
        })
        .unwrap();
    assert_eq!(outcome.status, OutcomeStatus::Failed);
    let diagnostic = outcome
        .diagnostic_failure
        .expect("expected diagnostic failure projection");
    assert_eq!(diagnostic.reason_code, DiagnosticReasonCode::NetworkFailed);
}

#[test]
fn delivery_suppression_policy_suppresses_failure_class() {
    let (manager, _store) = manager_with(Vec::new());
    let (target, _) = seed_delivery_preference_state(&manager, "suppress-target");
    let mut by_class = HashMap::new();
    by_class.insert(ResultClass::Failure, target.target_id.clone());
    manager
        .upsert_preference(DeliveryPreference {
            preference_id: "pref-suppress".to_string(),
            environment_scope: "test".to_string(),
            scope_kind: PreferenceScopeKind::UserDefault,
            preferred_targets_by_class: by_class,
            suppression_policy: Some(SuppressionPolicy {
                suppress_failure: true,
                ..SuppressionPolicy::default()
            }),
            ..DeliveryPreference::default()
        })
        .unwrap();

    let outcome = manager
        .emit_outcome(OutcomeInput {
            source_kind: "run".to_string(),
            source_id: "run_suppressed".to_string(),
            run_id: "run_suppressed".to_string(),
            result_class: ResultClass::Failure,
            payload_preview: "do not send".to_string(),
            ..OutcomeInput::default()
        })
        .unwrap();
    assert_eq!(outcome.mode, kura_delivery::DeliveryMode::Suppressed);
    assert_eq!(outcome.status, OutcomeStatus::Suppressed);
    assert_eq!(
        outcome.suppression_reason,
        "failure result suppressed by policy"
    );
}

#[test]
fn delivery_no_active_preference_is_suppressed() {
    let (manager, _store) = manager_with(Vec::new());
    let outcome = manager
        .emit_outcome(OutcomeInput {
            source_kind: "run".to_string(),
            source_id: "run_no_pref".to_string(),
            result_class: ResultClass::Urgent,
            ..OutcomeInput::default()
        })
        .unwrap();
    assert_eq!(outcome.mode, kura_delivery::DeliveryMode::Suppressed);
    assert_eq!(outcome.status, OutcomeStatus::Suppressed);
    assert_eq!(outcome.suppression_reason, "no active delivery preference");
}

#[test]
fn delivery_non_active_target_fails_without_retry() {
    let (manager, _store) =
        manager_with(vec![ScriptedAdapter::new(TargetKind::TestSink, Vec::new())]);
    let (target, _) = seed_delivery_preference_state(&manager, "disabled-target");
    let (updated, ok) = manager
        .update_target_status(&target.target_id, TargetStatus::Disabled)
        .unwrap();
    assert!(ok);
    assert_eq!(updated.status, TargetStatus::Disabled);

    let outcome = manager
        .emit_outcome(OutcomeInput {
            source_kind: "run".to_string(),
            source_id: "run_disabled_target".to_string(),
            result_class: ResultClass::Failure,
            ..OutcomeInput::default()
        })
        .unwrap();
    assert_eq!(outcome.status, OutcomeStatus::Failed);
    assert_eq!(outcome.attempts.len(), 1);
    assert_eq!(outcome.attempts[0].failure_class, "target_unavailable");
}

#[test]
fn delivery_emit_is_idempotent_per_source() {
    let (manager, _store) =
        manager_with(vec![ScriptedAdapter::new(TargetKind::TestSink, Vec::new())]);
    seed_delivery_preference_state(&manager, "idem-target");
    let input = OutcomeInput {
        source_kind: "run".to_string(),
        source_id: "run_idem".to_string(),
        result_class: ResultClass::Urgent,
        ..OutcomeInput::default()
    };
    let first = manager.emit_outcome(input.clone()).unwrap();
    let second = manager.emit_outcome(input).unwrap();
    assert_eq!(
        first.delivery_id, second.delivery_id,
        "duplicate emit must reuse the outcome"
    );
}

#[test]
fn delivery_test_sink_records_messages() {
    use kura_delivery::TestSinkAdapter;
    let sink = Arc::new(TestSinkAdapter::new());
    let (manager, _store) = manager_with(vec![
        Arc::clone(&sink) as Arc<dyn kura_delivery::DeliveryAdapter>
    ]);
    let (target, _) = seed_delivery_preference_state(&manager, "sink-target");
    let outcome = manager
        .emit_outcome(OutcomeInput {
            source_kind: "run".to_string(),
            source_id: "run_sink".to_string(),
            run_id: "run_sink".to_string(),
            result_class: ResultClass::Urgent,
            payload_preview: "hello sink".to_string(),
            ..OutcomeInput::default()
        })
        .unwrap();
    assert_eq!(outcome.status, OutcomeStatus::Delivered);
    let messages = sink.messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].target_id, target.target_id);
    assert_eq!(messages[0].delivery_id, outcome.delivery_id);
    assert_eq!(messages[0].result_class, ResultClass::Urgent);
    assert_eq!(messages[0].payload_preview, "hello sink");
}

#[test]
fn delivery_target_and_preference_crud_roundtrips() {
    let (manager, _store) = manager_with(Vec::new());
    let created = manager
        .create_target(DeliveryTarget {
            target_id: "crud-target".to_string(),
            display_name: "CRUD".to_string(),
            target_kind: TargetKind::TestSink,
            environment_scope: "test".to_string(),
            ..DeliveryTarget::default()
        })
        .unwrap();
    assert!(created.supports_immediate);
    assert!(created.supports_digest, "test sink targets support digest");
    assert_eq!(created.environment_scope, "test");
    assert_eq!(created.status, TargetStatus::Active);

    let listed = manager.list_targets().unwrap();
    assert_eq!(listed.len(), 1);
    let (got, ok) = manager.get_target("crud-target").unwrap();
    assert!(ok);
    assert_eq!(got.display_name, "CRUD");
    let (missing, ok) = manager.get_target("nope").unwrap();
    assert!(!ok);
    assert!(missing.target_id.is_empty());

    let pref = manager
        .upsert_preference(DeliveryPreference {
            preference_id: "pref-crud".to_string(),
            environment_scope: "test".to_string(),
            scope_kind: PreferenceScopeKind::UserDefault,
            preferred_targets_by_class: HashMap::new(),
            ..DeliveryPreference::default()
        })
        .unwrap();
    assert!(pref.active, "upserted preferences are activated");
    let (got_pref, ok) = manager.get_preference("pref-crud").unwrap();
    assert!(ok);
    assert_eq!(got_pref.preference_id, "pref-crud");
    assert_eq!(manager.list_preferences().unwrap().len(), 1);
}

#[test]
fn delivery_missing_required_fields_error() {
    let (manager, _store) = manager_with(Vec::new());
    let err = manager
        .create_target(DeliveryTarget {
            display_name: "No ID".to_string(),
            ..DeliveryTarget::default()
        })
        .unwrap_err();
    assert_eq!(err, kura_delivery::DeliveryError::TargetIdRequired);
    let err = manager
        .create_target(DeliveryTarget {
            target_id: "no-display".to_string(),
            ..DeliveryTarget::default()
        })
        .unwrap_err();
    assert_eq!(err, kura_delivery::DeliveryError::DisplayNameRequired);
    let err = manager
        .upsert_preference(DeliveryPreference::default())
        .unwrap_err();
    assert_eq!(err, kura_delivery::DeliveryError::PreferenceIdRequired);
}
