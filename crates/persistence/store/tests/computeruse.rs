use chrono::{DateTime, Utc};
use kura_computeruse::{
    Action, ActionKind, ActionStatus, Artifact, ArtifactKind, ArtifactStatus, MatchResult,
    PageSummary, RiskLevel, Session, SessionStatus, TargetMatchContext, TrustedPageScope,
};
use kura_runtime::{Run, RunStatus};
use kura_store::SQLiteStore;

fn temp_dir(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("kura_store_cu_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().to_string()
}

fn make_run() -> Run {
    let now = Utc::now();
    Run {
        run_id: "run_cu".to_string(),
        session_id: String::new(),
        entrypoint: "computer use test".to_string(),
        status: RunStatus::Running,
        goal: "drive browser".to_string(),
        created_at: now,
        updated_at: now,
        ..Run::default()
    }
}

fn make_session(now: DateTime<Utc>) -> Session {
    Session {
        computer_use_session_id: "cusess_1".to_string(),
        environment_scope: "test".to_string(),
        run_id: "run_cu".to_string(),
        workflow_id: "wf_1".to_string(),
        workflow_step_id: "wfs_1".to_string(),
        status: SessionStatus::Active,
        driver_kind: "browser".to_string(),
        trusted_page_scope: Some(TrustedPageScope {
            scope_id: "scope_1".to_string(),
            computer_use_session_id: "cusess_1".to_string(),
            origin: "https://example.com".to_string(),
            page_url: "https://example.com/".to_string(),
            title: "Example".to_string(),
            scope_revision: 1,
            derived_from_action_id: String::new(),
            created_at: now,
        }),
        current_page: Some(PageSummary {
            url: "https://example.com/".to_string(),
            title: "Example".to_string(),
        }),
        last_action_id: String::new(),
        started_at: now,
        updated_at: now,
        closed_at: None,
        interrupted_at: None,
        actions: Vec::new(),
    }
}

fn make_action(now: DateTime<Utc>) -> Action {
    let mut input = serde_json::Map::new();
    input.insert(
        "url".to_string(),
        serde_json::json!("https://example.com/page"),
    );
    Action {
        computer_use_action_id: "cuact_1".to_string(),
        environment_scope: "test".to_string(),
        computer_use_session_id: "cusess_1".to_string(),
        run_id: "run_cu".to_string(),
        step_id: "step_1".to_string(),
        tool_call_id: "tc_1".to_string(),
        workflow_id: "wf_1".to_string(),
        workflow_step_id: "wfs_1".to_string(),
        action_kind: ActionKind::Navigate,
        status: ActionStatus::Completed,
        risk_level: RiskLevel::Low,
        approval_id: "apr_1".to_string(),
        target_match_context: Some(TargetMatchContext {
            match_strategy: "css".to_string(),
            expected_page_url: "https://example.com/page".to_string(),
            expected_selector: "#submit".to_string(),
            expected_text: "Go".to_string(),
            trusted_scope_revision: 1,
            observed_page_url: "https://example.com/page".to_string(),
            observed_selector_state: String::new(),
            match_result: Some(MatchResult::Matched),
            evaluated_at: Some(now),
        }),
        page_before: None,
        page_after: Some(PageSummary {
            url: "https://example.com/page".to_string(),
            title: "Page".to_string(),
        }),
        failure_class: String::new(),
        failure_reason: String::new(),
        requested_at: now,
        updated_at: now,
        completed_at: Some(now),
        input,
        artifacts: Vec::new(),
    }
}

fn make_artifact(now: DateTime<Utc>) -> Artifact {
    Artifact {
        artifact_id: "cuart_1".to_string(),
        environment_scope: "test".to_string(),
        computer_use_session_id: "cusess_1".to_string(),
        computer_use_action_id: "cuact_1".to_string(),
        run_id: "run_cu".to_string(),
        kind: ArtifactKind::PageSnapshot,
        status: ArtifactStatus::Available,
        mime_type: "application/json".to_string(),
        file_name: "snapshot.json".to_string(),
        byte_size: 128,
        storage_key: "s3://bucket/snap.json".to_string(),
        sha256: "abc123".to_string(),
        created_at: now,
        available_at: Some(now),
        capture_failure_reason: String::new(),
    }
}

#[test]
fn computer_use_session_round_trips_through_sqlite() {
    let dir = temp_dir("session");
    let store = SQLiteStore::new(&dir).unwrap();
    store.upsert_run(&make_run()).unwrap();
    let now = Utc::now();
    let session = make_session(now);
    store.upsert_computer_use_session(&session).unwrap();

    let listed = store.list_computer_use_sessions("test", "run_cu").unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.computer_use_session_id, "cusess_1");
    assert_eq!(got.environment_scope, "test");
    assert_eq!(got.run_id, "run_cu");
    assert_eq!(got.workflow_id, "wf_1");
    assert_eq!(got.workflow_step_id, "wfs_1");
    assert_eq!(got.status, SessionStatus::Active);
    assert_eq!(got.driver_kind, "browser");
    assert!(got.trusted_page_scope.is_some());
    let scope = got.trusted_page_scope.as_ref().unwrap();
    assert_eq!(scope.origin, "https://example.com");
    assert_eq!(scope.scope_revision, 1);
    assert!(got.current_page.is_some());
    assert_eq!(
        got.current_page.as_ref().unwrap().url,
        "https://example.com/"
    );

    let fetched = store
        .get_computer_use_session("test", "run_cu", "cusess_1")
        .unwrap()
        .expect("found");
    assert_eq!(fetched.computer_use_session_id, "cusess_1");
    assert_eq!(fetched.status, SessionStatus::Active);
    assert_eq!(
        store
            .get_computer_use_session("test", "run_cu", "missing")
            .unwrap(),
        None
    );

    // Upserting the same id overwrites the row instead of duplicating it.
    let mut updated = session;
    updated.status = SessionStatus::Blocked;
    store.upsert_computer_use_session(&updated).unwrap();
    let listed = store.list_computer_use_sessions("test", "run_cu").unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].status, SessionStatus::Blocked);
}

