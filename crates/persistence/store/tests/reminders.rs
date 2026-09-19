//! Round-trip integration tests for the reminders / consumer-policy / secret-scope CRUD
//! methods ported from `daemon/internal/store/store.go` into `reminders.rs`,
//! `consumer_policy.rs`, and `secret_scope.rs`. Each test constructs a record, upserts it,
//! lists/gets it back, and asserts key fields. Wiring required before these compile: declare
//! `pub mod reminders; pub mod consumer_policy; pub mod secret_scope;` in `lib.rs` so the
//! record structs and the occurrence filter are reachable from this integration test.

use chrono::Utc;
use kura_runtime::{Run, RunStatus};
use kura_store::{
    SQLiteStore,
    consumer_policy::ConsumerPolicyRecordRecord,
    reminders::{
        ReminderActionRecord, ReminderOccurrenceFilter, ReminderOccurrenceRecord, ReminderRecord,
    },
    secret_scope::SecretScopeBindingRecord,
};

fn temp_dir(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("kura_store_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().to_string()
}

/// Creates a run so FK-shaped columns (occurrence/action run_id) reference an existing row.
fn upsert_run(store: &SQLiteStore, run_id: &str) {
    let now = Utc::now();
    let run = Run {
        run_id: run_id.to_string(),
        session_id: String::new(),
        entrypoint: "test entrypoint".to_string(),
        status: RunStatus::Running,
        goal: "test goal".to_string(),
        created_at: now,
        updated_at: now,
        ..Run::default()
    };
    store.upsert_run(&run).unwrap();
}

#[test]
fn reminder_round_trips_through_sqlite() {
    let dir = temp_dir("reminder");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();

    let mut reminder = ReminderRecord {
        reminder_id: "rem_1".to_string(),
        environment_scope: "test".to_string(),
        tenant_id: String::new(),
        behavior_mode: "schedule".to_string(),
        current_state: "active".to_string(),
        next_due_at: Some(now),
        active_occurrence_id: "occ_1".to_string(),
        updated_at: now,
        document: "{\"kind\":\"reminder\"}".to_string(),
    };
    store.upsert_reminder(&reminder).unwrap();
    // Upsert again through the ON CONFLICT path with a changed field.
    reminder.current_state = "paused".to_string();
    store.upsert_reminder(&reminder).unwrap();

    let listed = store.list_reminders("test").unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.reminder_id, "rem_1");
    assert_eq!(got.environment_scope, "test");
    assert_eq!(got.behavior_mode, "schedule");
    assert_eq!(got.current_state, "paused");
    assert_eq!(got.active_occurrence_id, "occ_1");
    assert_eq!(got.document, "{\"kind\":\"reminder\"}");
    assert!(got.next_due_at.is_some());

    let fetched = store.get_reminder("test", "rem_1").unwrap().expect("found");
    assert_eq!(fetched.current_state, "paused");
    assert_eq!(store.get_reminder("test", "missing").unwrap(), None);
    // No rows in a different environment scope.
    assert!(store.list_reminders("prod").unwrap().is_empty());
}

