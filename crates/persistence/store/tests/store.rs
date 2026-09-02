use std::path::Path;

use kura_store::{schema_migrations, SQLiteStore, CURRENT_SCHEMA_VERSION};

fn temp_dir(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("kura_store_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().to_string()
}

#[test]
fn opens_store_and_creates_schema_migrations_table() {
    let dir = temp_dir("open");
    let store = SQLiteStore::new(&dir).unwrap();
    assert_eq!(store.data_dir(), dir);
    assert!(Path::new(store.db_path()).exists());

    // All ported migrations are applied on open, up to the head of the ported list.
    let applied: i64 = store_conn_query(store.db_path(), "SELECT MAX(version) FROM schema_migrations");
    assert_eq!(applied, schema_migrations().last().unwrap().version);
}

#[test]
fn migrations_are_ordered_and_start_at_baseline() {
    let migrations = schema_migrations();
    assert!(migrations.len() >= 1);
    assert_eq!(migrations[0].version, 1);
    assert_eq!(migrations[0].name, "baseline_v1_first_release");
    for pair in migrations.windows(2) {
        assert!(pair[0].version < pair[1].version);
    }
    // The head constant must track the migration list. If it lags, a database
    // this build writes is rejected on reopen as "newer than supported"; if it
    // leads, an unmigrated database is accepted silently.
    assert_eq!(
        CURRENT_SCHEMA_VERSION,
        migrations.last().unwrap().version,
        "CURRENT_SCHEMA_VERSION must equal the highest migration version"
    );
}

fn store_conn_query(db_path: &str, query: &str) -> i64 {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    conn.query_row(query, [], |row| row.get(0)).unwrap()
}
use chrono::Utc;
use kura_capabilities::{Capability, Status as CapabilityStatus};
use kura_router::{Session, SessionKind, SessionStatus};
use kura_events::{Event, Filter, Resource, Scope};
use kura_llm::{Dispatch, DispatchStatus, Message, MessageRole, Usage};
use kura_policy::{Approval, ApprovalStatus, Decision, DecisionOutcome};
use kura_providers::{
    AuthMode, AuthState, AuthStatus, Check, CheckStatus, Family, Model, Preference,
};
use kura_runtime::{Run, RunCheckpoint, RunStatus, Step, StepStatus, ToolCall, ToolCallStatus};

fn make_run() -> Run {
    let now = Utc::now();
    Run {
        run_id: "run_test".to_string(),
        session_id: String::new(),
        entrypoint: "test entrypoint".to_string(),
        status: RunStatus::Running,
        goal: "test goal".to_string(),
        created_at: now,
        updated_at: now,
        ..Run::default()
    }
}

fn make_step() -> Step {
    let now = Utc::now();
    Step {
        step_id: "step_1".to_string(),
        run_id: "run_test".to_string(),
        workflow_id: "wf_1".to_string(),
        workflow_step_id: "wfs_1".to_string(),
        attempt: 2,
        title: "Do the thing".to_string(),
        kind: "task".to_string(),
        status: StepStatus::Completed,
        created_at: now,
        updated_at: now,
        input: Some(serde_json::json!({"a": 1})),
        output: Some(serde_json::json!({"b": "done"})),
    }
}

fn make_tool_call() -> ToolCall {
    let now = Utc::now();
    let mut sandbox = serde_json::Map::new();
    sandbox.insert("session".to_string(), serde_json::json!("s-1"));
    ToolCall {
        tool_call_id: "tc_1".to_string(),
        run_id: "run_test".to_string(),
        step_id: "step_1".to_string(),
        invocation_kind: "mcp_tool".to_string(),
        capability_id: "cap_1".to_string(),
        mcp_server_id: "mcp_1".to_string(),
        mcp_tool_name: "search".to_string(),
        tool_name: "search".to_string(),
        status: ToolCallStatus::Completed,
        sandbox_execution_id: "sand_1".to_string(),
        failure_class: "timeout".to_string(),
        error: "boom".to_string(),
        input: Some(serde_json::json!({"q": "hi"})),
        output: Some(serde_json::json!({"r": 1})),
        sandbox,
        integration_bindings: vec![kura_integrations::BindingSummary {
            integration_id: "int_1".to_string(),
            domain_kind: "calendar".to_string(),
            display_name: "Calendar".to_string(),
            readiness_at_invocation: kura_integrations::ReadinessStatus::Healthy,
            backend_kind: kura_integrations::BackendKind::Native,
            ..kura_integrations::BindingSummary::default()
        }],
        created_at: now,
        updated_at: now,
        ..ToolCall::default()
    }
}

#[test]
fn upsert_run_and_read_tenant_id() {
    let dir = temp_dir("run");
    let store = SQLiteStore::new(&dir).unwrap();
    store.upsert_run(&make_run()).unwrap();
    // No tenant is bound through the legacy path, so the tenant id is absent.
    assert_eq!(store.run_tenant_id("run_test").unwrap(), None);
}

#[test]
fn step_round_trips_through_sqlite() {
    let dir = temp_dir("step");
    let store = SQLiteStore::new(&dir).unwrap();
    store.upsert_run(&make_run()).unwrap();
    let step = make_step();
    store.upsert_step(&step).unwrap();

    let listed = store.list_steps("run_test").unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.step_id, "step_1");
    assert_eq!(got.run_id, "run_test");
    assert_eq!(got.workflow_id, "wf_1");
    assert_eq!(got.workflow_step_id, "wfs_1");
    assert_eq!(got.attempt, 2);
    assert_eq!(got.title, "Do the thing");
    assert_eq!(got.kind, "task");
    assert_eq!(got.status, StepStatus::Completed);
    assert_eq!(got.input, Some(serde_json::json!({"a": 1})));
    assert_eq!(got.output, Some(serde_json::json!({"b": "done"})));
}

