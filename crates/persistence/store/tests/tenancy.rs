//! Integration tests for the tenant-aware store primitives (rs/store/src/tenancy.rs).
//!
//! Covers RAW-method round-trips and the fail-closed tenant-scoping semantics:
//! cross-tenant by-id lookups return the ERR_CROSS_TENANT_ROW sentinel, lists never
//! leak rows owned by other tenants, and the *ForTenantSafe upserts atomically
//! refuse to clobber a row owned by a different tenant.

use chrono::Utc;
use kura_events::{Event, Filter, Resource, Scope};
use kura_identity::{LifecycleStatus, Tenant, TenantKind};
use kura_llm::{Dispatch, DispatchStatus, Message, MessageRole, Usage};
use kura_policy::{Approval, ApprovalStatus, Decision, DecisionOutcome};
use kura_router::{Session, SessionKind, SessionStatus};
use kura_runtime::{Run, RunCheckpoint, RunStatus, Step, StepStatus, ToolCall, ToolCallStatus};
use kura_store::SQLiteStore;

fn temp_dir(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("kura_store_tenancy_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().to_string()
}

fn make_run(id: &str, tenant: &str) -> Run {
    let now = Utc::now();
    Run {
        run_id: id.to_string(),
        session_id: String::new(),
        entrypoint: "test entrypoint".to_string(),
        status: RunStatus::Running,
        goal: "test goal".to_string(),
        created_at: now,
        updated_at: now,
        ..Run::default()
    }
}

fn make_step(id: &str, run_id: &str) -> Step {
    let now = Utc::now();
    Step {
        step_id: id.to_string(),
        run_id: run_id.to_string(),
        attempt: 1,
        title: "Do the thing".to_string(),
        kind: "task".to_string(),
        status: StepStatus::Completed,
        created_at: now,
        updated_at: now,
        input: Some(serde_json::json!({"a": 1})),
        output: Some(serde_json::json!({"b": "done"})),
        ..Step::default()
    }
}

fn make_tool_call(id: &str, run_id: &str, step_id: &str) -> ToolCall {
    let now = Utc::now();
    ToolCall {
        tool_call_id: id.to_string(),
        run_id: run_id.to_string(),
        step_id: step_id.to_string(),
        invocation_kind: "local_tool".to_string(),
        capability_id: "cap_1".to_string(),
        tool_name: "lookup".to_string(),
        status: ToolCallStatus::Completed,
        input: Some(serde_json::json!({"q": "hi"})),
        output: Some(serde_json::json!({"r": 1})),
        created_at: now,
        updated_at: now,
        ..ToolCall::default()
    }
}

fn make_session(id: &str) -> Session {
    let now = Utc::now();
    Session {
        session_id: id.to_string(),
        kind: SessionKind::Direct,
        status: SessionStatus::Active,
        channel: "cli".to_string(),
        account_id: String::new(),
        peer_id: "peer_1".to_string(),
        thread_id: String::new(),
        routing_key: format!("rk_{id}"),
        generation: 1,
        created_at: now,
        updated_at: now,
        last_active_at: now,
        last_reset_at: None,
        active_profile_projection: None,
    }
}

fn make_dispatch(id: &str) -> Dispatch {
    let now = Utc::now();
    Dispatch {
        tools: Vec::new(),
        tool_calls: Vec::new(),
        dispatch_id: id.to_string(),
        provider: "openai".to_string(),
        model: "gpt-4o".to_string(),
        messages: vec![Message { role: MessageRole::User, content: "hi".to_string() }],
        stream: false,
        status: DispatchStatus::Completed,
        output: "hello".to_string(),
        finish_reason: "stop".to_string(),
        usage: Usage { input_tokens: 3, output_tokens: 1, total_tokens: 4 },
        error_code: String::new(),
        error: String::new(),
        timeout_ms: 30000,
        partial: false,
        max_retries: 2,
        attempt_count: 1,
        created_at: now,
        updated_at: now,
        started_at: Some(now),
        completed_at: Some(now),
    }
}

fn make_checkpoint(run: &Run) -> RunCheckpoint {
    RunCheckpoint {
        run: run.clone(),
        steps: Vec::new(),
        tool_calls: Vec::new(),
        captured_at: Utc::now(),
    }
}

fn make_event(id: &str) -> Event {
    Event {
        event_id: id.to_string(),
        category: "run".to_string(),
        name: "run.created".to_string(),
        resource: Resource { kind: "run".to_string(), id: "run_1".to_string() },
        scope: Scope::default(),
        ..Event::default()
    }
}

#[test]
fn runs_round_trip_and_tenant_scoping() {
    let store = SQLiteStore::new(&temp_dir("runs_scope")).unwrap();
    store.upsert_run_for_tenant_safe(&make_run("run_a", "ten_a"), "ten_a").unwrap();
    store.upsert_run_for_tenant_safe(&make_run("run_b", "ten_b"), "ten_b").unwrap();

    let a = store.list_runs_for_tenant_raw("ten_a").unwrap();
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].run_id, "run_a");

    let b = store.list_runs_for_tenant_raw("ten_b").unwrap();
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].run_id, "run_b");

    // NULL-tenant rows are NOT returned by the tenant-aware list (fail-closed).
    store.upsert_run(&make_run("run_unbound", "unused")).unwrap();
    assert_eq!(store.list_runs_for_tenant_raw("ten_a").unwrap().len(), 1);
}

