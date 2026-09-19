//! Ports of daemon/internal/reminders/live_validation_test.go and
//! live_validation_fake_test.go.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::temp_dir;
use kura_livevalidation::{FakeOutcome, SafetyClass, ToolClass};
use kura_reminders::{Dependencies, Manager, live_validation_matrix_rows};
use kura_store::SQLiteStore;
use parking_lot::Mutex;

#[test]
fn reminder_lifecycle_live_validation_classification() {
    let rows = live_validation_matrix_rows();
    assert_eq!(rows.len(), 1, "len(rows)={}, want 1", rows.len());
    assert_eq!(rows[0].tool_class, ToolClass::REMINDER_LIFECYCLE_MUTATION);
    assert_eq!(rows[0].safety_class, SafetyClass::IDEMPOTENT_MUTATION);
}

fn manager_without_backend() -> Manager {
    Manager::new(Dependencies {
        environment_scope: "test".to_string(),
        store: Arc::new(Mutex::new(SQLiteStore::new(&temp_dir()).unwrap())),
        event_bus: None,
        delivery: None,
        workflow_launcher: None,
        clock: None,
        tick_interval: Duration::from_millis(10),
    })
}

#[test]
fn reminder_lifecycle_live_validation_fake_outcomes() {
    let manager = manager_without_backend();
    let duplicate =
        manager.run_live_validation_outcome(&FakeOutcome::from(FakeOutcome::DUPLICATE_RETRY));
    assert_eq!(
        duplicate.outcome.to_string(),
        "completed",
        "duplicate retry should classify as idempotent retry completion: {duplicate:?}"
    );
    assert!(duplicate.automatic_retry_allowed);

    let unknown =
        manager.run_live_validation_outcome(&FakeOutcome::from(FakeOutcome::SUBMIT_UNKNOWN));
    assert_eq!(
        unknown.outcome.to_string(),
        "operator_action_needed",
        "submit unknown should project operator action needed: {unknown:?}"
    );
}
