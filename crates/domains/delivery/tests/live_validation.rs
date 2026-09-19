//! Live-validation row and test-sink fake-outcome tests (ports of live_validation_test.go
//! and live_validation_fake_test.go).

use kura_delivery::{TestSinkAdapter, live_validation_matrix_rows};
use kura_livevalidation::{FakeOutcome, MatrixApproval, SafetyClass, fake_outcome_result_for};

#[test]
fn delivery_live_validation_rows_classify_dispatch_and_connector_send() {
    let rows = live_validation_matrix_rows();
    assert_eq!(rows.len(), 2);
    for row in &rows {
        assert_eq!(
            row.safety_class,
            SafetyClass::from(SafetyClass::NON_IDEMPOTENT_MUTATION),
            "delivery row must be a non-idempotent mutation: {row:?}"
        );
        assert_eq!(
            row.approval,
            MatrixApproval::from(MatrixApproval::PER_ACTION),
            "delivery row must require per-action approval: {row:?}"
        );
    }
}

#[test]
fn delivery_test_sink_live_validation_outcomes() {
    let sink = TestSinkAdapter::new();
    let completed = sink.run_live_validation_outcome(&FakeOutcome::from(FakeOutcome::COMPLETED));
    assert_eq!(completed.outcome.as_str(), "completed");
    let submit_unknown =
        sink.run_live_validation_outcome(&FakeOutcome::from(FakeOutcome::SUBMIT_UNKNOWN));
    assert_eq!(submit_unknown.outcome.as_str(), "operator_action_needed");
    assert!(
        submit_unknown.ambiguous_commit,
        "submit unknown must be ambiguous"
    );

    // Sanity: the ported helper agrees with the livevalidation crate directly.
    let direct = fake_outcome_result_for(
        &FakeOutcome::from(FakeOutcome::COMPLETED),
        &SafetyClass::from(SafetyClass::NON_IDEMPOTENT_MUTATION),
    );
    assert_eq!(direct.outcome.as_str(), completed.outcome.as_str());
}