#[test]
fn reminder_occurrence_round_trips_through_sqlite() {
    let dir = temp_dir("reminder_occ");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();

    // The occurrence references a run and a reminder; create both first.
    upsert_run(&store, "run_reminder");
    let reminder = ReminderRecord {
        reminder_id: "rem_1".to_string(),
        environment_scope: "test".to_string(),
        tenant_id: String::new(),
        behavior_mode: "schedule".to_string(),
        current_state: "active".to_string(),
        next_due_at: None,
        active_occurrence_id: String::new(),
        updated_at: now,
        document: "{\"kind\":\"reminder\"}".to_string(),
    };
    store.upsert_reminder(&reminder).unwrap();

    let mut occurrence = ReminderOccurrenceRecord {
        occurrence_id: "occ_1".to_string(),
        reminder_id: "rem_1".to_string(),
        environment_scope: "test".to_string(),
        state: "scheduled".to_string(),
        scheduled_for: now,
        run_id: "run_reminder".to_string(),
        workflow_id: String::new(),
        latest_delivery_id: "deliv_1".to_string(),
        latest_delivery_status: "delivered".to_string(),
        updated_at: now,
        document: "{\"kind\":\"occurrence\"}".to_string(),
    };
    store.upsert_reminder_occurrence(&occurrence).unwrap();
    // Upsert again through the ON CONFLICT path with a changed field.
    occurrence.state = "fired".to_string();
    store.upsert_reminder_occurrence(&occurrence).unwrap();

    let listed = store
        .list_reminder_occurrences("test", &ReminderOccurrenceFilter::default())
        .unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.occurrence_id, "occ_1");
    assert_eq!(got.reminder_id, "rem_1");
    assert_eq!(got.state, "fired");
    assert_eq!(got.run_id, "run_reminder");
    assert_eq!(got.latest_delivery_id, "deliv_1");
    assert_eq!(got.latest_delivery_status, "delivered");
    assert_eq!(got.document, "{\"kind\":\"occurrence\"}");

    // Dynamic filter matches and misses.
    let state_filter = ReminderOccurrenceFilter {
        state: "fired".to_string(),
        ..Default::default()
    };
    assert_eq!(
        store
            .list_reminder_occurrences("test", &state_filter)
            .unwrap()
            .len(),
        1
    );
    let missed_filter = ReminderOccurrenceFilter {
        state: "cancelled".to_string(),
        ..Default::default()
    };
    assert!(
        store
            .list_reminder_occurrences("test", &missed_filter)
            .unwrap()
            .is_empty()
    );
    let run_filter = ReminderOccurrenceFilter {
        run_id: "run_reminder".to_string(),
        ..Default::default()
    };
    assert_eq!(
        store
            .list_reminder_occurrences("test", &run_filter)
            .unwrap()
            .len(),
        1
    );
    let window_filter = ReminderOccurrenceFilter {
        scheduled_before: Some(now + chrono::Duration::seconds(60)),
        scheduled_after: Some(now - chrono::Duration::seconds(60)),
        ..Default::default()
    };
    assert_eq!(
        store
            .list_reminder_occurrences("test", &window_filter)
            .unwrap()
            .len(),
        1
    );
    let before_filter = ReminderOccurrenceFilter {
        scheduled_before: Some(now - chrono::Duration::seconds(60)),
        ..Default::default()
    };
    assert!(
        store
            .list_reminder_occurrences("test", &before_filter)
            .unwrap()
            .is_empty()
    );

    let fetched = store
        .get_reminder_occurrence("test", "occ_1")
        .unwrap()
        .expect("found");
    assert_eq!(fetched.state, "fired");
    assert_eq!(
        store.get_reminder_occurrence("test", "missing").unwrap(),
        None
    );
    assert!(
        store
            .list_reminder_occurrences("prod", &ReminderOccurrenceFilter::default())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn reminder_action_round_trips_through_sqlite() {
    let dir = temp_dir("reminder_action");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();

    upsert_run(&store, "run_reminder");
    let reminder = ReminderRecord {
        reminder_id: "rem_1".to_string(),
        environment_scope: "test".to_string(),
        tenant_id: String::new(),
        behavior_mode: "schedule".to_string(),
        current_state: "active".to_string(),
        next_due_at: None,
        active_occurrence_id: "occ_1".to_string(),
        updated_at: now,
        document: "{\"kind\":\"reminder\"}".to_string(),
    };
    store.upsert_reminder(&reminder).unwrap();

    let first = ReminderActionRecord {
        action_id: "act_1".to_string(),
        reminder_id: "rem_1".to_string(),
        occurrence_id: "occ_1".to_string(),
        action_kind: "snooze".to_string(),
        new_state: "snoozed".to_string(),
        run_id: "run_reminder".to_string(),
        workflow_id: String::new(),
        delivery_id: String::new(),
        created_at: now,
        document: "{\"kind\":\"action\"}".to_string(),
    };
    store.append_reminder_action(&first).unwrap();
    let second = ReminderActionRecord {
        action_id: "act_2".to_string(),
        created_at: now + chrono::Duration::seconds(10),
        ..first.clone()
    };
    store.append_reminder_action(&second).unwrap();

    let actions = store.list_reminder_actions("test", "rem_1").unwrap();
    assert_eq!(actions.len(), 2);
    // Ordered by created_at DESC.
    assert_eq!(actions[0].action_id, "act_2");
    assert_eq!(actions[0].action_kind, "snooze");
    assert_eq!(actions[0].new_state, "snoozed");
    assert_eq!(actions[0].occurrence_id, "occ_1");
    assert_eq!(actions[0].run_id, "run_reminder");
    assert_eq!(actions[1].action_id, "act_1");
    // Scoped to the reminder's environment scope.
    assert!(
        store
            .list_reminder_actions("prod", "rem_1")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn consumer_policy_record_round_trips_through_sqlite() {
    let dir = temp_dir("consumer_policy");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();

    let mut record = ConsumerPolicyRecordRecord {
        policy_record_id: "pr_1".to_string(),
        consumer_kind: "tool_call".to_string(),
        consumer_id: "tc_1".to_string(),
        operation_kind: "run_tool".to_string(),
        declaration_id: "decl_1".to_string(),
        status: "approved".to_string(),
        decision: "allow".to_string(),
        approval_status: "approved".to_string(),
        secret_resolution: "resolved".to_string(),
        requested_by: "user_1".to_string(),
        sandbox_execution_id: "exec_1".to_string(),
        tool_call_id: "tc_1".to_string(),
        provider_operation_id: "pop_1".to_string(),
        started_at: now,
        completed_at: Some(now),
        document: "{\"kind\":\"policy\"}".to_string(),
    };
    store.upsert_consumer_policy_record(&record).unwrap();
    // Upsert again through the ON CONFLICT path with a changed field.
    record.status = "denied".to_string();
    store.upsert_consumer_policy_record(&record).unwrap();

    let listed = store.list_consumer_policy_records().unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.policy_record_id, "pr_1");
    assert_eq!(got.consumer_kind, "tool_call");
    assert_eq!(got.consumer_id, "tc_1");
    assert_eq!(got.operation_kind, "run_tool");
    assert_eq!(got.declaration_id, "decl_1");
    assert_eq!(got.status, "denied");
    assert_eq!(got.decision, "allow");
    assert_eq!(got.approval_status, "approved");
    assert_eq!(got.secret_resolution, "resolved");
    assert_eq!(got.requested_by, "user_1");
    assert_eq!(got.sandbox_execution_id, "exec_1");
    assert_eq!(got.tool_call_id, "tc_1");
    assert_eq!(got.provider_operation_id, "pop_1");
    assert_eq!(got.document, "{\"kind\":\"policy\"}");
    assert!(got.completed_at.is_some());
}

#[test]
fn secret_scope_binding_round_trips_through_sqlite() {
    let dir = temp_dir("secret_scope");
    let store = SQLiteStore::new(&dir).unwrap();

    let mut binding = SecretScopeBindingRecord {
        binding_id: "bind_1".to_string(),
        consumer_kind: "integration".to_string(),
        consumer_id: "int_cal".to_string(),
        environment_scope: "test".to_string(),
        secret_ref: "google/client_secret".to_string(),
        default_source: "env".to_string(),
        delivery_kind: "direct".to_string(),
        active: true,
        document: "{\"kind\":\"binding\"}".to_string(),
    };
    store.upsert_secret_scope_binding(&binding).unwrap();
    // Upsert again through the ON CONFLICT path with a changed field.
    binding.active = false;
    store.upsert_secret_scope_binding(&binding).unwrap();

    let listed = store
        .list_secret_scope_bindings("integration", "int_cal")
        .unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.binding_id, "bind_1");
    assert_eq!(got.consumer_kind, "integration");
    assert_eq!(got.consumer_id, "int_cal");
    assert_eq!(got.environment_scope, "test");
    assert_eq!(got.secret_ref, "google/client_secret");
    assert_eq!(got.default_source, "env");
    assert_eq!(got.delivery_kind, "direct");
    assert_eq!(got.active, false);
    assert_eq!(got.document, "{\"kind\":\"binding\"}");
    // No bindings for a different consumer.
    assert!(
        store
            .list_secret_scope_bindings("integration", "int_mail")
            .unwrap()
            .is_empty()
    );
}
