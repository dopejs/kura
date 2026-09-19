//! Shared helpers for the delivery integration tests: temp SQLite stores, a scripted
//! adapter (Go `scriptedAdapter`), preference seeding (Go `seedDeliveryPreferenceState`),
//! and status wait loops (Go `waitForOutcomeStatus` / `waitForWindowStatus`).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use kura_delivery::{
    DeliveryAdapter, DeliveryOutcome, DeliveryPreference, DeliveryTarget, Manager, OutcomeStatus,
    ResultClass, SendResult, SummaryWindowStatus, TargetKind,
};
use kura_events::Bus;
use kura_store::SQLiteStore;
use parking_lot::Mutex;

/// A fresh temp data dir per test name (mirrors the store crate tests).
#[must_use]
pub fn temp_dir(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("kura_delivery_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().to_string()
}

/// A shared SQLite store handle for a test.
#[must_use]
pub fn store(name: &str) -> Arc<Mutex<SQLiteStore>> {
    Arc::new(Mutex::new(SQLiteStore::new(&temp_dir(name)).unwrap()))
}

/// Port of the Go `scriptedAdapter`: each send consumes the next scripted result; sends
/// beyond the script succeed.
pub struct ScriptedAdapter {
    target_kind: TargetKind,
    results: Vec<Result<(), String>>,
    next: AtomicUsize,
    sends: AtomicUsize,
}

impl ScriptedAdapter {
    #[must_use]
    pub fn new(target_kind: TargetKind, results: Vec<Result<(), String>>) -> Arc<Self> {
        Arc::new(ScriptedAdapter {
            target_kind,
            results,
            next: AtomicUsize::new(0),
            sends: AtomicUsize::new(0),
        })
    }

    #[must_use]
    pub fn sends(&self) -> usize {
        self.sends.load(Ordering::SeqCst)
    }
}

impl DeliveryAdapter for ScriptedAdapter {
    fn supports(&self, kind: TargetKind) -> bool {
        kind == self.target_kind
    }

    fn send(
        &self,
        _target: DeliveryTarget,
        _outcome: DeliveryOutcome,
    ) -> Result<SendResult, String> {
        let idx = self.next.fetch_add(1, Ordering::SeqCst);
        self.sends.fetch_add(1, Ordering::SeqCst);
        match self.results.get(idx) {
            Some(Err(err)) => Err(err.clone()),
            _ => Ok(SendResult {
                transport_kind: self.target_kind.as_str().to_string(),
                receipt_summary: "ok".to_string(),
                ..SendResult::default()
            }),
        }
    }
}

/// Port of Go `seedDeliveryPreferenceState`: one active test-sink target wired to all three
/// result classes through a user-default preference.
#[must_use]
pub fn seed_delivery_preference_state(
    manager: &Manager,
    target_id: &str,
) -> (DeliveryTarget, DeliveryPreference) {
    let target = manager
        .create_target(DeliveryTarget {
            target_id: target_id.to_string(),
            display_name: "Primary".to_string(),
            target_kind: TargetKind::TestSink,
            environment_scope: "test".to_string(),
            ..DeliveryTarget::default()
        })
        .unwrap();
    let mut by_class = std::collections::HashMap::new();
    by_class.insert(ResultClass::RoutineSuccess, target.target_id.clone());
    by_class.insert(ResultClass::Urgent, target.target_id.clone());
    by_class.insert(ResultClass::Failure, target.target_id.clone());
    let pref = manager
        .upsert_preference(DeliveryPreference {
            preference_id: "pref-default".to_string(),
            environment_scope: "test".to_string(),
            scope_kind: kura_delivery::PreferenceScopeKind::UserDefault,
            preferred_targets_by_class: by_class,
            ..DeliveryPreference::default()
        })
        .unwrap();
    (target, pref)
}

/// Port of Go `waitForOutcomeStatus`: polls up to 3s for the outcome status.
#[must_use]
pub fn wait_for_outcome_status(
    manager: &Manager,
    delivery_id: &str,
    expected: OutcomeStatus,
) -> DeliveryOutcome {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if let Ok((outcome, true)) = manager.get_outcome(delivery_id) {
            if outcome.status == expected {
                return outcome;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let last = manager
        .get_outcome(delivery_id)
        .map(|(o, _)| o)
        .unwrap_or_default();
    panic!("delivery {delivery_id} did not reach {expected}, last={last:?}");
}

/// Port of Go `waitForWindowStatus`: polls up to 3s for the window status.
pub fn wait_for_window_status(
    manager: &Manager,
    summary_window_id: &str,
    expected: SummaryWindowStatus,
) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if let Ok((window, true)) = manager.get_summary_window(summary_window_id) {
            if window.status == expected {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let last = manager
        .get_summary_window(summary_window_id)
        .map(|(w, _)| w)
        .unwrap_or_default();
    panic!("summary window {summary_window_id} did not reach {expected}, last={last:?}");
}

/// A manager wired to the given adapters over a fresh test store. The temp dir is unique per
/// call so parallel tests never share a SQLite file.
#[must_use]
pub fn manager_with(adapters: Vec<Arc<dyn DeliveryAdapter>>) -> (Manager, Arc<Mutex<SQLiteStore>>) {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let name = format!("mgr_{}", COUNTER.fetch_add(1, Ordering::SeqCst));
    let store = store(&name);
    let manager = Manager::new("test", Bus::new(), Arc::clone(&store), adapters);
    (manager, store)
}
