//! Digest integration tests (port of digest_test.go): routine successes batch into one
//! summary window, urgent results bypass, and the window emits a digest delivery.

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use kura_delivery::{
    DeliveryMode, DeliveryPreference, DeliveryTarget, OutcomeInput, OutcomeStatus,
    PreferenceScopeKind, ResultClass, SummaryPolicy, TargetKind,
};
use kura_events::Bus;
use kura_store::delivery::DeliverySummaryWindowRecord;

use common::{ScriptedAdapter, store, wait_for_window_status};

fn seed_digest_preference_state(
    manager: &kura_delivery::Manager,
) -> (DeliveryTarget, DeliveryPreference) {
    let target = manager
        .create_target(DeliveryTarget {
            target_id: "digest-target".to_string(),
            display_name: "Digest Target".to_string(),
            target_kind: TargetKind::TestSink,
            environment_scope: "test".to_string(),
            ..DeliveryTarget::default()
        })
        .unwrap();
    let mut by_class = HashMap::new();
    by_class.insert(ResultClass::RoutineSuccess, target.target_id.clone());
    by_class.insert(ResultClass::Urgent, target.target_id.clone());
    by_class.insert(ResultClass::Failure, target.target_id.clone());
    let pref = manager
        .upsert_preference(DeliveryPreference {
            preference_id: "pref-digest".to_string(),
            environment_scope: "test".to_string(),
            scope_kind: PreferenceScopeKind::UserDefault,
            preferred_targets_by_class: by_class,
            summary_policy: Some(SummaryPolicy {
                routine_success_mode: Some(DeliveryMode::Digest),
                window_minutes: 1,
            }),
            ..DeliveryPreference::default()
        })
        .unwrap();
    (target, pref)
}

#[test]
fn digest_windows_batch_routine_success_and_urgent_bypasses() {
    let store = store("digest");
    let sink = Arc::new(kura_delivery::TestSinkAdapter::new());
    let manager = kura_delivery::Manager::new(
        "test",
        Bus::new(),
        Arc::clone(&store),
        vec![Arc::clone(&sink) as Arc<dyn kura_delivery::DeliveryAdapter>],
    );
    let (target, _) = seed_digest_preference_state(&manager);

    let first = manager
        .emit_outcome(OutcomeInput {
            source_kind: "run".to_string(),
            source_id: "run_success_1".to_string(),
            run_id: "run_success_1".to_string(),
            result_class: ResultClass::RoutineSuccess,
            payload_preview: "routine success 1".to_string(),
            ..OutcomeInput::default()
        })
        .unwrap();
    let second = manager
        .emit_outcome(OutcomeInput {
            source_kind: "run".to_string(),
            source_id: "run_success_2".to_string(),
            run_id: "run_success_2".to_string(),
            result_class: ResultClass::RoutineSuccess,
            payload_preview: "routine success 2".to_string(),
            ..OutcomeInput::default()
        })
        .unwrap();
    let urgent = manager
        .emit_outcome(OutcomeInput {
            source_kind: "run".to_string(),
            source_id: "run_urgent".to_string(),
            run_id: "run_urgent".to_string(),
            result_class: ResultClass::Urgent,
            payload_preview: "urgent".to_string(),
            ..OutcomeInput::default()
        })
        .unwrap();

    assert_eq!(first.mode, DeliveryMode::Digest);
    assert_eq!(second.mode, DeliveryMode::Digest);
    assert_eq!(first.status, OutcomeStatus::Queued);
    assert!(!first.summary_window_id.is_empty());
    assert_eq!(
        first.summary_window_id, second.summary_window_id,
        "routine successes share one window"
    );
    assert_eq!(urgent.status, OutcomeStatus::Delivered);
    assert_eq!(urgent.mode, DeliveryMode::Immediate);

    let (mut window, ok) = manager
        .get_summary_window(&first.summary_window_id)
        .unwrap();
    assert!(ok);
    assert_eq!(window.result_count, 2);

    // Advance the window past its deadline through the shared store, clear the scheduled
    // emission (a 1-minute thread is parked), and emit manually.
    window.window_ends_at = Utc::now() - chrono::Duration::seconds(1);
    window.updated_at = Utc::now();
    store
        .lock()
        .upsert_delivery_summary_window(&DeliverySummaryWindowRecord {
            summary_window_id: window.summary_window_id.clone(),
            environment_scope: window.environment_scope.clone(),
            target_id: window.target_id.clone(),
            preference_id: window.preference_id.clone(),
            status: window.status.as_str().to_string(),
            window_ends_at: window.window_ends_at,
            updated_at: window.updated_at,
            document: serde_json::to_string(&window).unwrap(),
        })
        .unwrap();
    manager.clear_window_schedule(&window.summary_window_id);
    manager.emit_window(&window.summary_window_id).unwrap();

    wait_for_window_status(
        &manager,
        &window.summary_window_id,
        kura_delivery::SummaryWindowStatus::Delivered,
    );
    let (delivered_window, _) = manager
        .get_summary_window(&window.summary_window_id)
        .unwrap();
    assert!(!delivered_window.emitted_delivery_id.is_empty());
    let (digest_outcome, ok) = manager
        .get_outcome(&delivered_window.emitted_delivery_id)
        .unwrap();
    assert!(ok);
    assert_eq!(digest_outcome.status, OutcomeStatus::Delivered);
    assert_eq!(digest_outcome.chosen_target_id, target.target_id);
    assert_eq!(digest_outcome.source_kind, "summary_window");
    assert_eq!(
        digest_outcome.payload_preview,
        "digest summary with 2 routed results"
    );

    // The digest delivery itself lands in the sink (2 batched results -> 1 emitted message).
    let messages = sink.messages();
    assert_eq!(
        messages.len(),
        2,
        "one urgent immediate + one digest emission: {messages:?}"
    );
    assert_eq!(
        messages[1].payload_preview,
        "digest summary with 2 routed results"
    );
}

