//! Pre-tenant (schema v21) seed data: at least one parent + one child row in
//! every in-scope tenant-owned table, so the head-migration backfill drivers
//! are exercised end-to-end. Port of daemon/internal/store/migrationfixture/seeds.go.

use std::collections::HashMap;

use rusqlite::{Connection, params};

use kura_store::SQLiteStore;

use crate::{FIXTURE_TIMESTAMP, open_fixture_connection};

pub const TS: &str = FIXTURE_TIMESTAMP;

/// Row counts for every table the pre-tenant fixture seeded (Go SeedRowCounts).
pub type SeedRowCounts = HashMap<String, i64>;

pub(crate) fn query_head(query: &str) -> &str {
    query.split('(').next().unwrap_or(query).trim()
}

pub(crate) fn exec_insert(
    conn: &Connection,
    query: &str,
    values: &[&dyn rusqlite::ToSql],
) -> Result<(), String> {
    conn.execute(query, values)
        .map(|_| ())
        .map_err(|e| format!("seed {}: {e}", query_head(query)))
}

fn count_rows(conn: &Connection, table: &str) -> Result<i64, String> {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .map_err(|e| format!("count {table}: {e}"))
}

/// Seeds the pre-tenant rows into a store opened at v21 (Go BuildPreTenantV21Fixture
/// seeding half). Runs through a sibling connection to the store's SQLite file.
pub fn seed_pre_tenant_v21(store: &SQLiteStore) -> Result<(), String> {
    let conn = open_fixture_connection(store.db_path())?;
    seed_runtime(&conn)?;
    seed_schedules(&conn)?;
    seed_workflows(&conn)?;
    seed_integrations_delivery(&conn)?;
    seed_calendar_mail_reminders(&conn)?;
    seed_computer_use(&conn)?;
    seed_approvals_decisions(&conn)?;
    seed_evaluation(&conn)?;
    seed_harness(&conn)?;
    seed_connector_messages(&conn)?;
    seed_events(&conn)?;
    Ok(())
}

fn seed_runtime(conn: &Connection) -> Result<(), String> {
    // sess_seed intentionally omits account_id/thread_id so head migrations can
    // exercise partial legacy session projection without connector-local state.
    exec_insert(
        conn,
        "INSERT INTO sessions (session_id, kind, status, channel, peer_id, routing_key, generation, created_at, updated_at, last_active_at) VALUES (?,?,?,?,?,?,?,?,?,?)",
        params![
            "sess_seed",
            "chat",
            "active",
            "test",
            "peer_1",
            "rk_seed",
            1i64,
            TS,
            TS,
            TS
        ],
    )?;
    exec_insert(
        conn,
        "INSERT INTO runs (run_id, session_id, entrypoint, status, goal, created_at, updated_at) VALUES (?,?,?,?,?,?,?)",
        params!["run_seed", "sess_seed", "test", "queued", "g", TS, TS],
    )?;
    exec_insert(
        conn,
        "INSERT INTO steps (step_id, run_id, title, kind, status, created_at, updated_at) VALUES (?,?,?,?,?,?,?)",
        params!["step_seed", "run_seed", "step", "model", "queued", TS, TS],
    )?;
    exec_insert(
        conn,
        "INSERT INTO tool_calls (tool_call_id, run_id, step_id, capability_id, tool_name, status, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?)",
        params![
            "tc_seed",
            "run_seed",
            "step_seed",
            "cap_a",
            "tool_a",
            "requested",
            TS,
            TS
        ],
    )?;
    exec_insert(
        conn,
        "INSERT INTO llm_dispatches (dispatch_id, provider, model, messages_json, stream, status, output_text, usage_json, timeout_ms, max_retries, attempt_count, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
        params![
            "dsp_seed",
            "openai",
            "gpt",
            "[]",
            0i64,
            "completed",
            "",
            "{}",
            1000i64,
            0i64,
            1i64,
            TS,
            TS
        ],
    )?;
    exec_insert(
        conn,
        "INSERT INTO checkpoints (checkpoint_id, run_id, captured_at, snapshot_json) VALUES (?,?,?,?)",
        params![
            "chk_seed",
            "run_seed",
            TS,
            "{\"run\":{\"runId\":\"run_seed\",\"entrypoint\":\"test\",\"status\":\"queued\",\"goal\":\"g\",\"createdAt\":\"2025-01-01T00:00:00Z\",\"updatedAt\":\"2025-01-01T00:00:00Z\"},\"steps\":[],\"toolCalls\":[]}"
        ],
    )?;
    Ok(())
}

