//! Trait-surface tests for `kura_computeruse::Store` implemented by
//! `ComputerUseStoreHandle` (the Send + Sync newtype over the SQLite store).
//! The underlying DAOs are covered by tests/computeruse.rs; here we exercise
//! the exact trait methods a manager/route would call through the handle.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use kura_computeruse::{
    Action, ActionKind, ActionStatus, Artifact, ArtifactKind, ArtifactStatus, PageSummary,
    RiskLevel, Session, SessionStatus, Store, TargetMatchContext, TrustedPageScope,
};
use kura_runtime::{Run, RunStatus};
use kura_store::{ComputerUseStoreHandle, SQLiteStore};

fn temp_dir(name: &str) -> String {
    let dir =
        std::env::temp_dir().join(format!("kura_store_cu_trait_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().to_string()
}

/// The computer_use_sessions row references runs.run_id; seed the run first.
fn seed_run(store: &SQLiteStore) {
    let now = Utc::now();
    let run = Run {
        run_id: "run_cu".to_string(),
        session_id: String::new(),
        entrypoint: "computer use test".to_string(),
        status: RunStatus::Running,
        goal: "drive browser".to_string(),
        created_at: now,
        updated_at: now,
        ..Run::default()
    };
    store.upsert_run(&run).unwrap();
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
        last_action_id: "cuact_1".to_string(),
        started_at: now,
        updated_at: now,
        closed_at: None,
        interrupted_at: None,
        actions: Vec::new(),
    }
}

fn make_action(now: DateTime<Utc>, waiting_approval: bool) -> Action {
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
        status: if waiting_approval {
            ActionStatus::WaitingApproval
        } else {
            ActionStatus::Running
        },
        risk_level: RiskLevel::High,
        approval_id: if waiting_approval {
            "apr_1".to_string()
        } else {
            String::new()
        },
        target_match_context: Some(TargetMatchContext {
            match_strategy: "css".to_string(),
            expected_page_url: "https://example.com/page".to_string(),
            expected_selector: "#submit".to_string(),
            expected_text: "Submit".to_string(),
            trusted_scope_revision: 1,
            observed_page_url: String::new(),
            observed_selector_state: String::new(),
            match_result: None,
            evaluated_at: None,
        }),
        page_before: Some(PageSummary {
            url: "https://example.com/".to_string(),
            title: "Example".to_string(),
        }),
        page_after: None,
        failure_class: String::new(),
        failure_reason: String::new(),
        requested_at: now,
        updated_at: now,
        completed_at: None,
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
        kind: ArtifactKind::Screenshot,
        status: ArtifactStatus::Available,
        mime_type: "image/png".to_string(),
        file_name: "shot.png".to_string(),
        byte_size: 1024,
        storage_key: "cuart_1".to_string(),
        sha256: "abc123".to_string(),
        created_at: now,
        available_at: Some(now),
        capture_failure_reason: String::new(),
    }
}

#[test]
fn computer_use_store_trait_round_trip() {
    let dir = temp_dir("roundtrip");
    let store = SQLiteStore::new(&dir).unwrap();
    seed_run(&store);
    let handle = Arc::new(ComputerUseStoreHandle::new(store));
    let now = Utc::now();

    // Sessions through the trait.
    let session = make_session(now);
    handle.upsert_computer_use_session(&session).unwrap();
    let listed = handle.list_computer_use_sessions("test", "run_cu").unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].computer_use_session_id, "cusess_1");
    let got = handle
        .get_computer_use_session("test", "run_cu", "cusess_1")
        .unwrap()
        .expect("session present");
    assert_eq!(got.status, SessionStatus::Active);
    assert_eq!(got.trusted_page_scope.unwrap().scope_id, "scope_1");

    // Actions through the trait.
    let action = make_action(now, true);
    handle.upsert_computer_use_action(&action).unwrap();
    let actions = handle
        .list_computer_use_actions("test", "run_cu", "cusess_1")
        .unwrap();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].action_kind, ActionKind::Navigate);
    let got_action = handle
        .get_computer_use_action("test", "run_cu", "cusess_1", "cuact_1")
        .unwrap()
        .expect("action present");
    assert_eq!(got_action.status, ActionStatus::WaitingApproval);
    // Pending-by-approval lookup (used by policy approval reconciliation).
    let pending = handle
        .find_pending_computer_use_action_by_approval("test", "apr_1")
        .unwrap()
        .expect("pending action");
    assert_eq!(pending.computer_use_action_id, "cuact_1");

    // Artifacts through the trait.
    let artifact = make_artifact(now);
    handle.upsert_computer_use_artifact(&artifact).unwrap();
    let artifacts = handle
        .list_computer_use_artifacts_for_action("test", "run_cu", "cuact_1")
        .unwrap();
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].kind, ArtifactKind::Screenshot);
    let got_artifact = handle
        .get_computer_use_artifact("test", "cuart_1")
        .unwrap()
        .expect("artifact present");
    assert_eq!(got_artifact.sha256, "abc123");
}

#[test]
fn computer_use_store_trait_mark_in_flight_interrupted() {
    let dir = temp_dir("interrupt");
    let store = SQLiteStore::new(&dir).unwrap();
    seed_run(&store);
    let handle = Arc::new(ComputerUseStoreHandle::new(store));
    let now = Utc::now();

    let session = make_session(now);
    handle.upsert_computer_use_session(&session).unwrap();
    let mut closed = make_session(now);
    closed.computer_use_session_id = "cusess_closed".to_string();
    closed.status = SessionStatus::Closed;
    handle.upsert_computer_use_session(&closed).unwrap();

    let running = make_action(now, false);
    handle.upsert_computer_use_action(&running).unwrap();
    let mut done = make_action(now, false);
    done.computer_use_action_id = "cuact_done".to_string();
    done.status = ActionStatus::Completed;
    handle.upsert_computer_use_action(&done).unwrap();

    let interrupted_at = Utc::now();
    let (sessions, actions) = handle
        .mark_in_flight_computer_use_interrupted("test", interrupted_at)
        .unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].computer_use_session_id, "cusess_1");
    assert_eq!(sessions[0].status, SessionStatus::Interrupted);
    assert_eq!(sessions[0].interrupted_at, Some(interrupted_at));
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].computer_use_action_id, "cuact_1");
    assert_eq!(actions[0].status, ActionStatus::Interrupted);

    // Persisted, not just returned.
    let got = handle
        .get_computer_use_session("test", "run_cu", "cusess_1")
        .unwrap()
        .expect("session");
    assert_eq!(got.status, SessionStatus::Interrupted);
    let got_action = handle
        .get_computer_use_action("test", "run_cu", "cusess_1", "cuact_1")
        .unwrap()
        .expect("action");
    assert_eq!(got_action.status, ActionStatus::Interrupted);
}