#[test]
fn digest_empty_window_is_cancelled() {
    let (manager, store) = common::manager_with(Vec::new());
    // A window with no routed results (result_count 0) created directly through the store.
    let now = Utc::now();
    let window = kura_delivery::SummaryWindow {
        summary_window_id: "window_empty".to_string(),
        environment_scope: "test".to_string(),
        target_id: "t".to_string(),
        preference_id: "p".to_string(),
        status: kura_delivery::SummaryWindowStatus::Open,
        window_started_at: now,
        window_ends_at: now - chrono::Duration::seconds(1),
        result_count: 0,
        created_at: now,
        updated_at: now,
        ..kura_delivery::SummaryWindow::default()
    };
    store
        .lock()
        .upsert_delivery_summary_window(&DeliverySummaryWindowRecord {
            summary_window_id: window.summary_window_id.clone(),
            environment_scope: window.environment_scope.clone(),
            target_id: window.target_id.clone(),
            preference_id: window.preference_id.clone(),
            status: window.status.as_str().to_string(),
            window_ends_at: window.window_ends_at,
            updated_at: window.updated_at,
            document: serde_json::to_string(&window).unwrap(),
        })
        .unwrap();
    manager.emit_window("window_empty").unwrap();
    let (window, _) = manager.get_summary_window("window_empty").unwrap();
    assert_eq!(window.status, kura_delivery::SummaryWindowStatus::Cancelled);
}

#[test]
fn digest_restore_rearms_open_and_ready_windows() {
    let (manager, _store) =
        common::manager_with(vec![ScriptedAdapter::new(TargetKind::TestSink, Vec::new())]);
    seed_digest_preference_state(&manager);
    let outcome = manager
        .emit_outcome(OutcomeInput {
            source_kind: "run".to_string(),
            source_id: "run_digest_restore".to_string(),
            result_class: ResultClass::RoutineSuccess,
            ..OutcomeInput::default()
        })
        .unwrap();
    assert_eq!(outcome.mode, DeliveryMode::Digest);
    let windows = manager.list_summary_windows().unwrap();
    assert_eq!(windows.len(), 1);
    assert_eq!(windows[0].status, kura_delivery::SummaryWindowStatus::Open);

    // Restore must re-arm the open window (a parked thread already sleeps for the window,
    // so restore() itself must not error and the window list is unchanged).
    manager.restore().unwrap();
    let windows = manager.list_summary_windows().unwrap();
    assert_eq!(windows.len(), 1);
}