#[test]
fn get_run_for_tenant_raw_cross_tenant_is_sentinel() {
    let store = SQLiteStore::new(&temp_dir("run_get")).unwrap();
    store.upsert_run_for_tenant_safe(&make_run("run_a", "ten_a"), "ten_a").unwrap();

    // Own tenant: found.
    let run = store.get_run_for_tenant_raw("run_a", "ten_a").unwrap().expect("owned run");
    assert_eq!(run.run_id, "run_a");
    // Missing: Ok(None), no error.
    assert!(store.get_run_for_tenant_raw("nope", "ten_a").unwrap().is_none());
    // Cross-tenant: the sentinel, and never a Some(run).
    let err = store.get_run_for_tenant_raw("run_a", "ten_b").unwrap_err();
    assert!(SQLiteStore::is_cross_tenant_row(&err));
    assert_eq!(err, SQLiteStore::ERR_CROSS_TENANT_ROW);
}

#[test]
fn upsert_run_for_tenant_safe_refuses_cross_tenant_write() {
    let store = SQLiteStore::new(&temp_dir("run_write")).unwrap();
    let mut run = make_run("run_a", "ten_a");
    run.goal = "original".to_string();
    store.upsert_run_for_tenant_safe(&run, "ten_a").unwrap();

    // Tenant B attempts to overwrite A's row: refused atomically, row preserved.
    let mut hijack = make_run("run_a", "ten_b");
    hijack.goal = "hijacked".to_string();
    let err = store.upsert_run_for_tenant_safe(&hijack, "ten_b").unwrap_err();
    assert!(SQLiteStore::is_cross_tenant_row(&err));

    let run = store.get_run_for_tenant_raw("run_a", "ten_a").unwrap().expect("still owned by A");
    assert_eq!(run.goal, "original");

    // Same-tenant re-write is idempotent.
    let mut updated = run.clone();
    updated.goal = "updated".to_string();
    store.upsert_run_for_tenant_safe(&updated, "ten_a").unwrap();
    assert_eq!(store.get_run_for_tenant_raw("run_a", "ten_a").unwrap().unwrap().goal, "updated");
}

#[test]
fn bind_and_delete_row_for_tenant() {
    let store = SQLiteStore::new(&temp_dir("bind_delete")).unwrap();
    // Tenantless write leaves tenant_id NULL; bind claims it.
    store.upsert_run(&make_run("run_bind", "unused")).unwrap();
    assert_eq!(store.lookup_row_tenant("runs", "run_id", "run_bind").unwrap(), None);
    store.bind_row_tenant("runs", "run_id", "run_bind", "ten_a").unwrap();
    assert_eq!(
        store.lookup_row_tenant("runs", "run_id", "run_bind").unwrap(),
        Some("ten_a".to_string())
    );

    // Cross-tenant bind is refused.
    let err = store.bind_row_tenant("runs", "run_id", "run_bind", "ten_b").unwrap_err();
    assert!(SQLiteStore::is_cross_tenant_row(&err));

    // Delete: own tenant deletes, missing row false, cross-tenant refused.
    store.upsert_run_for_tenant_safe(&make_run("run_del", "ten_a"), "ten_a").unwrap();
    assert!(store.delete_row_for_tenant("runs", "run_id", "run_del", "ten_a").unwrap());
    assert!(!store.delete_row_for_tenant("runs", "run_id", "run_del", "ten_a").unwrap());
    let err = store.delete_row_for_tenant("runs", "run_id", "run_bind", "ten_b").unwrap_err();
    assert!(SQLiteStore::is_cross_tenant_row(&err));
}

