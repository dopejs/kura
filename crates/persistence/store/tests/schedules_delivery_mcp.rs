//! Round-trip integration tests for the ported schedule, delivery, and MCP CRUD paths.
//! Each test constructs a store-local record, upserts it, lists/gets it back, and asserts the key
//! fields survived the SQLite round trip. Foreign-key parents (schedules, delivery outcomes, MCP
//! servers/tools) are created before their dependent rows.

use chrono::{DateTime, Utc};
use kura_store::SQLiteStore;
use kura_store::delivery::{
    DeliveryAttemptRecord, DeliveryOutcomeFilter, DeliveryOutcomeRecord, DeliveryPreferenceRecord,
    DeliverySummaryWindowRecord, DeliveryTargetRecord,
};
use kura_store::mcp::{
    MCPServerRecord, MCPServerStateRecord, MCPToolExposureRuleRecord, MCPToolRecord,
};
use kura_store::schedule::{ScheduleDispatchAttemptRecord, ScheduleRecord, ScheduleTargetRecord};

fn temp_dir(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("kura_store_sdm_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().to_string()
}

fn make_schedule(now: DateTime<Utc>) -> ScheduleRecord {
    ScheduleRecord {
        schedule_id: "sched_1".to_string(),
        environment_scope: "test".to_string(),
        tenant_id: String::new(),
        kind: "cron".to_string(),
        status: "active".to_string(),
        target_ref_id: "target_1".to_string(),
        timezone: "UTC".to_string(),
        next_due_at: Some(now),
        last_attempt_at: None,
        last_outcome: String::new(),
        created_at: now,
        updated_at: now,
        paused_at: None,
        cancelled_at: None,
        completed_at: None,
        document: r#"{"kind":"cron"}"#.to_string(),
    }
}

#[test]
fn schedule_record_round_trips_through_sqlite() {
    let dir = temp_dir("schedule");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();
    let schedule = make_schedule(now);
    store.upsert_schedule(&schedule).unwrap();

    let listed = store.list_schedules("test").unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.schedule_id, "sched_1");
    assert_eq!(got.environment_scope, "test");
    assert_eq!(got.kind, "cron");
    assert_eq!(got.status, "active");
    assert_eq!(got.target_ref_id, "target_1");
    assert_eq!(got.timezone, "UTC");
    assert_eq!(got.next_due_at, Some(now));
    assert_eq!(got.last_attempt_at, None);
    assert_eq!(got.document, r#"{"kind":"cron"}"#);

    let fetched = store
        .get_schedule("test", "sched_1")
        .unwrap()
        .expect("found");
    assert_eq!(fetched.schedule_id, "sched_1");
    assert_eq!(fetched.kind, "cron");
    assert_eq!(fetched.next_due_at, Some(now));
    assert_eq!(store.get_schedule("test", "missing").unwrap(), None);
    assert_eq!(store.get_schedule("other", "sched_1").unwrap(), None);

    // Upserting the same id overwrites the row instead of duplicating it.
    let mut updated = schedule;
    updated.status = "paused".to_string();
    updated.paused_at = Some(now);
    store.upsert_schedule(&updated).unwrap();
    let listed = store.list_schedules("test").unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].status, "paused");
    assert_eq!(listed[0].paused_at, Some(now));
}