#[test]
fn tool_call_round_trips_through_sqlite() {
    let dir = temp_dir("toolcall");
    let store = SQLiteStore::new(&dir).unwrap();
    store.upsert_run(&make_run()).unwrap();
    store.upsert_step(&make_step()).unwrap();
    let tc = make_tool_call();
    store.upsert_tool_call(&tc).unwrap();

    let listed = store.list_tool_calls("run_test", "step_1").unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.tool_call_id, "tc_1");
    assert_eq!(got.invocation_kind, "mcp_tool");
    assert_eq!(got.capability_id, "cap_1");
    assert_eq!(got.mcp_server_id, "mcp_1");
    assert_eq!(got.mcp_tool_name, "search");
    assert_eq!(got.status, ToolCallStatus::Completed);
    assert_eq!(got.sandbox_execution_id, "sand_1");
    assert_eq!(got.failure_class, "timeout");
    assert_eq!(got.error, "boom");
    assert_eq!(got.input, Some(serde_json::json!({"q": "hi"})));
    assert_eq!(got.output, Some(serde_json::json!({"r": 1})));
    assert_eq!(got.sandbox.get("session"), Some(&serde_json::json!("s-1")));
    assert_eq!(got.integration_bindings.len(), 1);
    assert_eq!(got.integration_bindings[0].integration_id, "int_1");
    assert_eq!(got.integration_bindings[0].backend_kind, kura_integrations::BackendKind::Native);
}