#[test]
fn steps_and_tool_calls_round_trip_and_scoping() {
    let store = SQLiteStore::new(&temp_dir("steps_tools")).unwrap();
    let run = make_run("run_st", "ten_a");
    store.upsert_run_for_tenant_safe(&run, "ten_a").unwrap();
    let step = make_step("step_a", "run_st");
    store.upsert_step_for_tenant_safe(&step, "ten_a").unwrap();
    let tc = make_tool_call("tc_a", "run_st", "step_a");
    store.upsert_tool_call_for_tenant_safe(&tc, "ten_a").unwrap();

    // Tenant A sees both; tenant B sees neither.
    let a_steps = store.list_steps_for_tenant_raw("ten_a", "run_st").unwrap();
    assert_eq!(a_steps.len(), 1);
    assert_eq!(a_steps[0].step_id, "step_a");
    let a_tcs = store.list_tool_calls_for_tenant_raw("ten_a", "run_st", "step_a").unwrap();
    assert_eq!(a_tcs.len(), 1);
    assert_eq!(a_tcs[0].tool_call_id, "tc_a");
    assert!(store.list_steps_for_tenant_raw("ten_b", "run_st").unwrap().is_empty());
    assert!(store.list_tool_calls_for_tenant_raw("ten_b", "run_st", "step_a").unwrap().is_empty());

    // Cross-tenant step upsert is refused atomically.
    let mut hijack = step.clone();
    hijack.title = "hijacked".to_string();
    let err = store.upsert_step_for_tenant_safe(&hijack, "ten_b").unwrap_err();
    assert!(SQLiteStore::is_cross_tenant_row(&err));
    assert_eq!(store.list_steps_for_tenant_raw("ten_a", "run_st").unwrap()[0].title, "Do the thing");
}

#[test]
fn sessions_round_trip_and_scoping() {
    let store = SQLiteStore::new(&temp_dir("sessions")).unwrap();
    store.upsert_session_for_tenant_safe(&make_session("sess_a"), "ten_a").unwrap();
    store.upsert_session_for_tenant_safe(&make_session("sess_b"), "ten_b").unwrap();

    let a = store.list_sessions_for_tenant_raw("ten_a").unwrap();
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].session_id, "sess_a");
    assert_eq!(store.list_sessions_for_tenant_raw("ten_b").unwrap()[0].session_id, "sess_b");
}

#[test]
fn llm_dispatches_round_trip_and_scoping() {
    let store = SQLiteStore::new(&temp_dir("llm")).unwrap();
    store.upsert_llm_dispatch_for_tenant_safe(&make_dispatch("disp_a"), "ten_a").unwrap();
    store.upsert_llm_dispatch_for_tenant_safe(&make_dispatch("disp_b"), "ten_b").unwrap();

    let a = store.list_llm_dispatches_for_tenant_raw("ten_a").unwrap();
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].dispatch_id, "disp_a");

    // Cross-tenant by-id lookup -> sentinel; missing -> None.
    let err = store.get_llm_dispatch_for_tenant_raw("disp_a", "ten_b").unwrap_err();
    assert!(SQLiteStore::is_cross_tenant_row(&err));
    assert!(store.get_llm_dispatch_for_tenant_raw("disp_a", "ten_a").unwrap().is_some());
    assert!(store.get_llm_dispatch_for_tenant_raw("nope", "ten_a").unwrap().is_none());

    // Cross-tenant write refused.
    let err = store.upsert_llm_dispatch_for_tenant_safe(&make_dispatch("disp_a"), "ten_b").unwrap_err();
    assert!(SQLiteStore::is_cross_tenant_row(&err));
}

#[test]
fn checkpoints_round_trip_and_scoping() {
    let store = SQLiteStore::new(&temp_dir("checkpoints")).unwrap();
    let run = make_run("run_ck", "ten_a");
    store.upsert_run_for_tenant_safe(&run, "ten_a").unwrap();
    store.save_checkpoint_for_tenant_safe(&make_checkpoint(&run), "ten_a").unwrap();

    let owned = store.list_latest_checkpoints_for_tenant_raw("ten_a").unwrap();
    assert_eq!(owned.len(), 1);
    assert_eq!(owned[0].run.run_id, "run_ck");
    assert!(store.list_latest_checkpoints_for_tenant_raw("ten_b").unwrap().is_empty());
}