#[test]
fn schedule_target_round_trips_through_sqlite() {
    let dir = temp_dir("scheduletarget");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();
    store.upsert_schedule(&make_schedule(now)).unwrap();

    let target = ScheduleTargetRecord {
        target_ref_id: "target_1".to_string(),
        schedule_id: "sched_1".to_string(),
        target_kind: "agent".to_string(),
        revision: 3,
        active: true,
        updated_at: now,
        document: r#"{"kind":"agent"}"#.to_string(),
    };
    store.upsert_schedule_target(&target).unwrap();

    let fetched = store
        .get_schedule_target("sched_1", "target_1")
        .unwrap()
        .expect("found");
    assert_eq!(fetched.target_ref_id, "target_1");
    assert_eq!(fetched.schedule_id, "sched_1");
    assert_eq!(fetched.target_kind, "agent");
    assert_eq!(fetched.revision, 3);
    assert_eq!(fetched.active, true);
    assert_eq!(fetched.updated_at, now);
    assert_eq!(fetched.document, r#"{"kind":"agent"}"#);
    assert_eq!(
        store.get_schedule_target("sched_1", "missing").unwrap(),
        None
    );

    // Upserting the same id overwrites the row instead of duplicating it.
    let mut updated = target;
    updated.revision = 4;
    updated.active = false;
    store.upsert_schedule_target(&updated).unwrap();
    let fetched = store
        .get_schedule_target("sched_1", "target_1")
        .unwrap()
        .expect("found");
    assert_eq!(fetched.revision, 4);
    assert_eq!(fetched.active, false);
}

#[test]
fn schedule_dispatch_attempt_round_trips_through_sqlite() {
    let dir = temp_dir("schedattempt");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();
    store.upsert_schedule(&make_schedule(now)).unwrap();

    let attempt = ScheduleDispatchAttemptRecord {
        attempt_id: "att_1".to_string(),
        schedule_id: "sched_1".to_string(),
        due_at: now,
        trigger_source: "cron".to_string(),
        dispatch_status: "dispatched".to_string(),
        failure_class: String::new(),
        failure_reason: String::new(),
        retry_count: 0,
        retry_budget: 3,
        next_retry_at: Some(now),
        resolved_target_revision: 3,
        run_id: "run_1".to_string(),
        workflow_id: "wf_1".to_string(),
        downstream_status: "running".to_string(),
        skipped_reason: String::new(),
        missed_count: 0,
        created_at: now,
        updated_at: now,
        document: r#"{"status":"dispatched"}"#.to_string(),
    };
    store.upsert_schedule_dispatch_attempt(&attempt).unwrap();

    let listed = store.list_schedule_dispatch_attempts("sched_1").unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.attempt_id, "att_1");
    assert_eq!(got.schedule_id, "sched_1");
    assert_eq!(got.due_at, now);
    assert_eq!(got.trigger_source, "cron");
    assert_eq!(got.dispatch_status, "dispatched");
    assert_eq!(got.retry_count, 0);
    assert_eq!(got.retry_budget, 3);
    assert_eq!(got.next_retry_at, Some(now));
    assert_eq!(got.resolved_target_revision, 3);
    assert_eq!(got.run_id, "run_1");
    assert_eq!(got.workflow_id, "wf_1");
    assert_eq!(got.downstream_status, "running");
    assert_eq!(got.missed_count, 0);
    assert_eq!(
        store
            .list_schedule_dispatch_attempts("missing")
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn delivery_target_round_trips_through_sqlite() {
    let dir = temp_dir("deltarget");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();
    let target = DeliveryTargetRecord {
        target_id: "dt_1".to_string(),
        environment_scope: "test".to_string(),
        target_kind: "email".to_string(),
        status: "active".to_string(),
        updated_at: now,
        document: r#"{"kind":"email"}"#.to_string(),
    };
    store.upsert_delivery_target(&target).unwrap();

    let listed = store.list_delivery_targets("test").unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.target_id, "dt_1");
    assert_eq!(got.environment_scope, "test");
    assert_eq!(got.target_kind, "email");
    assert_eq!(got.status, "active");
    assert_eq!(got.updated_at, now);
    assert_eq!(got.document, r#"{"kind":"email"}"#);

    let fetched = store
        .get_delivery_target("test", "dt_1")
        .unwrap()
        .expect("found");
    assert_eq!(fetched.target_id, "dt_1");
    assert_eq!(fetched.target_kind, "email");
    assert_eq!(store.get_delivery_target("test", "missing").unwrap(), None);
    assert_eq!(store.list_delivery_targets("other").unwrap().len(), 0);
}

#[test]
fn delivery_preference_round_trips_through_sqlite() {
    let dir = temp_dir("delpref");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();
    let preference = DeliveryPreferenceRecord {
        preference_id: "dp_1".to_string(),
        environment_scope: "test".to_string(),
        scope_kind: "channel".to_string(),
        integration_id: "int_1".to_string(),
        active: true,
        updated_at: now,
        document: r#"{"scope":"channel"}"#.to_string(),
    };
    store.upsert_delivery_preference(&preference).unwrap();

    let listed = store.list_delivery_preferences("test").unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.preference_id, "dp_1");
    assert_eq!(got.environment_scope, "test");
    assert_eq!(got.scope_kind, "channel");
    assert_eq!(got.integration_id, "int_1");
    assert_eq!(got.active, true);
    assert_eq!(got.updated_at, now);

    let fetched = store
        .get_delivery_preference("test", "dp_1")
        .unwrap()
        .expect("found");
    assert_eq!(fetched.preference_id, "dp_1");
    assert_eq!(fetched.active, true);
    assert_eq!(
        store.get_delivery_preference("test", "missing").unwrap(),
        None
    );
}

#[test]
fn delivery_outcome_round_trips_and_filters_through_sqlite() {
    let dir = temp_dir("deloutcome");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();
    let outcome = DeliveryOutcomeRecord {
        delivery_id: "do_1".to_string(),
        environment_scope: "test".to_string(),
        source_kind: "run_complete".to_string(),
        source_id: "run_1".to_string(),
        run_id: "run_1".to_string(),
        workflow_id: "wf_1".to_string(),
        schedule_id: "sched_1".to_string(),
        integration_id: "int_1".to_string(),
        status: "delivered".to_string(),
        chosen_target_id: "dt_1".to_string(),
        preference_id: "dp_1".to_string(),
        summary_window_id: "sw_1".to_string(),
        updated_at: now,
        document: r#"{"status":"delivered"}"#.to_string(),
    };
    store.upsert_delivery_outcome(&outcome).unwrap();

    let listed = store
        .list_delivery_outcomes("test", &DeliveryOutcomeFilter::default())
        .unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.delivery_id, "do_1");
    assert_eq!(got.environment_scope, "test");
    assert_eq!(got.source_kind, "run_complete");
    assert_eq!(got.source_id, "run_1");
    assert_eq!(got.run_id, "run_1");
    assert_eq!(got.workflow_id, "wf_1");
    assert_eq!(got.schedule_id, "sched_1");
    assert_eq!(got.integration_id, "int_1");
    assert_eq!(got.status, "delivered");
    assert_eq!(got.chosen_target_id, "dt_1");
    assert_eq!(got.preference_id, "dp_1");
    assert_eq!(got.summary_window_id, "sw_1");
    assert_eq!(got.updated_at, now);

    let by_run = DeliveryOutcomeFilter {
        run_id: "run_1".to_string(),
        ..DeliveryOutcomeFilter::default()
    };
    assert_eq!(
        store.list_delivery_outcomes("test", &by_run).unwrap().len(),
        1
    );
    let by_target = DeliveryOutcomeFilter {
        target_id: "dt_1".to_string(),
        ..DeliveryOutcomeFilter::default()
    };
    assert_eq!(
        store
            .list_delivery_outcomes("test", &by_target)
            .unwrap()
            .len(),
        1
    );
    let by_status = DeliveryOutcomeFilter {
        status: "pending".to_string(),
        ..DeliveryOutcomeFilter::default()
    };
    assert_eq!(
        store
            .list_delivery_outcomes("test", &by_status)
            .unwrap()
            .len(),
        0
    );

    let fetched = store
        .get_delivery_outcome("test", "do_1")
        .unwrap()
        .expect("found");
    assert_eq!(fetched.delivery_id, "do_1");
    assert_eq!(fetched.status, "delivered");
    assert_eq!(store.get_delivery_outcome("test", "missing").unwrap(), None);
}

