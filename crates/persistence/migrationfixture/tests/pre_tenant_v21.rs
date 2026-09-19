//! Legacy-core fixture: every in-scope table carries its seeded parent +
//! child rows on the first-release baseline schema.

mod common;

use common::{count, open_conn, temp_dir};
use kura_migrationfixture::{FIXTURE_TIMESTAMP, build_pre_tenant_v21_fixture, count_seeded_rows};

#[test]
fn pre_tenant_v21_fixture_seeds_all_parent_child_pairs() {
    let dir = temp_dir("pre_tenant_v21");
    let store = build_pre_tenant_v21_fixture(&dir).unwrap();
    assert_eq!(
        store.schema_version().unwrap(),
        kura_store::CURRENT_SCHEMA_VERSION
    );

    let counts = count_seeded_rows(&store).unwrap();
    // Runtime chain: session -> run -> step -> tool_call + llm dispatch + checkpoint.
    assert_eq!(counts.get("sessions"), Some(&1));
    assert_eq!(counts.get("runs"), Some(&1));
    assert_eq!(counts.get("steps"), Some(&1));
    assert_eq!(counts.get("tool_calls"), Some(&1));
    assert_eq!(counts.get("llm_dispatches"), Some(&1));
    assert_eq!(counts.get("checkpoints"), Some(&1));
    // Schedules.
    assert_eq!(counts.get("schedules"), Some(&1));
    assert_eq!(counts.get("schedule_targets"), Some(&1));
    assert_eq!(counts.get("schedule_dispatch_attempts"), Some(&1));
    // Workflows.
    assert_eq!(counts.get("workflows"), Some(&1));
    assert_eq!(counts.get("workflow_steps"), Some(&1));
    assert_eq!(counts.get("workflow_dependencies"), Some(&1));
    assert_eq!(counts.get("workflow_handoffs"), Some(&1));
    // Integrations + delivery.
    assert_eq!(counts.get("integrations"), Some(&2)); // calendar + mail integrations
    assert_eq!(counts.get("delivery_targets"), Some(&1));
    assert_eq!(counts.get("delivery_outcomes"), Some(&1));
    assert_eq!(counts.get("delivery_attempts"), Some(&1));
    // Calendar.
    assert_eq!(counts.get("calendar_accounts"), Some(&1));
    assert_eq!(counts.get("calendar_operations"), Some(&1));
    assert_eq!(counts.get("calendar_artifacts"), Some(&1));
    // Mail.
    assert_eq!(counts.get("mail_accounts"), Some(&1));
    assert_eq!(counts.get("mail_operations"), Some(&1));
    assert_eq!(counts.get("mail_artifacts"), Some(&1));
    // Reminders.
    assert_eq!(counts.get("reminders"), Some(&1));
    assert_eq!(counts.get("reminder_occurrences"), Some(&1));
    assert_eq!(counts.get("reminder_actions"), Some(&1));
    // Computer use.
    assert_eq!(counts.get("computer_use_sessions"), Some(&1));
    assert_eq!(counts.get("computer_use_actions"), Some(&1));
    assert_eq!(counts.get("computer_use_artifacts"), Some(&1));
    // Approvals / decisions.
    assert_eq!(counts.get("approvals"), Some(&1));
    assert_eq!(counts.get("decisions"), Some(&1));
    // Evaluation replay harness.
    assert_eq!(counts.get("evaluation_replay_candidates"), Some(&1));
    assert_eq!(counts.get("evaluation_replay_attempts"), Some(&1));
    // Harness support rows.
    assert_eq!(counts.get("consumer_policy_records"), Some(&1));
    assert_eq!(counts.get("provider_preferences"), Some(&1));
    assert_eq!(counts.get("secret_scope_bindings"), Some(&1));
    assert_eq!(counts.get("sandbox_executions"), Some(&1));
    // Connector messages (session-parented, run-parented, orphan).
    assert_eq!(counts.get("connector_messages"), Some(&3));
    // Events (run, system, connector, legacy connector, capability-only).
    assert_eq!(counts.get("events"), Some(&5));
}

#[test]
fn pre_tenant_v21_fixture_seeds_exact_ids_and_payloads() {
    let dir = temp_dir("pre_tenant_v21_ids");
    let store = build_pre_tenant_v21_fixture(&dir).unwrap();
    let conn = open_conn(store.db_path());

    let session: (String, String, String, String, String, String, i64) = conn
        .query_row(
            "SELECT kind, status, channel, peer_id, routing_key, last_active_at, generation FROM sessions WHERE session_id = 'sess_seed'",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(
        session,
        (
            "chat".to_string(),
            "active".to_string(),
            "test".to_string(),
            "peer_1".to_string(),
            "rk_seed".to_string(),
            FIXTURE_TIMESTAMP.to_string(),
            1
        )
    );

    let run: (String, String, String) = conn
        .query_row(
            "SELECT session_id, entrypoint, status FROM runs WHERE run_id = 'run_seed'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        run,
        (
            "sess_seed".to_string(),
            "test".to_string(),
            "queued".to_string()
        )
    );

    // The checkpoint snapshot_json carries the exact Go payload.
    let snapshot: String = conn
        .query_row(
            "SELECT snapshot_json FROM checkpoints WHERE checkpoint_id = 'chk_seed'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        snapshot,
        r#"{"run":{"runId":"run_seed","entrypoint":"test","status":"queued","goal":"g","createdAt":"2025-01-01T00:00:00Z","updatedAt":"2025-01-01T00:00:00Z"},"steps":[],"toolCalls":[]}"#
    );

    // Connector messages: session-parented, run-parented, orphan.
    let cm_session: Option<String> = conn
        .query_row(
            "SELECT session_id FROM connector_messages WHERE delivery_id = 'cm_seed_session'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(cm_session.as_deref(), Some("sess_seed"));
    let cm_run: Option<String> = conn
        .query_row(
            "SELECT run_id FROM connector_messages WHERE delivery_id = 'cm_seed_run'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(cm_run.as_deref(), Some("run_seed"));
    let orphan_session: Option<String> = conn
        .query_row(
            "SELECT session_id FROM connector_messages WHERE delivery_id = 'cm_seed_orphan'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(orphan_session.is_none());

    // Events cover the four shapes (run / system / connector / capability).
    assert_eq!(count(&conn, "events"), 5);
    let run_event: (Option<String>, Option<String>, String, String) = conn
        .query_row(
            "SELECT run_id, connector_id, resource_kind, resource_id FROM events WHERE event_id = 'evt_run_seed'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        run_event,
        (
            Some("run_seed".to_string()),
            None,
            "run".to_string(),
            "run_seed".to_string()
        )
    );
    let cap_event: (Option<String>, String, String) = conn
        .query_row(
            "SELECT capability_id, resource_kind, resource_id FROM events WHERE event_id = 'evt_cap_only'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        cap_event,
        (
            Some("cap_a".to_string()),
            "capability".to_string(),
            "cap_a".to_string()
        )
    );
}