#[test]
fn computer_use_action_round_trips_through_sqlite() {
    let dir = temp_dir("action");
    let store = SQLiteStore::new(&dir).unwrap();
    store.upsert_run(&make_run()).unwrap();
    let now = Utc::now();
    store
        .upsert_computer_use_session(&make_session(now))
        .unwrap();
    let action = make_action(now);
    store.upsert_computer_use_action(&action).unwrap();

    let listed = store
        .list_computer_use_actions("test", "run_cu", "cusess_1")
        .unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.computer_use_action_id, "cuact_1");
    assert_eq!(got.computer_use_session_id, "cusess_1");
    assert_eq!(got.step_id, "step_1");
    assert_eq!(got.tool_call_id, "tc_1");
    assert_eq!(got.action_kind, ActionKind::Navigate);
    assert_eq!(got.status, ActionStatus::Completed);
    assert_eq!(got.risk_level, RiskLevel::Low);
    assert_eq!(got.approval_id, "apr_1");
    assert_eq!(
        got.input.get("url"),
        Some(&serde_json::json!("https://example.com/page"))
    );
    assert!(got.target_match_context.is_some());
    assert_eq!(
        got.target_match_context.as_ref().unwrap().match_result,
        Some(MatchResult::Matched)
    );
    assert!(got.page_after.is_some());
    assert_eq!(
        got.page_after.as_ref().unwrap().url,
        "https://example.com/page"
    );
    assert_eq!(got.completed_at, Some(now));

    let fetched = store
        .get_computer_use_action("test", "run_cu", "cusess_1", "cuact_1")
        .unwrap()
        .expect("found");
    assert_eq!(fetched.computer_use_action_id, "cuact_1");
    assert_eq!(fetched.status, ActionStatus::Completed);
    assert_eq!(
        store
            .get_computer_use_action("test", "run_cu", "cusess_1", "missing")
            .unwrap(),
        None
    );
}

#[test]
fn computer_use_artifact_round_trips_through_sqlite() {
    let dir = temp_dir("artifact");
    let store = SQLiteStore::new(&dir).unwrap();
    store.upsert_run(&make_run()).unwrap();
    let now = Utc::now();
    store
        .upsert_computer_use_session(&make_session(now))
        .unwrap();
    store.upsert_computer_use_action(&make_action(now)).unwrap();
    let artifact = make_artifact(now);
    store.upsert_computer_use_artifact(&artifact).unwrap();

    let listed = store
        .list_computer_use_artifacts_for_action("test", "run_cu", "cuact_1")
        .unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.artifact_id, "cuart_1");
    assert_eq!(got.computer_use_session_id, "cusess_1");
    assert_eq!(got.computer_use_action_id, "cuact_1");
    assert_eq!(got.kind, ArtifactKind::PageSnapshot);
    assert_eq!(got.status, ArtifactStatus::Available);
    assert_eq!(got.mime_type, "application/json");
    assert_eq!(got.file_name, "snapshot.json");
    assert_eq!(got.byte_size, 128);
    assert_eq!(got.storage_key, "s3://bucket/snap.json");
    assert_eq!(got.sha256, "abc123");
    assert_eq!(got.available_at, Some(now));

    let fetched = store
        .get_computer_use_artifact("test", "cuart_1")
        .unwrap()
        .expect("found");
    assert_eq!(fetched.artifact_id, "cuart_1");
    assert_eq!(fetched.kind, ArtifactKind::PageSnapshot);
    assert_eq!(
        store.get_computer_use_artifact("test", "missing").unwrap(),
        None
    );
}

