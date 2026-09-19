//! Shared helpers for the kura-reminders integration tests (port of the Go
//! manager_test.go harness: fake clock, fake workflow launcher, temp SQLite store with a
//! bootstrapped personal tenant + default-tenant cache, delivery target/preference
//! seeding, and the reminders manager construction).

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use chrono::{DateTime, Utc};
use kura_delivery::{
    DeliveryAdapter, DeliveryPreference, DeliveryTarget, Manager as DeliveryManager,
    PreferenceScopeKind, ResultClass, TargetKind, TestSinkAdapter,
};
use kura_events::Bus;
use kura_identity::{LifecycleStatus, Tenant, TenantKind};
use kura_reminders::{Clock, Dependencies, Manager, WorkflowLaunchResult, WorkflowLauncher};
use kura_store::SQLiteStore;
use parking_lot::Mutex;

/// Go fakeClock.
pub struct FakeClock {
    now: Mutex<DateTime<Utc>>,
}

impl FakeClock {
    #[must_use]
    pub fn new(now: DateTime<Utc>) -> Self {
        FakeClock {
            now: Mutex::new(now),
        }
    }

    pub fn set(&self, now: DateTime<Utc>) {
        *self.now.lock() = now.with_timezone(&Utc);
    }
}

impl Clock for FakeClock {
    fn now(&self) -> DateTime<Utc> {
        *self.now.lock()
    }
}

/// Go fakeWorkflowLauncher.
pub struct FakeWorkflowLauncher {
    result: WorkflowLaunchResult,
    err: Option<String>,
}

impl FakeWorkflowLauncher {
    #[must_use]
    pub fn ok(result: WorkflowLaunchResult) -> Self {
        FakeWorkflowLauncher { result, err: None }
    }

    #[must_use]
    pub fn failing(err: &str) -> Self {
        FakeWorkflowLauncher {
            result: WorkflowLaunchResult::default(),
            err: Some(err.to_string()),
        }
    }
}

impl WorkflowLauncher for FakeWorkflowLauncher {
    fn launch_reminder_workflow(
        &self,
        _cfg: &kura_reminders::WorkflowLaunchConfig,
        _reminder_id: &str,
        _occurrence_id: &str,
    ) -> Result<WorkflowLaunchResult, String> {
        match &self.err {
            Some(err) => Err(err.clone()),
            None => Ok(self.result.clone()),
        }
    }
}

/// Go reminderManagerHarnessOptions.
pub struct HarnessOptions {
    pub workflow_launcher: Option<Arc<dyn WorkflowLauncher>>,
    pub tick_interval: Duration,
}

impl Default for HarnessOptions {
    fn default() -> Self {
        HarnessOptions {
            workflow_launcher: None,
            tick_interval: Duration::from_millis(10),
        }
    }
}

/// The reminders manager + its delivery manager, fake clock, and shared store.
pub struct Harness {
    pub manager: Manager,
    pub delivery: DeliveryManager,
    pub clock: Arc<FakeClock>,
    pub store: Arc<Mutex<SQLiteStore>>,
}

/// Go bootstrapTestPersonalTenant's tenant id.
pub const TEST_TENANT_ID: &str = "ten_test_personal";

/// A fresh temp data dir per call (parallel tests never share a SQLite file).
#[must_use]
pub fn temp_dir() -> String {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("kura_reminders_{}_{}", std::process::id(), n));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().to_string()
}

/// A fresh store handle for a test.
#[must_use]
pub fn store() -> Arc<Mutex<SQLiteStore>> {
    Arc::new(Mutex::new(SQLiteStore::new(&temp_dir()).unwrap()))
}

/// Go bootstrapTestPersonalTenant: inserts a personal tenant row directly and primes the
/// default-tenant cache so follow-up projections (and the legacy tenant-safe upserts used
/// by this harness) bind tenant_id correctly.
pub fn bootstrap_personal_tenant(store: &Arc<Mutex<SQLiteStore>>) {
    let now: DateTime<Utc> = "2026-04-01T00:00:00Z".parse().unwrap();
    let tenant = Tenant {
        tenant_id: TEST_TENANT_ID.to_string(),
        tenant_kind: TenantKind::Personal,
        display_name: "Test Personal".to_string(),
        status: LifecycleStatus::Active,
        created_at: now,
        updated_at: now,
        created_by_principal_id: String::new(),
        default_owner_principal_id: String::new(),
        caller_membership_role: None,
        caller_membership_status: None,
        caller_permissions: Vec::new(),
        default_for_current_token: false,
        default_for_current_principal: false,
    };
    store.lock().upsert_tenant(&tenant).unwrap();
    store.lock().seed_default_tenant_cache().unwrap();
}

/// Go newReminderManagerHarness: temp store + personal tenant bootstrap + event bus +
/// delivery manager wired to a test-sink target/preference + reminders manager with a
/// fake clock.
#[must_use]
pub fn harness(opts: HarnessOptions) -> Harness {
    let store = store();
    bootstrap_personal_tenant(&store);
    let bus = Bus::new();
    let delivery = DeliveryManager::new(
        "test",
        bus.clone(),
        Arc::clone(&store),
        vec![Arc::new(TestSinkAdapter::new()) as Arc<dyn DeliveryAdapter>],
    );
    let target = delivery
        .create_target(DeliveryTarget {
            target_id: "reminder-target".to_string(),
            display_name: "Reminder Target".to_string(),
            target_kind: TargetKind::TestSink,
            environment_scope: "test".to_string(),
            ..DeliveryTarget::default()
        })
        .unwrap();
    let mut by_class = HashMap::new();
    by_class.insert(ResultClass::RoutineSuccess, target.target_id.clone());
    by_class.insert(ResultClass::Urgent, target.target_id.clone());
    by_class.insert(ResultClass::Failure, target.target_id.clone());
    delivery
        .upsert_preference(DeliveryPreference {
            preference_id: "reminder-pref".to_string(),
            environment_scope: "test".to_string(),
            scope_kind: PreferenceScopeKind::UserDefault,
            preferred_targets_by_class: by_class,
            ..DeliveryPreference::default()
        })
        .unwrap();
    let clock = Arc::new(FakeClock::new("2026-04-23T09:00:00Z".parse().unwrap()));
    let manager = Manager::new(Dependencies {
        environment_scope: "test".to_string(),
        store: Arc::clone(&store),
        event_bus: Some(bus),
        delivery: Some(delivery.clone()),
        workflow_launcher: opts.workflow_launcher,
        clock: Some(clock.clone() as Arc<dyn Clock>),
        tick_interval: opts.tick_interval,
    });
    Harness {
        manager,
        delivery,
        clock,
        store,
    }
}