fn seed_schedules(conn: &Connection) -> Result<(), String> {
    exec_insert(
        conn,
        "INSERT INTO schedules (schedule_id, environment_scope, kind, status, target_ref_id, created_at, updated_at, document_json) VALUES (?,?,?,?,?,?,?,?)",
        params![
            "sch_seed", "test", "cron", "active", "tgt_seed", TS, TS, "{}"
        ],
    )?;
    exec_insert(
        conn,
        "INSERT INTO schedule_targets (target_ref_id, schedule_id, target_kind, revision, active, updated_at, document_json) VALUES (?,?,?,?,?,?,?)",
        params!["tgt_seed", "sch_seed", "workflow", 1i64, 1i64, TS, "{}"],
    )?;
    exec_insert(
        conn,
        "INSERT INTO schedule_dispatch_attempts (attempt_id, schedule_id, due_at, trigger_source, dispatch_status, retry_count, retry_budget, resolved_target_revision, downstream_status, missed_count, created_at, updated_at, document_json) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
        params![
            "atm_sch_seed",
            "sch_seed",
            TS,
            "auto",
            "queued",
            0i64,
            3i64,
            1i64,
            "pending",
            0i64,
            TS,
            TS,
            "{}"
        ],
    )?;
    Ok(())
}

fn seed_workflows(conn: &Connection) -> Result<(), String> {
    exec_insert(
        conn,
        "INSERT INTO workflows (workflow_id, run_id, environment_scope, goal, status, created_at, updated_at, document_json) VALUES (?,?,?,?,?,?,?,?)",
        params!["wf_seed", "run_seed", "test", "g", "queued", TS, TS, "{}"],
    )?;
    exec_insert(
        conn,
        "INSERT INTO workflow_steps (workflow_step_id, workflow_id, position, status, attempt_count, max_attempts, document_json) VALUES (?,?,?,?,?,?,?)",
        params!["wfs_seed", "wf_seed", 1i64, "queued", 0i64, 3i64, "{}"],
    )?;
    exec_insert(
        conn,
        "INSERT INTO workflow_dependencies (dependency_id, workflow_id, document_json) VALUES (?,?,?)",
        params!["wdep_seed", "wf_seed", "{}"],
    )?;
    exec_insert(
        conn,
        "INSERT INTO workflow_handoffs (handoff_id, workflow_id, status, document_json) VALUES (?,?,?,?)",
        params!["whof_seed", "wf_seed", "pending", "{}"],
    )?;
    Ok(())
}

fn seed_integrations_delivery(conn: &Connection) -> Result<(), String> {
    exec_insert(
        conn,
        "INSERT INTO integrations (integration_id, domain_kind, environment_scope, backend_kind, readiness_status, canonical_default, updated_at, document_json) VALUES (?,?,?,?,?,?,?,?)",
        params![
            "int_seed", "calendar", "test", "google", "ready", 1i64, TS, "{}"
        ],
    )?;
    exec_insert(
        conn,
        "INSERT INTO delivery_targets (target_id, environment_scope, target_kind, status, updated_at, document_json) VALUES (?,?,?,?,?,?)",
        params!["deltgt_seed", "test", "discord", "ready", TS, "{}"],
    )?;
    exec_insert(
        conn,
        "INSERT INTO delivery_preferences (preference_id, environment_scope, scope_kind, active, updated_at, document_json) VALUES (?,?,?,?,?,?)",
        params!["delpref_seed", "test", "global", 1i64, TS, "{}"],
    )?;
    exec_insert(
        conn,
        "INSERT INTO delivery_outcomes (delivery_id, environment_scope, source_kind, source_id, status, updated_at, document_json) VALUES (?,?,?,?,?,?,?)",
        params!["delout_seed", "test", "run", "run_seed", "queued", TS, "{}"],
    )?;
    exec_insert(
        conn,
        "INSERT INTO delivery_attempts (attempt_id, delivery_id, attempt_number, target_id, status, document_json) VALUES (?,?,?,?,?,?)",
        params![
            "atm_del_seed",
            "delout_seed",
            1i64,
            "deltgt_seed",
            "queued",
            "{}"
        ],
    )?;
    exec_insert(
        conn,
        "INSERT INTO delivery_summary_windows (summary_window_id, environment_scope, target_id, preference_id, status, window_ends_at, updated_at, document_json) VALUES (?,?,?,?,?,?,?,?)",
        params![
            "sw_seed",
            "test",
            "deltgt_seed",
            "delpref_seed",
            "open",
            TS,
            TS,
            "{}"
        ],
    )?;
    Ok(())
}