#[test]
fn run_exists_for_tenant_does_not_leak() {
    let store = SQLiteStore::new(&temp_dir("run_exists")).unwrap();
    store.upsert_run_for_tenant_safe(&make_run("run_ex", "ten_a"), "ten_a").unwrap();
    assert!(store.run_exists_for_tenant("run_ex", "ten_a").unwrap());
    // Cross-tenant and missing are indistinguishable: both false.
    assert!(!store.run_exists_for_tenant("run_ex", "ten_b").unwrap());
    assert!(!store.run_exists_for_tenant("missing", "ten_a").unwrap());
}

#[test]
fn events_append_and_list_for_tenant() {
    let store = SQLiteStore::new(&temp_dir("events")).unwrap();
    let persisted = store.append_event_for_tenant_raw(&make_event("evt_a"), "ten_a").unwrap();
    assert_eq!(persisted.tenant_id, "ten_a");
    assert!(persisted.sequence > 0);
    store.append_event_for_tenant_raw(&make_event("evt_b"), "ten_b").unwrap();

    let a = store.list_events_for_tenant_raw("ten_a", &Filter::default()).unwrap();
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].event_id, "evt_a");

    // Duplicate event_id from another tenant -> sentinel, row preserved.
    let err = store.append_event_for_tenant_raw(&make_event("evt_a"), "ten_b").unwrap_err();
    assert!(SQLiteStore::is_cross_tenant_row(&err));
    assert_eq!(store.list_events_for_tenant_raw("ten_a", &Filter::default()).unwrap().len(), 1);

    // Filter by category scopes the tenant list.
    let filter = Filter { category: "run".to_string(), ..Filter::default() };
    assert_eq!(store.list_events_for_tenant_raw("ten_a", &filter).unwrap().len(), 1);
}

#[test]
fn approvals_and_decisions_for_tenant() {
    let store = SQLiteStore::new(&temp_dir("approvals")).unwrap();
    let now = Utc::now();
    let approval = Approval {
        approval_id: "apr_a".to_string(),
        action: "run_tool".to_string(),
        resource_kind: "tool_call".to_string(),
        resource_id: "tc_1".to_string(),
        reason: "allow".to_string(),
        status: ApprovalStatus::Pending,
        created_at: now,
        updated_at: now,
        ..Approval::default()
    };
    store.upsert_approval(&approval).unwrap();
    store.bind_row_tenant("approvals", "approval_id", "apr_a", "ten_a").unwrap();

    let decision = Decision {
        decision_id: "dec_a".to_string(),
        action: "run_tool".to_string(),
        resource_kind: "tool_call".to_string(),
        resource_id: "tc_1".to_string(),
        outcome: DecisionOutcome::Approved,
        reason: "ok".to_string(),
        approval_id: "apr_a".to_string(),
        created_at: now,
        ..Decision::default()
    };
    store.upsert_decision(&decision).unwrap();
    store.bind_row_tenant("decisions", "decision_id", "dec_a", "ten_a").unwrap();

    assert_eq!(store.list_approvals_for_tenant_raw("ten_a").unwrap().len(), 1);
    assert_eq!(store.list_decisions_for_tenant_raw("ten_a").unwrap().len(), 1);
    assert!(store.list_approvals_for_tenant_raw("ten_b").unwrap().is_empty());
    assert!(store.list_decisions_for_tenant_raw("ten_b").unwrap().is_empty());
}

#[test]
fn default_tenant_resolver_fails_closed_then_resolves() {
    let store = SQLiteStore::new(&temp_dir("default_tenant")).unwrap();

    // Pre-bootstrap: resolver returns the unavailable sentinel and binding returns None.
    let err = store.resolve_default_personal_tenant_id().unwrap_err();
    assert!(SQLiteStore::is_default_personal_tenant_unavailable(&err));
    assert!(store.resolve_default_tenant_binding().is_none());

    // Seed a personal tenant via the identity store.
    let now = Utc::now();
    store
        .upsert_tenant(&Tenant {
            tenant_id: "ten_default".to_string(),
            tenant_kind: TenantKind::Personal,
            display_name: "Operator".to_string(),
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
        })
        .unwrap();

    assert_eq!(store.resolve_default_personal_tenant_id().unwrap(), "ten_default");
    assert_eq!(store.resolve_default_tenant_binding().unwrap(), "ten_default");
}