#[test]
fn checkpoint_round_trips_through_sqlite() {
    let dir = temp_dir("checkpoint");
    let store = SQLiteStore::new(&dir).unwrap();
    let run = make_run();
    let step = make_step();
    let tool_call = make_tool_call();
    store.upsert_run(&run).unwrap();
    store.upsert_step(&step).unwrap();
    store.upsert_tool_call(&tool_call).unwrap();

    let checkpoint = RunCheckpoint {
        run: run.clone(),
        steps: vec![step.clone()],
        tool_calls: vec![tool_call.clone()],
        captured_at: Utc::now(),
    };
    store.save_checkpoint(&checkpoint).unwrap();

    let listed = store.list_latest_checkpoints().unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.run.run_id, "run_test");
    assert_eq!(got.run.goal, "test goal");
    assert_eq!(got.steps.len(), 1);
    assert_eq!(got.steps[0].step_id, "step_1");
    assert_eq!(got.tool_calls.len(), 1);
    assert_eq!(got.tool_calls[0].tool_call_id, "tc_1");
}
#[test]
fn session_round_trips_through_sqlite() {
    let dir = temp_dir("session");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();
    let session = Session {
        session_id: "sess_1".to_string(),
        kind: SessionKind::Group,
        status: SessionStatus::Active,
        channel: "discord".to_string(),
        account_id: "acct_1".to_string(),
        peer_id: "peer_1".to_string(),
        thread_id: "thread_1".to_string(),
        routing_key: "discord:peer_1:thread_1".to_string(),
        generation: 3,
        created_at: now,
        updated_at: now,
        last_active_at: now,
        last_reset_at: Some(now),
        active_profile_projection: None,
    };
    store.upsert_session(&session).unwrap();

    let listed = store.list_sessions().unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.session_id, "sess_1");
    assert_eq!(got.kind, SessionKind::Group);
    assert_eq!(got.status, SessionStatus::Active);
    assert_eq!(got.channel, "discord");
    assert_eq!(got.account_id, "acct_1");
    assert_eq!(got.peer_id, "peer_1");
    assert_eq!(got.thread_id, "thread_1");
    assert_eq!(got.routing_key, "discord:peer_1:thread_1");
    assert_eq!(got.generation, 3);
    assert!(got.last_reset_at.is_some());
}

#[test]
fn capability_round_trips_through_sqlite() {
    let dir = temp_dir("capability");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();
    let capability = Capability {
        capability_id: "cap_1".to_string(),
        kind: "browser".to_string(),
        display_name: "Browser".to_string(),
        status: CapabilityStatus::Healthy,
        failure_count: 2,
        restart_count: 1,
        backoff_seconds: 30,
        next_restart_at: Some(now),
        last_restart_at: Some(now),
        last_heartbeat_at: Some(now),
        last_failure_reason: "timeout".to_string(),
        created_at: now,
        updated_at: now,
    };
    store.upsert_capability(&capability).unwrap();

    let listed = store.list_capabilities().unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.capability_id, "cap_1");
    assert_eq!(got.kind, "browser");
    assert_eq!(got.display_name, "Browser");
    assert_eq!(got.status, CapabilityStatus::Healthy);
    assert_eq!(got.failure_count, 2);
    assert_eq!(got.restart_count, 1);
    assert_eq!(got.backoff_seconds, 30);
    assert_eq!(got.last_failure_reason, "timeout");
    assert!(got.next_restart_at.is_some());
}
#[test]
fn a_dispatch_remembers_the_tools_it_was_given_and_asked_for() {
    // What the model could see is what the record has to show, or a turn that
    // called a tool cannot be explained from the text afterwards.
    let dir = temp_dir("llmtools");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();
    let tools = vec![kura_llm::ToolSpec {
        name: "loopforge_status".to_string(),
        description: "read project state".to_string(),
        parameters: serde_json::json!({"type": "object"}),
    }];
    let tool_calls = vec![kura_llm::ToolCall {
        call_id: "call_1".to_string(),
        name: "loopforge_status".to_string(),
        arguments: "{}".to_string(),
    }];
    let dispatch = Dispatch {
        tools: tools.clone(),
        tool_calls: tool_calls.clone(),
        dispatch_id: "disp_tools".to_string(),
        provider: "anthropic".to_string(),
        model: "m".to_string(),
        messages: vec![Message { role: MessageRole::User, content: "where am i".to_string(), ..Default::default() }],
        stream: false,
        status: DispatchStatus::Completed,
        output: String::new(),
        finish_reason: "stop".to_string(),
        usage: Usage::default(),
        error_code: String::new(),
        error: String::new(),
        timeout_ms: 30000,
        partial: false,
        max_retries: 0,
        attempt_count: 1,
        created_at: now,
        updated_at: now,
        started_at: Some(now),
        completed_at: Some(now),
    };
    store.upsert_llm_dispatch(&dispatch).unwrap();

    let listed = store.list_llm_dispatches().unwrap();
    let got = listed.iter().find(|d| d.dispatch_id == "disp_tools").unwrap();
    assert_eq!(got.tools, tools);
    assert_eq!(got.tool_calls, tool_calls);
}