fn seed_calendar_mail_reminders(conn: &Connection) -> Result<(), String> {
    exec_insert(
        conn,
        "INSERT INTO calendar_accounts (calendar_account_id, integration_id, environment_scope, readiness_status, canonical_default, updated_at, document_json) VALUES (?,?,?,?,?,?,?)",
        params!["cal_seed", "int_seed", "test", "ready", 1i64, TS, "{}"],
    )?;
    exec_insert(
        conn,
        "INSERT INTO calendar_operations (operation_id, integration_id, calendar_account_id, environment_scope, operation_class, status, updated_at, document_json) VALUES (?,?,?,?,?,?,?,?)",
        params![
            "calop_seed",
            "int_seed",
            "cal_seed",
            "test",
            "list",
            "ok",
            TS,
            "{}"
        ],
    )?;
    exec_insert(
        conn,
        "INSERT INTO calendar_artifacts (artifact_id, operation_id, integration_id, environment_scope, kind, created_at, document_json) VALUES (?,?,?,?,?,?,?)",
        params![
            "calart_seed",
            "calop_seed",
            "int_seed",
            "test",
            "event",
            TS,
            "{}"
        ],
    )?;
    // Mail uses its own integration row (FK is UNIQUE on integration_id).
    exec_insert(
        conn,
        "INSERT INTO integrations (integration_id, domain_kind, environment_scope, backend_kind, readiness_status, canonical_default, updated_at, document_json) VALUES (?,?,?,?,?,?,?,?)",
        params![
            "int_mail_seed",
            "mail",
            "test",
            "gmail",
            "ready",
            1i64,
            TS,
            "{}"
        ],
    )?;
    exec_insert(
        conn,
        "INSERT INTO mail_accounts (mail_account_id, integration_id, environment_scope, readiness_status, canonical_default, updated_at, document_json) VALUES (?,?,?,?,?,?,?)",
        params![
            "mail_seed",
            "int_mail_seed",
            "test",
            "ready",
            1i64,
            TS,
            "{}"
        ],
    )?;
    exec_insert(
        conn,
        "INSERT INTO mail_operations (operation_id, integration_id, mail_account_id, environment_scope, operation_class, status, result_mode, updated_at, document_json) VALUES (?,?,?,?,?,?,?,?,?)",
        params![
            "mailop_seed",
            "int_mail_seed",
            "mail_seed",
            "test",
            "list",
            "ok",
            "json",
            TS,
            "{}"
        ],
    )?;
    exec_insert(
        conn,
        "INSERT INTO mail_artifacts (artifact_id, operation_id, integration_id, environment_scope, kind, created_at, document_json) VALUES (?,?,?,?,?,?,?)",
        params![
            "mailart_seed",
            "mailop_seed",
            "int_mail_seed",
            "test",
            "message",
            TS,
            "{}"
        ],
    )?;
    exec_insert(
        conn,
        "INSERT INTO reminders (reminder_id, environment_scope, behavior_mode, current_state, updated_at, document_json) VALUES (?,?,?,?,?,?)",
        params!["rem_seed", "test", "single", "active", TS, "{}"],
    )?;
    exec_insert(
        conn,
        "INSERT INTO reminder_occurrences (occurrence_id, reminder_id, environment_scope, state, scheduled_for, updated_at, document_json) VALUES (?,?,?,?,?,?,?)",
        params!["remocc_seed", "rem_seed", "test", "scheduled", TS, TS, "{}"],
    )?;
    exec_insert(
        conn,
        "INSERT INTO reminder_actions (action_id, reminder_id, action_kind, created_at, document_json) VALUES (?,?,?,?,?)",
        params!["remact_seed", "rem_seed", "fire", TS, "{}"],
    )?;
    Ok(())
}

fn seed_computer_use(conn: &Connection) -> Result<(), String> {
    exec_insert(
        conn,
        "INSERT INTO computer_use_sessions (computer_use_session_id, environment_scope, run_id, status, driver_kind, started_at, updated_at, document_json) VALUES (?,?,?,?,?,?,?,?)",
        params![
            "cus_seed",
            "test",
            "run_seed",
            "active",
            "playwright",
            TS,
            TS,
            "{}"
        ],
    )?;
    exec_insert(
        conn,
        "INSERT INTO computer_use_actions (computer_use_action_id, environment_scope, computer_use_session_id, run_id, action_kind, status, risk_level, requested_at, updated_at, document_json) VALUES (?,?,?,?,?,?,?,?,?,?)",
        params![
            "cua_seed",
            "test",
            "cus_seed",
            "run_seed",
            "click",
            "completed",
            "low",
            TS,
            TS,
            "{}"
        ],
    )?;
    exec_insert(
        conn,
        "INSERT INTO computer_use_artifacts (artifact_id, environment_scope, computer_use_session_id, computer_use_action_id, run_id, kind, status, byte_size, created_at, document_json) VALUES (?,?,?,?,?,?,?,?,?,?)",
        params![
            "cuart_seed",
            "test",
            "cus_seed",
            "cua_seed",
            "run_seed",
            "screenshot",
            "ready",
            0i64,
            TS,
            "{}"
        ],
    )?;
    Ok(())
}