#[test]
fn delivery_attempt_round_trips_through_sqlite() {
    let dir = temp_dir("delattempt");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();
    // delivery_attempts.delivery_id references delivery_outcomes; create the parent first.
    let outcome = DeliveryOutcomeRecord {
        delivery_id: "do_1".to_string(),
        environment_scope: "test".to_string(),
        source_kind: "run_complete".to_string(),
        source_id: "run_1".to_string(),
        run_id: "run_1".to_string(),
        workflow_id: String::new(),
        schedule_id: String::new(),
        integration_id: String::new(),
        status: "delivering".to_string(),
        chosen_target_id: String::new(),
        preference_id: String::new(),
        summary_window_id: String::new(),
        updated_at: now,
        document: r#"{"status":"delivering"}"#.to_string(),
    };
    store.upsert_delivery_outcome(&outcome).unwrap();

    let attempt = DeliveryAttemptRecord {
        attempt_id: "da_1".to_string(),
        delivery_id: "do_1".to_string(),
        attempt_number: 2,
        target_id: "dt_1".to_string(),
        status: "failed".to_string(),
        next_retry_at: Some(now),
        document: r#"{"status":"failed"}"#.to_string(),
    };
    store.upsert_delivery_attempt(&attempt).unwrap();

    let listed = store.list_delivery_attempts("do_1").unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.attempt_id, "da_1");
    assert_eq!(got.delivery_id, "do_1");
    assert_eq!(got.attempt_number, 2);
    assert_eq!(got.target_id, "dt_1");
    assert_eq!(got.status, "failed");
    assert_eq!(got.next_retry_at, Some(now));
    assert_eq!(got.document, r#"{"status":"failed"}"#);
    assert_eq!(store.list_delivery_attempts("missing").unwrap().len(), 0);
}