#[test]
fn every_migration_survives_being_replayed() {
    // The legacy re-stamp resets a database to the baseline and applies every
    // post-baseline migration again, against a schema that already has them.
    // `CREATE TABLE`/`CREATE INDEX` say `IF NOT EXISTS`; SQLite has no
    // `ADD COLUMN IF NOT EXISTS`, so a migration adding one fails that replay
    // and the database stops opening. Adding the tool columns hit exactly this.
    let dir = temp_dir("replay");
    {
        let store = SQLiteStore::new(&dir).unwrap();
        assert_eq!(store.schema_version().unwrap(), kura_store::CURRENT_SCHEMA_VERSION);
        let conn = rusqlite::Connection::open(store.db_path()).unwrap();
        conn.execute("DELETE FROM schema_migrations WHERE version > 1", []).unwrap();
    }

    let store = SQLiteStore::new(&dir).expect("a replayed migration must not fail the open");
    assert_eq!(store.schema_version().unwrap(), kura_store::CURRENT_SCHEMA_VERSION);
}

#[test]
fn llm_dispatch_round_trips_through_sqlite() {
    let dir = temp_dir("llm");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();
    let dispatch = Dispatch {
        tools: Vec::new(),
        tool_calls: Vec::new(),
        dispatch_id: "disp_1".to_string(),
        provider: "openai".to_string(),
        model: "gpt-4o".to_string(),
        messages: vec![Message { role: MessageRole::User, content: "hi".to_string(), ..Default::default() }],
        stream: true,
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
    };
    store.upsert_llm_dispatch(&dispatch).unwrap();

    let listed = store.list_llm_dispatches().unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.dispatch_id, "disp_1");
    assert_eq!(got.provider, "openai");
    assert_eq!(got.model, "gpt-4o");
    assert_eq!(got.stream, true);
    assert_eq!(got.status, DispatchStatus::Completed);
    assert_eq!(got.output, "hello");
    assert_eq!(got.finish_reason, "stop");
    assert_eq!(got.messages.len(), 1);
    assert_eq!(got.messages[0].content, "hi");
    assert_eq!(got.usage.total_tokens, 4);
    assert_eq!(got.timeout_ms, 30000);
    assert!(got.started_at.is_some());
    assert!(got.completed_at.is_some());

    let fetched = store.get_llm_dispatch("disp_1").unwrap().expect("found");
    assert_eq!(fetched.dispatch_id, "disp_1");
    assert_eq!(store.get_llm_dispatch("missing").unwrap(), None);
}
#[test]
fn provider_check_and_auth_state_round_trip() {
    let dir = temp_dir("provider");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();

    let check = Check {
        check_id: "chk_1".to_string(),
        provider_id: "prov_1".to_string(),
        family: Family::OpenAICompatible,
        auth_mode: AuthMode::ApiKey,
        status: CheckStatus::Passed,
        model: "gpt-4o".to_string(),
        endpoint: "https://api.openai.com".to_string(),
        error_class: String::new(),
        error_code: String::new(),
        error_message: String::new(),
        usage: Usage { input_tokens: 5, output_tokens: 5, total_tokens: 10 },
        created_at: now,
        completed_at: now,
    };
    store.upsert_provider_check(&check).unwrap();
    let checks = store.list_provider_checks("prov_1").unwrap();
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].family, Family::OpenAICompatible);
    assert_eq!(checks[0].status, CheckStatus::Passed);
    assert_eq!(checks[0].usage.total_tokens, 10);
    assert_eq!(store.get_provider_check("prov_1", "chk_1").unwrap().unwrap().check_id, "chk_1");

    let mut metadata = std::collections::HashMap::new();
    metadata.insert("region".to_string(), "us-east-1".to_string());
    let state = AuthState {
        tenant_id: String::new(),
        provider_id: "prov_1".to_string(),
        family: Family::OpenAICompatible,
        auth_mode: AuthMode::ApiKey,
        status: AuthStatus::Authenticated,
        cli_path: String::new(),
        cli_available: true,
        account_label: "acct".to_string(),
        account_id: "acc_1".to_string(),
        plan: "pro".to_string(),
        auth_method: "key".to_string(),
        login_command: vec!["login".to_string()],
        logout_command: vec!["logout".to_string()],
        last_checked_at: now,
        last_authenticated_at: Some(now),
        last_error: String::new(),
        metadata,
        sandbox: Some(serde_json::json!({"session": "s"})),
    };
    store.upsert_provider_auth_state(&state).unwrap();
    let states = store.list_provider_auth_states().unwrap();
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].status, AuthStatus::Authenticated);
    assert_eq!(states[0].cli_available, true);
    assert_eq!(states[0].login_command, vec!["login".to_string()]);
    assert_eq!(states[0].metadata.get("region"), Some(&"us-east-1".to_string()));
    assert!(states[0].sandbox.is_some());
}