fn seed_approvals_decisions(conn: &Connection) -> Result<(), String> {
    exec_insert(
        conn,
        "INSERT INTO approvals (approval_id, action, reason, status, created_at, updated_at) VALUES (?,?,?,?,?,?)",
        params!["apr_seed", "test.action", "r", "pending", TS, TS],
    )?;
    exec_insert(
        conn,
        "INSERT INTO decisions (decision_id, action, outcome, reason, approval_id, created_at) VALUES (?,?,?,?,?,?)",
        params!["dec_seed", "test.action", "approved", "ok", "apr_seed", TS],
    )?;
    Ok(())
}

fn seed_evaluation(conn: &Connection) -> Result<(), String> {
    exec_insert(
        conn,
        "INSERT INTO evaluation_replay_candidates (candidate_id, environment_scope, candidate_kind, source_kind, source_id, readiness_status, created_at, updated_at, document_json) VALUES (?,?,?,?,?,?,?,?,?)",
        params![
            "cand_seed",
            "test",
            "run",
            "run",
            "run_seed",
            "ready",
            TS,
            TS,
            "{}"
        ],
    )?;
    exec_insert(
        conn,
        "INSERT INTO evaluation_replay_attempts (attempt_id, candidate_id, environment_scope, mode, status, created_at, updated_at, document_json) VALUES (?,?,?,?,?,?,?,?)",
        params![
            "evatm_seed",
            "cand_seed",
            "test",
            "shadow",
            "queued",
            TS,
            TS,
            "{}"
        ],
    )?;
    Ok(())
}

fn seed_harness(conn: &Connection) -> Result<(), String> {
    exec_insert(
        conn,
        "INSERT INTO consumer_policy_records (policy_record_id, consumer_kind, consumer_id, operation_kind, status, decision, approval_status, secret_resolution, started_at, document_json) VALUES (?,?,?,?,?,?,?,?,?,?)",
        params![
            "polrec_seed",
            "skill",
            "skl_a",
            "tool_call",
            "completed",
            "allow",
            "n/a",
            "skipped",
            TS,
            "{}"
        ],
    )?;
    exec_insert(
        conn,
        "INSERT INTO provider_preferences (provider_id, default_model, updated_at) VALUES (?,?,?)",
        params!["openai", "gpt", TS],
    )?;
    exec_insert(
        conn,
        "INSERT INTO secret_scope_bindings (binding_id, consumer_kind, consumer_id, environment_scope, secret_ref, default_source, delivery_kind, active, document_json) VALUES (?,?,?,?,?,?,?,?,?)",
        params![
            "binding_seed",
            "skill",
            "skl_a",
            "test",
            "ref://x",
            "env",
            "header",
            1i64,
            "{}"
        ],
    )?;
    exec_insert(
        conn,
        "INSERT INTO sandbox_executions (execution_id, profile_id, backend_kind, status, requested_at, updated_at, document_json) VALUES (?,?,?,?,?,?,?)",
        params!["exec_seed", "default", "docker", "queued", TS, TS, "{}"],
    )?;
    // mcp_servers / mcp_tools / mcp_tool_exposure_rules are NOT seeded
    // here: the production app restore path expects tool documents with
    // a richer shape than a minimal fixture can provide, and the
    // backfill behavior for mcp_tool_exposure_rules (composite-PK bulk
    // fanout) is already covered by store/tenant_backfill_special_test.go.
    // Adding them to the fixture would bind the test to MCP restore
    // semantics that are orthogonal to Roadmap 35.
    Ok(())
}