#[test]
fn delivery_summary_window_round_trips_through_sqlite() {
    let dir = temp_dir("delsumwindow");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();
    let window = DeliverySummaryWindowRecord {
        summary_window_id: "sw_1".to_string(),
        environment_scope: "test".to_string(),
        target_id: "dt_1".to_string(),
        preference_id: "dp_1".to_string(),
        status: "open".to_string(),
        window_ends_at: now,
        updated_at: now,
        document: r#"{"status":"open"}"#.to_string(),
    };
    store.upsert_delivery_summary_window(&window).unwrap();

    let listed = store.list_delivery_summary_windows("test").unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.summary_window_id, "sw_1");
    assert_eq!(got.environment_scope, "test");
    assert_eq!(got.target_id, "dt_1");
    assert_eq!(got.preference_id, "dp_1");
    assert_eq!(got.status, "open");
    assert_eq!(got.window_ends_at, now);
    assert_eq!(got.updated_at, now);

    let fetched = store
        .get_delivery_summary_window("test", "sw_1")
        .unwrap()
        .expect("found");
    assert_eq!(fetched.summary_window_id, "sw_1");
    assert_eq!(fetched.status, "open");
    assert_eq!(
        store
            .get_delivery_summary_window("test", "missing")
            .unwrap(),
        None
    );
}

#[test]
fn mcp_server_and_state_round_trip_through_sqlite() {
    let dir = temp_dir("mcpserver");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();
    let server = MCPServerRecord {
        server_id: "mcp_1".to_string(),
        enabled: true,
        updated_at: now,
        document: r#"{"name":"files"}"#.to_string(),
    };
    store.upsert_mcp_server(&server).unwrap();

    let listed = store.list_mcp_servers().unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.server_id, "mcp_1");
    assert_eq!(got.enabled, true);
    assert_eq!(got.updated_at, now);
    assert_eq!(got.document, r#"{"name":"files"}"#);

    // Upserting the same id overwrites the row instead of duplicating it.
    let mut updated = server;
    updated.enabled = false;
    store.upsert_mcp_server(&updated).unwrap();
    let listed = store.list_mcp_servers().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].enabled, false);

    // mcp_server_states.server_id references mcp_servers; the server already exists.
    let state = MCPServerStateRecord {
        server_id: "mcp_1".to_string(),
        status: "connected".to_string(),
        updated_at: now,
        document: r#"{"status":"connected"}"#.to_string(),
    };
    store.upsert_mcp_server_state(&state).unwrap();
    let states = store.list_mcp_server_states().unwrap();
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].server_id, "mcp_1");
    assert_eq!(states[0].status, "connected");

    store.delete_mcp_server("mcp_1").unwrap();
    assert_eq!(store.list_mcp_servers().unwrap().len(), 0);
    // The server state row cascades away with its parent.
    assert_eq!(store.list_mcp_server_states().unwrap().len(), 0);
}