#[test]
fn provider_models_and_preference_round_trip() {
    let dir = temp_dir("provmodel");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();

    let model = Model {
        provider_id: "prov_1".to_string(),
        model_id: "gpt-4o".to_string(),
        display_name: "GPT-4o".to_string(),
        description: "flagship".to_string(),
        default: true,
        available: true,
        source: "managed".to_string(),
        chat: true,
        stream: true,
        coding: true,
        tool_use: true,
        reasoning_levels: vec!["low".to_string(), "high".to_string()],
    };
    store.replace_provider_models("prov_1", &[model]).unwrap();
    let models = store.list_provider_models().unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].model_id, "gpt-4o");
    assert_eq!(models[0].default, true);
    assert_eq!(models[0].tool_use, true);
    assert_eq!(models[0].reasoning_levels.len(), 2);
    assert_eq!(store.list_provider_models_by_provider("prov_1").unwrap().len(), 1);

    let preference = Preference { provider_id: "prov_1".to_string(), default_model: "gpt-4o".to_string(), updated_at: now };
    store.upsert_provider_preference(&preference).unwrap();
    let prefs = store.list_provider_preferences().unwrap();
    assert_eq!(prefs.len(), 1);
    assert_eq!(prefs[0].default_model, "gpt-4o");
}

#[test]
fn approval_and_decision_round_trip() {
    let dir = temp_dir("policy");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();

    let approval = Approval {
        approval_id: "apr_1".to_string(),
        action: "send_email".to_string(),
        resource_kind: "mail".to_string(),
        resource_id: "mail_1".to_string(),
        reason: "user requested".to_string(),
        requested_by: "user_1".to_string(),
        status: ApprovalStatus::Pending,
        created_at: now,
        updated_at: now,
        resolved_at: None,
        resolution: String::new(),
        comment: String::new(),
        sandbox: None,
        integration_bindings: vec![],
    };
    store.upsert_approval(&approval).unwrap();
    let approvals = store.list_approvals().unwrap();
    assert_eq!(approvals.len(), 1);
    assert_eq!(approvals[0].status, ApprovalStatus::Pending);
    assert_eq!(approvals[0].resource_kind, "mail");

    let decision = Decision {
        decision_id: "dec_1".to_string(),
        action: "send_email".to_string(),
        resource_kind: "mail".to_string(),
        resource_id: "mail_1".to_string(),
        outcome: DecisionOutcome::Allowed,
        reason: "policy allows".to_string(),
        approval_id: String::new(),
        created_at: now,
        sandbox: None,
    };
    store.upsert_decision(&decision).unwrap();
    let decisions = store.list_decisions().unwrap();
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].outcome, DecisionOutcome::Allowed);
}
#[test]
fn manager_document_round_trips_through_sqlite() {
    let dir = temp_dir("managdoc");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();
    let doc = kura_store::ManagerDocument {
        doc_kind: "triage".to_string(),
        doc_id: "t1".to_string(),
        environment_scope: "test".to_string(),
        tenant_id: "tenant_1".to_string(),
        document_json: "{\"a\":1}".to_string(),
        updated_at: now,
    };
    store.put_manager_document(&doc).unwrap();

    let listed = store.list_manager_documents("triage").unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].doc_id, "t1");
    assert_eq!(listed[0].environment_scope, "test");
    assert_eq!(listed[0].document_json, "{\"a\":1}");

    store.delete_manager_document("triage", "t1").unwrap();
    assert_eq!(store.list_manager_documents("triage").unwrap().len(), 0);
    assert_eq!(store.schema_version().unwrap(), kura_store::CURRENT_SCHEMA_VERSION);
}
#[test]
fn sandbox_execution_round_trips_through_sqlite() {
    let dir = temp_dir("sandboxexec");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();
    let record = kura_store::SandboxExecutionRecord {
        execution_id: "exec_1".to_string(),
        profile_id: "prof_1".to_string(),
        backend_kind: "docker".to_string(),
        status: "running".to_string(),
        approval_id: String::new(),
        requested_at: now,
        updated_at: now,
        started_at: Some(now),
        completed_at: None,
        document: "{\"kind\":\"exec\"}".to_string(),
    };
    store.upsert_sandbox_execution(&record).unwrap();
    let listed = store.list_sandbox_executions().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].execution_id, "exec_1");
    assert_eq!(listed[0].backend_kind, "docker");
    assert_eq!(listed[0].approval_id, "");
    assert_eq!(listed[0].document, "{\"kind\":\"exec\"}");
    assert!(listed[0].started_at.is_some());
    assert!(listed[0].completed_at.is_none());
}
#[test]
fn legacy_dev_head_database_is_restamped_as_baseline() {
    // A pre-release development database stamped at the legacy head (v55)
    // holds the exact baseline schema; reopening re-stamps it as baseline v1.
    let dir = temp_dir("migratev");
    {
        let store = SQLiteStore::new(&dir).unwrap();
        let conn = rusqlite::Connection::open(store.db_path()).unwrap();
        conn.execute("DELETE FROM schema_migrations", []).unwrap();
        conn.execute(
            "INSERT INTO schema_migrations (version, name, applied_at) VALUES (55, 'legacy_head', '2026-06-30T00:00:00Z')",
            [],
        )
        .unwrap();
    }
    let store = SQLiteStore::new(&dir).unwrap();
    // The re-stamp lands on baseline v1, then any post-baseline migrations
    // (v2+) apply on top.
    assert_eq!(store.schema_version().unwrap(), kura_store::CURRENT_SCHEMA_VERSION);
}
#[test]
fn event_append_and_list_round_trip() {
    let dir = temp_dir("events");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();
    let mut payload = serde_json::Map::new();
    payload.insert("k".to_string(), serde_json::json!("v"));
    let event = Event {
        event_id: "evt_1".to_string(),
        sequence: 0,
        environment_scope: "test".to_string(),
        tenant_id: String::new(),
        category: "audit".to_string(),
        name: "audit.cross_tenant_access_denied".to_string(),
        occurred_at: now,
        scope: Scope { run_id: "run_1".to_string(), ..Scope::default() },
        resource: Resource { kind: "run".to_string(), id: "run_1".to_string() },
        payload: payload.clone(),
    };
    let appended = store.append_event(&event).unwrap();
    assert!(appended.sequence > 0);

    let listed = store
        .list_events(&Filter {
            environment_scope: "test".to_string(),
            category: "audit".to_string(),
            ..Filter::default()
        })
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].event_id, "evt_1");
    assert_eq!(listed[0].name, "audit.cross_tenant_access_denied");
    assert_eq!(listed[0].scope.run_id, "run_1");
    assert_eq!(listed[0].resource.kind, "run");
    assert_eq!(listed[0].payload.get("k"), Some(&serde_json::json!("v")));
    assert_eq!(listed[0].sequence, appended.sequence);

    // Cursor filter: no rows after the last sequence.
    let after = store
        .list_events(&Filter { cursor: appended.sequence, ..Filter::default() })
        .unwrap();
    assert!(after.is_empty());
}