fn seed_connector_messages(conn: &Connection) -> Result<(), String> {
    exec_insert(
        conn,
        "INSERT INTO connector_messages (delivery_id, connector_id, direction, channel_id, content, status, created_at, updated_at, session_id) VALUES (?,?,?,?,?,?,?,?,?)",
        params![
            "cm_seed_session",
            "discord_a",
            "out",
            "ch_a",
            "hi",
            "queued",
            TS,
            TS,
            "sess_seed"
        ],
    )?;
    exec_insert(
        conn,
        "INSERT INTO connector_messages (delivery_id, connector_id, direction, channel_id, content, status, created_at, updated_at, run_id) VALUES (?,?,?,?,?,?,?,?,?)",
        params![
            "cm_seed_run",
            "discord_a",
            "out",
            "ch_a",
            "hi",
            "queued",
            TS,
            TS,
            "run_seed"
        ],
    )?;
    exec_insert(
        conn,
        "INSERT INTO connector_messages (delivery_id, connector_id, direction, channel_id, content, status, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?)",
        params![
            "cm_seed_orphan",
            "discord_a",
            "out",
            "ch_a",
            "hi",
            "queued",
            TS,
            TS
        ],
    )?;
    Ok(())
}

fn seed_events(conn: &Connection) -> Result<(), String> {
    // Tenant-owned event with run_id parent.
    exec_insert(
        conn,
        "INSERT INTO events (event_id, category, name, occurred_at, run_id, resource_kind, resource_id, payload_json) VALUES (?,?,?,?,?,?,?,?)",
        params![
            "evt_run_seed",
            "run",
            "run.created",
            TS,
            "run_seed",
            "run",
            "run_seed",
            "{}"
        ],
    )?;
    // Global system event.
    exec_insert(
        conn,
        "INSERT INTO events (event_id, category, name, occurred_at, resource_kind, resource_id, payload_json) VALUES (?,?,?,?,?,?,?)",
        params![
            "evt_sys_seed",
            "system",
            "system.heartbeat",
            TS,
            "system",
            "heartbeat",
            "{}"
        ],
    )?;
    // Connector event with proper resource pointer.
    exec_insert(
        conn,
        "INSERT INTO events (event_id, category, name, occurred_at, connector_id, resource_kind, resource_id, payload_json) VALUES (?,?,?,?,?,?,?,?)",
        params![
            "evt_cm_seed",
            "connector.message",
            "connector.message.delivered",
            TS,
            "discord_a",
            "connector_message",
            "cm_seed_session",
            "{}"
        ],
    )?;
    // Legacy connector event missing resource pointer - should reclassify to connector_global.
    exec_insert(
        conn,
        "INSERT INTO events (event_id, category, name, occurred_at, connector_id, resource_kind, resource_id, payload_json) VALUES (?,?,?,?,?,?,?,?)",
        params![
            "evt_cm_orphan",
            "connector.legacy",
            "connector.legacy.event",
            TS,
            "discord_a",
            "",
            "",
            "{}"
        ],
    )?;
    // Capability-only event - should reclassify to capability_global.
    exec_insert(
        conn,
        "INSERT INTO events (event_id, category, name, occurred_at, capability_id, resource_kind, resource_id, payload_json) VALUES (?,?,?,?,?,?,?,?)",
        params![
            "evt_cap_only",
            "capability.heartbeat",
            "cap.heartbeat",
            TS,
            "cap_a",
            "capability",
            "cap_a",
            "{}"
        ],
    )?;
    Ok(())
}

/// Row-count snapshot for every table the fixture seeded, used to assert the
/// head migration is loss-less (Go CountSeededRows).
pub fn count_seeded_rows(store: &SQLiteStore) -> Result<SeedRowCounts, String> {
    let conn = open_fixture_connection(store.db_path())?;
    let tables = [
        "sessions",
        "runs",
        "steps",
        "tool_calls",
        "llm_dispatches",
        "checkpoints",
        "schedules",
        "schedule_targets",
        "schedule_dispatch_attempts",
        "workflows",
        "workflow_steps",
        "workflow_dependencies",
        "workflow_handoffs",
        "integrations",
        "delivery_targets",
        "delivery_outcomes",
        "delivery_attempts",
        "calendar_accounts",
        "calendar_operations",
        "calendar_artifacts",
        "mail_accounts",
        "mail_operations",
        "mail_artifacts",
        "reminders",
        "reminder_occurrences",
        "reminder_actions",
        "computer_use_sessions",
        "computer_use_actions",
        "computer_use_artifacts",
        "approvals",
        "decisions",
        "evaluation_replay_candidates",
        "evaluation_replay_attempts",
        "consumer_policy_records",
        "provider_preferences",
        "secret_scope_bindings",
        "sandbox_executions",
        "connector_messages",
        "events",
    ];
    let mut out = SeedRowCounts::new();
    for table in tables {
        out.insert(table.to_string(), count_rows(&conn, table)?);
    }
    Ok(out)
}