#[test]
fn mcp_tool_and_exposure_rule_round_trip_through_sqlite() {
    let dir = temp_dir("mcptool");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();
    // mcp_tools.server_id references mcp_servers; create the server first.
    store
        .upsert_mcp_server(&MCPServerRecord {
            server_id: "mcp_1".to_string(),
            enabled: true,
            updated_at: now,
            document: r#"{"name":"files"}"#.to_string(),
        })
        .unwrap();

    let tool = MCPToolRecord {
        server_id: "mcp_1".to_string(),
        tool_name: "read_file".to_string(),
        discovery_status: "discovered".to_string(),
        updated_at: now,
        last_discovered_at: Some(now),
        document: r#"{"name":"read_file"}"#.to_string(),
    };
    store.upsert_mcp_tool(&tool).unwrap();

    let listed = store.list_mcp_tools("mcp_1").unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.server_id, "mcp_1");
    assert_eq!(got.tool_name, "read_file");
    assert_eq!(got.discovery_status, "discovered");
    assert_eq!(got.updated_at, now);
    assert_eq!(got.last_discovered_at, Some(now));
    assert_eq!(got.document, r#"{"name":"read_file"}"#);
    // An empty server id lists tools across all servers.
    assert_eq!(store.list_mcp_tools("").unwrap().len(), 1);

    // mcp_tool_exposure_rules references mcp_tools; the tool already exists.
    let rule = MCPToolExposureRuleRecord {
        server_id: "mcp_1".to_string(),
        tool_name: "read_file".to_string(),
        runtime_surface: "agent".to_string(),
        exposure_mode: "allow".to_string(),
        active: true,
        updated_at: now,
        document: r#"{"mode":"allow"}"#.to_string(),
    };
    store.upsert_mcp_tool_exposure_rule(&rule).unwrap();
    let rules = store.list_mcp_tool_exposure_rules("mcp_1").unwrap();
    assert_eq!(rules.len(), 1);
    let got_rule = &rules[0];
    assert_eq!(got_rule.server_id, "mcp_1");
    assert_eq!(got_rule.tool_name, "read_file");
    assert_eq!(got_rule.runtime_surface, "agent");
    assert_eq!(got_rule.exposure_mode, "allow");
    assert_eq!(got_rule.active, true);
    assert_eq!(store.list_mcp_tool_exposure_rules("").unwrap().len(), 1);

    // ReplaceMCPTools swaps the tool set for the server in one transaction.
    let replacement = MCPToolRecord {
        server_id: "mcp_1".to_string(),
        tool_name: "write_file".to_string(),
        discovery_status: "discovered".to_string(),
        updated_at: now,
        last_discovered_at: None,
        document: r#"{"name":"write_file"}"#.to_string(),
    };
    store.replace_mcp_tools("mcp_1", &[replacement]).unwrap();
    let tools = store.list_mcp_tools("mcp_1").unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].tool_name, "write_file");
    assert_eq!(tools[0].last_discovered_at, None);
    // The old exposure rule referenced the replaced tool and cascades away.
    assert_eq!(
        store.list_mcp_tool_exposure_rules("mcp_1").unwrap().len(),
        0
    );
}