#[test]
fn find_pending_computer_use_action_by_approval_returns_waiting_action() {
    let dir = temp_dir("pending");
    let store = SQLiteStore::new(&dir).unwrap();
    store.upsert_run(&make_run()).unwrap();
    let now = Utc::now();
    store
        .upsert_computer_use_session(&make_session(now))
        .unwrap();

    let mut waiting = make_action(now);
    waiting.computer_use_action_id = "cuact_pending".to_string();
    waiting.status = ActionStatus::WaitingApproval;
    waiting.approval_id = "apr_123".to_string();
    store.upsert_computer_use_action(&waiting).unwrap();

    let mut done = make_action(now);
    done.computer_use_action_id = "cuact_done".to_string();
    done.status = ActionStatus::Completed;
    done.approval_id = "apr_123".to_string();
    store.upsert_computer_use_action(&done).unwrap();

    let found = store
        .find_pending_computer_use_action_by_approval("test", "apr_123")
        .unwrap()
        .expect("pending action");
    assert_eq!(found.computer_use_action_id, "cuact_pending");
    assert_eq!(found.status, ActionStatus::WaitingApproval);
    assert_eq!(found.approval_id, "apr_123");
    assert_eq!(
        store
            .find_pending_computer_use_action_by_approval("test", "missing")
            .unwrap(),
        None
    );
}

#[test]
fn mark_inflight_computer_use_interrupted_marks_sessions_and_actions() {
    let dir = temp_dir("inflight");
    let store = SQLiteStore::new(&dir).unwrap();
    store.upsert_run(&make_run()).unwrap();
    let now = Utc::now();

    let mut active = make_session(now);
    active.status = SessionStatus::Active;
    store.upsert_computer_use_session(&active).unwrap();

    let mut closed = make_session(now);
    closed.computer_use_session_id = "cusess_closed".to_string();
    closed.status = SessionStatus::Closed;
    store.upsert_computer_use_session(&closed).unwrap();

    let mut running = make_action(now);
    running.status = ActionStatus::Running;
    running.approval_id = String::new();
    store.upsert_computer_use_action(&running).unwrap();

    let mut done = make_action(now);
    done.computer_use_action_id = "cuact_done".to_string();
    done.status = ActionStatus::Completed;
    done.approval_id = String::new();
    store.upsert_computer_use_action(&done).unwrap();

    let interrupted_at = Utc::now();
    let (sessions, actions) = store
        .mark_inflight_computer_use_interrupted("test", &interrupted_at)
        .unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].computer_use_session_id, "cusess_1");
    assert_eq!(sessions[0].status, SessionStatus::Interrupted);
    assert_eq!(sessions[0].interrupted_at, Some(interrupted_at));
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].computer_use_action_id, "cuact_1");
    assert_eq!(actions[0].status, ActionStatus::Interrupted);
    assert_eq!(actions[0].failure_class, "interrupted");
    assert_eq!(
        actions[0].failure_reason,
        "daemon restarted before computer-use action completed"
    );
    assert_eq!(actions[0].completed_at, Some(interrupted_at));

    // The interruption is persisted, not just returned in memory.
    let got_session = store
        .get_computer_use_session("test", "run_cu", "cusess_1")
        .unwrap()
        .expect("session");
    assert_eq!(got_session.status, SessionStatus::Interrupted);
    assert_eq!(got_session.interrupted_at, Some(interrupted_at));
    let got_action = store
        .get_computer_use_action("test", "run_cu", "cusess_1", "cuact_1")
        .unwrap()
        .expect("action");
    assert_eq!(got_action.status, ActionStatus::Interrupted);
    assert_eq!(got_action.failure_class, "interrupted");
    assert_eq!(got_action.completed_at, Some(interrupted_at));
}
