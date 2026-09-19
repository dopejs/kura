//! Port of `daemon/internal/delivery/test_sink.go`: the repo-owned test sink adapter used by
//! integration tests. Messages are recorded in memory (mutex-guarded) exactly like the Go
//! adapter; nothing crosses a real transport.

use chrono::{DateTime, Utc};
use kura_livevalidation::{FakeOutcome, FakeOutcomeResult, SafetyClass, fake_outcome_result_for};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::{
    DeliveryAdapter, DeliveryOutcome, DeliveryTarget, ResultClass, SendResult, TargetKind,
    TargetStatus,
};

/// A recorded test-sink message (port of `TestSinkMessage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestSinkMessage {
    pub target_id: String,
    pub delivery_id: String,
    pub result_class: ResultClass,
    pub payload_preview: String,
    pub recorded_at: DateTime<Utc>,
}

/// In-memory sink adapter (port of `TestSinkAdapter`).
pub struct TestSinkAdapter {
    messages: Mutex<Vec<TestSinkMessage>>,
}

impl TestSinkAdapter {
    #[must_use]
    pub fn new() -> Self {
        TestSinkAdapter {
            messages: Mutex::new(Vec::new()),
        }
    }

    /// Port of `RunLiveValidationOutcome`: the fake-outcome verdict for a non-idempotent
    /// mutation (delivery dispatch is never auto-retried against the sink).
    #[must_use]
    pub fn run_live_validation_outcome(&self, outcome: &FakeOutcome) -> FakeOutcomeResult {
        fake_outcome_result_for(
            outcome,
            &SafetyClass::from(SafetyClass::NON_IDEMPOTENT_MUTATION),
        )
    }

    /// Port of `Messages`: a copy of the recorded messages.
    #[must_use]
    pub fn messages(&self) -> Vec<TestSinkMessage> {
        self.messages.lock().clone()
    }
}

impl Default for TestSinkAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl DeliveryAdapter for TestSinkAdapter {
    fn supports(&self, kind: TargetKind) -> bool {
        kind == TargetKind::TestSink
    }

    fn send(&self, target: DeliveryTarget, outcome: DeliveryOutcome) -> Result<SendResult, String> {
        if target.status != TargetStatus::Active {
            return Err(format!("target {} is not active", target.target_id));
        }
        self.messages.lock().push(TestSinkMessage {
            target_id: target.target_id,
            delivery_id: outcome.delivery_id,
            result_class: outcome.result_class,
            payload_preview: outcome.payload_preview,
            recorded_at: Utc::now(),
        });
        Ok(SendResult {
            transport_kind: TargetKind::TestSink.as_str().to_string(),
            receipt_summary: "stored in repo-owned test sink".to_string(),
            ..SendResult::default()
        })
    }
}
