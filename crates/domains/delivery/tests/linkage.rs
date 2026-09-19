//! Linkage integration tests (port of linkage.go): latest-delivery summaries for run,
//! workflow, and schedule-attempt surfaces.

mod common;

use kura_delivery::{OutcomeInput, ResultClass};

use common::{ScriptedAdapter, manager_with, seed_delivery_preference_state};
use kura_delivery::TargetKind;

#[test]
fn latest_summary_for_run_and_workflow() {
    let (manager, _store) =
        manager_with(vec![ScriptedAdapter::new(TargetKind::TestSink, Vec::new())]);
    seed_delivery_preference_state(&manager, "linkage-target");
    let outcome = manager
        .emit_outcome(OutcomeInput {
            source_kind: "run".to_string(),
            source_id: "run_linkage".to_string(),
            run_id: "run_linkage".to_string(),
            result_class: ResultClass::Urgent,
            payload_preview: "link me".to_string(),
            ..OutcomeInput::default()
        })
        .unwrap();
    assert_eq!(outcome.status, kura_delivery::OutcomeStatus::Delivered);

    let (summary, ok) = manager.latest_summary_for_run("run_linkage").unwrap();
    assert!(ok);
    assert_eq!(summary.latest_delivery_id, outcome.delivery_id);
    assert_eq!(summary.latest_delivery_status, "delivered");
    assert_eq!(summary.latest_delivery_target_id, outcome.chosen_target_id);

    // The run outcome is not a workflow outcome and vice versa; blank run ids find nothing.
    let (summary, ok) = manager.latest_summary_for_workflow("wf_1").unwrap();
    assert!(!ok);
    assert!(summary.latest_delivery_id.is_empty());
    let (summary, ok) = manager.latest_summary_for_run("missing").unwrap();
    assert!(!ok);
}

#[test]
fn latest_summaries_for_schedule_attempts() {
    let (manager, _store) =
        manager_with(vec![ScriptedAdapter::new(TargetKind::TestSink, Vec::new())]);
    seed_delivery_preference_state(&manager, "schedule-linkage-target");
    let emit = |source_id: &str, attempt: &str| {
        manager
            .emit_outcome(OutcomeInput {
                source_kind: "schedule".to_string(),
                source_id: source_id.to_string(),
                schedule_id: "sched_linkage".to_string(),
                schedule_attempt_id: attempt.to_string(),
                result_class: ResultClass::RoutineSuccess,
                payload_preview: "scheduled".to_string(),
                ..OutcomeInput::default()
            })
            .unwrap()
    };
    let first = emit("run_sched_1", "attempt_1");
    let second = emit("run_sched_2", "attempt_2");
    // A second outcome on the same attempt id must not replace the first summary.
    let dup = emit("run_sched_1b", "attempt_1");

    let summaries = manager
        .latest_summaries_for_schedule_attempts("sched_linkage")
        .unwrap();
    assert_eq!(summaries.len(), 2);
    // Outcomes list newest-first (updated_at DESC); the newest outcome per attempt wins.
    assert_eq!(summaries["attempt_1"].latest_delivery_id, dup.delivery_id);
    assert_eq!(
        summaries["attempt_2"].latest_delivery_id,
        second.delivery_id
    );
    assert_ne!(summaries["attempt_1"].latest_delivery_id, first.delivery_id);
}
